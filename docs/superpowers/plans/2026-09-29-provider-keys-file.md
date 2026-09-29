# Provider Keys from a File — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not read, print or modify any `.env` or secrets file. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-29):** blyte reads provider keys only from its environment. A Claude Code session
started from a shell without `GEMINI_API_KEY` (spatial-service, 2026-09-29) failed all three of its
`blyte dispatch` calls at turn 0, and the session did the work with Sonnet subagents instead. blyte
now also reads `~/.config/blyte/env` (`NAME=value` lines) for any variable not already set, so every
session finds the key however it was started. The file is refused if other users can read it. Tests
must never load a developer's real key: several run blyte without one to test the "no key" path, so
the workspace's cargo config sets an empty `BLYTE_ENV_FILE`, which turns the file off.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: `~/.config/blyte/env`

**Files:** create `crates/blyte-cli/src/env_file.rs`, `crates/blyte-cli/tests/test_env_file.rs`,
`.cargo/config.toml`; modify `crates/blyte-cli/src/lib.rs`, `crates/blyte-cli/src/main.rs`,
`docs/getting-started.md`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md`.

Write each new file in one go (for example with a heredoc), exactly as given.

- [ ] **Step 1: create `.cargo/config.toml`:**

```toml
# Tests run blyte without a provider key to exercise the "no key" paths. An empty BLYTE_ENV_FILE
# turns off blyte's key file (~/.config/blyte/env), so a developer's own key is never loaded by a test.
[env]
BLYTE_ENV_FILE = ""
```

- [ ] **Step 2: create `crates/blyte-cli/tests/test_env_file.rs`:**

```rust
//! blyte reads provider keys from ~/.config/blyte/env when they are not exported: a Claude Code
//! session started without GEMINI_API_KEY failed every dispatch at turn 0 (2026-09-29).

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use blyte_cli::env_file::{load, parse};

#[test]
fn parse_reads_names_and_values() {
    let text =
        "# keys\nexport GEMINI_API_KEY=abc\nB=\"q v\"\nC='s'\n\nnot a line\n1-BAD=x\nD = e \n";
    assert_eq!(
        parse(text),
        vec![
            ("GEMINI_API_KEY".to_string(), "abc".to_string()),
            ("B".to_string(), "q v".to_string()),
            ("C".to_string(), "s".to_string()),
            ("D".to_string(), "e".to_string()),
        ]
    );
}

