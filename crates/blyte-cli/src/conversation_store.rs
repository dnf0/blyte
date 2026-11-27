use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use blyte_provider::Message;
use serde::Serialize;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConversation {
    pub model: Option<String>,
    pub goal: Option<String>,
    pub messages: Vec<Message>,
}

pub struct ConversationStore {
    path: PathBuf,
    last_model: Option<String>,
    last_goal: Option<String>,
    saved_messages: usize,
    file_initialized: bool,
}

#[derive(Serialize)]
struct VersionLine {
    version: u64,
}

#[derive(Serialize)]
struct ModelLine<'a> {
    model: &'a str,
}

#[derive(Serialize)]
struct GoalLine<'a> {
    goal: Option<&'a str>,
}

#[derive(Serialize)]
struct MessageLine<'a> {
    message: &'a Message,
}

/// Returns `<surface token dir>/<surface id>.conversation.jsonl`.
pub fn conversation_path_for_id(surface_id: &str) -> Result<PathBuf, String> {
    blyte_agent::sandbox::validate_surface_id(surface_id)?;
    let dir = blyte_agent::sandbox::surface_token_dir().ok_or_else(|| {
        "cannot determine surface token directory: neither XDG_STATE_HOME nor HOME is set"
            .to_string()
    })?;
    Ok(dir.join(format!("{surface_id}.conversation.jsonl")))
}

fn format_read_error(path: &Path, line_no: usize, reason: &str) -> String {
    format!(
        "cannot read the saved conversation {} (line {line_no}): {reason}; delete that file to resume without the model's memory",
        path.display()
    )
}

fn push_json_line<T: Serialize>(buf: &mut Vec<u8>, value: &T) -> Result<(), String> {
    serde_json::to_writer(&mut *buf, value)
        .map_err(|e| format!("failed to serialize conversation line: {e}"))?;
    buf.push(b'\n');
    Ok(())
}

fn encode_full_file(
    model: Option<&str>,
    goal: Option<&str>,
    messages: &[Message],
) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    push_json_line(&mut buf, &VersionLine { version: 1 })?;
    if let Some(m) = model {
        push_json_line(&mut buf, &ModelLine { model: m })?;
    }
    if goal.is_some() {
        push_json_line(&mut buf, &GoalLine { goal })?;
    }
    for message in messages {
        push_json_line(&mut buf, &MessageLine { message })?;
    }
    Ok(buf)
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_file_mode_0600(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot set permissions on {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_file_mode_0600(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn open_append_0600(path: &Path) -> Result<std::fs::File, String> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))
}

fn rewrite_atomic(
    path: &Path,
    model: Option<&str>,
    goal: Option<&str>,
    messages: &[Message],
) -> Result<(), String> {
    ensure_parent_dir(path)?;
    let buf = encode_full_file(model, goal, messages)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("conversation.jsonl");
    let pid = std::process::id();
    let mut attempt = 0u64;
    let (tmp_path, mut file) = loop {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{base_name}.tmp.{pid}.{seq}.{attempt}"));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&candidate) {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt += 1;
            }
            Err(e) => {
                return Err(format!("cannot create {}: {e}", candidate.display()));
            }
        }
    };
    let write_res = file
        .write_all(&buf)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("cannot write {}: {e}", tmp_path.display()));
    }
    if let Err(e) = set_file_mode_0600(&tmp_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "cannot rename {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ));
    }
    Ok(())
}

fn strip_trailing_unmatched_tool_calls(messages: &mut Vec<Message>) -> bool {
    let mut changed = false;
    loop {
        let results_start = messages
            .iter()
            .rposition(|m| !matches!(m, Message::ToolResult { .. }))
            .map(|i| i + 1)
            .unwrap_or(0);
        let calls_start = messages[..results_start]
            .iter()
            .rposition(|m| !matches!(m, Message::ToolCall { .. }))
            .map(|i| i + 1)
            .unwrap_or(0);
        let num_calls = results_start - calls_start;
        let num_results = messages.len() - results_start;
        if num_calls > num_results {
            messages.drain(calls_start + num_results..results_start);
            changed = true;
        } else {
            break;
        }
    }
    changed
}

