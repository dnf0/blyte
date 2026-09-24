# Fold Notes Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** The first time a fold appears, the worker records, in a fixed JSON shape, what it still needs from the turns being folded. From then on, that fold's digest carries the note alongside the harness's one-line-per-call summary.

**Why:** SKILL.state (arXiv 2608.26263) gets its results from *model-authored* structured state: facts, dead ends, relevant files. Superstate has the mechanism (the `note` tool, `State::apply_agent_patch`), but:
- the worker called `note` 0 times in all four build-cython runs on 2026-09-24;
- `folded`/`compact` never render state anyway.

So a fold keeps only a mechanical summary, and conclusions reached in the folded turns are lost. The harness now asks for the note at exactly the moment a fold happens.

**Design:**
- **Prefix-sharing request.** The note request is the previous turn's request, plus the messages since, plus one instruction. It uses the same system prompt and tools, so all but the newest turn is read from cache.
- **Stored in state.** The rendered note goes in `state.runtime.fold_notes`, keyed by the fold's end index. `Folded`/`Compact` stay pure and render it identically every turn.
- **Cache behaviour kept.** The prompt still changes only when a fold happens (compact v2's property).
- **Any failure accepts.** A provider error, tool calls in the reply or unparseable JSON store an empty note, so the digest is the harness's alone. A fold is never asked twice.
- **Off by default.** It is switched on by `Orchestrator.fold_notes` (CLI `--fold-notes`), to be measured before it becomes a default.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `feat/fold-notes` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass. The **only** permitted edits to existing test files are the mechanical literal additions in Step 7.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Fold notes

**Files:**
- Modify: `crates/superstate-agent/src/state.rs`, `crates/superstate-agent/src/context.rs`, `crates/superstate-agent/src/run.rs`
- Modify: `crates/superstate-cli/src/cli.rs`, `crates/superstate-cli/src/task_runner.rs`, `crates/superstate-cli/src/report.rs`
- Modify (literals only, Step 7): `crates/superstate-agent/tests/test_working_set.rs`, `crates/superstate-cli/tests/test_task_runner.rs`, `crates/superstate-cli/tests/test_run_report.rs`, `crates/superstate-cli/tests/test_tdd_phases.rs`
- Test: `crates/superstate-agent/tests/test_fold_notes.rs` (create)

- [x] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_fold_notes.rs`:

```rust
//! Fold notes: the first time a fold appears, the worker records what to keep from the turns
//! being folded, and that fold's digest carries the note from then on.

use std::sync::Mutex;

use serde_json::json;
use superstate_agent::context::{Compact, ContextStrategy, Folded};
use superstate_agent::fs::LocalFs;
use superstate_agent::run::{
    render_fold_note, Budget, Outcome, RunReport, Runner, FOLD_NOTE_MARKER,
};
use superstate_agent::shell::LocalShell;
use superstate_agent::state::State;
use superstate_provider::{Message, Provider, ProviderError, Request, Response, ToolCall};

const NOTE: &str = r#"{"findings": ["the bug is in parse() at src/lib.rs:42"], "ruled_out": ["upgrading serde"], "files": {"src/lib.rs": "the parser"}, "next": ["fix the off-by-one"]}"#;

/// Answers fold-note requests with `note_reply`, everything else from `script` in order.
struct Scripted {
    script: Mutex<Vec<Response>>,
    note_reply: String,
    seen: Mutex<Vec<Request>>,
}

fn is_note_request(req: &Request) -> bool {
    matches!(req.messages.last(), Some(Message::User(t)) if t.starts_with(FOLD_NOTE_MARKER))
}

impl Provider for Scripted {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        if is_note_request(req) {
            return Ok(Response {
                text: self.note_reply.clone(),
                ..Default::default()
            });
        }
        let mut s = self.script.lock().unwrap();
        Ok(if s.is_empty() {
            Response::default()
        } else {
            s.remove(0)
        })
    }
    fn model_id(&self) -> String {
        "scripted".into()
    }
}

