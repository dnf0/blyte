# Superstate Massive Adoption & Founder Prestige Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build, package, and launch the Superstate open-source ecosystem to achieve viral developer adoption, top Hacker News mindshare, and maximum founder prestige.

**Architecture:** A multi-platform Rust distribution pipeline (`cargo`, `brew`, prebuilt matrix) coupled with an interactive repository initializer (`superstate init`), official MCP server registry packaging, a formal empirical whitepaper, and a viral GitHub Actions self-healing CI bot.

**Tech Stack:** Rust 2021, Cargo, GitHub Actions, Homebrew, Node.js (MCP wrapper), Python / LaTeX / Markdown (Whitepaper & MkDocs Material).

## Global Constraints

- Never break existing CLI or MCP tool schemas.
- Ensure prebuilt binaries are statically linked or have minimal OS dependencies.
- Zero external network calls during core state operations; local-first by default.
- Strictly adhere to Conventional Commits format (`feat:`, `chore:`, `docs:`).

---

### Task 1: Prebuilt Multi-Platform Binary Releases & Homebrew Tap Setup

- **Depends on:** none

**Files:**
- Create: `/Users/danielfisher/repos/superstate/.github/workflows/release.yml`
- Create: `/Users/danielfisher/repos/superstate/Formula/superstate.rb`
- Test: `/Users/danielfisher/repos/superstate/tests/release_smoke.sh`

**Interfaces:**
- Consumes: `cargo build --release` across targets
- Produces: GitHub Release assets (`superstate-vX.Y.Z-aarch64-apple-darwin.tar.gz`, `superstate-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`, etc.) and a functional Homebrew formula.

- [ ] **Step 1: Write release test smoke script**
Create `tests/release_smoke.sh` verifying that binary version prints cleanly and passes basic sanity checks.

- [ ] **Step 2: Create GitHub Actions matrix release workflow**
Write `.github/workflows/release.yml` building for:
  - `aarch64-apple-darwin` (macOS Apple Silicon)
  - `x86_64-apple-darwin` (macOS Intel)
  - `x86_64-unknown-linux-gnu` (Linux x86)
  - `aarch64-unknown-linux-gnu` (Linux ARM64)
  - `x86_64-pc-windows-msvc` (Windows)

- [ ] **Step 3: Create Homebrew formula template**
Write `Formula/superstate.rb` referencing release tarballs with automatic architecture detection.

- [ ] **Step 4: Commit release infrastructure**
```bash
git add .github/workflows/release.yml Formula/ tests/release_smoke.sh
git commit -m "chore(release): configure multi-platform binary matrix and Homebrew formula"
```

---

### Task 2: Interactive `superstate init` Repository Wizard

- **Depends on:** Task 1

**Files:**
- Create: `/Users/danielfisher/repos/superstate/crates/superstate/src/commands/init.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate/src/main.rs`
- Modify: `/Users/danielfisher/repos/superstate/crates/superstate/src/cli.rs`
- Test: `/Users/danielfisher/repos/superstate/crates/superstate/tests/init_test.rs`

**Interfaces:**
- Consumes: Local file system (`Cargo.toml`, `pyproject.toml`, `package.json`, `go.mod`)
- Produces: `superstate init` command that creates `.agent-state/` and starter `plan.md` with auto-detected test gates.

- [ ] **Step 1: Write integration tests for `superstate init`**
Test that running `superstate init` in a Python, Rust, or Node repository auto-detects `pytest`, `cargo test`, or `npm test` and writes `.agent-state/state.json`.

- [ ] **Step 2: Implement repository heuristic detector in `init.rs`**
Inspect directory roots for manifest files and extract appropriate default test runners and linters.

- [ ] **Step 3: Wire `init` subcommand into `cli.rs` and `main.rs`**
Add `Init` variant to CLI enum with optional `--force` and `--yes` flags.

- [ ] **Step 4: Verify tests pass**
```bash
cargo test -p superstate --test init_test
```

- [ ] **Step 5: Commit `superstate init` feature**
```bash
git add crates/superstate/
git commit -m "feat(cli): implement interactive superstate init repository wizard"
```

---

### Task 3: MCP Registry & Smithery Integration Metadata

- **Depends on:** Task 1

**Files:**
- Create: `/Users/danielfisher/repos/superstate/packages/mcp/package.json`
- Create: `/Users/danielfisher/repos/superstate/packages/mcp/bin/superstate-mcp.js`
- Create: `/Users/danielfisher/repos/superstate/smithery.yaml`
- Test: `/Users/danielfisher/repos/superstate/packages/mcp/test_mcp_spawn.js`

**Interfaces:**
- Consumes: `superstate mcp` binary
- Produces: `npx @superstate/mcp` command that auto-downloads or resolves the native binary and launches stdio JSON-RPC.

- [ ] **Step 1: Create npm package scaffold for `npx @superstate/mcp`**
Write `packages/mcp/package.json` and a lightweight Node launcher that locates or downloads the native binary and pipes stdio.

- [ ] **Step 2: Create `smithery.yaml` for Smithery and MCP registry indexing**
Write metadata including tool descriptions, category ("Developer Tools / Agent Runtime"), and stdio config.

- [ ] **Step 3: Test local spawn over JSON-RPC**
Run `node packages/mcp/bin/superstate-mcp.js` and verify JSON-RPC `initialize` handshake succeeds.

