//! The system prompt asks for the gate's quick parts before stopping: every gate failure in
//! shadow runs S1-S3 was a formatting or lint error the model could have caught in seconds,
//! and each cost a full rerun of the suite.

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

fn system_prompt() -> String {
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
    };
    r.run(&mut State::new("g", "true"));
    let sys = p.seen.lock().unwrap()[0].system.clone();
    sys
}

#[test]
fn the_prompt_asks_for_the_quick_checks_before_stopping() {
    let sys = system_prompt();
    assert!(sys.contains("run its quick parts yourself"), "{sys}");
    assert!(sys.contains("formatting, lint"), "{sys}");
}

#[test]
fn the_prompt_no_longer_reads_as_a_ban_on_checking() {
    let sys = system_prompt();
    assert!(!sys.contains("cannot run it as your final step"), "{sys}");
    assert!(sys.contains("you do not decide the verdict"));
}
