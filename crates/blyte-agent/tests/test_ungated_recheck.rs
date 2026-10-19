//! Ungated, the model re-runs the searches that found the problems before it stops: on
//! sanitize-git-repo it checked only the strings it had replaced, and two secrets it had found
//! in a third file were still there (2026-09-27).

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    seen: Mutex<Vec<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response::default())
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

fn system_prompt(gate: &str) -> String {
    let d = tempfile::tempdir().unwrap();
    let p = Stopper {
        seen: Mutex::new(Vec::new()),
    };
    let r = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 1,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
    };
    r.run(&mut State::new("g", gate));
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

const RECHECK: &str =
    "run those same searches again before you stop and confirm they find nothing left to fix";

#[test]
fn ungated_the_model_is_told_to_repeat_its_searches() {
    assert!(system_prompt("").contains(RECHECK));
}

#[test]
fn gated_the_prompt_is_unchanged() {
    assert!(!system_prompt("cargo test").contains(RECHECK));
}
