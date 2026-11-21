use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cli::SurfaceArgs;
use crate::http::{Request, Response};
use crate::queue_actions::{Action, Hold};
use crate::repl::ReplConfig;
use crate::session::{AgentRunner, Attachment, Session};

const INDEX_HTML: &str = include_str!("../assets/surface/index.html");
const MAX_WAIT_MS: u64 = 25_000;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct Surface {
    pub root: PathBuf,
    pub repo: String,
    pub session: Session,
    pub session_id: String,
    pub started_at: u64,
    pub days: u64,
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

fn handle_runs(surface: &Surface, now: u64) -> Response {
    let mut live = match crate::live_runs::collect(&surface.root, now, surface.days) {
        Ok(l) => l,
        Err(e) => return json_error(500, e),
    };
    let mut any_written = false;
    for run in &live.runs {
        if run.checks_recorded || !surface.root.join(".wt").join(&run.id).is_dir() {
            continue;
        }
        match crate::queue_actions::ensure_checks(&surface.root, &run.id) {
            Ok(true) => any_written = true,
            Ok(false) => {}
            Err(e) => return json_error(500, e),
        }
    }
    if any_written {
        live = match crate::live_runs::collect(&surface.root, now, surface.days) {
            Ok(l) => l,
            Err(e) => return json_error(500, e),
        };
    }

    let mut runs_with_hold = Vec::with_capacity(live.runs.len());
    for run in &live.runs {
        let hold = match crate::queue_actions::read_hold(&surface.root, &run.id) {
            Ok(h) => h,
            Err(e) => return json_error(500, e),
        };
        runs_with_hold.push(RunWithHold { run, hold });
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
            if !wt.exists() {
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
        decision.orchestrator_message.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => return json_error(500, e),
    };
    Response::json(
        200,
        &serde_json::json!({
            "decision": {
                "run": decision.run,
                "action": action_str,
                "note": decision.note,
                "orchestrator_message": decision.orchestrator_message,
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

pub fn route(surface: &Surface, req: &Request, now: u64) -> Response {
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

    match req.path.as_str() {
        "/" | "/index.html" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            Response::html(INDEX_HTML.as_bytes().to_vec())
        }
        "/api/session" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            session_response(surface)
        }
        "/api/session/events" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            handle_events(surface, req)
        }
        "/api/session/messages" => {
            if req.method != "POST" {
                return method_not_allowed(req);
            }
            handle_post_message(surface, req)
        }
        "/api/models" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            handle_models()
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
        "/api/runs" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            handle_runs(surface, now)
        }
        "/api/map" => {
            if req.method != "GET" {
                return method_not_allowed(req);
            }
            handle_map(surface)
        }
        path => {
            if let Some(rest) = path.strip_prefix("/api/runs/") {
                if let Some(id) = rest.strip_suffix("/diff") {
                    if !id.is_empty() && !id.contains('/') {
                        if req.method != "GET" {
                            return method_not_allowed(req);
                        }
                        return handle_run_diff(surface, id);
                    }
                }
                if let Some(id) = rest.strip_suffix("/decision") {
                    if !id.is_empty() && !id.contains('/') {
                        if req.method != "POST" {
                            return method_not_allowed(req);
                        }
                        return handle_run_decision(surface, id, req, now);
                    }
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
        format!("GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
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

pub fn run(args: &SurfaceArgs, config: ReplConfig) -> i32 {
    run_with_io(args, config, &mut std::io::stdout(), &mut std::io::stderr())
}

pub fn run_with_io(
    args: &SurfaceArgs,
    mut config: ReplConfig,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
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
    match std::fs::read_to_string(&surface_file_path) {
        Ok(raw) => match serde_json::from_str::<SurfaceFile>(&raw) {
            Ok(existing) => {
                if probe_existing_surface(existing.port, &existing.session_id) {
                    let url = format!("http://127.0.0.1:{}/", existing.port);
                    let _ = writeln!(out, "blyte: surface already running at {url}");
                    let _ = out.flush();
                    if !args.no_open {
                        open_browser(&url);
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

    if !config.orchestrate {
        config.orchestrate = true;
        if config.max_turns == 25 || config.max_turns == 60 {
            config.max_turns = 1000;
        }
        if config.fold_budget.is_none() {
            config.fold_budget = Some(100_000);
        }
    }
    config.repo_root = root.clone();

    let model = resolve_initial_model(&config);
    let session = Session::start(Box::new(AgentRunner::new(&config)), model);
    let started_dur = match now_duration() {
        Ok(d) => d,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };
    let started_at = started_dur.as_secs();
    let session_id = new_session_id(started_dur.as_nanos());

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

    if let Err(e) = std::fs::create_dir_all(&wt_dir) {
        let _ = writeln!(err, "blyte: cannot create {}: {e}", wt_dir.display());
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
            let _ = writeln!(err, "blyte: cannot serialize surface file: {e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::write(&surface_file_path, record_json) {
        let _ = writeln!(
            err,
            "blyte: cannot write {}: {e}",
            surface_file_path.display()
        );
        return 1;
    }

    let url = format!("http://127.0.0.1:{actual_port}/");
    let _ = writeln!(out, "blyte: surface at {url}");
    let _ = out.flush();
    if !args.no_open {
        open_browser(&url);
    }

    let surface = Arc::new(Surface {
        root,
        repo,
        session,
        session_id,
        started_at,
        days: args.days,
    });

    let serve_result = crate::http::serve(listener, move |req| match now_duration() {
        Ok(d) => route(&surface, req, d.as_secs()),
        Err(e) => json_error(500, e),
    });

    let _ = std::fs::remove_file(&surface_file_path);

    match serve_result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "blyte: server error: {e}");
            1
        }
    }
}
