//! Ungated, no command checks the work, so the prompt must not promise one or tell the model to
//! leave the full test run to it: three benchmark tasks failed on one or two edge-case tests after
//! the model stopped without running the suite.

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

#[test]
fn ungated_the_prompt_says_nothing_checks_the_work() {
    let sys = system_prompt("");
    assert!(sys.contains("No command checks your work"), "{sys}");
    assert!(sys.contains("tests you cannot see"), "{sys}");
    assert!(
        sys.contains("run the project's existing test suite"),
        "{sys}"
    );
    assert!(sys.contains("edge cases"), "{sys}");
}

#[test]
fn ungated_the_prompt_promises_no_command_and_no_full_run_for_you() {
    let sys = system_prompt("  \n ");
    assert!(
        !sys.contains("``"),
        "an empty command must not appear: {sys}"
    );
    assert!(!sys.contains("leave the slow full run"), "{sys}");
    assert!(!sys.contains("you do not decide the verdict"), "{sys}");
}

#[test]
fn gated_the_prompt_is_unchanged() {
    let sys = system_prompt("cargo test");
    assert!(sys.contains("`cargo test`"), "{sys}");
    assert!(sys.contains("leave the slow full run to it"), "{sys}");
    assert!(!sys.contains("No command checks your work"), "{sys}");
}
