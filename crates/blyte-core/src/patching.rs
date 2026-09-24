//! State validation, patching, and plan decomposition.

use crate::models::{ExecutionState, TaskSpec};
use crate::plan_parser::parse_plan_markdown;
use serde_json::{Map, Value};

/// Validate a state document, returning every problem rather than the first.
///
/// Reporting one error at a time turns fixing a malformed state into a guessing
/// game, so all failures are collected.
pub fn validate_state(raw: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    let obj = match raw.as_object() {
        Some(o) => o,
        None => return vec!["State must be a JSON object.".to_string()],
    };

    for required in ["status", "step", "completed_tasks", "pending_tasks"] {
        if !obj.contains_key(required) {
            errors.push(format!("Missing required field '{}'.", required));
        }
    }
    if let Some(step) = obj.get("step") {
        if !step.is_u64() {
            errors.push("Field 'step' must be a non-negative integer.".to_string());
        }
    }
    for list in [
        "completed_tasks",
        "pending_tasks",
        "errors",
        "notes",
        "modified_files",
    ] {
        if let Some(v) = obj.get(list) {
            if !v.is_array() {
                errors.push(format!("Field '{}' must be an array.", list));
            }
        }
    }
    if let Some(status) = obj.get("status").and_then(|s| s.as_str()) {
        const ALLOWED: [&str; 4] = ["in_progress", "completed", "failed", "InProgress"];
        if !ALLOWED.iter().any(|a| a.eq_ignore_ascii_case(status)) {
            errors.push(format!("Field 'status' has unexpected value '{}'.", status));
        }
    }
    errors
}

/// Merge `patch` into `target` with null-deletion semantics.
///
/// A JSON null removes the key rather than setting it to null, which is what
/// makes a patch able to express "unset this" without a separate operation.
/// Objects merge recursively; arrays and scalars replace wholesale.
pub fn apply_patch(target: &mut Value, patch: &Value) {
    let (Some(target_obj), Some(patch_obj)) = (target.as_object_mut(), patch.as_object()) else {
        *target = patch.clone();
        return;
    };

    for (key, value) in patch_obj {
        if value.is_null() {
            target_obj.remove(key);
            continue;
        }
        match target_obj.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                apply_patch(existing, value)
            }
            _ => {
                target_obj.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Append a note to a raw state document, creating the ledger if absent.
pub fn append_note(target: &mut Value, author: &str, note: &str) {
    let obj = match target.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let entry = {
        let mut m = Map::new();
        m.insert(
            "timestamp".into(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        m.insert("author".into(), Value::String(author.to_string()));
        m.insert("note".into(), Value::String(note.to_string()));
        Value::Object(m)
    };
    match obj.get_mut("notes").and_then(|n| n.as_array_mut()) {
        Some(arr) => arr.push(entry),
        None => {
            obj.insert("notes".into(), Value::Array(vec![entry]));
        }
    }
}

/// Split coarse plan tasks into atomic child micro-tasks (`Task X.Y`).
///
/// A task is split on its `Files:` entries -- one child per file touched -- since
/// that is the boundary at which work can be verified and committed separately.
/// A task naming one file or none is already atomic and is left alone.
pub fn decompose_plan(plan_markdown: &str) -> Result<String, String> {
    let tasks = parse_plan_markdown(plan_markdown).map_err(|e| e.to_string())?;
    let mut out = String::new();

    for task in &tasks {
        if task.files_modify.len() <= 1 {
            out.push_str(&render_task(&task.id, task));
            continue;
        }
        for (idx, file) in task.files_modify.iter().enumerate() {
            let child_id = format!("{}.{}", task.id, idx + 1);
            let mut child = task.clone();
            child.title = format!("{} ({})", task.title, file.display());
            child.files_modify = vec![file.clone()];
            // Children run in order: each depends on the one before it.
            child.depends_on = if idx == 0 {
                task.depends_on.clone()
            } else {
                vec![format!("{}.{}", task.id, idx)]
            };
            out.push_str(&render_task(&child_id, &child));
        }
    }
    Ok(out)
}

fn render_task(id: &str, task: &TaskSpec) -> String {
    let mut s = format!("### {}: {}\n", id, task.title);
    let deps = if task.depends_on.is_empty() {
        "none".to_string()
    } else {
        task.depends_on.join(", ")
    };
    s.push_str(&format!("- **Depends on:** {}\n", deps));
    if !task.files_modify.is_empty() {
        s.push_str("**Files:**\n");
        for f in &task.files_modify {
            s.push_str(&format!("- Modify: `{}`\n", f.display()));
        }
    }
    if let Some(ref cmd) = task.verification_cmd {
        s.push_str(&format!("**Verification Command:** `{}`\n", cmd));
    }
    s.push('\n');
    s
}

/// Serialise a state back to JSON, preserving unknown fields where possible.
pub fn state_to_value(state: &ExecutionState) -> Result<Value, String> {
    serde_json::to_value(state).map_err(|e| e.to_string())
}
