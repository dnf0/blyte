# Superstate Detached Handoff Protocol & Execution Engine Design

## 1. Problem Statement & Motivation

Modern AI-assisted engineering conflates two opposing paradigms into a single chat window:
1. **Interactive Brainstorming (High Ambiguity, Conversational):** The human and AI explore requirements, compare architectural designs, inspect codebase schemas, and establish invariants.
2. **Autonomous Execution (Low Ambiguity, Deterministic):** The code is constructed, verified with tests, committed atomically, and merged.

### The Undiagnosed Friction Today:
* **The Chat Trap:** When a conversational LLM finishes brainstorming, it currently attempts to execute the code *inside the same chat window*. It carries 50,000+ tokens of brainstorming dialogue into every coding turn, causing exponential token costs, rate limits, and conversational drift.
* **The LLM-as-Supervisor Failure:** An LLM cannot reliably supervise its own execution. Without an external harness forcing verification, LLMs hallucinate completion, skip test runs, and declare "Done!" prematurely.
* **The Passive Handoff Flaw:** Current MCP handoffs return a prompt string back to the calling chat session. The chat agent simply hands off to *itself*, leaving the developer trapped babysitting the token stream.

---

## 2. Core Architecture: The Detached Handoff

```
┌────────────────────────────────────────────────────────┐
│ 1. BRAINSTORMING IN CHAT (Claude Code / agy / Cursor) │
│                                                        │
│ Human & Agent establish:                               │
│ - Goal & target files                                  │
│ - Ordered milestones                                   │
│ - Concrete verification command (e.g. `pytest -q`)     │
└───────────────────────────┬────────────────────────────┘
                            │
                            │ Agent calls MCP:
                            │ `superstate_handoff(
                            │    goal="...",
                            │    milestones=[...],
                            │    verify_cmd="pytest -q",
                            │    detach=true
                            │ )`
                            ▼
┌────────────────────────────────────────────────────────┐
│ 2. SUPERSTATE MCP HANDOFF (THE DETACHMENT POINT)       │
│                                                        │
│ 1. Runs `superstate_preflight` on `verify_cmd`.        │
│ 2. Captures developer's current git branch (`base_branch`)│
│ 3. Provisions worktree `.wt/<job-id>`                  │
│ 4. Writes `.agent-state/state.json`                    │
│ 5. Spawns detached daemon: `superstate daemon --job ...`│
│ 6. Immediately returns to chat agent:                  │
│    { "status": "detached", "job_id": "job-9a1b", ... } │
└───────────────────────────┬────────────────────────────┘
                            │
              Chat closes   │ Detached process
              Human is free │ runs in background
                            ▼
┌────────────────────────────────────────────────────────┐
│ 3. SUPERSTATE DETACHED RUNNER (Condition B Engine)     │
│                                                        │
│ Loop per milestone:                                    │
│  a. Read active task from `.agent-state/state.json`    │
│  b. Render bounded prompt (`state_prompt`, 0 chat ctx) │
│  c. Invoke headless coder worker (`claude -p` / API)   │
│  d. Execute `verify_cmd` in isolated worktree          │
│  e. Pass: Commit checkpoint, merge to `base_branch`    │
│  f. Fail: Feed error diff back to worker (up to retry) │
│  g. Final: Merge worktree, write completion report     │
└───────────────────────────┬────────────────────────────┘
                            │
         Interrogate anytime│ Emits completion event
         `superstate status`│
                            ▼
┌────────────────────────────────────────────────────────┐
│ 4. COMPLETION & INTERROGATION                          │
│                                                        │
│ - Interrogation: `superstate status`, `superstate logs`│
│ - Completion: OS notification / terminal bell / webhook│
│ - Completion Artifact: `.agent-state/completion.json`  │
│   (Diffstat, verification proof, commit SHAs)          │
└────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Component Specifications

### A. The Handoff Contract (`superstate_handoff`)

```json
{
  "name": "superstate_handoff",
  "description": "Detach execution to the autonomous Superstate daemon in an isolated worktree.",
  "parameters": {
    "goal": { "type": "string", "description": "High-level objective" },
    "milestones": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "title": { "type": "string" },
          "verify_cmd": { "type": "string" },
          "files": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["title", "verify_cmd"]
      }
    },
    "verify_cmd": { "type": "string", "description": "Global verification fallback" },
    "driver": { 
      "type": "string", 
      "enum": ["auto", "claude", "agy", "anthropic_api", "gemini_api"],
      "default": "auto" 
    },
    "detach": { "type": "boolean", "default": true }
  },
  "required": ["goal"]
}
```

#### Response (Immediate Detachment):
```json
{
  "status": "detached",
  "job_id": "job-7c2a1e",
  "base_branch": "feat/user-auth",
  "worktree_path": ".wt/job-7c2a1e",
  "milestones_count": 3,
  "active_milestone": "1: Database migrations for sessions",
  "log_file": ".agent-state/logs/job-7c2a1e.log",
  "message": "Superstate has assumed execution ownership. Chat session is released. Monitor with `superstate status`."
}
```

---

### B. The Detached Daemon Runner (`superstate daemon`)

The daemon runs as a detached child process (`setsid` on Unix / background job):
1. **Zero Context Leakage:** The headless worker is spawned with `state_prompt` derived strictly from the current git tree, acceptance criteria, and epistemic ledger notes. Discarded chat tokens from the brainstorming phase never touch the execution turn.
2. **Verification Hard Gate:** After each worker turn, the daemon independently runs `verify_cmd` via `std::process::Command`. The worker has zero ability to declare success—only an exit code of `0` unlocks advancement.
3. **Branch-Scoped Merging:** Milestones are merged directly into the developer's working feature branch (`base_branch`), producing clean, atomic commits:
   * `feat(auth): database migrations for sessions (checkpoint: 4a9f1c)`
   * `feat(auth): JWT validation middleware (checkpoint: 8c3e2b)`
4. **Lease & Spending Safety:** Uses dynamic leases. If a worker gets stuck in a loop and cannot pass the gate within the lease allocation, the daemon halts the job, cleanly rolls back untracked changes, and flags the milestone as `blocked`.

---

### C. Live Interrogation Protocol (`superstate status` & MCP)

At any time while the detached daemon is running, the human or any LLM agent can query the execution state without interrupting it:

#### CLI Query:
```bash
superstate status
```
```
Superstate Job: job-7c2a1e [RUNNING] (PID: 41892)
Base Branch: feat/user-auth | Worktree: .wt/job-7c2a1e
Spend: $0.21 | Lease: 14:18 remaining

Milestones:
  [✓] 1. Database migrations for sessions (verified in 42s)
  [▶] 2. JWT validation middleware (Turn 1/3, verifying...)
  [ ] 3. Endpoints & route wiring

Last Verification Output (exit 1):
  FAILED tests/test_jwt.py::test_expired_token - AssertionError: Expected 401, got 500
```

#### MCP Tool: `state_status(json=true)`:
Allows another agent (or the original chat session if reopened) to inspect ground truth programmatically.

---

### D. Piping Completions Back

When the final milestone passes, the daemon:
1. Merges the worktree back into `base_branch`.
2. Cleans up `.wt/job-7c2a1e`.
3. Writes `.agent-state/completion.json`:
   ```json
   {
     "job_id": "job-7c2a1e",
     "status": "completed",
     "base_branch": "feat/user-auth",
     "commits": ["4a9f1c", "8c3e2b", "f109ad"],
     "milestones_completed": 3,
     "total_spend_usd": 0.42,
     "wall_clock_sec": 184.2,
     "verification_proof": "All 42 tests passed in 1.4s"
   }
   ```
4. Emits an OS notification / terminal bell / webhook so the developer is immediately alerted that the verified code is ready for review.
