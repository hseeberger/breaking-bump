use crate::{
    application::{Cargo, Snapshot},
    domain::{
        Undecided,
        package::{Dependency, Location, Package, Workspace},
        reference::{ApiItems, DefiningPackage},
    },
    infrastructure::rustdoc,
};
use anyhow::{Context, bail};
use cargo_metadata::{CargoOpt, DependencyKind, Message, Metadata, MetadataCommand, TargetKind};
use rustdoc_types::Crate;
use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    env,
    ffi::OsString,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const STD_CRATES: [&str; 5] = ["std", "core", "alloc", "proc_macro", "test"];

pub(crate) struct CargoCli {
    target_dir: PathBuf,
}

impl CargoCli {
    pub(crate) fn new(target_dir: PathBuf) -> Self {
        Self { target_dir }
    }
}

impl Cargo for CargoCli {
    type Snapshot = CargoSnapshot;

    fn workspace(&self, directory: &Path) -> anyhow::Result<Workspace> {
        let metadata = metadata(directory, true)?;
        Ok(Workspace {
            members: members(&metadata)?,
            metadata: metadata.workspace_metadata.clone(),
        })
    }

    fn snapshot(&self, directory: &Path) -> anyhow::Result<CargoSnapshot> {
        let metadata = metadata(directory, false)?;
        let resolve = metadata
            .resolve
            .as_ref()
            .context("cargo metadata returned no resolve graph")?;
        let packages = metadata
            .packages
            .iter()
            .map(|package| {
                let summary = Summary {
                    name: package.name.to_string(),
                    version: package.version.clone(),
                };
                (package.id.repr.clone(), summary)
            })
            .collect();
        let normal_dependencies = resolve
            .nodes
            .iter()
            .map(|node| {
                let dependencies = node
                    .deps
                    .iter()
                    .filter(|dependency| {
                        dependency
                            .dep_kinds
                            .iter()
                            .any(|kind| kind.kind == DependencyKind::Normal)
                    })
                    .map(|dependency| dependency.pkg.repr.clone())
                    .collect();
                (node.id.repr.clone(), dependencies)
            })
            .collect();
        let libraries = members(&metadata)?
            .into_iter()
            .filter(Package::is_published_library)
            .collect();
        Ok(CargoSnapshot {
            directory: directory.to_path_buf(),
            target_dir: self.target_dir.clone(),
            sysroot: sysroot()?,
            packages,
            normal_dependencies,
            libraries,
        })
    }
}

pub(crate) struct CargoSnapshot {
    directory: PathBuf,
    target_dir: PathBuf,
    sysroot: PathBuf,
    packages: HashMap<String, Summary>,
    normal_dependencies: HashMap<String, Vec<String>>,
    libraries: Vec<Package>,
}

impl Snapshot for CargoSnapshot {
    fn libraries(&self) -> &[Package] {
        &self.libraries
    }

    fn api_items(&self, library: &Package) -> anyhow::Result<ApiItems> {
        let (doc, artifacts) = self.rustdoc(library)?;
        let sites = rustdoc::crate_grouped_references(&doc)?;
        let defining = defining_packages(
            &doc,
            sites.keys().copied().collect(),
            &artifacts,
            &self.packages,
            &self.sysroot,
            &library.name,
        )?;
        let mut items = ApiItems::new();
        for (crate_id, references) in sites {
            let Some(package) = defining.get(&crate_id) else {
                continue;
            };
            for reference in references {
                items.entry(reference).or_default().insert(package.clone());
            }
        }
        Ok(items)
    }

