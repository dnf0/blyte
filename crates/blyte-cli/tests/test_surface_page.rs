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

const threadEl = {
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

global.document = {
  getElementById(id) { return id === "conversation-thread" ? threadEl : null; },
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
const liveWait = threadEl.querySelector('[data-seq="101"]');
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
assert.strictEqual(threadEl.querySelector('[data-seq="103"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(threadEl.querySelector('[data-seq="103"]').statusEl.textContent, "exit 1");
assert.strictEqual(threadEl.querySelector('[data-seq="105"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(threadEl.querySelector('[data-seq="105"]').statusEl.textContent, "exit 2");
assert.strictEqual(threadEl.querySelector('[data-seq="107"]').statusEl.className, "tool-status tool-error");
assert.strictEqual(threadEl.querySelector('[data-seq="107"]').statusEl.textContent, "exit 2");
assert.strictEqual(threadEl.querySelector('[data-seq="109"]').statusEl.className, "tool-status tool-waiting");
assert.strictEqual(threadEl.querySelector('[data-seq="109"]').statusEl.textContent, "still waiting");
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
});
assert.ok(missingBranchHtml.includes("branch"), missingBranchHtml);
assert.ok(missingBranchHtml.includes("request-validation-error"), missingBranchHtml);

const missingStepHtml = renderRequestCard({
  request: "req-bad-step",
  runs: ["T11"],
  state: "check_failed",
  output_tail: "boom",
});
assert.ok(missingStepHtml.includes("failing_step"), missingStepHtml);
assert.ok(missingStepHtml.includes("request-validation-error"), missingStepHtml);

const missingErrorHtml = renderRequestCard({
  request: "req-bad-err",
  runs: ["T12"],
  state: "error",
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
