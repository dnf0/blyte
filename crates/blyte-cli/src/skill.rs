use std::path::PathBuf;
use std::process::Command;

pub const SKILL_MD: &str = include_str!("../assets/blyte-implementer/SKILL.md");

pub fn skill_dir(project: bool) -> Result<PathBuf, String> {
    if project {
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
        let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        Ok(root.join(".claude/skills/blyte-implementer"))
    } else {
        let home = std::env::var("HOME")
            .map_err(|_| "HOME environment variable is unset or empty".to_string())?;
        if home.trim().is_empty() {
            return Err("HOME environment variable is unset or empty".to_string());
        }
        Ok(PathBuf::from(home).join(".claude/skills/blyte-implementer"))
    }
}

pub fn install(project: bool, force: bool) -> Result<PathBuf, String> {
    let dir = skill_dir(project)?;
    let skill_path = dir.join("SKILL.md");

    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path)
            .map_err(|e| format!("failed to read {}: {e}", skill_path.display()))?;
        if existing == SKILL_MD {
            return Ok(skill_path);
        }
        if !force {
            return Err(format!(
                "{} already exists with different content; pass --force to overwrite",
                skill_path.display()
            ));
        }
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create directory {}: {e}", dir.display()))?;
    std::fs::write(&skill_path, SKILL_MD)
        .map_err(|e| format!("failed to write {}: {e}", skill_path.display()))?;

    Ok(skill_path)
}