fn parse_version_line(path: &Path, line: &str) -> Result<(), String> {
    let val: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format_read_error(path, 1, &e.to_string()))?;
    let Some(map) = val.as_object() else {
        return Err(format_read_error(
            path,
            1,
            "first line must be {\"version\":1}",
        ));
    };
    if map.len() != 1 {
        if let Some(unknown) = map.keys().find(|k| k.as_str() != "version") {
            return Err(format_read_error(
                path,
                1,
                &format!("unknown key \"{unknown}\""),
            ));
        }
        return Err(format_read_error(
            path,
            1,
            "first line must be {\"version\":1}",
        ));
    }
    match map.get("version") {
        Some(v) if v.as_u64() == Some(1) => Ok(()),
        Some(v) => Err(format_read_error(
            path,
            1,
            &format!("unsupported version {v}; expected 1"),
        )),
        None => {
            if let Some(unknown) = map.keys().next() {
                Err(format_read_error(
                    path,
                    1,
                    &format!("unknown key \"{unknown}\""),
                ))
            } else {
                Err(format_read_error(
                    path,
                    1,
                    "first line must be {\"version\":1}",
                ))
            }
        }
    }
}

impl ConversationStore {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn new_for_fresh(path: PathBuf) -> Result<Self, String> {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Err(format!(
                "conversation file already exists for new surface: {}",
                path.display()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path,
                last_model: None,
                last_goal: None,
                saved_messages: 0,
                file_initialized: false,
            }),
            Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
        }
    }

    pub fn load(path: PathBuf) -> Result<(Self, Option<LoadedConversation>), String> {
        let raw = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((
                    Self {
                        path,
                        last_model: None,
                        last_goal: None,
                        saved_messages: 0,
                        file_initialized: false,
                    },
                    None,
                ));
            }
            Err(e) => return Err(format_read_error(&path, 1, &e.to_string())),
        };

        let (complete_bytes, truncate_to) = if !raw.is_empty() && !raw.ends_with(b"\n") {
            let cut = raw
                .iter()
                .rposition(|&b| b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            (&raw[..cut], Some(cut as u64))
        } else {
            (&raw[..], None)
        };

        let lines: Vec<&[u8]> = if complete_bytes.is_empty() {
            Vec::new()
        } else {
            let trimmed = complete_bytes.strip_suffix(b"\n").unwrap_or(complete_bytes);
            trimmed.split(|&b| b == b'\n').collect()
        };

        let Some(first_raw) = lines.first() else {
            return Err(format_read_error(
                &path,
                1,
                "first line must be {\"version\":1}",
            ));
        };
        let first_str = std::str::from_utf8(first_raw)
            .map_err(|e| format_read_error(&path, 1, &e.to_string()))?;
        parse_version_line(&path, first_str)?;

        let mut model: Option<String> = None;
        let mut goal: Option<String> = None;
        let mut messages: Vec<Message> = Vec::new();

        for (idx, raw_line) in lines.iter().enumerate().skip(1) {
            let line_no = idx + 1;
            let line_str = std::str::from_utf8(raw_line)
                .map_err(|e| format_read_error(&path, line_no, &e.to_string()))?;
            let val: serde_json::Value = serde_json::from_str(line_str)
                .map_err(|e| format_read_error(&path, line_no, &e.to_string()))?;
            let serde_json::Value::Object(map) = val else {
                return Err(format_read_error(&path, line_no, "expected a JSON object"));
            };
            if map.len() != 1 {
                if let Some(unknown) = map
                    .keys()
                    .find(|k| !matches!(k.as_str(), "model" | "goal" | "message"))
                {
                    return Err(format_read_error(
                        &path,
                        line_no,
                        &format!("unknown key \"{unknown}\""),
                    ));
                }
                return Err(format_read_error(
                    &path,
                    line_no,
                    "expected a JSON object with exactly one key",
                ));
            }
            let (key, value) = map.into_iter().next().unwrap();
            match key.as_str() {
                "model" => {
                    let parsed: String = serde_json::from_value(value)
                        .map_err(|e| format_read_error(&path, line_no, &e.to_string()))?;
                    model = Some(parsed);
                }
                "goal" => {
                    let parsed: Option<String> = serde_json::from_value(value)
                        .map_err(|e| format_read_error(&path, line_no, &e.to_string()))?;
                    goal = parsed;
                }
                "message" => {
                    let parsed: Message = serde_json::from_value(value)
                        .map_err(|e| format_read_error(&path, line_no, &e.to_string()))?;
                    messages.push(parsed);
                }
                other => {
                    return Err(format_read_error(
                        &path,
                        line_no,
                        &format!("unknown key \"{other}\""),
                    ));
                }
            }
        }

        let trimmed_tool_calls = strip_trailing_unmatched_tool_calls(&mut messages);
        if trimmed_tool_calls {
            rewrite_atomic(&path, model.as_deref(), goal.as_deref(), &messages)?;
        } else if let Some(cut) = truncate_to {
            let f = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|e| format!("cannot truncate {}: {e}", path.display()))?;
            f.set_len(cut)
                .and_then(|()| f.sync_all())
                .map_err(|e| format!("cannot truncate {}: {e}", path.display()))?;
        }

        let store = Self {
            path,
            last_model: model.clone(),
            last_goal: goal.clone(),
            saved_messages: messages.len(),
            file_initialized: true,
        };
        Ok((
            store,
            Some(LoadedConversation {
                model,
                goal,
                messages,
            }),
        ))
    }

    pub fn save(
        &mut self,
        model: &str,
        goal: Option<&str>,
        messages: &[Message],
    ) -> Result<(), String> {
        if self.file_initialized && messages.len() < self.saved_messages {
            rewrite_atomic(&self.path, Some(model), goal, messages)?;
            self.last_model = Some(model.to_string());
            self.last_goal = goal.map(str::to_string);
            self.saved_messages = messages.len();
            return Ok(());
        }

        let mut buf = Vec::new();
        if !self.file_initialized {
            push_json_line(&mut buf, &VersionLine { version: 1 })?;
        }
        if self.last_model.as_deref() != Some(model) {
            push_json_line(&mut buf, &ModelLine { model })?;
        }
        if self.last_goal.as_deref() != goal {
            push_json_line(&mut buf, &GoalLine { goal })?;
        }
        for message in &messages[self.saved_messages..] {
            push_json_line(&mut buf, &MessageLine { message })?;
        }

        if buf.is_empty() && self.file_initialized {
            return Ok(());
        }

        if !self.file_initialized {
            ensure_parent_dir(&self.path)?;
        }
        let mut file = open_append_0600(&self.path)?;
        if !self.file_initialized {
            set_file_mode_0600(&self.path)?;
        }
        file.write_all(&buf)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))?;

        self.file_initialized = true;
        self.last_model = Some(model.to_string());
        self.last_goal = goal.map(str::to_string);
        self.saved_messages = messages.len();
        Ok(())
    }

    pub fn rewrite(
        &mut self,
        model: Option<&str>,
        goal: Option<&str>,
        messages: &[Message],
    ) -> Result<(), String> {
        rewrite_atomic(&self.path, model, goal, messages)?;
        self.file_initialized = true;
        self.last_model = model.map(str::to_string);
        self.last_goal = goal.map(str::to_string);
        self.saved_messages = messages.len();
        Ok(())
    }
}

/// Replaces loaded messages with a single digest message and rewrites the store when the starting
/// model's provider kind differs from the last recorded model's provider kind.
pub fn maybe_digest_for_provider(
    store: &mut ConversationStore,
    loaded: &mut LoadedConversation,
    starting_model: &str,
) -> Result<bool, String> {
    if loaded.messages.is_empty() {
        return Ok(false);
    }
    let Some(ref saved_model) = loaded.model else {
        return Ok(false);
    };
    let saved_kind = blyte_provider::detect_provider_kind(Some(saved_model.as_str()));
    let starting_kind = blyte_provider::detect_provider_kind(Some(starting_model));
    if saved_kind == starting_kind {
        return Ok(false);
    }
    let summary = blyte_agent::context::digest(&loaded.messages, 1, loaded.messages.len());
    loaded.messages = vec![summary];
    loaded.model = Some(starting_model.to_string());
    store.rewrite(
        Some(starting_model),
        loaded.goal.as_deref(),
        &loaded.messages,
    )?;
    Ok(true)
}
