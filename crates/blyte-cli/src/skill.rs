use std::path::{Path, PathBuf};
use std::process::Command;

pub const SKILL_MD: &str = include_str!("../assets/blyte-implementer/SKILL.md");

pub const DEFAULT_BLOCK: &str = "\
<!-- blyte:default -->
## Implementation: delegate to blyte, in parallel

For code changes in a git repository that has a check command (tests, lint), plan and review
the work yourself and delegate the implementation to blyte with the `blyte-implementer` skill.
Split the work into independent tasks and dispatch them in parallel (`blyte dispatch`, one
worktree each); run tasks one after another only when one depends on another. Keep for
yourself: design decisions, ambiguous requirements, one-line edits, and anything with no check.
<!-- /blyte:default -->";

pub const CONTEXT_BLOCK: &str = "\
<!-- blyte:context -->
## Context: keep this session small

Everything a tool returns stays in the conversation and is re-read on every turn, so it is most of
what a session costs.
- Trim command output at the source: `| tail -n 20`, `grep` for the lines you need, `sed -n`
  ranges, `--quiet`/`-q`; `git diff --stat` before any full diff.
- Read files by range (offset and limit), not whole, once you know where to look.
- For a dispatched run, read `blyte review <id>` and the JSON fields you need
  (`jq -r .outcome .wt/<id>.json`), not the whole summary or log; search `.wt/<id>.log` with
  `grep`/`tail` only for what `== checks` flags.
- Write tasks as `### Task <ID>` sections of the plan and dispatch with `--plan`/`--plan-task`;
  do not write task files that copy the plan.
- An investigation needing more than about five file reads goes to `blyte dispatch --research`;
  read its report, not the files.
- One session per piece of work. When the status line shows the context past about 60% of its
  window, tell the user to `/compact` or start a new session before going on.
<!-- /blyte:context -->";

