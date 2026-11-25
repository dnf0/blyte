use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::SurfaceArgs;
use crate::http::{Request, Response};
use crate::queue_actions::{Action, Hold};
use crate::repl::ReplConfig;
use crate::requests::{Publisher, RealPublisher, RequestActionError, RequestsState};
use crate::session::{AgentRunner, AnswerError, ApprovalAnswer, Attachment, EntryBody, Session};

pub const TOKEN_HEADER: &str = "X-Blyte-Token";
pub const RECONCILE_SAFETY_NET_INTERVAL: Duration = Duration::from_secs(30);

const INDEX_HTML: &str = include_str!("../assets/surface/index.html");
const MAX_WAIT_MS: u64 = 25_000;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RECONCILE_POLL_WAIT: Duration = Duration::from_millis(50);
const TOKEN_BYTES: usize = 32;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct Surface {
    pub root: PathBuf,
    pub repo: String,
    pub session: Session,
    pub session_id: String,
    pub started_at: u64,
    pub days: u64,
    pub token: String,
    pub requests: Arc<RequestsState>,
    pub publisher: Arc<dyn Publisher>,
    pub reconcile_lock: Mutex<()>,
    pub reconcile_requested: AtomicBool,
}

pub fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| format!("failed to open /dev/urandom: {e}"))?;
    file.read_exact(&mut bytes)
        .map_err(|e| format!("failed to read /dev/urandom: {e}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

pub fn constant_time_eq(provided: &[u8], expected: &[u8]) -> bool {
    let mut diff = provided.len() ^ expected.len();
    for (i, &exp) in expected.iter().enumerate() {
        let prov = provided.get(i).copied().unwrap_or(0);
        diff |= usize::from(prov ^ exp);
    }
    std::hint::black_box(diff) == 0
}

fn has_valid_token(surface: &Surface, req: &Request) -> bool {
    if surface.token.is_empty() {
        return false;
    }
    let mut values = req.header_values(TOKEN_HEADER);
    let Some(provided) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    constant_time_eq(provided.as_bytes(), surface.token.as_bytes())
}

#[derive(Serialize, Deserialize)]
struct SurfaceFile {
    pid: u32,
    port: u16,
    session_id: String,
    started_at: u64,
}

#[derive(Serialize)]
struct RunWithHold<'a> {
    #[serde(flatten)]
    run: &'a crate::replay::Run,
    hold: Option<Hold>,
    combined: bool,
}

#[derive(Serialize)]
struct LiveRunsResponse<'a> {
    generated_at: u64,
    repo: &'a str,
    running: &'a [crate::live_runs::RunningRun],
    runs: Vec<RunWithHold<'a>>,
    skipped: &'a [crate::replay::Skipped],
}

fn json_error(status: u16, msg: impl Into<String>) -> Response {
    Response::json(status, &serde_json::json!({ "error": msg.into() }))
}

fn method_not_allowed(req: &Request) -> Response {
    json_error(
        405,
        format!("method {} not allowed for {}", req.method, req.path),
    )
}

fn is_valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn parse_json_object(body: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, Response> {
    let val: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| json_error(400, format!("invalid JSON body: {e}")))?;
    val.as_object()
        .cloned()
        .ok_or_else(|| json_error(400, "request body must be a JSON object"))
}

fn health_response(surface: &Surface) -> Response {
    Response::json(
        200,
        &serde_json::json!({ "session_id": surface.session_id }),
    )
}

fn session_response(surface: &Surface) -> Response {
    let info = surface.session.info();
    Response::json(
        200,
        &serde_json::json!({
            "session_id": surface.session_id,
            "repo": surface.repo,
            "started_at": surface.started_at,
            "model": info.model,
            "effort": info.effort,
            "effort_supported": info.effort_supported,
            "busy": info.busy,
            "queued": info.queued,
            "last_seq": info.last_seq,
        }),
    )
}