    fn package_grouped_foreign_paths(
        &self,
        library: &Package,
    ) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
        let (doc, artifacts) = self.rustdoc(library)?;
        let references = rustdoc::foreign_references(&doc)?;
        let defining = defining_packages(
            &doc,
            references
                .iter()
                .map(|reference| reference.crate_id)
                .collect(),
            &artifacts,
            &self.packages,
            &self.sysroot,
            &library.name,
        )?;
        let mut items = BTreeMap::<String, BTreeSet<String>>::new();
        for reference in references {
            if let Some(package) = defining.get(&reference.crate_id) {
                items
                    .entry(package.id.clone())
                    .or_default()
                    .insert(reference.path);
            }
        }
        Ok(items)
    }

    fn direct_dependencies(&self, package_id: &str) -> Vec<String> {
        self.normal_dependencies
            .get(package_id)
            .cloned()
            .unwrap_or_default()
    }

    fn distances(&self, package_id: &str) -> BTreeMap<String, usize> {
        let mut distances = BTreeMap::from([(package_id.to_owned(), 0)]);
        let mut queue = VecDeque::from([package_id.to_owned()]);
        while let Some(current) = queue.pop_front() {
            let distance = distances[&current];
            for dependency in self.direct_dependencies(&current) {
                if !distances.contains_key(&dependency) {
                    distances.insert(dependency.clone(), distance + 1);
                    queue.push_back(dependency);
                }
            }
        }
        distances
    }

    fn package_name(&self, package_id: &str) -> &str {
        self.packages
            .get(package_id)
            .map(|summary| summary.name.as_str())
            .unwrap_or("?")
    }

    fn package_version(&self, package_id: &str) -> String {
        self.packages
            .get(package_id)
            .map(|summary| summary.version.to_string())
            .unwrap_or_default()
    }
}

impl CargoSnapshot {
    fn rustdoc(&self, library: &Package) -> anyhow::Result<(Crate, HashMap<PathBuf, String>)> {
        let Some(crate_name) = &library.library else {
            bail!("{} has no library", library.name);
        };
        let mut child = Command::new(cargo())
            .arg("rustdoc")
            .args(["--quiet", "--message-format", "json", "--manifest-path"])
            .arg(self.directory.join("Cargo.toml"))
            .arg("--package")
            .arg(format!("{}@{}", library.name, library.version))
            .args(["--lib", "--all-features", "--target-dir"])
            .arg(&self.target_dir)
            .args(["--", "-Zunstable-options", "--output-format", "json"])
            .current_dir(&self.directory)
            .stdout(Stdio::piped())
            .spawn()
            .context("cannot run cargo rustdoc")?;
        let stdout = child.stdout.take().context("cargo rustdoc has no stdout")?;
        let mut artifacts = HashMap::new();
        for message in Message::parse_stream(BufReader::new(stdout)) {
            if let Message::CompilerArtifact(artifact) = message? {
                for file in artifact.filenames {
                    if let Ok(file) = fs::canonicalize(&file) {
                        artifacts.insert(file, artifact.package_id.repr.clone());
                    }
                }
            }
        }
        let status = child.wait()?;
        if !status.success() {
            bail!("cargo rustdoc failed for {} ({status})", library.name);
        }
        let doc = rustdoc::read(
            &self
                .target_dir
                .join("doc")
                .join(format!("{crate_name}.json")),
        )?;
        Ok((doc, artifacts))
    }
}

struct Summary {
    name: String,
    version: Version,
}

fn defining_packages(
    doc: &Crate,
    crate_ids: BTreeSet<u32>,
    artifacts: &HashMap<PathBuf, String>,
    packages: &HashMap<String, Summary>,
    sysroot: &Path,
    library: &str,
) -> anyhow::Result<BTreeMap<u32, DefiningPackage>> {
    let mut defining = BTreeMap::new();
    for crate_id in crate_ids {
        let Some(external) = doc.external_crates.get(&crate_id) else {
            return Err(Undecided(format!(
                "{library}: rustdoc refers to the unknown crate id {crate_id}"
            ))
            .into());
        };
        if STD_CRATES.contains(&external.name.as_str()) {
            continue;
        }
        let path = fs::canonicalize(&external.path).ok();
        if path.as_ref().is_some_and(|path| path.starts_with(sysroot)) {
            continue;
        }
        let Some((id, summary)) = path
            .and_then(|path| artifacts.get(&path))
            .and_then(|id| packages.get(id).map(|summary| (id, summary)))
        else {
            return Err(Undecided(format!(
                "{library}: cannot map crate {} ({}) to a package",
                external.name,
                external.path.display()
            ))
            .into());
        };
        defining.insert(
            crate_id,
            DefiningPackage {
                name: summary.name.clone(),
                version: summary.version.clone(),
                id: id.clone(),
            },
        );
    }
    Ok(defining)
}