- [ ] **Step 4: Commit MCP registry packaging**
```bash
git add packages/mcp/ smithery.yaml
git commit -m "feat(mcp): add npx runner and smithery registry configuration"
```

---

### Task 4: Empirical Technical Whitepaper & Academic Preprint

- **Depends on:** none

**Files:**
- Create: `/Users/danielfisher/repos/superstate-eval/docs/paper/superstate-architecture.md`
- Create: `/Users/danielfisher/repos/superstate-eval/docs/paper/figures/token_scaling.svg`
- Create: `/Users/danielfisher/repos/superstate-eval/docs/paper/figures/pass_rate_triad.svg`
- Modify: `/Users/danielfisher/repos/superstate-eval/README.md`

**Interfaces:**
- Consumes: Benchmark data from `results/frontier_triad_30tasks_reverted.json` and `results/frontier_mcp_harness_30tasks.json`
- Produces: Formal preprint paper detailing the architecture, mathematical model of cognitive anchoring, and complete empirical ablation tables.

- [ ] **Step 1: Extract final benchmark telemetry into publication figures**
Generate high-resolution SVG charts showing:
  - Test Pass-Rate Triad (Vanilla vs Superpowers vs Superstate)
  - Context Token Scaling ($O(N)$ vs $O(N^2)$)
  - Economic Total Cost of Ownership ($803 vs $853 vs $903)

- [ ] **Step 2: Write formal technical paper `superstate-architecture.md`**
Structure:
  - Abstract & Introduction: The Verification Deficit in Modern Agents
  - System Design: Deterministic State Gating & Stateless Milestone Retries
  - Experimental Setup: 30-Task Frontier Triad (DataCurve + TerminalBench)
  - Results & Discussion: 97.8% F2P completion, 100% regression safety, $0.13/task
  - Related Work: Comparison with LangChain, Devin, SWE-agent, AutoGen

- [ ] **Step 3: Update repository README with paper abstract and leaderboard**
Embed benchmark badges and key architecture diagrams directly in `README.md`.

- [ ] **Step 4: Commit paper and figures**
```bash
git add docs/paper/ README.md
git commit -m "docs(paper): publish technical whitepaper on deterministic state gating"
```

---

### Task 5: Launch Campaign Package (Show HN, Viral X Thread, Video Script)

- **Depends on:** Task 4

**Files:**
- Create: `/Users/danielfisher/repos/superstate/docs/launch/show_hn.md`
- Create: `/Users/danielfisher/repos/superstate/docs/launch/x_thread.md`
- Create: `/Users/danielfisher/repos/superstate/docs/launch/video_script.md`
- Create: `/Users/danielfisher/repos/superstate/docs/guides/cursor_quickstart.md`
- Create: `/Users/danielfisher/repos/superstate/docs/guides/claude_code_quickstart.md`

**Interfaces:**
- Consumes: Whitepaper narrative and benchmark results
- Produces: Polished, ready-to-publish launch copy and 60-second video demo storyboard.

- [ ] **Step 1: Write "Show HN" announcement post in `show_hn.md`**
Draft the exact copy, technical rationale, benchmark link, and terminal demo gif specs.

- [ ] **Step 2: Write viral X/Twitter launch thread in `x_thread.md`**
Hook: "Why do AI coding agents get stuck in infinite retry loops? Today we're releasing Superstate..." (8-tweet thread with graphs, video, and repo link).

- [ ] **Step 3: Write 60-second side-by-side demo script in `video_script.md`**
Timecoded script showing split screen of Claude Code failing on a bug vs Superstate self-healing in 40s.

- [ ] **Step 4: Write Cursor and Claude Code quickstart guides**
Provide copy-paste JSON configurations for `~/.cursor/mcp.json` and `claude_desktop_config.json`.

- [ ] **Step 5: Commit launch collateral**
```bash
git add docs/launch/ docs/guides/
git commit -m "docs(launch): prepare Show HN, viral social copy, and editor guides"
```

---

### Task 6: Turnkey GitHub Action (`superstate/action@v1`)

- **Depends on:** Task 1, Task 2

**Files:**
- Create: `/Users/danielfisher/repos/superstate/action/action.yml`
- Create: `/Users/danielfisher/repos/superstate/action/entrypoint.sh`
- Create: `/Users/danielfisher/repos/superstate/action/README.md`
- Test: `/Users/danielfisher/repos/superstate/.github/workflows/test-action.yml`

**Interfaces:**
- Consumes: GitHub Actions runner environment, GITHUB_TOKEN, test command
- Produces: Composite action that installs `superstate`, executes self-healing on failed CI runs, and comments on the PR with verification proof.

- [ ] **Step 1: Write composite `action.yml` specification**
Define inputs (`github_token`, `verify_cmd`, `model`, `api_key`) and steps to download native binary and initialize state.

- [ ] **Step 2: Write headless execution script `entrypoint.sh`**
Implement the worktree isolation, milestone dispatch, and PR comment generator.

- [ ] **Step 3: Write test CI workflow `test-action.yml`**
Run the action against a synthetic failing test in GitHub Actions and verify it catches the error and exits cleanly.

- [ ] **Step 4: Commit GitHub Action**
```bash
git add action/ .github/workflows/test-action.yml
git commit -m "feat(action): implement turnkey superstate-action for autonomous CI repair"
```
