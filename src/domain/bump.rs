use crate::domain::{
    Undecided,
    compatibility::requirement_key,
    package::{Package, Workspace},
};
use semver::VersionReq;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bump {
    pub(crate) package: String,
    pub(crate) dependency: String,
    pub(crate) old: VersionReq,
    pub(crate) new: VersionReq,
}

pub(crate) fn incompatible_bumps(old: &Workspace, new: &Workspace) -> Result<Vec<Bump>, Undecided> {
    let old_members = old
        .members
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let member_names = old
        .members
        .iter()
        .chain(&new.members)
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut bumps = Vec::new();
    for package in new.members.iter().filter(|package| package.published) {
        let Some(old_package) = old_members.get(package.name.as_str()) else {
            continue;
        };
        let old_requirements = requirements(old_package)?;
        for (dependency, new_requirement) in requirements(package)? {
            if member_names.contains(dependency) {
                continue;
            }
            let Some(old_requirement) = old_requirements.get(dependency) else {
                continue;
            };
            if *old_requirement == new_requirement {
                continue;
            }
            if requirement_key(old_requirement)? != requirement_key(new_requirement)? {
                bumps.push(Bump {
                    package: package.name.clone(),
                    dependency: dependency.to_owned(),
                    old: (*old_requirement).clone(),
                    new: new_requirement.clone(),
                });
            }
        }
    }
    Ok(bumps)
}

fn requirements(package: &Package) -> Result<BTreeMap<&str, &VersionReq>, Undecided> {
    let mut requirements = BTreeMap::new();
    for dependency in &package.dependencies {
        let requirement = requirements
            .entry(dependency.name.as_str())
            .or_insert(&dependency.requirement);
        if **requirement != dependency.requirement {
            return Err(Undecided(format!(
                "{} has several requirements for {}",
                package.name, dependency.name
            )));
        }
    }
    Ok(requirements)
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bump::incompatible_bumps,
        package::{Dependency, Location, Package, Workspace},
    };
    use semver::{Version, VersionReq};

    #[test]
    fn ignores_workspace_members() {
        let bumps =
            incompatible_bumps(&workspace("^0.27.1", "^1.0"), &workspace("^0.28.0", "^2.0"))
                .unwrap();
        let bumps = bumps
            .iter()
            .map(|bump| (bump.package.as_str(), bump.dependency.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(bumps, [("plugin", "ext")]);
    }

    #[test]
    fn several_requirements_are_undecided() {
        let mut new = workspace("^0.27.1", "^2.0");
        new.members[1].dependencies.push(dependency("ext", "^3.0"));
        assert!(incompatible_bumps(&workspace("^0.27.1", "^1.0"), &new).is_err());
    }

    fn workspace(core: &str, external: &str) -> Workspace {
        Workspace {
            members: vec![
                package("core", vec![]),
                package(
                    "plugin",
                    vec![dependency("core", core), dependency("ext", external)],
                ),
            ],
            metadata: serde_json::Value::Null,
        }
    }

    fn package(name: &str, dependencies: Vec<Dependency>) -> Package {
        Package {
            id: name.to_owned(),
            name: name.to_owned(),
            version: Version::new(1, 0, 0),
            library: Some(name.to_owned()),
            published: true,
            manifest: format!("{name}/Cargo.toml").into(),
            dependencies,
        }
    }

    fn dependency(name: &str, requirement: &str) -> Dependency {
        Dependency {
            name: name.to_owned(),
            requirement: VersionReq::parse(requirement).unwrap(),
            location: Location::Own,
        }
    }
}
