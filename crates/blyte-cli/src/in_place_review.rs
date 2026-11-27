use std::collections::{BTreeMap, HashSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::cli::{ReviewArgs, UndoArgs};
use crate::dispatch::{host_commands, outside_reads, refused_reads};

fn read_required_json(path: &Path) -> Result<Option<serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let val: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    Ok(Some(val))
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn read_manifest_map(
    manifest_path: &Path,
    summary_val: &serde_json::Value,
    summary_path: &Path,
) -> Result<BTreeMap<String, bool>, String> {
    let changed_arr = summary_val
        .get("changed_files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("missing changed_files in {}", summary_path.display()))?;
    let text = match std::fs::read_to_string(manifest_path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if changed_arr.is_empty() {
                return Ok(BTreeMap::new());
            }
            return Err(format!("missing manifest {}", manifest_path.display()));
        }
        Err(e) => return Err(format!("cannot read {}: {e}", manifest_path.display())),
    };

    let mut entries = BTreeMap::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let change: blyte_agent::snapshot_fs::Change = serde_json::from_str(line).map_err(|e| {
            format!(
                "cannot parse manifest {} line {line_no}: {e}",
                manifest_path.display()
            )
        })?;
        entries.entry(change.path).or_insert(change.existed);
    }
    for item in changed_arr {
        let Some(path) = item.as_str() else {
            return Err(format!(
                "invalid changed_files entry in {}",
                summary_path.display()
            ));
        };
        if !entries.contains_key(path) {
            return Err(format!(
                "manifest {} is missing changed file {path}",
                manifest_path.display()
            ));
        }
    }
    Ok(entries)
}

fn validate_rel_path(rel_path: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel_path);
    let mut has_normal = false;
    for comp in p.components() {
        match comp {
            Component::Normal(_) => has_normal = true,
            _ => {
                return Err(format!(
                    "invalid path in manifest: {rel_path} (must be relative inside the folder)"
                ));
            }
        }
    }
    if !has_normal {
        return Err(format!("invalid empty path in manifest: {rel_path:?}"));
    }
    Ok(p.to_path_buf())
}

pub fn record_verdict(
    store: &Path,
    id: &str,
    verdict: crate::cli::ReviewVerdict,
    notes: &str,
) -> Result<(), String> {
    let now = crate::dispatch_in_place::unix_timestamp_secs(std::time::SystemTime::now())?;
    record_verdict_at(store, id, verdict, notes, now)
}

pub fn record_verdict_at(
    store: &Path,
    id: &str,
    verdict: crate::cli::ReviewVerdict,
    notes: &str,
    now: u64,
) -> Result<(), String> {
    let verdict_path = store.join(format!("{id}.verdict.json"));
    let verdict_obj = serde_json::json!({
        "verdict": verdict.as_str(),
        "notes": notes,
        "ts": now,
    });
    let json_str = serde_json::to_string_pretty(&verdict_obj)
        .map_err(|e| format!("cannot serialize verdict: {e}"))?;
    std::fs::write(&verdict_path, json_str + "\n")
        .map_err(|e| format!("cannot write {}: {e}", verdict_path.display()))
}

pub fn is_undone(store: &Path, id: &str) -> bool {
    store.join(format!("{id}.undone.json")).exists()
}

pub struct RunChecks {
    pub flags: Vec<String>,
    pub hcmds: Vec<String>,
    pub reads: Vec<String>,
    pub undone: bool,
}

