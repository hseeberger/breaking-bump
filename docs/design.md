# How breaking-bump works

[spec.md](spec.md) says what breaking-bump does. This document says how it does it and why,
top-down, with links into the implementation.

## The question

`check` answers one question: does an item that is public in both commits now come from a
semver-incompatible version of a package? Answering it takes four things: the public API of a
crate, the package behind every foreign item in it, a way to recognize the same item in two
builds, and the bump to blame for a change.

A wrong answer is expensive in both directions. A missed breaking bump ships a non-breaking
release, and a false alarm teaches people to ignore the check. Whatever the analysis cannot
resolve with certainty is therefore reported as `undecided`, never guessed. `Undecided`
([domain/mod.rs](../src/domain/mod.rs)) travels as an error through the infrastructure and the
application services, which turn it into a result of its own with its own exit code.

## The steps of a check

[application/check.rs](../src/application/check.rs) runs the cheap steps first and stops as soon
as the answer is known:

1. If no `Cargo.toml` changed between the merge base and the head, the result is `compatible`.
   Nothing is checked out or built.
2. Both commits are checked out as temporary git worktrees
   ([infrastructure/git.rs](../src/infrastructure/git.rs)). The working tree is never touched, and
   a worktree is removed when its handle is dropped.
3. `cargo metadata --no-deps` on both worktrees gives the requirements of the published library
   crates. A bump whose compatibility key changes
   ([domain/compatibility.rs](../src/domain/compatibility.rs),
   [domain/bump.rs](../src/domain/bump.rs)) is incompatible. Without such a bump the result is
   `compatible`, still without a build.
4. Only now rustdoc runs, and only for the crates with an incompatible bump, once per commit.
5. The references of both snapshots are compared, the changes are attributed to bumps and the
   rules are applied.

Both worktrees share one target directory, so the second rustdoc run and later checks reuse most
of the build.

## From rustdoc JSON to Cargo packages

rustdoc JSON describes the public API of one crate. Its `external_crates` name every other crate
it refers to, but only by crate name and by the path of the compiled artifact. A crate name is not
enough: a build can contain two versions of the same crate, and that is exactly the situation a
breaking bump creates.

Cargo knows the missing part. With `--message-format json`, `cargo rustdoc` reports for every
compiled package the files it produced. [infrastructure/cargo.rs](../src/infrastructure/cargo.rs)
joins the two by the canonical path of the artifact: the path in `external_crates` leads to the
Cargo package ID, and the package ID to the name and the exact version. Crates of the standard
library, recognized by name or by a path below the sysroot, are skipped. A crate that cannot be
mapped makes the result `undecided`.

rustdoc runs with `--all-features`, so optional dependencies are part of the documented API.

## The reference walk

A foreign item can sit anywhere in an item of the public API: in a return type, a field, a
generic bound, a trait of an `impl`, the arguments of a path.
[infrastructure/rustdoc.rs](../src/infrastructure/rustdoc.rs) does not visit these places type by
type. It serializes the `inner` of each local item to JSON and collects every `id` together with
the JSON path that leads to it. That path is the position of the reference.

The walk is generic on purpose. A typed visitor needs one arm per type of the rustdoc model, and
it silently misses a field that a later format version adds. The generic walk sees the new field
without a change.

Blanket and synthetic implementations are skipped, since the crate does not write them.

## Recognizing an item in two builds

The IDs in rustdoc JSON are only valid within one file, so the two snapshots are matched by what
a reference looks like. A `Reference` ([domain/reference.rs](../src/domain/reference.rs)) consists
of:

- the site, a readable name of the item that holds the reference: its path, `impl Trait for Type`
  for an implementation, and the name of the parent as a prefix for fields, variants and methods,
- the position within that item,
- the path and the kind of the foreign item.

Two different items can have the same site, for example two `impl` blocks for the same type. If
such items refer to the same path in different crates, the match would be ambiguous, and the
result is `undecided`.

A reference that exists in both snapshots is compared by the name and the version of the package
that defines the foreign item ([domain/changes.rs](../src/domain/changes.rs)). Package IDs take no
part in this comparison. They contain the version and, for path dependencies, the path of the
worktree, so they always differ between the snapshots. Within one snapshot they are exact, and
that is where they are used.

## Blaming a bump

A changed package is often not the bumped dependency itself. A crate may expose `dep_c::Thing`
only because `dep_b` re-exports it, and a bump of `dep_b` then changes `dep_c` in the public API.
A pull request can also contain several bumps.

