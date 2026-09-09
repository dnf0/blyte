use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub severity: Severity,
    pub file: PathBuf,
    pub line_range: (usize, usize),
    pub problem: String,
    pub suggested_fix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
    #[serde(default)]
    pub has_blocking_issues: bool,
}

impl ReviewReport {
    pub fn clean(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            findings: Vec::new(),
            has_blocking_issues: false,
        }
    }

    pub fn new(summary: impl Into<String>, findings: Vec<ReviewFinding>) -> Self {
        let has_blocking_issues = findings
            .iter()
            .any(|f| matches!(f.severity, Severity::Critical | Severity::High));
        Self {
            summary: summary.into(),
            findings,
            has_blocking_issues,
        }
    }
}
