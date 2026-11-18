# breaking-bump

**Detect dependency bumps that break the public API of your Rust crates.**

[![Crates.io][crates-badge]][crates-url]
[![license][license-badge]][license-url]
[![build][build-badge]][build-url]

[crates-badge]: https://img.shields.io/crates/v/cargo-breaking-bump
[crates-url]: https://crates.io/crates/cargo-breaking-bump
[license-badge]: https://img.shields.io/github/license/hseeberger/breaking-bump
[license-url]: https://github.com/hseeberger/breaking-bump/blob/main/LICENSE
[build-badge]: https://img.shields.io/github/actions/workflow/status/hseeberger/breaking-bump/ci.yml?branch=main
[build-url]: https://github.com/hseeberger/breaking-bump/actions/workflows/ci.yml

A dependency that appears in the public API of a crate is part of that API. Bumping it to a
semver-incompatible version, for example utoipa 5.5 to 6.0 or async-nats 0.50 to 0.51, breaks the
API of the crate, although not a single line of its own code changes. Dependabot does not mark
such a bump as breaking and cargo-semver-checks does not detect it, so a release tool like
release-plz proposes a non-breaking release.

breaking-bump detects these bumps. It is a Cargo subcommand, and it comes with a reusable GitHub
workflow that checks every pull request and adds the breaking marker to Dependabot pull requests.

## The problem, on a real pull request

