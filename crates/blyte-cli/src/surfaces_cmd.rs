use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use crate::surface_registry::{find_by_name, live_entries, RegistryEntry};

fn repository_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

pub fn format_surfaces_table(entries: &[RegistryEntry], current_id: Option<&str>) -> String {
    if entries.is_empty() {
        return "no surfaces are running\n".to_string();
    }

    let rows: Vec<(String, String, &str, &str)> = entries
        .iter()
        .map(|entry| {
            let name = if current_id == Some(entry.id.as_str()) {
                format!("{} (this surface)", entry.name)
            } else {
                entry.name.clone()
            };
            (
                name,
                repository_name(&entry.root),
                entry.branch.as_str(),
                entry.doing.as_str(),
            )
        })
        .collect();

    let name_w = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let repo_w = rows
        .iter()
        .map(|row| row.1.chars().count())
        .max()
        .unwrap_or(0)
        .max("REPOSITORY".len());
    let branch_w = rows
        .iter()
        .map(|row| row.2.chars().count())
        .max()
        .unwrap_or(0)
        .max("BRANCH".len());

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<name_w$}  {:<repo_w$}  {:<branch_w$}  DOING",
        "NAME", "REPOSITORY", "BRANCH"
    );
    for (name, repo, branch, doing) in rows {
        let _ = writeln!(
            out,
            "{name:<name_w$}  {repo:<repo_w$}  {branch:<branch_w$}  {doing}"
        );
    }
    out
}

pub fn run_surfaces(dir: &Path, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let entries = match live_entries(dir) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    if json {
        match serde_json::to_string_pretty(&entries) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
                return 0;
            }
            Err(e) => {
                let _ = writeln!(err, "blyte: failed to serialize surfaces: {e}");
                return 1;
            }
        }
    }

    let _ = write!(out, "{}", format_surfaces_table(&entries, None));
    0
}

pub fn run_open(
    dir: &Path,
    name: &str,
    opener: &dyn Fn(&str),
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let entry = match find_by_name(dir, name) {
        Ok(entry) => entry,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    let token_path = match blyte_agent::sandbox::surface_token_path_for_id(&entry.id) {
        Ok(path) => path,
        Err(e) => {
            let _ = writeln!(err, "blyte: {e}");
            return 1;
        }
    };

    let raw_token = match std::fs::read_to_string(&token_path) {
        Ok(text) => text,
        Err(_) => {
            let _ = writeln!(
                err,
                "blyte: surface {} has no usable token at {}",
                entry.name,
                token_path.display()
            );
            return 1;
        }
    };

    let token = raw_token.trim();
    if token.is_empty() || token.contains(['\n', '\r']) {
        let _ = writeln!(
            err,
            "blyte: surface {} has no usable token at {}",
            entry.name,
            token_path.display()
        );
        return 1;
    }

    let url = format!("http://127.0.0.1:{}/#token={token}", entry.port);
    opener(&url);
    let _ = writeln!(out, "blyte: opened surface {}", entry.name);
    0
}
