use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use blyte_agent::approval::{Approval, Approver, NoApprover, PublishDetails};
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::{
    execute, set_blyte_started_by, set_publish_tool, specs, PublishTool, PUBLISH_BODY_MAX_BYTES,
    PUBLISH_TITLE_MAX_CHARS,
};
use serde_json::json;

static PUBLISH_LOCK: Mutex<()> = Mutex::new(());

struct PublishSeamGuard {
    _lock: MutexGuard<'static, ()>,
}

impl PublishSeamGuard {
    fn acquire(tool: Option<Arc<dyn PublishTool>>) -> Self {
        let lock = PUBLISH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_blyte_started_by(None);
        set_publish_tool(tool);
        Self { _lock: lock }
    }
}

impl Drop for PublishSeamGuard {
    fn drop(&mut self) {
        set_publish_tool(None);
        set_blyte_started_by(None);
    }
}

struct UnusedShell;

impl Shell for UnusedShell {
    fn exec(&self, _: &str, _: &Path, _: Duration) -> Result<Output, String> {
        panic!("shell must not be called by publish");
    }

    fn describe(&self) -> String {
        String::new()
    }
}

#[derive(Default)]
struct RecordingPublishTool {
    calls: Mutex<Vec<(String, String, String)>>,
}

impl RecordingPublishTool {
    fn recorded(&self) -> Vec<(String, String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl PublishTool for RecordingPublishTool {
    fn publish(&self, request: &str, title: &str, body: &str) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((request.to_string(), title.to_string(), body.to_string()));
        Ok("https://example.test/org/repo/pull/1".to_string())
    }
}

#[test]
fn publish_is_not_listed_without_seam_and_listed_with_exact_fields_with_seam() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _g = PublishSeamGuard::acquire(None);
        let all = specs();
        assert!(!all.iter().any(|t| t.name == "publish"));
        let out = execute(
            dir.path(),
            &UnusedShell,
            &LocalFs,
            "publish",
            &json!({"request": "req-1", "title": "Add feature", "body": "Details"}),
        );
        assert!(out.is_error);
        assert!(out.content.contains("not available"), "{}", out.content);
    }

    {
        let tool = Arc::new(RecordingPublishTool::default());
        let _g = PublishSeamGuard::acquire(Some(tool.clone()));
        let all = specs();
        let publish = all
            .iter()
            .find(|t| t.name == "publish")
            .expect("publish must be listed when seam is registered");
        let props = publish.parameters["properties"].as_object().unwrap();
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["body", "request", "title"]);
        assert_eq!(publish.parameters["additionalProperties"], json!(false));

        let extra = execute(
            dir.path(),
            &UnusedShell,
            &LocalFs,
            "publish",
            &json!({
                "request": "req-1",
                "title": "Add feature",
                "body": "Details",
                "branch": "main"
            }),
        );
        assert!(extra.is_error);
        assert!(extra.content.contains("branch"), "{}", extra.content);
        assert!(tool.recorded().is_empty());
    }
}

#[test]
fn publish_refuses_peer_turn_and_invalid_title_or_body() {
    let dir = tempfile::tempdir().unwrap();
    let tool = Arc::new(RecordingPublishTool::default());
    let _g = PublishSeamGuard::acquire(Some(tool.clone()));

    set_blyte_started_by(Some("other-surface".to_string()));
    let peer = execute(
        dir.path(),
        &UnusedShell,
        &LocalFs,
        "publish",
        &json!({"request": "req-1", "title": "Add feature", "body": "Details"}),
    );
    assert!(peer.is_error);
    assert!(peer.content.contains("other-surface"), "{}", peer.content);
    assert!(tool.recorded().is_empty());
    set_blyte_started_by(None);

    let blank_title = execute(
        dir.path(),
        &UnusedShell,
        &LocalFs,
        "publish",
        &json!({"request": "req-1", "title": "   \t\n ", "body": "Details"}),
    );
    assert!(blank_title.is_error);
    assert!(
        blank_title.content.contains("blank"),
        "{}",
        blank_title.content
    );
    assert!(tool.recorded().is_empty());

    let too_long_title = "a".repeat(PUBLISH_TITLE_MAX_CHARS + 1);
    let long_title = execute(
        dir.path(),
        &UnusedShell,
        &LocalFs,
        "publish",
        &json!({"request": "req-1", "title": too_long_title, "body": "Details"}),
    );
    assert!(long_title.is_error);
    assert!(long_title.content.contains("200"), "{}", long_title.content);
    assert!(tool.recorded().is_empty());

    let too_long_body = "b".repeat(PUBLISH_BODY_MAX_BYTES + 1);
    let long_body = execute(
        dir.path(),
        &UnusedShell,
        &LocalFs,
        "publish",
        &json!({"request": "req-1", "title": "Valid", "body": too_long_body}),
    );
    assert!(long_body.is_error);
    assert!(long_body.content.contains("20000"), "{}", long_body.content);
    assert!(tool.recorded().is_empty());

    let max_title = "é".repeat(PUBLISH_TITLE_MAX_CHARS);
    let max_body = "x".repeat(PUBLISH_BODY_MAX_BYTES);
    let ok = execute(
        dir.path(),
        &UnusedShell,
        &LocalFs,
        "publish",
        &json!({"request": "req-1", "title": max_title, "body": max_body}),
    );
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(ok.content, "https://example.test/org/repo/pull/1");
    assert_eq!(tool.recorded().len(), 1);
}

#[test]
fn no_approver_returns_unavailable_for_ask_publish() {
    let details = PublishDetails {
        request: "req-1".to_string(),
        remote_url: "https://example.test/org/repo.git".to_string(),
        branch: "blyte/req-1".to_string(),
        base: "main".to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        commits: vec!["Add feature".to_string()],
        title: "Add feature".to_string(),
        body: "Body".to_string(),
    };
    assert_eq!(NoApprover.ask_publish(&details), Approval::Unavailable);
}
