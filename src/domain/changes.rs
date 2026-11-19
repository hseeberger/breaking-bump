use crate::domain::{
    compatibility::version_key,
    reference::{ApiItems, DefiningPackage, Reference},
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) reference: Reference,
    pub(crate) package: String,
    pub(crate) old: BTreeSet<DefiningPackage>,
    pub(crate) new: BTreeSet<DefiningPackage>,
}

pub(crate) fn changes(old: &ApiItems, new: &ApiItems) -> Vec<Change> {
    let mut changes = Vec::new();
    for (reference, old_packages) in old {
        let Some(new_packages) = new.get(reference) else {
            continue;
        };
        let names = old_packages
            .iter()
            .chain(new_packages)
            .map(|package| package.name.as_str())
            .collect::<BTreeSet<_>>();
        for name in names {
            let old = named(old_packages, name);
            let new = named(new_packages, name);
            let old_keys = old
                .iter()
                .map(|package| version_key(&package.version))
                .collect::<Vec<_>>();
            let new_keys = new
                .iter()
                .map(|package| version_key(&package.version))
                .collect::<Vec<_>>();
            let incompatible = old_keys.iter().any(|key| !new_keys.contains(key))
                || new_keys.iter().any(|key| !old_keys.contains(key));
            if !old.is_empty() && !new.is_empty() && incompatible {
                changes.push(Change {
                    reference: reference.clone(),
                    package: name.to_owned(),
                    old,
                    new,
                });
            }
        }
    }
    changes
}

fn named(packages: &BTreeSet<DefiningPackage>, name: &str) -> BTreeSet<DefiningPackage> {
    packages
        .iter()
        .filter(|package| package.name == name)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        changes::changes,
        reference::{ApiItems, DefiningPackage, Reference},
    };
    use semver::Version;
    use std::collections::BTreeSet;

    #[test]
    fn matches_references_by_site_and_position() {
        let old = items([
            ("s::f", "/function/sig/output", "a::T", "a", "1.2.0"),
            ("s::g", "/function/sig/output", "b::U", "b", "0.3.1"),
            ("s::h", "/function/sig/output", "c::V", "c", "1.0.0"),
        ]);
        let new = items([
            ("s::f", "/function/sig/output", "a::T", "a", "2.0.0"),
            ("s::g", "/function/sig/output", "b::U", "b", "0.3.9"),
            ("s::k", "/function/sig/output", "c::V", "c", "2.0.0"),
        ]);
        let changes = changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reference.to_string(), "`a::T` in `s::f`");
        assert_eq!(changes[0].package, "a");
    }

    #[test]
    fn other_sites_do_not_hide_a_change() {
        let old = items([
            ("s::transitive", "/output", "c::Thing", "c", "1.0.0"),
            ("s::direct", "/output", "c::Thing", "c", "2.0.0"),
        ]);
        let new = items([
            ("s::transitive", "/output", "c::Thing", "c", "2.0.0"),
            ("s::direct", "/output", "c::Thing", "c", "2.0.0"),
        ]);
        assert_eq!(changes(&old, &new).len(), 1);
    }

    #[test]
    fn zero_minor_versions_are_incompatible() {
        let old = items([("s::f", "/output", "n::Client", "n", "0.50.0")]);
        let new = items([("s::f", "/output", "n::Client", "n", "0.51.0")]);
        assert_eq!(changes(&old, &new).len(), 1);
    }

    #[test]
    fn package_ids_do_not_take_part_in_the_match() {
        let old = items([("s::f", "/output", "a::T", "a", "1.0.0")]);
        let mut new = old.clone();
        for packages in new.values_mut() {
            *packages = packages
                .iter()
                .map(|package| DefiningPackage {
                    id: format!("{} (other worktree)", package.id),
                    ..package.clone()
                })
                .collect();
        }
        assert!(changes(&old, &new).is_empty());
    }

    fn items<const N: usize>(entries: [(&str, &str, &str, &str, &str); N]) -> ApiItems {
        entries
            .into_iter()
            .map(|(site, position, path, name, version)| {
                let reference = Reference {
                    site: site.to_owned(),
                    position: position.to_owned(),
                    path: path.to_owned(),
                    kind: "struct".to_owned(),
                };
                let package = DefiningPackage {
                    id: format!("{name} {version}"),
                    name: name.to_owned(),
                    version: Version::parse(version).unwrap(),
                };
                (reference, BTreeSet::from([package]))
            })
            .collect()
    }
}
