//! The model's `bash` goes through `exec_user` (its session); the harness's commands through
//! `exec`.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>);
impl Shell for Recorder {
    fn exec(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.0.lock().unwrap().push("exec");
        Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    fn exec_user(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.0.lock().unwrap().push("exec_user");
        Ok(Output {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    fn describe(&self) -> String {
        String::new()
    }
}

#[test]
fn the_models_bash_uses_its_session() {
    let d = tempfile::tempdir().unwrap();
    let r = Recorder::default();
    execute(
        d.path(),
        &r,
        &LocalFs,
        "bash",
        &json!({"command": "cd src"}),
    );
    assert_eq!(*r.0.lock().unwrap(), vec!["exec_user"]);
}