/// Four turns, each reading a different 40-line file, then done. Folds under a 300-token budget.
fn run_with(note_reply: &str, fold_notes: bool) -> (Outcome, RunReport, Vec<Request>) {
    let d = tempfile::tempdir().unwrap();
    let body = format!("{}\n", "x".repeat(50)).repeat(40);
    let mut script = Vec::new();
    for i in 0..4 {
        std::fs::write(d.path().join(format!("f{i}.txt")), &body).unwrap();
        script.push(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "read".into(),
                arguments: json!({"path": format!("f{i}.txt")}),
                signature: None,
            }],
            ..Default::default()
        });
    }
    script.push(Response {
        text: "done".into(),
        ..Default::default()
    });
    let p = Scripted {
        script: Mutex::new(script),
        note_reply: note_reply.into(),
        seen: Mutex::new(Vec::new()),
    };
    let folded = Folded { budget_tokens: 300 };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &folded,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 10,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: None,
    };
    let mut st = State::new("read the files", "true");
    st.orchestrator.fold_notes = fold_notes;
    let (outcome, report) = runner.run(&mut st);
    let seen = p.seen.lock().unwrap().clone();
    (outcome, report, seen)
}

fn has_notes(req: &Request, needle: &str) -> bool {
    req.messages.iter().any(|m| {
        matches!(m, Message::User(t) if t.starts_with("## Earlier turns, folded")
            && t.contains("### Your notes from these turns")
            && t.contains(needle))
    })
}

#[test]
fn a_new_fold_gets_a_note_that_its_digest_then_carries() {
    let (outcome, report, seen) = run_with(NOTE, true);
    assert_eq!(outcome, Outcome::Verified);
    let notes: Vec<usize> = (0..seen.len()).filter(|&k| is_note_request(&seen[k])).collect();
    assert!(!notes.is_empty(), "expected at least one fold-note request");
    assert_eq!(report.fold_notes as usize, notes.len());
    assert_eq!(report.fold_note_failures, 0);

    for &k in &notes {
        // Shares the previous main request's prefix, system prompt and tools, so it caches.
        let prev = (0..k)
            .rev()
            .find(|&j| !is_note_request(&seen[j]))
            .expect("a main request before the note request");
        let (note, main) = (&seen[k], &seen[prev]);
        assert!(note.messages.len() > main.messages.len());
        assert_eq!(&note.messages[..main.messages.len()], &main.messages[..]);
        assert_eq!(note.system, main.system);
        assert_eq!(note.tools.len(), main.tools.len());
    }

    let after = (notes[0] + 1..seen.len())
        .find(|&j| !is_note_request(&seen[j]))
        .expect("a main request after the first note");
    assert!(
        has_notes(&seen[after], "the bug is in parse() at src/lib.rs:42"),
        "the digest must carry the note"
    );
    assert!(has_notes(&seen[after], "`src/lib.rs`: the parser"));
}

#[test]
fn an_unusable_note_leaves_the_harness_digest_and_is_not_retried() {
    let (outcome, report, seen) = run_with("I'd rather keep working.", true);
    assert_eq!(outcome, Outcome::Verified);
    let asked = seen.iter().filter(|r| is_note_request(r)).count();
    assert!(asked > 0);
    assert_eq!(report.fold_notes, 0);
    assert_eq!(report.fold_note_failures as usize, asked, "one request per fold, no retries");
    assert!(!seen.iter().any(|r| has_notes(r, "")));
}

#[test]
fn off_by_default() {
    let (_, report, seen) = run_with(NOTE, false);
    assert!(!seen.iter().any(is_note_request));
    assert_eq!(report.fold_notes + report.fold_note_failures, 0);
    assert!(!State::new("g", "v").orchestrator.fold_notes);
}