fn handle_events(surface: &Surface, req: &Request) -> Response {
    let after_raw = match req.query_value("after") {
        Some(v) => v,
        None => return json_error(400, "missing required query parameter: after"),
    };
    let after = match after_raw.parse::<u64>() {
        Ok(v) => v,
        Err(_) => {
            return json_error(
                400,
                format!("query parameter after must be a non-negative integer, got '{after_raw}'"),
            );
        }
    };
    let wait_ms = match req.query_value("wait_ms") {
        None => 0,
        Some(raw) => match raw.parse::<u64>() {
            Ok(v) if v <= MAX_WAIT_MS => v,
            Ok(v) => {
                return json_error(
                    400,
                    format!("query parameter wait_ms must be at most {MAX_WAIT_MS}, got {v}"),
                );
            }
            Err(_) => {
                return json_error(
                    400,
                    format!("query parameter wait_ms must be a non-negative integer, got '{raw}'"),
                );
            }
        },
    };
    let entries = surface
        .session
        .entries_after(after, Duration::from_millis(wait_ms));
    let last_seq = surface.session.info().last_seq;
    Response::json(
        200,
        &serde_json::json!({
            "entries": entries,
            "last_seq": last_seq,
        }),
    )
}

fn handle_post_message(surface: &Surface, req: &Request) -> Response {
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let text = match obj.get("text") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(_) => return json_error(400, "field text must be a string"),
        None => return json_error(400, "missing required field: text"),
    };
    let report_ids: Vec<String> = match obj.get("reports") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(arr)) => {
            let mut ids = Vec::with_capacity(arr.len());
            for item in arr {
                match item.as_str() {
                    Some(s) => ids.push(s.to_string()),
                    None => return json_error(400, "field reports must be an array of strings"),
                }
            }
            ids
        }
        Some(_) => return json_error(400, "field reports must be an array of strings"),
    };

    let mut attachments = Vec::with_capacity(report_ids.len());
    for id in report_ids {
        let path = surface.root.join(".wt").join(format!("{id}.report.md"));
        if !is_valid_run_id(&id) {
            return json_error(
                400,
                format!("invalid report run id '{id}' (path: {})", path.display()),
            );
        }
        match std::fs::read_to_string(&path) {
            Ok(body) => attachments.push(Attachment {
                run: id,
                text: body,
            }),
            Err(e) => {
                return json_error(
                    400,
                    format!(
                        "cannot read report for run '{id}' at {}: {e}",
                        path.display()
                    ),
                );
            }
        }
    }

    match surface.session.submit(&text, attachments) {
        Ok(seq) => Response::json(200, &serde_json::json!({ "seq": seq })),
        Err(e) => json_error(400, e),
    }
}

fn handle_models() -> Response {
    let discovered = blyte_provider::discover_all_models();
    let mut models = Vec::new();
    let mut unavailable = Vec::new();
    let mut error_parts = Vec::new();

    for (provider, result) in discovered {
        match result {
            Ok(list) => {
                for m in list {
                    let effort_supported = crate::session::model_supports_effort(&m.id);
                    models.push(serde_json::json!({
                        "id": m.id,
                        "effort_supported": effort_supported,
                    }));
                }
            }
            Err(e) => {
                let err_text = e.to_string();
                error_parts.push(format!("{provider}: {err_text}"));
                unavailable.push(serde_json::json!({
                    "provider": provider,
                    "error": err_text,
                }));
            }
        }
    }

    if models.is_empty() {
        let msg = if error_parts.is_empty() {
            "no models listed by any provider".to_string()
        } else {
            format!("no models available: {}", error_parts.join("; "))
        };
        return json_error(502, msg);
    }

    Response::json(
        200,
        &serde_json::json!({
            "models": models,
            "unavailable": unavailable,
        }),
    )
}

fn handle_post_model(surface: &Surface, req: &Request) -> Response {
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let model = match obj.get("model") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(_) => return json_error(400, "field model must be a string"),
        None => return json_error(400, "missing required field: model"),
    };
    if model.trim().is_empty() {
        return json_error(400, "field model must not be empty");
    }
    if let Err(e) = blyte_provider::resolve_provider(Some(&model)) {
        return json_error(400, e.to_string());
    }
    if let Err(e) = surface.session.set_model(&model) {
        return json_error(400, e);
    }
    session_response(surface)
}

