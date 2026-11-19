mod common;

use crate::common::{Fixture, Options, bin_dir, host, nightly, stable};
use std::{env, process::Command};

#[test]
fn exposed_bump_without_marker() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert_eq!(run.outputs["head"], head);
    assert!(
        run.stdout.contains("`dep_sig::S` in `subject::signature`"),
        "{run}"
    );
    assert!(run.stdout.contains("(own)"), "{run}");
}

#[test]
fn exposed_bump_with_marker() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture.check(&base, &head).assert(0, "breaking", "");
}

#[test]
fn breaking_change_footer() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump(
        "dep_sig",
        "2.0.0",
        "build(deps): bump dep_sig\n\nBREAKING CHANGE: dep_sig 2",
    );
    fixture.check(&base, &head).assert(0, "breaking", "");
}

#[test]
fn derive_bump() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_marker", "2.0.0", "build(deps): bump dep_marker");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert!(run.stdout.contains("dep_marker::Marker"), "{run}");
}

#[test]
fn optional_bump_that_changes_a_transitive_crate() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert!(run.stdout.contains("dep_c::Thing"), "{run}");
}

#[test]
fn bump_whose_exposed_package_stays_compatible() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
    fixture.check(&base, &head).assert(0, "compatible", "");
}

#[test]
fn bump_that_moves_a_reexport_to_the_version_used_directly() {
    let mut fixture = Fixture::with(Options {
        private_dep_c: Some("2.0.0"),
        ..Options::default()
    });
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn bump_hidden_by_another_public_use_of_the_new_version() {
    for keep_dep_c_1 in [false, true] {
        let mut fixture = Fixture::with(Options {
            private_dep_c: Some("2.0.0"),
            public_dep_c: true,
            keep_dep_c_1,
            ..Options::default()
        });
        let base = fixture.head();
        fixture.dep_b_uses_dep_c_2 = true;
        let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
        let run = fixture.check(&base, &head);
        run.assert(1, "breaking", "missing-marker");
        assert!(run.stdout.contains("transitive"), "{run}");
    }
}

#[test]
fn same_named_impls_in_different_modules() {
    let mut fixture = Fixture::with(Options {
        private_dep_c: Some("2.0.0"),
        public_dep_c: true,
        keep_dep_c_1: true,
        same_named_impls: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert!(run.stdout.contains("<subject::first::S>::get"), "{run}");
    assert!(!run.stdout.contains("second"), "{run}");
}

#[test]
fn only_the_bump_that_reaches_the_changed_version_is_blamed() {
    let mut fixture = Fixture::with(Options {
        keep_dep_c_1: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    fixture.versions.insert("dep_keep", "2.0.0".to_owned());
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b and dep_keep");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert!(run.stdout.contains("bumping dep_b from"), "{run}");
    assert!(!run.stdout.contains("bumping dep_keep from"), "{run}");
}

#[test]
fn each_bump_gets_only_the_changes_it_explains() {
    let mut fixture = Fixture::with(Options {
        keep_dep_c_1: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    fixture.dep_keep_uses_dep_c_3 = true;
    fixture.versions.insert("dep_keep", "2.0.0".to_owned());
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b and dep_keep");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    let finding = |dependency: &str| {
        run.stdout
            .lines()
            .find(|line| line.contains(&format!("bumping {dependency} from")))
            .unwrap_or_else(|| panic!("no finding for {dependency}: {run}"))
            .to_owned()
    };
    let dep_b = finding("dep_b");
    assert!(dep_b.contains("from 1.0.0 to 2.0.0"), "{run}");
    assert!(dep_b.contains("subject::transitive"), "{run}");
    assert!(!dep_b.contains("subject::kept"), "{run}");
    let dep_keep = finding("dep_keep");
    assert!(dep_keep.contains("from 1.0.0 to 3.0.0"), "{run}");
    assert!(dep_keep.contains("subject::kept"), "{run}");
    assert!(!dep_keep.contains("subject::transitive"), "{run}");
}

#[test]
fn bumps_that_are_not_breaking() {
    for (name, version) in [
        ("dep_private", "2.0.0"),
        ("dep_dev", "2.0.0"),
        ("dep_sig", "1.1.0"),
    ] {
        let mut fixture = Fixture::new();
        let base = fixture.head();
        let head = fixture.bump(name, version, &format!("build(deps): bump {name}"));
        fixture.check(&base, &head).assert(0, "compatible", "");
    }
}

#[test]
fn no_cargo_toml_change() {
    let fixture = Fixture::new();
    let base = fixture.head();
    fixture.write("README.md", "readme");
    let head = fixture.commit("docs: add readme");
    let run = fixture.check(&base, &head);
    run.assert(0, "compatible", "");
    assert!(run.stdout.contains("no Cargo.toml changed"), "{run}");
}

#[test]
fn compares_with_merge_base() {
    let mut fixture = Fixture::new();
    let fork = fixture.head();
    let main = fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture.git(&["checkout", "--quiet", "-b", "branch", &fork]);
    fixture.versions.insert("dep_sig", "1.0.0".to_owned());
    fixture.description = "changed".to_owned();
    fixture.write_all();
    let head = fixture.commit("chore: change description");
    fixture.check(&main, &head).assert(0, "compatible", "");
}

#[test]
fn bump_that_changes_a_crate_also_used_in_another_version() {
    let mut fixture = Fixture::with(Options {
        private_dep_c: Some("1.0.0"),
        ..Options::default()
    });
    let base = fixture.head();
    fixture.dep_b_uses_dep_c_2 = true;
    let head = fixture.bump("dep_b", "2.0.0", "build(deps): bump dep_b");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn cargo_config_of_the_checked_commit() {
    let mut fixture = Fixture::with(Options {
        cargo_env: true,
        ..Options::default()
    });
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn configured_build_target() {
    let mut fixture = Fixture::new();
    let unconfigured = fixture.head();
    fixture.write(
        ".cargo/config.toml",
        &format!("[build]\ntarget = \"{}\"\n", host()),
    );
    let configured = fixture.commit("build: configure the build target");
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    for base in [&unconfigured, &configured] {
        fixture
            .check(base, &head)
            .assert(1, "breaking", "missing-marker");
    }
}

#[test]
fn requirement_that_is_no_plain_caret_is_undecided() {
    let mut fixture = Fixture::new();
    fixture.requirements.insert("dep_sig", "~1.0".to_owned());
    fixture.write_all();
    let base = fixture.commit("chore: tilde requirement");
    fixture.requirements.insert("dep_sig", "~2.0".to_owned());
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    let run = fixture.check(&base, &head);
    run.assert(2, "undecided", "");
    assert!(
        run.stdout
            .contains("cannot decide: requirement `~1.0` is not a plain caret requirement"),
        "{run}"
    );
}

#[test]
fn bump_at_each_position_of_the_public_api() {
    let mut fixture = Fixture::with(Options {
        positions: true,
        ..Options::default()
    });
    for (name, item) in [
        ("dep_field", "`dep_field::S` in `subject::Holder::field`"),
        ("dep_bound", "`dep_bound::Tr` in `subject::bounded`"),
        ("dep_arg", "`dep_arg::S` in `subject::listed`"),
        (
            "dep_from",
            "`dep_from::S` in `<subject::Local as core::convert::From<dep_from::S>>`",
        ),
        (
            "dep_variant",
            "`dep_variant::S` in `subject::Choice::Variant",
        ),
        ("dep_method", "`dep_method::S` in `subject::Api::method`"),
    ] {
        let base = fixture.head();
        let head = fixture.bump(name, "2.0.0", &format!("build(deps): bump {name}"));
        let run = fixture.check(&base, &head);
        run.assert(1, "breaking", "missing-marker");
        assert!(run.stdout.contains(item), "{name}: {run}");
    }
}

#[test]
fn target_specific_dependency() {
    for (workspace, location, violations) in [
        (false, "(own)", "missing-marker"),
        (true, "(inherited)", "inherited,missing-marker"),
    ] {
        let mut fixture = Fixture::with(Options {
            workspace,
            target_specific: true,
            ..Options::default()
        });
        let base = fixture.head();
        let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
        let run = fixture.check(&base, &head);
        run.assert(1, "breaking", violations);
        assert!(run.stdout.contains(location), "{run}");
    }
}

#[test]
fn renamed_dependency() {
    for (workspace, location, violations) in [
        (false, "(own)", "missing-marker"),
        (true, "(inherited)", "inherited,missing-marker"),
    ] {
        let mut fixture = Fixture::with(Options {
            workspace,
            renamed: true,
            ..Options::default()
        });
        let base = fixture.head();
        let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
        let run = fixture.check(&base, &head);
        run.assert(1, "breaking", violations);
        assert!(run.stdout.contains("bumping dep_sig from"), "{run}");
        assert!(run.stdout.contains(location), "{run}");
    }
}

#[test]
fn unpublished_crate_is_not_checked() {
    let mut fixture = Fixture::with(Options {
        unpublished: true,
        ..Options::default()
    });
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    let run = fixture.check(&base, &head);
    run.assert(0, "compatible", "");
    assert!(
        run.stdout.contains("no incompatible dependency bump"),
        "{run}"
    );
}

#[test]
fn marker_counts_only_for_the_crate_whose_manifest_it_changes() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        second_member: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.bump("dep_reexport", "2.0.0", "build(deps)!: bump dep_reexport");
    let head = fixture.bump("dep_other", "2.0.0", "build(deps): bump dep_other");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    let lines = run.stdout.lines().collect::<Vec<_>>();
    let finding = |start: &str| {
        lines
            .iter()
            .position(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no finding `{start}`: {run}"))
    };
    let subject = finding("subject: bumping dep_reexport from");
    assert!(!lines[subject + 1].contains("missing-marker"), "{run}");
    let other = finding("other: bumping dep_other from");
    assert!(lines[other + 1].starts_with("  missing-marker: "), "{run}");
    assert!(lines[other + 1].contains("other/Cargo.toml"), "{run}");
}

#[test]
fn dependency_on_another_member_is_ignored() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        second_member: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.subject_version = "0.2.0".to_owned();
    fixture.write_all();
    let head = fixture.commit("feat!: change subject");
    let run = fixture.check(&base, &head);
    run.assert(0, "compatible", "");
    assert!(
        run.stdout.contains("no incompatible dependency bump"),
        "{run}"
    );
}

#[test]
fn workspace_in_a_subdirectory_of_the_repository() {
    for (message, code, violations) in [
        ("build(deps): bump dep_sig", 1, "missing-marker"),
        ("build(deps)!: bump dep_sig", 0, ""),
    ] {
        let mut fixture = Fixture::with(Options {
            nested: true,
            ..Options::default()
        });
        let base = fixture.head();
        let head = fixture.bump("dep_sig", "2.0.0", message);
        fixture
            .check(&base, &head)
            .assert(code, "breaking", violations);
    }
}

#[test]
fn check_from_a_member_of_a_workspace_in_a_subdirectory() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        second_member: true,
        nested: true,
        ..Options::default()
    });
    let base = fixture.head();
    let unmarked = fixture.bump("dep_other", "2.0.0", "build(deps): bump dep_other");
    let marked = fixture.bump("dep_other", "3.0.0", "build(deps)!: bump dep_other");
    let member = fixture.workspace_dir().join("subject");
    let run = fixture.run_in(&member, &["check", "--base", &base, "--head", &unmarked]);
    run.assert(1, "breaking", "missing-marker");
    assert!(
        run.stdout
            .contains("that changes other/Cargo.toml is marked"),
        "{run}"
    );
    fixture
        .run_in(&member, &["check", "--base", &base, "--head", &marked])
        .assert(0, "breaking", "");
}

#[test]
fn check_from_a_member_added_in_the_head_commit() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        nested: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.options.second_member = true;
    fixture.write_all();
    let head = fixture.commit("feat: add other");
    let member = fixture.workspace_dir().join("other");
    let run = fixture.run_in(&member, &["check", "--base", &base, "--head", &head]);
    run.assert(0, "compatible", "");
    assert!(
        run.stdout.contains("no incompatible dependency bump"),
        "{run}"
    );
}

#[test]
fn check_from_a_member_missing_in_the_head_commit() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        nested: true,
        ..Options::default()
    });
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture.options.second_member = true;
    fixture.write_all();
    fixture.commit("feat: add other");
    let member = fixture.workspace_dir().join("other");
    fixture
        .run_in(&member, &["check", "--base", &base, "--head", &head])
        .assert(1, "breaking", "inherited,missing-marker");
}

