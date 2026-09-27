# `grep` Shows the Match Inside a Long Line — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** when a line that `grep` matches is longer than the line limit (500 characters), the
result shows an excerpt **centred on the match**, not the line's first 500 characters. A clipped
line elsewhere (command output, `read`) says that `grep` can show text around a match.

**Why (2026-09-27):** on FrontierHarness's `sanitize-git-repo`, blyte found a secret in a
minified JSON file whose one line is 21,367 characters long. Every tool cut the line from the
start (`grep` and command output at 500 characters, `read` at 2,000), so the match itself was never
visible; blyte fixed the other two files and missed this one. The task failed.

**Tech Stack:** Rust, `regex` (add `regex = "1"` to `blyte-agent`; it is already in the tree
through ripgrep's crates).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**.
- Lines at or under the limit are unchanged; the 60,000-character result cap is unchanged.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Match-centred excerpts, and a clip note that points to them

**Files:** `crates/blyte-agent/Cargo.toml`, `crates/blyte-agent/src/tools.rs`,
`crates/blyte-agent/tests/test_grep_long_lines.rs` (new).

**What to build:**

- `pub fn excerpt_around(text: &str, start: usize, end: usize, max: usize) -> String` in `tools.rs`,
  where `start..end` is the byte range of the match in `text` (on char boundaries). When
  `text` has at most `max` characters, return it unchanged. Otherwise keep the whole match
  (cut the match itself to `max` characters if it alone is longer) and split the rest of the
  `max` budget evenly before and after it, shifting any unused side to the other; prefix
  `…[<n> chars] ` when characters were dropped before, and suffix ` …[<n> chars]` when dropped
  after, `<n>` being the count dropped. Never split a UTF-8 character.
- The `grep` tool: for every result line longer than `MAX_LINE_CHARS`, keep its location prefix
  unchanged (`<n>:` for a single file, `<path>:<n>:` otherwise: everything up to and including
  the first `:<digits>:`, or the leading `<digits>:`), find the first match of the pattern in the
  rest with `regex` (if the pattern does not compile with `regex`, or does not match, leave the
  line as it is), and replace the rest with `excerpt_around(rest, s, e, MAX_LINE_CHARS)`. Do this
  before the result is bounded, so the generic clipping never sees these lines as over-long.
- `bound_output_with`: the clip note becomes
  `…[<n> more chars clipped by blyte; the grep tool shows the text around a match]`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-agent/tests/test_grep_long_lines.rs`:

```rust
//! `grep` shows the match inside a long line: on sanitize-git-repo a secret sat at character
//! ~15,000 of a 21,367-character minified JSON line, every tool cut the line from the start,
//! and blyte never saw it (2026-09-27).

use blyte_agent::tools::{bound_output, excerpt_around, MAX_LINE_CHARS};

#[test]
fn a_short_text_is_unchanged() {
    assert_eq!(excerpt_around("abc TOKEN def", 4, 9, 500), "abc TOKEN def");
}

#[test]
fn the_excerpt_is_centred_on_the_match_and_says_what_it_dropped() {
    let text = format!("{}hf_SECRET123{}", "a".repeat(15_000), "b".repeat(6_000));
    let start = 15_000;
    let end = start + "hf_SECRET123".len();
    let e = excerpt_around(&text, start, end, 500);
    assert!(e.contains("hf_SECRET123"), "{e}");
    assert!(e.starts_with("…["), "{e}");
    assert!(e.ends_with(" chars]"), "{e}");
    let kept = e.chars().filter(|c| *c == 'a' || *c == 'b').count();
    assert!(kept <= 500 && kept >= 480, "{kept}");
}

#[test]
fn a_match_near_the_start_spends_the_budget_after_it() {
    let text = format!("xTOKEN{}", "z".repeat(5_000));
    let e = excerpt_around(&text, 1, 6, 500);
    assert!(e.starts_with("xTOKEN"), "{e}");
    assert!(!e.starts_with("…["), "nothing dropped before");
}

#[test]
fn multibyte_text_is_never_split() {
    let text = format!("{}TOKEN{}", "é".repeat(3_000), "ü".repeat(3_000));
    let start = "é".len() * 3_000;
    let e = excerpt_around(&text, start, start + 5, 100);
    assert!(e.contains("TOKEN"));
}

#[test]
fn the_clip_note_points_to_grep() {
    let long = "q".repeat(MAX_LINE_CHARS + 50);
    let out = bound_output(&long);
    assert!(out.contains("clipped by blyte; the grep tool shows the text around a match"), "{out}");
}
```

Also add a test in the same file that runs the `grep` **tool** (through the same entry point the
existing grep tool tests use; find them with `grep -rn '"grep"' crates/blyte-agent/tests`) on a
temporary file whose single line is `"a" * 15_000 + "hf_SECRET123" + "b" * 6_000`, pattern
`hf_[A-Z0-9]+`, and asserts the result contains `hf_SECRET123` and starts its match line with `1:`.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_grep_long_lines`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "fix(agent): grep shows the match inside a long line instead of the line's first 500 characters"
```
