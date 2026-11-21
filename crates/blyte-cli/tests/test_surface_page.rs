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
        "/api/models",
        "/api/session/model",
        "/api/session/effort",
        "/api/runs",
        "/decision",
        "/diff",
        "/api/map",
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
    let output = std::process::Command::new("node")
        .arg("-e")
        .arg(&combined)
        .output()
        .expect("node must be available to run surface script tests");
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
