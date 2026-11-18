use crate::{
    application::{Cargo, Snapshot},
    domain::{
        Undecided,
        package::{Location, Package},
    },
};
use anyhow::bail;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug)]
pub(crate) struct Audit {
    pub(crate) libraries: Vec<LibraryAudit>,
}

impl Audit {
    pub(crate) fn is_complete(&self) -> bool {
        self.libraries
            .iter()
            .all(|library| matches!(library.verdict, Verdict::Exposed(_)))
    }
}

#[derive(Debug)]
pub(crate) struct LibraryAudit {
    pub(crate) name: String,
    pub(crate) verdict: Verdict,
}

#[derive(Debug)]
pub(crate) enum Verdict {
    Exposed(Vec<Exposure>),
    Undecided(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Exposure {
    pub(crate) dependency: String,
    pub(crate) requirements: Vec<String>,
    pub(crate) location: Location,
    pub(crate) through: Vec<String>,
    pub(crate) example: String,
    pub(crate) more: usize,
}

pub(crate) fn exposed<C>(cargo: &C, directory: &Path, packages: &[String]) -> anyhow::Result<Audit>
where
    C: Cargo,
{
    let snapshot = cargo.snapshot(directory)?;
    for package in packages {
        if !snapshot
            .libraries()
            .iter()
            .any(|library| &library.name == package)
        {
            bail!("{package} is not a published library of the workspace");
        }
    }

    let mut libraries = Vec::new();
    for library in snapshot.libraries() {
        if !packages.is_empty() && !packages.contains(&library.name) {
            continue;
        }
        let verdict = match exposures(&snapshot, library) {
            Ok(exposures) => Verdict::Exposed(exposures),
            Err(error) => Verdict::Undecided(error.downcast::<Undecided>()?.0),
        };
        libraries.push(LibraryAudit {
            name: library.name.clone(),
            verdict,
        });
    }
    Ok(Audit { libraries })
}

fn exposures<S>(snapshot: &S, library: &Package) -> anyhow::Result<Vec<Exposure>>
where
    S: Snapshot,
{
    let foreign_items = snapshot.foreign_items(library)?;
    let distances = snapshot
        .direct_dependencies(&library.id)
        .into_iter()
        .map(|dependency| {
            let distances = snapshot.distances(&dependency);
            (dependency, distances)
        })
        .collect::<Vec<_>>();

    let mut owned = BTreeMap::<&str, BTreeMap<&str, &BTreeSet<String>>>::new();
    for (package, items) in &foreign_items {
        let reach = distances
            .iter()
            .filter_map(|(dependency, distances)| {
                distances
                    .get(package)
                    .map(|distance| (dependency.as_str(), *distance))
            })
            .collect::<Vec<_>>();
        let Some(closest) = reach.iter().map(|(_, distance)| *distance).min() else {
            return Err(Undecided(format!(
                "{}: cannot attribute {} {} in the public API to a dependency",
                library.name,
                snapshot.package_name(package),
                snapshot.package_version(package)
            ))
            .into());
        };
        for (owner, _) in reach.iter().filter(|(_, distance)| *distance == closest) {
            owned
                .entry(owner)
                .or_default()
                .insert(package.as_str(), items);
        }
    }

    let mut exposures = owned
        .into_iter()
        .map(|(owner, packages)| exposure(snapshot, library, owner, &packages))
        .collect::<Vec<_>>();
    exposures.sort_by(|a, b| a.dependency.cmp(&b.dependency));
    Ok(exposures)
}

fn exposure<S>(
    snapshot: &S,
    library: &Package,
    owner: &str,
    packages: &BTreeMap<&str, &BTreeSet<String>>,
) -> Exposure
where
    S: Snapshot,
{
    let dependency = snapshot.package_name(owner).to_owned();
    let own_items = packages
        .get(owner)
        .into_iter()
        .flat_map(|items| items.iter());
    let reached_items = packages
        .iter()
        .filter(|(package, _)| **package != owner)
        .flat_map(|(_, items)| items.iter());
    let items = own_items.chain(reached_items).collect::<Vec<_>>();
    let through = packages
        .keys()
        .filter(|package| **package != owner)
        .map(|package| snapshot.package_name(package).to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let requirements = library
        .declarations(&dependency)
        .map(|declaration| declaration.requirement.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let distinct = items.iter().collect::<BTreeSet<_>>().len();
    Exposure {
        location: library.location(&dependency),
        dependency,
        requirements,
        through,
        example: items
            .first()
            .map(|item| (*item).clone())
            .unwrap_or_default(),
        more: distinct.saturating_sub(1),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{
            Cargo, Snapshot,
            exposed::{Exposure, Verdict, exposed},
        },
        domain::{
            Undecided,
            package::{Dependency, Location, Package, Workspace},
            reference::ApiItems,
        },
    };
    use semver::{Version, VersionReq};
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::Path,
    };

    #[test]
    fn an_unmapped_crate_leaves_the_rest_of_the_audit() {
        let audit = exposed(&Fake, Path::new("."), &[]).unwrap();
        assert!(!audit.is_complete());
        assert_eq!(audit.libraries.len(), 2);
        assert!(matches!(&audit.libraries[0].verdict, Verdict::Undecided(_)));
        let Verdict::Exposed(exposures) = &audit.libraries[1].verdict else {
            panic!("second library is undecided");
        };
        assert_eq!(
            exposures,
            &[
                Exposure {
                    dependency: "a".to_owned(),
                    requirements: vec!["^1".to_owned()],
                    location: Location::Inherited,
                    through: vec!["c".to_owned()],
                    example: "a::A".to_owned(),
                    more: 1,
                },
                Exposure {
                    dependency: "b".to_owned(),
                    requirements: vec!["^1".to_owned()],
                    location: Location::Own,
                    through: vec![],
                    example: "b::B".to_owned(),
                    more: 0,
                },
            ]
        );
    }

    #[test]
    fn restricts_the_audit_to_the_given_packages() {
        let audit = exposed(&Fake, Path::new("."), &["good".to_owned()]).unwrap();
        assert!(audit.is_complete());
        assert_eq!(audit.libraries.len(), 1);
        assert!(exposed(&Fake, Path::new("."), &["unknown".to_owned()]).is_err());
    }

    struct Fake;

    struct FakeSnapshot {
        libraries: Vec<Package>,
    }

    impl Cargo for Fake {
        type Snapshot = FakeSnapshot;

        fn workspace(&self, _directory: &Path) -> anyhow::Result<Workspace> {
            unreachable!()
        }

        fn snapshot(&self, _directory: &Path) -> anyhow::Result<FakeSnapshot> {
            Ok(FakeSnapshot {
                libraries: vec![library("bad"), library("good")],
            })
        }
    }

    impl Snapshot for FakeSnapshot {
        fn libraries(&self) -> &[Package] {
            &self.libraries
        }

        fn api_items(&self, _library: &Package) -> anyhow::Result<ApiItems> {
            unreachable!()
        }

        fn foreign_items(
            &self,
            library: &Package,
        ) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
            if library.name == "bad" {
                return Err(Undecided("cannot map crate x".to_owned()).into());
            }
            Ok(BTreeMap::from([
                ("a".to_owned(), BTreeSet::from(["a::A".to_owned()])),
                ("b".to_owned(), BTreeSet::from(["b::B".to_owned()])),
                ("c".to_owned(), BTreeSet::from(["c::C".to_owned()])),
            ]))
        }

        fn direct_dependencies(&self, _package_id: &str) -> Vec<String> {
            vec!["a".to_owned(), "b".to_owned()]
        }

        fn distances(&self, package_id: &str) -> BTreeMap<String, usize> {
            match package_id {
                "a" => BTreeMap::from([("a".to_owned(), 0), ("c".to_owned(), 1)]),
                _ => BTreeMap::from([(package_id.to_owned(), 0)]),
            }
        }

        fn package_name(&self, package_id: &str) -> &str {
            ["a", "b", "c"]
                .into_iter()
                .find(|name| *name == package_id)
                .unwrap_or("?")
        }

        fn package_version(&self, _package_id: &str) -> String {
            "1.0.0".to_owned()
        }
    }

    fn library(name: &str) -> Package {
        Package {
            id: name.to_owned(),
            name: name.to_owned(),
            version: Version::new(1, 0, 0),
            library: Some(name.to_owned()),
            published: true,
            manifest: format!("{name}/Cargo.toml").into(),
            dependencies: vec![
                Dependency {
                    name: "a".to_owned(),
                    requirement: VersionReq::parse("^1").unwrap(),
                    location: Location::Inherited,
                },
                Dependency {
                    name: "b".to_owned(),
                    requirement: VersionReq::parse("^1").unwrap(),
                    location: Location::Own,
                },
            ],
        }
    }
}
