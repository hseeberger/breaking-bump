# Plan: build breaking-bump from docs/spec.md

## Context

public-deps-check is a Python script behind a composite GitHub Action. Seven repos use it:
api-version, pub-sub-client, error-ext, configured, eventsourced, composed (hseeberger) and tellus
(tellus-rs). `docs/spec.md` specifies its successor, breaking-bump: a Rust Cargo subcommand
(`check`, `mark`, `exposed`) plus a reusable workflow. This plan gets from the spec to a released
tool and seven migrated repos.

Decisions taken while planning:

- **New repo, no history.** GitHub does not redirect `uses: hseeberger/public-deps-check@...`
  after a rename, so the old repo stays untouched until the end and is then archived.
- **api-version is the template** for project files, CI, release-plz and GitHub settings.
- **The workflow reads its own `Cargo.toml`.** It fetches `Cargo.toml` at `job.workflow_sha` and
  installs that version from crates.io. No version number in the workflow file, so release-plz
  works unchanged. The GitHub docs confirm that `job.workflow_sha` names the commit of the called
  workflow.
- **The crate starts at 0.1.0**, because the new repo has no earlier release.
- **`mark` edits the title before it pushes**, so a retry can finish an interrupted run.
- **The marked commit has no custom author or committer.** GitHub only signs API commits without
  them, and the rulesets require signed commits.

Steps that need a go-ahead when they come up, because they are public or hard to undo: creating
GitHub repos, the first publish to crates.io, ruleset changes, archiving public-deps-check.

## Step 1: new repo and project files

1. Create `~/code/hseeberger/breaking-bump` with `git init`. Create `hseeberger/breaking-bump` on
   GitHub (public) with the settings of api-version: merge commits only, auto-merge on, delete
   branch on merge, no wiki, no projects.
2. Copy from api-version and adapt: `rustfmt.toml`, `taplo.toml`, `rust-toolchain.toml`
   (1.100.0), `justfile`, `.gitignore`, `LICENSE`, `.github/CODEOWNERS`,
   `.github/dependabot.yml`, `.github/workflows/{ci,audit,dependabot-automerge,release-plz}.yml`.
   Drop the `public-deps` jobs from `ci.yml`. The test job installs the derived nightly next to
   the stable.
3. Move `docs/spec.md` over and save this plan as `docs/plan.md`. Write a new `AGENTS.md` once
   the code exists.
4. Amend the spec:
   - Distribution and the check job: "installs the version that `Cargo.toml` declares at the
     workflow's own commit" replaces the pin sentences.
   - `mark` preconditions: the pull request title must be conventional, with or without `!`. The
     commit subject must still have no `!`.
   - `mark` action, in this order: create the commit, add the `!` to the title if it is missing,
     push with the lease.
   - `mark` exit codes: a head that is no longer the examined commit exits 0 wherever it is
     detected (API check or rejected lease). After a rejected lease the output says that the
     title was already changed.
   - Documentation note: if the branch moves between the title edit and the push, the title can
     keep a `!` for a commit that `mark` never marked.
5. Copy the `default` ruleset of api-version without "Check Public Dependencies". The secrets
   `APP_CLIENT_ID`, `APP_PRIVATE_KEY` (Actions and Dependabot) and `CARGO_REGISTRY_TOKEN` are
   yours to set.

## Step 2: crate skeleton and preflight

Single package `cargo-breaking-bump`, binary only, version 0.1.0. Cargo starts it as
`cargo-breaking-bump breaking-bump <subcommand> ...`, so the CLI expects `breaking-bump` as its
first argument.

Dependencies: `clap` (derive), `cargo_metadata` 0.23, `rustdoc-types` 0.61 (format 61, which
matches `nightly-2026-11-09`), `semver`, `serde`, `serde_json`, `toml`, `thiserror`, `anyhow`,
`ureq` 3, `base64`, `tempfile`.

Module layout by ring:

| Ring | Module | Content |
|---|---|---|
| Domain model | `domain::{compatibility, reference, finding, settings, marker, bot}` | compatibility key, foreign reference (site, position, path, kind), finding with location, violation, result, settings, marker detection, `Bot` trait with `Dependabot` |
| Domain services | `domain::{changes, rules}` | changed items between two snapshots; `require-marker` and `deny-inherited` |
| Application services | `application::{preflight, check, exposed, mark}` | one service per subcommand; they define the traits `Repository` (git), `Workspace` (cargo, rustdoc) and `Forge` (GitHub) |
| Infrastructure | `infrastructure::{git, cargo, rustdoc, github}` | implementations of those traits; `main.rs` wires them and maps results to exit codes |

