# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What this is

`cargo-breaking-bump`, a Cargo subcommand (binary crate only) with the subcommands `check`, `mark`
and `exposed`, plus the reusable workflow `.github/workflows/breaking-bump.yml`. The README
describes the behavior for users. Keep it in sync when behavior changes.

It replaces the Python tool hseeberger/public-deps-check and keeps its analysis.

## Commands

```sh
just all                                  # check, fmt, lint, test, doc, as CI runs them
cargo test --test check -- inherited      # integration tests matching a name
cargo test --bin cargo-breaking-bump      # unit tests only
```

The integration tests need the nightly whose date matches the pinned stable in
`rust-toolchain.toml` (derived from `rustc --version`, as in the justfile).
`BREAKING_BUMP_TEST_TOOLCHAIN` overrides it. They build real git repositories and crates in temp
directories (`tests/common/mod.rs`, a port of the Python fixture) and run the built binary with
`breaking-bump` as its first argument, as Cargo does. Each fixture has its own target directory.

Running it locally against another repository:

```sh
RUSTUP_TOOLCHAIN=nightly-YYYY-MM-DD path/to/cargo-breaking-bump breaking-bump check --base origin/main
```

## Architecture

Onion rings, one module each:

- `domain`: workspaces, packages and their dependencies, compatibility keys, bumps, references,
  changes between snapshots, findings and violations, settings, rules, markers, pull request
  preconditions and the `Bot` trait (`Dependabot`).
- `application`: one service per subcommand (`check`, `exposed`, `mark`, `preflight`). They define
  the traits the infrastructure implements: `Cargo` and `Snapshot` (in `application/mod.rs`),
  `Repository` and `Worktree` (`check`), `Forge` (`mark`), `Toolchain` (`preflight`).
- `infrastructure`: `git` (worktrees, log, push with lease), `cargo` (cargo metadata, cargo
  rustdoc, artifact mapping), `rustdoc` (reference walk and site names on `rustdoc_types::Crate`),
  `github` (REST client on ureq), `toolchain` (preflight probe).
- `main.rs` parses the CLI, wires the infrastructure into the services, prints and maps results to
  exit codes.

`Undecided` (in `domain`) is the escape hatch for anything the analysis cannot resolve with
certainty. It travels as an `anyhow::Error` and the services downcast it into the `undecided`
result.

Things that are easy to break:

- References are matched across the two snapshots by site, position, foreign path and kind, then
  compared by package name and version. Package IDs are only compared within one snapshot (they
  differ between worktrees). Attribution to a bump uses package IDs.
- The reference walk serializes `item.inner` to JSON on purpose, so a field added to rustdoc's
  format is not missed.
- A dependency's declarations are compared by manifest key, target and requirement, so two aliases
  or two targets that swap majors count as a change.
- The rustdoc JSON path comes from cargo's artifact message, since a configured build target moves
  the file.
- Finding manifests are relative to Cargo's workspace root and the marker search path to the Git
  root; the two differ for a workspace in a subdirectory.
- `check` runs cargo only in the worktrees, never in the working copy, so a broken uncommitted
  manifest only causes a warning.
- The head workspace is resolved from the nearest ancestor of the invocation directory that exists
  in the head commit, the merge-base workspace at the head workspace's root. The invocation
  directory may exist in neither commit.
- The marker search uses `git log --full-history`; without it a merge hides a marked commit on its
  second parent.
- `mark` changes the title before it pushes, so a retry can finish an interrupted run.
- The marked commit must have no custom author or committer, otherwise GitHub does not sign it.

## Upgrading rustdoc-types

Each release supports exactly one rustdoc JSON `format_version`. A nightly with a new format
needs a new `rustdoc-types` and a release of breaking-bump before the consumers bump their
toolchain. The table of supported toolchains in the README gets a row for that release.

## Releases

release-plz, as in the other crates of hseeberger. The reusable workflow reads the version from
`Cargo.toml` at its own commit, so a release needs no change to the workflow.
