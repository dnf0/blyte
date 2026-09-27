use blyte_agent::context::Conversational;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Outcome, Runner};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_provider::{Provider, ProviderError, Request, Response};

struct DoneProvider;

impl Provider for DoneProvider {
    fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }
    fn model_id(&self) -> String {
        "done".into()
    }
}

fn runner<'a>(p: &'a DoneProvider, root: &std::path::Path) -> Runner<'a> {
    Runner {
        provider: p,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.to_path_buf(),
        budget: Budget {
            max_turns: 5,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
    }
}

#[test]
fn gate_wait_ms_is_recorded_per_gate_attempt() {
    let d = tempfile::tempdir().unwrap();
    let slot_dir = tempfile::tempdir().unwrap();
    std::env::set_var("BLYTE_GATE_SLOT_DIR", slot_dir.path());
    let p = DoneProvider;
    let mut st = State::new("g", "true");
    let (outcome, report) = runner(&p, d.path()).run(&mut st);
    std::env::remove_var("BLYTE_GATE_SLOT_DIR");
    assert_eq!(outcome, Outcome::Verified);
    assert_eq!(report.gate_wait_ms.len(), report.gate_attempts as usize);
}