pub fn checks_for_run(
    root: &Path,
    store: &Path,
    id: &str,
    summary_val: &serde_json::Value,
    summary_path: &Path,
) -> Result<RunChecks, String> {
    let canonical = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
    let undone_path = store.join(format!("{id}.undone.json"));
    let undone_val = read_required_json(&undone_path)?;

    let log_path = store.join(format!("{id}.log"));
    let log_content = std::fs::read_to_string(&log_path)
        .map_err(|e| format!("cannot read the run's log {}: {e}", log_path.display()))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        "HOME is not set, so reads outside the folder cannot be checked".to_string()
    })?;
    let hcmds = host_commands(&log_content);
    let reads = outside_reads(&log_content, &[canonical], &home);
    let (refused_count, refused_paths) = refused_reads(&log_content);

    let outcome = summary_val
        .get("outcome")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing outcome in {}", summary_path.display()))?;

    let mut flags = Vec::new();
    if !matches!(outcome, "finished" | "verified") {
        flags.push(format!("outcome {outcome}: review in depth"));
    }
    if !hcmds.is_empty() {
        flags.push(format!(
            "{} host command(s): see == host commands",
            hcmds.len()
        ));
    }
    if !reads.is_empty() {
        flags.push(format!(
            "{} read(s) outside the repository: see == reads outside the repository",
            reads.len()
        ));
    }
    if refused_count > 0 {
        let first_3: Vec<&str> = refused_paths.iter().take(3).map(|s| s.as_str()).collect();
        flags.push(format!(
            "refused reads: {refused_count} (outside the workspace and --allow-read): {}",
            first_3.join(", ")
        ));
    }
    let undone = undone_val.is_some();
    if let Some(ref u) = undone_val {
        if let Some(ts) = u.get("undone_at").and_then(|v| v.as_u64()) {
            flags.push(format!("run {id} was undone at {ts}"));
        } else {
            flags.push(format!("run {id} was undone"));
        }
    }

    Ok(RunChecks {
        flags,
        hcmds,
        reads,
        undone,
    })
}

fn format_files_section(manifest: &BTreeMap<String, bool>) -> String {
    let mut out = String::from("== files\n");
    for (path, existed) in manifest {
        let kind = if *existed { "modified" } else { "created" };
        out.push_str(&format!("{kind} {path}\n"));
    }
    out
}

fn format_diff_section(diff: &str) -> String {
    let mut out = String::from("== diff\n");
    if !diff.is_empty() {
        out.push_str(diff);
        if !diff.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub(crate) fn parse_research_flag(
    start_val: &serde_json::Value,
    start_path: &Path,
) -> Result<bool, String> {
    match start_val.get("research") {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!(
            "invalid research field in {} (expected boolean)",
            start_path.display()
        )),
    }
}

pub fn diff_for_run(store: &Path, id: &str) -> Result<String, String> {
    let start_path = store.join(format!("{id}.start.json"));
    let start_val =
        read_required_json(&start_path)?.ok_or_else(|| format!("no run {id} in this folder"))?;

    let summary_path = store.join(format!("{id}.json"));
    let summary_val =
        read_required_json(&summary_path)?.ok_or_else(|| format!("run {id} has not finished"))?;

    let is_research = parse_research_flag(&start_val, &start_path)?
        || store.join(format!("{id}.report.md")).exists();

    if is_research {
        let report_path = store.join(format!("{id}.report.md"));
        return std::fs::read_to_string(&report_path)
            .map_err(|e| format!("cannot read report {}: {e}", report_path.display()));
    }

    let manifest_path = store.join(format!("{id}.changes.json"));
    let manifest = read_manifest_map(&manifest_path, &summary_val, &summary_path)?;
    let diff_path = store.join(format!("{id}.diff"));
    let diff_content = std::fs::read_to_string(&diff_path)
        .map_err(|e| format!("cannot read diff file {}: {e}", diff_path.display()))?;

    let mut out = format_files_section(&manifest);
    out.push_str(&format_diff_section(&diff_content));
    Ok(out)
}

pub fn review(args: &ReviewArgs) -> i32 {
    match review_inner(args) {
        Ok(()) => 0,
        Err(msg) => {
            if msg.starts_with("error:") {
                eprintln!("{msg}");
            } else {
                eprintln!("error: {msg}");
            }
            2
        }
    }
}

fn review_inner(args: &ReviewArgs) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("{e}"))?;
    let root = cwd
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", cwd.display()))?;
    let store = crate::dispatch_in_place::records_dir_for(&cwd)?;

    let start_path = store.join(format!("{}.start.json", args.id));
    let start_val = read_required_json(&start_path)?
        .ok_or_else(|| format!("no run {} in this folder", args.id))?;

    let summary_path = store.join(format!("{}.json", args.id));
    let summary_val = read_required_json(&summary_path)?
        .ok_or_else(|| format!("run {} has not finished", args.id))?;

    if args.notes.is_some() && args.verdict.is_none() {
        return Err("--notes requires --verdict".to_string());
    }
    if let Some(verdict) = args.verdict {
        record_verdict(
            &store,
            &args.id,
            verdict,
            args.notes.as_deref().unwrap_or(""),
        )?;
        println!("recorded verdict {} for {}", verdict.as_str(), args.id);
        return Ok(());
    }

    let is_research = parse_research_flag(&start_val, &start_path)?;

    let manifest = if is_research {
        BTreeMap::new()
    } else {
        let manifest_path = store.join(format!("{}.changes.json", args.id));
        read_manifest_map(&manifest_path, &summary_val, &summary_path)?
    };

    let diff_content = if args.diff {
        let diff_path = store.join(format!("{}.diff", args.id));
        let text = std::fs::read_to_string(&diff_path)
            .map_err(|e| format!("cannot read diff file {}: {e}", diff_path.display()))?;
        Some(text)
    } else {
        None
    };

    let checks = checks_for_run(&root, &store, &args.id, &summary_val, &summary_path)?;
    let outcome = summary_val
        .get("outcome")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing outcome in {}", summary_path.display()))?;

    println!("== checks");
    if checks.flags.is_empty() {
        println!("nothing flagged");
    } else {
        for flag in &checks.flags {
            println!("! {flag}");
        }
    }

    if !is_research {
        print!("{}", format_files_section(&manifest));
    }

    println!("== report");
    println!("outcome: {outcome}");
    let final_message = summary_val.get("final_message").and_then(|x| x.as_str());
    if let Some(err) = summary_val
        .get("error")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        if !final_message.is_some_and(|m| m.contains(err)) {
            println!("error: {err}");
        }
    }
    if let Some(turns) = summary_val.get("turns").and_then(|x| x.as_u64()) {
        println!("turns: {turns}");
    }
    if let Some(cost) = summary_val.get("cost_usd").and_then(|x| x.as_f64()) {
        println!("cost_usd: {cost}");
    }
    if let Some(msg) = final_message {
        println!("final_message: {msg}");
    }

    if let Some(failures) = summary_val.get("gate_failures").and_then(|x| x.as_array()) {
        let entries: Vec<&str> = failures.iter().filter_map(|x| x.as_str()).collect();
        if let Some(last) = entries.last() {
            println!("== check failures");
            if entries.len() > 1 {
                println!("({} failed attempts; the last one)", entries.len());
            }
            println!("{last}");
        }
    }

    if !checks.hcmds.is_empty() {
        println!("== host commands (may have changed things outside the worktree)");
        for cmd in checks.hcmds {
            println!("{cmd}");
        }
    }
    if !checks.reads.is_empty() {
        println!("== reads outside the repository");
        for cmd in checks.reads {
            println!("{cmd}");
        }
    }

    if let Some(diff) = diff_content {
        print!("{}", format_diff_section(&diff));
    }

    Ok(())
}