#[test]
fn inherited_dependency() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        ..Options::default()
    });
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "inherited,missing-marker");
    assert!(run.stdout.contains("(inherited)"), "{run}");
}

#[test]
fn marked_inherited_dependency_is_still_a_violation() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        ..Options::default()
    });
    let base = fixture.head();
    fixture.description = "changed".to_owned();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "inherited");
}

#[test]
fn two_findings_without_marker_name_the_violation_once() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.versions.insert("dep_trait", "2.0.0".to_owned());
    let head = fixture.bump(
        "dep_sig",
        "2.0.0",
        "build(deps): bump dep_sig and dep_trait",
    );
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert_eq!(run.stdout.matches("  missing-marker:").count(), 2, "{run}");
}

#[test]
fn settings_can_disable_both_rules() {
    let mut fixture = Fixture::with(Options {
        workspace: true,
        ..Options::default()
    });
    fixture.settings = Some("require-marker = false\ndeny-inherited = false".to_owned());
    fixture.write_all();
    let base = fixture.commit("chore: report only");
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture.check(&base, &head).assert(0, "breaking", "");
}

#[test]
fn settings_come_from_the_merge_base() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.settings = Some("require-marker = false".to_owned());
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn unknown_settings_are_an_error() {
    let mut fixture = Fixture::new();
    fixture.settings = Some("require-markers = false".to_owned());
    fixture.write_all();
    let base = fixture.commit("chore: misspelled setting");
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    let run = fixture.check(&base, &head);
    run.assert(3, "error", "");
    assert!(run.stderr.contains("breaking-bump"), "{run}");
}

