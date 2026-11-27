fn load_surface_html() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/surface/index.html");
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
}

#[test]
fn page_has_its_parts() {
    let html = load_surface_html();
    for id in [
        "conversation",
        "conversation-form",
        "model-select",
        "effort-select",
        "queue",
        "map",
        "report-reader",
        "attached-reports",
        "lost-server",
        "missing-token",
        "requests-panel",
        "requests",
    ] {
        let needle = format!("id=\"{id}\"");
        assert!(
            html.contains(&needle),
            "surface page must contain element with {needle}"
        );
    }
}

#[test]
fn page_calls_every_route() {
    let html = load_surface_html();
    for route in [
        "/api/session",
        "/api/session/events",
        "/api/session/messages",
        "/api/session/approvals/",
        "/api/models",
        "/api/session/model",
        "/api/session/effort",
        "/api/runs",
        "/decision",
        "/diff",
        "/api/map",
        "/api/requests",
        "/push",
        "/pull-request",
    ] {
        assert!(html.contains(route), "surface page must call route {route}");
    }
}

#[test]
fn page_handles_every_entry_kind() {
    let html = load_surface_html();
    for kind in [
        "user_message",
        "peer_message",
        "turn",
        "tool_started",
        "tool_finished",
        "retry",
        "error",
        "idle",
        "model_changed",
        "effort_changed",
        "decision",
        "approval_requested",
        "approval_answered",
    ] {
        let double_quoted = format!("\"{kind}\"");
        let single_quoted = format!("'{kind}'");
        assert!(
            html.contains(&double_quoted) || html.contains(&single_quoted),
            "surface page must handle entry kind {kind} as a quoted string"
        );
    }
    assert!(
        html.contains("unknown entry kind"),
        "surface page must handle unknown entry kinds explicitly"
    );
}

#[test]
fn mock_only_parts_are_gone() {
    let html = load_surface_html();
    for forbidden in [
        "/api/replay",
        "guide-close",
        "checkin-skip",
        "Save record",
        "Next arrival",
    ] {
        assert!(
            !html.contains(forbidden),
            "surface page must not contain mock-only string {forbidden}"
        );
    }
}

#[test]
fn no_external_resources() {
    let html = load_surface_html();
    let lower = html.to_ascii_lowercase();
    for attr in [
        "src=\"http://",
        "src='http://",
        "src=\"https://",
        "src='https://",
        "href=\"http://",
        "href='http://",
        "href=\"https://",
        "href='https://",
    ] {
        assert!(
            !lower.contains(attr),
            "surface page must not reference external resources via {attr}"
        );
    }
    assert!(
        !lower.contains("@import"),
        "surface page must not use @import"
    );
    for forbidden in ["window.prompt(", "alert(", "confirm("] {
        assert!(
            !html.contains(forbidden),
            "surface page must not call {forbidden}"
        );
    }
}