// The one test in this binary that touches the process environment.
#[test]
fn load_sets_only_unset_variables_and_refuses_a_file_others_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("env");
    std::fs::write(
        &path,
        "ZZ_BLYTE_ENVFILE_NEW=from-file\nZZ_BLYTE_ENVFILE_SET=from-file\n",
    )
    .unwrap();
    std::env::set_var("ZZ_BLYTE_ENVFILE_SET", "from-env");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&path).unwrap_err().contains("chmod 600"));
    assert!(std::env::var("ZZ_BLYTE_ENVFILE_NEW").is_err());

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        load(&path).unwrap(),
        vec!["ZZ_BLYTE_ENVFILE_NEW".to_string()]
    );
    assert_eq!(std::env::var("ZZ_BLYTE_ENVFILE_NEW").unwrap(), "from-file");
    assert_eq!(std::env::var("ZZ_BLYTE_ENVFILE_SET").unwrap(), "from-env");

    assert_eq!(
        load(&dir.path().join("missing")).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn the_binary_loads_the_file_named_by_blyte_env_file() {
    // Network-free: a file other users can read is refused with a warning, even for --version,
    // so the binary did look at it; an empty BLYTE_ENV_FILE turns the file off.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("env");
    std::fs::write(&path, "ZZ_UNUSED=1\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let stderr = |file: &std::ffi::OsStr| {
        let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
            .arg("--version")
            .env("BLYTE_ENV_FILE", file)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    assert!(stderr(path.as_os_str()).contains("chmod 600"));
    assert!(!stderr(std::ffi::OsStr::new("")).contains("chmod 600"));
}
```

  Run: `cargo test -p blyte --test test_env_file`. Expected: it fails to compile (no `env_file` module).

- [ ] **Step 3: create `crates/blyte-cli/src/env_file.rs`:**

```rust
//! Provider keys from a file, for sessions that did not export them: a Claude Code session started
//! without `GEMINI_API_KEY` failed every `blyte dispatch` at turn 0 (spatial-service, 2026-09-29).
//!
//! The file is `$BLYTE_ENV_FILE` (empty turns it off), else `$XDG_CONFIG_HOME/blyte/env`, else
//! `~/.config/blyte/env`: `NAME=value` lines, with an optional `export ` prefix, surrounding quotes,
//! blank lines and `#` comments. A variable already set in the environment wins. A file other users
//! can read is not loaded.

use std::path::{Path, PathBuf};

/// Where blyte looks for the file, or `None` when it is turned off.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BLYTE_ENV_FILE") {
        return if p.is_empty() {
            None
        } else {
            Some(PathBuf::from(p))
        };
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x).join("blyte/env"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/blyte/env"))
}

/// The `NAME=value` pairs in `text`.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// Sets each variable in the file at `path` that is not already set, and returns their names.
/// A missing file sets nothing. Call it before any threads start: it uses `std::env::set_var`.
pub fn load(path: &Path) -> Result<Vec<String>, String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(Vec::new());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{} can be read by other users, so it was not loaded; run: chmod 600 {}",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut set = Vec::new();
    for (name, value) in parse(&text) {
        if std::env::var_os(&name).is_none() {
            std::env::set_var(&name, value);
            set.push(name);
        }
    }
    Ok(set)
}
```

  In `crates/blyte-cli/src/lib.rs`, add `pub mod env_file;` after `pub mod dispatch;`.

- [ ] **Step 4: `crates/blyte-cli/src/main.rs`.** In `fn main()`, between `restore_sigpipe();` and
  `let cli = Cli::parse();`, insert:

```rust
    // Before anything else reads the environment, and before any threads start.
    if let Some(path) = blyte_cli::env_file::default_path() {
        if let Err(e) = blyte_cli::env_file::load(&path) {
            eprintln!("warning: {e}");
        }
    }
```

  Run: `cargo test -p blyte --test test_env_file`. Expected: 3 passed.

- [ ] **Step 5: docs.** In `docs/getting-started.md`, directly after the code block containing
  `export GEMINI_API_KEY=...` (section "2. Give it a model"), insert this paragraph and block
  (keep a blank line before the following paragraph, "Other providers ..."):

````markdown

A session that did not start with the key exported (a Claude Code session opened from another
shell, say) will not see it. So you can keep it in a file instead, which blyte reads for any
variable not already set:

```bash
mkdir -p ~/.config/blyte && printf 'GEMINI_API_KEY=%s\n' "$GEMINI_API_KEY" > ~/.config/blyte/env
chmod 600 ~/.config/blyte/env
```

blyte refuses the file if other users can read it. `BLYTE_ENV_FILE` names another file; an empty
`BLYTE_ENV_FILE` turns it off.
````

  In `crates/blyte-cli/assets/blyte-implementer/SKILL.md`, replace the line
  `- The implementer model's API key must be in the environment; blyte never prints it.` with:

```markdown
- The implementer model's API key must be in the environment or in `~/.config/blyte/env` (`NAME=value`, `chmod 600`); blyte never prints it. If a dispatch fails with "no API key", tell the user; do not do the work some other way.
```

- [ ] **Step 6: run the checks and commit.**

  Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
  Expected: all pass.

```bash
git add .cargo/config.toml crates/blyte-cli/src/env_file.rs crates/blyte-cli/src/lib.rs crates/blyte-cli/src/main.rs crates/blyte-cli/tests/test_env_file.rs docs/getting-started.md crates/blyte-cli/assets/blyte-implementer/SKILL.md
git commit --no-gpg-sign -m "feat(cli): read provider keys from ~/.config/blyte/env when they are not exported"
```