#[test]
fn marker_anywhere_in_the_range() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture.write("README.md", "readme");
    let head = fixture.commit("docs: add readme");
    fixture.check(&base, &head).assert(0, "breaking", "");
}

#[test]
fn marked_commit_must_change_the_manifest() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture.write("README.md", "readme");
    let head = fixture.commit("docs!: add readme");
    fixture
        .check(&base, &head)
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn marker_on_the_second_parent_of_a_merge() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.git(&["checkout", "--quiet", "-b", "marked"]);
    fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture.git(&["checkout", "--quiet", "-b", "unmarked", &base]);
    fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture.git(&["merge", "--quiet", "--no-edit", "--no-ff", "marked"]);
    let head = fixture.head();
    fixture.check(&base, &head).assert(0, "breaking", "");
}

#[test]
fn dirty_manifest_is_reported() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps)!: bump dep_sig");
    fixture.description = "dirty".to_owned();
    fixture.write_all();
    let run = fixture.check(&base, &head);
    run.assert(0, "breaking", "");
    assert!(
        run.stderr
            .contains("warning: Cargo.toml has uncommitted changes"),
        "{run}"
    );
}

#[test]
fn broken_uncommitted_manifest_is_only_a_warning() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture.write("Cargo.toml", "[package\n");
    let run = fixture.check(&base, &head);
    run.assert(1, "breaking", "missing-marker");
    assert!(
        run.stderr
            .contains("warning: Cargo.toml has uncommitted changes"),
        "{run}"
    );
}