pub const CODEX_RULES: &str = "\
# Written by `blyte skill install --harness codex`; removed by `blyte skill uninstall --harness codex`.
# blyte dispatch needs the network and applies its own sandbox, so it runs outside Codex's.
prefix_rule(pattern=[\"blyte\", \"dispatch\"], decision=\"allow\")
prefix_rule(pattern=[\"blyte\", \"review\"], decision=\"allow\")
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub fn parse_all(s: &str) -> Result<Vec<Self>, String> {
        match s {
            "claude" => Ok(vec![Harness::Claude]),
            "codex" => Ok(vec![Harness::Codex]),
            "all" => Ok(vec![Harness::Claude, Harness::Codex]),
            other => Err(format!("unknown harness: {other}")),
        }
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "not a git repository: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

pub fn home_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME environment variable is unset or empty".to_string())?;
    if home.trim().is_empty() {
        return Err("HOME environment variable is unset or empty".to_string());
    }
    Ok(PathBuf::from(home))
}

pub fn skill_dir(project: bool) -> Result<PathBuf, String> {
    if project {
        Ok(repo_root()?.join(".claude/skills/blyte-implementer"))
    } else {
        Ok(home_dir()?.join(".claude/skills/blyte-implementer"))
    }
}

pub fn claude_md_path(project: bool) -> Result<PathBuf, String> {
    if project {
        Ok(repo_root()?.join("CLAUDE.md"))
    } else {
        Ok(home_dir()?.join(".claude/CLAUDE.md"))
    }
}

pub fn skill_dirs(harness: Harness, project: bool) -> Result<Vec<PathBuf>, String> {
    match harness {
        Harness::Claude => Ok(vec![skill_dir(project)?]),
        Harness::Codex => {
            if project {
                Ok(vec![repo_root()?.join(".agents/skills/blyte-implementer")])
            } else {
                let home = home_dir()?;
                Ok(vec![
                    home.join(".agents/skills/blyte-implementer"),
                    home.join(".codex/skills/blyte-implementer"),
                ])
            }
        }
    }
}

pub fn instructions_path(harness: Harness, project: bool) -> Result<PathBuf, String> {
    match harness {
        Harness::Claude => claude_md_path(project),
        Harness::Codex => {
            if project {
                Ok(repo_root()?.join("AGENTS.md"))
            } else {
                Ok(home_dir()?.join(".codex/AGENTS.md"))
            }
        }
    }
}

pub fn codex_rules_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex/rules/blyte.rules"))
}

pub fn set_block(text: &str, block: &str) -> String {
    let start_marker = block.lines().next().unwrap_or("");
    let end_marker = block.lines().last().unwrap_or("");

    if !start_marker.is_empty() && !end_marker.is_empty() {
        if let Some(start) = text.find(start_marker) {
            if let Some(end_rel) = text[start..].find(end_marker) {
                let end = start + end_rel + end_marker.len();
                let mut result = format!("{}{block}{}", &text[..start], &text[end..]);
                while result.ends_with("\n\n") {
                    result.pop();
                }
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                return result;
            }
        }
    }

    if text.is_empty() {
        format!("{block}\n")
    } else if text.ends_with('\n') {
        format!("{text}\n{block}\n")
    } else {
        format!("{text}\n\n{block}\n")
    }
}

pub fn remove_block(text: &str, block: &str) -> String {
    let start_marker = block.lines().next().unwrap_or("");
    let end_marker = block.lines().last().unwrap_or("");

    if start_marker.is_empty() || end_marker.is_empty() {
        return text.to_string();
    }

    let Some(start) = text.find(start_marker) else {
        return text.to_string();
    };
    let Some(end_rel) = text[start..].find(end_marker) else {
        return text.to_string();
    };
    let end = start + end_rel + end_marker.len();

    let prefix = &text[..start];
    let prefix_clean = if prefix.ends_with("\n\n") {
        &prefix[..prefix.len() - 1]
    } else if prefix == "\n" {
        ""
    } else {
        prefix
    };

    let suffix = if text[end..].starts_with('\n') {
        &text[end + 1..]
    } else {
        &text[end..]
    };

    format!("{prefix_clean}{suffix}")
}

pub fn set_default_block(text: &str) -> String {
    set_block(text, DEFAULT_BLOCK)
}

pub fn remove_default_block(text: &str) -> String {
    remove_block(text, DEFAULT_BLOCK)
}

pub fn ensure_context_block(home: &Path) -> Result<Option<PathBuf>, String> {
    let claude_dir = home.join(".claude");
    if !claude_dir.exists() {
        return Ok(None);
    }
    let claude_md = claude_dir.join("CLAUDE.md");
    let existing = if claude_md.exists() {
        std::fs::read_to_string(&claude_md)
            .map_err(|e| format!("failed to read {}: {e}", claude_md.display()))?
    } else {
        String::new()
    };
    let updated = set_block(&existing, CONTEXT_BLOCK);
    if updated != existing {
        std::fs::write(&claude_md, updated)
            .map_err(|e| format!("failed to write {}: {e}", claude_md.display()))?;
        Ok(Some(claude_md))
    } else {
        Ok(None)
    }
}

pub const STATUS_LINE_REFRESH_SECS: u64 = 30;

pub fn set_status_line(settings: &str) -> Result<Option<String>, String> {
    let mut val: serde_json::Value = if settings.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(settings).map_err(|e| e.to_string())?
    };

    let Some(obj) = val.as_object_mut() else {
        return Err("settings must be a JSON object".to_string());
    };

    if let Some(sl) = obj.get_mut("statusLine").and_then(|v| v.as_object_mut()) {
        let cmd = sl.get("command").and_then(|c| c.as_str());
        if let Some(cmd_str) = cmd {
            if cmd_str.starts_with("blyte statusline") {
                if sl.get("refreshInterval").is_some_and(|v| !v.is_null()) {
                    return Ok(None);
                }
                sl.insert(
                    "refreshInterval".to_string(),
                    serde_json::json!(STATUS_LINE_REFRESH_SECS),
                );
                let mut out = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
                out.push('\n');
                return Ok(Some(out));
            }
        }

        let command_str = match cmd {
            Some(old) => format!("blyte statusline --after '{}'", old.replace('\'', r"'\''")),
            None => "blyte statusline".to_string(),
        };

        sl.insert(
            "type".to_string(),
            serde_json::Value::String("command".to_string()),
        );
        sl.insert(
            "command".to_string(),
            serde_json::Value::String(command_str),
        );
        if sl
            .get("refreshInterval")
            .is_none_or(serde_json::Value::is_null)
        {
            sl.insert(
                "refreshInterval".to_string(),
                serde_json::json!(STATUS_LINE_REFRESH_SECS),
            );
        }
    } else {
        let mut sl_map = serde_json::Map::new();
        sl_map.insert(
            "type".to_string(),
            serde_json::Value::String("command".to_string()),
        );
        sl_map.insert(
            "command".to_string(),
            serde_json::Value::String("blyte statusline".to_string()),
        );
        sl_map.insert(
            "refreshInterval".to_string(),
            serde_json::json!(STATUS_LINE_REFRESH_SECS),
        );
        obj.insert("statusLine".to_string(), serde_json::Value::Object(sl_map));
    }

    let mut out = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(Some(out))
}

pub fn remove_status_line(settings: &str) -> Result<Option<String>, String> {
    let mut val: serde_json::Value = if settings.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(settings).map_err(|e| e.to_string())?
    };

    let Some(obj) = val.as_object_mut() else {
        return Err("settings must be a JSON object".to_string());
    };

    let Some(sl) = obj.get("statusLine") else {
        return Ok(None);
    };

    let Some(cmd) = sl.get("command").and_then(|c| c.as_str()) else {
        return Ok(None);
    };

    const PREFIX: &str = "blyte statusline --after '";
    if cmd == "blyte statusline" {
        obj.remove("statusLine");
    } else if let Some(rest) = cmd.strip_prefix(PREFIX) {
        if let Some(inside) = rest.strip_suffix('\'') {
            let old = inside.replace(r"'\''", "'");
            if let Some(sl_obj) = obj.get_mut("statusLine").and_then(|v| v.as_object_mut()) {
                sl_obj.insert("command".to_string(), serde_json::Value::String(old));
                if sl_obj.get("refreshInterval").and_then(|v| v.as_u64())
                    == Some(STATUS_LINE_REFRESH_SECS)
                {
                    sl_obj.remove("refreshInterval");
                }
            } else {
                return Ok(None);
            }
        } else {
            return Ok(None);
        }
    } else {
        return Ok(None);
    }

    let mut out = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(Some(out))
}

pub fn install_for(
    harness: Harness,
    project: bool,
    force: bool,
    no_default: bool,
    no_statusline: bool,
) -> Result<Vec<PathBuf>, String> {
    let claude_context_written = if harness == Harness::Claude && !project {
        let home = home_dir()?;
        ensure_context_block(&home)?
    } else {
        None
    };

    let dirs = skill_dirs(harness, project)?;
    let skill_paths: Vec<PathBuf> = dirs.iter().map(|d| d.join("SKILL.md")).collect();

    for skill_path in &skill_paths {
        if skill_path.exists() {
            let existing = std::fs::read_to_string(skill_path)
                .map_err(|e| format!("failed to read {}: {e}", skill_path.display()))?;
            if existing != SKILL_MD && !force {
                return Err(format!(
                    "{} already exists with different content; pass --force to overwrite",
                    skill_path.display()
                ));
            }
        }
    }

    let mut written = Vec::new();
    for (dir, skill_path) in dirs.into_iter().zip(skill_paths) {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("failed to create directory {}: {e}", dir.display()))?;
        std::fs::write(&skill_path, SKILL_MD)
            .map_err(|e| format!("failed to write {}: {e}", skill_path.display()))?;
        let marker = dir.join(".blyte-installed");
        std::fs::write(&marker, SKILL_MD)
            .map_err(|e| format!("failed to write {}: {e}", marker.display()))?;
        written.push(skill_path);
    }

    let md_path = instructions_path(harness, project)?;
    if !no_default {
        if let Some(parent) = md_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
        let existing = if md_path.exists() {
            std::fs::read_to_string(&md_path)
                .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?
        } else {
            String::new()
        };
        let updated = set_default_block(&existing);
        std::fs::write(&md_path, updated)
            .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
        if !written.contains(&md_path) {
            written.push(md_path);
        }
    } else if md_path.exists() {
        // Switching to "only when asked" drops the default block an earlier install wrote, so it
        // does not need an uninstall first (2026-09-30).
        let existing = std::fs::read_to_string(&md_path)
            .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?;
        let updated = remove_default_block(&existing);
        if updated != existing {
            std::fs::write(&md_path, updated)
                .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
            if !written.contains(&md_path) {
                written.push(md_path);
            }
        }
    }

    if let Some(path) = claude_context_written {
        if !written.contains(&path) {
            written.push(path);
        }
    }

    if harness == Harness::Codex && !project {
        let rules_path = codex_rules_path()?;
        if let Some(parent) = rules_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        }
        std::fs::write(&rules_path, CODEX_RULES)
            .map_err(|e| format!("failed to write {}: {e}", rules_path.display()))?;
        written.push(rules_path);
    }

    if harness == Harness::Claude && !project && !no_statusline {
        let home = home_dir()?;
        let claude_dir = home.join(".claude");
        let settings_path = claude_dir.join("settings.json");
        let backup_path = claude_dir.join("settings.json.blyte-backup");

        let existing = if settings_path.exists() {
            match std::fs::read_to_string(&settings_path) {
                Ok(s) => s,
                Err(e) => {
                    return Err(format!("failed to read {}: {e}", settings_path.display()));
                }
            }
        } else {
            String::new()
        };

        match set_status_line(&existing) {
            Ok(Some(updated)) => {
                if settings_path.exists() && !backup_path.exists() {
                    std::fs::copy(&settings_path, &backup_path).map_err(|e| {
                        format!("failed to create backup {}: {e}", backup_path.display())
                    })?;
                }
                if let Some(parent) = settings_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!("failed to create directory {}: {e}", parent.display())
                    })?;
                }
                std::fs::write(&settings_path, &updated)
                    .map_err(|e| format!("failed to write {}: {e}", settings_path.display()))?;

                if updated.contains("blyte statusline --after") {
                    println!("status line: wrapped your status line command");
                } else {
                    println!("status line: set to blyte statusline");
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("warning: could not update {}: {e}", settings_path.display());
            }
        }
    }

    Ok(written)
}

