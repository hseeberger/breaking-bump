mod common;

use crate::common::Fixture;

#[test]
fn exposed_dependencies() {
    let fixture = Fixture::new();
    let run = fixture.run(&["exposed"]);
    assert_eq!(run.code, 0, "{run}");
    let lines = run.stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "subject", "{run}");
    let dependencies = lines[1..]
        .iter()
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        dependencies,
        [
            "dep_b",
            "dep_marker",
            "dep_reexport",
            "dep_sig",
            "dep_trait"
        ],
        "{run}"
    );
    let line = |name: &str| {
        lines
            .iter()
            .find(|line| line.trim_start().starts_with(&format!("{name} ")))
            .unwrap()
            .to_string()
    };
    assert!(line("dep_marker").contains("`dep_marker::Marker`"), "{run}");
    assert!(line("dep_b").contains("through dep_c"), "{run}");
    assert!(line("dep_b").contains("`dep_c::Thing`"), "{run}");
    assert!(
        line("dep_sig").starts_with("  dep_sig ^1.0.0 (own)"),
        "{run}"
    );
}

#[test]
fn location_of_inherited_dependencies() {
    let fixture = Fixture::with(common::Options {
        workspace: true,
        ..Default::default()
    });
    let run = fixture.run(&["exposed", "-p", "subject"]);
    assert_eq!(run.code, 0, "{run}");
    assert!(run.stdout.contains("  dep_sig ^1.0.0 (inherited)"), "{run}");
}

#[test]
fn unknown_package_is_an_error() {
    let fixture = Fixture::new();
    let run = fixture.run(&["exposed", "--package", "unknown"]);
    assert_eq!(run.code, 3, "{run}");
}

#[test]
fn includes_uncommitted_changes() {
    let mut fixture = Fixture::new();
    fixture.versions.insert("dep_sig", "2.0.0".to_owned());
    fixture.write_all();
    let run = fixture.run(&["exposed"]);
    assert_eq!(run.code, 0, "{run}");
    assert!(run.stdout.contains("  dep_sig ^2.0.0 (own)"), "{run}");
}
