use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Severity {
    #[serde(alias = "critical", alias = "CRITICAL")]
    Critical,
    #[serde(alias = "high", alias = "HIGH")]
    High,
    #[serde(alias = "medium", alias = "MEDIUM")]
    Medium,
    #[serde(alias = "low", alias = "LOW")]
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
    #[serde(default)]
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

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Critical => write!(f, "Critical"),
            Severity::High => write!(f, "High"),
            Severity::Medium => write!(f, "Medium"),
            Severity::Low => write!(f, "Low"),
        }
    }
}

impl std::fmt::Display for ReviewFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {}:{}-{}\n   Problem: {}\n   Suggested Fix: {}",
            self.severity,
            self.file.display(),
            self.line_range.0,
            self.line_range.1,
            self.problem,
            self.suggested_fix
        )
    }
}

impl std::fmt::Display for ReviewReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Review Summary: {}", self.summary)?;
        if self.has_blocking_issues {
            writeln!(f, "Status: Blocking issues detected")?;
        } else {
            writeln!(f, "Status: Clean / Non-blocking")?;
        }
        if self.findings.is_empty() {
            write!(f, "No findings reported.")?;
        } else {
            writeln!(f, "\nFindings ({}):", self.findings.len())?;
            for (idx, finding) in self.findings.iter().enumerate() {
                if idx > 0 {
                    writeln!(f)?;
                }
                write!(f, "{}. {}", idx + 1, finding)?;
            }
        }
        Ok(())
    }
}