pub fn uninstall_for(harness: Harness, project: bool) -> Result<Vec<PathBuf>, String> {
    let dirs = skill_dirs(harness, project)?;
    let mut changed = Vec::new();

    for dir in dirs {
        let marker = dir.join(".blyte-installed");
        if marker.exists() {
            std::fs::remove_file(&marker)
                .map_err(|e| format!("failed to remove {}: {e}", marker.display()))?;
        }

        let skill_path = dir.join("SKILL.md");
        if skill_path.exists() {
            std::fs::remove_file(&skill_path)
                .map_err(|e| format!("failed to remove {}: {e}", skill_path.display()))?;
            changed.push(skill_path);
        }

        if dir.exists() {
            if let Ok(mut entries) = std::fs::read_dir(&dir) {
                if entries.next().is_none() {
                    let _ = std::fs::remove_dir(&dir);
                }
            }
        }
    }

    let md_path = instructions_path(harness, project)?;
    if md_path.exists() {
        let existing = std::fs::read_to_string(&md_path)
            .map_err(|e| format!("failed to read {}: {e}", md_path.display()))?;
        let mut updated = remove_default_block(&existing);
        if harness == Harness::Claude && !project {
            updated = remove_block(&updated, CONTEXT_BLOCK);
        }
        if updated != existing {
            std::fs::write(&md_path, updated)
                .map_err(|e| format!("failed to write {}: {e}", md_path.display()))?;
            changed.push(md_path);
        }
    }

    if harness == Harness::Codex && !project {
        let rules_path = codex_rules_path()?;
        if rules_path.exists() {
            std::fs::remove_file(&rules_path)
                .map_err(|e| format!("failed to remove {}: {e}", rules_path.display()))?;
            if let Some(parent) = rules_path.parent() {
                if parent.exists() {
                    if let Ok(mut entries) = std::fs::read_dir(parent) {
                        if entries.next().is_none() {
                            let _ = std::fs::remove_dir(parent);
                        }
                    }
                }
            }
            changed.push(rules_path);
        }
    }

    if harness == Harness::Claude && !project {
        let home = home_dir()?;
        let settings_path = home.join(".claude/settings.json");
        if settings_path.exists() {
            if let Ok(existing) = std::fs::read_to_string(&settings_path) {
                match remove_status_line(&existing) {
                    Ok(Some(updated)) => {
                        std::fs::write(&settings_path, updated).map_err(|e| {
                            format!("failed to write {}: {e}", settings_path.display())
                        })?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("warning: could not update {}: {e}", settings_path.display());
                    }
                }
            }
        }
    }

    Ok(changed)
}