fn handle_post_effort(surface: &Surface, req: &Request) -> Response {
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let effort = match obj.get("effort") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return json_error(400, "field effort must be a string or null"),
        None => return json_error(400, "missing required field: effort"),
    };
    if let Err(e) = surface.session.set_effort(effort.as_deref()) {
        return json_error(400, e);
    }
    session_response(surface)
}

fn handle_post_approval(surface: &Surface, id: &str, req: &Request) -> Response {
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    if obj.len() != 1 {
        return json_error(400, "request body must contain only field answer");
    }
    let answer = match obj.get("answer") {
        Some(serde_json::Value::String(s)) if s == "yes" => ApprovalAnswer::Yes,
        Some(serde_json::Value::String(s)) if s == "no" => ApprovalAnswer::No,
        Some(serde_json::Value::String(other)) => {
            return json_error(
                400,
                format!("field answer must be \"yes\" or \"no\", got '{other}'"),
            );
        }
        Some(_) => return json_error(400, "field answer must be \"yes\" or \"no\""),
        None => return json_error(400, "missing required field: answer"),
    };
    match surface.session.answer_approval(id, answer) {
        Ok(seq) => Response::json(
            200,
            &serde_json::json!({
                "id": id,
                "answer": answer.as_str(),
                "seq": seq,
            }),
        ),
        Err(AnswerError::NotFound) => json_error(404, format!("unknown approval id: {id}")),
        Err(AnswerError::AlreadyAnswered) => {
            json_error(409, format!("approval {id} has already been answered"))
        }
    }
}

fn reconcile_runs(
    surface: &Surface,
    now: u64,
) -> Result<(crate::live_runs::LiveRuns, Vec<String>), String> {
    let _guard = surface
        .reconcile_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut live = crate::live_runs::collect(&surface.root, now, surface.days)?;
    let mut any_written = false;
    for run in &live.runs {
        if run.checks_recorded {
            continue;
        }
        if crate::queue_actions::ensure_checks(&surface.root, &run.id)? {
            any_written = true;
        }
    }
    if any_written {
        live = crate::live_runs::collect(&surface.root, now, surface.days)?;
    }

    let mut errors = Vec::new();
    if let Err(e) = crate::run_results::report_finished_runs(surface) {
        errors.push(format!("run report failed: {e}"));
    }

    let mut triggered = BTreeSet::new();
    for run in &live.runs {
        if run.research {
            continue;
        }
        if let Some(ref req_id) = run.request {
            if triggered.insert(req_id.clone()) {
                if let Err(e) = crate::requests::maybe_combine(surface, req_id) {
                    errors.push(format!("combine trigger failed for {req_id}: {e}"));
                }
            }
        }
    }

    Ok((live, errors))
}

fn handle_runs(surface: &Surface, now: u64) -> Response {
    let (live, errors) = match reconcile_runs(surface, now) {
        Ok(res) => res,
        Err(e) => return json_error(500, e),
    };
    for msg in errors {
        surface.session.record_error(&msg);
    }

    let run_refs: Vec<(&str, Option<&str>)> = live
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run.request.as_deref()))
        .collect();
    let combined_ids = match crate::requests::combined_run_ids(&surface.root, &run_refs) {
        Ok(ids) => ids,
        Err(e) => return json_error(500, e),
    };

    let mut runs_with_hold = Vec::with_capacity(live.runs.len());
    for run in &live.runs {
        let hold = match crate::queue_actions::read_hold(&surface.root, &run.id) {
            Ok(h) => h,
            Err(e) => return json_error(500, e),
        };
        let combined = combined_ids.contains(&run.id);
        runs_with_hold.push(RunWithHold {
            run,
            hold,
            combined,
        });
    }

    let payload = LiveRunsResponse {
        generated_at: live.generated_at,
        repo: &live.repo,
        running: &live.running,
        runs: runs_with_hold,
        skipped: &live.skipped,
    };
    match serde_json::to_value(&payload) {
        Ok(val) => Response::json(200, &val),
        Err(e) => json_error(500, format!("failed to serialize runs: {e}")),
    }
}

fn handle_runs_changed(surface: &Surface) -> Response {
    surface.reconcile_requested.store(true, Ordering::Release);
    Response::json(202, &serde_json::json!({}))
}

