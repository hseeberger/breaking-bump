use crate::{
    application::{Cargo, Snapshot},
    domain::{
        Undecided,
        bump::{Bump, incompatible_bumps},
        changes::{Change, changes},
        finding::{Finding, Violation},
        marker::is_marked,
        package::Package,
        reference::DefiningPackage,
        rules::violations,
        settings::Settings,
    },
};
use anyhow::{Context, anyhow};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) trait Repository {
    type Worktree: Worktree;

    fn commit(&self, revision: &str) -> anyhow::Result<String>;

    fn default_base(&self) -> anyhow::Result<Option<String>>;

    fn merge_base(&self, base: &str, head: &str) -> anyhow::Result<String>;

    fn changed_paths(&self, from: &str, to: &str) -> anyhow::Result<Vec<String>>;

    fn dirty_manifests(&self) -> anyhow::Result<Vec<String>>;

    fn worktree(&self, commit: &str) -> anyhow::Result<Self::Worktree>;

    // `path` is relative to the workspace directory; history is not simplified.
    fn messages(&self, from: &str, to: &str, path: &Path) -> anyhow::Result<Vec<String>>;
}

pub(crate) trait Worktree {
    fn workspace(&self) -> &Path;
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
    Breaking(Vec<Judged>),
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
pub(crate) struct Judged {
    pub(crate) finding: Finding,
    pub(crate) violations: Vec<Violation>,
}

pub(crate) fn check<R, C>(repository: &R, cargo: &C, base: Option<&str>, head: &str) -> Report
where
    R: Repository,
    C: Cargo,
{
    let head = match repository.commit(head) {
        Ok(head) => head,
        Err(error) => return Report::error(error),
    };
    let mut warnings = Vec::new();
    let outcome = match compare(repository, cargo, base, &head, &mut warnings) {
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
        .any(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == "Cargo.toml")
        });
    if !changed_manifest {
        return Ok(Outcome::Compatible("no Cargo.toml changed".to_owned()));
    }

    let old_tree = repository.worktree(&merge_base)?;
    let new_tree = repository.worktree(head)?;
    let old_workspace = cargo.workspace(old_tree.workspace())?;
    let new_workspace = cargo.workspace(new_tree.workspace())?;
    let settings = Settings::from_workspace_metadata(&old_workspace.metadata)?;
    let bumps = incompatible_bumps(&old_workspace, &new_workspace)?;
    if bumps.is_empty() {
        return Ok(Outcome::Compatible(
            "no incompatible dependency bump".to_owned(),
        ));
    }

    let old = cargo.snapshot(old_tree.workspace())?;
    let new = cargo.snapshot(new_tree.workspace())?;
    let findings = findings(&old, &new, &bumps, new_tree.workspace())?;
    if findings.is_empty() {
        return Ok(Outcome::Compatible(
            "no public API item changed to an incompatible version".to_owned(),
        ));
    }

    let mut marked = BTreeMap::new();
    let mut judged = Vec::new();
    for finding in findings {
        let is_marked = if settings.require_marker {
            match marked.get(&finding.manifest) {
                Some(is_marked) => *is_marked,
                None => {
                    let is_marked = repository
                        .messages(&merge_base, head, &finding.manifest)?
                        .iter()
                        .any(|message| is_marked(message));
                    marked.insert(finding.manifest.clone(), is_marked);
                    is_marked
                }
            }
        } else {
            false
        };
        let violations = violations(&finding, &settings, is_marked);
        judged.push(Judged {
            finding,
            violations,
        });
    }
    Ok(Outcome::Breaking(judged))
}

fn findings<S>(old: &S, new: &S, bumps: &[Bump], workspace: &Path) -> anyhow::Result<Vec<Finding>>
where
    S: Snapshot,
{
    let names = bumps
        .iter()
        .map(|bump| bump.package.as_str())
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
            .filter(|bump| bump.package == name)
            .collect::<Vec<_>>();
        let manifest = new_library
            .manifest
            .strip_prefix(workspace)
            .with_context(|| {
                format!(
                    "manifest {} is outside of the workspace",
                    new_library.manifest.display()
                )
            })?
            .to_path_buf();
        for (package, group) in grouped(changes) {
            let culprits = culprits(old, new, old_library, new_library, &bumps, &group);
            if culprits.is_empty() {
                return Err(Undecided(format!(
                    "{name}: cannot attribute the change of {package} in the public API to a \
                     bumped dependency"
                ))
                .into());
            }
            for bump in culprits {
                findings.push(Finding {
                    bump: bump.clone(),
                    package: package.clone(),
                    old_versions: versions(&group.old),
                    new_versions: versions(&group.new),
                    items: group.items.clone(),
                    location: new_library.location(&bump.dependency),
                    manifest: manifest.clone(),
                });
            }
        }
    }
    Ok(findings)
}

struct Group {
    old: BTreeSet<DefiningPackage>,
    new: BTreeSet<DefiningPackage>,
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

fn grouped(changes: Vec<Change>) -> BTreeMap<String, Group> {
    let mut groups = BTreeMap::<String, Group>::new();
    for change in changes {
        let group = groups.entry(change.package).or_insert_with(|| Group {
            old: BTreeSet::new(),
            new: BTreeSet::new(),
            items: Vec::new(),
        });
        group.old.extend(change.old);
        group.new.extend(change.new);
        group.items.push(change.reference.to_string());
    }
    groups
}

fn culprits<'a, S>(
    old: &S,
    new: &S,
    old_library: &Package,
    new_library: &Package,
    bumps: &[&'a Bump],
    group: &Group,
) -> Vec<&'a Bump>
where
    S: Snapshot,
{
    let reaches_new = bumps
        .iter()
        .copied()
        .filter(|bump| reaches(new, new_library, &bump.dependency, &group.new))
        .collect::<Vec<_>>();
    let reaches_both = reaches_new
        .iter()
        .copied()
        .filter(|bump| reaches(old, old_library, &bump.dependency, &group.old))
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

fn versions(packages: &BTreeSet<DefiningPackage>) -> BTreeSet<semver::Version> {
    packages
        .iter()
        .map(|package| package.version.clone())
        .collect()
}
