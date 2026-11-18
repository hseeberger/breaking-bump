# breaking-bump specification

breaking-bump detects when a dependency that appears in the public API of a published Rust crate
is bumped to a semver-incompatible version. Neither cargo-semver-checks nor Dependabot flag such
a bump as breaking, so a release tool would otherwise propose a non-breaking release.

## Use cases

1. **Pull request check.** CI checks every pull request. A semver-incompatible bump of a
   dependency that appears in the public API of a published crate is reported, and the settings
   of the repository decide whether the check fails.
2. **Marking bot pull requests.** When Dependabot opens such a pull request without a breaking
   marker, the marker is added automatically. Nobody amends the bot's commit by hand, and no job
   builds the pull request's code while holding write credentials.
3. **Local check.** Before pushing, a developer gets the same answer CI would give.
4. **Audit.** A maintainer lists, per published crate, the dependencies that appear in its public
   API.

## Interface

breaking-bump is a Cargo subcommand, `cargo breaking-bump`, with the subcommands `check`, `mark`
and `exposed`.

### Toolchain

breaking-bump has no option to select a toolchain. It runs plain `cargo` and `rustc` and so uses
the toolchain it was started with. The caller selects a nightly with `cargo +<nightly>
breaking-bump ...`; the repository, not breaking-bump, decides which nightly that is.

`check` and `exposed` start with a preflight that always runs:

- Started on a non-nightly toolchain, breaking-bump stops right away with a hint to run it with
  `cargo +nightly-...`.
- It generates rustdoc JSON for a tiny generated crate and compares the `format_version` with the
  one it supports. On a mismatch it stops and names both versions.

The preflight runs before anything else, so a toolchain bump that changes no `Cargo.toml` still
fails when the format no longer matches.

### Settings

`check` reads its settings from the workspace manifest:

```toml
[workspace.metadata.breaking-bump]
require-marker = true
deny-inherited = true
```

Both default to `true`. A repository that only wants the findings reported sets both to `false`.
The settings come from the merge base, not from the head, so a pull request cannot weaken its own
check.

## check

### Input

- A git repository with a Cargo workspace, the current directory by default.
- A base commit, `--base`, by default `origin/HEAD`. If that is not set, `check` stops and asks for
  `--base`. `check` never fetches.
- A head commit, `--head`, by default `HEAD`.

The head is compared with the merge base of base and head. Both commits are checked out as
temporary git worktrees. The working tree is never touched, and uncommitted changes do not count.
If a `Cargo.toml` in the working tree has uncommitted changes, `check` warns about it.

### What is checked

- Only published library crates (`publish` not `false`) that exist in both commits.
- Only normal dependencies, including optional and target-specific ones. Dev and build
  dependencies are not checked. Dependencies on other members of the workspace are ignored.
- A bump is incompatible when the leftmost nonzero version part of the requirement changes. A
  changed requirement that is not a plain caret requirement makes the result `undecided`.
- Only crates with an incompatible bump are documented with rustdoc. A pull request that changes
  no `Cargo.toml` needs no rustdoc run.
- rustdoc runs with all features of the crate enabled, so optional dependencies are checked. A
  crate that does not build with all features makes the result `error`.
- A bump is breaking when an item that is public in both commits now comes from an incompatible
  version of the bumped dependency, or of a crate reached through it. Each item is mapped to the
  exact package version that defines it. A crate that cannot be mapped makes the result
  `undecided`.

Each breaking finding has a location: `own` when the crate declares the dependency in its own
manifest, `inherited` when it declares it with `workspace = true`.

### Rules

- With `require-marker`, every breaking finding needs a marked commit between the merge base and
  the head that changes the affected crate's own `Cargo.toml`. A commit is marked when its subject
  has a `!` (`type!:` or `type(scope)!:`) or its message has a `BREAKING CHANGE:` footer. A
  finding without one is a `missing-marker` violation.
- With `deny-inherited`, every breaking finding with the location `inherited` is an `inherited`
  violation, marked or not. A release tool that assigns commits to crates by directory, such as
  release-plz, never attributes the bump to the crate, so a marker cannot help.

Without `require-marker`, `check` does not read commit messages at all.

### Result and exit codes

The result is one of:

- `compatible`: no breaking finding.
- `breaking`: at least one breaking finding.
- `undecided`: breaking-bump cannot decide.
- `error`: anything else, such as a failed build or a missing git object.

The exit code is:

- 0 for `compatible`, and for `breaking` without violations.
- 1 for `breaking` with at least one violation.
- 2 for `undecided`.
- 3 for `error`.

### Output

For each breaking finding, `check` prints the crate, the bumped dependency with its old and new
requirement, the package whose version changed with both versions, up to three public items that
carry it, the location and any violation with what to do about it.

