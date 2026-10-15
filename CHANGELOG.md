## v0.32.1 (2026-10-15)

### Fix

- **provider**: schedule gemini-4-argon pricing from 2026-12-01 (#274)

## v0.32.0 (2026-10-15)

### Feat

- **gemini**: stream responses with an idle timeout (#271)

## v0.31.0 (2026-10-15)

### Feat

- **agent**: the orchestrator dispatches early (#270)

## v0.30.1 (2026-10-15)

### Fix

- **cli**: report usage and cost when the watchdog stops a run (#266)

## v0.30.0 (2026-10-14)

### Feat

- **cli**: choose the default model during blyte skill install (#263)

## v0.29.1 (2026-10-14)

### Fix

- **agent**: return an error, not a panic, when read's end_line is before start_line (#262)

## v0.29.0 (2026-10-14)

### Feat

- **orchestrate**: headless orchestrator with a host dispatch tool (#260)

## v0.28.1 (2026-10-14)

### Fix

- **release**: bump blyte-agent's blyte-provider pin with the version (#257)

## v0.28.0 (2026-10-14)

### Feat

- publish to crates.io from dnf0/blyte (#255)

## v0.27.0 (2026-10-12)

### Feat

- **skill**: status line refreshes every 30s so dispatch costs update while idle (#253)

## v0.26.0 (2026-10-08)

### Feat

- **skill**: keep context rules in the global CLAUDE.md (#251)

## v0.25.0 (2026-10-07)

### Feat

- **dashboard**: tag jobs with the session that dispatched them
- **plan**: blyte plan lists a plan's tasks, their state and dispatch commands
- **orchestrate**: start a plan from blyte plan, not from blyte's source
- **skill**: installing for Claude Code sets the status line to blyte statusline
- **statusline**: blyte statusline, Claude Code's line with blyte's costs
- **config**: allow_read in blyte.toml for paths every dispatch may read
- **usage**: log a Claude Code session's dispatches under its session id

### Fix

- **dashboard**: a job's title is its task heading
- **agent**: an empty gate is skipped instead of run
- **statusline**: an --after command that does not read its input no longer kills blyte
- **plans**: dependencies are the plan's task ids in the Depends on clause
- **repl**: /state shows the instructions' length, not the whole prompt

## v0.24.0 (2026-10-07)

### Feat

- **init**: propose uv sync as the worktree setup for uv repositories
- **dispatch**: run blyte.toml's setup in each new worktree before the implementer
- **usage**: blyte cost and the session's combined cost in the REPL
- **usage**: give interactive sessions and dispatches ids the usage log can attribute
- **usage**: log every run's tokens and cost to a machine-wide usage file
- **repl**: blyte --orchestrate runs interactive blyte as the orchestrator that dispatches implementers
- **repl**: keep the conversation across prompts so a reply to the model's question has its context
- **agent**: orchestrator instructions: preamble, skill and the user's orchestrator.md
- **wait**: blyte wait blocks until dispatched runs finish and prints one line per run

### Fix

- **cli**: a one-word prompt is an error, so an unknown subcommand never starts an agent
- **shell**: let BLYTE_SESSION reach commands so dispatches inherit the session id
- **dispatch**: say when a plan heading is hidden inside an unclosed code block
- **gemini**: send a reply's text and tool calls back as one model turn
- **wait**: give a dispatch started just before blyte wait time to write its start file
- **agent**: show a tool's whole command on its event line instead of the first 70 characters
- **agent**: tell the orchestrator to start on the plan instead of reading --help
- **wait**: wait a grace period before calling a run dead, so a run that is writing its summary is not misreported
- **repl**: show the model's whole reply instead of cutting it at 80 characters

## v0.23.1 (2026-10-07)

### Perf

- **dashboard**: the Plans view builds only running plans and never overlaps requests (#243)

## v0.23.0 (2026-10-07)

### Feat

- **provider**: vertex/<model> reaches Gemini through Vertex AI with application-default credentials (#241)

## v0.22.0 (2026-10-06)

### Feat

- **dashboard**: every run belongs to a plan; a multi-repository plan is one plan with a lane per repository (#239)

## v0.21.1 (2026-10-06)

### Fix

- **skill**: dispatch plan tasks without a task file; review starts from the checks section (#237)

## v0.21.0 (2026-10-06)

### Feat

- **review**: --model-review asks the implementer model for a first review (trial) (#235)

## v0.20.0 (2026-10-06)

### Feat

- **dispatch**: take the task from --plan and --plan-task when no task file is given (#233)

## v0.19.0 (2026-10-06)

### Feat

- **review**: a checks section flags files and commits the task does not name (#231)

## v0.18.1 (2026-10-02)

### Fix

- **skill**: a task touching more than about 20 files names its script or is split (#226)

## v0.18.0 (2026-10-02)

### Feat

- tighter model timeouts and research report reminders (#223)

## v0.17.0 (2026-10-02)

### Feat

- **clean**: follow-up chains and squash-merged runs count as merged; a cheap dispatch-time clean (#220)

## v0.16.0 (2026-10-02)

### Feat

- **cli**: warn when the gate misses CI checks; flag a superseded blyte-research skill; skill guidance from heavy use (#218)

## v0.15.0 (2026-10-02)

### Feat

- **clean**: work merged into the run's base branch counts as merged; report sessions' merged worktrees (#216)

## v0.14.0 (2026-10-02)

### Feat

- **plans**: a dispatched plan file is its own plan, wherever it lives (#214)

## v0.13.0 (2026-10-01)

### Feat

- **cli**: blyte clean removes worktrees once their work is merged (#212)

## v0.12.0 (2026-10-01)

### Feat

- **plans**: follow-up tasks dispatched with a plan show as added during the work (#210)

## v0.11.2 (2026-10-01)

### Fix

- **dashboard**: an honest Needs attention list, running-only Plans, one colour per node (#208)

## v0.11.1 (2026-10-01)

### Fix

- gate slots stop eating deadlines, failing tests survive the output cut, Plans shows the run that counts (#206)

## v0.11.0 (2026-10-01)

### Feat

- honest errors (blocked, watchdog progress) and a 300-turn research default (#204)

## v0.10.0 (2026-10-01)

### Feat

- **cli**: the dashboard's Plans view; research reads the repository directly (#202)

## v0.9.0 (2026-10-01)

### Feat

- **cli**: research budget, update refreshes every skill copy, review flags outside reads (#200)

## v0.8.0 (2026-10-01)

### Feat

- **cli**: blyte dispatch --research, and run reports record the effort level (#198)

## v0.7.8 (2026-10-01)

### Fix

- **cli**: the dashboard names a dispatch after its repository, not its worktree's folder (#196)

## v0.7.7 (2026-10-01)

### Fix

- Node tool caches under the sandbox, and init drops overlapping pytest steps (#194)

## v0.7.6 (2026-10-01)

### Fix

- **cli**: the dispatch rules tell the model not to run the full suite itself (#192)

## v0.7.5 (2026-10-01)

### Fix

- **cli**: the running dashboard sees newly registered repos; init recognises python -m checks (#190)

## v0.7.4 (2026-09-30)

### Fix

- **cli**: linked worktrees use the main checkout's blyte.toml; init falls back when CI is stale (#188)

## v0.7.3 (2026-09-30)

### Fix

- **cli**: the repository registry gets one whole line per dispatch, and no temp repositories (#185)

## v0.7.2 (2026-09-30)

### Fix

- **provider**: the default Gemini model runs at high effort, as the docs say (#180)

## v0.7.1 (2026-09-30)

### Fix

- **cli**: a run never outlives --max-seconds by more than 30 s, and always leaves a report (#179)

## v0.7.0 (2026-09-30)

### Feat

- **cli**: blyte owns the dispatch workflow: verdicts, env, a proven init, a global dashboard (#178)

## v0.6.0 (2026-09-30)

### Feat

- **cli**: the dashboard in blyte's palette, type and mark (#177)

## v0.5.0 (2026-09-30)

### Feat

- **cli**: dashboard rows show the job id (#175)

## v0.4.0 (2026-09-30)

### Feat

- **cli**: blyte dashboard, a local page of running and recent jobs (#172)

## v0.3.0 (2026-09-30)

### Feat

- **cli**: blyte update installs the latest release, with --check and --rollback (#169)

## v0.2.3 (2026-09-30)

### Fix

- **license**: standardise on Apache-2.0 (#166)

## v0.2.2 (2026-09-30)

### Fix

- **cli**: Codex requests escalation for blyte dispatch; drop the untested labels (#164)

## v0.2.1 (2026-09-30)

### Fix

- **build**: the sibling crate pins follow the release version (#162)

## v0.2.0 (2026-09-30)

### Feat

- **agent**: load the repository's rules for agents on every run (#160)

## v0.1.1 (2026-09-30)

### Fix

- **cli**: skill install --no-default removes an earlier default block (#156)

## v0.1.0 (2026-09-29)

### Feat

- **cli**: read provider keys from ~/.config/blyte/env when they are not exported (#151)
- **agent**: the run report records the turn of each gate attempt (#146)
- **agent**: read takes several paths, so one call can bring several files into one turn (#136)
- **provider**: Gemini on Vertex AI with the user's Application Default Credentials (vertex/ model prefix) (#135)
- **agent**: the prompt shows the model how to batch independent calls in one turn (#134)
- **agent**: ungated, blyte re-runs the searches that found the problems before it stops (#122)
- **agent**: a gate retry runs the failing steps first and stops if they still fail (#121)
- **agent**: gates take a machine-wide slot, so parallel runs do not all build and test at once (#119)
- **cli**: blyte status shows what a running dispatch is doing (#118)
- **agent**: on Linux, commands can read only system directories, the workspace, tool caches and grants (#116)
- **cli**: blyte.toml records a repository's gate and budgets; blyte init detects them (#115)
- **cli**: blyte skill install --harness codex sets up Codex CLI as the orchestrator (#109)
- **cli**: installing the skill makes blyte Claude Code's default implementer, with parallel dispatch (#107)
- **cli**: blyte dispatch, blyte review and blyte skill install (#103)
- **cli**: the run report records the sandbox, network mode, read grants and withheld environment names (#100)
- **agent**: --network=off stops the commands blyte runs from connecting out (#99)
- **agent**: on Linux blyte's shell runs under Landlock, refusing writes outside the workspace (#97)
- **agent**: commands no longer inherit credential-looking environment variables (#96)
- **agent**: the macOS sandbox refuses reads of credential stores and .env files (#95)
- **agent**: --max-cost stops a run once list-price spend reaches the cap (#94)
- **provider**: ChatGPT-login provider (chatgpt/) (#92)
- **agent**: on macOS blyte's shell runs sandboxed, refusing writes outside the workspace (#91)
- **report**: keep each failed gate attempt's output; name the failing steps (#88)
- **provider**: OpenAI-compatible provider (openai/, openrouter/) (#82)
- **report**: model, tool and gate time per run, and calls per tool (#77)
- **cli**: the binary knows the commit it was built from (#76)
- **agent**: glob respects .gitignore and skips .git (#65)
- fast tools — persistent shell sessions, native ripgrep, one HTTP client (#63)
- **agent**: model-free harness benchmark (#62)
- **agent**: adaptive context — fold budget grows on re-fetch of folded content (#59)
- per-turn usage in the run report, and --fold-budget (#57)
- resilience — server retry hints, cost cap, journal and resume (#56)
- **agent**: fold notes (--fold-notes) (#55)
- folded default context, completeness review gate (--review), compact context (#53)
- **agent**: parallel tool calls travel as whole turn blocks (#51)
- **agent**: folded context and provider retry; superstate matches agy with history kept (#50)
- **agent**: state-window context keeps the last tool results in view (#48)
- native harness autonomous TDD gating (--tdd) (#47)
- native harness eval surface (run report, deadline, ablatable loop-breaker, effort parity) (#46)
- **agent**: implement harness-observed working set and loop-breaker guard (#44)
- **provider**: multi-provider cloud model discovery and interactive selection (#43)
- **cli**: unified interactive agent harness and headless solver (#42)
- **agent**: shell process groups and container seam parity (#41)
- **guard**: deterministic PreToolUse file read guard and AST shunting (#39)
- **daemon**: milestone-scoped test impact slicing (#38)
- **core**: add DaemonError::Blocked, unique milestone IDs, and strict delimiter matching
- **cli**: verify and test diamond DAG daemon execution from CLI
- **daemon**: wire DAG milestone transitions, checkpoint commits, and scoped rollback
- **core**: implement DAG milestone resolution and deadlock detection in advance_step
- **shunt**: implement native tool output shunting layer (Spotify Portal pattern) (#35)
- **daemon**: enable deterministic git rollback by default (#34)
- **daemon**: implement deterministic git rollback on verification failure (#33)
- **daemon**: add advisory gate mode to match superstate_mcp's stopping rule
- **daemon**: do not accept an unconditional gate pass as completion
- **daemon**: make the retry budget progress-aware
- **core**: implement automatic worktree merge and completion artifact on daemon success
- **mcp**: streamline tool catalog to strict detached handoff gateway
- **mcp**: support from_agent, to_agent, and turnkey resumption in superstate_handoff
- **cli**: add superstate state handoff subcommand for cross-agent transitions
- **core**: implement standardized cross-agent handoff artifact and resumption generator
- **core**: implement autonomous driver preset expansion for claude and agy
- **mcp**: add multi-daemon fleet visibility to state_list, state_status, and handoff
- **cli**: support multi-daemon fleet management, state-scoped PIDs, and worktree integration
- **core**: harden DaemonRunner with StepAdvancer, driver rescue, and rich retries
- **mcp**: support detached autonomous execution in superstate_handoff
- **cli**: implement detached daemon lifecycle and interrogation commands
- **core**: implement DaemonRunner for autonomous milestone execution
- **mcp**: add schema parity and end-to-end progressive verification
- **core**: implement infer_impacted_tests for automated test slicing
- **mcp**: support verify_stage and generous pre-merge timeout for progressive verification
- **mcp**: implement worktree_merge pre-merge regression verification gate
- **Task 6**: Full Verification, Binary Build & End-to-End Suite
- **Task 5**: Worktree Lifecycle Handlers (`crates/superstate-mcp/src/server.rs`)
- **mcp**: implement worktree_list, worktree_remove, and worktree_clean handlers
- **Task 4**: Maintenance, Reconciliation, DAG & Parameter Parity (`crates/superstate-mcp/src/server.rs`)
- **mcp**: implement maintenance, reconciliation, DAG tools, and parameter parity
- **Task 3**: Safety, Checkpoint, Rollback & Atomic Exec Handlers (`crates/superstate-mcp/src/server.rs`)
- **mcp**: implement state_checkpoint, state_rollback, and state_exec handlers
- **Task 2**: State Inspection, Status & Briefing Handlers (`crates/superstate-mcp/src/server.rs`)
- **mcp**: implement state_status, state_show, state_resume, and state_metrics handlers
- **mcp**: declare schemas for all CLI parity tools and parameters
- **cli,mcp**: default state init to in-place execution with explicit worktree opt-in and MCP tool parity
- **core**: record global goal in ExecutionState and render in turnkey prompts
- **cli**: add --ignore-untracked-glob support to state next for full agent-rules compatibility (#15)
- **worktree**: namespace worktree branches and paths by feature branch slug
- **mcp**: support branch-scoped state resolution for concurrent agents
- **cli**: add branch-scoped state resolution and state list command
- **core**: add branch-scoped state paths and worktree reverse lookup
- **mcp**: support multi-milestone handoff and sequential worktree transitions
- **cli**: add --milestones flag and automatic sequential worktree progression
- **worktree**: add current_branch detection and target active feature branch
- **core**: add MilestoneSpec and init_state_milestones DAG initialization
- **mcp**: implement superstate_preflight and superstate_handoff with tools/call dispatch
- **cli**: support direct state initialization from goal and verify-cmd
- native worktree milestone lifecycle (#11)
- **cli**: add superstate worktree subcommand with create, list, merge, remove
- **worktree**: implement worktree lifecycle engine with atomic merge and destroy
- **worktree**: implement reflink cache sharing powered by worktrunk
- **worktree**: scaffold superstate-worktree crate with worktrunk dependency
- **cli**: auto-initialize git baseline and harness exclusions in state init
- **state**: complete the command surface — sync, exec, doctor, validate, apply, resume, metrics, decompose
- **state**: add `checkpoint` and `rollback`
- **state**: share one command surface, target prompts by task, replay the ledger
- **state**: add `state ready` for DAG-unblocked task selection
- **cd**: add gated crates.io CD workflow and configure manifest dependency versions
- **cli**: implement unified superstate CLI, 5-step handshake, doctor, and documentation
- **rules,mcp**: implement minijinja guidance generator and native stdio MCP server
- **review**: implement daemon-less adversarial code review engine and diff analyzer
- **syntax**: implement native polyglot tree-sitter AST symbol extractor
- **core**: implement deterministic state machine, plan parser, and multi-gate verifier

### Fix

- **agent**: withhold AWS_PROFILE and AWS_DEFAULT_PROFILE with the credentials (#150)
- **cli**: skill install says Claude Code writes the plan and delegates, not plans and reviews (#148)
- **agent**: the run report keeps the head and the tail of a failed gate's output (#143)
- **agent**: the grep tool's fallback uses extended regex, so alternation matches without rg (#124)
- **agent**: gate slots never deadlock: only the blyte command takes them, and not from inside a gate (#123)
- **agent**: grep shows the matches inside a long line (#117)
- **cli**: the skill runs dispatch in the foreground in a headless session (#105)
- **agent**: commands keep the inherited PATH in front of the login profile's (#104)
- **agent**: an exhausted usage quota stops the run at once; ChatGPT login problems are authentication errors (#98)
- **agent**: the shell session recreates its temp directory before each command (#93)
- **agent**: ungated, the prompt says nothing checks the work and asks for a full test run (#89)
- **agent**: blyte tells the model to change nothing outside the repository (#87)
- **agent**: commands never inherit provider credentials or BLYTE_* configuration (#69)
- **agent**: the event log shows a read's line range (#58)
- **agent**: bound every tool result by size; grep fallback skips dependency trees (#52)
- **review**: ignore sigpipe on unix in code reviewer and test diagnostics
- **daemon**: ignore sigpipe on unix and enforce terminal rollback on failure
- **daemon**: pass --no-gpg-sign on in-place milestone checkpoint commits
- **core**: fail closed on initial deadlock and prevent prefix collisions in dependency satisfaction
- **daemon**: detect progress inside untracked directories
- **core**: fall back to root state.json on feature branch when branch state does not exist
- **core**: ensure state_path resolution in resume briefing and sanitize slug in handoff package
- **core**: ensure token-exact matching and agent-specific permission flags in driver preset resolution
- add cross-platform kill_pid support and eliminate concurrent completion.json race
- **core,e2e**: scope completion report by slug and verify genuine branch worktree isolation in fleet test
- **cli**: resolve daemon pid cleanup, ancestor state resolution, and cancel deduplication
- **core**: bound driver stderr note and write prompt to stdin in background thread
- **mcp**: relativize synthesized test cmd to worktree and enforce as blocking gate
- **mcp**: handle JSON-RPC notifications gracefully without responses
- **mcp**: align inputSchema naming, state_apply oneOf patch, and worktree slug matching
- **cli,mcp**: refine worktree defaults, exclude initialization, and MCP schema parity
- **core**: skip rendering global goal heading when goal is empty or whitespace
- **worktree**: stage pre-existing files in baseline commit before worktree creation
- **core**: centralize project root resolution and turnkey prompt state contract
- **cli**: address code review findings for state list and prompt path quoting
- **cli**: capture base_branch and safe unwrap in single goal init
- **core**: address Task 1 review findings in state machine and cli tests
- **mcp**: enforce verification gates, worktree merge, and preflight timeout in MCP server
- **cli**: robust cwd starts_with fallback and detached head test
- **cli**: guard detached HEAD in merge and active CWD in remove
- **cli**: validate base branch, prevent cache conflict, and add inner cwd tests
- **cli**: guard primary worktree and resolve repo root in merge and remove
- **state**: find the project's state from anywhere inside it
- **state**: use the relative-path-safe repo root everywhere
- **state**: accept checkbox plans and record gate failures in state
- **cd**: handle dry-run verification for interdependent workspace crates

### Refactor

- **agent**: remove the commit footer (it did not reduce git-inspection turns) (#86)
- remove --review and --tdd (the orchestrator owns completeness) (#70)
- remove the note tool; sweep dead code and unused dependencies (#68)
- remove the loop-breaker, working set and action ledger (#67)
- remove what did not earn its place (#66)
- rename superstate to blyte (#61)
- **mcp**: deduplicate daemon status resolution and harden test lifecycle
- **worktree**: simplify init status fallback check
- **ledger**: read both locations instead of migrating
- **ledger**: keep findings in an append-only file beside the state

### Perf

- **provider**: Anthropic requests mark cache breakpoints; cache writes count as input (#85)
- **agent**: a successful git commit reports the new commit and the tree state (#84)
- **agent**: read keeps lines up to 2,000 characters whole (#83)
- **provider**: Ollama thinking is off unless the model name ends in +think (#79)
- **agent**: the prompt asks for the gate's quick checks before stopping (#74)
- **agent**: a read from line 1 of a file that fits returns the whole file (#73)
- **agent**: a multi-line gate runs every step and reports all failures at once (#72)
- **agent**: read returns whole files up to 2,000 lines (#71)
- **agent**: lean session protocol and native glob (#64)
- **agent**: compact masks superseded outputs only at folds (#54)
- **agent**: keep a stable prompt prefix so Gemini's implicit cache applies (#49)