Preflight (`check` and `exposed`): `rustc -vV` must report a nightly release, otherwise stop with
the `cargo +nightly-...` hint. Then run `rustdoc -Zunstable-options --output-format json` on a
generated one-line crate in a temp dir and compare `format_version` with
`rustdoc_types::FORMAT_VERSION`.

Child processes are plain `cargo`, `rustc` and `rustdoc` from `PATH`. The toolchain comes from
`RUSTUP_TOOLCHAIN`, which `cargo +<nightly>` sets.

## Step 3: check

Port from `public_deps_check.py`, keeping behavior unless the spec or this plan says otherwise:

| Python | Rust |
|---|---|
| `compatibility_key`, `PLAIN_CARET` | `domain::compatibility`, on `semver::VersionReq` with exactly one caret comparator and an empty prerelease (`^2.0.0-rc.1` stays `undecided`) |
| `incompatible_bumps`, `normal_requirements`, `is_published`, `library_name` | `application::check` over `cargo_metadata` types |
| `changed_items` | `domain::changes` |
| `has_marker` | `domain::marker` |
| `foreign_reference_sites`, `local_sites`, `child_ids`, `type_text`, `referenced_positions` | `infrastructure::rustdoc` |
| `Snapshot`, `DependencyGraph`, `rustdoc_json`, `rustc_sysroot`, `is_workspace_dependency` | `infrastructure::cargo` |
| `worktree`, `git` | `infrastructure::git`, worktree removed on drop |

rustdoc JSON is deserialized into `rustdoc_types::Crate`. Every load first compares the
`format_version` field with the supported one. Items, paths, external crates, impls and site
names use the typed model. Only the reference walk stays generic: it serializes `item.inner` to
a `serde_json::Value` and collects every `id` field with its JSON path, as the Python does. A
typed visitor would need an arm per type and would silently miss a field added later.

New compared with the Python, each with its own tests:

- `--base` defaults to `origin/HEAD` and stops when unset. `--head` defaults to `HEAD`. A dirty
  `Cargo.toml` gives a warning.
- Settings from `[workspace.metadata.breaking-bump]` of the merge base, defaults `true`, unknown
  keys are an error.
- Attribution by Cargo package ID. The Python finds the bumps that lead to a changed package by
  package name (`reachable_names`), so a bump that reaches another version of a same-named
  package is blamed, too. The port keeps the package IDs from the artifact mapping. Reachability
  is checked inside each snapshot: the old defining ID against the old graph, the new defining
  ID against the new graph. Across the snapshots nothing changes compared with `changed_items`:
  references are matched by local item, position, foreign path and kind, then the version sets
  are compared per defining package name. IDs are never compared across snapshots, because an
  ID changes with the version and, for path packages, with the worktree path.
- One finding per crate, bumped dependency and changed package, so each has exactly one location
  (`own` or `inherited`). The Python merges all bumps that lead to a package.
- `require-marker`: any marked commit in `merge-base..head` that changes the crate's own
  `Cargo.toml` (`git log --full-history <range> -- <manifest>`). Without `--full-history` git
  follows only one parent of a merge whose manifest is unchanged, and misses a marked commit on
  the other parent. The Python only reads the head commit.
- `deny-inherited` as a separate violation.
- Results `compatible`, `breaking`, `undecided` and `error`. The exit code is not the result:
  0 for `compatible` and for `breaking` without violations, 1 for `breaking` with a violation,
  2 for `undecided`, 3 for `error`. A marker only clears `missing-marker`, so a marked bump is
  `breaking` with exit 0 when no other violation remains. A marked inherited bump still exits 1
  under `deny-inherited`. With both settings `false`, every breaking bump exits 0.
- `GITHUB_OUTPUT` gets `result`, `violations` and `head`. `violations` is the sorted set of
  violation kinds, comma-separated, so each kind appears at most once.
- Target directory `target/breaking-bump`.

## Step 4: exposed

Port `exposed_dependencies`, but on the same artifact-to-package mapping that `check` uses. The
Python matches crates by library name and silently drops crates it does not know. With the
mapping, a crate that cannot be mapped is reported as undecided, as the spec demands.

