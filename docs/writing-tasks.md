# Writing tasks for blyte

In the plan → delegate → review pattern, the task is where the frontier model's judgement is
handed over. blyte sees only the task and the repository, nothing from the conversation that
produced it. A good task is one a careful engineer could finish without asking a single question.
Everything on this page comes from running the pattern on real work.

## A template

```markdown
## Task: <one line: what changes>

<Why, in one or two sentences. A line of context prevents literal-minded mistakes.>

**Files:** `src/parse.rs`, `tests/parse_empty.rs` (new).

**What to build** (names are part of the contract):
- `parse("")` returns `Err(ParseError::Empty)` instead of panicking.
- `ParseError::Empty` displays as `"empty input"`.

**Tests** (add verbatim):

    #[test]
    fn empty_input_is_an_error() {
        assert_eq!(parse(""), Err(ParseError::Empty));
    }

**Constraints:** existing tests pass unmodified; no new dependencies.

**Commit message:** `fix(parse): empty input is an error, not a panic`
```

## What to put in

**The files.** List every file to create or change. It keeps blyte in scope, and it makes review
fast: anything outside the list is a question.

**The contract, exactly.** Names, signatures, error types, output formats: anything another part
of the code or a test depends on. Leave the implementation to blyte; pin down the interface.

**The tests, written out.** This is the most valuable part of a task. With the tests given
verbatim, blyte knows exactly what must pass, and the gate checks what you meant rather than what
it guessed. Say which command runs them.

**What must not change.** "Existing tests pass unmodified" is the most useful constraint there is:
without it, a model under pressure may edit a failing test instead of the code.

**The exact commit message.** "Use the message in the plan" produced the wrong message in two of
our first six dispatches; stating it verbatim fixed that.

## Check the task before you send it

- **The tests fail now.** Run them against the current code. A test that already passes proves
  nothing, and blyte will "finish" at once.
- **The tests can pass.** Make sure the intended change can satisfy them under the tools' limits.
  One of our tests fed a single 20,000-character line through output that is cut at 500
  characters a line; both harnesses spent dozens of turns on it and bent the code to pass it.
- **The gate is the check you trust.** Formatting and lint first, the full test run last; every
  step runs and every failure comes back at once. Keep it well under the run's time budget.

## Sizing

- **One task, one commit.** A task that needs several unrelated commits is several tasks.
- **Coupled tasks go together.** If task 2 cannot pass without task 1 (task 1 removes something
  task 2's files still use), send them as one task, or give task 1 the minimal call-site change.
  Never tell blyte to "stop if X breaks" when X is bound to break.
- **Independent tasks run in parallel.** Disjoint files, no ordering: dispatch them at once, each
  in its own worktree. Say in each which sibling tasks are *not* in its tree, so it does not "fix"
  their absence.

## Removals and renames

Search for the removed thing's names **as strings** (CLI values, config keys, harness or arm names
in tests), not just its type and module names. Plans that searched only for identifiers twice
missed tests that used a removed feature by name.

## Keep for yourself

Do not delegate a task that needs a design decision, has ambiguous requirements, has no check that
would catch a wrong result, or touches credentials, deployment or other repositories. Those are
the parts the frontier model is for.

## When a task comes back short

Read blyte's summary and the diff. Then:

- **trivial issue** (a typo, a missed import): fix it yourself;
- **blyte misread the task:** send a follow-up task that says exactly what to change, under a new
  id, based on its branch;
- **the task was wrong:** fix the task, not the result, and dispatch again.
