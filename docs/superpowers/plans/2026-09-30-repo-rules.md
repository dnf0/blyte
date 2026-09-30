# blyte Loads the Repository's Rules Itself — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-30):** a task got different rules depending on how blyte was started. Through the
local dispatch scripts, the repository's `.claude/agy-rules.md` was appended to the prompt; through
`blyte dispatch` directly (colleagues, Codex, other sessions) blyte saw only the task text, so rules
such as "AWS: read-only calls only" never reached it. blyte now loads the repository's rules itself
on every run: `.blyte/rules.md`, else `.claude/agy-rules.md`, else `AGENTS.md` (never `CLAUDE.md`,
which is written for the orchestrator), from the working tree or the main checkout, capped at
12,000 characters, in its system prompt.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: repository rules in the system prompt

**Files:** create `crates/blyte-agent/src/rules.rs` and `crates/blyte-agent/tests/test_repo_rules.rs`;
modify `crates/blyte-agent/src/lib.rs`, `crates/blyte-agent/src/run.rs`,
`crates/blyte-cli/src/task_runner.rs`, `docs/usage.md`. Write each new file in one go, exactly as given.

- [ ] **Step 1: create `crates/blyte-agent/tests/test_repo_rules.rs`:**

```rust
//! blyte loads the repository's rules for agents itself, however it was started (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_agent::rules::{load, prompt_section, MAX_RULES_CHARS};

fn write(root: &Path, name: &str, text: &str) {
    let p = root.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn the_first_rule_file_found_wins() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(load(d.path()), None);
    assert_eq!(prompt_section(d.path()), "");

    write(d.path(), "AGENTS.md", "agents rules");
    assert_eq!(load(d.path()).unwrap().text, "agents rules");

    write(d.path(), ".claude/agy-rules.md", "implementer rules");
    assert_eq!(load(d.path()).unwrap().text, "implementer rules");

    write(d.path(), ".blyte/rules.md", "  blyte rules\n");
    let r = load(d.path()).unwrap();
    assert_eq!(r.text, "blyte rules");
    assert!(r.source.ends_with(".blyte/rules.md"));

    let section = prompt_section(d.path());
    assert!(section.starts_with("## Repository rules\n"), "{section}");
    assert!(section.contains("blyte rules"), "{section}");
}

#[test]
fn claude_md_is_not_read_and_empty_files_are_skipped() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "CLAUDE.md", "orchestrator instructions");
    assert_eq!(load(d.path()), None);
    write(d.path(), ".claude/agy-rules.md", "   \n");
    write(d.path(), "AGENTS.md", "agents rules");
    assert_eq!(load(d.path()).unwrap().text, "agents rules");
}

#[test]
fn long_rules_are_cut_with_a_note() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "AGENTS.md", &"x".repeat(MAX_RULES_CHARS + 500));
    let r = load(d.path()).unwrap();
    assert!(r.text.starts_with(&"x".repeat(MAX_RULES_CHARS)));
    assert!(
        r.text
            .ends_with("[cut at 12000 characters; the rest is in AGENTS.md]"),
        "{}",
        &r.text[r.text.len() - 80..]
    );
}

#[test]
fn a_worktree_finds_rules_kept_untracked_in_the_main_checkout() {
    let d = tempfile::tempdir().unwrap();
    let main = d.path().join("repo");
    std::fs::create_dir_all(&main).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    };
    git(&main, &["init", "-q"]);
    write(&main, "f.txt", "a\n");
    git(&main, &["add", "f.txt"]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
            "--no-gpg-sign",
        ],
    );
    write(&main, ".claude/agy-rules.md", "untracked implementer rules");
    let wt = d.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);

    let r = load(&wt).unwrap();
    assert_eq!(r.text, "untracked implementer rules");
    assert!(r.source.starts_with(main.canonicalize().unwrap()) || r.source.starts_with(&main));
}
```

  Run: `cargo test -p blyte-agent --test test_repo_rules`. Expected: it fails to compile (no `rules` module).

- [ ] **Step 2: create `crates/blyte-agent/src/rules.rs`:**

