# Task 1: MCP Tool Declarations & Schemas (`crates/superstate-mcp/src/tools.rs`)

- **Depends on:** none

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_tool_schemas.rs`

**Interfaces:**
- Consumes: `McpTool`, `McpToolInputSchema` from `superstate_mcp::tools`
- Produces: Complete list of MCP tool definitions in `list_all_mcp_tools()`:
  - `state_status`, `state_show`, `state_resume`, `state_metrics`
  - `state_checkpoint`, `state_rollback`, `state_exec`
  - `state_ready`, `state_sync`, `state_doctor`, `state_validate`, `state_apply`, `state_list`
  - `worktree_list`, `worktree_remove`, `worktree_clean`
  - Parameter additions to `state_next` (`ignore_untracked_glob`) and `state_prompt` (`with_graph`, `task`).

## Steps (TDD)

1. Write failing test in `crates/superstate-mcp/tests/test_mcp_tool_schemas.rs`:
   Verify `list_all_mcp_tools()` contains all 25 tools:
   - `state_init`
   - `state_show`
   - `state_status`
   - `state_next`
   - `state_ready`
   - `state_checkpoint`
   - `state_rollback`
   - `state_prompt`
   - `state_sync`
   - `state_exec`
   - `state_doctor`
   - `state_validate`
   - `state_apply`
   - `state_resume`
   - `state_metrics`
   - `state_list`
   - `state_note`
   - `worktree_create`
   - `worktree_merge`
   - `worktree_list`
   - `worktree_remove`
   - `worktree_clean`
   - `superstate_review`
   - `superstate_preflight`
   - `superstate_handoff`
   Also test that:
   - `state_next` schema has `ignore_untracked_glob` property (type string)
   - `state_prompt` schema has `with_graph` (type boolean) and `task` (type string)
   - `state_exec` schema has `verify_cmd` (type string, required: ["verify_cmd"]), `state`, `verify_git_clean`, `checkpoint`, `rollback_on_fail`, `advance`
   - `worktree_remove` schema has `branch` (type string, required: ["branch"]), `force`, `repo`
   - `state_sync` schema has `plan` (type string, required: ["plan"]), `state`, `out`

2. Run test to verify it fails:
   `cargo test -p superstate-mcp --test test_mcp_tool_schemas`

3. Implement tool definitions in `crates/superstate-mcp/src/tools.rs`:
   Add the missing tools and update `state_next` and `state_prompt` schemas.

4. Run test to verify it passes:
   `cargo test -p superstate-mcp --test test_mcp_tool_schemas`

5. Verify formatting and linting:
   `cargo clippy -p superstate-mcp && cargo fmt -p superstate-mcp --check`

6. Commit:
   `git add crates/superstate-mcp/src/tools.rs crates/superstate-mcp/tests/test_mcp_tool_schemas.rs`
   `git commit -m "feat(mcp): declare schemas for all CLI parity tools and parameters"`