Further changes: no private list, `-p/--package`, working tree as input, location per
dependency, the crate a dependency is reached through, one example item plus the count of
further items, and exit 2 with the remaining crates still listed.

## Step 5: mark

- `infrastructure::github`: `ureq` client for `GET /pulls/{n}`, `GET /commits/{sha}`,
  `POST /git/commits` and `PATCH /pulls/{n}`. It honors `GITHUB_API_URL`. The token comes from
  `GH_TOKEN` or `GITHUB_TOKEN`.
- Preconditions are one pure domain function over the pull request and the commit. It returns
  the first failed precondition by name. A title that already has the `!` passes.
- Order: create the commit (no author, no committer), add the `!` to the title if missing, push
  with the lease from a temporary bare repo (port of `push_with_lease` and `authorized_env`).
  A title edit starts no workflow run, so the push is the first step that can cancel the job.
- A rejected lease exits 0 and reports that the title was already changed. There is no rollback
  of the title.
- `Bot` trait: recognizes the author, checks the subject format, rewrites subject and title.
  `Dependabot` is the only implementation.

## Step 6: tests

Unit tests port `CompatibilityTest`, `IncompatibleBumpsTest`, `ChangedItemsTest`, `SiteTest`,
`MarkerTest` and the unmapped-crate case. They add tests for the rules, the settings, the
prerelease requirement, the preconditions and the title rewrite.

`mark` runs against a fake `Forge`, with these cases among others:

- A retry against the same head with an already marked title finishes the push.
- The branch moves after the title edit: the lease is rejected, the exit code is 0 and the output
  names the changed title.
- A failed title edit leaves the branch untouched.

Integration tests in `tests/` run the built binary (`CARGO_BIN_EXE_cargo-breaking-bump`) with
`breaking-bump` as first argument, as Cargo does. They assert exit code, output and the
`GITHUB_OUTPUT` file. They port `Fixture` with all its variants and every case of `AnalyzeTest`,
`LeaseTest` and `CredentialTest`. From `ExposureTest` only `test_exposed_dependencies` is
ported, since `test_private_exception` tests the private list that is gone. New cases:

- Two bumps in one pull request that reach different versions of a same-named package: only the
  bump that reaches the changed version is blamed.
- A marked breaking bump with the location `own`, and an unmarked one with both settings
  `false`: `result=breaking`, empty `violations` and exit 0 together.
- A marked breaking bump with the location `inherited` under `deny-inherited`:
  `violations=inherited` and exit 1.
- Two findings that both lack a marker write `violations=missing-marker`. An inherited finding
  next to one that lacks a marker writes `violations=inherited,missing-marker`.
- A merge in the range whose parents have the same manifest, with the marked commit only on the
  second parent: the marker is found.
- `exposed` with one unmapped crate next to a crate that audits fine: exit 2, the second crate is
  still listed.
- One smoke test through `cargo breaking-bump` itself, with the binary's directory on `PATH`.

The child gets `RUSTUP_TOOLCHAIN` set to the nightly derived from `rustc --version`
(`BREAKING_BUMP_TEST_TOOLCHAIN` overrides it) and `CARGO` removed. Verify first that this works
under `cargo test`, since cargo exports the stable toolchain to test processes.

Each test uses its own target directory, because parallel tests would otherwise overwrite each
other's `doc/subject.json`. If the suite is too slow, measure a shared directory with serialized
tests against it.

## Step 7: parity with the Python on real repos

Before the first release, run both tools locally in all seven repos:

- `exposed`: same dependencies per crate as the Python lists.
- `check` on past Dependabot bumps with known outcomes (utoipa 5.5 to 6.0, async-nats 0.50 to
  0.51, and one `workspace = true` case in eventsourced or tellus): same findings.

Expected differences are those listed in steps 3 and 4, in particular the attribution by
package ID, the missing private list and unmapped crates in `exposed`. Any other difference is a
bug.

## Step 8: reusable workflow and scratch repo

`.github/workflows/breaking-bump.yml` with `on: workflow_call`, input `nightly`, secrets
`app-client-id` and `app-private-key`:

- Job `check` ("Check"): `contents: read`, checkout with full history, install the nightly,
  rust-cache, read the version from `Cargo.toml` at `job.workflow_sha`, `cargo install
  cargo-breaking-bump --version <v> --locked`, run `cargo +<nightly> breaking-bump check` with
  the base and head SHA of the pull request. Outputs: `result`, `violations`, `head`.