fn metadata(directory: &Path, no_deps: bool) -> anyhow::Result<Metadata> {
    let mut command = MetadataCommand::new();
    command
        .cargo_path(cargo())
        .manifest_path(directory.join("Cargo.toml"))
        .current_dir(directory);
    if no_deps {
        command.no_deps();
    } else {
        command.features(CargoOpt::AllFeatures);
    }
    command
        .exec()
        .with_context(|| format!("cargo metadata failed in {}", directory.display()))
}

fn members(metadata: &Metadata) -> anyhow::Result<Vec<Package>> {
    metadata
        .workspace_packages()
        .into_iter()
        .map(package)
        .collect()
}

fn package(package: &cargo_metadata::Package) -> anyhow::Result<Package> {
    let manifest = package.manifest_path.clone().into_std_path_buf();
    let inherited = inherited_keys(&manifest)?;
    let dependencies = package
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind == DependencyKind::Normal)
        .map(|dependency| {
            let key = dependency.rename.as_ref().unwrap_or(&dependency.name);
            Dependency {
                name: dependency.name.clone(),
                requirement: dependency.req.clone(),
                location: if inherited.contains(key) {
                    Location::Inherited
                } else {
                    Location::Own
                },
            }
        })
        .collect();
    let library = package
        .targets
        .iter()
        .find(|target| {
            target.kind.iter().any(|kind| {
                matches!(
                    kind,
                    TargetKind::Lib | TargetKind::RLib | TargetKind::DyLib | TargetKind::ProcMacro
                )
            })
        })
        .map(|target| target.name.replace('-', "_"));
    Ok(Package {
        id: package.id.repr.clone(),
        name: package.name.to_string(),
        version: package.version.clone(),
        library,
        published: package
            .publish
            .as_ref()
            .is_none_or(|registries| !registries.is_empty()),
        manifest,
        dependencies,
    })
}

fn inherited_keys(manifest: &Path) -> anyhow::Result<BTreeSet<String>> {
    let text = fs::read_to_string(manifest)
        .with_context(|| format!("cannot read {}", manifest.display()))?;
    let manifest = text
        .parse::<toml::Table>()
        .with_context(|| format!("cannot parse {}", manifest.display()))?;
    let targets = manifest
        .get("target")
        .and_then(|targets| targets.as_table())
        .into_iter()
        .flat_map(|targets| targets.values())
        .filter_map(|target| target.get("dependencies"));
    let tables = manifest
        .get("dependencies")
        .into_iter()
        .chain(targets)
        .filter_map(|table| table.as_table());
    let mut keys = BTreeSet::new();
    for table in tables {
        for (key, spec) in table {
            let inherited = spec
                .get("workspace")
                .and_then(|workspace| workspace.as_bool())
                .unwrap_or_default();
            if inherited {
                keys.insert(key.clone());
            }
        }
    }
    Ok(keys)
}

fn sysroot() -> anyhow::Result<PathBuf> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .context("cannot run rustc")?;
    if !output.status.success() {
        bail!(
            "rustc --print sysroot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let sysroot = String::from_utf8(output.stdout)?;
    fs::canonicalize(sysroot.trim()).context("cannot resolve the sysroot")
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

#[cfg(test)]
mod tests {
    use crate::{
        domain::Undecided,
        infrastructure::{cargo::defining_packages, rustdoc::tests::doc},
    };
    use std::{
        collections::{BTreeSet, HashMap},
        path::Path,
    };

    #[test]
    fn an_unmapped_crate_is_undecided() {
        let error = defining_packages(
            &doc(9, 10),
            BTreeSet::from([1]),
            &HashMap::new(),
            &HashMap::new(),
            Path::new("/sysroot"),
            "subject",
        )
        .unwrap_err();
        assert!(error.is::<Undecided>());
    }

    #[test]
    fn an_unknown_crate_id_is_undecided() {
        let error = defining_packages(
            &doc(9, 10),
            BTreeSet::from([3]),
            &HashMap::new(),
            &HashMap::new(),
            Path::new("/sysroot"),
            "subject",
        )
        .unwrap_err();
        assert!(error.is::<Undecided>());
    }
}