fn handle_run_diff(surface: &Surface, id: &str) -> Response {
    if !is_valid_run_id(id) {
        return json_error(404, format!("unknown run: {id}"));
    }
    let wt_dir = surface.root.join(".wt");
    let wt = wt_dir.join(id);
    let summary = wt_dir.join(format!("{id}.json"));
    let report_json = wt_dir.join(format!("{id}.report.json"));
    let report_md = wt_dir.join(format!("{id}.report.md"));
    if !wt.exists() && !summary.exists() && !report_json.exists() && !report_md.exists() {
        return json_error(404, format!("unknown run: {id}"));
    }
    match crate::queue_actions::compute_review(&surface.root, id, true) {
        Ok(data) => {
            let body = if !data.wt_exists {
                "(worktree removed)\n".to_string()
            } else if data.diff_out.is_empty() {
                String::new()
            } else {
                format!("{}\n", data.diff_out)
            };
            Response::text(200, &body)
        }
        Err(e) => {
            if !wt.is_dir() || !wt.join(".git").exists() {
                json_error(404, e)
            } else {
                json_error(500, e)
            }
        }
    }
}

fn parse_action(raw: &str) -> Result<Action, String> {
    match raw {
        "accept" => Ok(Action::Accept),
        "send_back" => Ok(Action::SendBack),
        "redo" => Ok(Action::Redo),
        "hold" => Ok(Action::Hold),
        "mark_read" => Ok(Action::MarkRead),
        other => Err(format!(
            "unknown action '{other}'; must be one of accept, send_back, redo, hold, mark_read"
        )),
    }
}

fn handle_run_decision(surface: &Surface, id: &str, req: &Request, now: u64) -> Response {
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let action_str = match obj.get("action") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(_) => return json_error(400, "field action must be a string"),
        None => return json_error(400, "missing required field: action"),
    };
    let action = match parse_action(&action_str) {
        Ok(a) => a,
        Err(e) => return json_error(400, e),
    };
    let note = match obj.get("note") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return json_error(400, "field note must be a string or null"),
    };

    let decision =
        match crate::queue_actions::decide(&surface.root, id, action, note.as_deref(), now) {
            Ok(d) => d,
            Err(e) => return json_error(400, e),
        };
    let seq = match surface.session.record_decision(
        &decision.run,
        &action_str,
        decision.note.as_deref(),
        decision.conversation_message.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => return json_error(500, e),
    };
    if action == Action::Accept {
        match crate::requests::run_request_id(&surface.root, &decision.run) {
            Ok(Some(req_id)) => {
                if let Err(e) = crate::requests::maybe_combine(surface, &req_id) {
                    surface
                        .session
                        .record_error(&format!("combine trigger failed for {req_id}: {e}"));
                }
            }
            Ok(None) => {}
            Err(e) => return json_error(500, e),
        }
    }
    Response::json(
        200,
        &serde_json::json!({
            "decision": {
                "run": decision.run,
                "action": action_str,
                "note": decision.note,
                "conversation_message": decision.conversation_message,
            },
            "seq": seq,
        }),
    )
}

fn handle_map(surface: &Surface) -> Response {
    match crate::map::build(&surface.root, &surface.repo) {
        Ok(data) => match serde_json::to_value(&data) {
            Ok(val) => Response::json(200, &val),
            Err(e) => json_error(500, format!("failed to serialize map: {e}")),
        },
        Err(e) => json_error(422, e),
    }
}

fn handle_requests(surface: &Surface) -> Response {
    match crate::requests::list_requests(surface) {
        Ok(list) => match serde_json::to_value(&list) {
            Ok(val) => Response::json(200, &val),
            Err(e) => json_error(500, format!("failed to serialize requests: {e}")),
        },
        Err(e) => json_error(500, e),
    }
}

fn request_action_error_response(err: RequestActionError) -> Response {
    match err {
        RequestActionError::BadRequest(msg) => json_error(400, msg),
        RequestActionError::Conflict(msg) => json_error(409, msg),
        RequestActionError::Internal(msg) => json_error(500, msg),
    }
}

fn handle_request_push(surface: &Surface, id: &str) -> Response {
    match crate::requests::push_request(surface, id) {
        Ok(()) => Response::json(200, &serde_json::json!({ "pushed": true })),
        Err(err) => request_action_error_response(err),
    }
}

