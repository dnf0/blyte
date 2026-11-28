# A surface opens a pull request unasked and merges only on the user's word

Date: 2026-11-28. Decided by the user. Code facts: research run PLR1 (2026-11-28).

## Decision

By default a surface works the way the user's planning harness does today:

- **Opening a pull request needs no approval.** When a request is combined, the surface's model
  may push its branch and open the pull request.
- **Merging needs the user's direct approval, every time.** No setting waives it.

## Today

`publish` asks for a click on the surface's page on every call
(`requests::publish_request_with_approval` calls `Approver::ask_publish`). A terminal surface has
no `publish` tool, and `TtyApprover::ask_publish` returns `Unavailable`. No tool merges: landing is
`scripts/land.sh`, run by a person or another harness.

## Part 1: publishing (plan `docs/plans/2026-11-28-publish-default.md`)

A new table in `blyte.toml`:

```toml
[surface]
publish = "auto"   # or "ask"
```

- `auto`, the default and the intended value: `publish` does not ask. Every other check stays:
  the request belongs to this surface, is combined, is not being combined, has no pull request
  yet, the turn was not started by another surface, and the branch has not moved.
- `ask`: today's behaviour, a click on the page.
- Any other value is an error when `blyte.toml` is read, naming the value.
- Each publish is written to the surface's log as a notice with the branch and the pull request
  URL, so an unasked publish is visible afterwards.
- A terminal surface still has no `publish` tool: it builds no `Surface` (no requests state), so
  that needs its own design. Not part of this change.

What this gives up: with `auto`, text the model read can lead to a branch being pushed and a pull
request opened with no person in between. The branch is one blyte combined from this surface's own
verified runs, and nothing reaches the default branch without the merge approval. `docs/security.md`
says so and names `publish = "ask"`.

## Part 2: landing (not planned yet)

A `land` tool for the surface's model:

1. reads the pull request (`gh pr view`): open, not a draft, not conflicting, head equal to the
   published branch's commit;
2. runs the repository's landing check: a new `[surface] land_check` command in `blyte.toml`
   (for this repository `scripts/land.sh --check <pr>`); with none set, the tool refuses and names
   the setting;
3. asks the user: a card on the page, a prompt in a terminal, through a new `Approver::ask_land`.
   A turn started by another surface cannot land;
4. merges with `gh pr merge --squash --match-head-commit <sha>`;
5. fetches the default branch and fast-forwards the main checkout when it is on that branch and
   clean; otherwise it says what it left.

Open for the plan: whether the check runs in the main checkout or a worktree of the pull request's
head, and what is removed after the merge.
