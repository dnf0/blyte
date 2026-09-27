# Rename the Repositories to Blyte — Plan

**Status:** planned 2026-09-27. Decisions (user): the harness repo becomes **`climate-x-org/blyte`**
(rename in place, keeping history, issues, PRs and CI); the eval repo becomes **`dnfcx/blyte-eval`**
with its Python names renamed too; going public is **planned now and done later**, after a
history secret scan.

**Already done:** the code rename (#61): crates `blyte-agent`, `blyte-provider`, `blyte-cli`,
binary `blyte`, and all live text. crates.io names `blyte`, `blyte-cli`, `blyte-agent` and
`blyte-provider` are reserved (owner `dnf0`, version 0.0.0 placeholders).

**Not renamed, on purpose:** historical plans and specs (`docs/superpowers/plans`, `…/specs`),
`CHANGELOG.md` entries, committed eval results (`results/`), and the eval's `FINDINGS.md` /
`EXPERIMENTS.md` narrative. They are records of what was true at the time.

GitHub keeps a redirect from a renamed repository's old URL (web, git and API), so nothing breaks
at the moment of the rename; the steps below remove the dependence on the redirect.

---

## Phase 1: the harness repository (`climate-x-org/superstate` → `climate-x-org/blyte`)

Outward-facing: confirm with the user immediately before step 1.

1. `gh repo rename blyte --repo climate-x-org/superstate`, and set the description:
   `gh repo edit climate-x-org/blyte --description "A lean coding-agent harness in Rust: one loop, a sealed verification gate, sandboxed tools." --add-topic coding-agent --add-topic rust --add-topic llm --add-topic harness`.
2. In the repo (one docs/chore PR): `Cargo.toml` `repository`/`homepage`, `mkdocs.yml` `repo_url`,
   `CONTRIBUTING.md` clone URL, README badges and links → `climate-x-org/blyte`.
3. Local checkout: `mv ~/repos/superstate ~/repos/blyte`, then
   `git remote set-url origin https://github.com/climate-x-org/blyte.git`. Remove or recreate any
   `.wt/` worktrees first (`git worktree list`; they hold absolute paths).
4. Tooling that names the old path:
   - `~/.claude/skills/agy-orchestration/` (`dispatch-record.py`'s `BLYTE_SRC` default, `SKILL.md`).
   - `superstate-eval` driver defaults (`lab/engine/drivers/superstate_native.py` looks in
     `~/repos/superstate/target/…`), handled in Phase 2.
   - Claude Code's project memory is keyed by path (`~/.claude/projects/-Users-danielfisher-repos-superstate/`):
     copy it to `…-repos-blyte/` so the memory follows the checkout.
   - Editor/session config that records the path (e.g. `~/.config/herdr/session.json`): update by hand.
5. Reinstall: `cargo install --path crates/blyte-cli --locked` from the new path (the build stamp is unaffected).

## Phase 2: the eval repository (`dnfcx/superstate-eval` → `dnfcx/blyte-eval`)

1. `gh repo rename blyte-eval --repo dnfcx/superstate-eval` (confirm first); local
   `mv ~/repos/superstate-eval ~/repos/blyte-eval` and `git remote set-url`.
2. Code rename, as one A/B plan task (shadow-run blyte vs agy), about 465 occurrences in 72 files
   under `lab/`, `tests/`, `scripts/`, `Makefile`, `pyproject.toml`:
   - package name `superstate-eval` → `blyte-eval`; module/driver names
     `superstate_native` → `blyte_native`, `SuperstateNativeDriver` → `BlyteNativeDriver`;
     `SUPERSTATE_BIN` → `BLYTE_BIN` (keep reading `SUPERSTATE_BIN` as a deprecated fallback for one
     release); default binary paths → `~/repos/blyte/target/…`.
   - Dead arms still carrying the old name (`superstate_mcp`, `superstate_daemon`,
     `superstate_agent_mcp` adapters and their tests): delete if unused by any live harness,
     otherwise rename. Decide from `lab/engine/harnesses.py`, not by name.
   - Harness names in `--harnesses` stay stable for result comparability: keep accepting the old
     spelling where results reference it.
   - Excluded: `results/`, `FINDINGS.md`, `EXPERIMENTS.md`, `traces/`, `data/`.
   - Checks: `make lint && make test` (including the Docker tests).
3. Cross-repo references: superstate's `report.rs`/`run.rs` comments and the research write-up say
   `superstate-eval` → `blyte-eval` (a docs PR in the harness repo).

## Phase 3: before going public (later; each item is a gate)

1. **History secret scan** of both repos' full history (`gitleaks detect --log-opts="--all"` or
   `trufflehog git file://. --since-commit` from the first commit). Anything real: rotate it, then
   decide between rewriting history and publishing a fresh squashed history (the user's call).
   Known false positives: planted fixtures in the eval's `sanitize-git-repo` results.
2. **Company sign-off** to make a `climate-x-org` repository public. The crates declare
   `Apache-2.0 OR MIT` but the repo has only the Apache text (`LICENSE`): add `LICENSE-MIT` with the
   company's copyright line (and rename `LICENSE` to `LICENSE-APACHE`), or drop MIT from `Cargo.toml`.
3. **Third-party content** in the eval repo's `results/` and task copies: confirm the licences of
   Terminal-Bench / DeepSWE task material before publishing it, or keep the eval repo private.
4. **Community files:** `SECURITY.md` (added with the docs overhaul), a code of conduct, issue and
   PR templates.
5. **Docs hosting:** GitHub Pages from `mkdocs gh-deploy` (Pages on a private repo needs a paid
   plan; enable it when the repo goes public).
6. **Publishing:** replace the crates.io placeholders with real releases through the existing
   `cd.yml` (dry run first); `release-plz.toml` currently has `publish = false`.
7. **README claims:** the A/B and FrontierHarness numbers carry their caveats in the README;
   re-check them against the ledger on the day.