- Job `mark` ("Mark"): `needs: check`, `permissions: {}`. Its condition is `!cancelled()`,
  `violations == 'missing-marker'` and the author `dependabot[bot]`. Without `!cancelled()`
  GitHub would skip it, because a `missing-marker` violation fails the check job. It creates the
  App token, installs the same version and runs `mark`. It has no checkout step. The check job
  stays failed until the run for the marked commit passes.

Before the first run, verify that every installation of the GitHub App has "Pull requests:
write", which the title edit needs.

A scratch repo (ask before creating it) tests the flow before anything is published. It has a
small library crate, Dependabot without groups, several outdated dependencies in the public API,
the GitHub App and the concurrency settings of the consumers (`cancel-in-progress: true`).
Several dependencies give several pull requests, and `@dependabot recreate` gives a fresh
unmarked one when more are needed. It calls the reusable
workflow on a temporary branch of breaking-bump. On that branch the install step builds from
source: `cargo install --git` with the repository and commit from `job.workflow_repository` and
`job.workflow_sha`. Calling the real reusable workflow keeps the check name as it will be later
and exercises `job.workflow_sha`. The scratch repo answers these questions:

1. Is the marked commit verified (`verification.verified`)?
2. Does Dependabot still rebase a pull request after `mark` rewrote its commit?
3. Does the whole flow work under cancellation: Dependabot pull request, failed check, marked
   title and commit, the push cancels the old run, the new run passes?
4. What is the exact name of the status check? Expected: "Breaking Bump / Check".

The temporary branch is deleted afterwards.

## Step 9: release 0.1.0 and measure the install

README: purpose, install, the three subcommands, settings, the workflow snippet, the squash-merge
note from the spec, the note about a title that keeps its `!`, limitations. Then release with
release-plz as in api-version.

With 0.1.0 on crates.io, the scratch repo pins the workflow to the released SHA, as the consumers
will. The pull request from step 8 is already marked, so its run would skip the mark job. Every
run that has to exercise `mark` therefore uses a fresh, unmarked Dependabot pull request. These
runs check the install from crates.io in both jobs and measure `cargo install`, cold and warm,
in the check job and in the mark job. The numbers decide between plain install, `actions/cache`
and prebuilt binaries (`taiki-e/install-action`).

## Step 10: migrate the seven repos

api-version first, tellus last. One pull request with one commit per repo
(`ci: replace public-deps-check with breaking-bump`):

- `ci.yml`: the jobs `public-deps` and `public-deps-mark` become one job "Breaking Bump" that
  calls `hseeberger/breaking-bump/.github/workflows/breaking-bump.yml@<sha> # v0.1.x` with the
  nightly and the two secrets. The job keeps `if: github.event_name == 'pull_request'` and
  `needs: toolchain` from the old `public-deps` job, because the CI workflows also run on pushes
  and manual dispatch, where no pull request SHAs exist.
- `justfile`: the recipe `public-deps` becomes `breaking-bump`, running
  `cargo +{{ nightly }} breaking-bump check`.
- No settings table. The defaults give today's behavior.

Cutover per repo:

1. Confirm that the ruleset requires branches to be up to date
   (`strict_required_status_checks_policy`). Then every other open pull request must be updated
   after the merge anyway, and that run produces the new check.
2. See "Breaking Bump / Check" green on the migration pull request.
3. Swap "Check Public Dependencies" for the new check in the ruleset.
4. Merge.

## Step 11: follow-up

- `rust:bump` skill (`claude-plugins/plugins/rust/skills/bump/SKILL.md`): if the new nightly has
  another rustdoc `format_version`, bump and release breaking-bump first. Each consumer then gets
  the toolchain and the workflow pin in the same commit. That is the one exception to "only
  `rust-toolchain.toml` changes".
- Point the README of public-deps-check to breaking-bump and archive the repo.
- Delete the memory `breaking-bump-mark-author-experiment`. Its open question is answered (no
  custom author), and this plan holds what is left to observe.

## Verification

- `just all` in breaking-bump: check, fmt, lint, test, doc. All ported Python cases and the new
  cases pass.
- Step 7: same results as the Python in all seven repos, apart from the expected differences.
- Step 8: the four questions answered with observed results, before the release.
- Step 9: the install from crates.io works, with measured cold and warm times.
- Step 10: in each migrated repo the new check is green on the migration pull request, and the
  next breaking Dependabot pull request gets marked.
