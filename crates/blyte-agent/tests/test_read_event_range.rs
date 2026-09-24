//! The event log shows a read's line range, so paging through a file is distinguishable from
//! re-reading it.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Event, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall};
use serde_json::json;

struct Scripted(Mutex<Vec<Response>>);

impl Provider for Scripted {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut s = self.0.lock().unwrap();
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

#[test]
fn a_ranged_read_is_logged_with_its_range() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let read = |id: &str, args: serde_json::Value| ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: args,
        signature: None,
    };
    let p = Scripted(Mutex::new(vec![Response {
        tool_calls: vec![
            read("r1", json!({"path": "a.txt"})),
            read(
                "r2",
                json!({"path": "a.txt", "start_line": 2, "end_line": 3}),
            ),
        ],
        ..Default::default()
    }]));
    let inputs: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let on_event = |e: Event<'_>| {
        if let Event::Tool { input, .. } = e {
            inputs.lock().unwrap().push(input);
        }
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 3,
            max_gate_attempts: 1,
            max_duration: None,
            max_cost_usd: None,
        },
        rollback: false,
        loop_breaker: false,
        on_event: Some(&on_event),
    };
    runner.run(&mut State::new("read", "true"));
    assert_eq!(*inputs.lock().unwrap(), vec!["a.txt", "a.txt (L2-L3)"]);
}