For every change, the bumps of the crate are searched for the ones whose dependency reaches the
changed package in the graph of normal dependencies. The new package is looked up in the new
snapshot and the old one in the old snapshot, each by its package ID. A bump that reaches both is
preferred to one that reaches only the new package. If no bump reaches the change, the result is
`undecided`.

There is one finding per crate, bumped dependency and changed package. Each finding therefore
has exactly one location, `own` or `inherited`.

## Rules and markers

The settings come from the workspace manifest of the merge base
([domain/settings.rs](../src/domain/settings.rs)). A pull request that turns a rule off does not
weaken its own check, and an unknown key is an error, so a misspelled setting does not turn a
rule off unnoticed.

`require-marker` looks for a marked commit between the merge base and the head that changes the
manifest of the affected crate. It uses `git log --full-history`. Without it, git simplifies the
history: at a merge whose parents have the same manifest it follows only one of them, and a
marked commit on the other parent is not found.

`deny-inherited` exists because of how release tools assign commits to crates. release-plz
assigns by directory. A bump of a dependency declared with `workspace = true` changes the
workspace manifest and not the directory of the crate, so the crate is never told, with or
without a marker.

## Marking a pull request

`mark` ([application/mark.rs](../src/application/mark.rs)) must not make things worse than they
are. All preconditions are checked before anything changes
([domain/pull_request.rs](../src/domain/pull_request.rs)): the pull request comes from the bot and
from the same repository, its head is still the examined commit, and that commit has one parent,
changes a `Cargo.toml` and has a conventional subject without a marker.

Then three things happen, in this order:

1. The marked commit is created through the GitHub API, with the same tree and parent. It has no
   custom author or committer, because GitHub signs a commit created through the API only
   then, and a ruleset that requires signed commits rejects an unsigned one.
2. The `!` is added to the pull request title, unless it is already there.
3. The branch is moved with `git push --force-with-lease`, leased on the examined commit.

The title comes before the push because of what each step triggers. A title edit starts no
workflow run. The push starts one, and if the caller's workflow uses `cancel-in-progress`, that
new run cancels the run `mark` is part of. If the title came last, a canceled run could leave a
marked commit with an unmarked title. In the chosen order an interrupted run leaves at most a marked title, and since
a marked title passes the preconditions, a retry finishes the job.

The lease protects a branch that moved while `mark` was running, for example by a rebase of
Dependabot. A rejected lease is not an error: the pull request has a new head, which gets its
own check.

The token never appears on a command line. It reaches git through `GIT_CONFIG_*` environment
variables as an HTTP header, and git's error output is reported without it.

Everything that is specific to a bot (recognizing the author, the format of the subject and how
to rewrite it) is behind the `Bot` trait ([domain/bot.rs](../src/domain/bot.rs)). `Dependabot` is
its only implementation.

## The workflow

The reusable workflow
([.github/workflows/breaking-bump.yml](../.github/workflows/breaking-bump.yml)) splits the work
into two jobs, so that code from the pull request and credentials never meet.

- The check job builds the dependencies of the pull request, and their build scripts run
  arbitrary code. It gets no secrets and only `contents: read`.
- The mark job has the token of a GitHub App, which can push. It talks to the GitHub API and to
  git only, and never checks out the pull request.

The workflow file contains no version number. Each job reads the version from `Cargo.toml` at the
commit of the workflow itself and installs that version from crates.io. A caller that pins the
workflow to a commit thereby pins the tool, and a release needs no change to the workflow.

## Structure of the code

The code follows the onion architecture, one module per ring:

- [domain](../src/domain): the model and the domain services. Compatibility keys, bumps,
  references, changes, findings, violations, settings, rules, markers and the preconditions of
  `mark`. No I/O.
- [application](../src/application): one application service per subcommand, plus the preflight.
  They define the traits they need from the outside: `Cargo` and `Snapshot`, `Repository` and
  `Worktree`, `Forge`, `Toolchain`.
- [infrastructure](../src/infrastructure): the implementations of those traits on git, Cargo,
  rustdoc JSON and the GitHub REST API.
- [main.rs](../src/main.rs): the command line, the wiring, the output and the exit codes.

The unit tests of the application services run against fakes of these traits. The integration
tests in [tests](../tests) build real git repositories and crates and run the binary on them.

## rustdoc JSON format versions

rustdoc JSON is unstable, and its format version changes with nightlies. Each release of
breaking-bump supports exactly one format version, the one of its `rustdoc-types` dependency. The
preflight ([application/preflight.rs](../src/application/preflight.rs)) documents a one-line
crate and compares its format version with the supported one before anything else, so a
mismatch is reported as such and not as a parse error somewhere in the middle of a check.
