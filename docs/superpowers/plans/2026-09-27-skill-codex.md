# The Skill for Codex — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not write to the real `~/.claude`, `~/.codex` or `~/.agents`: tests use a temporary `HOME`. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `blyte skill install --harness codex` sets up OpenAI's Codex CLI as the orchestrator
with blyte underneath, as `blyte skill install` does for Claude Code. `--harness all` does both.
The skill text becomes harness-neutral.

**Why (2026-09-27):** Codex reads the same `SKILL.md` format, from `~/.agents/skills` (personal)
and `<repo>/.agents/skills` (per repository), with older versions reading `~/.codex/skills`; its
standing instructions live in `AGENTS.md` (`~/.codex/AGENTS.md`, or the repository's). Codex runs
shell commands in its own sandbox, usually without network access, and blyte's own sandbox cannot
start inside another one, so `blyte dispatch` and `blyte review` must run outside Codex's sandbox:
Codex rules (`prefix_rule(..., decision="allow")` in `~/.codex/rules/*.rules`) pre-approve them,
and the skill tells Codex to request unsandboxed execution for exactly those two commands.
**Untested against a live Codex** (no Codex account with quota yet): the docs must say so.

**Tech Stack:** Rust, clap.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. Claude Code behaviour (paths, block, messages) is unchanged
  when `--harness` is omitted.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `--harness claude|codex|all`

**Files:** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md`,
`crates/blyte-cli/tests/test_skill_codex.rs` (new).

**What to build:**

- `pub enum Harness { Claude, Codex }` in `skill.rs` (derive `Debug, Clone, Copy, PartialEq, Eq`).
- `SkillAction::Install` and `SkillAction::Uninstall` gain
  `#[arg(long, default_value = "claude", value_parser = ["claude", "codex", "all"])] harness: String`
  ("Which orchestrator to set up: claude (Claude Code), codex (Codex CLI) or all"). `all` means
  Claude then Codex.
- `pub fn skill_dirs(harness: Harness, project: bool) -> Result<Vec<PathBuf>, String>`:
  - Claude: exactly what `skill_dir(project)` returns today (one path).
  - Codex, personal: `$HOME/.agents/skills/blyte-implementer` and `$HOME/.codex/skills/blyte-implementer`.
  - Codex, `--project`: `<repo root>/.agents/skills/blyte-implementer`.
- `pub fn instructions_path(harness: Harness, project: bool) -> Result<PathBuf, String>`:
  Claude: `claude_md_path(project)` (unchanged); Codex: `$HOME/.codex/AGENTS.md`, or
  `<repo root>/AGENTS.md` with `project`.
- `pub const CODEX_RULES: &str`, exactly:
  ```text
  # Written by `blyte skill install --harness codex`; removed by `blyte skill uninstall --harness codex`.
  # blyte dispatch needs the network and applies its own sandbox, so it runs outside Codex's.
  prefix_rule(pattern=["blyte", "dispatch"], decision="allow")
  prefix_rule(pattern=["blyte", "review"], decision="allow")
  ```
  written to `$HOME/.codex/rules/blyte.rules` by a personal Codex install (not with `--project`),
  overwriting any previous copy.
- `pub fn install_for(harness: Harness, project: bool, force: bool, no_default: bool) -> Result<Vec<PathBuf>, String>`:
  writes `SKILL_MD` to every skill dir (the existing "refuse an edited copy without `--force`"
  rule applies to each), the default block (`set_default_block`) to the instructions path unless
  `no_default`, and for personal Codex the rules file. Returns every path written. The existing
  `install(project, force, no_default)` keeps its signature and behaviour (Claude only).
- `pub fn uninstall_for(harness: Harness, project: bool) -> Result<Vec<PathBuf>, String>`: the
  reverse, as `uninstall` does today, plus deleting the rules file for personal Codex. The existing
  `uninstall(project)` keeps its signature and behaviour (Claude only).
- `main.rs`: `install` prints `installed <path>` for each skill file, `updated <path>` for each
  instructions file, `wrote <path>` for the rules file, then one line per harness: Claude as today;
  Codex: `Codex now plans and reviews, and delegates implementation to blyte, in parallel, by default (untested)`
  (or, with `--no-default`, `to use it, ask Codex: "use blyte to implement this"`). `uninstall`
  as today, over all paths.
- **Skill text** (`assets/blyte-implementer/SKILL.md`): replace the two paragraphs from
  `**Parallel tasks in an interactive session:**` through the end of the fenced block that ends
  with `wait` (keep that block's content) with exactly:
  ````markdown
  **Starting parallel tasks.** If your harness tells you when a background command exits (an
  interactive Claude Code session does), send every independent `blyte dispatch` as its own
  background command in the same message and review each as it lands. Otherwise (Codex, or a
  one-shot session such as `claude -p`, where background commands die with the session), start
  them together and wait in the foreground:

  ```bash
  mkdir -p .wt
  blyte dispatch T1 /abs/task-T1.md --gate '...' > .wt/T1.json &
  blyte dispatch T2 /abs/task-T2.md --gate '...' > .wt/T2.json &
  wait
  ```

  **Under Codex:** run `blyte dispatch` and `blyte review` outside Codex's sandbox (request
  escalated permissions for them; `blyte skill install --harness codex` pre-approves exactly these
  two commands). They need the network to reach blyte's model, and blyte applies its own sandbox
  to everything the implementer runs.
  ````

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_skill_codex.rs`:

```rust
//! `blyte skill install --harness codex` sets up Codex CLI as the orchestrator with blyte
//! underneath: the skill where Codex finds it, the default block in AGENTS.md, and rules that let
//! `blyte dispatch` run outside Codex's sandbox (2026-09-27). Untested against a live Codex.

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{CODEX_RULES, DEFAULT_BLOCK, SKILL_MD};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn a_personal_codex_install_writes_skills_agents_md_and_rules() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(cwd.path(), home.path(), &["skill", "install", "--harness", "codex"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let h = home.path();
    for p in [".agents/skills/blyte-implementer/SKILL.md", ".codex/skills/blyte-implementer/SKILL.md"] {
        assert_eq!(std::fs::read_to_string(h.join(p)).unwrap(), SKILL_MD, "{p}");
    }
    assert!(std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap().contains(DEFAULT_BLOCK));
    assert_eq!(std::fs::read_to_string(h.join(".codex/rules/blyte.rules")).unwrap(), CODEX_RULES);
    assert!(!h.join(".claude").exists(), "codex only");
    assert!(String::from_utf8_lossy(&out.stdout).contains("(untested)"));

    let out = blyte(cwd.path(), home.path(), &["skill", "uninstall", "--harness", "codex"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!h.join(".agents/skills/blyte-implementer/SKILL.md").exists());
    assert!(!h.join(".codex/rules/blyte.rules").exists());
    assert!(!std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap().contains("blyte:default"));
}

#[test]
fn all_installs_for_both_and_the_default_is_still_claude() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    assert_eq!(blyte(cwd.path(), home.path(), &["skill", "install", "--harness", "all"]).status.code(), Some(0));
    assert!(home.path().join(".claude/skills/blyte-implementer/SKILL.md").exists());
    assert!(home.path().join(".agents/skills/blyte-implementer/SKILL.md").exists());

    let home2 = tempfile::tempdir().unwrap();
    assert_eq!(blyte(cwd.path(), home2.path(), &["skill", "install"]).status.code(), Some(0));
    assert!(home2.path().join(".claude/skills/blyte-implementer/SKILL.md").exists());
    assert!(!home2.path().join(".agents").exists());
    assert!(!home2.path().join(".codex").exists());
}

#[test]
fn a_project_codex_install_goes_into_the_repository_without_rules() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(repo.path()).status().unwrap().success());
    let out = blyte(repo.path(), home.path(), &["skill", "install", "--harness", "codex", "--project"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(repo.path().join(".agents/skills/blyte-implementer/SKILL.md").exists());
    assert!(std::fs::read_to_string(repo.path().join("AGENTS.md")).unwrap().contains(DEFAULT_BLOCK));
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn the_skill_covers_both_harnesses() {
    for s in ["**Starting parallel tasks.**", "**Under Codex:**", "escalated permissions", "claude -p"] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
    assert!(CODEX_RULES.contains(r#"prefix_rule(pattern=["blyte", "dispatch"], decision="allow")"#));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_skill_codex`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte skill install --harness codex sets up Codex CLI as the orchestrator (untested against a live Codex)"
```