#[test]
fn missing_default_base_is_an_error() {
    let fixture = Fixture::new();
    let run = fixture.run(&["check"]);
    run.assert(3, "error", "");
    assert!(run.stderr.contains("--base"), "{run}");
}

#[test]
fn default_base_is_origin_head() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    fixture.git(&["update-ref", "refs/remotes/origin/main", &base]);
    fixture.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    fixture
        .run(&["check"])
        .assert(1, "breaking", "missing-marker");
}

#[test]
fn stable_toolchain_is_rejected() {
    let fixture = Fixture::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-breaking-bump"));
    command.args(["breaking-bump", "check", "--base", "HEAD"]);
    let run = fixture.execute(command, Some(&stable()));
    assert_eq!(run.code, 3, "{run}");
    assert!(run.stderr.contains("cargo +nightly"), "{run}");
}

#[test]
fn through_cargo() {
    let mut fixture = Fixture::new();
    let base = fixture.head();
    let head = fixture.bump("dep_sig", "2.0.0", "build(deps): bump dep_sig");
    let path = env::join_paths(
        [bin_dir()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let mut command = Command::new("cargo");
    command
        .arg(format!("+{}", nightly()))
        .args(["breaking-bump", "check", "--base", &base, "--head", &head])
        .env("PATH", path);
    let run = fixture.execute(command, None);
    run.assert(1, "breaking", "missing-marker");
}
