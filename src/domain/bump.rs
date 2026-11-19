use crate::domain::{
    Undecided,
    compatibility::requirement_key,
    package::{Package, Workspace},
};
use semver::VersionReq;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bump {
    pub(crate) dependent: String,
    pub(crate) dependency: String,
    pub(crate) old: VersionReq,
    pub(crate) new: VersionReq,
}

pub(crate) fn incompatible_bumps(old: &Workspace, new: &Workspace) -> Result<Vec<Bump>, Undecided> {
    let old_members = old
        .members
        .iter()
        .filter(|package| package.is_published_library())
        .map(|package| (package.name.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let member_names = old
        .members
        .iter()
        .chain(&new.members)
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut bumps = Vec::new();
    for package in new
        .members
        .iter()
        .filter(|package| package.is_published_library())
    {
        let Some(old_package) = old_members.get(package.name.as_str()) else {
            continue;
        };
        let old_declarations = dependency_grouped_declarations(old_package);
        for (dependency, new_declarations) in dependency_grouped_declarations(package) {
            if member_names.contains(dependency) {
                continue;
            }
            let Some(old_declarations) = old_declarations.get(dependency) else {
                continue;
            };
            if *old_declarations == new_declarations {
                continue;
            }
            let old_requirement = requirement(old_package, dependency)?;
            let new_requirement = requirement(package, dependency)?;
            if old_requirement == new_requirement {
                continue;
            }
            if requirement_key(old_requirement)? != requirement_key(new_requirement)? {
                bumps.push(Bump {
                    dependent: package.name.clone(),
                    dependency: dependency.to_owned(),
                    old: old_requirement.clone(),
                    new: new_requirement.clone(),
                });
            }
        }
    }
    Ok(bumps)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Declaration<'a> {
    key: &'a str,
    target: Option<&'a str>,
    requirement: String,
}

fn dependency_grouped_declarations(package: &Package) -> BTreeMap<&str, BTreeSet<Declaration<'_>>> {
    let mut declarations = BTreeMap::<&str, BTreeSet<_>>::new();
    for dependency in &package.dependencies {
        declarations
            .entry(dependency.name.as_str())
            .or_default()
            .insert(Declaration {
                key: &dependency.key,
                target: dependency.target.as_deref(),
                requirement: dependency.requirement.to_string(),
            });
    }
    declarations
}

fn requirement<'a>(package: &'a Package, dependency: &str) -> Result<&'a VersionReq, Undecided> {
    let mut requirements = package
        .declarations(dependency)
        .map(|declaration| &declaration.requirement);
    match requirements.next() {
        Some(first) if requirements.all(|requirement| requirement == first) => Ok(first),
        _ => Err(Undecided(format!(
            "{} has several requirements for {dependency}",
            package.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bump::incompatible_bumps,
        package::{Dependency, Location, Package, Workspace},
    };
    use semver::{Version, VersionReq};
    use serde_json::Value;

    #[test]
    fn ignores_workspace_members() {
        let bumps =
            incompatible_bumps(&workspace("^0.27.1", "^1.0"), &workspace("^0.28.0", "^2.0"))
                .unwrap();
        let bumps = bumps
            .iter()
            .map(|bump| (bump.dependent.as_str(), bump.dependency.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(bumps, [("plugin", "ext")]);
    }

    #[test]
    fn several_requirements_are_undecided() {
        let mut new = workspace("^0.27.1", "^2.0");
        new.members[1].dependencies.push(dependency("ext", "^3.0"));
        assert!(incompatible_bumps(&workspace("^0.27.1", "^1.0"), &new).is_err());
    }

    #[test]
    fn unchanged_requirements_on_two_majors_are_no_bump() {
        let with_two_majors = |other: &str| {
            let mut workspace = workspace("^0.27.1", "^1.0");
            let plugin = &mut workspace.members[1];
            plugin.dependencies.push(renamed("ext", "ext2", "^2.0"));
            plugin.dependencies.push(dependency("other", other));
            workspace
        };
        let bumps = incompatible_bumps(&with_two_majors("^1.0"), &with_two_majors("^2.0")).unwrap();
        let bumps = bumps
            .iter()
            .map(|bump| (bump.dependent.as_str(), bump.dependency.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(bumps, [("plugin", "other")]);
    }

    #[test]
    fn aliases_that_swap_majors_are_undecided() {
        let with_two_majors = |own: &str, renamed_requirement: &str| {
            let mut workspace = workspace("^0.27.1", own);
            workspace.members[1]
                .dependencies
                .push(renamed("ext", "ext2", renamed_requirement));
            workspace
        };
        assert!(
            incompatible_bumps(
                &with_two_majors("^1.0", "^2.0"),
                &with_two_majors("^2.0", "^1.0")
            )
            .is_err()
        );
    }

    #[test]
    fn targets_that_swap_majors_are_undecided() {
        let with_two_targets = |unix: &str, windows: &str| {
            let mut workspace = workspace("^0.27.1", "^1.0");
            let plugin = &mut workspace.members[1];
            plugin
                .dependencies
                .push(targeted("http", "cfg(unix)", unix));
            plugin
                .dependencies
                .push(targeted("http", "cfg(windows)", windows));
            workspace
        };
        assert!(
            incompatible_bumps(
                &with_two_targets("^0.2", "^1.0"),
                &with_two_targets("^1.0", "^0.2")
            )
            .is_err()
        );
    }

    #[test]
    fn several_requirements_for_a_workspace_member_are_ignored() {
        let mut new = workspace("^0.28.0", "^1.0");
        new.members[1]
            .dependencies
            .push(renamed("core", "core2", "^0.29.0"));
        let bumps = incompatible_bumps(&workspace("^0.27.1", "^1.0"), &new).unwrap();
        assert!(bumps.is_empty());
    }

    #[test]
    fn ignores_crates_without_a_library() {
        let binary = |requirement: &str| Workspace {
            root: "/workspace".into(),
            members: vec![Package {
                library: None,
                ..package("tool", vec![dependency("serde", requirement)])
            }],
            metadata: Value::Null,
        };
        let bumps = incompatible_bumps(&binary("~1.0"), &binary("~2.0")).unwrap();
        assert!(bumps.is_empty());
    }

    #[test]
    fn ignores_crates_that_become_a_library() {
        let tool = |library: Option<String>, requirement: &str| Workspace {
            root: "/workspace".into(),
            members: vec![Package {
                library,
                ..package("tool", vec![dependency("serde", requirement)])
            }],
            metadata: Value::Null,
        };
        let bumps = incompatible_bumps(&tool(None, "~1.0"), &tool(Some("tool".to_owned()), "~2.0"))
            .unwrap();
        assert!(bumps.is_empty());
    }

    fn workspace(core: &str, external: &str) -> Workspace {
        Workspace {
            root: "/workspace".into(),
            members: vec![
                package("core", vec![]),
                package(
                    "plugin",
                    vec![dependency("core", core), dependency("ext", external)],
                ),
            ],
            metadata: Value::Null,
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
        renamed(name, name, requirement)
    }

    fn renamed(name: &str, key: &str, requirement: &str) -> Dependency {
        Dependency {
            name: name.to_owned(),
            key: key.to_owned(),
            target: None,
            requirement: VersionReq::parse(requirement).unwrap(),
            location: Location::Own,
        }
    }

    fn targeted(name: &str, target: &str, requirement: &str) -> Dependency {
        Dependency {
            target: Some(target.to_owned()),
            ..dependency(name, requirement)
        }
    }
}
