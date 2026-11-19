use crate::{
    application::{Cargo, Snapshot, default_target_dir},
    domain::{
        Undecided,
        bump::{Bump, incompatible_bumps},
        changes::{Change, changes},
        finding::{Finding, Violation},
        marker::is_marked,
        package::{Package, is_manifest},
        reference::DefiningPackage,
        rules::violations,
        settings::Settings,
    },
};
use anyhow::{Context, anyhow};
use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) trait Repository {
    type Worktree: Worktree;

    fn root(&self) -> &Path;

    fn commit(&self, revision: &str) -> anyhow::Result<String>;

    fn default_base(&self) -> anyhow::Result<Option<String>>;

    fn merge_base(&self, base: &str, head: &str) -> anyhow::Result<String>;

    fn changed_paths(&self, from: &str, to: &str) -> anyhow::Result<Vec<String>>;

    fn dirty_manifests(&self) -> anyhow::Result<Vec<String>>;

    fn worktree(&self, commit: &str) -> anyhow::Result<Self::Worktree>;

    // `path` is relative to the repository root; history is not simplified.
    fn messages(&self, from: &str, to: &str, path: &Path) -> anyhow::Result<Vec<String>>;
}

pub(crate) trait Worktree {
    fn root(&self) -> &Path;

    fn directory(&self) -> &Path;
}

#[derive(Debug)]
pub(crate) struct Report {
    pub(crate) head: Option<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) outcome: Outcome,
}

impl Report {
    pub(crate) fn error(error: anyhow::Error) -> Self {
        Self {
            head: None,
            warnings: vec![],
            outcome: Outcome::Error(error),
        }
    }

    pub(crate) fn violations(&self) -> BTreeSet<Violation> {
        match &self.outcome {
            Outcome::Breaking(findings) => findings
                .iter()
                .flat_map(|finding| finding.violations.iter().copied())
                .collect(),
            _ => BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Compatible(String),
    Breaking(Vec<JudgedFinding>),
    Undecided(String),
    Error(anyhow::Error),
}

impl Outcome {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Outcome::Compatible(_) => "compatible",
            Outcome::Breaking(_) => "breaking",
            Outcome::Undecided(_) => "undecided",
            Outcome::Error(_) => "error",
        }
    }
}

#[derive(Debug)]
pub(crate) struct JudgedFinding {
    pub(crate) finding: Finding,
    pub(crate) violations: Vec<Violation>,
}

pub(crate) fn check<R, C>(
    repository: &R,
    cargo: &C,
    base: Option<&str>,
    head: &str,
    target_dir: Option<&Path>,
) -> Report
where
    R: Repository,
    C: Cargo,
{
    let head = match repository.commit(head) {
        Ok(head) => head,
        Err(error) => return Report::error(error),
    };
    let mut warnings = Vec::new();
    let outcome = match compare(repository, cargo, base, &head, target_dir, &mut warnings) {
        Ok(outcome) => outcome,
        Err(error) => match error.downcast::<Undecided>() {
            Ok(undecided) => Outcome::Undecided(undecided.0),
            Err(error) => Outcome::Error(error),
        },
    };
    Report {
        head: Some(head),
        warnings,
        outcome,
    }
}

