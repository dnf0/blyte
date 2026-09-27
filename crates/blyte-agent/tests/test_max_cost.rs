//! A spending cap stops the run before the next model call once list-price spend reaches it.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response, ToolCall, Usage};
use serde_json::json;

/// Always asks for one more tool call; each call reports 100K fresh input and 10K output tokens,
/// which at Gemini 3.8 Flash list price is $0.075 + $0.0375 = $0.1125 per call.
struct Spender {
    calls: Mutex<u32>,
}

impl Provider for Spender {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        Ok(Response {
            tool_calls: vec![ToolCall {
                id: format!("c{n}"),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
                signature: None,
            }],
            usage: Usage {
                input: 100_000,
                cached: 0,
                output: 10_000,
                thinking: 0,
            },
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "gemini/gemini-3.8-flash".into()
    }
}

fn run(cap: Option<f64>) -> (Outcome, blyte_agent::run::RunReport, u32) {
    let d = tempfile::tempdir().unwrap();
    let p = Spender {
        calls: Mutex::new(0),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 20,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    let mut st = State::new("g", "true");
    st.orchestrator.max_cost_usd = cap;
    let (outcome, report) = r.run(&mut st);
    let calls = *p.calls.lock().unwrap();
    (outcome, report, calls)
}

#[test]
fn the_run_stops_once_spend_reaches_the_cap() {
    let (outcome, report, calls) = run(Some(0.30));
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(report.cost_capped);
    // $0.1125 per call: after 3 calls spend is $0.3375 >= $0.30, so a 4th call never starts.
    assert_eq!(
        calls, 3,
        "stopped before the call that would begin over the cap"
    );
}

#[test]
fn without_a_cap_the_turn_budget_is_what_stops_it() {
    let (outcome, report, calls) = run(None);
    assert_eq!(outcome, Outcome::BudgetExhausted);
    assert!(!report.cost_capped);
    assert_eq!(calls, 20);
}
