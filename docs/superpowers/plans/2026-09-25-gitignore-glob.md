# .gitignore-aware Glob Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** `glob` follows fd/ripgrep conventions: `.gitignore`d files and `.git` are skipped. Other dotfiles (`.github/`, `.env.example`) stay visible. The behaviour is the same locally and in containers.

**Why:** The user chose this on 2026-09-25. Exact bash `**` semantics walk `target/`, `node_modules` and `.git`. That is slow on large trees (273 ms for `**/*.rs` at this repository's root, which has a `target/`), and it can fill results with build artefacts that the model then reads. `grep` already skips ignored files (native ripgrep).

**Design:**
- **Local** (`search::glob_paths`): the `ignore` walk now applies `.gitignore`, `.git/info/exclude`, the global gitignore and `.ignore` (in a git repository, as ripgrep does). Hidden files are included; `.git` is filtered out. Directories still match, and the result limit and sort are unchanged.
- **Containers:** one command lists the repository's files, `git -c core.quotePath=off ls-files -co --exclude-standard` (tracked and untracked, `.gitignore` applied). The same `globset` matcher runs in Rust over those files and their ancestor directories (`search::glob_filter`). Outside a git repository, or without git, it falls back to today's shell glob.
- **Tool description:** "Respects .gitignore; `.git` is skipped."

**Tech Stack:** Rust 2021 (no new dependencies).

## Global Constraints

- Base branch: `feat/gitignore-glob` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- Do not run Docker. `Cargo.lock` is gitignored.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. Known timing-flaky tests: `blyte-mcp`'s `test_blyte_status_and_cancel_lifecycle`, `blyte-cli`'s `test_cli_daemon_detached_forwarding`, and `blyte-core`'s `test_daemon_advisory_review_clean_diff_advances`. If one alone fails, re-run once and report both results.
- The semantics change is intentional. Permitted test edits: `crates/blyte-agent/tests/test_native_glob.rs` is **replaced** (Step 1), plus one assertion in `test_tools.rs` (Step 5). All other existing tests must pass unmodified.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `.gitignore`-aware glob, local and container

**Files:**
- Modify: `crates/blyte-agent/src/search.rs`, `crates/blyte-agent/src/tools.rs`
- Replace: `crates/blyte-agent/tests/test_native_glob.rs`
- Modify (one assertion): `crates/blyte-agent/tests/test_tools.rs`

- [ ] **Step 1: Replace the glob tests.** Overwrite `crates/blyte-agent/tests/test_native_glob.rs` with:

```rust
//! `glob` follows fd/ripgrep conventions: `.gitignore`d files and `.git` are skipped, other
//! dotfiles are kept, directories match. Local workspaces expand in-process; containers list the
//! repository with one `git ls-files` and match here.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::search::{glob_filter, glob_paths};
use blyte_agent::shell::{LocalShell, Output, Shell};
use blyte_agent::tools::execute;

struct NoShell;
impl Shell for NoShell {
    fn exec(&self, command: &str, _: &Path, _: std::time::Duration) -> Result<Output, String> {
        panic!("glob spawned a command: {command}");
    }
    fn describe(&self) -> String {
        String::new()
    }
}

/// Host files presented as a container's (not local), so the tool takes the container route.
struct NotLocal;
impl blyte_agent::fs::Fs for NotLocal {
    fn read(&self, p: &Path) -> Result<String, String> {
        blyte_agent::fs::Fs::read(&LocalFs, p)
    }
    fn write(&self, p: &Path, c: &str) -> Result<(), String> {
        blyte_agent::fs::Fs::write(&LocalFs, p, c)
    }
    fn canonicalize(&self, p: &Path) -> Result<std::path::PathBuf, String> {
        blyte_agent::fs::Fs::canonicalize(&LocalFs, p)
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    Command::new("git").args(["init", "-q"]).current_dir(r).status().unwrap();
    for dir in ["src/deep", "target/debug", "node_modules/pkg", ".github/workflows"] {
        std::fs::create_dir_all(r.join(dir)).unwrap();
    }
    std::fs::write(r.join(".gitignore"), "target/\nnode_modules/\n*.log\n").unwrap();
    for f in [
        "a.rs",
        ".env.example",
        "src/b.rs",
        "src/deep/c.rs",
        "target/debug/build.rs",
        "node_modules/pkg/i.js",
        "debug.log",
        ".github/workflows/ci.yml",
    ] {
        std::fs::write(r.join(f), "x").unwrap();
    }
    d
}

#[test]
fn ignored_files_and_dot_git_are_skipped_and_dotfiles_kept() {
    let d = repo();
    assert_eq!(
        glob_paths(d.path(), "**/*.rs", 200).unwrap(),
        vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
    );
    assert_eq!(
        glob_paths(d.path(), "*", 200).unwrap(),
        vec![".env.example", ".github", ".gitignore", "a.rs", "src"]
    );
    assert_eq!(
        glob_paths(d.path(), "**/*.yml", 200).unwrap(),
        vec![".github/workflows/ci.yml"]
    );
    assert_eq!(glob_paths(d.path(), "**/*.log", 200).unwrap(), Vec::<String>::new());
    assert_eq!(glob_paths(d.path(), "src/*", 200).unwrap(), vec!["src/b.rs", "src/deep"]);
}

#[test]
fn limits_and_patterns_it_does_not_handle() {
    let d = repo();
    assert_eq!(glob_paths(d.path(), "**/*.rs", 2).unwrap().len(), 2);
    assert!(glob_paths(d.path(), "/etc/*", 200).is_none());
    assert!(glob_paths(d.path(), "../*", 200).is_none());
    assert!(glob_paths(d.path(), "src/", 200).is_none());
}

#[test]
fn a_file_listing_is_filtered_with_directories_derived() {
    let listing = "a.rs\nsrc/b.rs\nsrc/deep/c.rs\n.github/workflows/ci.yml\n";
    assert_eq!(
        glob_filter(listing, "src/*", 200).unwrap(),
        vec!["src/b.rs", "src/deep"]
    );
    assert_eq!(
        glob_filter(listing, "**/*.rs", 200).unwrap(),
        vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
    );
    assert_eq!(glob_filter(listing, "*", 200).unwrap(), vec![".github", "a.rs", "src"]);
    assert!(glob_filter(listing, "/abs/*", 200).is_none());
}

#[test]
fn the_local_tool_expands_without_spawning() {
    let d = repo();
    let o = execute(d.path(), &NoShell, &LocalFs, "glob", &json!({"pattern": "src/**/*.rs"}));
    assert_eq!(o.content, "2 file(s):\nsrc/b.rs\nsrc/deep/c.rs");
    let none = execute(d.path(), &NoShell, &LocalFs, "glob", &json!({"pattern": "*.zz"}));
    assert_eq!(none.content, "no files match *.zz");
}

#[test]
fn the_container_route_lists_with_git_and_matches_the_same() {
    let d = repo();
    let o = execute(d.path(), &LocalShell, &NotLocal, "glob", &json!({"pattern": "**/*.rs"}));
    assert_eq!(o.content, "3 file(s):\na.rs\nsrc/b.rs\nsrc/deep/c.rs");
}

#[test]
fn outside_a_git_repository_the_container_route_falls_back_to_the_shell() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("target")).unwrap();
    std::fs::write(d.path().join("target/x.rs"), "x").unwrap();
    let o = execute(d.path(), &LocalShell, &NotLocal, "glob", &json!({"pattern": "**/*.rs"}));
    assert_eq!(o.content, "1 file(s):\ntarget/x.rs");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_native_glob`. Expected: a compile error, because `glob_filter` does not exist.

- [ ] **Step 3: Search functions.** In `crates/blyte-agent/src/search.rs`:
  1. Replace the whole `glob_paths` function, including its doc comment, with:

```rust
/// The glob matcher shared by local and container globs. `None` for patterns not handled here
/// (absolute, `.` or `..` components, a trailing `/`); callers send those to the shell.
fn glob_matcher(pattern: &str) -> Option<globset::GlobMatcher> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.split('/').any(|c| c == "." || c == "..")
    {
        return None;
    }
    Some(
        globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .ok()?
            .compile_matcher(),
    )
}

fn is_meta(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// Paths under `root` matching a glob, fd/ripgrep style: `.gitignore`d files (in a git
/// repository), `.ignore` rules and `.git` are skipped; other dotfiles are kept; `**` crosses
/// directories; directories match too; symlinks are not followed. Sorted bytewise, at most `max`.
/// `None` for patterns not handled here. A bash `**` walked `target/` and `node_modules`: 273 ms
/// for `**/*.rs` at a Rust repository's root, and results full of build artefacts.
pub fn glob_paths(root: &Path, pattern: &str, max: usize) -> Option<Vec<String>> {
    let matcher = glob_matcher(pattern)?;
    if !is_meta(pattern) {
        return Some(if root.join(pattern).exists() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        });
    }
    // Walk from the pattern's literal leading directories, only as deep as it can reach.
    let comps: Vec<&str> = pattern.split('/').collect();
    let lit = comps[..comps.len() - 1]
        .iter()
        .take_while(|c| !is_meta(c))
        .count();
    let base = comps[..lit]
        .iter()
        .fold(root.to_path_buf(), |p, c| p.join(c));
    if !base.is_dir() {
        return Some(Vec::new());
    }
    let mut walk = WalkBuilder::new(&base);
    walk.hidden(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
    if !pattern.contains("**") {
        walk.max_depth(Some(comps.len() - lit));
    }
    let mut hits: Vec<String> = walk
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?.to_path_buf();
            matcher
                .is_match(&rel)
                .then(|| rel.to_string_lossy().into_owned())
        })
        .collect();
    hits.sort();
    hits.truncate(max);
    Some(hits)
}

/// Paths from a newline-separated file listing (e.g. `git ls-files`) matching a glob, with the
/// listed files' ancestor directories as candidates too, so `src/*` still shows `src/deep`. The
/// container route: one command lists the repository, matching happens here with the same rules
/// as [`glob_paths`]. Sorted bytewise, at most `max`. `None` for patterns not handled here.
pub fn glob_filter(listing: &str, pattern: &str, max: usize) -> Option<Vec<String>> {
    let matcher = glob_matcher(pattern)?;
    let mut candidates = std::collections::BTreeSet::new();
    for line in listing.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        let mut p = Path::new(line);
        candidates.insert(line.to_string());
        while let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            candidates.insert(parent.to_string_lossy().into_owned());
            p = parent;
        }
    }
    let mut hits: Vec<String> = candidates
        .into_iter()
        .filter(|c| matcher.is_match(Path::new(c)))
        .collect();
    hits.truncate(max);
    Some(hits)
}
```

  (`BTreeSet` iteration is already bytewise-sorted, so `glob_filter` needs no separate sort.)

- [ ] **Step 4: The `glob` tool.** In `crates/blyte-agent/src/tools.rs`:
  1. In `specs()`, change the `glob` tool's description to: `"Find files by glob pattern, e.g. `src/**/*.rs`. Respects .gitignore; `.git` is skipped. Fast, and the right first move when you do not know where something lives."` (as a Rust string: keep the existing `\` line-continuation style).
  2. In the `"glob" =>` arm, replace the block that begins `// A local workspace expands in-process;` (the `if fs.is_local() { ... }` block added by #64) with:

```rust
            // .gitignore-aware, fd/ripgrep style. A local workspace expands in-process; a
            // container lists its repository with one `git ls-files` and matches here. Outside a
            // git repository, or for patterns not handled here, the shell glob below runs as before.
            let outcome = |hits: Vec<String>| {
                if hits.is_empty() {
                    ToolOutcome::ok(format!("no files match {pattern}"))
                } else {
                    ToolOutcome::ok(format!("{} file(s):\n{}", hits.len(), hits.join("\n")))
                }
            };
            if fs.is_local() {
                if let Some(hits) = crate::search::glob_paths(root, &pattern, MAX_MATCHES) {
                    return outcome(hits);
                }
            } else if let Ok(o) = shell.exec(
                "if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
                 git -c core.quotePath=off ls-files -co --exclude-standard; \
                 else echo __BLYTE_NO_GIT__; fi",
                root,
                COMMAND_TIMEOUT,
            ) {
                if o.exit_code == 0 && !o.stdout.starts_with("__BLYTE_NO_GIT__") {
                    if let Some(hits) = crate::search::glob_filter(&o.stdout, &pattern, MAX_MATCHES)
                    {
                        return outcome(hits);
                    }
                }
            }
```

- [ ] **Step 5: The seam test.** In `crates/blyte-agent/tests/test_tools.rs`, in `search_tools_also_go_through_the_seam`, change the assertion `assert!(seen.iter().any(|c| c.contains("*.rs")));` to `assert!(seen.iter().any(|c| c.contains("ls-files") || c.contains("*.rs")));` and add a comment above it: `// glob lists a container's repository with git ls-files (or falls back to a shell glob).` This is the only permitted edit to that file.

- [ ] **Step 6: Run the tests.** Run: `cargo test -p blyte-agent --test test_native_glob --test test_tools --test test_native_grep`. Expected: all pass (6 in `test_native_glob`). If `ignored_files_and_dot_git_are_skipped_and_dotfiles_kept` fails, report the actual vectors verbatim and stop.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): glob respects .gitignore and skips .git (fd/ripgrep style), local and container"
```