fn handle_request_pull_request(surface: &Surface, id: &str, req: &Request) -> Response {
    if let Err(e) = crate::dispatch::validate_request_id(id) {
        return json_error(400, e);
    }
    let obj = match parse_json_object(&req.body) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let title = match obj.get("title") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(serde_json::Value::String(_)) => {
            return json_error(400, "field title must not be empty");
        }
        Some(_) => return json_error(400, "field title must be a string"),
        None => return json_error(400, "missing required field: title"),
    };
    let body = match obj.get("body") {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(_) => return json_error(400, "field body must be a string"),
    };
    match crate::requests::open_request_pull_request(surface, id, &title, &body) {
        Ok(url) => Response::json(200, &serde_json::json!({ "url": url })),
        Err(err) => request_action_error_response(err),
    }
}

#[derive(Clone, Copy)]
enum ApiGetHandler {
    Session,
    Events,
    Models,
    Runs,
    Requests,
    Map,
    RunDiff,
}

const API_GET_ROUTES: &[(&str, ApiGetHandler)] = &[
    ("/api/session", ApiGetHandler::Session),
    ("/api/session/events", ApiGetHandler::Events),
    ("/api/models", ApiGetHandler::Models),
    ("/api/runs", ApiGetHandler::Runs),
    ("/api/requests", ApiGetHandler::Requests),
    ("/api/map", ApiGetHandler::Map),
    ("/api/runs/{id}/diff", ApiGetHandler::RunDiff),
];

pub fn authenticated_api_get_routes() -> Vec<&'static str> {
    API_GET_ROUTES.iter().map(|(pattern, _)| *pattern).collect()
}

fn match_api_get_route<'a>(pattern: &str, path: &'a str) -> Option<Option<&'a str>> {
    if let Some((prefix, suffix)) = pattern.split_once("{id}") {
        let rest = path.strip_prefix(prefix)?;
        let id = rest.strip_suffix(suffix)?;
        if !id.is_empty() && !id.contains('/') {
            return Some(Some(id));
        }
        None
    } else if pattern == path {
        Some(None)
    } else {
        None
    }
}

pub fn route(surface: &Surface, req: &Request, now: u64) -> Response {
    let is_health_get = req.path == "/api/health" && req.method == "GET";
    if (req.path.starts_with("/api/") || req.method == "POST")
        && !is_health_get
        && !has_valid_token(surface, req)
    {
        return json_error(
            403,
            "missing or invalid surface token; open the page from the URL blyte printed",
        );
    }

    if let Some((status, content_type, body)) = crate::dashboard::shared_asset(&req.path) {
        if req.method != "GET" {
            return method_not_allowed(req);
        }
        return Response {
            status,
            content_type,
            body,
        };
    }

    if req.path == "/" || req.path == "/index.html" {
        if req.method != "GET" {
            return method_not_allowed(req);
        }
        return Response::html(INDEX_HTML.as_bytes().to_vec());
    }

    if req.path == "/api/health" {
        if req.method != "GET" {
            return method_not_allowed(req);
        }
        return health_response(surface);
    }

    for &(pattern, handler) in API_GET_ROUTES {
        if let Some(id) = match_api_get_route(pattern, &req.path) {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            return match handler {
                ApiGetHandler::Session => session_response(surface),
                ApiGetHandler::Events => handle_events(surface, req),
                ApiGetHandler::Models => handle_models(),
                ApiGetHandler::Runs => handle_runs(surface, now),
                ApiGetHandler::Requests => handle_requests(surface),
                ApiGetHandler::Map => handle_map(surface),
                ApiGetHandler::RunDiff => handle_run_diff(surface, id.unwrap_or("")),
            };
        }
    }

    match req.path.as_str() {
        "/api/session/messages" => {
            if req.method != "POST" {
                return method_not_allowed(req);
            }
            handle_post_message(surface, req)
        }
        "/api/session/model" => {
            if req.method != "POST" {
                return method_not_allowed(req);
            }
            handle_post_model(surface, req)
        }
        "/api/session/effort" => {
            if req.method != "POST" {
                return method_not_allowed(req);
            }
            handle_post_effort(surface, req)
        }
        "/api/runs/changed" => {
            if req.method != "POST" {
                return method_not_allowed(req);
            }
            handle_runs_changed(surface)
        }
        path => {
            if let Some(id) = path.strip_prefix("/api/session/approvals/") {
                if !id.is_empty() && !id.contains('/') {
                    if req.method != "POST" {
                        return method_not_allowed(req);
                    }
                    return handle_post_approval(surface, id, req);
                }
            }
            if let Some(rest) = path.strip_prefix("/api/runs/") {
                if let Some(id) = rest.strip_suffix("/decision") {
                    if !id.is_empty() && !id.contains('/') {
                        if req.method != "POST" {
                            return method_not_allowed(req);
                        }
                        return handle_run_decision(surface, id, req, now);
                    }
                }
            }
            if let Some(rest) = path.strip_prefix("/api/requests/") {
                if let Some(id) = rest.strip_suffix("/push") {
                    if req.method != "POST" {
                        return method_not_allowed(req);
                    }
                    return handle_request_push(surface, id);
                }
                if let Some(id) = rest.strip_suffix("/pull-request") {
                    if req.method != "POST" {
                        return method_not_allowed(req);
                    }
                    return handle_request_pull_request(surface, id, req);
                }
            }
            json_error(404, format!("not found: {}", req.path))
        }
    }
}

