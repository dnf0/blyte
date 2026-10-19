use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::research_check::{identifiers, unseen_names};
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

fn seen_from(text: &str) -> HashSet<String> {
    identifiers(text).map(str::to_owned).collect()
}

#[test]
fn unseen_code_names_in_prose_are_flagged() {
    let seen = seen_from("import stale_cache");
    let report = "The limit `MAX_CONCURRENT_SPATIAL_ACTIVITIES` is 4, \
        `stale_cache.purge_for_project(...)` clears it, and \
        `spectra.overwritten_asset_attributes` stores it.\n";
    let problems = unseen_names(report, &seen);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(
        problems[0].contains("`MAX_CONCURRENT_SPATIAL_ACTIVITIES`"),
        "{}",
        problems[0]
    );
    assert!(
        problems[1].contains("`purge_for_project`"),
        "{}",
        problems[1]
    );
    assert!(
        problems[2].contains("`overwritten_asset_attributes`"),
        "{}",
        problems[2]
    );
    assert!(problems[0].starts_with("report.md:1: `MAX_CONCURRENT_SPATIAL_ACTIVITIES` does not appear in anything this run read (the task, files or command output); name only what you have seen"));
}

#[test]
fn fenced_blocks_and_seen_names_pass() {
    let empty = HashSet::new();
    let fenced = "In code:\n```python\nMAX_CONCURRENT_SPATIAL_ACTIVITIES = 4\nstale_cache.purge_for_project()\nspectra.overwritten_asset_attributes\n```\n";
    assert!(unseen_names(fenced, &empty).is_empty());

    let seen = seen_from(
        "MAX_CONCURRENT_SPATIAL_ACTIVITIES stale_cache purge_for_project overwritten_asset_attributes camelCase PascalCase",
    );
    let prose = "Uses `MAX_CONCURRENT_SPATIAL_ACTIVITIES`, `stale_cache.purge_for_project(...)`, \
        `spectra.overwritten_asset_attributes`, `camelCase`, and `PascalCase`.\n";
    assert!(unseen_names(prose, &seen).is_empty());
}

#[test]
fn plain_words_paths_numbers_and_flags_are_ignored() {
    let empty = HashSet::new();
    let report = "See `report.md`, `spectra`, `Gate`, `report`, `42`, `3.14`, and `--flags` or `--allow-read`.\n";
    assert!(unseen_names(report, &empty).is_empty());
}

#[test]
fn distinct_names_are_reported_once_at_first_line() {
    let empty = HashSet::new();
    let report = "Line one `made_up_name` and `camelCase`.\nLine two `made_up_name` again.\n";
    let problems = unseen_names(report, &empty);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].starts_with("report.md:1: `made_up_name`"));
    assert!(problems[1].starts_with("report.md:1: `camelCase`"));
}

struct ReadThenWriteReport {
    turn: Mutex<usize>,
}

impl Provider for ReadThenWriteReport {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        Ok(match *t {
            1 => Response {
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "src.py"}),
                    signature: None,
                }],
                ..Default::default()
            },
            2 => Response {
                tool_calls: vec![ToolCall {
                    id: "c2".into(),
                    name: "write".into(),
                    arguments: json!({
                        "path": "report.md",
                        "content": "Found `real_name` and `made_up_name` in the module.\n",
                    }),
                    signature: None,
                }],
                ..Default::default()
            },
            _ => Response {
                text: "done".into(),
                ..Default::default()
            },
        })
    }

    fn model_id(&self) -> String {
        "scripted".into()
    }
}

#[test]
fn runner_flags_unseen_name_and_allows_name_from_tool_result() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("src.py"), "def real_name():\n    pass\n").unwrap();

    let provider = ReadThenWriteReport {
        turn: Mutex::new(0),
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 2,
            max_gate_attempts: 1,
            max_duration: Some(Duration::from_secs(30)),
        },
        rollback: false,
        on_event: None,
        research: true,
    };

    let mut st = State::new("research task", "test -s report.md");
    let (outcome, report) = runner.run(&mut st);

    assert_eq!(outcome, Outcome::BudgetExhausted);
    let problems = report
        .quote_problems
        .expect("quote_problems should be populated");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("`made_up_name`"), "{}", problems[0]);
}