pub fn install(
    project: bool,
    force: bool,
    no_default: bool,
    no_statusline: bool,
) -> Result<PathBuf, String> {
    let written = install_for(Harness::Claude, project, force, no_default, no_statusline)?;
    Ok(written.into_iter().next().unwrap())
}

pub fn uninstall(project: bool) -> Result<Vec<PathBuf>, String> {
    uninstall_for(Harness::Claude, project)
}

/// Directories that hold an older, separate `blyte-research` skill superseded by `blyte-implementer`.
pub fn superseded_skills(home: &Path) -> Vec<PathBuf> {
    let candidates = [
        home.join(".claude/skills/blyte-research"),
        home.join(".codex/skills/blyte-research"),
        home.join(".agents/skills/blyte-research"),
    ];
    let mut found = Vec::new();
    for p in candidates {
        if p.exists() && !found.contains(&p) {
            found.push(p);
        }
    }
    found
}

pub fn superseded_warning(path: &Path) -> String {
    format!(
        "blyte: {} is superseded by the blyte-implementer skill's research section (default 300 turns); remove it: rm -r {}",
        path.display(),
        path.display()
    )
}

pub fn warn_superseded(home: &Path) {
    for path in superseded_skills(home) {
        eprintln!("{}", superseded_warning(&path));
    }
}