fn find_git_toplevel(dir: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git rev-parse --show-toplevel: {e}"))?;
    if !out.status.success() {
        return Err(format!("not inside a git repository ({})", dir.display()));
    }
    let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path_str.is_empty() {
        return Err(format!("not inside a git repository ({})", dir.display()));
    }
    Ok(PathBuf::from(path_str))
}

fn repo_name_for(root: &Path) -> Result<String, String> {
    root.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("cannot determine repository name for {}", root.display()))
}

fn now_duration() -> Result<Duration, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock is before Unix epoch: {e}"))
}

fn new_session_id(nanos: u128) -> String {
    let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("surface-{}-{nanos}-{seq}", std::process::id())
}

fn probe_existing_surface(port: u16, expected_session_id: &str) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
    let req =
        format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut raw = Vec::new();
    if stream.read_to_end(&mut raw).is_err() {
        return false;
    }
    let text = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some(parts) => parts,
        None => return false,
    };
    let status_line = head.lines().next().unwrap_or("");
    if !(status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200")) {
        return false;
    }
    let val: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    val.get("session_id").and_then(|v| v.as_str()) == Some(expected_session_id)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";
    if let Err(e) = Command::new(cmd).arg(url).spawn() {
        eprintln!("blyte: could not open a browser with {cmd}: {e}; open {url} yourself");
    }
}

fn resolve_initial_model(config: &ReplConfig) -> String {
    if let Some(m) = config.model.as_deref().filter(|s| !s.trim().is_empty()) {
        return m.to_string();
    }
    if let Ok(m) = std::env::var("BLYTE_MODEL") {
        if !m.trim().is_empty() {
            return m;
        }
    }
    if let Ok(p) = blyte_provider::resolve_provider(None) {
        return p.model_id();
    }
    "gemini-3.8-flash-high".to_string()
}

#[cfg(unix)]
static SIGNAL_STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_shutdown_signal(_: libc::c_int) {
    SIGNAL_STOP.store(true, Ordering::Relaxed);
}

fn write_token_file(wt_dir: &Path, token_file_path: &Path, token: &str) -> Result<(), String> {
    let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = wt_dir.join(format!(".surface.token.{}.{seq}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&tmp_path)
        .map_err(|e| format!("cannot create {}: {e}", token_file_path.display()))?;
    if let Err(e) = writeln!(file, "{token}") {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("cannot write {}: {e}", token_file_path.display()));
    }
    drop(file);
    if let Err(e) = std::fs::rename(&tmp_path, token_file_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("cannot write {}: {e}", token_file_path.display()));
    }
    Ok(())
}

fn read_token_file(token_file_path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(token_file_path)
        .map_err(|e| format!("cannot read {}: {e}", token_file_path.display()))?;
    let token = raw.trim().to_string();
    if token.is_empty() {
        return Err(format!("{} is empty", token_file_path.display()));
    }
    Ok(token)
}

