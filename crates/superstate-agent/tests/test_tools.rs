//! Tool behaviour, concentrating on the ways a tool can quietly do the wrong thing.

use serde_json::json;
use superstate_agent::tools::{distil, execute, specs, OUTPUT_LINE_BUDGET};

fn ws() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

#[test]
fn no_tool_offers_a_background_mode() {
    // 47.6% of all input tokens in EXP-081 went to turns that only waited for a background
    // task (FINDINGS 13.29). The fix is that there is nothing to background and nothing to
    // poll -- asserted here so it cannot be reintroduced for convenience.
    // Checked against tool NAMES and PARAMETER names, not raw text: the bash description
    // legitimately contains the word "poll" while telling the model there is nothing to poll,
    // and a substring match on prose flagged it.
    let specs = specs();
    for t in &specs {
        assert!(!t.name.starts_with("job_"), "job control tool present: {}", t.name);
        let props = t.parameters.get("properties").and_then(|p| p.as_object());
        for key in props.into_iter().flatten().map(|(k, _)| k.as_str()) {
            assert!(
                !matches!(key, "run_in_background" | "background" | "async" | "detach"),
                "tool `{}` offers a background parameter `{key}`", t.name
            );
        }
    }
    assert!(specs.iter().any(|t| t.name == "bash"), "bash must exist to be constrained");
}

#[test]
fn bash_blocks_until_the_command_finishes() {
    let d = ws();
    let r = execute(d.path(), "bash", &json!({"command": "sleep 1; echo done"}));
    assert!(r.content.contains("done"), "must return the output, not a job handle: {r:?}");
    assert!(!r.is_error);
}

#[test]
fn bash_always_states_the_exit_code() {
    // A non-zero exit with quiet output otherwise reads as success.
    let d = ws();
    let r = execute(d.path(), "bash", &json!({"command": "exit 3"}));
    assert!(r.content.starts_with("exit 3"), "got {:?}", r.content);
    assert!(r.is_error, "a failing command must be marked an error");
}

#[test]
fn a_path_escaping_the_workspace_is_refused() {
    // The previous harness relied on a prompt rule: "confine all inspections to the workspace".
    // Invariant IV exists because agents ignore rules like that under pressure.
    let d = ws();
    for bad in ["../../../etc/passwd", "/etc/passwd"] {
        let r = execute(d.path(), "read", &json!({"path": bad}));
        assert!(r.is_error && r.content.contains("escapes the workspace"),
                "{bad} must be refused, got {r:?}");
    }
}

#[test]
fn a_new_file_outside_the_workspace_is_refused_even_though_it_does_not_exist_yet() {
    // Canonicalising only existing paths would wave through exactly the dangerous case.
    let d = ws();
    let r = execute(d.path(), "write", &json!({"path": "../escaped.txt", "content": "x"}));
    assert!(r.is_error, "writing outside the workspace must fail: {r:?}");
}

#[test]
fn edit_refuses_an_ambiguous_match_instead_of_guessing() {
    // Replacing the first of several matches silently edits the wrong place, and the agent
    // has no way to tell that it did.
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "let x = 1;\nlet x = 1;\n").unwrap();
    let r = execute(d.path(), "edit", &json!({"path": "a.rs", "old": "let x = 1;", "new": "let x = 2;"}));
    assert!(r.is_error && r.content.contains("appears 2 times"), "got {r:?}");
    assert_eq!(std::fs::read_to_string(d.path().join("a.rs")).unwrap(), "let x = 1;\nlet x = 1;\n",
               "a refused edit must not modify the file");
}

#[test]
fn edit_says_to_read_first_when_the_target_is_absent() {
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "fn main() {}\n").unwrap();
    let r = execute(d.path(), "edit", &json!({"path": "a.rs", "old": "nope", "new": "x"}));
    assert!(r.is_error && r.content.contains("Read it first"), "got {r:?}");
}

#[test]
fn read_numbers_lines_so_edits_can_be_precise() {
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
    let r = execute(d.path(), "read", &json!({"path": "a.rs", "start_line": 2, "end_line": 3}));
    assert!(r.content.contains("2\ttwo"), "got {:?}", r.content);
    assert!(!r.content.contains("one"), "range must be respected");
}

#[test]
fn distil_keeps_the_tail_because_test_runners_summarise_at_the_end() {
    // Head-only truncation would hide the answer and keep the noise -- the opposite of useful
    // for the 25,105-line `go test` output this exists for (FINDINGS 13.26).
    let text: String = (1..=5000).map(|i| format!("line {i}\n")).collect();
    let out = distil(&text, OUTPUT_LINE_BUDGET);
    assert!(out.contains("line 1"), "head kept");
    assert!(out.contains("line 5000"), "TAIL kept -- the summary lives here");
    assert!(out.contains("elided by superstate"), "elision must be stated, not silent");
    assert!(out.lines().count() < OUTPUT_LINE_BUDGET + 10);
}

#[test]
fn distil_leaves_short_output_untouched() {
    assert_eq!(distil("a\nb\nc", OUTPUT_LINE_BUDGET), "a\nb\nc");
}
