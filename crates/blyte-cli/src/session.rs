use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const RESTART_NOTICE_TEXT: &str =
    "blyte restarted: the entries above are from before; the model does not remember them";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub ts: u64,
    #[serde(flatten)]
    pub body: EntryBody,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryBody {
    UserMessage {
        text: String,
        reports: Vec<String>,
        queued: bool,
    },
    Turn {
        n: u32,
        text: String,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    },
    ToolStarted {
        name: String,
        input: String,
    },
    ToolFinished {
        name: String,
        summary: String,
        is_error: bool,
    },
    Retry {
        attempt: u32,
        max: u32,
        wait_ms: u64,
        reason: String,
    },
    Error {
        message: String,
    },
    Idle {
        turns: u32,
        cost_usd: Option<f64>,
    },
    ModelChanged {
        model: String,
    },
    EffortChanged {
        effort: Option<String>,
    },
    Decision {
        run: String,
        action: String,
        note: Option<String>,
    },
    ApprovalRequested {
        id: String,
        command: String,
        reason: String,
    },
    ApprovalAnswered {
        id: String,
        command: String,
        answer: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalAnswer {
    Yes,
    No,
}

impl ApprovalAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalAnswer::Yes => "yes",
            ApprovalAnswer::No => "no",
        }
    }

    fn to_approval(self) -> blyte_agent::approval::Approval {
        match self {
            ApprovalAnswer::Yes => blyte_agent::approval::Approval::Approved,
            ApprovalAnswer::No => blyte_agent::approval::Approval::Denied,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerError {
    NotFound,
    AlreadyAnswered,
}

pub struct SurfaceApprover {
    shared: Arc<Shared>,
}

impl blyte_agent::approval::Approver for SurfaceApprover {
    fn ask(&self, command: &str, reason: &str) -> blyte_agent::approval::Approval {
        let mut state = self.shared.lock_state();
        if state.closed {
            return blyte_agent::approval::Approval::Denied;
        }
        state.next_approval_id += 1;
        let order = state.next_approval_id;
        let id = format!("approval-{order}");
        state.approvals.insert(
            id.clone(),
            ApprovalRecord {
                order,
                command: command.to_string(),
                answer: None,
            },
        );
        state.push_entry(EntryBody::ApprovalRequested {
            id: id.clone(),
            command: command.to_string(),
            reason: reason.to_string(),
        });
        self.shared.log_cv.notify_all();

        loop {
            if let Some(answer) = state.approvals.get(&id).and_then(|r| r.answer) {
                return answer.to_approval();
            }
            if state.closed {
                return blyte_agent::approval::Approval::Denied;
            }
            state = self
                .shared
                .approval_cv
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub run: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SessionInfo {
    pub model: String,
    pub effort: Option<String>,
    pub effort_supported: bool,
    pub busy: bool,
    pub queued: usize,
    pub last_seq: u64,
}

pub trait PromptRunner: Send + 'static {
    fn run(
        &mut self,
        prompt: &str,
        model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String>;
}

pub struct AgentRunner {
    repl: crate::repl::ReplSession,
    approver: Arc<dyn blyte_agent::approval::Approver>,
}

impl AgentRunner {
    pub fn new(
        config: &crate::repl::ReplConfig,
        approver: Arc<dyn blyte_agent::approval::Approver>,
    ) -> Self {
        let mut cfg = config.clone();
        cfg.orchestrate = true;
        let repl = crate::repl::ReplSession::try_new_with_approver(&cfg, Some(approver.clone()))
            .expect("failed to initialize REPL session");
        Self { repl, approver }
    }
}

impl PromptRunner for AgentRunner {
    fn run(
        &mut self,
        prompt: &str,
        model: &str,
        emit: &(dyn Fn(EntryBody) + Sync),
    ) -> Result<(), String> {
        self.repl.model = model.to_string();
        self.repl.state.orchestrator.goal = prompt.to_string();
        if let Some(ref gate) = self.repl.gate {
            self.repl.state.orchestrator.verification_cmd = gate.clone();
        }
        let task_config = self.repl.task_config_for_prompt(prompt);
        let pricing = blyte_provider::pricing(model);
        let on_event = |event: &blyte_agent::run::Event<'_>| match event {
            blyte_agent::run::Event::Turn { n, text, usage } => {
                emit(EntryBody::Turn {
                    n: *n,
                    text: (*text).to_string(),
                    input_tokens: usage.input + usage.cached,
                    output_tokens: usage.output + usage.thinking,
                    cost_usd: pricing.map(|p| p.cost(usage)),
                });
            }
            blyte_agent::run::Event::ToolStart { name, input } => {
                emit(EntryBody::ToolStarted {
                    name: (*name).to_string(),
                    input: input.clone(),
                });
            }
            blyte_agent::run::Event::Tool {
                name,
                summary,
                is_error,
                ..
            } => {
                emit(EntryBody::ToolFinished {
                    name: (*name).to_string(),
                    summary: summary.clone(),
                    is_error: *is_error,
                });
            }
            blyte_agent::run::Event::Retry {
                attempt,
                max,
                wait_ms,
                reason,
            } => {
                emit(EntryBody::Retry {
                    attempt: *attempt,
                    max: *max,
                    wait_ms: *wait_ms,
                    reason: reason.clone(),
                });
            }
            _ => {}
        };
        let (outcome, _) = crate::task_runner::run_task_with_approver(
            &task_config,
            &mut self.repl.state,
            Some(self.approver.clone()),
            &on_event,
        )?;
        match outcome {
            blyte_agent::run::Outcome::Verified | blyte_agent::run::Outcome::Finished => Ok(()),
            blyte_agent::run::Outcome::BudgetExhausted => Err("budget exhausted".to_string()),
            blyte_agent::run::Outcome::GateFailed => Err("gate failed".to_string()),
            blyte_agent::run::Outcome::Blocked(msg) | blyte_agent::run::Outcome::Error(msg) => {
                Err(msg)
            }
        }
    }
}

struct ApprovalRecord {
    order: u64,
    command: String,
    answer: Option<ApprovalAnswer>,
}

struct QueuedMessage {
    request_id: String,
    prompt: String,
}

struct EntryLog {
    path: PathBuf,
    file: File,
}

impl EntryLog {
    fn open(path: PathBuf, err: &mut dyn Write) -> Result<(Self, Vec<Entry>), String> {
        let (entries, truncate_to, needs_newline) = load_entries_file(&path, err)?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
            }
        }
        if let Some(len) = truncate_to {
            let f = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|e| format!("failed to truncate {}: {e}", path.display()))?;
            f.set_len(len)
                .map_err(|e| format!("failed to truncate {}: {e}", path.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
        if needs_newline {
            file.write_all(b"\n")
                .and_then(|_| file.flush())
                .map_err(|e| format!("failed to write to {}: {e}", path.display()))?;
        }
        Ok((Self { path, file }, entries))
    }

    fn append(&mut self, entry: &Entry) {
        let mut line = match serde_json::to_string(entry) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "blyte: failed to serialize entry for {}: {e}",
                    self.path.display()
                );
                return;
            }
        };
        line.push('\n');
        if let Err(e) = self
            .file
            .write_all(line.as_bytes())
            .and_then(|_| self.file.flush())
        {
            eprintln!("blyte: failed to write to {}: {e}", self.path.display());
        }
    }
}

fn load_entries_file(
    path: &Path,
    err: &mut dyn Write,
) -> Result<(Vec<Entry>, Option<u64>, bool), String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((Vec::new(), None, false)),
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    if bytes.is_empty() {
        return Ok((Vec::new(), None, false));
    }

    let mut entries = Vec::new();
    let mut pos = 0usize;
    let mut line_no = 0usize;
    let mut truncate_to = None;
    let mut valid_end = 0usize;

    while pos < bytes.len() {
        line_no += 1;
        let start = pos;
        let end = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(rel) => pos + rel + 1,
            None => bytes.len(),
        };
        let is_last = end == bytes.len();
        let line_slice = &bytes[start..end];
        let parsed = std::str::from_utf8(line_slice)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Entry>(s).map_err(|e| e.to_string()));
        match parsed {
            Ok(entry) => {
                entries.push(entry);
                valid_end = end;
            }
            Err(reason) => {
                if is_last {
                    let _ = writeln!(
                        err,
                        "blyte: dropping truncated last line in {} at line {}: {}",
                        path.display(),
                        line_no,
                        reason
                    );
                    let _ = err.flush();
                    truncate_to = Some(start as u64);
                } else {
                    return Err(format!(
                        "invalid entry in {} at line {}: {}",
                        path.display(),
                        line_no,
                        reason
                    ));
                }
            }
        }
        pos = end;
    }

    let needs_newline = valid_end > 0 && bytes[valid_end - 1] != b'\n';
    Ok((entries, truncate_to, needs_newline))
}