pub fn run(args: &SurfaceArgs, config: ReplConfig) -> i32 {
    #[cfg(unix)]
    {
        SIGNAL_STOP.store(false, Ordering::Relaxed);
        unsafe {
            libc::signal(
                libc::SIGINT,
                handle_shutdown_signal as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                handle_shutdown_signal as *const () as libc::sighandler_t,
            );
        }
    }
    run_inner(
        args,
        config,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &mut open_browser,
        || {
            #[cfg(unix)]
            {
                SIGNAL_STOP.load(Ordering::Relaxed)
            }
            #[cfg(not(unix))]
            {
                false
            }
        },
    )
}

pub fn run_with_io(
    args: &SurfaceArgs,
    config: ReplConfig,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    run_inner(args, config, out, err, &mut open_browser, || false)
}

pub fn run_with_hooks(
    args: &SurfaceArgs,
    config: ReplConfig,
    out: &mut dyn Write,
    err: &mut dyn Write,
    opener: &mut dyn FnMut(&str),
    stop: Arc<AtomicBool>,
) -> i32 {
    run_inner(args, config, out, err, opener, move || {
        stop.load(Ordering::Relaxed)
    })
}

struct ReconcileTimerGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ReconcileTimerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_reconcile_timer(surface: Arc<Surface>) -> ReconcileTimerGuard {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut after = surface.session.info().last_seq;
        let mut next_tick = Instant::now();
        let mut seen_errors: HashSet<String> = HashSet::new();
        while !thread_stop.load(Ordering::Acquire) {
            let wait = if surface.reconcile_requested.load(Ordering::Acquire) {
                Duration::ZERO
            } else {
                next_tick
                    .saturating_duration_since(Instant::now())
                    .min(RECONCILE_POLL_WAIT)
            };
            let entries = surface.session.entries_after(after, wait);
            let mut went_idle = false;
            for entry in entries {
                after = entry.seq;
                if matches!(entry.body, EntryBody::Idle { .. }) {
                    went_idle = true;
                }
            }
            if thread_stop.load(Ordering::Acquire) {
                break;
            }
            let requested = surface.reconcile_requested.swap(false, Ordering::AcqRel);
            if went_idle || requested || Instant::now() >= next_tick {
                match now_duration().and_then(|d| reconcile_runs(&surface, d.as_secs())) {
                    Ok((_, errors)) => {
                        for msg in errors {
                            if seen_errors.insert(msg.clone()) {
                                surface.session.record_error(&msg);
                                eprintln!("blyte: {msg}");
                            }
                        }
                    }
                    Err(e) => {
                        if seen_errors.insert(e.clone()) {
                            eprintln!("blyte: {e}");
                        }
                    }
                }
                next_tick = Instant::now() + RECONCILE_SAFETY_NET_INTERVAL;
            }
        }
    });
    ReconcileTimerGuard {
        stop,
        handle: Some(handle),
    }
}

