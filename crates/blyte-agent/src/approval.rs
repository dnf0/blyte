use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishDetails {
    pub request: String,
    pub remote_url: String,
    pub branch: String,
    pub base: String,
    pub commit: String,
    pub commits: Vec<String>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Denied,
    Unavailable,
}

pub trait Approver: Send + Sync {
    fn ask(&self, command: &str, reason: &str) -> Approval;

    fn ask_publish(&self, _details: &PublishDetails) -> Approval {
        Approval::Unavailable
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoApprover;

impl Approver for NoApprover {
    fn ask(&self, _command: &str, _reason: &str) -> Approval {
        Approval::Unavailable
    }

    fn ask_publish(&self, _details: &PublishDetails) -> Approval {
        Approval::Unavailable
    }
}

pub fn is_simple_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    !trimmed.chars().any(|c| {
        matches!(
            c,
            '|' | ';' | '&' | '<' | '>' | '$' | '`' | '(' | ')' | '{' | '}' | '\n' | '\r' | '\\'
        )
    })
}

pub fn matching_rule<'a>(command: &str, rules: &'a [String]) -> Option<&'a str> {
    if !is_simple_command(command) {
        return None;
    }
    let cmd = command.trim();
    for rule in rules {
        let r = rule.trim();
        if r.is_empty() {
            continue;
        }
        if cmd == r
            || cmd
                .strip_prefix(r)
                .is_some_and(|rest| rest.starts_with(' '))
        {
            return Some(r);
        }
    }
    None
}

#[derive(Clone)]
pub struct ApprovalConfig {
    pub approver: Arc<dyn Approver>,
    pub rules: Vec<String>,
}