pub const RECOMMENDED_MODEL: &str = "gemini-4-argon";
pub const FLASH_MODEL: &str = "gemini-3.8-flash-high";
pub const ARGON_TIMEOUT_SECS: &str = "600";

fn parse_model_choice(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() || s == "1" {
        return Some(RECOMMENDED_MODEL.to_string());
    }
    if s == "2" {
        return Some(FLASH_MODEL.to_string());
    }
    if s.len() < 2 || s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | ':' | '.'))
    {
        return None;
    }
    Some(s.to_string())
}

/// Prompts for the default model on `writer` and reads the answer from `reader`. Re-asks once on
/// an answer that is not `1`, `2`, empty, or a valid model id.
pub fn prompt_model<R: std::io::BufRead, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
) -> Result<String, String> {
    write!(
        writer,
        "Default implementer model:\n  \
         1) {RECOMMENDED_MODEL} (recommended: passes more gates in fewer turns and lower total cost)\n  \
         2) {FLASH_MODEL} (cheaper per token, more turns)\n  \
         3) any other model id, typed in\n\
         Choice [1]: "
    )
    .and_then(|_| writer.flush())
    .map_err(|e| e.to_string())?;

    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if let Some(chosen) = parse_model_choice(&line) {
        return Ok(chosen);
    }

    write!(
        writer,
        "Enter 1, 2, or a model id (default {RECOMMENDED_MODEL}) [1]: "
    )
    .and_then(|_| writer.flush())
    .map_err(|e| e.to_string())?;

    line.clear();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(parse_model_choice(&line).unwrap_or_else(|| RECOMMENDED_MODEL.to_string()))
}

fn write_model_env<W: std::io::Write>(
    env_path: &Path,
    model: &str,
    writer: &mut W,
) -> Result<(), String> {
    if model.to_ascii_lowercase().contains("argon") {
        crate::env_file::set(
            env_path,
            &[
                ("BLYTE_MODEL", model),
                ("BLYTE_MODEL_TIMEOUT_SECS", ARGON_TIMEOUT_SECS),
            ],
        )?;
        writeln!(
            writer,
            "wrote BLYTE_MODEL={model} and BLYTE_MODEL_TIMEOUT_SECS={ARGON_TIMEOUT_SECS} to {}",
            env_path.display()
        )
        .map_err(|e| e.to_string())?;
    } else {
        crate::env_file::set(env_path, &[("BLYTE_MODEL", model)])?;
        writeln!(
            writer,
            "wrote BLYTE_MODEL={model} to {}",
            env_path.display()
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Sets or reports the default implementer model in `env_path` during `blyte skill install`.
pub fn configure_model<R: std::io::BufRead, W: std::io::Write>(
    env_path: &Path,
    model_flag: Option<&str>,
    is_terminal: bool,
    reader: &mut R,
    writer: &mut W,
) -> Result<Option<String>, String> {
    if let Some(model) = model_flag {
        let model = model.trim();
        if model.is_empty() {
            return Err("--model cannot be empty".to_string());
        }
        write_model_env(env_path, model, writer)?;
        return Ok(Some(model.to_string()));
    }

    if let Some(existing) = crate::env_file::get(env_path, "BLYTE_MODEL")? {
        writeln!(writer, "BLYTE_MODEL={existing} (in {})", env_path.display())
            .map_err(|e| e.to_string())?;
        return Ok(Some(existing));
    }

    if !is_terminal {
        return Ok(None);
    }

    let chosen = prompt_model(reader, writer)?;
    write_model_env(env_path, &chosen, writer)?;
    Ok(Some(chosen))
}
