# Contributing to breaking-bump

Thank you for considering a contribution. Bug reports, fixes, documentation and new features are
all welcome.

For anything beyond a small fix, please open an issue first and describe what you want to change.
A report about a wrong result is most useful with the output of `check` and, if possible, the
repository and the two commits it ran on.

## Who contributes

breaking-bump welcomes contributions from people who engage with the project. Using AI tools is
fine, but you are the author: you understand every line you submit, you answer review questions
yourself, and you stay with your pull request until it is merged or closed. Pull requests from
autonomous agents, or from accounts which open pull requests across many unrelated projects
without engaging, are closed without review.

## Building and testing

You need `just`, `taplo` and, next to the stable toolchain pinned in `rust-toolchain.toml`, the
nightly of the same date. `rustc --version` shows that date. Then run the full local gate before
you open a pull request:

```sh
just all
```

`just all` checks, formats, lints, tests and builds the documentation. Use `just fmt` rather than
`cargo fmt`, since the rustfmt configuration needs nightly. The integration tests build real git
repositories and crates in temporary directories and run the built binary on them with that
nightly; `BREAKING_BUMP_TEST_TOOLCHAIN` selects another one.

## Commits and pull requests

A pull request consists of exactly one commit, which CI enforces. Squash your changes and force
push when you address review comments.

The commit message is a single subject line of at most 72 characters, without a body, following
[Conventional Commits](https://www.conventionalcommits.org), for example `fix: find a marker on
the second parent of a merge`. release-plz builds the changelog from these subjects, so the
subject is what users will read.

Sign off your commit with `git commit -s`. The `Signed-off-by` trailer certifies that you may
contribute the change under the project's license, as the
[Developer Certificate of Origin](https://developercertificate.org) states.

## Code and documentation

The [README](README.md) describes the behavior; keep it consistent with your change.
[AGENTS.md](AGENTS.md) describes the architecture and lists the things that are easy to break.

A change in behavior comes with a test. The integration tests in [tests](tests) cover a check end
to end; a new case usually is a new option of the fixture in
[tests/common/mod.rs](tests/common/mod.rs).

## License

breaking-bump is licensed under the [Apache 2.0 License](LICENSE). Every contribution submitted
for inclusion is licensed under the same terms, as section 5 of the license states.
