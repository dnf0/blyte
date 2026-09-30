# Codex: the Escalation That Works, and "Untested" Off — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-30):** the first end-to-end run under Codex CLI (0.157, `gpt-5.6-terra`) worked only
once Codex was told to set `sandbox_permissions` to `"require_escalated"` on `blyte dispatch`. With the
skill's old wording ("request escalated permissions"), Codex ran the dispatch inside its sandbox,
where `.git` is read-only and there is no network, so it failed, and Codex then wrote the code
itself. The skill now names the mechanism, says merges need escalation too, and says to stop rather
than implement; and the "(untested)" labels come off, since the workflow now has been run end to end.

## Global Constraints

- Work only in your own worktree. Existing tests pass, except the two edits to
  `crates/blyte-cli/tests/test_skill_codex.rs` below (the "(untested)" label check is inverted).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: the Codex wording and labels

**Files:** `crates/blyte-cli/assets/blyte-implementer/SKILL.md`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/tests/test_skill_codex.rs`, `README.md`, `docs/orchestration.md`.

Make each replacement exactly (each old text occurs once). A short script is easiest.

- [ ] **Edit 1: `crates/blyte-cli/assets/blyte-implementer/SKILL.md`.** Replace exactly this text:

````text
**Under Codex:** run `blyte dispatch` and `blyte review` outside Codex's sandbox (request
escalated permissions for them; `blyte skill install --harness codex` pre-approves exactly these
two commands). They need the network to reach blyte's model, and blyte applies its own sandbox
to everything the implementer runs.
````

  with:

````text
**Under Codex:** run `blyte dispatch` and `blyte review` outside Codex's sandbox, with escalated
permissions: set `sandbox_permissions` to `"require_escalated"` on that shell command, with a
one-line justification. They reach blyte's model over the network and create git worktrees, which
Codex's sandbox does not allow (it makes `.git` read-only); blyte applies its own sandbox to
everything the implementer runs. `blyte skill install --harness codex` pre-approves exactly these
two commands, so no prompt appears. Merging a result into the main checkout also writes to `.git`:
run it with escalated permissions too, or leave the merge to the user. If `blyte dispatch` cannot
run, stop and say why; do not implement the change yourself.
````

- [ ] **Edit 2: `crates/blyte-cli/src/main.rs`.** Replace exactly this text:

````text
"Codex now writes the plan and delegates implementation to blyte, in parallel, by default (untested)"
````

  with:

````text
"Codex now writes the plan and delegates implementation to blyte, in parallel, by default"
````

- [ ] **Edit 3: `crates/blyte-cli/tests/test_skill_codex.rs`.** Replace exactly this text:

````text
    assert!(String::from_utf8_lossy(&out.stdout).contains("(untested)"));
````

  with:

````text
    assert!(!String::from_utf8_lossy(&out.stdout).contains("(untested)"));
````

- [ ] **Edit 4: `crates/blyte-cli/tests/test_skill_codex.rs`.** Replace exactly this text:

````text
Untested against a live Codex.
````

  with:

````text
Run end to end against a live Codex on 2026-09-30.
````

- [ ] **Edit 5: `README.md`.** Replace exactly this text:

````text
rules that let `blyte dispatch` run outside Codex's sandbox. Untested end to end so far.
````

  with:

````text
rules that let `blyte dispatch` run outside Codex's sandbox. Run end to end with Codex CLI on 2026-09-30.
````

- [ ] **Edit 6: `docs/orchestration.md`.** Replace exactly this text:

````text
## With Codex (untested)

````

  with:

````text
## With Codex

````

- [ ] **Edit 7: `docs/orchestration.md`.** Replace exactly this text:

````text
Codex has no notification when a background command exits, so it starts parallel dispatches
together and waits for them. The rules file has been checked with `codex execpolicy check`; the
whole workflow has **not yet been run end to end under Codex**.
````

  with:

````text
Codex has no notification when a background command exits, so it starts parallel dispatches
together and waits for them.

Codex must ask for escalated permissions on those two commands (`sandbox_permissions:
"require_escalated"`): the rules file approves an escalation, but a command run without one stays
in Codex's sandbox, where `.git` is read-only and there is no network, so `blyte dispatch` cannot
create its worktree. The skill tells Codex so. Merging a result also writes to `.git`, so Codex
needs escalation for it too, or leaves the merge to you.

**Run end to end on 2026-09-30** (Codex CLI 0.157, `gpt-5.6-terra`, a small Python task with a
`unittest` gate): Codex picked up the skill from the `AGENTS.md` default, wrote the task, ran
`blyte dispatch` outside its sandbox, followed it with `blyte status`, reviewed it with `blyte
review --diff` and left the verified commit on `wip/T1` for the merge (blyte: 27 turns, under two
minutes, $0.19). Before the skill named `sandbox_permissions`, Codex ran the dispatch inside its
sandbox, where it failed, and then wrote the code itself; `codex exec` also needs
`--sandbox workspace-write` and `-c approval_policy=on-request` for escalation to be possible.
````

- [ ] **Run the checks and commit.**

```bash
git add crates/blyte-cli/assets/blyte-implementer/SKILL.md crates/blyte-cli/src/main.rs crates/blyte-cli/tests/test_skill_codex.rs README.md docs/orchestration.md
git commit --no-gpg-sign -m "fix(cli): Codex requests escalation for blyte dispatch; drop the untested labels"
```