When `GITHUB_OUTPUT` is set, `check` also writes the result, the violations that occurred and the
checked head commit to it.

### Constraints

- `check` needs no secrets. It builds the dependencies of the commits it checks, and their build
  scripts run arbitrary code.
- It uses `target/breaking-bump` as its target directory, or `CARGO_TARGET_DIR` when set, so
  repeated runs and CI caches reuse the build.

## mark

### Input

- The repository (`owner/name`), the pull request number and the head commit that `check`
  examined.
- A token in the environment, never as an argument, that can create commits and push to the pull
  request branch.

### Preconditions

`mark` checks all of these before it changes anything:

- The pull request comes from a supported bot. For now, that is Dependabot.
- The pull request comes from the same repository, not from a fork.
- Its head is still the examined commit.
- That commit has a single parent and changes a `Cargo.toml`.
- The commit subject is conventional (`type: ...` or `type(scope): ...`) and has no `!` yet.
- The pull request title is conventional, with or without `!`. A title that already has the `!`
  lets a retry finish an interrupted run.

### Action

In this order:

1. It creates a new commit through the GitHub API with the same tree, the same parent and the
   marked subject. The commit has no custom author or committer, so GitHub signs it.
2. It adds the `!` to the pull request title, unless the title already has it. A title edit
   starts no workflow run.
3. It moves the pull request branch to the new commit with `git push --force-with-lease`, leased
   on the examined commit. If the branch has moved meanwhile, it is left alone.

It never checks out or builds the pull request.

### Result and exit codes

- 0 when the pull request was marked, or when its head is no longer the examined commit. That
  holds wherever the move is detected, before any change or at the push. After a rejected push
  the output says that the title was already changed.
- 1 when a precondition fails. The output names it.
- 3 for errors.

### Extensibility

Everything bot-specific (recognizing the author, the subject format and how to rewrite it) sits
behind one interface, so other bots such as Renovate can be added. The only supported forge is
GitHub.

## exposed

### Input

- The workspace as it is in the working tree, uncommitted changes included.
- Optionally `-p`/`--package <name>`, repeatable, to restrict the audit to certain crates.

### Output

For each published library crate, `exposed` lists every normal dependency that appears in the
public API, directly or through a crate reached through it. In the second case it names that
crate, too. For each dependency it prints the requirement, the location (`own` or `inherited`), an
example item and the number of further items. Dependencies that do not appear in the public API
are not listed.

### Exit codes

- 0 when the audit is complete.
- 2 when a crate cannot be mapped to a package. That crate is reported as undecided, and the rest
  is still listed.
- 3 for errors.

## GitHub integration

A reusable workflow in the breaking-bump repository contains both jobs, so a repository calls it
instead of copying them:

- The check job checks out the pull request, installs the given nightly and the version of
  breaking-bump that `Cargo.toml` declares at the workflow's own commit, and runs `check`. It
  gets no secrets and only `contents: read`.
- The mark job runs only when the check reports `missing-marker` as its only kind of violation and
  the pull request comes from Dependabot. It gets a GitHub App token, which also makes the new
  commit start CI again. It never checks out the pull request.

The workflow takes the nightly as input and the GitHub App's client ID and private key as secrets.
An optional input names the tools the build needs beyond Rust, such as `protoc`.

The documentation must say:

> With a squash merge, GitHub builds the commit message anew: depending on the repository
> settings, the subject comes from the pull request title or from the commit. `mark` adds the `!`
> to both. If you edit the title, or the message in the merge dialog, keep the `!`. Without it,
> the breaking bump reaches `main` unmarked and your release tool proposes a non-breaking release.

It must also say:

> `mark` adds the `!` to the pull request title before it moves the branch. If the branch moves in
> between, the title keeps a `!` for a commit that `mark` never marked. Check the title before a
> squash merge.

## Distribution

- The crate is published to crates.io as `cargo-breaking-bump`.
- The reusable workflow installs the version that `Cargo.toml` declares at the workflow's own
  commit (`job.workflow_sha`), so a caller that pins the workflow also pins the tool. The workflow
  file holds no version number.

## Implementation

- breaking-bump is written in Rust.
- It reads rustdoc JSON with `rustdoc-types`. Each release supports exactly one `format_version`.

## Limitations

- With squash merges, `check` cannot see the final commit message, so `require-marker` only
  covers merge commits and rebase merges.
- A crate whose features exclude each other cannot be checked, because rustdoc runs with all
  features enabled.
- Dependabot no longer rebases a pull request that `mark` changed. Comment `@dependabot recreate`
  instead: Dependabot rebuilds the commit unmarked, and the workflow marks it again.

## Out of scope

- A release check that compares the last release tag with `main`.
- Bots other than Dependabot, and forges other than GitHub.
- A list of dependencies to ignore.
- A composite GitHub Action; the reusable workflow is the only GitHub integration.
