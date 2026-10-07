# Context follow-up: pick up on 2026-10-08

Start a new session in `~/repos/blyte` and read this file first.

## Done on 2026-10-07

- **CR1 merged** (`2d89fd6`, verdict accepted). blyte keeps a `<!-- blyte:context -->` block of context
  rules in `~/.claude/CLAUDE.md`. `blyte skill install` writes it, `uninstall` removes it, and every
  `blyte dispatch` puts it back if it is missing or out of date. Plan:
  `docs/superpowers/plans/2026-10-07-context-rules.md`.
- The block is installed on this machine. The old "Keep sessions small" rules in the global CLAUDE.md
  were cut down to the facts specific to this machine (backup of the earlier file in that session's
  scratchpad, not kept).
- New specs and plans go in `docs/specs/` and `docs/plans/` (`093a381`; global CLAUDE.md updated).
  `docs/superpowers/` keeps the older ones.
- Decided against a central plans vault (Karpathy's LLM Wiki): plans stay in the repo with their code.
  Revisit only if the same domain knowledge keeps being re-explained to new sessions.
- Release binary rebuilt: `blyte --version` shows `0.25.0 (2d89fd6)`.

## State to know

- `main` is **3 commits ahead of `origin/main`** and not pushed (`e399e27`, `093a381`, `2d89fd6`).
  Releases go by PR (v0.25.0 was #250), so these need a branch and PR, or a push if the user says so.

## To do, in order

### 1. Get the three commits onto origin

Ask the user how: push to `main`, or move them to a branch and open a PR. Then decide whether CR1 is
worth a release (v0.26.0) so that `blyte update` gives other users the context block.

### 2. Find out why the 822K session never auto-compacted

`~/.claude/settings.json` has `autoCompactWindow: 200000` and `autoCompactEnabled: true`, yet a
session reached 822K on 2026-10-07. Check, read-only:
- whether that session ran on `opus[1m]`, and whether it started before the setting was added;
- whether `autoCompactWindow` is honoured on the 1M model (the `claude-code-guide` agent can answer).
If the setting does not work there, that alone explains the 822K, and it is the cheapest fix.

### 3. Decide whether hooks are still needed

Proposed on 2026-10-07 and not started. Wait for item 2 and a few days with the new rules first.
- **Context guard**: a `UserPromptSubmit`/`PostToolUse` hook (`blyte hook`, installed by
  `blyte skill install`) that reads the last turn's token count from the transcript. Past a soft limit
  (~120K) it adds a note telling Claude to suggest `/compact`; past a hard limit (~300K) it blocks the
  prompt with that message.
- **Output-size nudge**: after a Bash result over ~8KB, one line telling Claude the size and to trim at
  the source next time.
Only worth building if sessions still grow past the window with the rules in place.

### 4. Measure

Around 2026-10-10 to 10-13, re-run the context breakdown from 2026-10-06 (see memory
`context-breakdown-2026-10-06`) and compare the Bash share (was ~45%) and session sizes. That is the
evidence for or against item 3.