fn compare<R, C>(
    repository: &R,
    cargo: &C,
    base: Option<&str>,
    head: &str,
    target_dir: Option<&Path>,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Outcome>
where
    R: Repository,
    C: Cargo,
{
    let base = match base {
        Some(base) => base.to_owned(),
        None => repository
            .default_base()?
            .ok_or_else(|| anyhow!("origin/HEAD is not set; pass the base commit with --base"))?,
    };
    let merge_base = repository.merge_base(&base, head)?;

    for manifest in repository.dirty_manifests()? {
        warnings.push(format!(
            "{manifest} has uncommitted changes, which check does not see"
        ));
    }

    let changed_manifest = repository
        .changed_paths(&merge_base, head)?
        .iter()
        .any(|path| is_manifest(Path::new(path)));
    if !changed_manifest {
        return Ok(Outcome::Compatible("no Cargo.toml changed".to_owned()));
    }

    let old_tree = repository.worktree(&merge_base)?;
    let new_tree = repository.worktree(head)?;
    let new_workspace = cargo.workspace(new_tree.directory())?;
    let workspace_path = new_workspace
        .root
        .strip_prefix(new_tree.root())
        .with_context(|| {
            format!(
                "workspace root {} is outside of the worktree",
                new_workspace.root.display()
            )
        })?;
    let old_workspace = cargo.workspace(&old_tree.root().join(workspace_path))?;
    let settings = Settings::from_workspace_metadata(&old_workspace.metadata)?;
    let bumps = incompatible_bumps(&old_workspace, &new_workspace)?;
    if bumps.is_empty() {
        return Ok(Outcome::Compatible(
            "no incompatible dependency bump".to_owned(),
        ));
    }

    let target_dir = match target_dir {
        Some(target_dir) => target_dir.to_path_buf(),
        None => default_target_dir(&repository.root().join(workspace_path)),
    };
    let old = cargo.snapshot(&old_workspace.root, &target_dir)?;
    let new = cargo.snapshot(&new_workspace.root, &target_dir)?;
    let findings = findings(&old, &new, &bumps, &new_workspace.root)?;
    if findings.is_empty() {
        return Ok(Outcome::Compatible(
            "no public API item changed to an incompatible version".to_owned(),
        ));
    }

    let findings = judged_findings(
        repository,
        &settings,
        &merge_base,
        head,
        workspace_path,
        findings,
    )?;
    Ok(Outcome::Breaking(findings))
}

fn findings<S>(
    old: &S,
    new: &S,
    bumps: &[Bump],
    workspace_root: &Path,
) -> anyhow::Result<Vec<Finding>>
where
    S: Snapshot,
{
    let names = bumps
        .iter()
        .map(|bump| bump.dependent.as_str())
        .collect::<BTreeSet<_>>();
    let mut findings = Vec::new();
    for name in names {
        let (Some(old_library), Some(new_library)) = (library(old, name), library(new, name))
        else {
            continue;
        };
        let changes = changes(&old.api_items(old_library)?, &new.api_items(new_library)?);
        let bumps = bumps
            .iter()
            .filter(|bump| bump.dependent == name)
            .collect::<Vec<_>>();
        let manifest = new_library
            .manifest
            .strip_prefix(workspace_root)
            .with_context(|| {
                format!(
                    "manifest {} is outside of the workspace",
                    new_library.manifest.display()
                )
            })?
            .to_path_buf();
        let mut partial_findings = BTreeMap::<(&str, &str), PartialFinding>::new();
        for change in &changes {
            let culprits = culprits(old, new, old_library, new_library, &bumps, change);
            if culprits.is_empty() {
                return Err(Undecided(format!(
                    "{name}: cannot attribute the change of {} at {} to a bumped dependency",
                    change.package, change.reference
                ))
                .into());
            }
            for bump in culprits {
                let partial_finding = partial_findings
                    .entry((bump.dependency.as_str(), change.package.as_str()))
                    .or_insert_with(|| PartialFinding {
                        bump,
                        old_versions: BTreeSet::new(),
                        new_versions: BTreeSet::new(),
                        items: Vec::new(),
                    });
                partial_finding.old_versions.extend(versions(&change.old));
                partial_finding.new_versions.extend(versions(&change.new));
                partial_finding.items.push(change.reference.to_string());
            }
        }
        for ((_, package), partial_finding) in partial_findings {
            findings.push(Finding {
                bump: partial_finding.bump.clone(),
                package: package.to_owned(),
                old_versions: partial_finding.old_versions,
                new_versions: partial_finding.new_versions,
                items: partial_finding.items,
                location: new_library.location(&partial_finding.bump.dependency),
                manifest: manifest.clone(),
            });
        }
    }
    Ok(findings)
}

struct PartialFinding<'a> {
    bump: &'a Bump,
    old_versions: BTreeSet<Version>,
    new_versions: BTreeSet<Version>,
    items: Vec<String>,
}

fn library<'a, S>(snapshot: &'a S, name: &str) -> Option<&'a Package>
where
    S: Snapshot,
{
    snapshot
        .libraries()
        .iter()
        .find(|library| library.name == name)
}

fn culprits<'a, S>(
    old: &S,
    new: &S,
    old_library: &Package,
    new_library: &Package,
    bumps: &[&'a Bump],
    change: &Change,
) -> Vec<&'a Bump>
where
    S: Snapshot,
{
    let reaches_new = bumps
        .iter()
        .copied()
        .filter(|bump| reaches(new, new_library, &bump.dependency, &change.new))
        .collect::<Vec<_>>();
    let reaches_both = reaches_new
        .iter()
        .copied()
        .filter(|bump| reaches(old, old_library, &bump.dependency, &change.old))
        .collect::<Vec<_>>();
    if reaches_both.is_empty() {
        reaches_new
    } else {
        reaches_both
    }
}

fn reaches<S>(
    snapshot: &S,
    library: &Package,
    dependency: &str,
    packages: &BTreeSet<DefiningPackage>,
) -> bool
where
    S: Snapshot,
{
    snapshot
        .direct_dependencies(&library.id)
        .iter()
        .filter(|id| snapshot.package_name(id) == dependency)
        .any(|id| {
            let distances = snapshot.distances(id);
            packages
                .iter()
                .any(|package| distances.contains_key(&package.id))
        })
}

fn versions(packages: &BTreeSet<DefiningPackage>) -> BTreeSet<Version> {
    packages
        .iter()
        .map(|package| package.version.clone())
        .collect()
}

