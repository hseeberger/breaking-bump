# breaking-bump

breaking-bump detects when a dependency that appears in the public API of a published Rust crate
is bumped to a semver-incompatible version, for example utoipa 5.5 to 6.0 or async-nats 0.50 to
0.51. Neither cargo-semver-checks nor Dependabot flag such a bump as breaking, so a release tool
like release-plz would otherwise propose a non-breaking release.

breaking-bump is a Cargo subcommand. It comes with a reusable GitHub workflow that checks pull
requests and adds the breaking marker to Dependabot pull requests.

## Installation

```sh
cargo install cargo-breaking-bump --locked
```

breaking-bump reads rustdoc JSON, which only a nightly toolchain emits. Each release supports
exactly one rustdoc JSON format version. Run it with a nightly that emits that version, for
example the nightly whose date matches your pinned stable toolchain:

```sh
cargo +nightly-2026-11-09 breaking-bump check
```

Before anything else, `check` and `exposed` verify that they run on a nightly and that its rustdoc
JSON format is the supported one. Otherwise they stop and say why.

## check

```sh
cargo +nightly-2026-11-09 breaking-bump check [--base <commit>] [--head <commit>]
```

`check` compares the head commit (by default `HEAD`) with the merge base of base (by default
`origin/HEAD`) and head. It checks out both commits as temporary git worktrees, so uncommitted
changes do not count. It never fetches.

Only published library crates and their normal dependencies are checked, including optional and
target-specific ones. Dependencies on other members of the workspace are ignored. A bump is
incompatible when the leftmost nonzero part of the version requirement changes. rustdoc runs only
for crates with such a bump, on both commits. A bump is breaking when an item that is public in
both commits now comes from an incompatible version of the bumped dependency, or of a crate
reached through it.

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
It never checks out or builds the pull request.

It only acts on a pull request from Dependabot in the same repository, with a single-parent head
commit that changes a `Cargo.toml` and has a conventional, unmarked subject, and with a
conventional title. Then it creates the marked commit through the GitHub API, without a custom
author or committer, so GitHub signs it. It adds the `!` to the pull request title unless the
title already has it. Finally it moves the branch with `git push --force-with-lease`, leased on
the examined commit.

It exits with 0 when the pull request was marked, or when its head is no longer the examined
commit. It exits with 1 when a precondition fails and with 3 for errors.

## GitHub workflow

The reusable workflow runs `check` on every pull request and, when `missing-marker` is the only
kind of violation and the pull request comes from Dependabot, `mark`:

```yaml
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

The check job gets no secrets and only `contents: read`. It installs the version of breaking-bump
that `Cargo.toml` declares at the workflow's own commit, so pinning the workflow also pins the
tool. The mark job uses a token of a GitHub App with "Contents: write" and "Pull requests: write",
which also makes the marked commit start CI again. For Dependabot pull requests, the two secrets
must be Dependabot secrets. The required status check is named "Breaking Bump / Check".

With a squash merge, GitHub builds the commit message anew: depending on the repository settings,
the subject comes from the pull request title or from the commit. `mark` adds the `!` to both. If
you edit the title, or the message in the merge dialog, keep the `!`. Without it, the breaking
bump reaches `main` unmarked and your release tool proposes a non-breaking release.

`mark` adds the `!` to the title before it moves the branch. If the branch moves in between, the
title keeps a `!` for a commit that `mark` never marked. Check the title before a squash merge.

## Limitations

- With squash merges, `check` cannot see the final commit message, so `require-marker` only
  covers merge commits and rebase merges.
- Only Dependabot pull requests on GitHub can be marked.

## License

Apache-2.0