fn run_inner<S>(
    args: &SurfaceArgs,
    mut config: ReplConfig,
    out: &mut dyn Write,
    err: &mut dyn Write,
    opener: &mut dyn FnMut(&str),
    should_stop: S,
) -> i32
where
    S: Fn() -> bool,
{
    let root = match find_git_toplevel(&config.repo_root) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };
    let repo = match repo_name_for(&root) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    let wt_dir = root.join(".wt");
    let surface_file_path = wt_dir.join("surface.json");
    let token_file_path = wt_dir.join("surface.token");
    match std::fs::read_to_string(&surface_file_path) {
        Ok(raw) => match serde_json::from_str::<SurfaceFile>(&raw) {
            Ok(existing) => {
                if probe_existing_surface(existing.port, &existing.session_id) {
                    let token = match read_token_file(&token_file_path) {
                        Ok(t) => t,
                        Err(e) => {
                            let _ = writeln!(err, "blyte: {e}");
                            let _ = err.flush();
                            return 1;
                        }
                    };
                    let url = format!("http://127.0.0.1:{}/#token={token}", existing.port);
                    let _ = writeln!(out, "blyte: surface already running at {url}");
                    let _ = out.flush();
                    if !args.no_open {
                        opener(&url);
                    }
                    return 0;
                }
                let _ = writeln!(
                    err,
                    "blyte: stale surface file {} (port {}, session {}); replacing it",
                    surface_file_path.display(),
                    existing.port,
                    existing.session_id
                );
                let _ = err.flush();
            }
            Err(e) => {
                let _ = writeln!(
                    err,
                    "blyte: cannot parse {}: {e}",
                    surface_file_path.display()
                );
                return 1;
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            let _ = writeln!(
                err,
                "blyte: cannot read {}: {e}",
                surface_file_path.display()
            );
            return 1;
        }
    }

    if !config.converse {
        config.converse = true;
        if config.max_turns == 25 || config.max_turns == 60 {
            config.max_turns = 1000;
        }
        if config.fold_budget.is_none() {
            config.fold_budget = Some(100_000);
        }
    }
    config.repo_root = root.clone();

    let started_dur = match now_duration() {
        Ok(d) => d,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };
    let started_at = started_dur.as_secs();
    let session_id = new_session_id(started_dur.as_nanos());
    if std::env::var("BLYTE_SESSION")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        std::env::set_var("BLYTE_SESSION", &session_id);
    }

    let token = match generate_token() {
        Ok(t) => t,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    let bind_addr = format!("127.0.0.1:{}", args.port);
    let listener = match TcpListener::bind(&bind_addr) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(err, "blyte: failed to bind {bind_addr}: {e}");
            return 1;
        }
    };
    let actual_port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            let _ = writeln!(err, "blyte: failed to read listener address: {e}");
            return 1;
        }
    };

    let main_checkout = crate::config::main_checkout(&root).unwrap_or_else(|| root.clone());
    let entries_log_path = main_checkout
        .join(".wt")
        .join("sessions")
        .join("entries.jsonl");
    let model = resolve_initial_model(&config);
    let session = match Session::start_with_log_io(model, Some(entries_log_path), err, |approver| {
        Box::new(AgentRunner::new(&config, approver))
    }) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            let _ = err.flush();
            return 1;
        }
    };

    if let Err(e) = std::fs::create_dir_all(&wt_dir) {
        let _ = writeln!(err, "blyte: cannot create {}: {e}", wt_dir.display());
        return 1;
    }
    if let Err(e) = write_token_file(&wt_dir, &token_file_path, &token) {
        let _ = writeln!(err, "blyte: {e}");
        return 1;
    }
    let record = SurfaceFile {
        pid: std::process::id(),
        port: actual_port,
        session_id: session_id.clone(),
        started_at,
    };
    let record_json = match serde_json::to_string_pretty(&record) {
        Ok(s) => s + "\n",
        Err(e) => {
            let _ = std::fs::remove_file(&token_file_path);
            let _ = writeln!(err, "blyte: cannot serialize surface file: {e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::write(&surface_file_path, record_json) {
        let _ = std::fs::remove_file(&token_file_path);
        let _ = writeln!(
            err,
            "blyte: cannot write {}: {e}",
            surface_file_path.display()
        );
        return 1;
    }

    let url = format!("http://127.0.0.1:{actual_port}/#token={token}");
    let _ = writeln!(out, "blyte: surface at {url}");
    let _ = out.flush();
    if !args.no_open {
        opener(&url);
    }

    let surface = Arc::new(Surface {
        root,
        repo,
        session,
        session_id,
        started_at,
        days: args.days,
        token,
        requests: Arc::new(RequestsState::new()),
        publisher: Arc::new(RealPublisher),
        reconcile_lock: Mutex::new(()),
        reconcile_requested: AtomicBool::new(false),
    });

    let timer_guard = spawn_reconcile_timer(Arc::clone(&surface));

    let serve_result =
        crate::http::serve_until(listener, should_stop, move |req| match now_duration() {
            Ok(d) => route(&surface, req, d.as_secs()),
            Err(e) => json_error(500, e),
        });

    drop(timer_guard);

    let _ = std::fs::remove_file(&surface_file_path);
    let _ = std::fs::remove_file(&token_file_path);

    match serve_result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "blyte: server error: {e}");
            1
        }
    }
}
