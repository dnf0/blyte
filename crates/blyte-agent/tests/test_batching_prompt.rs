use std::sync::Mutex;

use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct Stopper {
    captured: Mutex<Option<Request>>,
}

impl Provider for Stopper {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let mut guard = self.captured.lock().unwrap();
        if guard.is_none() {
            *guard = Some(req.clone());
        }
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "stopper".into()
    }
}

#[test]
fn prompt_and_read_description_guide_batching() {
    let d = tempfile::tempdir().unwrap();
    let p = Stopper {
        captured: Mutex::new(None),
    };
    let runner = Runner {
        provider: &p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: d.path().to_path_buf(),
        budget: Budget {
            max_turns: 2,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    };
    runner.run(&mut State::new("anything", "true"));

    let req = p
        .captured
        .lock()
        .unwrap()
        .take()
        .expect("provider captured a request");
    assert!(
        req.system
            .contains("call `read` three times in the same response"),
        "system prompt missing batching example: {}",
        req.system
    );
    assert!(
        req.system.contains("turns are what a run costs"),
        "system prompt missing cost explanation: {}",
        req.system
    );

    let read_spec = req
        .tools
        .iter()
        .find(|t| t.name == "read")
        .expect("read tool spec present");
    assert!(
        read_spec
            .description
            .ends_with("call read once per file in the same response."),
        "read description unexpected: {}",
        read_spec.description
    );
}
