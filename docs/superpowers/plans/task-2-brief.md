# Task 2 Brief: Configuration & CLI Flag (`--impact-slicing`)

## Global Constraints
- Compile cleanly with `cargo check --all` and pass `cargo test --all`.
- No compiler or clippy warnings (`cargo clippy --all-targets -- -D warnings`).
- Skip GPG verification on all commits (`--no-gpg-sign`).
- Zero prompt engineering: all behavior governed by git diffs, process exit codes (`$? == 0`), and state invariants.

## Task Details
- **Task:** Task 2: Configuration & CLI Flag (`--impact-slicing`)
- **Depends on:** Task 1
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-daemon-test-impact-slicing`

**Files:**
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/commands/daemon.rs`
- Create: `crates/superstate-cli/tests/test_cli_impact_slicing.rs`

**Interfaces:**
- Consumes: `DaemonConfig`
- Produces: `DaemonConfig.impact_slicing: bool`, CLI `--impact-slicing` flag

## Implementation Steps
Follow TDD strictly:
1. Write CLI parsing test in `crates/superstate-cli/tests/test_cli_impact_slicing.rs`:
   ```rust
   use clap::Parser;
   use superstate_cli::state_cli::{Commands, StateArgs, Subcommands};

   #[test]
   fn test_cli_impact_slicing_flag_parsing() {
       let args = StateArgs::try_parse_from(["state", "daemon", "--impact-slicing"])
           .expect("Failed to parse --impact-slicing flag");
       match args.command {
           Subcommands::Daemon(daemon_args) => {
               assert!(daemon_args.impact_slicing, "Expected impact_slicing to be true");
           }
           _ => panic!("Expected Subcommands::Daemon"),
       }
   }
   ```
2. Run `cargo test -p superstate-cli --test test_cli_impact_slicing` to confirm failure (unrecognized argument `--impact-slicing`).
3. Add `impact_slicing` to `DaemonConfig` and CLI args:
   - In `crates/superstate-core/src/daemon.rs`:
     - Add `pub impact_slicing: bool` to `DaemonConfig` (defaulting to `false` in `Default::default()`).
   - In `crates/superstate-cli/src/state_cli.rs`:
     - Add `#[arg(long, help = "Enable milestone-scoped test impact slicing for fast fail-fast feedback")] pub impact_slicing: bool` to `DaemonArgs`.
   - In `crates/superstate-cli/src/commands/daemon.rs`:
     - Wire `impact_slicing: args.impact_slicing` when constructing `DaemonConfig`.
4. Run `cargo test -p superstate-cli --test test_cli_impact_slicing` and verify PASS.
5. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
6. Commit changes:
   ```bash
   git add crates/superstate-core/src/daemon.rs crates/superstate-cli/src/state_cli.rs crates/superstate-cli/src/commands/daemon.rs crates/superstate-cli/tests/test_cli_impact_slicing.rs
   git commit --no-gpg-sign -m "feat(cli): wire --impact-slicing flag into daemon configuration"
   ```
7. Write implementer report to `docs/superpowers/plans/task-2-report.md`.
8. Report status: DONE / DONE_WITH_CONCERNS / BLOCKED.
