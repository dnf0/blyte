//! `blyte.toml` repository configuration: gate detection, rendering, and loading.

use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchConfig {
    pub gate: Option<String>,
    pub model: Option<String>,
    pub max_seconds: Option<u64>,
    pub max_turns: Option<u32>,
    pub max_cost: Option<f64>,
    pub network: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    #[serde(default)]
    pub dispatch: DispatchConfig,
}

pub fn load(root: &Path) -> Result<Option<RepoConfig>, String> {
    let path = root.join("blyte.toml");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: RepoConfig =
        toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(config))
}

pub fn detect(root: &Path) -> Vec<String> {
    // A Makefile's own targets are how a repository says it is checked: blyte-eval's
    // `make lint` runs ruff and pyright, which per-language guessing would miss.
    let make = make_targets(root);
    if make.iter().any(|t| t == "check") {
        return vec!["make check".to_string()];
    }
    let from_make: Vec<String> = ["lint", "test"]
        .iter()
        .filter(|t| make.iter().any(|m| m == *t))
        .map(|t| format!("make {t}"))
        .collect();
    if !from_make.is_empty() {
        return from_make;
    }

    let mut steps = Vec::new();

    // Rust (Cargo.toml)
    if root.join("Cargo.toml").exists() {
        steps.push("cargo fmt --all --check".to_string());
        steps.push("cargo clippy --all-targets -- -D warnings".to_string());
        steps.push("cargo test --workspace --all-targets".to_string());
    }

    // Python (pyproject.toml)
    if root.join("pyproject.toml").exists() {
        let pyproject = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
        let runner = if root.join("uv.lock").exists() {
            "uv run "
        } else {
            "python3 -m "
        };
        if pyproject.contains("[tool.ruff") || root.join("ruff.toml").exists() {
            steps.push(format!("{runner}ruff check ."));
        }
        if pyproject.contains("[tool.pytest") || root.join("tests").is_dir() {
            steps.push(format!("{runner}pytest -q"));
        }
    }

    // Node (package.json)
    if root.join("package.json").exists() {
        let runner = if root.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else if root.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm"
        };
        if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = val.get("scripts").and_then(|s| s.as_object()) {
                    for script in ["lint", "typecheck", "test"] {
                        if scripts.contains_key(script) {
                            let cmd = if script == "test" {
                                format!("{runner} test")
                            } else if runner == "yarn" {
                                format!("yarn {script}")
                            } else {
                                format!("{runner} run {script}")
                            };
                            steps.push(cmd);
                        }
                    }
                }
            }
        }
    }

    // Go (go.mod)
    if root.join("go.mod").exists() {
        steps.push(r#"test -z "$(gofmt -l .)""#.to_string());
        steps.push("go vet ./...".to_string());
        steps.push("go test ./...".to_string());
    }

    steps
}

pub fn render(gate: &[String]) -> String {
    let mut gate_str = String::new();
    for step in gate {
        gate_str.push_str(step);
        gate_str.push('\n');
    }
    format!(
        r#"# blyte.toml: how this repository's work is checked, for `blyte dispatch`.
[dispatch]
gate = """
{gate_str}"""
# model = "gemini-3.8-flash-high"
max_seconds = 3600
max_turns = 300
# max_cost = 2.0
# network = "on"
"#
    )
}

/// The targets a `Makefile` (or `makefile`, `GNUmakefile`) at `root` defines: lines of the form
/// `name:` at the start of a line, excluding variable assignments (`name := …`) and special
/// targets (`.PHONY`).
fn make_targets(root: &Path) -> Vec<String> {
    let text = ["GNUmakefile", "makefile", "Makefile"]
        .iter()
        .find_map(|f| std::fs::read_to_string(root.join(f)).ok())
        .unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            if rest.starts_with('=')
                || name.starts_with(['.', '\t', ' ', '#'])
                || name.contains(['=', '$', ' '])
            {
                return None;
            }
            Some(name.trim().to_string())
        })
        .collect()
}