enum UndoAction {
    Restore {
        rel_path: String,
        target: PathBuf,
        before_bytes: Vec<u8>,
    },
    Remove {
        rel_path: String,
        target: PathBuf,
    },
    AlreadyRestored {
        rel_path: String,
    },
    AlreadyRemoved {
        rel_path: String,
    },
}

pub fn undo(args: &UndoArgs) -> i32 {
    match undo_inner(args) {
        Ok(()) => 0,
        Err(msg) => {
            if msg.starts_with("error:") {
                eprintln!("{msg}");
            } else {
                eprintln!("error: {msg}");
            }
            2
        }
    }
}

fn undo_inner(args: &UndoArgs) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("{e}"))?;
    let lines = undo_in(&cwd, &args.id)?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

pub fn undo_in(cwd: &Path, id: &str) -> Result<Vec<String>, String> {
    let root = cwd
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", cwd.display()))?;

    let git_out = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if git_out.status.success() {
        return Err("undo is for runs in a folder that is not a git repository".to_string());
    }

    let state_dir = crate::surface_registry::state_dir()?;
    let store = crate::surface_dir::run_store(&state_dir, &root)?;
    let lock_path = crate::surface_dir::in_place_lock_path(&state_dir, &root)?;

    let start_path = store.join(format!("{id}.start.json"));
    let start_val =
        read_required_json(&start_path)?.ok_or_else(|| format!("no run {id} in this folder"))?;

    let summary_path = store.join(format!("{id}.json"));
    let summary_val =
        read_required_json(&summary_path)?.ok_or_else(|| format!("run {id} has not finished"))?;

    if parse_research_flag(&start_val, &start_path)? {
        return Err(format!(
            "run {id} is a research run and made no edits to undo"
        ));
    }

    let undone_path = store.join(format!("{id}.undone.json"));
    if undone_path.exists() {
        return Err(format!("run {id} was already undone"));
    }

    let holder = blyte_agent::folder_lock::Holder {
        pid: std::process::id(),
        kind: "undo".to_string(),
        id: id.to_string(),
    };
    let _lock = blyte_agent::folder_lock::acquire(&lock_path, &holder)?;

    if undone_path.exists() {
        return Err(format!("run {id} was already undone"));
    }

    let manifest_path = store.join(format!("{id}.changes.json"));
    let manifest = read_manifest_map(&manifest_path, &summary_val, &summary_path)?;

    let changed_set: HashSet<String> = summary_val
        .get("changed_files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("missing changed_files in {}", summary_path.display()))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let before_dir = store.join(format!("{id}.before"));
    let after_dir = store.join(format!("{id}.after"));

    let mut actions = Vec::with_capacity(manifest.len());
    let mut drifted = Vec::new();

    for (rel_path, existed) in &manifest {
        let rel = validate_rel_path(rel_path)?;
        let target = root.join(&rel);
        let current = read_optional_bytes(&target)?;
        let after_path = after_dir.join(&rel);
        let after_bytes = if changed_set.contains(rel_path) {
            Some(std::fs::read(&after_path).map_err(|e| {
                format!(
                    "missing after-copy for {rel_path} at {}: {e}",
                    after_path.display()
                )
            })?)
        } else {
            read_optional_bytes(&after_path)?
        };

        if *existed {
            let before_path = before_dir.join(&rel);
            let before_bytes = std::fs::read(&before_path).map_err(|e| {
                format!(
                    "missing saved original for {rel_path} at {}: {e}",
                    before_path.display()
                )
            })?;
            if let Some(ref cur) = current {
                if cur == &before_bytes {
                    actions.push(UndoAction::AlreadyRestored {
                        rel_path: rel_path.clone(),
                    });
                } else if after_bytes.as_ref().is_some_and(|a| cur == a) {
                    actions.push(UndoAction::Restore {
                        rel_path: rel_path.clone(),
                        target,
                        before_bytes,
                    });
                } else {
                    drifted.push(rel_path.clone());
                }
            } else {
                drifted.push(rel_path.clone());
            }
        } else if current.is_none() {
            actions.push(UndoAction::AlreadyRemoved {
                rel_path: rel_path.clone(),
            });
        } else if let Some(ref cur) = current {
            if after_bytes.as_ref().is_some_and(|a| cur == a) {
                actions.push(UndoAction::Remove {
                    rel_path: rel_path.clone(),
                    target,
                });
            } else {
                drifted.push(rel_path.clone());
            }
        }
    }

    if !drifted.is_empty() {
        return Err(format!(
            "changed since run {id} ended: {}",
            drifted.join(", ")
        ));
    }

    let mut lines = Vec::with_capacity(actions.len());
    for action in actions {
        match action {
            UndoAction::Restore {
                rel_path,
                target,
                before_bytes,
            } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
                std::fs::write(&target, &before_bytes)
                    .map_err(|e| format!("cannot restore {}: {e}", target.display()))?;
                lines.push(format!("restored {rel_path}"));
            }
            UndoAction::Remove { rel_path, target } => {
                std::fs::remove_file(&target)
                    .map_err(|e| format!("cannot remove {}: {e}", target.display()))?;
                lines.push(format!("removed {rel_path}"));
            }
            UndoAction::AlreadyRestored { rel_path } => {
                lines.push(format!("restored {rel_path} (already restored)"));
            }
            UndoAction::AlreadyRemoved { rel_path } => {
                lines.push(format!("removed {rel_path} (already removed)"));
            }
        }
    }

    let undone_at = crate::dispatch_in_place::unix_timestamp_secs(std::time::SystemTime::now())?;
    let marker = serde_json::json!({ "undone_at": undone_at });
    let text = serde_json::to_string_pretty(&marker)
        .map_err(|e| format!("cannot serialize undo marker: {e}"))?;
    std::fs::write(&undone_path, text + "\n")
        .map_err(|e| format!("cannot write {}: {e}", undone_path.display()))?;

    drop(_lock);
    Ok(lines)
}