fn history(turns: usize) -> Vec<Message> {
    let mut h = vec![Message::User("Begin. fix it".into())];
    for t in 0..turns {
        h.push(Message::ToolCall {
            id: format!("c{t}"),
            name: "bash".into(),
            arguments: json!({"command": format!("cat f{t}.txt")}),
            signature: None,
        });
        h.push(Message::ToolResult {
            id: format!("c{t}"),
            name: "bash".into(),
            content: format!("{}\n", "y".repeat(50)).repeat(40),
            is_error: false,
        });
    }
    h
}

#[test]
fn a_stored_note_appears_only_in_its_own_folds_digest() {
    let f = Folded { budget_tokens: 300 };
    let h = history(6);
    let folds = f.folds(&h);
    assert!(folds.len() >= 2, "{folds:?}");
    assert_eq!(Compact { budget_tokens: 300, min_dedup_chars: 200 }.folds(&h), folds);

    let mut st = State::new("fix it", "");
    let plain = f.build(&st, &h);
    st.runtime
        .fold_notes
        .insert(folds[0].1, "Established:\n- NOTE_ONE\n".into());
    let noted = f.build(&st, &h);
    assert_eq!(plain.len(), noted.len());
    let changed: Vec<usize> = (0..plain.len()).filter(|&i| plain[i] != noted[i]).collect();
    assert_eq!(changed, vec![1], "only the first fold's digest changes");
    let Message::User(t) = &noted[1] else {
        panic!("expected a digest");
    };
    assert!(t.contains("### Your notes from these turns\nEstablished:\n- NOTE_ONE"), "{t}");

    // An empty note (a failed request) renders as the harness digest alone.
    st.runtime.fold_notes.insert(folds[0].1, String::new());
    assert_eq!(f.build(&st, &h), plain);
}