```rust
//! The repository's own rules for agents, loaded on every run so that blyte gets them however it
//! was started (2026-09-30: runs started with `blyte dispatch` directly saw only the task text,
//! while the dispatch scripts appended `.claude/agy-rules.md`).
//!
//! The first file found wins: `.blyte/rules.md` (written for blyte), `.claude/agy-rules.md` (the
//! implementer rules repositories already keep for headless agents), then `AGENTS.md` (the
//! cross-tool convention). `CLAUDE.md` is not read: it is written for the orchestrator. Each is
//! looked for in the working tree, then in the main checkout, since the file may be untracked.

use std::path::{Path, PathBuf};

/// The files blyte reads rules from, in order of precedence (relative to the repository root).
pub const RULE_FILES: [&str; 3] = [".blyte/rules.md", ".claude/agy-rules.md", "AGENTS.md"];

/// Rules longer than this are cut, with a note, so one large file cannot crowd out the task.
pub const MAX_RULES_CHARS: usize = 12_000;

/// Repository rules: where they came from and their text (already capped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRules {
    pub source: PathBuf,
    pub text: String,
}

/// The main checkout's root when `root` is a linked git worktree, else `None`.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let main = common.parent()?.to_path_buf();
    let same = main.canonicalize().ok()? == root.canonicalize().ok()?;
    (!same).then_some(main)
}

/// Load the repository's rules for `root`, or `None` when it has none.
pub fn load(root: &Path) -> Option<RepoRules> {
    let mut roots = vec![root.to_path_buf()];
    roots.extend(main_checkout(root));
    for name in RULE_FILES {
        for base in &roots {
            let path = base.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let text = if text.chars().count() > MAX_RULES_CHARS {
                let cut: String = text.chars().take(MAX_RULES_CHARS).collect();
                format!("{cut}\n\n[cut at {MAX_RULES_CHARS} characters; the rest is in {name}]")
            } else {
                text.to_string()
            };
            return Some(RepoRules { source: path, text });
        }
    }
    None
}

/// The system prompt section for the repository's rules; empty when it has none.
pub fn prompt_section(root: &Path) -> String {
    match load(root) {
        Some(r) => format!(
            "## Repository rules\nThe repository's own rules for agents, from `{}`. Follow them \
             unless the task says otherwise; where they name a check command, it is the same \
             check that decides completion.\n\n{}\n\n",
            r.source.display(),
            r.text
        ),
        None => String::new(),
    }
}
```

  In `crates/blyte-agent/src/lib.rs`, add `pub mod rules;` directly before `pub mod run;`.

- [ ] **Step 3: the system prompt (`crates/blyte-agent/src/run.rs`, `fn system_prompt`).** In the
  `format!` string, directly before the line `             ## Working efficiently\n\`, insert the line

```rust
             {rules}\
```

  and in the `format!` arguments, after `bins = bins,`, add

```rust
            rules = crate::rules::prompt_section(&self.root),
```

- [ ] **Step 4: say which file was used (`crates/blyte-cli/src/task_runner.rs`).** In the host
  (non-container) branch, directly after `blyte_agent::shell::set_pass_env(config.pass_env.clone());`, add

```rust
            if let Some(r) = blyte_agent::rules::load(&config.root) {
                eprintln!("blyte: repository rules from {}", r.source.display());
            }
```

  Run: `cargo test -p blyte-agent --test test_repo_rules`. Expected: 4 passed.

- [ ] **Step 5: docs.** In `docs/usage.md`, directly before the line `## Budgets`, insert:

````markdown
## Repository rules

blyte reads the repository's own rules for agents on every run and puts them in its system prompt,
however it was started (a direct `blyte dispatch`, a script, another agent). The first file found
wins: `.blyte/rules.md` (written for blyte), `.claude/agy-rules.md` (implementer rules kept for
headless agents), then `AGENTS.md`. `CLAUDE.md` is not read, because it is written for the
orchestrator. Each is looked for in the working tree, then in the main checkout (a rules file may
be untracked). Rules are capped at 12,000 characters. The run prints which file it used
(`blyte: repository rules from …`). Container runs read none.

````

- [ ] **Step 6: run the checks and commit.**

```bash
git add crates/blyte-agent/src/rules.rs crates/blyte-agent/src/lib.rs crates/blyte-agent/src/run.rs crates/blyte-cli/src/task_runner.rs crates/blyte-agent/tests/test_repo_rules.rs docs/usage.md
git commit --no-gpg-sign -m "feat(agent): load the repository's rules for agents on every run"
```
