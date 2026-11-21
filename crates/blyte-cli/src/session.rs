use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Entry {
    pub seq: u64,
    pub ts: u64,
    #[serde(flatten)]
    pub body: EntryBody,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
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
}

impl AgentRunner {
    pub fn new(config: &crate::repl::ReplConfig) -> Self {
        let mut cfg = config.clone();
        cfg.orchestrate = true;
        let repl = crate::repl::ReplSession::new(&cfg);
        Self { repl }
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
        let (outcome, _) = crate::task_runner::run_task_with_state_and_events(
            &task_config,
            &mut self.repl.state,
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

struct SessionState {
    entries: Vec<Entry>,
    queue: VecDeque<String>,
    model: String,
    effort: Option<String>,
    busy: bool,
    closed: bool,
}

impl SessionState {
    fn push_entry(&mut self, body: EntryBody) -> u64 {
        let seq = self.entries.len() as u64 + 1;
        let ts = now_secs();
        self.entries.push(Entry { seq, ts, body });
        seq
    }

    fn effective_model(&self) -> String {
        match &self.effort {
            Some(effort) if model_supports_effort(&self.model) => {
                format!("{}-{}", self.model, effort)
            }
            _ => self.model.clone(),
        }
    }
}

struct Shared {
    state: Mutex<SessionState>,
    log_cv: Condvar,
    work_cv: Condvar,
}

impl Shared {
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
        self.shared.work_cv.notify_all();
        self.shared.log_cv.notify_all();
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
        let (base_model, effort) = parse_model_and_effort(&model);
        let shared = Arc::new(Shared {
            state: Mutex::new(SessionState {
                entries: Vec::new(),
                queue: VecDeque::new(),
                model: base_model,
                effort,
                busy: false,
                closed: false,
            }),
            log_cv: Condvar::new(),
            work_cv: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        std::thread::spawn(move || worker_loop(runner, worker_shared));
        Session {
            guard: Arc::new(SessionGuard { shared }),
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
        state.queue.push_back(full_prompt);
        self.guard.shared.work_cv.notify_one();
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }

    pub fn entries_after(&self, after: u64, wait: Duration) -> Vec<Entry> {
        let mut state = self.guard.shared.lock_state();
        if (state.entries.len() as u64) <= after && !wait.is_zero() {
            let deadline = Instant::now() + wait;
            while (state.entries.len() as u64) <= after {
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
        let start = usize::try_from(after).unwrap_or(usize::MAX);
        if start >= state.entries.len() {
            Vec::new()
        } else {
            state.entries[start..].to_vec()
        }
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.guard.shared.lock_state();
        SessionInfo {
            effort_supported: model_supports_effort(&state.model),
            model: state.model.clone(),
            effort: state.effort.clone(),
            busy: state.busy,
            queued: state.queue.len(),
            last_seq: state.entries.len() as u64,
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
            state.queue.push_back(msg.to_string());
            self.guard.shared.work_cv.notify_one();
        }
        self.guard.shared.log_cv.notify_all();
        Ok(seq)
    }
}

fn worker_loop(mut runner: Box<dyn PromptRunner>, shared: Arc<Shared>) {
    loop {
        let (prompt, effective_model) = {
            let mut state = shared.lock_state();
            loop {
                if state.closed {
                    return;
                }
                if let Some(prompt) = state.queue.pop_front() {
                    state.busy = true;
                    let model = state.effective_model();
                    break (prompt, model);
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
            runner.run(&prompt, &effective_model, &emit)
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

fn model_supports_effort(model: &str) -> bool {
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