fn max_approval_id(entries: &[Entry]) -> u64 {
    entries
        .iter()
        .filter_map(|e| match &e.body {
            EntryBody::ApprovalRequested { id, .. } | EntryBody::ApprovalAnswered { id, .. } => {
                id.strip_prefix("approval-")?.parse::<u64>().ok()
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

struct SessionState {
    entries: Vec<Entry>,
    last_seq: u64,
    log: Option<EntryLog>,
    queue: VecDeque<QueuedMessage>,
    session_prefix: String,
    request_seq: u64,
    model: String,
    effort: Option<String>,
    busy: bool,
    closed: bool,
    approvals: HashMap<String, ApprovalRecord>,
    next_approval_id: u64,
}

impl SessionState {
    fn push_entry(&mut self, body: EntryBody) -> u64 {
        self.last_seq += 1;
        let seq = self.last_seq;
        let ts = now_secs();
        let entry = Entry { seq, ts, body };
        if let Some(log) = &mut self.log {
            log.append(&entry);
        }
        self.entries.push(entry);
        seq
    }

    fn next_request_id(&mut self) -> String {
        self.request_seq += 1;
        format!("{}-{}", self.session_prefix, self.request_seq)
    }

    fn effective_model(&self) -> String {
        match &self.effort {
            Some(effort) if model_supports_effort(&self.model) => {
                format!("{}-{}", self.model, effort)
            }
            _ => self.model.clone(),
        }
    }

    fn deny_all_pending(&mut self) -> bool {
        let mut pending: Vec<(u64, String, String)> = self
            .approvals
            .iter()
            .filter(|(_, record)| record.answer.is_none())
            .map(|(id, record)| (record.order, id.clone(), record.command.clone()))
            .collect();
        if pending.is_empty() {
            return false;
        }
        pending.sort_by_key(|(order, _, _)| *order);
        for (_, id, command) in pending {
            if let Some(record) = self.approvals.get_mut(&id) {
                record.answer = Some(ApprovalAnswer::No);
            }
            self.push_entry(EntryBody::ApprovalAnswered {
                id,
                command,
                answer: ApprovalAnswer::No.as_str().to_string(),
            });
        }
        true
    }
}

struct Shared {
    state: Mutex<SessionState>,
    log_cv: Condvar,
    work_cv: Condvar,
    approval_cv: Condvar,
}

impl Shared {
    fn new(model: String, effort: Option<String>) -> Self {
        Self {
            state: Mutex::new(SessionState {
                entries: Vec::new(),
                last_seq: 0,
                log: None,
                queue: VecDeque::new(),
                session_prefix: new_session_prefix(),
                request_seq: 0,
                model,
                effort,
                busy: false,
                closed: false,
                approvals: HashMap::new(),
                next_approval_id: 0,
            }),
            log_cv: Condvar::new(),
            work_cv: Condvar::new(),
            approval_cv: Condvar::new(),
        }
    }

    fn new_with_log(
        model: String,
        effort: Option<String>,
        log_path: Option<PathBuf>,
        err: &mut dyn Write,
    ) -> Result<Self, String> {
        let (log, entries) = match log_path {
            Some(path) => {
                let (log, entries) = EntryLog::open(path, err)?;
                (Some(log), entries)
            }
            None => (None, Vec::new()),
        };
        let last_seq = entries.iter().map(|e| e.seq).max().unwrap_or(0);
        let next_approval_id = max_approval_id(&entries);
        let had_entries = !entries.is_empty();
        let mut state = SessionState {
            entries,
            last_seq,
            log,
            queue: VecDeque::new(),
            session_prefix: new_session_prefix(),
            request_seq: 0,
            model,
            effort,
            busy: false,
            closed: false,
            approvals: HashMap::new(),
            next_approval_id,
        };
        if had_entries {
            state.push_entry(EntryBody::Error {
                message: RESTART_NOTICE_TEXT.to_string(),
            });
        }
        Ok(Self {
            state: Mutex::new(state),
            log_cv: Condvar::new(),
            work_cv: Condvar::new(),
            approval_cv: Condvar::new(),
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct SessionGuard {
    shared: Arc<Shared>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut state = self.shared.lock_state();
        state.closed = true;
        state.deny_all_pending();
        self.shared.work_cv.notify_all();
        self.shared.log_cv.notify_all();
        self.shared.approval_cv.notify_all();
    }
}

#[derive(Clone)]
pub struct Session {
    guard: Arc<SessionGuard>,
}

struct TurnTotals {
    turns: u32,
    cost_usd: Option<f64>,
}

impl Session {
    pub fn start(runner: Box<dyn PromptRunner>, model: String) -> Session {
        Self::start_with(model, |_| runner)
    }

    pub fn start_with<F>(model: String, build_runner: F) -> Session
    where
        F: FnOnce(Arc<SurfaceApprover>) -> Box<dyn PromptRunner>,
    {
        let (base_model, effort) = parse_model_and_effort(&model);
        let shared = Arc::new(Shared::new(base_model, effort));
        let approver = Arc::new(SurfaceApprover {
            shared: Arc::clone(&shared),
        });
        let runner = build_runner(approver);
        let worker_shared = Arc::clone(&shared);
        std::thread::spawn(move || worker_loop(runner, worker_shared));
        Session {
            guard: Arc::new(SessionGuard { shared }),
        }
    }

    pub fn start_with_log<F>(
        model: String,
        log_path: Option<PathBuf>,
        build_runner: F,
    ) -> Result<Session, String>
    where
        F: FnOnce(Arc<SurfaceApprover>) -> Box<dyn PromptRunner>,
    {
        let mut stderr = std::io::stderr();
        Self::start_with_log_io(model, log_path, &mut stderr, build_runner)
    }

    pub fn start_with_log_io<F>(
        model: String,
        log_path: Option<PathBuf>,
        err: &mut dyn Write,
        build_runner: F,
    ) -> Result<Session, String>
    where
        F: FnOnce(Arc<SurfaceApprover>) -> Box<dyn PromptRunner>,
    {
        let (base_model, effort) = parse_model_and_effort(&model);
        let shared = Arc::new(Shared::new_with_log(base_model, effort, log_path, err)?);
        let approver = Arc::new(SurfaceApprover {
            shared: Arc::clone(&shared),
        });
        let runner = build_runner(approver);
        let worker_shared = Arc::clone(&shared);
        std::thread::spawn(move || worker_loop(runner, worker_shared));
        Ok(Session {
            guard: Arc::new(SessionGuard { shared }),
        })
    }

    #[doc(hidden)]
    pub fn start_paused(
        runner: Box<dyn PromptRunner>,
        model: String,
    ) -> (Session, std::sync::mpsc::Sender<()>) {
        let (base_model, effort) = parse_model_and_effort(&model);
        let shared = Arc::new(Shared::new(base_model, effort));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            if rx.recv().is_ok() {
                worker_loop(runner, worker_shared);
            }
        });
        (
            Session {
                guard: Arc::new(SessionGuard { shared }),
            },
            tx,
        )
    }

    pub fn approver(&self) -> Arc<SurfaceApprover> {
        Arc::new(SurfaceApprover {
            shared: Arc::clone(&self.guard.shared),
        })
    }

    pub fn answer_approval(&self, id: &str, answer: ApprovalAnswer) -> Result<u64, AnswerError> {
        let mut state = self.guard.shared.lock_state();
        let command = match state.approvals.get_mut(id) {
            None => return Err(AnswerError::NotFound),
            Some(record) if record.answer.is_some() => return Err(AnswerError::AlreadyAnswered),
            Some(record) => {
                record.answer = Some(answer);
                record.command.clone()
            }
        };
        let seq = state.push_entry(EntryBody::ApprovalAnswered {
            id: id.to_string(),
            command,
            answer: answer.as_str().to_string(),
        });
        self.guard.shared.approval_cv.notify_all();
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }

    pub fn stop_run(&self) {
        let mut state = self.guard.shared.lock_state();
        if state.deny_all_pending() {
            self.guard.shared.approval_cv.notify_all();
            self.guard.shared.log_cv.notify_all();
        }
    }

    pub fn submit(&self, text: &str, attachments: Vec<Attachment>) -> Result<u64, String> {
        if text.trim().is_empty() {
            return Err("a message needs text".to_string());
        }
        let reports = attachments.iter().map(|a| a.run.clone()).collect();
        let full_prompt = format_prompt(text, &attachments);
        let mut state = self.guard.shared.lock_state();
        let queued = state.busy || !state.queue.is_empty();
        let seq = state.push_entry(EntryBody::UserMessage {
            text: text.to_string(),
            reports,
            queued,
        });
        let request_id = state.next_request_id();
        state.queue.push_back(QueuedMessage {
            request_id,
            prompt: full_prompt,
        });
        self.guard.shared.work_cv.notify_one();
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }

    pub fn entries_after(&self, after: u64, wait: Duration) -> Vec<Entry> {
        let mut state = self.guard.shared.lock_state();
        if state.last_seq <= after && !wait.is_zero() {
            let deadline = Instant::now() + wait;
            while state.last_seq <= after {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (guard, _) = self
                    .guard
                    .shared
                    .log_cv
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|e| e.into_inner());
                state = guard;
            }
        }
        state
            .entries
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect()
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.guard.shared.lock_state();
        SessionInfo {
            effort_supported: model_supports_effort(&state.model),
            model: state.model.clone(),
            effort: state.effort.clone(),
            busy: state.busy,
            queued: state.queue.len(),
            last_seq: state.last_seq,
        }
    }

    pub fn set_model(&self, model: &str) -> Result<(), String> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Err("model cannot be empty".to_string());
        }
        let mut state = self.guard.shared.lock_state();
        let supported = model_supports_effort(trimmed);
        if supported {
            let (base, parsed_effort) = blyte_provider::gemini::split_effort(trimmed);
            state.model = base.clone();
            state.push_entry(EntryBody::ModelChanged { model: base });
            if let Some(new_effort) = parsed_effort {
                if state.effort.as_deref() != Some(new_effort.as_str()) {
                    state.effort = Some(new_effort.clone());
                    state.push_entry(EntryBody::EffortChanged {
                        effort: Some(new_effort),
                    });
                }
            }
        } else {
            state.model = trimmed.to_string();
            state.push_entry(EntryBody::ModelChanged {
                model: trimmed.to_string(),
            });
            if state.effort.is_some() {
                state.effort = None;
                state.push_entry(EntryBody::EffortChanged { effort: None });
            }
        }
        self.guard.shared.log_cv.notify_all();
        Ok(())
    }

    pub fn set_effort(&self, effort: Option<&str>) -> Result<(), String> {
        let normalized = match effort {
            None => None,
            Some("low") => Some("low".to_string()),
            Some("medium") => Some("medium".to_string()),
            Some("high") => Some("high".to_string()),
            Some(_) => return Err("effort must be low, medium or high".to_string()),
        };
        let mut state = self.guard.shared.lock_state();
        if !model_supports_effort(&state.model) {
            return Err(format!("{} does not take an effort setting", state.model));
        }
        state.effort = normalized.clone();
        state.push_entry(EntryBody::EffortChanged { effort: normalized });
        self.guard.shared.log_cv.notify_all();
        Ok(())
    }

    pub fn record_decision(
        &self,
        run: &str,
        action: &str,
        note: Option<&str>,
        orchestrator_message: Option<&str>,
    ) -> Result<u64, String> {
        if let Some(msg) = orchestrator_message {
            if msg.trim().is_empty() {
                return Err("a message needs text".to_string());
            }
        }
        let mut state = self.guard.shared.lock_state();
        let seq = state.push_entry(EntryBody::Decision {
            run: run.to_string(),
            action: action.to_string(),
            note: note.map(str::to_string),
        });
        if let Some(msg) = orchestrator_message {
            let queued = state.busy || !state.queue.is_empty();
            state.push_entry(EntryBody::UserMessage {
                text: msg.to_string(),
                reports: Vec::new(),
                queued,
            });
            let request_id = state.next_request_id();
            state.queue.push_back(QueuedMessage {
                request_id,
                prompt: msg.to_string(),
            });
            self.guard.shared.work_cv.notify_one();
        }
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }

    pub fn queue_for_request(&self, request_id: &str, text: &str) -> Result<u64, String> {
        if text.trim().is_empty() {
            return Err("a message needs text".to_string());
        }
        let mut state = self.guard.shared.lock_state();
        let queued = state.busy || !state.queue.is_empty();
        let seq = state.push_entry(EntryBody::UserMessage {
            text: text.to_string(),
            reports: Vec::new(),
            queued,
        });
        state.queue.push_back(QueuedMessage {
            request_id: request_id.to_string(),
            prompt: text.to_string(),
        });
        self.guard.shared.work_cv.notify_one();
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }

    pub fn record_error(&self, message: &str) -> u64 {
        let mut state = self.guard.shared.lock_state();
        let seq = state.push_entry(EntryBody::Error {
            message: message.to_string(),
        });
        self.guard.shared.log_cv.notify_all();
        seq
    }
}

struct RequestGuard;

impl RequestGuard {
    fn enter(request_id: String) -> Self {
        blyte_agent::tools::set_blyte_request(Some(request_id));
        Self
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        blyte_agent::tools::set_blyte_request(None);
    }
}

fn worker_loop(mut runner: Box<dyn PromptRunner>, shared: Arc<Shared>) {
    loop {
        let (msg, effective_model) = {
            let mut state = shared.lock_state();
            loop {
                if state.closed {
                    return;
                }
                if let Some(msg) = state.queue.pop_front() {
                    state.busy = true;
                    let model = state.effective_model();
                    break (msg, model);
                }
                state = shared
                    .work_cv
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };

        let totals = Mutex::new(TurnTotals {
            turns: 0,
            cost_usd: Some(0.0),
        });
        let emit_shared = Arc::clone(&shared);
        let emit = |body: EntryBody| {
            if let EntryBody::Turn { cost_usd, .. } = &body {
                let mut t = totals.lock().unwrap_or_else(|e| e.into_inner());
                t.turns += 1;
                match (t.cost_usd, cost_usd) {
                    (Some(acc), Some(c)) => t.cost_usd = Some(acc + c),
                    _ => t.cost_usd = None,
                }
            }
            let mut state = emit_shared.lock_state();
            state.push_entry(body);
            emit_shared.log_cv.notify_all();
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _req_guard = RequestGuard::enter(msg.request_id);
            runner.run(&msg.prompt, &effective_model, &emit)
        }));

        let (turns, cost_usd) = {
            let t = totals.lock().unwrap_or_else(|e| e.into_inner());
            (t.turns, t.cost_usd)
        };

        let mut state = shared.lock_state();
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                state.push_entry(EntryBody::Error { message });
            }
            Err(payload) => {
                let message = panic_message(payload);
                state.push_entry(EntryBody::Error { message });
            }
        }
        state.push_entry(EntryBody::Idle { turns, cost_usd });
        state.busy = false;
        shared.log_cv.notify_all();
    }
}

fn format_prompt(text: &str, attachments: &[Attachment]) -> String {
    let mut out = text.to_string();
    for att in attachments {
        out.push_str("\n\nResearch report ");
        out.push_str(&att.run);
        out.push_str(":\n");
        out.push_str(&att.text);
    }
    out
}

pub(crate) fn model_supports_effort(model: &str) -> bool {
    blyte_provider::detect_provider_kind(Some(model)) == blyte_provider::ProviderKind::Gemini
}

fn parse_model_and_effort(model: &str) -> (String, Option<String>) {
    let trimmed = model.trim();
    if model_supports_effort(trimmed) {
        blyte_provider::gemini::split_effort(trimmed)
    } else {
        (trimmed.to_string(), None)
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "runner panicked".to_string()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn new_session_prefix() -> String {
    let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("r{}-{}-{}", now_millis(), std::process::id(), seq)
}
