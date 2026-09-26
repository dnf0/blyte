# Ollama: Thinking Off by Default — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** blyte's Ollama provider sends `"think": false` on every chat request unless the model name carries a `+think` suffix (`qwen3:14b+think`), which sends `"think": true` and is stripped from the model name on the wire.

**Why (local run, 2026-09-26):** blyte on local `qwen3:14b` fixed a two-bug task in 4 turns but spent 435 s of model time, because Qwen3 thinks by default and blyte never turned it off. The same question took 31.3 s with thinking and 1.8 s without. Sending `"think": false` is accepted by models without thinking (checked on `qwen2.5-coder:14b`), while `"think": true` is rejected by them ("does not support thinking"), so off is the safe default and on must be an explicit opt-in. `+` cannot appear in an Ollama model tag, so the suffix cannot collide with a real tag. (Gemini's thinking level is chosen the same way, by a model-name suffix.)

**Tech Stack:** Rust 2021.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- Change only the Ollama provider; the registry already routes `+think` names to Ollama because they contain `:`.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Thinking off by default for Ollama

**Files:** `crates/blyte-provider/src/ollama.rs`; new test `crates/blyte-provider/tests/test_ollama_think.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-provider/tests/test_ollama_think.rs`:

```rust
//! Thinking is off unless asked for: local Qwen3 spent 435 s thinking through a 4-turn task
//! (31.3 s against 1.8 s per answer). `"think": false` is safe on every model; `true` is
//! rejected by models without thinking, so it is an explicit `+think` opt-in.

use blyte_provider::ollama::Ollama;
use blyte_provider::{Message, Request};

fn req() -> Request {
    Request {
        system: String::new(),
        messages: vec![Message::User("hi".into())],
        tools: vec![],
        max_output_tokens: None,
    }
}

fn local() -> Ollama {
    Ollama::from_env_or_default().with_api_root("http://localhost:11434")
}

#[test]
fn thinking_is_off_by_default() {
    let b = local().with_model("qwen3:14b").body(&req());
    assert_eq!(b["think"], false);
    assert_eq!(b["model"], "qwen3:14b");
}

#[test]
fn a_think_suffix_turns_it_on_and_is_stripped_from_the_wire() {
    let b = local().with_model("qwen3:14b+think").body(&req());
    assert_eq!(b["think"], true);
    assert_eq!(b["model"], "qwen3:14b");
}

#[test]
fn the_suffix_works_with_the_provider_prefix() {
    let b = local().with_model("ollama/qwen3:14b+think").body(&req());
    assert_eq!(b["think"], true);
    assert_eq!(b["model"], "qwen3:14b");
}
```

  The test needs a way to build an `Ollama` without environment variables. Add a public constructor `Ollama::from_env_or_default()` that never fails: the same as `from_env()`, but with no key requirement (no key, `DEFAULT_API_ROOT`, `DEFAULT_MODEL`) when neither `OLLAMA_API_KEY` nor `OLLAMA_HOST` is set.

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte-provider --test test_ollama_think`. Expected: compile error (`from_env_or_default` missing), then failures (`think` absent).

- [ ] **Step 3: Implement.** In `crates/blyte-provider/src/ollama.rs`:
  1. Add a `think: bool` field to `Ollama`.
  2. Wherever the model name is set (`from_env`, `with_model`, and the new constructor), strip a trailing `+think` after stripping the `ollama/` prefix, and set `think` to whether it was there.
  3. In `body()`, always set `"think": self.think` in the request body.
  4. Update the module doc comment with one line: thinking is off unless the model name ends in `+think`.

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte-provider --test test_ollama_think --test test_ollama_wire --test test_registry`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(provider): Ollama thinking is off unless the model name ends in +think"
```