fn judged_findings<R>(
    repository: &R,
    settings: &Settings,
    merge_base: &str,
    head: &str,
    workspace_path: &Path,
    findings: Vec<Finding>,
) -> anyhow::Result<Vec<JudgedFinding>>
where
    R: Repository,
{
    let mut manifest_marks = BTreeMap::new();
    let mut judged_findings = Vec::new();
    for finding in findings {
        let marked = if settings.require_marker {
            match manifest_marks.get(&finding.manifest) {
                Some(marked) => *marked,
                None => {
                    let marked = repository
                        .messages(merge_base, head, &workspace_path.join(&finding.manifest))?
                        .iter()
                        .any(|message| is_marked(message));
                    manifest_marks.insert(finding.manifest.clone(), marked);
                    marked
                }
            }
        } else {
            false
        };
        let violations = violations(&finding, settings, marked);
        judged_findings.push(JudgedFinding {
            finding,
            violations,
        });
    }
    Ok(judged_findings)
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{Snapshot, check::findings},
        domain::{
            Undecided,
            bump::Bump,
            package::{Dependency, Location, Package},
            reference::{ApiItems, DefiningPackage, Reference},
        },
    };
    use semver::{Version, VersionReq};
    use std::{
        collections::{BTreeMap, BTreeSet, VecDeque},
        path::Path,
    };

    #[test]
    fn a_change_is_attributed_to_the_bump_that_reaches_it() {
        let old = snapshot("1.0.0", &[("a 1.0.0", "c 1.0.0")]);
        let new = snapshot("2.0.0", &[("a 2.0.0", "c 2.0.0")]);
        let findings =
            findings(&old, &new, &[bump("a"), bump("b")], Path::new("/workspace")).unwrap();
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.bump.dependency, "a");
        assert_eq!(finding.package, "c");
        assert_eq!(
            finding.old_versions,
            BTreeSet::from([Version::new(1, 0, 0)])
        );
        assert_eq!(
            finding.new_versions,
            BTreeSet::from([Version::new(2, 0, 0)])
        );
        assert_eq!(finding.items, ["`c::T` in `subject::f`"]);
        assert_eq!(finding.location, Location::Own);
        assert_eq!(finding.manifest, Path::new("subject/Cargo.toml"));
    }

    #[test]
    fn a_change_that_no_bump_reaches_is_undecided() {
        let old = snapshot("1.0.0", &[]);
        let new = snapshot("2.0.0", &[]);
        let error =
            findings(&old, &new, &[bump("a"), bump("b")], Path::new("/workspace")).unwrap_err();
        let undecided = error.downcast::<Undecided>().unwrap();
        assert!(
            undecided.0.contains("cannot attribute the change of c"),
            "{undecided}"
        );
    }

    struct FakeSnapshot {
        libraries: Vec<Package>,
        api_items: ApiItems,
        dependencies: BTreeMap<String, Vec<String>>,
    }

    impl Snapshot for FakeSnapshot {
        fn libraries(&self) -> &[Package] {
            &self.libraries
        }

        fn api_items(&self, _library: &Package) -> anyhow::Result<ApiItems> {
            Ok(self.api_items.clone())
        }

        fn package_grouped_foreign_paths(
            &self,
            _library: &Package,
        ) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
            unreachable!()
        }

        fn direct_dependencies(&self, package_id: &str) -> Vec<String> {
            self.dependencies
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
            self.dependencies
                .keys()
                .find(|id| *id == package_id)
                .and_then(|id| id.split(' ').next())
                .unwrap_or("?")
        }

        fn package_version(&self, _package_id: &str) -> String {
            unreachable!()
        }
    }

    fn snapshot(version: &str, edges: &[(&str, &str)]) -> FakeSnapshot {
        let dependencies = [format!("a {version}"), format!("b {version}")];
        let mut graph = BTreeMap::from([
            ("subject 0.1.0".to_owned(), dependencies.to_vec()),
            (dependencies[0].clone(), vec![]),
            (dependencies[1].clone(), vec![]),
        ]);
        for (from, to) in edges {
            graph
                .entry((*from).to_owned())
                .or_default()
                .push((*to).to_owned());
            graph.entry((*to).to_owned()).or_default();
        }
        let reference = Reference {
            site: "subject::f".to_owned(),
            position: "/function/sig/output".to_owned(),
            path: "c::T".to_owned(),
            kind: "struct".to_owned(),
        };
        let package = DefiningPackage {
            id: format!("c {version}"),
            name: "c".to_owned(),
            version: Version::parse(version).unwrap(),
        };
        FakeSnapshot {
            libraries: vec![Package {
                id: "subject 0.1.0".to_owned(),
                name: "subject".to_owned(),
                version: Version::new(0, 1, 0),
                library: Some("subject".to_owned()),
                published: true,
                manifest: "/workspace/subject/Cargo.toml".into(),
                dependencies: ["a", "b"]
                    .into_iter()
                    .map(|name| Dependency {
                        name: name.to_owned(),
                        key: name.to_owned(),
                        target: None,
                        requirement: VersionReq::parse(version).unwrap(),
                        location: Location::Own,
                    })
                    .collect(),
            }],
            api_items: ApiItems::from([(reference, BTreeSet::from([package]))]),
            dependencies: graph,
        }
    }

    fn bump(dependency: &str) -> Bump {
        Bump {
            dependent: "subject".to_owned(),
            dependency: dependency.to_owned(),
            old: VersionReq::parse("^1").unwrap(),
            new: VersionReq::parse("^2").unwrap(),
        }
    }
}