#[test]
fn notes_render_deterministically_from_json() {
    let r = render_fold_note(&format!("Here you go:\n```json\n{NOTE}\n```")).unwrap();
    assert_eq!(
        r,
        "Established:\n- the bug is in parse() at src/lib.rs:42\n\
         Ruled out:\n- upgrading serde\n\
         Files:\n- `src/lib.rs`: the parser\n\
         Still to do:\n- fix the off-by-one\n"
    );
    assert_eq!(render_fold_note("no json here"), None);
    assert_eq!(render_fold_note("{}"), None);
    assert_eq!(render_fold_note(r#"{"findings": []}"#), None);
}
```

- [x] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_fold_notes`. Expected: compile errors, because `render_fold_note`, `FOLD_NOTE_MARKER`, `folds`, `fold_notes` and `fold_note_failures` do not exist.

- [x] **Step 3: State.** In `crates/superstate-agent/src/state.rs`:
  1. In `pub struct Orchestrator`, after the `review` field, add:

```rust
    /// When a fold first appears, ask the worker for a structured note on the turns being folded
    /// and carry it in that fold's digest (`run::FOLD_NOTE_MARKER`). Set by the caller.
    #[serde(default)]
    pub fold_notes: bool,
```

  2. In `State::new`, add `fold_notes: false,` after `review: false,`.
  3. In `pub struct Runtime`, after the `working_set` field, add:

```rust
    /// Rendered fold notes, keyed by the end of the fold they describe (an index into
    /// `history[1..]`, stable because folds are prefix-stable). Empty means the request failed
    /// and the fold keeps the harness digest alone.
    #[serde(default)]
    pub fold_notes: std::collections::BTreeMap<usize, String>,
```

- [x] **Step 4: Context.** In `crates/superstate-agent/src/context.rs`:
  1. In `pub trait ContextStrategy`, after `fn build(...)`, add:

```rust
    /// The folds this strategy makes over `history`, as `(a, b)` ranges of `history[1..]`.
    /// Strategies that never fold return none.
    fn folds(&self, _history: &[Message]) -> Vec<(usize, usize)> {
        Vec::new()
    }
```

  2. Change `fn digest(folded: &[Message], first: usize, last: usize) -> Message {` to `fn digest(folded: &[Message], first: usize, last: usize, note: Option<&str>) -> Message {`. Directly before its final `Message::User(text)`, add:

```rust
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        text.push_str("\n### Your notes from these turns\n");
        text.push_str(note);
    }
```

  3. In `impl ContextStrategy for Folded`: rename the `_state` parameter of `build` to `state`. Change the digest call to `out.push(digest(&rest[a..b], a + 2, b + 1, state.runtime.fold_notes.get(&b).map(String::as_str)));`. Add the method:

```rust
    fn folds(&self, history: &[Message]) -> Vec<(usize, usize)> {
        history
            .split_first()
            .map(|(_, rest)| self.plan(rest).folds)
            .unwrap_or_default()
    }
```

  4. In `impl ContextStrategy for Compact`, add:

```rust
    fn folds(&self, history: &[Message]) -> Vec<(usize, usize)> {
        Folded {
            budget_tokens: self.budget_tokens,
        }
        .folds(history)
    }
```

- [x] **Step 5: The runner.** In `crates/superstate-agent/src/run.rs`:
  1. Change the import `use superstate_provider::{Message, Provider, ProviderError, Request, ToolCall, Usage};` to also import `ToolSpec`.
  2. In `pub struct RunReport`, after `review_rejections`, add:

```rust
    /// Fold notes recorded, and fold-note requests that failed (the fold kept the harness digest).
    pub fold_notes: u32,
    pub fold_note_failures: u32,
```

  3. Directly after the `retry_delay` function, add:

```rust
/// First line of a fold-note request. Test providers recognise note requests by it.
pub const FOLD_NOTE_MARKER: &str = "## Fold notes requested by the harness";

/// Render a fold-note reply: a JSON object with any of `findings`, `ruled_out` and `next`
/// (string lists) and `files` (path -> role). `None` when there is no such object or it says
/// nothing, so the fold keeps the harness digest alone. Deterministic, so the note enters the
/// prompt identically every turn.
pub fn render_fold_note(text: &str) -> Option<String> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if end < start {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
    let obj = v.as_object()?;
    let list = |key: &str| -> Vec<String> {
        obj.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .take(12)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = String::new();
    let mut section = |title: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("{title}:\n"));
            for it in items {
                out.push_str(&format!("- {it}\n"));
            }
        }
    };
    section("Established", list("findings"));
    section("Ruled out", list("ruled_out"));
    let files: Vec<String> = obj
        .get("files")
        .and_then(|x| x.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(p, r)| r.as_str().map(|r| format!("`{p}`: {}", r.trim())))
                .take(12)
                .collect()
        })
        .unwrap_or_default();
    section("Files", files);
    section("Still to do", list("next"));
    (!out.is_empty()).then_some(out)
}
```

  4. In the second `impl<'a> Runner<'a>` block, directly after `fn review(...) { ... }`, add:

```rust
    /// Ask the worker what to keep from messages `first..=last`, which are about to be folded.
    /// `prefix` is the previous request's messages plus everything since, and the system prompt
    /// and tools are the loop's own, so all but the newest turn is read from cache. Any failure
    /// yields `None`: the fold keeps the harness digest.
    fn fold_note(
        &self,
        system: &str,
        specs: &[ToolSpec],
        prefix: Vec<Message>,
        (first, last): (usize, usize),
        report: &mut RunReport,
    ) -> Option<String> {
        let mut messages = prefix;
        messages.push(Message::User(format!(
            "{FOLD_NOTE_MARKER}\n\
             Messages {first}-{last} are about to be folded: from the next turn you will see only \
             a one-line summary of each call there, not its output. Record now what you still \
             need from them. Reply with only a JSON object and no tool calls:\n\
             {{\"findings\": [\"facts established, with paths, line numbers and identifiers \
             verbatim\"], \"ruled_out\": [\"approaches tried and why they failed\"], \
             \"files\": {{\"path\": \"what it is for\"}}, \"next\": [\"what remains\"]}}\n\
             Keep it under 300 words."
        )));
        let req = Request {
            system: system.to_string(),
            messages,
            tools: specs.to_vec(),
            max_output_tokens: None,
        };
        let resp = self.provider.complete(&req).ok()?;
        report.usage.input += resp.usage.input;
        report.usage.cached += resp.usage.cached;
        report.usage.output += resp.usage.output;
        report.usage.thinking += resp.usage.thinking;
        if !resp.tool_calls.is_empty() {
            return None;
        }
        render_fold_note(&resp.text)
    }
```

  5. In `fn run_until`, directly after `let mut review_rounds = 0;`, add:

```rust
        // Fold notes are keyed by position in this history, which starts afresh here (each phase
        // of a multi-phase run has its own).
        state.runtime.fold_notes.clear();
        let mut last_sent: Option<(Vec<Message>, usize)> = None;
```

  6. In the turn loop, directly before `let req = Request {` (the one whose `messages` is `self.context.build(state, &history)`), add:

```rust
            // The first time a fold appears, the worker records what to keep from it. The prompt
            // changes then anyway, so the note costs no extra cache miss.
            if state.orchestrator.fold_notes {
                if let Some((sent, len)) = &last_sent {
                    for (a, b) in self.context.folds(&history) {
                        if state.runtime.fold_notes.contains_key(&b) {
                            continue;
                        }
                        let mut prefix = sent.clone();
                        prefix.extend_from_slice(&history[*len..]);
                        let note =
                            self.fold_note(&system, &specs, prefix, (a + 2, b + 1), &mut report);
                        if note.is_some() {
                            report.fold_notes += 1;
                        } else {
                            report.fold_note_failures += 1;
                        }
                        state.runtime.fold_notes.insert(b, note.unwrap_or_default());
                    }
                }
            }
```

  7. Directly after that `let req = Request { ... };` statement, add:

```rust
            last_sent = Some((req.messages.clone(), history.len()));
```

- [x] **Step 6: CLI.**
  1. `crates/superstate-cli/src/cli.rs`: after the `review` field, add:

```rust
    /// When a fold first appears, ask the worker for a structured note on the folded turns and
    /// carry it in the digest (folded and compact contexts)
    #[arg(long)]
    pub fold_notes: bool,
```

  2. `crates/superstate-cli/src/task_runner.rs`:
     - In `pub struct TaskConfig`, after `review`, add `/// Fold notes (`--fold-notes`).` and `pub fold_notes: bool,`.
     - In the `From<&Cli>` impl, add `fold_notes: cli.fold_notes,`.
     - After `state.orchestrator.review = config.review;`, add `state.orchestrator.fold_notes = config.fold_notes;`.
  3. `crates/superstate-cli/src/report.rs`:
     - In `RunRecord`, after `review_rejections`, add `pub fold_notes: u32,` and `pub fold_note_failures: u32,`.
     - In `base`, add `fold_notes: 0,` and `fold_note_failures: 0,`.
     - In `from_run`, add `fold_notes: report.fold_notes,` and `fold_note_failures: report.fold_note_failures,`.

- [x] **Step 7: Literal additions in existing tests (mechanical, nothing else).**
  - `crates/superstate-agent/tests/test_working_set.rs`: in the `Orchestrator { ... }` literal, add `fold_notes: false,` after `review: false,`.
  - `crates/superstate-cli/tests/test_task_runner.rs` (4 literals), `test_run_report.rs` (1), `test_tdd_phases.rs` (1): in each `TaskConfig { ... }` literal, add `fold_notes: false,` after `review: false,`.
  - `crates/superstate-cli/tests/test_run_report.rs`: in the `RunReport { ... }` literal, add `fold_notes: 0,` and `fold_note_failures: 0,` after `review_rejections: 0,`.

- [x] **Step 8: Run the new tests.** Run: `cargo test -p superstate-agent --test test_fold_notes --test test_folded --test test_compact --test test_parallel_calls`. Expected: all pass (5 new).

- [x] **Step 9: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [x] **Step 10: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(agent): fold notes — the worker records what to keep when a fold first appears"
```
