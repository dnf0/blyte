use std::path::{Path, PathBuf};
use superstate_core::ledger::append_ledger_note;
use superstate_core::models::LedgerEntry;
use superstate_core::state_machine::resolve_state_path;
use superstate_review::diff::get_git_diff;
use superstate_review::runner::{execute_review, ReviewConfig};
use superstate_review::schema::ReviewReport;

pub struct ReviewArgs {
    pub commit_range: Option<String>,
    pub repo: PathBuf,
    pub driver: Option<String>,
    pub state_path: Option<PathBuf>,
    pub record: bool,
    pub json: bool,
    pub fail_on_blocking: bool,
}

pub fn cmd_review(args: ReviewArgs) -> Result<(), String> {
    let diff = match args.commit_range.as_deref() {
        Some(range) => get_git_diff(&args.repo, Some(range))
            .map_err(|e| format!("Failed to get git diff: {}", e))?,
        None => {
            // If no commit range is provided, check if HEAD exists to get both staged and unstaged diff
            match get_git_diff(&args.repo, Some("HEAD")) {
                Ok(d) if !d.trim().is_empty() => d,
                Ok(_) => {
                    // HEAD exists but diff against HEAD is empty. Check unstaged diff as fallback.
                    get_git_diff(&args.repo, None).unwrap_or_default()
                }
                Err(_) => {
                    // HEAD does not exist (e.g. freshly initialized repo before first commit)
                    get_git_diff(&args.repo, None)
                        .map_err(|e| format!("Failed to get git diff: {}", e))?
                }
            }
        }
    };

    if diff.trim().is_empty() {
        if args.json {
            let report = ReviewReport::clean("No changes detected in diff.");
            let json_str = serde_json::to_string_pretty(&report)
                .map_err(|e| format!("Failed to serialize report to JSON: {}", e))?;
            println!("{}", json_str);
        } else {
            println!("No changes detected in diff. Review clean.");
        }
        return Ok(());
    }

    let config = ReviewConfig {
        driver: args.driver,
        timeout_secs: 120,
    };

    let report = execute_review(&args.repo, &diff, None, &config)
        .map_err(|e| format!("Review failed: {}", e))?;

    if args.record && (report.has_blocking_issues || !report.findings.is_empty()) {
        let state_file = match &args.state_path {
            Some(p) => resolve_state_path(p, &args.repo),
            None => {
                let default_path = Path::new(".agent-state").join("state.json");
                resolve_state_path(&default_path, &args.repo)
            }
        };

        if state_file.exists() {
            for finding in &report.findings {
                let note = format!(
                    "[review-advisory] {}:{}-{}: {} (Fix: {})",
                    finding.file.display(),
                    finding.line_range.0,
                    finding.line_range.1,
                    finding.problem,
                    finding.suggested_fix
                );
                let entry = LedgerEntry::new("reviewer", note);
                append_ledger_note(&state_file, entry)
                    .map_err(|e| format!("Failed to append review finding to ledger: {}", e))?;
            }
        }
    }

    if args.json {
        let json_str = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("Failed to serialize report to JSON: {}", e))?;
        println!("{}", json_str);
    } else {
        println!("{}", report);
    }

    if args.fail_on_blocking && report.has_blocking_issues {
        return Err("Review failed: blocking issues detected".to_string());
    }

    Ok(())
}
