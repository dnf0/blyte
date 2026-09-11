use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    #[default]
    NotStarted,
    InProgress,
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct TaskSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub files_modify: Vec<PathBuf>,
    #[serde(default)]
    pub interfaces_produces: Vec<String>,
    #[serde(default)]
    pub verification_cmd: Option<String>,
}

impl<'de> Deserialize<'de> for TaskSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawTaskSpec {
            id: Option<String>,
            title: Option<String>,
            #[serde(default)]
            depends_on: Vec<String>,
            #[serde(default)]
            files_modify: Vec<PathBuf>,
            #[serde(default)]
            interfaces_produces: Vec<String>,
            #[serde(default)]
            verification_cmd: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum TaskSpecHelper {
            Str(String),
            Obj(RawTaskSpec),
        }

        match TaskSpecHelper::deserialize(deserializer)? {
            TaskSpecHelper::Str(s) => Ok(TaskSpec {
                id: s.split(':').next().unwrap_or(&s).trim().to_string(),
                title: s,
                depends_on: Vec::new(),
                files_modify: Vec::new(),
                interfaces_produces: Vec::new(),
                verification_cmd: None,
            }),
            TaskSpecHelper::Obj(raw) => Ok(TaskSpec {
                id: raw.id.unwrap_or_default(),
                title: raw.title.unwrap_or_default(),
                depends_on: raw.depends_on,
                files_modify: raw.files_modify,
                interfaces_produces: raw.interfaces_produces,
                verification_cmd: raw.verification_cmd,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerEntry {
    pub timestamp: DateTime<Utc>,
    pub author: String,
    pub note: String,
}

impl<'de> Deserialize<'de> for LedgerEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawLedgerEntry {
            timestamp: Option<DateTime<Utc>>,
            author: Option<String>,
            note: String,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum LedgerHelper {
            Str(String),
            Obj(RawLedgerEntry),
        }

        match LedgerHelper::deserialize(deserializer)? {
            LedgerHelper::Str(s) => Ok(LedgerEntry {
                timestamp: Utc::now(),
                author: "agent".to_string(),
                note: s,
            }),
            LedgerHelper::Obj(raw) => Ok(LedgerEntry {
                timestamp: raw.timestamp.unwrap_or_else(Utc::now),
                author: raw.author.unwrap_or_else(|| "agent".to_string()),
                note: raw.note,
            }),
        }
    }
}

impl LedgerEntry {
    pub fn new(author: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            author: author.into(),
            note: note.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionState {
    pub status: ExecutionStatus,
    pub step: usize,
    #[serde(default)]
    pub active_task: Option<TaskSpec>,
    #[serde(default)]
    pub pending_tasks: Vec<TaskSpec>,
    #[serde(default)]
    pub completed_tasks: Vec<TaskSpec>,
    #[serde(default)]
    pub modified_files: Vec<PathBuf>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub plan_path: Option<PathBuf>,
    #[serde(default)]
    pub notes: Vec<LedgerEntry>,
    /// Path to the isolated Git worktree active for the current task, if any.
    #[serde(default)]
    pub active_worktree: Option<PathBuf>,
    /// Branch name associated with the active worktree, if any.
    #[serde(default)]
    pub active_branch: Option<String>,
    /// Whether to disable worktrees and operate directly in the workspace root.
    #[serde(default)]
    pub no_worktree: bool,
}

impl Default for ExecutionState {
    fn default() -> Self {
        Self {
            status: ExecutionStatus::NotStarted,
            step: 0,
            active_task: None,
            pending_tasks: Vec::new(),
            completed_tasks: Vec::new(),
            modified_files: Vec::new(),
            errors: Vec::new(),
            plan_path: None,
            notes: Vec::new(),
            active_worktree: None,
            active_branch: None,
            no_worktree: false,
        }
    }
}
