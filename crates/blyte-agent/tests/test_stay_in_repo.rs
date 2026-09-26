//! The model is told the workspace boundary, and a failed gate asks for a fix to the change, not
//! to the machine: after a gate failure a model once spent 50 minutes deleting a host cache.

use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::{LocalShell, Shell};
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

fn run(gate: &str) -> (String, String) {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             touch x && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
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
            max_turns: 4,
            max_gate_attempts: 2,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    r.run(&mut State::new("g", gate));
    let seen = p.seen.lock().unwrap();
    (
        seen[0].system.clone(),
        format!("{:?}", seen.last().unwrap().messages),
    )
}

#[test]
fn the_system_prompt_states_the_workspace_boundary() {
    let (sys, _) = run("true");
    assert!(
        sys.contains("Change nothing outside the repository"),
        "{sys}"
    );
}

#[test]
fn a_failed_gate_asks_for_a_fix_to_the_change_not_the_machine() {
    let (_, last) = run("false");
    assert!(last.contains("Verification did not pass"), "{last:.400}");
    assert!(
        last.contains("fix your change, not the machine"),
        "{last:.600}"
    );
}
