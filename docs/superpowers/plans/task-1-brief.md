# Task 1: Binary Unification & Workspace Dependencies in crates/superstate-cli/Cargo.toml

## Goal
Unify the binary surface under `superstate` (retire legacy `state_main.rs` / `[[bin]] name = "state"`) and add workspace dependencies `superstate-agent` and `superstate-provider` to `crates/superstate-cli/Cargo.toml`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## Files
- Modify: `crates/superstate-cli/Cargo.toml`
- Delete: `crates/superstate-cli/src/state_main.rs`

## Detailed Steps
1. Edit `crates/superstate-cli/Cargo.toml`:
   - Keep description: `"Superstate unified interactive agent harness and state engine runner."`
   - Keep `[lib]` as `name = "superstate_cli"`
   - Ensure the ONLY `[[bin]]` section is:
     ```toml
     [[bin]]
     name = "superstate"
     path = "src/main.rs"
     ```
   - In `[dependencies]`, ensure these dependencies are present:
     ```toml
     libc = "0.2"
     superstate-core = { path = "../superstate-core", version = "0.1.0" }
     superstate-syntax = { path = "../superstate-syntax", version = "0.1.0" }
     superstate-review = { path = "../superstate-review", version = "0.1.0" }
     superstate-rules = { path = "../superstate-rules", version = "0.1.0" }
     superstate-mcp = { path = "../superstate-mcp", version = "0.1.0" }
     superstate-worktree = { path = "../superstate-worktree", version = "0.1.0" }
     superstate-agent = { path = "../superstate-agent", version = "0.1.0" }
     superstate-provider = { path = "../superstate-provider", version = "0.1.0" }
     clap.workspace = true
     serde.workspace = true
     serde_json.workspace = true
     chrono.workspace = true
     thiserror.workspace = true
     fd-lock.workspace = true
     tempfile.workspace = true
     ```
2. Remove `crates/superstate-cli/src/state_main.rs` using `git rm crates/superstate-cli/src/state_main.rs`.
3. Verify compilation:
   Run `cargo check -p superstate`
4. Commit:
   ```bash
   git add crates/superstate-cli/Cargo.toml
   git commit --no-gpg-sign -m "build(cli): unify binary under superstate and add agent dependencies"
   ```
5. Run `roborev status; roborev show HEAD`
6. Output report with commit hash and status.