With its `axum-utoipa` feature, the error type of
[error-ext](https://github.com/hseeberger/error-ext) implements `utoipa::ToSchema`, so utoipa is
part of its public API. In
[error-ext#275](https://github.com/hseeberger/error-ext/pull/275) Dependabot bumped utoipa from
5.5.0 to 6.0.0 with the subject `build(deps): bump utoipa in the cargo-major group across 1
directory`, without a `!`. One minute after the merge, release-plz proposed
[error-ext 0.4.6 -> 0.4.7](https://github.com/hseeberger/error-ext/pull/277) and labeled it "API
compatible changes".

That release was never published. If it had been, a crate using error-ext 0.4 together with
utoipa 5 would have received 0.4.7 with a plain `cargo update`. From then on the error type would
implement the `ToSchema` of utoipa 6, a different trait than the `ToSchema` of utoipa 5, and the
crate would no longer compile wherever it uses the two together.

This is what breaking-bump 0.1.1 prints for the commit of that pull request:

```console
$ cargo +nightly-2026-11-09 breaking-bump check --base efa16ab^ --head efa16ab
error-ext: bumping utoipa from ^5.5 to ^6.0 (own) changes utoipa from 5.5.0 to 6.0.0 in the public API: `utoipa::ToSchema` in `impl utoipa::ToSchema for error_ext::axum::Error`, `utoipa::openapi::RefOr` in `impl utoipa::ToSchema for error_ext::axum::Error::schemas`, `utoipa::openapi::schema::Schema` in `impl utoipa::ToSchema for error_ext::axum::Error::schemas` and 5 more
  missing-marker: no commit since the merge base that changes Cargo.toml is marked; mark one with `!` (`type!:` or `type(scope)!:`) or a `BREAKING CHANGE:` footer
result: breaking
$ echo $?
1
```

The first line is a **finding**: a bump, the package whose version changed in the public API and
the items that carry it. `(own)` is the **location** of the finding: error-ext declares utoipa in
its own `Cargo.toml`. The second line is a **violation**: the finding lacks a **marker**, the `!`
or the `BREAKING CHANGE:` footer of a conventional commit that tells a release tool about a
breaking change. In CI, the workflow would have added the `!` to that Dependabot pull request by
itself.

## Highlights

- **The real public API**: breaking-bump reads rustdoc JSON, so it finds a dependency in function
  signatures, struct and variant fields, trait implementations, generic bounds and arguments,
  re-exports and the output of derives.
- **Exact package versions**: every item is mapped to the Cargo package that defines it. A bump is
  only breaking when an item that is public in both commits moves to an incompatible version, be
  it of the bumped dependency or of a crate reached through it.
- **No alarm for private dependencies**: a bump of a dependency that stays out of the public API
  is `compatible`, however large it is.
- **Rules a pull request cannot weaken**: the settings are read from the merge base.
- **Marked Dependabot pull requests**: `mark` adds the `!` to the commit and to the title. GitHub
  signs the commit, and a branch that moved in the meantime is left alone.
- **Secrets kept away from untrusted code**: the check job builds the pull request and gets no
  secrets. The mark job has a token and never checks out or builds the pull request.
- **No guessing**: what the analysis cannot resolve with certainty is `undecided`, with its own
  exit code.

## Getting started

### In CI

Call the reusable workflow from the workflow that runs on your pull requests:

```yaml
jobs:
  toolchain:
    name: Setup Rust toolchain
    runs-on: ubuntu-latest
    outputs:
      nightly: ${{ steps.set_toolchain.outputs.nightly }}
    steps:
      - name: Checkout repository
        uses: actions/checkout@<sha> # vX.Y.Z

      - name: Output toolchain
        id: set_toolchain
        run: |
          nightly=$(rustc --version | grep -oE '[0-9]{4}-[0-9]{2}-[0-9]{2}' | sed 's/^/nightly-/')
          echo "nightly=$nightly" | tee -a "$GITHUB_OUTPUT"

  breaking-bump:
    name: Breaking Bump
    if: github.event_name == 'pull_request'
    needs: toolchain
    uses: hseeberger/breaking-bump/.github/workflows/breaking-bump.yml@<sha> # vX.Y.Z
    permissions:
      contents: read
    with:
      nightly: ${{ needs.toolchain.outputs.nightly }}
    secrets:
      app-client-id: ${{ secrets.APP_CLIENT_ID }}
      app-private-key: ${{ secrets.APP_PRIVATE_KEY }}
```

The `toolchain` job derives the nightly from the date of the stable toolchain that
`rust-toolchain.toml` pins. A fixed value such as `nightly: nightly-2026-11-09` works as well; see
[Supported toolchains](#supported-toolchains). If the build needs tools beyond Rust, such as
`protoc`, pass them as `tools`, comma-separated, to be installed with taiki-e/install-action.

The workflow has two jobs:

- **Check** runs `check` on the pull request. It gets no secrets and only `contents: read`. It
  installs the version of breaking-bump that `Cargo.toml` declares at the workflow's own commit,
  so pinning the workflow also pins the tool. Make "Breaking Bump / Check" a required status
  check.
- **Mark** runs `mark` when `missing-marker` is the only kind of violation and the pull request
  comes from Dependabot. It never checks out the pull request.

The mark job needs a GitHub App, because a commit pushed with the token of a GitHub App starts CI
again, which a commit pushed with the workflow's own token does not:

1. Create a GitHub App with the repository permissions "Contents: Read and write" and "Pull
   requests: Read and write", and install it on the repository.
2. Store its client ID and a private key as the secrets `APP_CLIENT_ID` and `APP_PRIVATE_KEY`.
   For Dependabot pull requests they must be Dependabot secrets.

Without the two secrets the check job works as described. The mark job then fails for a
Dependabot pull request that needs the marker, and the pull request stays unmarked.

With a squash merge, GitHub builds the commit message anew: depending on the repository settings,
the subject comes from the pull request title or from the commit. `mark` adds the `!` to both. If
you edit the title, or the message in the merge dialog, keep the `!`. Without it, the breaking
bump reaches `main` unmarked and your release tool proposes a non-breaking release.

`mark` adds the `!` to the title before it moves the branch. If the branch moves in between, the
title keeps a `!` for a commit that `mark` never marked. Check the title before a squash merge.

### Locally

```sh
cargo install cargo-breaking-bump --locked
cargo +nightly-2026-11-09 breaking-bump check
```

`check` gives the answer CI would give, for the commits of the current branch. `exposed` lists the
dependencies in the public API of your crates, which is a good first look at a workspace:

```sh
cargo +nightly-2026-11-09 breaking-bump exposed
```

## Supported toolchains

breaking-bump reads rustdoc JSON, which only a nightly toolchain emits. Each release supports
exactly one rustdoc JSON format version:

| breaking-bump | rustdoc JSON format | Known-good nightly   |
| ------------- | ------------------- | -------------------- |
| 0.1           | 61                  | `nightly-2026-11-09` |

Before anything else, `check` and `exposed` verify that they run on a nightly and that its rustdoc
JSON format is the supported one. Otherwise they stop and name both format versions.

## check

```sh
cargo +nightly-2026-11-09 breaking-bump check [--base <commit>] [--head <commit>]
```

`check` compares the head commit (by default `HEAD`) with the merge base of base (by default
`origin/HEAD`) and head. It checks out both commits as temporary git worktrees, so uncommitted
changes do not count; if a `Cargo.toml` has uncommitted changes, `check` warns about it. It never
fetches.

Only published library crates and their normal dependencies are checked, including optional and
target-specific ones. Dependencies on other members of the workspace are ignored. A bump is
incompatible when the leftmost nonzero part of the version requirement changes. rustdoc runs only
for crates with such a bump, on both commits and with all features enabled. A bump is breaking
when an item that is public in both commits now comes from an incompatible version of the bumped
dependency, or of a crate reached through it.

Each breaking finding has a location: `own` when the crate declares the dependency in its own
manifest, `inherited` when it declares it with `workspace = true`. Two rules decide whether a
finding is a violation:

- `require-marker`: some commit since the merge base that changes the crate's own `Cargo.toml`
  must be marked as breaking, with a `!` in its subject (`type!:` or `type(scope)!:`) or with a
  `BREAKING CHANGE:` footer. Otherwise the finding is a `missing-marker` violation.
- `deny-inherited`: an `inherited` finding is an `inherited` violation, marked or not. A release
  tool that assigns commits to crates by directory, like release-plz, never sees such a bump.

Both rules are on by default. A repository that only wants the findings reported turns them off
in its workspace manifest. The settings are read from the merge base, so a pull request cannot
weaken its own check.

```toml
[workspace.metadata.breaking-bump]
require-marker = false
deny-inherited = false
```

The result is `compatible`, `breaking`, `undecided` or `error`, with these exit codes:

| Exit code | Meaning                                                    |
| --------- | ---------------------------------------------------------- |
| 0         | `compatible`, or `breaking` without violations             |
| 1         | `breaking` with at least one violation                     |
| 2         | `undecided`: breaking-bump cannot decide with certainty    |
| 3         | `error`                                                    |

When `GITHUB_OUTPUT` is set, `check` also writes `result`, `violations` (the kinds of violations,
comma-separated) and `head` (the checked commit) to it.

`check` builds the dependencies of the commits it checks, and their build scripts run arbitrary
code. Run it without secrets. It uses `target/breaking-bump` as its target directory, or
`CARGO_TARGET_DIR` when set.

### When the result is undecided

breaking-bump prefers `undecided` to a guess. The output names the reason after `cannot decide:`.
These are the cases:

- A changed requirement is not a plain caret requirement, for example `~1.2`, `=1.2.3`,
  `>=1, <3`, `*` or the prerelease `^2.0.0-rc.1`. Writing the requirement as a plain version
  (`1.2`) resolves this case.
- A crate declares several different requirements for the same dependency.
- A crate that appears in the public API cannot be mapped to a Cargo package.
- Two public items with the same name refer to the same path in different crates.
- An item changed to an incompatible version, but no bumped dependency leads to it.

Apart from the first case, a human has to judge the bump. Please open an issue with the output,
since each of these cases marks a limit of the analysis.

## exposed

```sh
cargo +nightly-2026-11-09 breaking-bump exposed [-p <crate>]...
```

`exposed` lists, for each published library crate of the working tree, the normal dependencies
that appear in its public API, with their requirement, their location, an example item and the
number of further items. A dependency that exposes items of a crate reached through it names that
crate, too. It exits with 0 when the audit is complete, with 2 when a crate cannot be mapped to a
package (the rest is still listed) and with 3 for errors.

## mark

```sh
cargo breaking-bump mark --pr <number> --head <commit> [--repository <owner/name>]
```

`mark` adds the `!` to a Dependabot pull request whose head commit `check` examined. It reads a
token from `GH_TOKEN` or `GITHUB_TOKEN` and the repository from `GITHUB_REPOSITORY` unless given.
It never checks out or builds the pull request, and it needs no nightly.

It only acts on a pull request from Dependabot in the same repository, with a single-parent head
commit that changes a `Cargo.toml` and has a conventional, unmarked subject, and with a
conventional title. Then it creates the marked commit through the GitHub API, without a custom
author or committer, so GitHub signs it. It adds the `!` to the pull request title unless the
title already has it. Finally it moves the branch with `git push --force-with-lease`, leased on
the examined commit.

It exits with 0 when the pull request was marked, or when its head is no longer the examined
commit. It exits with 1 when a precondition fails and with 3 for errors.

## Related tools

- [cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) compares the API of a
  crate with its last release. It does not detect a bump of a dependency in the public API; see
  its issue [#447](https://github.com/obi1kenobi/cargo-semver-checks/issues/447). release-plz
  uses it for its API check, which is why the proposal above says "API compatible changes".
- [semwave](https://crates.io/crates/semwave) also reads rustdoc JSON. Given the dependency bumps
  between two git refs, it works out which crates of a workspace need a major, minor or patch
  bump. breaking-bump puts the result where a release tool reads it: it requires the marker in
  the commit history and adds it to Dependabot pull requests.
- Cargo's public and private dependencies
  ([RFC 3516](https://rust-lang.github.io/rfcs/3516-public-private-dependencies.html)) are still
  unstable. They let a crate declare which dependencies are public, the information `exposed`
  computes today, and lint against exposing the others.

## Used by

The workflow checks the pull requests of
[api-version](https://github.com/hseeberger/api-version),
[composed](https://github.com/hseeberger/composed),
[configured](https://github.com/hseeberger/configured),
[error-ext](https://github.com/hseeberger/error-ext),
[eventsourced](https://github.com/hseeberger/eventsourced),
[pub-sub-client](https://github.com/hseeberger/pub-sub-client) and
[tellus](https://github.com/tellus-rs/tellus).

## Limitations

- With squash merges, `check` cannot see the final commit message, so `require-marker` only
  covers merge commits and rebase merges.
- rustdoc runs with all features enabled. A crate whose features exclude each other cannot be
  checked; the result is `error`.
- Only Dependabot pull requests on GitHub can be marked.
- Dependabot no longer rebases a pull request that `mark` changed. Comment `@dependabot recreate`
  instead: Dependabot rebuilds the commit unmarked, and the workflow marks it again.

## Documentation

- [docs/spec.md](docs/spec.md): the specification, with every input, rule, result and exit code.
- [docs/design.md](docs/design.md): how breaking-bump works and why, from the two snapshots of a
  check down to the order in which `mark` changes a pull request.

## Development

The [justfile](justfile) defines the usual tasks; `just all` runs check, fmt, lint, test and doc.
Formatting needs `taplo` for TOML and runs on nightly, since the rustfmt configuration uses
unstable options. The integration tests build real git repositories and crates in temporary
directories and run the built binary on them. They need the nightly whose date matches the pinned
stable toolchain; `BREAKING_BUMP_TEST_TOOLCHAIN` overrides it.

## Contributing

Contributions are welcome; [CONTRIBUTING.md](CONTRIBUTING.md) describes how to build, test and
submit them.

## License

This code is open source software licensed under the
[Apache 2.0 License](http://www.apache.org/licenses/LICENSE-2.0.html).
