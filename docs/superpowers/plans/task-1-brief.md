# Task 1 Brief: Add libc Dependency to crates/superstate-agent

## Global Constraints
- Target crate: `crates/superstate-agent`
- Rust 2021 edition, zero compiler warnings (`cargo check -p superstate-agent`)
- Clean execution on Unix platforms (macOS / Linux)

## Task Description
Add `libc` dependency to `crates/superstate-agent/Cargo.toml` under `[target.'cfg(unix)'.dependencies]`.

### Files:
- Modify: `crates/superstate-agent/Cargo.toml`

### Steps:
1. In `crates/superstate-agent/Cargo.toml`, add:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```
2. Verify with `cargo check -p superstate-agent` (must succeed with 0 warnings).
3. Commit with message:
```bash
git add crates/superstate-agent/Cargo.toml
git commit --no-gpg-sign -m "build: add libc dependency for Unix process group killing"
```
4. Run `roborev status; roborev show HEAD` to verify.
5. Report back status: DONE.
