use crate::domain::Undecided;
use semver::{Op, Version, VersionReq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompatibilityKey(Vec<u64>);

pub(crate) fn requirement_key(requirement: &VersionReq) -> Result<CompatibilityKey, Undecided> {
    let [comparator] = requirement.comparators.as_slice() else {
        return Err(not_plain_caret(requirement));
    };
    if comparator.op != Op::Caret || !comparator.pre.is_empty() {
        return Err(not_plain_caret(requirement));
    }
    let parts = match (comparator.major, comparator.minor, comparator.patch) {
        (major, _, _) if major > 0 => vec![major],
        (_, None, _) => vec![0],
        (_, Some(minor), _) if minor > 0 => vec![0, minor],
        (_, Some(_), None) => vec![0, 0],
        (_, Some(_), Some(patch)) => vec![0, 0, patch],
    };
    Ok(CompatibilityKey(parts))
}

pub(crate) fn version_key(version: &Version) -> CompatibilityKey {
    let parts = if version.major > 0 {
        vec![version.major]
    } else if version.minor > 0 {
        vec![0, version.minor]
    } else {
        vec![0, 0, version.patch]
    };
    CompatibilityKey(parts)
}

fn not_plain_caret(requirement: &VersionReq) -> Undecided {
    Undecided(format!(
        "requirement `{requirement}` is not a plain caret requirement"
    ))
}

#[cfg(test)]
mod tests {
    use crate::domain::compatibility::{CompatibilityKey, requirement_key, version_key};
    use semver::{Version, VersionReq};

    #[test]
    fn caret_requirements() {
        for (requirement, parts) in [
            ("^6.0", vec![6]),
            ("^5.5", vec![5]),
            ("^0.50", vec![0, 50]),
            ("^0.51.0", vec![0, 51]),
            ("^0.0.3", vec![0, 0, 3]),
            ("1", vec![1]),
            ("^0", vec![0]),
            ("^0.0", vec![0, 0]),
        ] {
            let requirement = VersionReq::parse(requirement).unwrap();
            assert_eq!(
                requirement_key(&requirement).unwrap(),
                CompatibilityKey(parts)
            );
        }
    }

    #[test]
    fn other_requirements_are_undecided() {
        for requirement in ["~1.2", "=1.2.3", ">=1, <3", "*", "1.*", "^2.0.0-rc.1"] {
            let requirement = VersionReq::parse(requirement).unwrap();
            assert!(requirement_key(&requirement).is_err(), "{requirement}");
        }
    }

    #[test]
    fn version_keys() {
        assert_eq!(
            version_key(&Version::new(1, 2, 0)),
            version_key(&Version::new(1, 9, 3))
        );
        assert_ne!(
            version_key(&Version::new(0, 50, 0)),
            version_key(&Version::new(0, 51, 0))
        );
        assert_ne!(
            version_key(&Version::new(0, 0, 3)),
            version_key(&Version::new(0, 0, 4))
        );
    }
}
