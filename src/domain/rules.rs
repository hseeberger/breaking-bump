use crate::domain::{
    finding::{Finding, Violation},
    package::Location,
    settings::Settings,
};

pub(crate) fn violations(finding: &Finding, settings: &Settings, marked: bool) -> Vec<Violation> {
    let mut violations = Vec::new();
    if settings.deny_inherited && finding.location == Location::Inherited {
        violations.push(Violation::Inherited);
    }
    if settings.require_marker && !marked {
        violations.push(Violation::MissingMarker);
    }
    violations
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bump::Bump,
        finding::{Finding, Violation},
        package::Location,
        rules::violations,
        settings::Settings,
    };
    use semver::VersionReq;
    use std::collections::BTreeSet;

    #[test]
    fn a_marker_clears_only_missing_marker() {
        let settings = settings(true, true);
        assert_eq!(violations(&finding(Location::Own), &settings, true), []);
        assert_eq!(
            violations(&finding(Location::Own), &settings, false),
            [Violation::MissingMarker]
        );
        assert_eq!(
            violations(&finding(Location::Inherited), &settings, true),
            [Violation::Inherited]
        );
        assert_eq!(
            violations(&finding(Location::Inherited), &settings, false),
            [Violation::Inherited, Violation::MissingMarker]
        );
    }

    #[test]
    fn disabled_rules_report_nothing() {
        let settings = settings(false, false);
        assert_eq!(
            violations(&finding(Location::Inherited), &settings, false),
            []
        );
    }

    fn settings(require_marker: bool, deny_inherited: bool) -> Settings {
        Settings {
            require_marker,
            deny_inherited,
        }
    }

    fn finding(location: Location) -> Finding {
        Finding {
            bump: Bump {
                package: "subject".to_owned(),
                dependency: "dep".to_owned(),
                old: VersionReq::parse("^1").unwrap(),
                new: VersionReq::parse("^2").unwrap(),
            },
            package: "dep".to_owned(),
            old_versions: BTreeSet::new(),
            new_versions: BTreeSet::new(),
            items: vec![],
            location,
            manifest: "Cargo.toml".into(),
        }
    }
}
