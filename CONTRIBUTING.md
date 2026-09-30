# Contributing to blyte

Thanks for helping. blyte has one rule that shapes everything else: **every feature earns its place
in measurement, or it goes.** Read the [manifesto](https://github.com/climate-x-org/blyte/blob/main/MANIFESTO.md) before proposing something large.

## Set up

```bash
git clone https://github.com/climate-x-org/blyte.git blyte
cd blyte
cargo build
cargo install --path crates/blyte-cli --locked   # optional: put `blyte` on your PATH
```

blyte builds on macOS and Linux with a recent stable Rust toolchain.

## The checks

These are exactly what CI runs. A change is ready when all three pass:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-targets
```

The sandbox has platform-specific code. On macOS you can also compile the Linux path (Landlock)
with a cross-check, if you have `zig` installed and the target added
(`rustup target add aarch64-unknown-linux-gnu`); a two-line wrapper that calls
`zig cc -target aarch64-linux-gnu` and drops cargo's `--target=` flag is enough as the C compiler:

```bash
CC_aarch64_unknown_linux_gnu=<your-wrapper> \
  cargo clippy --target aarch64-unknown-linux-gnu --workspace --all-targets -- -D warnings
```

CI runs the Linux tests on Ubuntu, where Landlock is required to be present.

The docs site builds with [MkDocs Material](https://squidfunk.github.io/mkdocs-material/):

```bash
uvx --with mkdocs-material mkdocs serve     # live preview at http://127.0.0.1:8000
uvx --with mkdocs-material mkdocs build --strict
```

## How changes are made here

- **Tests first.** A fix starts with a test that fails for the reason the bug report gives. Test
  files open with a doc comment saying what went wrong and when, so the reason for the test
  survives.
- **Measure claims.** "Faster" or "cheaper" needs a paired comparison: the same task, from the same
  commit, with and without the change (or against another harness), run side by side. Maintainers
  develop blyte this way, with every task implemented by both blyte and a baseline agent and the
  better result merged. A pull request that claims a speed-up should say how it was measured.
- **Small and deletable.** Prefer a mechanical rule to a clever mechanism, and a deletion to an
  addition. If a feature stops paying for itself, remove it.
- **Keep the boundary.** blyte implements tasks. Planning, review and memory across runs belong to
  whatever drives it; proposals to add them will usually be declined.

## Commits and pull requests

- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`,
  `refactor:`, `chore:`, `test:`. The PR title is checked in CI.
- One logical change per PR. Say what changed, why, and how you verified it.
- Update the docs (`docs/`, `README.md`) in the same PR when behaviour or flags change.

## Layout

```
crates/blyte-agent/     the loop, context, tools, shell and fs seams, sessions, search, sandbox
crates/blyte-provider/  model providers and pricing
crates/blyte-cli/       the binary: headless runs, REPL, reports
docs/                   the docs site; research write-ups; architecture decisions
docs/superpowers/       historical plans and specs, as written at the time (some use the
                        harness's former name, superstate)
```

## Security issues

Please do not open a public issue for a vulnerability; see [SECURITY.md](https://github.com/climate-x-org/blyte/blob/main/SECURITY.md).

## Licence

By contributing you agree that your contribution is licensed under the project's licence, the
Apache License 2.0.

## Releases

Every merge to `main` is released automatically (`.github/workflows/release-on-merge.yml`). The
squash-merge title is the commit message, so the pull request title decides the release: `feat:`
bumps the minor version, `fix:`, `refactor:` and `perf:` the patch, and a breaking change (`!` or a
`BREAKING CHANGE:` footer) the minor while the version is 0.x; `docs:`, `ci:`, `chore:`, `test:` and
`style:` do not release. The workflow updates the version and `CHANGELOG.md` through a
`chore(release): vX.Y.Z` pull request, tags `vX.Y.Z`, publishes the GitHub release and attaches the
static binaries. The rules are in `.cz.toml` (commitizen).