#[test]
fn no_raw_html_from_the_server() {
    let html = load_surface_html();
    assert!(
        html.contains("function escapeHtml"),
        "surface page must define function escapeHtml"
    );
    let start = html
        .find("function renderTurn(")
        .expect("surface page must define function renderTurn");
    let rest = &html[start..];
    let end = rest[1..]
        .find("\n    function ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    let render_turn_body = &rest[..end];
    assert!(
        render_turn_body.contains("renderReportMarkdown("),
        "renderTurn must call renderReportMarkdown("
    );
}

fn extract_inline_script(html: &str) -> &str {
    let start = html
        .find("<script>")
        .expect("surface page must contain <script>")
        + "<script>".len();
    let end = html[start..]
        .find("</script>")
        .expect("surface page must contain </script>")
        + start;
    &html[start..end]
}

fn run_surface_script(harness: &str) {
    let html = load_surface_html();
    let script = extract_inline_script(&html);
    let combined = format!("{script}\n{harness}\n");
    // The script goes in on stdin: as an argument it is longer than Linux allows for one argument.
    use std::io::Write;
    let mut child = std::process::Command::new("node")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("node must be available to run surface script tests");
    let mut stdin = child.stdin.take().expect("node stdin");
    let writer = std::thread::spawn(move || stdin.write_all(combined.as_bytes()));
    let output = child.wait_with_output().expect("wait for node");
    writer
        .join()
        .expect("stdin writer thread")
        .expect("write the script to node");
    assert!(
        output.status.success(),
        "node test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn blyte_tool_formatting_and_wait_exit_2_behaviour() {
    let harness = r#"
const assert = require("assert");

assert.strictEqual(
  formatToolInput('{"args":["wait","--any","IS1"]}', "blyte"),
  "blyte wait --any IS1"
);
assert.strictEqual(
  formatToolInput({ args: ["wait", "--any", "IS1"] }, "blyte"),
  "blyte wait --any IS1"
);
assert.strictEqual(
  formatToolInput('{"args":["dispatch","T1","task.md"],"background":true}', "blyte"),
  "blyte dispatch T1 task.md (background)"
);
assert.strictEqual(
  formatToolInput('{"args":["dispatch","T1","task.md"],"background":false}', "blyte"),
  "blyte dispatch T1 task.md"
);
assert.strictEqual(
  formatToolInput('{"args":["dispatch","T1","task.md"]}', "blyte"),
  "blyte dispatch T1 task.md"
);
assert.strictEqual(
  formatToolInput('{"args":["dispatch","T1","my task.md",""]}', "blyte"),
  'blyte dispatch T1 "my task.md" ""'
);
assert.strictEqual(
  formatToolInput("blyte wait --any IS1", "blyte"),
  "blyte wait --any IS1"
);
assert.strictEqual(
  formatToolInput('{"args":["wait","--any","IS1"]}', "bash"),
  '{"args":["wait","--any","IS1"]}'
);

const ts = "2026-01-01T00:00:00Z";

const renderedWaitExit2 = renderEntry(
  { seq: 1, ts, kind: "tool_started", name: "blyte", input: '{"args":["wait","--any","IS1"]}' },
  { matchedFinish: { seq: 2, ts, kind: "tool_finished", name: "blyte", summary: "exit 2", is_error: true } }
);
assert.ok(renderedWaitExit2.includes('<span class="tool-input-first">blyte wait --any IS1</span>'), renderedWaitExit2);
assert.ok(renderedWaitExit2.includes('<pre class="tool-input-full">blyte wait --any IS1</pre>'), renderedWaitExit2);
assert.ok(renderedWaitExit2.includes('<span class="tool-status tool-waiting">still waiting</span>'), renderedWaitExit2);
assert.ok(!renderedWaitExit2.includes("tool-error"), renderedWaitExit2);
assert.ok(!renderedWaitExit2.includes("exit 2"), renderedWaitExit2);

const renderedWaitStillWaiting = renderEntry(
  { seq: 3, ts, kind: "tool_started", name: "blyte", input: "blyte wait --any IS1" },
  { matchedFinish: { seq: 4, ts, kind: "tool_finished", name: "blyte", summary: "still waiting", is_error: false } }
);
assert.ok(renderedWaitStillWaiting.includes('<span class="tool-status tool-waiting">still waiting</span>'), renderedWaitStillWaiting);
assert.ok(!renderedWaitStillWaiting.includes("tool-error"), renderedWaitStillWaiting);

const renderedWaitExit1 = renderEntry(
  { seq: 5, ts, kind: "tool_started", name: "blyte", input: '{"args":["wait","--any","IS1"]}' },
  { matchedFinish: { seq: 6, ts, kind: "tool_finished", name: "blyte", summary: "exit 1", is_error: true } }
);
assert.ok(renderedWaitExit1.includes('<span class="tool-status tool-error">exit 1</span>'), renderedWaitExit1);

const renderedStatusExit2 = renderEntry(
  { seq: 7, ts, kind: "tool_started", name: "blyte", input: '{"args":["status","IS1"]}' },
  { matchedFinish: { seq: 8, ts, kind: "tool_finished", name: "blyte", summary: "exit 2", is_error: true } }
);
assert.ok(renderedStatusExit2.includes('<span class="tool-input-first">blyte status IS1</span>'), renderedStatusExit2);
assert.ok(renderedStatusExit2.includes('<span class="tool-status tool-error">exit 2</span>'), renderedStatusExit2);

const renderedBashExit2 = renderEntry(
  { seq: 9, ts, kind: "tool_started", name: "bash", input: "blyte wait --any IS1" },
  { matchedFinish: { seq: 10, ts, kind: "tool_finished", name: "bash", summary: "exit 2", is_error: true } }
);
assert.ok(renderedBashExit2.includes('<span class="tool-status tool-error">exit 2</span>'), renderedBashExit2);

const renderedBgDispatch = renderEntry(
  { seq: 11, ts, kind: "tool_started", name: "blyte", input: '{"args":["dispatch","T1","task.md"],"background":true}' },
  { matchedFinish: { seq: 12, ts, kind: "tool_finished", name: "blyte", summary: "started T1 in the background", is_error: false } }
);
assert.ok(renderedBgDispatch.includes('<span class="tool-input-first">blyte dispatch T1 task.md (background)</span>'), renderedBgDispatch);
assert.ok(renderedBgDispatch.includes('<pre class="tool-input-full">blyte dispatch T1 task.md (background)</pre>'), renderedBgDispatch);
assert.ok(renderedBgDispatch.includes('<span class="tool-status tool-done">started T1 in the background</span>'), renderedBgDispatch);

function makeNode(html) {
  const seqMatch = html.match(/data-seq="([^"]*)"/);
  const statusMatch = html.match(/<span class="([^"]*tool-status[^"]*)">([^<]*)<\/span>/);
  const statusEl = statusMatch ? { className: statusMatch[1], textContent: statusMatch[2] } : null;
  return {
    html,
    seq: seqMatch ? seqMatch[1] : null,
    statusEl,
    hasAttribute(name) { return name === "data-seq" && this.seq !== null; },
    getAttribute(name) { return name === "data-seq" ? this.seq : null; },
    querySelector(sel) { return sel === ".tool-status" ? this.statusEl : null; },
  };
}

function makeContainer() {
  return {
    children: [],
    scrollHeight: 0,
    scrollTop: 0,
    clientHeight: 100,
    appendChild(node) { this.children.push(node); },
    insertBefore(node, ref) {
      const idx = this.children.indexOf(ref);
      if (idx === -1) this.children.push(node);
      else this.children.splice(idx, 0, node);
    },
    querySelector(sel) {
      if (sel === ".thread-empty") return null;
      const m = sel.match(/^\[data-seq="([^"]+)"\]$/);
      if (m) return this.children.find(c => c.seq === m[1]) || null;
      return null;
    },
  };
}

const threadEl = makeContainer();
const activityEl = makeContainer();

global.document = {
  getElementById(id) {
    if (id === "conversation-thread") return threadEl;
    if (id === "activity") return activityEl;
    return null;
  },
  createElement(tag) {
    if (tag === "template") {
      return {
        _inner: "",
        set innerHTML(v) { this._inner = v; },
        get content() { return { firstElementChild: makeNode(this._inner) }; },
      };
    }
    return {};
  },
};

applyLogEntries([
  { seq: 101, ts, kind: "tool_started", name: "blyte", input: '{"args":["wait","--any","IS1"]}' },
]);
applyLogEntries([
  { seq: 102, ts, kind: "tool_finished", name: "blyte", summary: "exit 2", is_error: true },
]);
const liveWait = activityEl.querySelector('[data-seq="101"]');
assert.strictEqual(liveWait.statusEl.className, "tool-status tool-waiting");
assert.strictEqual(liveWait.statusEl.textContent, "still waiting");

applyLogEntries([
  { seq: 103, ts, kind: "tool_started", name: "blyte", input: '{"args":["wait","--any","IS2"]}' },
  { seq: 104, ts, kind: "tool_finished", name: "blyte", summary: "exit 1", is_error: true },
  { seq: 105, ts, kind: "tool_started", name: "blyte", input: '{"args":["status","IS2"]}' },
  { seq: 106, ts, kind: "tool_finished", name: "blyte", summary: "exit 2", is_error: true },
  { seq: 107, ts, kind: "tool_started", name: "bash", input: "blyte wait --any IS2" },
  { seq: 108, ts, kind: "tool_finished", name: "bash", summary: "exit 2", is_error: true },
  { seq: 109, ts, kind: "tool_started", name: "blyte", input: "blyte wait --any IS3" },
  { seq: 110, ts, kind: "tool_finished", name: "blyte", summary: "still waiting", is_error: false },
]);
assert.strictEqual(activityEl.querySelector('[data-seq="103"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(activityEl.querySelector('[data-seq="103"]').statusEl.textContent, "exit 1");
assert.strictEqual(activityEl.querySelector('[data-seq="105"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(activityEl.querySelector('[data-seq="105"]').statusEl.textContent, "exit 2");
assert.strictEqual(activityEl.querySelector('[data-seq="107"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(activityEl.querySelector('[data-seq="107"]').statusEl.textContent, "exit 2");
assert.strictEqual(activityEl.querySelector('[data-seq="109"]').statusEl.className, "tool-status tool-waiting");
assert.strictEqual(activityEl.querySelector('[data-seq="109"]').statusEl.textContent, "still waiting");
assert.strictEqual(threadEl.children.filter(c => c.html.includes("entry-tool")).length, 0);
"#;
    run_surface_script(harness);
}

#[test]
fn page_carries_surface_token_handling() {
    let html = load_surface_html();
    assert!(
        html.contains("Cannot send until this page is opened from the URL blyte printed."),
        "missing missing-token banner text"
    );
    assert!(
        html.contains("X-Blyte-Token"),
        "missing surface token request header name"
    );
    assert!(
        html.contains("sessionStorage") && html.contains("replaceState"),
        "missing fragment token persistence and address-bar cleanup"
    );
}

#[test]
fn page_renders_approvals_as_escaped_text_without_approve_all() {
    let html = load_surface_html();
    let start = html
        .find("case \"approval_requested\":")
        .expect("surface page must render approval_requested entries");
    let rest = &html[start..];
    let end = rest
        .find("default:")
        .expect("approval cases must precede default case");
    let body = &rest[..end];
    assert!(
        body.contains("escapeHtml(entry.command)"),
        "approval rendering must escape entry.command"
    );
    assert!(
        body.contains("escapeHtml(entry.reason)"),
        "approval rendering must escape entry.reason"
    );
    assert!(
        body.contains("data-approval-answer=\"yes\"")
            && body.contains("data-approval-answer=\"no\""),
        "approval rendering must provide approve and refuse buttons"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        !lower.contains("approve all") && !lower.contains("remember"),
        "approval rendering must not offer approve-all or remembered answers"
    );
}

#[test]
fn requests_panel_renders_all_six_states_and_missing_field_errors() {
    let harness = r#"
const assert = require("assert");

const waitingHtml = renderRequestCard({
  request: "req-wait",
  runs: ["T1", "T2"],
  state: "waiting",
  waiting_reasons: ["T1 is still running", "T2 failed check"],
  started_by: null,
});
assert.ok(waitingHtml.includes("req-wait"), waitingHtml);
assert.ok(waitingHtml.includes("T1") && waitingHtml.includes("T2"), waitingHtml);
assert.ok(waitingHtml.includes("T1 is still running"), waitingHtml);
assert.ok(waitingHtml.includes("T2 failed check"), waitingHtml);
assert.ok(waitingHtml.includes('<div class="request-reason">T1 is still running</div>'), waitingHtml);
assert.ok(waitingHtml.includes('<div class="request-reason">T2 failed check</div>'), waitingHtml);

const combiningHtml = renderRequestCard({
  request: "req-comb",
  runs: ["T3"],
  state: "combining",
  started_by: null,
});
assert.ok(combiningHtml.includes("req-comb"), combiningHtml);
assert.ok(combiningHtml.includes("T3"), combiningHtml);
assert.ok(combiningHtml.toLowerCase().includes("combining"), combiningHtml);

const combinedHtml = renderRequestCard({
  request: "req-done",
  runs: ["T4", "T5"],
  state: "combined",
  branch: "blyte/req-done",
  base: "main",
  commits: [
    { sha: "abcdef1234567890", subject: "feat: first commit" },
    { sha: "9876543210fedcba", subject: "fix: second commit" },
  ],
  suggested_title: "feat: first commit",
  suggested_body: "feat: first commit\nfix: second commit",
  pushed: false,
  pull_request_url: null,
  started_by: null,
});
assert.ok(combinedHtml.includes("req-done"), combinedHtml);
assert.ok(combinedHtml.includes("T4") && combinedHtml.includes("T5"), combinedHtml);
assert.ok(combinedHtml.includes("blyte/req-done"), combinedHtml);
assert.ok(combinedHtml.includes("abcdef1"), combinedHtml);
assert.ok(!combinedHtml.includes("abcdef1234567890"), combinedHtml);
assert.ok(combinedHtml.includes("feat: first commit"), combinedHtml);
assert.ok(combinedHtml.includes("9876543"), combinedHtml);
assert.ok(combinedHtml.includes("fix: second commit"), combinedHtml);
assert.ok(combinedHtml.includes("check passed"), combinedHtml);

const conflictHtml = renderRequestCard({
  request: "req-conf",
  runs: ["T6", "T7"],
  state: "conflict",
  conflicting_run: "T7",
  conflicting_files: ["src/main.rs", "src/lib.rs"],
  started_by: null,
});
assert.ok(conflictHtml.includes("req-conf"), conflictHtml);
assert.ok(conflictHtml.includes("T6") && conflictHtml.includes("T7"), conflictHtml);
assert.ok(conflictHtml.includes("src/main.rs"), conflictHtml);
assert.ok(conflictHtml.includes("src/lib.rs"), conflictHtml);

const checkFailedHtml = renderRequestCard({
  request: "req-fail",
  runs: ["T8"],
  state: "check_failed",
  failing_step: "cargo test",
  output_tail: "test foo ... FAILED\nassertion failed",
  started_by: null,
});
assert.ok(checkFailedHtml.includes("req-fail"), checkFailedHtml);
assert.ok(checkFailedHtml.includes("T8"), checkFailedHtml);
assert.ok(checkFailedHtml.includes("cargo test"), checkFailedHtml);
assert.ok(
  checkFailedHtml.includes('<pre class="details-pre request-output-tail">test foo ... FAILED\nassertion failed</pre>'),
  checkFailedHtml
);

const errorHtml = renderRequestCard({
  request: "req-err",
  runs: ["T9"],
  state: "error",
  error: "git cherry-pick exited 128",
  started_by: null,
});
assert.ok(errorHtml.includes("req-err"), errorHtml);
assert.ok(errorHtml.includes("T9"), errorHtml);
assert.ok(errorHtml.includes("git cherry-pick exited 128"), errorHtml);

const missingBranchHtml = renderRequestCard({
  request: "req-bad-branch",
  runs: ["T10"],
  state: "combined",
  commits: [],
  pushed: false,
  pull_request_url: null,
  started_by: null,
});
assert.ok(missingBranchHtml.includes("branch"), missingBranchHtml);
assert.ok(missingBranchHtml.includes("request-validation-error"), missingBranchHtml);

const missingStepHtml = renderRequestCard({
  request: "req-bad-step",
  runs: ["T11"],
  state: "check_failed",
  output_tail: "boom",
  started_by: null,
});
assert.ok(missingStepHtml.includes("failing_step"), missingStepHtml);
assert.ok(missingStepHtml.includes("request-validation-error"), missingStepHtml);

const missingErrorHtml = renderRequestCard({
  request: "req-bad-err",
  runs: ["T12"],
  state: "error",
  started_by: null,
});
assert.ok(missingErrorHtml.includes("error"), missingErrorHtml);
assert.ok(missingErrorHtml.includes("request-validation-error"), missingErrorHtml);

const failedFetchHtml = renderRequestsList([], { error: "/api/requests failed with HTTP 500" });
assert.ok(failedFetchHtml.includes("/api/requests failed with HTTP 500"), failedFetchHtml);
assert.ok(!failedFetchHtml.includes("No requests"), failedFetchHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn requests_panel_combined_buttons_pr_form_and_escaping() {
    let harness = r#"
const assert = require("assert");

const baseCombined = {
  request: "req-1",
  runs: ["T1"],
  state: "combined",
  branch: "blyte/req-1",
  base: "main",
  commits: [{ sha: "1234567890abcdef", subject: "add <script>alert(1)</script> safely" }],
  suggested_title: "Suggested PR Title",
  suggested_body: "Suggested PR Body",
  pushed: false,
  pull_request_url: null,
  started_by: null,
};

const notPushedHtml = renderRequestCard(baseCombined);
assert.ok(
  notPushedHtml.includes('data-request-push="req-1">Push branch</button>'),
  notPushedHtml
);
assert.ok(
  notPushedHtml.includes('data-request-open-pr="req-1" disabled>Open pull request</button>'),
  notPushedHtml
);
assert.ok(!notPushedHtml.includes("<script>"), notPushedHtml);
assert.ok(notPushedHtml.includes("&lt;script&gt;alert(1)&lt;/script&gt;"), notPushedHtml);

const pushInFlightHtml = renderRequestCard(baseCombined, { pushInFlight: true });
assert.ok(
  pushInFlightHtml.includes('data-request-push="req-1" disabled>Push branch</button>'),
  pushInFlightHtml
);

const pushedNoUrlHtml = renderRequestCard(Object.assign({}, baseCombined, { pushed: true }));
assert.ok(
  pushedNoUrlHtml.includes('data-request-push="req-1" disabled>Pushed</button>'),
  pushedNoUrlHtml
);
assert.ok(
  pushedNoUrlHtml.includes('data-request-open-pr="req-1">Open pull request</button>'),
  pushedNoUrlHtml
);

const prFormOpenHtml = renderRequestCard(
  Object.assign({}, baseCombined, { pushed: true }),
  { prFormOpen: true }
);
assert.ok(prFormOpenHtml.includes('value="Suggested PR Title"'), prFormOpenHtml);
assert.ok(prFormOpenHtml.includes('>Suggested PR Body</textarea>'), prFormOpenHtml);
assert.ok(
  prFormOpenHtml.includes('data-request-pr-submit="req-1">Submit pull request</button>'),
  prFormOpenHtml
);

const emptyTitleFormHtml = renderRequestCard(
  Object.assign({}, baseCombined, { pushed: true }),
  { prFormOpen: true, prDraft: { title: "   ", body: "User body" } }
);
assert.ok(
  emptyTitleFormHtml.includes('data-request-pr-submit="req-1" disabled>Submit pull request</button>'),
  emptyTitleFormHtml
);
assert.ok(emptyTitleFormHtml.includes('>User body</textarea>'), emptyTitleFormHtml);

const preservedDraftHtml = renderRequestCard(
  Object.assign({}, baseCombined, { pushed: true }),
  { prFormOpen: true, prDraft: { title: "Edited title", body: "Edited body" } }
);
assert.ok(preservedDraftHtml.includes('value="Edited title"'), preservedDraftHtml);
assert.ok(preservedDraftHtml.includes('>Edited body</textarea>'), preservedDraftHtml);
assert.ok(!preservedDraftHtml.includes("Suggested PR Title"), preservedDraftHtml);

const withHttpsUrlHtml = renderRequestCard(
  Object.assign({}, baseCombined, {
    pushed: true,
    pull_request_url: "https://github.com/example/repo/pull/42",
  })
);
assert.ok(
  withHttpsUrlHtml.includes('<a class="request-pr-link" href="https://github.com/example/repo/pull/42"'),
  withHttpsUrlHtml
);
assert.ok(!withHttpsUrlHtml.includes("data-request-open-pr"), withHttpsUrlHtml);
assert.ok(!withHttpsUrlHtml.includes("Open pull request"), withHttpsUrlHtml);

const withNonHttpsUrlHtml = renderRequestCard(
  Object.assign({}, baseCombined, {
    pushed: true,
    pull_request_url: "javascript:alert(1)",
  })
);
assert.ok(!withNonHttpsUrlHtml.includes("<a "), withNonHttpsUrlHtml);
assert.ok(
  withNonHttpsUrlHtml.includes('<span class="request-pr-url-text">javascript:alert(1)</span>'),
  withNonHttpsUrlHtml
);

const withHttpUrlHtml = renderRequestCard(
  Object.assign({}, baseCombined, {
    pushed: true,
    pull_request_url: "http://example.com/pr/1",
  })
);
assert.ok(!withHttpUrlHtml.includes("<a "), withHttpUrlHtml);
assert.ok(
  withHttpUrlHtml.includes('<span class="request-pr-url-text">http://example.com/pr/1</span>'),
  withHttpUrlHtml
);

const boldErrHtml = renderRequestCard({
  request: "req-err-html",
  runs: ["T2"],
  state: "error",
  error: "failed <b>badly</b>",
  started_by: null,
});
assert.ok(!boldErrHtml.includes("<b>"), boldErrHtml);
assert.ok(boldErrHtml.includes("failed &lt;b&gt;badly&lt;/b&gt;"), boldErrHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn requests_post_helpers_send_secret_header_handle_errors_and_refetch() {
    let harness = r#"
const assert = require("assert");

(async () => {
  surfaceToken = "secret-token-123";
  const calls = [];
  global.fetch = async (url, options) => {
    calls.push({ url, options });
    if (url === "/api/requests/req-1/push") {
      return {
        ok: true,
        status: 200,
        async json() { return { pushed: true }; },
      };
    }
    if (url === "/api/requests/req-1/pull-request") {
      return {
        ok: true,
        status: 200,
        async json() { return { url: "https://github.com/org/repo/pull/7" }; },
      };
    }
    if (url === "/api/requests/req-fail/push") {
      return {
        ok: false,
        status: 409,
        async json() { return { error: "branch already diverged" }; },
      };
    }
    if (url === "/api/requests/req-fail-pr/pull-request") {
      return {
        ok: false,
        status: 500,
        async json() { return { error: "gh pr create failed" }; },
      };
    }
    if (url === "/api/requests") {
      return {
        ok: true,
        status: 200,
        async json() { return []; },
      };
    }
    throw new Error("unexpected fetch url: " + url);
  };

  let inFlightObservedPush = false;
  const origFetch = global.fetch;
  global.fetch = async (url, options) => {
    if (url === "/api/requests/req-1/push") {
      inFlightObservedPush = requestPushInFlight.has("req-1");
    }
    return origFetch(url, options);
  };

  const pushOk = await postRequestPush("req-1");
  assert.strictEqual(pushOk, true);
  assert.strictEqual(inFlightObservedPush, true);
  assert.strictEqual(calls.length, 2);
  assert.strictEqual(calls[0].url, "/api/requests/req-1/push");
  assert.strictEqual(calls[0].options.method, "POST");
  assert.strictEqual(calls[0].options.headers["X-Blyte-Token"], "secret-token-123");
  assert.strictEqual(calls[1].url, "/api/requests");
  assert.strictEqual(requestPushInFlight.has("req-1"), false);

  calls.length = 0;
  let inFlightObservedPr = false;
  global.fetch = async (url, options) => {
    if (url === "/api/requests/req-1/pull-request") {
      inFlightObservedPr = requestPrInFlight.has("req-1");
    }
    return origFetch(url, options);
  };
  requestPrFormOpen.add("req-1");
  requestPrDrafts.set("req-1", { title: "Add feature", body: "Details here" });
  const prOk = await postRequestPullRequest("req-1", "Add feature", "Details here");
  assert.strictEqual(prOk, true);
  assert.strictEqual(inFlightObservedPr, true);
  assert.strictEqual(calls.length, 2);
  assert.strictEqual(calls[0].url, "/api/requests/req-1/pull-request");
  assert.strictEqual(calls[0].options.method, "POST");
  assert.strictEqual(calls[0].options.headers["X-Blyte-Token"], "secret-token-123");
  assert.strictEqual(calls[0].options.headers["Content-Type"], "application/json");
  assert.deepStrictEqual(JSON.parse(calls[0].options.body), {
    title: "Add feature",
    body: "Details here",
  });
  assert.strictEqual(calls[1].url, "/api/requests");
  assert.strictEqual(requestPrInFlight.has("req-1"), false);
  assert.strictEqual(requestPrFormOpen.has("req-1"), false);

  calls.length = 0;
  const pushErr = await postRequestPush("req-fail");
  assert.strictEqual(pushErr, false);
  assert.strictEqual(calls.length, 1);
  assert.strictEqual(requestPushInFlight.has("req-fail"), false);
  assert.strictEqual(requestCardErrors.get("req-fail"), "branch already diverged");

  calls.length = 0;
  requestPrFormOpen.add("req-fail-pr");
  requestPrDrafts.set("req-fail-pr", { title: "Keep me", body: "Keep body" });
  const prErr = await postRequestPullRequest("req-fail-pr", "Keep me", "Keep body");
  assert.strictEqual(prErr, false);
  assert.strictEqual(calls.length, 1);
  assert.strictEqual(requestPrInFlight.has("req-fail-pr"), false);
  assert.strictEqual(requestCardErrors.get("req-fail-pr"), "gh pr create failed");
  assert.strictEqual(requestPrFormOpen.has("req-fail-pr"), true);
  assert.deepStrictEqual(requestPrDrafts.get("req-fail-pr"), { title: "Keep me", body: "Keep body" });

  global.fetch = async (url) => {
    if (url === "/api/requests") {
      return {
        ok: false,
        status: 503,
        async json() { return { error: "requests unavailable" }; },
      };
    }
    throw new Error("unexpected fetch url: " + url);
  };
  await fetchRequests();
  assert.strictEqual(requestsError, "requests unavailable");
  const errPanelHtml = renderRequestsList(liveRequests, { error: requestsError });
  assert.ok(errPanelHtml.includes("requests unavailable"), errPanelHtml);
  assert.ok(!errPanelHtml.includes("No requests"), errPanelHtml);
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"#;
    run_surface_script(harness);
}

#[test]
fn get_requests_send_the_token_and_a_403_stops_polling() {
    let harness = r#"
const assert = require("node:assert");
(async () => {
  const missingTokenBar = { hidden: true };
  const lostServerBar = { hidden: true };
  const elements = {
    "missing-token": missingTokenBar,
    "lost-server": lostServerBar,
  };
  global.document = {
    getElementById(id) { return elements[id] || null; },
    querySelectorAll() { return []; },
  };
  const clearedIntervals = [];
  let nextTimerId = 100;
  global.setInterval = () => {
    nextTimerId += 1;
    return nextTimerId;
  };
  global.clearInterval = (id) => {
    clearedIntervals.push(id);
  };

  surfaceToken = "tok-abc-999";
  updateMissingTokenUI();
  assert.strictEqual(missingTokenBar.hidden, true);

  const seen = [];
  global.fetch = async (url, options) => {
    seen.push({ url, options });
    return {
      ok: true,
      status: 200,
      async json() { return {}; },
    };
  };

  for (const url of [
    "/api/session",
    "/api/runs",
    "/api/requests",
    "/api/map",
    "/api/models",
    "/api/runs/T1/diff",
  ]) {
    const res = await fetchWithRetry(url);
    assert.strictEqual(res.status, 200);
  }
  assert.strictEqual(seen.length, 6);
  for (const call of seen) {
    assert.ok(call.options && call.options.headers, "missing headers for " + call.url);
    assert.strictEqual(
      call.options.headers["X-Blyte-Token"],
      "tok-abc-999",
      "wrong token header on GET " + call.url
    );
  }

  runsPollTimer = setInterval(fetchRuns, RUNS_POLL_MS);
  requestsPollTimer = setInterval(fetchRequests, RUNS_POLL_MS);
  const runsTimerId = runsPollTimer;
  const reqsTimerId = requestsPollTimer;

  const pollCalls = [];
  let timeoutCalls = 0;
  const origSetTimeout = global.setTimeout;
  global.setTimeout = (fn, ms) => {
    timeoutCalls += 1;
    return origSetTimeout(fn, ms);
  };
  global.fetch = async (url, options) => {
    pollCalls.push({ url, options });
    return {
      ok: false,
      status: 403,
      async json() { return { error: "missing or invalid surface token" }; },
    };
  };

  await pollEvents();

  assert.strictEqual(pollCalls.length, 1);
  assert.ok(pollCalls[0].url.startsWith("/api/session/events?"));
  assert.strictEqual(pollCalls[0].options.headers["X-Blyte-Token"], "tok-abc-999");
  assert.strictEqual(timeoutCalls, 0);
  assert.strictEqual(pollingStopped, true);
  assert.strictEqual(runsPollTimer, null);
  assert.strictEqual(requestsPollTimer, null);
  assert.ok(clearedIntervals.includes(runsTimerId));
  assert.ok(clearedIntervals.includes(reqsTimerId));
  assert.strictEqual(surfaceToken, null);
  assert.strictEqual(missingTokenBar.hidden, false);

  await fetchRuns();
  await fetchRequests();
  assert.strictEqual(pollCalls.length, 1);
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"#;
    run_surface_script(harness);
}

#[test]
fn polling_restarts_when_a_token_arrives_after_a_403() {
    let harness = r##"
const assert = require("node:assert");
(async () => {
  const elements = {};
  function makeStubElement(id) {
    const attrs = new Map();
    const classes = new Set();
    const metricVal = { textContent: "" };
    return {
      id,
      textContent: "",
      innerHTML: "",
      hidden: false,
      disabled: false,
      placeholder: "",
      value: "",
      checked: false,
      dataset: {},
      style: {},
      scrollHeight: 0,
      scrollTop: 0,
      clientHeight: 100,
      children: [],
      get title() { return attrs.get("title") || ""; },
      set title(v) {
        if (v === undefined || v === null) attrs.delete("title");
        else attrs.set("title", String(v));
      },
      classList: {
        toggle(name, force) {
          if (force === undefined) {
            if (classes.has(name)) classes.delete(name);
            else classes.add(name);
          } else if (force) {
            classes.add(name);
          } else {
            classes.delete(name);
          }
        },
        add(name) { classes.add(name); },
        remove(name) { classes.delete(name); },
        contains(name) { return classes.has(name); },
      },
      addEventListener() {},
      setAttribute(name, val) { attrs.set(name, String(val)); },
      getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
      hasAttribute(name) { return attrs.has(name); },
      removeAttribute(name) { attrs.delete(name); },
      querySelector(sel) { return sel === ".metric-val" ? metricVal : null; },
      querySelectorAll() { return []; },
      appendChild(node) { this.children.push(node); },
      insertBefore(node, ref) {
        const idx = this.children.indexOf(ref);
        if (idx === -1) this.children.push(node);
        else this.children.splice(idx, 0, node);
      },
    };
  }
  global.document = {
    getElementById(id) {
      if (!elements[id]) elements[id] = makeStubElement(id);
      return elements[id];
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener() {},
    createElement() {
      return {
        _inner: "",
        set innerHTML(v) { this._inner = v; },
        get content() { return { firstElementChild: makeStubElement("tpl") }; },
      };
    },
  };

  const windowListeners = {};
  const storage = new Map();
  global.window = {
    location: { pathname: "/", search: "", hash: "" },
    sessionStorage: {
      getItem(k) { return storage.has(k) ? storage.get(k) : null; },
      setItem(k, v) { storage.set(k, String(v)); },
      removeItem(k) { storage.delete(k); },
    },
    history: {
      replaceState(_state, _title, nextUrl) {
        const idx = nextUrl.indexOf("#");
        global.window.location.hash = idx === -1 ? "" : nextUrl.slice(idx);
      },
    },
    addEventListener(type, fn) {
      if (!windowListeners[type]) windowListeners[type] = [];
      windowListeners[type].push(fn);
    },
  };

  let nextTimerId = 100;
  global.setInterval = () => {
    nextTimerId += 1;
    return nextTimerId;
  };
  global.clearInterval = () => {};

  function makeUndecidedRun(id) {
    return {
      key: id,
      id,
      repo: "demo",
      title: "Task " + id,
      task: "Do " + id,
      outcome: "verified",
      research: false,
      checks_recorded: true,
      started_at: 1000,
      finished_at: 1010,
      turns: 1,
      plan_file: null,
      plan_task: null,
      final_message: "done",
      group: "core",
      verdict: null,
      verdict_notes: null,
      report: null,
      cost_usd: 0.01,
      verdict_ts: null,
      card: "clean",
      flags: [],
      changed_files: ["src/lib.rs"],
      commits: [{ sha: "1234567890abcdef", subject: "feat: " + id }],
      hold: null,
      combined: false,
    };
  }

  let allowRuns = false;
  let runsFetches = 0;
  let pendingEventResolve = null;
  global.fetch = async (url) => {
    if (url === "/api/session") {
      return {
        ok: true,
        status: 200,
        async json() {
          return {
            session_id: "s1",
            repo: "demo",
            model: "m1",
            started_at: 1000,
            effort: null,
            effort_supported: false,
            busy: false,
            queued: 0,
            last_seq: 0,
          };
        },
      };
    }
    if (url === "/api/models") {
      return {
        ok: true,
        status: 200,
        async json() { return { models: [{ id: "m1", effort_supported: false }] }; },
      };
    }
    if (url === "/api/requests") {
      return {
        ok: true,
        status: 200,
        async json() { return []; },
      };
    }
    if (url === "/api/runs") {
      runsFetches += 1;
      if (!allowRuns) {
        return {
          ok: false,
          status: 403,
          async json() { return { error: "missing or invalid surface token" }; },
        };
      }
      return {
        ok: true,
        status: 200,
        async json() {
          return {
            generated_at: 1020,
            repo: "demo",
            running: [],
            runs: [makeUndecidedRun("T1")],
            skipped: [],
          };
        },
      };
    }
    if (url.startsWith("/api/session/events?")) {
      if (!allowRuns) {
        return {
          ok: false,
          status: 403,
          async json() { return { error: "missing or invalid surface token" }; },
        };
      }
      return await new Promise(resolve => {
        pendingEventResolve = resolve;
      });
    }
    throw new Error("unexpected fetch url: " + url);
  };

  async function flush() {
    for (let i = 0; i < 10; i++) {
      await Promise.resolve();
    }
  }

  await init();
  await flush();

  assert.strictEqual(runsPollTimer, null);
  assert.strictEqual(pollingStopped, true);
  assert.strictEqual(runsFetches, 1);

  allowRuns = true;
  global.window.location.hash = "#token=fresh-surface-token";
  assert.ok(windowListeners.hashchange && windowListeners.hashchange.length === 1);
  for (const fn of windowListeners.hashchange) {
    fn();
  }
  await flush();

  assert.notStrictEqual(runsPollTimer, null);
  assert.strictEqual(pollingStopped, false);
  assert.strictEqual(runsFetches, 2);
  assert.strictEqual(document.getElementById("mode-review").textContent, "Review (1)");

  stopAllPolling();
  if (pendingEventResolve) {
    pendingEventResolve({
      ok: true,
      status: 200,
      async json() { return { entries: [], last_seq: 0 }; },
    });
  }
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"##;
    run_surface_script(harness);
}

#[test]
fn starting_polling_twice_leaves_one_timer() {
    let harness = r#"
const assert = require("node:assert");
(async () => {
  const elements = {};
  function makeStubElement(id) {
    const attrs = new Map();
    const classes = new Set();
    const metricVal = { textContent: "" };
    return {
      id,
      textContent: "",
      innerHTML: "",
      hidden: false,
      disabled: false,
      dataset: {},
      style: {},
      scrollHeight: 0,
      scrollTop: 0,
      clientHeight: 100,
      children: [],
      get title() { return attrs.get("title") || ""; },
      set title(v) {
        if (v === undefined || v === null) attrs.delete("title");
        else attrs.set("title", String(v));
      },
      classList: {
        toggle(name, force) {
          if (force === undefined) {
            if (classes.has(name)) classes.delete(name);
            else classes.add(name);
          } else if (force) {
            classes.add(name);
          } else {
            classes.delete(name);
          }
        },
        add(name) { classes.add(name); },
        remove(name) { classes.delete(name); },
      },
      setAttribute(name, val) { attrs.set(name, String(val)); },
      getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
      hasAttribute(name) { return attrs.has(name); },
      removeAttribute(name) { attrs.delete(name); },
      querySelector(sel) { return sel === ".metric-val" ? metricVal : null; },
      querySelectorAll() { return []; },
    };
  }
  global.document = {
    getElementById(id) {
      if (!elements[id]) elements[id] = makeStubElement(id);
      return elements[id];
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
  };

  const intervalCalls = [];
  let nextTimerId = 500;
  global.setInterval = (fn, ms) => {
    nextTimerId += 1;
    intervalCalls.push({ id: nextTimerId, fn, ms });
    return nextTimerId;
  };
  global.clearInterval = () => {};

  let eventFetches = 0;
  const eventResolvers = [];
  global.fetch = async (url) => {
    if (url === "/api/runs") {
      return {
        ok: true,
        status: 200,
        async json() {
          return { generated_at: 1, repo: "demo", running: [], runs: [], skipped: [] };
        },
      };
    }
    if (url === "/api/requests") {
      return {
        ok: true,
        status: 200,
        async json() { return []; },
      };
    }
    if (url.startsWith("/api/session/events?")) {
      eventFetches += 1;
      return await new Promise(resolve => {
        eventResolvers.push(resolve);
      });
    }
    throw new Error("unexpected fetch url: " + url);
  };

  startPolling();
  startPolling();

  for (let i = 0; i < 10; i++) {
    await Promise.resolve();
  }

  const runsTimers = intervalCalls.filter(c => c.fn === fetchRuns);
  const requestsTimers = intervalCalls.filter(c => c.fn === fetchRequests);
  assert.strictEqual(runsTimers.length, 1);
  assert.strictEqual(requestsTimers.length, 1);
  assert.strictEqual(intervalCalls.length, 2);
  assert.strictEqual(eventFetches, 1);

  eventResolvers.shift()({
    ok: true,
    status: 200,
    async json() { return { entries: [], last_seq: 0 }; },
  });
  for (let i = 0; i < 10; i++) {
    await Promise.resolve();
  }
  assert.strictEqual(eventFetches, 2);

  stopAllPolling();
  while (eventResolvers.length > 0) {
    eventResolvers.shift()({
      ok: true,
      status: 200,
      async json() { return { entries: [], last_seq: 0 }; },
    });
  }
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"#;
    run_surface_script(harness);
}

#[test]
fn a_failed_runs_fetch_marks_the_review_count() {
    let harness = r#"
const assert = require("node:assert");
(async () => {
  const elements = {};
  function makeStubElement(id) {
    const attrs = new Map();
    const classes = new Set();
    const metricVal = { textContent: "" };
    return {
      id,
      textContent: "",
      innerHTML: "",
      hidden: false,
      disabled: false,
      dataset: {},
      style: {},
      children: [],
      get title() { return attrs.get("title") || ""; },
      set title(v) {
        if (v === undefined || v === null) attrs.delete("title");
        else attrs.set("title", String(v));
      },
      classList: {
        toggle(name, force) {
          if (force === undefined) {
            if (classes.has(name)) classes.delete(name);
            else classes.add(name);
          } else if (force) {
            classes.add(name);
          } else {
            classes.delete(name);
          }
        },
        add(name) { classes.add(name); },
        remove(name) { classes.delete(name); },
      },
      setAttribute(name, val) { attrs.set(name, String(val)); },
      getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
      hasAttribute(name) { return attrs.has(name); },
      removeAttribute(name) { attrs.delete(name); },
      querySelector(sel) { return sel === ".metric-val" ? metricVal : null; },
      querySelectorAll() { return []; },
    };
  }
  global.document = {
    getElementById(id) {
      if (!elements[id]) elements[id] = makeStubElement(id);
      return elements[id];
    },
    querySelector() { return null; },
    querySelectorAll() { return []; },
  };

  function makeUndecidedRun(id) {
    return {
      key: id,
      id,
      repo: "demo",
      title: "Task " + id,
      task: "Do " + id,
      outcome: "verified",
      research: false,
      checks_recorded: true,
      started_at: 1000,
      finished_at: 1010,
      turns: 1,
      plan_file: null,
      plan_task: null,
      final_message: "done",
      group: "core",
      verdict: null,
      verdict_notes: null,
      report: null,
      cost_usd: 0.01,
      verdict_ts: null,
      card: "clean",
      flags: [],
      changed_files: ["src/lib.rs"],
      commits: [{ sha: "1234567890abcdef", subject: "feat: " + id }],
      hold: null,
      combined: false,
    };
  }

  let failRuns = false;
  global.fetch = async (url) => {
    if (url !== "/api/runs") {
      throw new Error("unexpected fetch url: " + url);
    }
    if (failRuns) {
      return {
        ok: false,
        status: 500,
        async text() { return "runs index exploded"; },
        async json() { throw new SyntaxError("Unexpected token"); },
      };
    }
    return {
      ok: true,
      status: 200,
      async json() {
        return {
          generated_at: 1020,
          repo: "demo",
          running: [],
          runs: [makeUndecidedRun("T1"), makeUndecidedRun("T2")],
          skipped: [],
        };
      },
    };
  };

  const reviewTab = document.getElementById("mode-review");
  const queueError = document.getElementById("queue-error");

  await fetchRuns();
  assert.strictEqual(reviewTab.textContent, "Review (2)");
  assert.strictEqual(reviewTab.title, "");
  assert.strictEqual(reviewTab.hasAttribute("title"), false);
  assert.strictEqual(queueError.hidden, true);

  failRuns = true;
  await fetchRuns();
  assert.strictEqual(reviewTab.textContent, "Review (2) !");
  assert.ok(reviewTab.title.includes("runs index exploded"), reviewTab.title);
  assert.strictEqual(reviewTab.hasAttribute("title"), true);
  assert.strictEqual(queueError.hidden, false);
  assert.ok(queueError.textContent.includes("runs index exploded"), queueError.textContent);

  renderAll(true);
  assert.strictEqual(reviewTab.textContent, "Review (2) !");
  assert.ok(reviewTab.title.includes("runs index exploded"), reviewTab.title);

  failRuns = false;
  await fetchRuns();
  assert.strictEqual(reviewTab.textContent, "Review (2)");
  assert.strictEqual(reviewTab.title, "");
  assert.strictEqual(reviewTab.hasAttribute("title"), false);
  assert.strictEqual(queueError.hidden, true);
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"#;
    run_surface_script(harness);
}

#[test]
fn decision_response_requires_conversation_message() {
    let harness = r#"
const assert = require("node:assert");

assert.strictEqual(
  validateDecisionResponse({
    seq: 1,
    decision: {
      run: "T1",
      action: "send_back",
      note: "fix edge case",
      conversation_message: "Send back T1",
    },
  }),
  null
);
assert.strictEqual(
  validateDecisionResponse({
    seq: 2,
    decision: {
      run: "T1",
      action: "accept",
      note: null,
      conversation_message: null,
    },
  }),
  null
);
const missingErr = validateDecisionResponse({
  seq: 3,
  decision: {
    run: "T1",
    action: "accept",
    note: null,
  },
});
assert.strictEqual(
  missingErr,
  "decision response missing field decision.conversation_message"
);
"#;
    run_surface_script(harness);
}

fn review_queue_test_prelude() -> &'static str {
    r#"
const assert = require("node:assert");
const elements = {};
function makeStubElement(id) {
  const attrs = new Map();
  const classes = new Set();
  const metricVal = { textContent: "" };
  return {
    id,
    textContent: "",
    innerHTML: "",
    hidden: false,
    disabled: false,
    dataset: {},
    style: {},
    children: [],
    get title() { return attrs.get("title") || ""; },
    set title(v) {
      if (v === undefined || v === null) attrs.delete("title");
      else attrs.set("title", String(v));
    },
    classList: {
      toggle(name, force) {
        if (force === undefined) {
          if (classes.has(name)) classes.delete(name);
          else classes.add(name);
        } else if (force) {
          classes.add(name);
        } else {
          classes.delete(name);
        }
      },
      add(name) { classes.add(name); },
      remove(name) { classes.delete(name); },
    },
    setAttribute(name, val) { attrs.set(name, String(val)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    hasAttribute(name) { return attrs.has(name); },
    removeAttribute(name) { attrs.delete(name); },
    querySelector(sel) { return sel === ".metric-val" ? metricVal : null; },
    querySelectorAll() { return []; },
  };
}
global.document = {
  getElementById(id) {
    if (!elements[id]) elements[id] = makeStubElement(id);
    return elements[id];
  },
  querySelector() { return null; },
  querySelectorAll() { return []; },
};
function makeRun(id, overrides) {
  return Object.assign({
    key: id,
    id,
    repo: "demo",
    title: "Task " + id,
    task: "Do " + id,
    outcome: "verified",
    research: false,
    checks_recorded: true,
    combined: false,
    started_at: 1000,
    finished_at: 1010,
    turns: 1,
    plan_file: null,
    plan_task: null,
    final_message: "done",
    group: "core",
    verdict: null,
    verdict_notes: null,
    report: null,
    cost_usd: 0.01,
    verdict_ts: null,
    card: "clean",
    flags: [],
    changed_files: ["src/lib.rs"],
    commits: [{ sha: "1234567890abcdef", subject: "feat: " + id }],
    hold: null,
  }, overrides || {});
}
function stubRunsFetch(runsPayload) {
  global.fetch = async (url) => {
    if (url !== "/api/runs") {
      throw new Error("unexpected fetch url: " + url);
    }
    return {
      ok: true,
      status: 200,
      async json() {
        return {
          generated_at: 1020,
          repo: "demo",
          running: [],
          runs: runsPayload,
          skipped: [],
        };
      },
    };
  };
}
"#
}

#[test]
fn a_clean_combined_run_is_not_in_the_review_count_or_list() {
    let script = format!(
        r#"{}
(async () => {{
  stubRunsFetch([
    makeRun("C1", {{ combined: true, card: "clean", hold: null, verdict: null }}),
    makeRun("U1", {{ combined: false, card: "clean", hold: null, verdict: null }}),
  ]);
  await fetchRuns();
  switchMode("review");
  assert.strictEqual(document.getElementById("mode-review").textContent, "Review (1)");
  const queueHtml = document.getElementById("queue").innerHTML;
  assert.ok(queueHtml.includes("Task U1"), queueHtml);
  assert.ok(!queueHtml.includes("Task C1"), queueHtml);
  assert.ok(!queueHtml.includes('data-run-key="C1"'), queueHtml);
}})().catch(err => {{
  console.error(err);
  process.exit(1);
}});
"#,
        review_queue_test_prelude()
    );
    run_surface_script(&script);
}

#[test]
fn a_flagged_combined_run_stays_in_review() {
    let script = format!(
        r#"{}
(async () => {{
  stubRunsFetch([
    makeRun("F1", {{ combined: true, card: "exception", flags: ["lint warning"], hold: null, verdict: null }}),
  ]);
  await fetchRuns();
  switchMode("review");
  assert.strictEqual(document.getElementById("mode-review").textContent, "Review (1)");
  const queueHtml = document.getElementById("queue").innerHTML;
  assert.ok(queueHtml.includes("Task F1"), queueHtml);
  assert.ok(queueHtml.includes("lint warning"), queueHtml);
}})().catch(err => {{
  console.error(err);
  process.exit(1);
}});
"#,
        review_queue_test_prelude()
    );
    run_surface_script(&script);
}

#[test]
fn a_held_combined_run_stays_in_review() {
    let script = format!(
        r#"{}
(async () => {{
  stubRunsFetch([
    makeRun("H1", {{
      combined: true,
      card: "clean",
      hold: {{ note: "wait for reviewer", ts: 1015 }},
      verdict: null,
    }}),
  ]);
  await fetchRuns();
  switchMode("review");
  assert.strictEqual(document.getElementById("mode-review").textContent, "Review (1)");
  const queueHtml = document.getElementById("queue").innerHTML;
  const heldHeadingIdx = queueHtml.indexOf('<div class="held-heading">Held</div>');
  const cardIdx = queueHtml.indexOf("Task H1");
  assert.ok(heldHeadingIdx !== -1, queueHtml);
  assert.ok(cardIdx > heldHeadingIdx, queueHtml);
  assert.ok(queueHtml.includes("wait for reviewer"), queueHtml);
}})().catch(err => {{
  console.error(err);
  process.exit(1);
}});
"#,
        review_queue_test_prelude()
    );
    run_surface_script(&script);
}

#[test]
fn a_runs_payload_without_combined_is_an_error() {
    let script = format!(
        r#"{}
(async () => {{
  const missingCombined = makeRun("M1");
  delete missingCombined.combined;
  stubRunsFetch([missingCombined]);
  await fetchRuns();
  const queueError = document.getElementById("queue-error");
  assert.strictEqual(queueError.hidden, false);
  assert.ok(queueError.textContent.includes("combined"), queueError.textContent);
  const reviewTab = document.getElementById("mode-review");
  assert.ok(reviewTab.title.includes("combined"), reviewTab.title);

  const nonBoolCombined = makeRun("B1", {{ combined: "yes" }});
  const err = validateRuns({{
    generated_at: 1020,
    repo: "demo",
    running: [],
    runs: [nonBoolCombined],
    skipped: [],
  }});
  assert.ok(err && err.includes("combined"), String(err));
}})().catch(err => {{
  console.error(err);
  process.exit(1);
}});
"#,
        review_queue_test_prelude()
    );
    run_surface_script(&script);
}

#[test]
fn a_conflict_card_without_files_is_a_validation_error() {
    let harness = r#"
const assert = require("assert");

const html = renderRequestCard({
  request: "req-empty-conf",
  runs: ["T1"],
  state: "conflict",
  conflicting_run: "T1",
  conflicting_files: [],
  started_by: null,
});
assert.ok(html.includes("request-validation-error"), html);
assert.ok(html.includes("conflicting_files"), html);
assert.ok(!html.includes("No conflicting files listed."), html);
"#;
    run_surface_script(harness);
}

#[test]
fn a_check_failed_card_without_output_is_a_validation_error() {
    let harness = r#"
const assert = require("assert");

const emptyHtml = renderRequestCard({
  request: "req-empty-out",
  runs: ["T1"],
  state: "check_failed",
  failing_step: "cargo test",
  output_tail: "",
  started_by: null,
});
assert.ok(emptyHtml.includes("request-validation-error"), emptyHtml);
assert.ok(emptyHtml.includes("output_tail"), emptyHtml);

const missingHtml = renderRequestCard({
  request: "req-missing-out",
  runs: ["T1"],
  state: "check_failed",
  failing_step: "cargo test",
  started_by: null,
});
assert.ok(missingHtml.includes("request-validation-error"), missingHtml);
assert.ok(missingHtml.includes("output_tail"), missingHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn a_user_message_keeps_its_line_breaks() {
    let html = load_surface_html();
    let start = html
        .find(".msg-line.user {")
        .expect("surface page must define .msg-line.user CSS rule");
    let rest = &html[start..];
    let end = rest
        .find('}')
        .expect(".msg-line.user CSS rule must close with }");
    let rule = &rest[..=end];
    assert!(
        rule.contains("white-space: pre-wrap"),
        ".msg-line.user rule must contain white-space: pre-wrap:\n{rule}"
    );
}

#[test]
fn a_done_request_renders_done_without_actions_after_waiting_and_bogus_is_rejected() {
    let harness = r#"
const assert = require("assert");

const doneReq = {
  request: "req-done",
  runs: ["D1"],
  state: "done",
  branch: "blyte/req-done",
  commits: [{ sha: "abcdef1234567", subject: "feat: merged work" }],
  pushed: true,
  pull_request_url: null,
  started_by: null,
};

const waitingReq = {
  request: "req-waiting",
  runs: ["W1"],
  state: "waiting",
  waiting_reasons: ["W1: needs review"],
  started_by: null,
};

const doneHtml = renderRequestCard(doneReq);
assert.ok(doneHtml.includes(">Done</span>"), doneHtml);
assert.ok(doneHtml.includes("request-state-done"), doneHtml);
assert.ok(!doneHtml.includes("request-state-error"), doneHtml);
assert.ok(!doneHtml.includes("request-validation-error"), doneHtml);
assert.ok(!doneHtml.includes("failed"), doneHtml);
assert.ok(!doneHtml.includes("<button"), doneHtml);
assert.ok(!doneHtml.includes("data-request-push"), doneHtml);
assert.ok(!doneHtml.includes("data-request-open-pr"), doneHtml);

const listHtml = renderRequestsList([doneReq, waitingReq], {});
const waitingPos = listHtml.indexOf('data-request-id="req-waiting"');
const donePos = listHtml.indexOf('data-request-id="req-done"');
assert.ok(waitingPos !== -1, listHtml);
assert.ok(donePos !== -1, listHtml);
assert.ok(waitingPos < donePos, "expected waiting before done: " + listHtml);

const bogusHtml = renderRequestCard({
  request: "req-bogus",
  runs: ["B1"],
  state: "bogus",
  started_by: null,
});
assert.ok(bogusHtml.includes("request-validation-error"), bogusHtml);
assert.ok(bogusHtml.includes("unknown state: bogus"), bogusHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn page_sets_document_title_from_surface_name() {
    let html = include_str!("../assets/surface/index.html");
    assert!(
        html.contains("data.surface_name") && html.contains("document.title"),
        "expected page source to set document.title from surface_name"
    );
    let harness = r#"
const assert = require("assert");
const elements = Object.create(null);
global.document = {
  title: "blyte",
  getElementById(id) {
    if (!elements[id]) {
      elements[id] = {
        textContent: "",
        innerHTML: "",
        disabled: false,
        style: {},
        classList: { toggle() {}, add() {}, remove() {} },
        setAttribute() {},
      };
    }
    return elements[id];
  },
  querySelector() { return null; },
  querySelectorAll() { return []; },
};
applySessionState({
  session_id: "s1",
  surface_name: "demo-2",
  repo: "demo",
  started_at: 1000,
  model: "m1",
  effort: null,
  effort_supported: false,
  busy: false,
  queued: 0,
  last_seq: 0,
}, false);
assert.strictEqual(document.title, "demo-2 - blyte");
"#;
    run_surface_script(harness);
}

#[test]
fn peer_message_shows_sender_and_text() {
    let html = load_surface_html();
    let start = html
        .find(".msg-line.peer {")
        .expect("surface page must define .msg-line.peer CSS rule");
    let rest = &html[start..];
    let end = rest
        .find('}')
        .expect(".msg-line.peer CSS rule must close with }");
    let rule = &rest[..=end];
    assert!(
        rule.contains("white-space: pre-wrap"),
        ".msg-line.peer rule must contain white-space: pre-wrap:\n{rule}"
    );

    let harness = r#"
const assert = require("assert");

const html = renderEntry({
  seq: 10,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: "please expose **v2**\nwith line breaks",
  queued: false,
  held: false,
});
assert.ok(html.includes("msg-line peer"), html);
assert.ok(!html.includes("msg-line user"), html);
assert.ok(!html.includes("msg-line assistant"), html);
assert.ok(html.includes("from surface api (shop)"), html);
assert.ok(html.includes("please expose **v2**\nwith line breaks"), html);
assert.ok(!html.includes("<strong>"), html);
assert.ok(!html.includes("msg-queued-badge"), html);
assert.ok(!html.includes("msg-held-note"), html);
"#;
    run_surface_script(harness);
}

#[test]
fn peer_message_text_and_names_are_escaped() {
    let harness = r#"
const assert = require("assert");

const html = renderEntry({
  seq: 11,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "<script>bad</script>",
  from_repository: "<b>",
  text: "<img src=x onerror=1>",
  queued: false,
  held: false,
});
assert.ok(!html.includes("<img"), html);
assert.ok(!html.includes("<b>"), html);
assert.ok(!html.includes("<script>"), html);
assert.ok(html.includes("&lt;img src=x onerror=1&gt;"), html);
assert.ok(html.includes("&lt;b&gt;"), html);
assert.ok(html.includes("&lt;script&gt;bad&lt;/script&gt;"), html);
"#;
    run_surface_script(harness);
}

#[test]
fn held_peer_message_says_why() {
    let harness = r#"
const assert = require("assert");

const html = renderEntry({
  seq: 12,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: "waiting for user",
  queued: false,
  held: true,
});
assert.ok(
  html.includes(
    "held: this surface has run five turns from other surfaces since you last wrote. It runs after your next message."
  ),
  html
);
"#;
    run_surface_script(harness);
}

#[test]
fn queued_peer_message_shows_waiting() {
    let harness = r#"
const assert = require("assert");

const queuedUser = renderEntry({
  seq: 13,
  ts: "2026-01-01T00:00:00Z",
  kind: "user_message",
  text: "user text",
  reports: [],
  queued: true,
});
const queuedPeer = renderEntry({
  seq: 14,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: "peer text",
  queued: true,
  held: false,
});
assert.ok(queuedUser.includes('<span class="msg-queued-badge">queued</span>'), queuedUser);
assert.ok(queuedPeer.includes('<span class="msg-queued-badge">queued</span>'), queuedPeer);

const clearedPeer = renderEntry(
  {
    seq: 14,
    ts: "2026-01-01T00:00:00Z",
    kind: "peer_message",
    from_name: "api",
    from_repository: "shop",
    text: "peer text",
    queued: true,
    held: false,
  },
  { stillQueued: false }
);
assert.ok(!clearedPeer.includes("msg-queued-badge"), clearedPeer);
"#;
    run_surface_script(harness);
}

#[test]
fn malformed_peer_message_is_reported() {
    let harness = r#"
const assert = require("assert");

const missingFromName = renderEntry({
  seq: 15,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_repository: "shop",
  text: "hello",
  queued: false,
  held: false,
});
assert.ok(missingFromName.includes("entry-error"), missingFromName);
assert.ok(missingFromName.includes("from_name"), missingFromName);

const badFromRepo = renderEntry({
  seq: 16,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: 42,
  text: "hello",
  queued: false,
  held: false,
});
assert.ok(badFromRepo.includes("entry-error"), badFromRepo);
assert.ok(badFromRepo.includes("from_repository"), badFromRepo);

const badText = renderEntry({
  seq: 17,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: null,
  queued: false,
  held: false,
});
assert.ok(badText.includes("entry-error"), badText);
assert.ok(badText.includes("text"), badText);

const badQueued = renderEntry({
  seq: 18,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: "hello",
  queued: "no",
  held: false,
});
assert.ok(badQueued.includes("entry-error"), badQueued);
assert.ok(badQueued.includes("queued"), badQueued);

const missingHeld = renderEntry({
  seq: 19,
  ts: "2026-01-01T00:00:00Z",
  kind: "peer_message",
  from_name: "api",
  from_repository: "shop",
  text: "hello",
  queued: false,
});
assert.ok(missingHeld.includes("entry-error"), missingHeld);
assert.ok(missingHeld.includes("held"), missingHeld);
"#;
    run_surface_script(harness);
}

#[test]
fn request_card_shows_started_by() {
    let harness = r#"
const assert = require("assert");

const html = renderRequestCard({
  request: "req-peer",
  runs: ["T1"],
  state: "combining",
  started_by: "api",
});
assert.ok(!html.includes("request-validation-error"), html);
assert.ok(html.includes("started by surface api"), html);
"#;
    run_surface_script(harness);
}

#[test]
fn request_card_started_by_is_escaped() {
    let harness = r#"
const assert = require("assert");

const html = renderRequestCard({
  request: "req-xss",
  runs: ["T1"],
  state: "combining",
  started_by: "<img src=x onerror=1>",
});
assert.ok(!html.includes("request-validation-error"), html);
assert.ok(!html.includes("<img"), html);
assert.ok(html.includes("started by surface &lt;img src=x onerror=1&gt;"), html);
"#;
    run_surface_script(harness);
}

#[test]
fn request_card_without_started_by_shows_nothing() {
    let harness = r#"
const assert = require("assert");

const html = renderRequestCard({
  request: "req-local",
  runs: ["T1"],
  state: "combining",
  started_by: null,
});
assert.ok(!html.includes("request-validation-error"), html);
assert.ok(!html.includes("started by surface"), html);
assert.ok(!html.includes("request-started-by"), html);
"#;
    run_surface_script(harness);
}

#[test]
fn request_card_rejects_a_bad_started_by() {
    let harness = r#"
const assert = require("assert");

const withNumber = {
  request: "req-num",
  runs: ["T1"],
  state: "combining",
  started_by: 42,
};
const numberErrors = validateRequestCard(withNumber);
assert.ok(numberErrors.some(e => e.includes("started_by")), JSON.stringify(numberErrors));
const numberHtml = renderRequestCard(withNumber);
assert.ok(numberHtml.includes("request-validation-error"), numberHtml);
assert.ok(numberHtml.includes("started_by"), numberHtml);
assert.ok(!numberHtml.includes("started by surface"), numberHtml);

const withEmpty = {
  request: "req-empty",
  runs: ["T1"],
  state: "combining",
  started_by: "",
};
const emptyErrors = validateRequestCard(withEmpty);
assert.ok(emptyErrors.some(e => e.includes("started_by")), JSON.stringify(emptyErrors));
const emptyHtml = renderRequestCard(withEmpty);
assert.ok(emptyHtml.includes("request-validation-error"), emptyHtml);
assert.ok(emptyHtml.includes("started_by"), emptyHtml);
assert.ok(!emptyHtml.includes("started by surface"), emptyHtml);

const withMissing = {
  request: "req-missing",
  runs: ["T1"],
  state: "combining",
};
const missingErrors = validateRequestCard(withMissing);
assert.ok(missingErrors.some(e => e.includes("started_by")), JSON.stringify(missingErrors));
const missingHtml = renderRequestCard(withMissing);
assert.ok(missingHtml.includes("request-validation-error"), missingHtml);
assert.ok(missingHtml.includes("started_by"), missingHtml);
assert.ok(!missingHtml.includes("started by surface"), missingHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn publish_approval_card_renders_details_and_escapes_script_as_text() {
    let harness = r#"
const assert = require("assert");

const html = renderEntry({
  seq: 42,
  ts: "2026-01-01T00:00:00Z",
  kind: "approval_requested",
  id: "approval-pub-1",
  command: "publish req-42",
  reason: "Ship <script>alert(1)</script>",
  publish: {
    request: "req-42",
    remote_url: "https://github.com/example/repo.git",
    branch: "blyte/req-42",
    base: "main",
    commit: "abcdef1234567890abcdef1234567890abcdef12",
    commits: ["First <b>commit</b>", "Second commit"],
    title: "Ship <script>alert(1)</script>",
    body: "Body with <script>alert('xss')</script> and\nmultiple lines"
  }
});

assert.ok(html.includes("entry-approval-publish"), html);
assert.ok(html.includes("Publish request req-42?"), html);
assert.ok(!html.includes("Command outside the sandbox"), html);
assert.ok(html.includes("https://github.com/example/repo.git"), html);
assert.ok(html.includes("blyte/req-42"), html);
assert.ok(html.includes("main"), html);
assert.ok(html.includes("abcdef1"), html);
assert.ok(!html.includes("abcdef1234567890"), html);
assert.ok(html.includes("First &lt;b&gt;commit&lt;/b&gt;"), html);
assert.ok(html.includes("Second commit"), html);
assert.ok(html.includes("Ship &lt;script&gt;alert(1)&lt;/script&gt;"), html);
assert.ok(html.includes('<pre class="approval-publish-body">Body with &lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt; and\nmultiple lines</pre>'), html);
assert.ok(!html.includes("<script>"), html);
assert.ok(!html.includes("<b>"), html);
assert.ok(html.includes('data-approval-answer="yes"'), html);
assert.ok(html.includes('data-approval-answer="no"'), html);

const plainHtml = renderEntry({
  seq: 43,
  ts: "2026-01-01T00:00:00Z",
  kind: "approval_requested",
  id: "approval-cmd-1",
  command: "npm install",
  reason: "install deps"
});
assert.ok(plainHtml.includes("Command outside the sandbox"), plainHtml);
assert.ok(!plainHtml.includes("Publish request"), plainHtml);
"#;
    run_surface_script(harness);
}

#[test]
fn header_shows_not_a_git_repository_when_session_repository_is_false() {
    let html = load_surface_html();
    assert!(
        html.contains("id=\"no-repo-badge\""),
        "surface page must include the no-repo badge element next to repo-name"
    );
    let harness = format!(
        "{}\n{}",
        review_queue_test_prelude(),
        r#"
assert.strictEqual(
  renderNoRepoNotice(false),
  "not a git repository: runs edit this folder in place; combining and publishing are off"
);
assert.strictEqual(renderNoRepoNotice(true), "");
assert.strictEqual(renderNoRepoNotice(undefined), "");

applySessionState({
  session_id: "surface-1",
  surface_name: "notes",
  repo: "notes",
  repository: false,
  started_at: 1700000000,
  model: "gemini-4-neon",
  effort: null,
  effort_supported: false,
  busy: false,
  queued: 0,
  last_seq: 0,
}, true);
assert.strictEqual(elements["repo-name"].textContent, "notes");
assert.strictEqual(
  elements["no-repo-badge"].innerHTML,
  "not a git repository: runs edit this folder in place; combining and publishing are off"
);
assert.strictEqual(elements["no-repo-badge"].hidden, false);

applySessionState({
  session_id: "surface-1",
  surface_name: "notes",
  repo: "notes",
  repository: true,
  started_at: 1700000000,
  model: "gemini-4-neon",
  effort: null,
  effort_supported: false,
  busy: false,
  queued: 0,
  last_seq: 0,
}, true);
assert.strictEqual(elements["no-repo-badge"].innerHTML, "");
assert.strictEqual(elements["no-repo-badge"].hidden, true);
"#
    );
    run_surface_script(&harness);
}

#[test]
fn surface_page_has_an_activity_tab() {
    let html = load_surface_html();
    assert!(
        html.contains("id=\"mode-activity\""),
        "surface page must contain id=\"mode-activity\""
    );
    assert!(
        html.contains("id=\"activity\""),
        "surface page must contain id=\"activity\""
    );

    let harness = format!(
        "{}\n{}",
        review_queue_test_prelude(),
        r#"
const tabIds = ["mode-design", "mode-review", "mode-activity"];
const paneIds = ["conversation", "review-wrap", "activity"];

for (const id of tabIds) {
  const el = document.getElementById(id);
  const tracked = new Set();
  el.classList = {
    toggle(name, force) {
      if (force) tracked.add(name);
      else tracked.delete(name);
    },
    add(name) { tracked.add(name); },
    remove(name) { tracked.delete(name); },
    contains(name) { return tracked.has(name); },
  };
}

function assertOnlyActive(expectedTab, expectedPane) {
  for (const id of tabIds) {
    const el = document.getElementById(id);
    const isExpected = id === expectedTab;
    assert.strictEqual(el.classList.contains("active"), isExpected, id + " active class");
    assert.strictEqual(el.getAttribute("aria-selected"), isExpected ? "true" : "false", id + " aria-selected");
  }
  for (const id of paneIds) {
    const el = document.getElementById(id);
    assert.strictEqual(el.hidden, id !== expectedPane, id + " hidden");
  }
}

renderModeChrome();
assertOnlyActive("mode-design", "conversation");

switchMode("review");
assertOnlyActive("mode-review", "review-wrap");

switchMode("activity");
assertOnlyActive("mode-activity", "activity");

switchMode("design");
assertOnlyActive("mode-design", "conversation");
"#
    );
    run_surface_script(&harness);
}

#[test]
fn surface_page_summarises_activity_in_the_chat() {
    let html = load_surface_html();
    assert!(
        html.contains("activity-summary"),
        "surface page must contain activity-summary"
    );
    assert!(
        html.contains("function activitySummaryText("),
        "surface page must define activitySummaryText"
    );

    let harness = r#"
const assert = require("assert");
const ts = "2026-01-01T00:00:00Z";

assert.strictEqual(
  activitySummaryText([
    { seq: 1, ts, kind: "tool_started", name: "bash", input: "ls" },
    { seq: 2, ts, kind: "tool_finished", name: "bash", summary: "exit 0", is_error: false },
    { seq: 3, ts, kind: "tool_started", name: "bash", input: "cargo test" },
    { seq: 4, ts, kind: "tool_finished", name: "bash", summary: "exit 1", is_error: true },
    { seq: 5, ts, kind: "tool_started", name: "read", input: "src/lib.rs" },
    { seq: 6, ts, kind: "tool_finished", name: "read", summary: "ok", is_error: false },
  ]),
  "2 commands · 1 file read · 1 failed"
);

assert.strictEqual(
  activitySummaryText([
    { seq: 1, ts, kind: "tool_started", name: "bash", input: "echo hi" },
    { seq: 2, ts, kind: "tool_finished", name: "bash", summary: "exit 0", is_error: false },
    { seq: 3, ts, kind: "tool_started", name: "write", input: "a.rs" },
    { seq: 4, ts, kind: "tool_finished", name: "write", summary: "ok", is_error: false },
    { seq: 5, ts, kind: "tool_started", name: "edit", input: "b.rs" },
    { seq: 6, ts, kind: "tool_finished", name: "edit", summary: "ok", is_error: false },
    { seq: 7, ts, kind: "tool_started", name: "blyte", input: '{"args":["wait","--any","T1"]}' },
    { seq: 8, ts, kind: "tool_finished", name: "blyte", summary: "exit 2", is_error: true },
    { seq: 9, ts, kind: "tool_started", name: "blyte", input: '{"args":["status"]}' },
    { seq: 10, ts, kind: "tool_finished", name: "blyte", summary: "exit 0", is_error: false },
    { seq: 11, ts, kind: "tool_started", name: "custom_probe", input: "x" },
    { seq: 12, ts, kind: "tool_finished", name: "custom_probe", summary: "ok", is_error: false },
  ]),
  "1 command · 2 files changed · 2 blyte · 1 custom_probe"
);

assert.strictEqual(activitySummaryText([]), "");
"#;
    run_surface_script(harness);
}

#[test]
fn activity_tab_groups_model_work_and_chat_shows_live_and_final_summary() {
    let harness = r#"
const assert = require("assert");
const ts = "2026-01-01T12:34:56Z";

function makeNode(html) {
  const seqMatch = html.match(/data-seq="([^"]*)"/);
  const replyMatch = html.match(/data-activity-reply="([^"]*)"/);
  const statusMatch = html.match(/<span class="([^"]*tool-status[^"]*)">([^<]*)<\/span>/);
  const statusEl = statusMatch ? { className: statusMatch[1], textContent: statusMatch[2] } : null;
  const textMatch = html.match(/^<[^>]+>([^<]*)<\/[^>]+>$/);
  const classes = new Set();
  const classMatch = html.match(/class="([^"]*)"/);
  if (classMatch) {
    for (const c of classMatch[1].split(/\s+/)) {
      if (c) classes.add(c);
    }
  }
  return {
    html,
    seq: seqMatch ? seqMatch[1] : null,
    replyId: replyMatch ? replyMatch[1] : null,
    textContent: textMatch ? textMatch[1] : "",
    statusEl,
    scrolled: false,
    classList: {
      toggle(name, force) {
        if (force) classes.add(name);
        else classes.delete(name);
      },
      contains(name) { return classes.has(name); },
    },
    hasAttribute(name) {
      if (name === "data-seq") return this.seq !== null;
      if (name === "data-activity-reply") return this.replyId !== null;
      return false;
    },
    getAttribute(name) {
      if (name === "data-seq") return this.seq;
      if (name === "data-activity-reply") return this.replyId;
      return null;
    },
    querySelector(sel) {
      if (sel === ".tool-status") return this.statusEl;
      return null;
    },
    scrollIntoView() { this.scrolled = true; },
  };
}

function makePane(id) {
  const attrs = new Map();
  const classes = new Set();
  return {
    id,
    children: [],
    hidden: false,
    innerHTML: "",
    textContent: "",
    scrollHeight: 0,
    scrollTop: 0,
    clientHeight: 100,
    style: {},
    dataset: {},
    classList: {
      toggle(name, force) {
        if (force) classes.add(name);
        else classes.delete(name);
      },
      add(name) { classes.add(name); },
      remove(name) { classes.delete(name); },
      contains(name) { return classes.has(name); },
    },
    setAttribute(name, val) { attrs.set(name, String(val)); },
    getAttribute(name) { return attrs.has(name) ? attrs.get(name) : null; },
    appendChild(node) { this.children.push(node); },
    insertBefore(node, ref) {
      const idx = this.children.indexOf(ref);
      if (idx === -1) this.children.push(node);
      else this.children.splice(idx, 0, node);
    },
    querySelector(sel) {
      if (sel === ".thread-empty") return null;
      if (sel === ".metric-val") return { textContent: "" };
      const seqM = sel.match(/^\[data-seq="([^"]+)"\]$/);
      if (seqM) return this.children.find(c => c.seq === seqM[1]) || null;
      const replyM = sel.match(/^\[data-activity-reply="([^"]+)"\]$/);
      if (replyM) return this.children.find(c => c.replyId === replyM[1]) || null;
      return null;
    },
    querySelectorAll() { return []; },
  };
}

const elements = {};
global.document = {
  getElementById(id) {
    if (!elements[id]) elements[id] = makePane(id);
    return elements[id];
  },
  querySelector() { return null; },
  querySelectorAll() { return []; },
  createElement(tag) {
    if (tag === "template") {
      return {
        _inner: "",
        set innerHTML(v) { this._inner = v; },
        get content() { return { firstElementChild: makeNode(this._inner) }; },
      };
    }
    return makePane(tag);
  },
};

const threadEl = document.getElementById("conversation-thread");
const activityEl = document.getElementById("activity");

const longPrompt = "Investigate why the parser rejects nested brackets when running integration tests across every crate in the workspace";

applyLogEntries([
  { seq: 1, ts, kind: "user_message", text: longPrompt, reports: [], queued: false },
  { seq: 2, ts, kind: "turn", n: 1, text: "", input_tokens: 10, output_tokens: 5, cost_usd: 0.01 },
  { seq: 3, ts, kind: "tool_started", name: "bash", input: "cargo test -p parser" },
]);

const summaryBtn = threadEl.children.find(c => c.classList.contains("activity-summary"));
assert.ok(summaryBtn, "chat must show activity-summary button while running");
assert.strictEqual(summaryBtn.textContent, "working · bash cargo test -p parser");
assert.ok(summaryBtn.classList.contains("activity-summary-running"));

applyLogEntries([
  { seq: 4, ts, kind: "tool_finished", name: "bash", summary: "exit 1", is_error: true },
  { seq: 5, ts, kind: "tool_started", name: "read", input: "src/parser.rs" },
  { seq: 6, ts, kind: "tool_finished", name: "read", summary: "120 lines", is_error: false },
  { seq: 7, ts, kind: "tool_started", name: "bash", input: "cargo test" },
]);

assert.strictEqual(summaryBtn.textContent, "working · bash cargo test");

applyLogEntries([
  { seq: 8, ts, kind: "tool_finished", name: "bash", summary: "exit 0", is_error: false },
  { seq: 9, ts, kind: "retry", attempt: 1, max: 3, wait_ms: 1000, reason: "overloaded" },
  { seq: 10, ts, kind: "turn", n: 2, text: "Found and explained the issue.", input_tokens: 20, output_tokens: 15, cost_usd: 0.02 },
  { seq: 11, ts, kind: "idle", turns: 2, cost_usd: 0.03, seconds: 2.5 },
  { seq: 12, ts, kind: "model_changed", model: "claude-opus" },
  { seq: 13, ts, kind: "effort_changed", effort: "high" },
  { seq: 14, ts, kind: "decision", run: "T1", action: "accept", note: null },
]);

assert.strictEqual(summaryBtn.textContent, "2 commands · 1 file read · 1 failed");
assert.ok(!summaryBtn.classList.contains("activity-summary-running"));

const summaryIdx = threadEl.children.indexOf(summaryBtn);
const replyIdx = threadEl.children.findIndex(c => c.html.includes("Found and explained the issue."));
assert.ok(summaryIdx > 0 && replyIdx > summaryIdx, "activity-summary must appear before the reply text");

assert.ok(!threadEl.children.some(c => c.html.includes("entry-tool")));
assert.ok(!threadEl.children.some(c => c.html.includes("entry-retry")));
assert.ok(!threadEl.children.some(c => c.html.includes("conversation model · turn")));
assert.ok(!threadEl.children.some(c => c.html.includes("model changed to")));
assert.ok(!threadEl.children.some(c => c.html.includes("effort changed to")));
assert.ok(!threadEl.children.some(c => c.html.includes("decision on T1")));
assert.ok(threadEl.children.some(c => c.html.includes("2 turns · USD 0.03 · 2.5 s")));

const heading = activityEl.children.find(c => c.classList.contains("activity-heading"));
assert.ok(heading, "activity pane must group entries under a heading");
assert.ok(heading.html.includes(longPrompt.slice(0, 80)), heading.html);
assert.ok(heading.html.includes("activity-heading-time"), heading.html);
assert.ok(activityEl.children.some(c => c.html.includes("conversation model · turn 1")));
assert.ok(activityEl.children.some(c => c.html.includes("conversation model · turn 2")));
assert.ok(activityEl.children.some(c => c.html.includes("entry-tool")));
assert.ok(activityEl.children.some(c => c.html.includes("entry-retry")));
assert.ok(activityEl.children.some(c => c.html.includes("model changed to claude-opus")));
assert.ok(activityEl.children.some(c => c.html.includes("effort changed to high")));
assert.ok(activityEl.children.some(c => c.html.includes("decision on T1: accept")));

switchMode("activity", summaryBtn.replyId);
assert.strictEqual(elements["activity"].hidden, false);
assert.strictEqual(elements["conversation"].hidden, true);
assert.strictEqual(heading.scrolled, true);

applyLogEntries([
  { seq: 20, ts, kind: "user_message", text: "Thanks!", reports: [], queued: false },
  { seq: 21, ts, kind: "turn", n: 1, text: "You are welcome.", input_tokens: 5, output_tokens: 5, cost_usd: 0.0 },
  { seq: 22, ts, kind: "idle", turns: 1, cost_usd: 0.0, seconds: 0.4 },
]);
assert.strictEqual(
  threadEl.children.filter(c => c.classList.contains("activity-summary")).length,
  1
);
"#;
    run_surface_script(harness);
}

#[test]
fn undo_button_renders_only_for_finished_not_undone_in_place_run_with_changed_files() {
    let html = load_surface_html();
    assert!(
        !html.contains("window.confirm("),
        "undo confirmation must happen in the page, not via a browser dialog"
    );
    let harness = format!(
        "{}\n{}",
        review_queue_test_prelude(),
        r##"
function baseRun(overrides) {
  return Object.assign({
    key: "notes/T1",
    id: "T1",
    repo: "notes",
    title: "Edit notes",
    task: "edit notes",
    outcome: "finished",
    research: false,
    checks_recorded: true,
    combined: false,
    started_at: 1700000000,
    finished_at: 1700000060,
    turns: 2,
    cost_usd: 0.01,
    plan_file: null,
    plan_task: null,
    final_message: "done",
    group: null,
    verdict: null,
    verdict_notes: null,
    verdict_ts: null,
    report: null,
    card: "clean",
    flags: [],
    changed_files: ["notes.txt"],
    commits: [],
    hold: null,
  }, overrides);
}

const inPlaceClean = baseRun({ key: "notes/T1", id: "T1", in_place: true, undone: false });
const inPlaceException = baseRun({
  key: "notes/T2",
  id: "T2",
  card: "exception",
  outcome: "failed",
  flags: ["outcome failed: review in depth"],
  in_place: true,
  undone: false,
});
const inPlaceUndone = baseRun({ key: "notes/T3", id: "T3", in_place: true, undone: true });
const inPlaceNoChanges = baseRun({ key: "notes/T4", id: "T4", in_place: true, undone: false, changed_files: [] });
const inPlaceResearch = baseRun({
  key: "notes/R1",
  id: "R1",
  research: true,
  card: "research",
  report: "# Report",
  in_place: true,
  undone: false,
  changed_files: ["notes.txt"],
});
const repoRun = baseRun({ key: "repo/T5", id: "T5", changed_files: ["README.md"] });

const payloadErr = validateRuns({
  generated_at: 1700000100,
  repo: "notes",
  running: [],
  runs: [inPlaceClean, inPlaceException, inPlaceUndone, inPlaceNoChanges, inPlaceResearch, repoRun],
  skipped: [],
});
assert.strictEqual(payloadErr, null, "validateRuns should accept in_place and undone flags");

assert.ok(renderRunCard(inPlaceClean).includes('data-undo-run="notes/T1"'), "clean in-place run with changes gets Undo button");
assert.ok(renderRunCard(inPlaceException).includes('data-undo-run="notes/T2"'), "exception in-place run with changes gets Undo button");
assert.ok(!renderRunCard(inPlaceUndone).includes("data-undo-run"), "undone in-place run has no Undo button");
assert.ok(!renderRunCard(inPlaceNoChanges).includes("data-undo-run"), "in-place run without changed files has no Undo button");
assert.ok(!renderRunCard(inPlaceResearch).includes("data-undo-run"), "research run has no Undo button");
assert.ok(!renderRunCard(repoRun).includes("data-undo-run"), "repository run has no Undo button");

pendingUndoConfirm.add(inPlaceClean.key);
const confirmHtml = renderRunCard(inPlaceClean);
assert.ok(confirmHtml.includes('data-undo-confirm="notes/T1"'), "in-page undo confirmation should render");
assert.ok(confirmHtml.includes('data-confirm-undo="notes/T1"'), "confirm button should render");
assert.ok(confirmHtml.includes('data-cancel-undo="notes/T1"'), "cancel button should render");
pendingUndoConfirm.delete(inPlaceClean.key);

undoResults.set(inPlaceClean.key, { ok: true, message: "restored notes.txt" });
assert.ok(renderRunCard(inPlaceClean).includes("restored notes.txt"), "undo result lines should render");
undoResults.set(inPlaceClean.key, { ok: false, message: "changed since run T1 ended: notes.txt" });
assert.ok(renderRunCard(inPlaceClean).includes("changed since run T1 ended: notes.txt"), "undo refusal should render");
"##
    );
    run_surface_script(&harness);
}

#[test]
fn post_undo_records_error_and_closes_confirm_on_network_failure() {
    let harness = format!(
        "{}\n{}",
        review_queue_test_prelude(),
        r##"
(async () => {
  const inPlaceClean = {
    key: "notes/T1",
    id: "T1",
    repo: "notes",
    title: "Edit notes",
    task: "edit notes",
    outcome: "finished",
    research: false,
    checks_recorded: true,
    combined: false,
    started_at: 1700000000,
    finished_at: 1700000060,
    turns: 2,
    cost_usd: 0.01,
    plan_file: null,
    plan_task: null,
    final_message: "done",
    group: null,
    verdict: null,
    verdict_notes: null,
    verdict_ts: null,
    report: null,
    card: "clean",
    flags: [],
    changed_files: ["notes.txt"],
    commits: [],
    hold: null,
    in_place: true,
    undone: false,
  };
  liveRuns = {
    generated_at: 1700000100,
    repo: "notes",
    running: [],
    runs: [inPlaceClean],
    skipped: [],
  };
  surfaceToken = "test-token";
  pendingUndoConfirm.add(inPlaceClean.key);
  fetchWithRetry = async () => {
    throw new Error("network down");
  };
  const ok = await postUndo(inPlaceClean.key);
  assert.strictEqual(ok, false);
  assert.strictEqual(pendingUndoConfirm.has(inPlaceClean.key), false);
  assert.strictEqual(undoInFlight.has(inPlaceClean.key), false);
  const result = undoResults.get(inPlaceClean.key);
  assert.ok(result && result.ok === false, "undoResults should record error");
  assert.ok(result.message.includes("network down"), "message=" + result.message);
  assert.ok(renderRunCard(inPlaceClean).includes("network down"));
})().catch(err => {
  console.error(err);
  process.exit(1);
});
"##
    );
    run_surface_script(&harness);
}
