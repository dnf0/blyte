# Roadmap

What is planned next for blyte, what is waiting on something, and what is parked. Dated items are
decisions; the evidence behind them is in `docs/evidence.md` and the design plans. Every release
is listed in `CHANGELOG.md`.

## In progress

- **Follow-ups to the Linux shell read protection (2026-11-28,
  `docs/plans/2026-11-28-follow-ups.md`):** on Linux a home folder under `/var` or `/opt` is
  supported; the unchecked profile builders are private; `blyte plan` accepts a task heading with
  no colon; `blyte update` updates the way blyte was installed, with no GitHub CLI. The Linux
  parts are confirmed only by the Linux test run after the merge.

## Next

The goal is a surface that does a whole change alone, on Gemini: plan, dispatch, review, publish
and land. A check of the code on 2026-11-28 found every task of the two 2026-11-26 plans merged,
and these gaps, in the order they block that goal:

- **A surface is not told when its request is combined,** and cannot look up a request's state,
  so it never asks to publish on its own.
- **A verified run with review flags stalls silently:** combining waits for a person's verdict and
  the surface is not woken to see the flag.
- **`reference.md` still says `--gate` overrides the gate;** the surface's tool refuses it.
- **A plan written by direct edit is not on `HEAD`,** so dispatching from it needs `--base wip/<id>`,
  which the instructions do not say.
- **Landing:** a surface cannot check a pull request, merge it, or bring `main` up to date, and so
  cannot finish a release either. What a surface may merge without the user is a decision to take
  first.
- **Publishing needs a click on the page every time,** and fails for a surface in a terminal. The
  click is deliberate; whether to allow it to be waived is the same decision.
- **Shell output is not checked for credential content** before it reaches the model. On Linux a
  workspace `.env` is readable by the shell (`docs/security.md`). A design is being researched.

## Done

Kept for about the last four weeks; older items are in `CHANGELOG.md` and the git history.

- **2026-11-28** On Linux a run's shell is not granted folders that hold credentials: every grant
  is compared with the protected paths before a profile is returned (#462). The surface shows
  what is waiting on the user, starts in another folder, and opens as an app window (#458).
- **2026-11-25 to 2026-11-27** Every `blyte` start is its own surface that owns its runs; a
  machine-wide list of surfaces (`blyte surfaces`, `blyte open`); surfaces message each other with
  `tell` (#425, #427, #429). A surface starts, dispatches, reviews and undoes runs in a folder that
  is not a git repository (#435, #448). A surface can ask to publish with the user's approval, and
  the gaps from the self-hosting trial are closed. Images, files and long text can be dropped into
  the surface (#444); a resumed surface's model remembers the conversation (#454).
- **2026-11-21 to 2026-11-24** Bare `blyte` starts a live surface (#377). The conversation model's
  shell is sandboxed (#382). A request for work is one run by default and blyte combines a
  request's runs (#402). Direct edits for small changes, gated and recorded as a run (#406). The
  check tests only the crates a change affects (#407).
- **2026-11-18 to 2026-11-20** Research reports quote code by reference, state what was read and
  are marked when unverified (#332, #334, #344). The gate fails on comments a run adds (#364); a
  run stops as blocked when the gate cannot run (#362). The default dispatch model is
  `gemini-4-neon` (#365). `blyte mock` replays real runs through the surface (#371).
- **2026-11-04** `blyte skill install` sets Claude Code's auto-compact window to 200k (#324).

## Parked

- **A shared Cargo build directory for worktrees.** Measured 2026-10-01: a warm build is 15 s of a
  gate of about two minutes, which the test run dominates (its two slowest tests were then cut from
  32 s to 1 s each). Worth it only if builds come to dominate; it would need a sandbox allowance,
  and parallel dispatches would queue on cargo's lock.
- **Codex as the planner, step 2.** The blocker (Codex's 5-minute MCP tool timeout) is fixed in
  blyte-eval #93; rerunning the Codex + blyte against Claude Code + blyte comparison is parked
  (2026-10-02): the failures were likely down to the Codex subscription level.

- **Open-sourcing blyte:** done. The repository is public at `github.com/dnf0/blyte` and every release
  publishes the crates to crates.io. `test_public_names` still fails on any internal name
  in the files.
- **Docs hosting:** done. GitHub Pages serves the docs at <https://dnf0.github.io/blyte/>, deployed by a
  workflow on merge to `main`. Still open: a custom domain (about USD 10-50 a year), which would keep
  links stable if the repository moves.
- **Still open now that the repository is public:**
  - Contributions: a DCO sign-off or a CLA. Put off on 2026-11-28: no outside contributions are
    expected, and Apache-2.0 section 5 already puts a submitted contribution under the project's
    licence. Decide when the first outside pull request arrives; a CLA is needed only to keep the
    option of relicensing.
