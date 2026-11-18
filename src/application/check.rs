use crate::{
    application::{Cargo, Snapshot},
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
        .any(|path| is_manifest(Path::new(path)));
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

    let findings = judged_findings(repository, &settings, &merge_base, head, findings)?;
    Ok(Outcome::Breaking(findings))
}

fn findings<S>(old: &S, new: &S, bumps: &[Bump], workspace: &Path) -> anyhow::Result<Vec<Finding>>
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
            .strip_prefix(workspace)
            .with_context(|| {
                format!(
                    "manifest {} is outside of the workspace",
                    new_library.manifest.display()
                )
            })?
            .to_path_buf();
        let mut evidences = BTreeMap::<(&str, &str), Evidence>::new();
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
                let evidence = evidences
                    .entry((bump.dependency.as_str(), change.package.as_str()))
                    .or_insert_with(|| Evidence {
                        bump,
                        old_versions: BTreeSet::new(),
                        new_versions: BTreeSet::new(),
                        items: Vec::new(),
                    });
                evidence.old_versions.extend(versions(&change.old));
                evidence.new_versions.extend(versions(&change.new));
                evidence.items.push(change.reference.to_string());
            }
        }
        for ((_, package), evidence) in evidences {
            findings.push(Finding {
                bump: evidence.bump.clone(),
                package: package.to_owned(),
                old_versions: evidence.old_versions,
                new_versions: evidence.new_versions,
                items: evidence.items,
                location: new_library.location(&evidence.bump.dependency),
                manifest: manifest.clone(),
            });
        }
    }
    Ok(findings)
}

struct Evidence<'a> {
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
                        .messages(merge_base, head, &finding.manifest)?
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
