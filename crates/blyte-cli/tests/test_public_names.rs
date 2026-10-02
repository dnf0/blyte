use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const PATTERNS: &[&str] = &[
    concat!("cli", "mate-x"),
    concat!("cli", "mate x"),
    concat!("cli", "matex"),
    concat!("pre", "mise-lookup"),
    concat!("pre", "mise_lookup"),
    concat!("pre", "mise_library"),
    concat!("infrastruc", "ture-live"),
    concat!("infrastruc", "ture-catalog"),
    concat!("pigeon", "hole"),
    concat!("spatial", "-service"),
    concat!("tile", "-service"),
    concat!("geo", "-jobs"),
    concat!("code", "artifact"),
    concat!("04330", "2440588"),
    concat!("reliability", "-orchestrator"),
    concat!("up", "draft"),
    concat!("mon", "isha"),
    concat!("/users", "/danielfisher"),
    concat!("claude.ai", "/artifact"),
];

fn should_skip(path: &str) -> bool {
    path.starts_with("docs/superpowers/")
        || path.starts_with("docs/adr/0001-")
        || path.starts_with("docs/adr/0002-")
        || path == "crates/blyte-cli/tests/public_names_allow.txt"
}

#[test]
fn public_names_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize workspace root");

    let allow_file = root.join("crates/blyte-cli/tests/public_names_allow.txt");
    let allow_text = std::fs::read_to_string(&allow_file).expect("read public_names_allow.txt");

    let mut allow_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut allow_entries: Vec<(String, String)> = Vec::new();
    for line in allow_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, substr) = line
            .split_once(':')
            .unwrap_or_else(|| panic!("invalid allow-list format in line: {line}"));
        let path = path.trim().to_string();
        let substr = substr.trim().to_string();
        allow_map
            .entry(path.clone())
            .or_default()
            .push(substr.clone());
        allow_entries.push((path, substr));
    }

    // Check for stale allow-list entries
    let mut stale: Vec<String> = Vec::new();
    for (path, substr) in &allow_entries {
        let file_path = root.join(path);
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                if !content.contains(substr) {
                    stale.push(format!("{path}: {substr}"));
                }
            }
            Err(_) => {
                stale.push(format!("{path}: {substr}"));
            }
        }
    }

    // List tracked files
    let output = Command::new("git")
        .arg("ls-files")
        .current_dir(&root)
        .output()
        .expect("execute git ls-files");
    assert!(output.status.success(), "git ls-files failed");
    let stdout = String::from_utf8(output.stdout).expect("git ls-files output not utf-8");

    let mut uncovered: Vec<String> = Vec::new();
    for tracked in stdout.lines() {
        let tracked = tracked.trim();
        if tracked.is_empty() || should_skip(tracked) {
            continue;
        }

        let file_path = root.join(tracked);
        let Ok(content) = std::fs::read_to_string(&file_path) else {
            // Binary or unreadable files are skipped
            continue;
        };

        let empty = Vec::new();
        let allowed = allow_map.get(tracked).unwrap_or(&empty);

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let line_lower = line.to_lowercase();
            if !PATTERNS.iter().any(|p| line_lower.contains(p)) {
                continue;
            }

            let mut masked = line.to_string();
            for substr in allowed {
                masked = masked.replace(substr, &" ".repeat(substr.len()));
            }

            let masked_lower = masked.to_lowercase();
            if PATTERNS.iter().any(|p| masked_lower.contains(p)) {
                uncovered.push(format!("{tracked}:{line_num}: {line}"));
            }
        }
    }

    if !uncovered.is_empty() || !stale.is_empty() {
        let mut msg = String::new();
        if !uncovered.is_empty() {
            msg.push_str("Uncovered hits:\n");
            for hit in &uncovered {
                msg.push_str(&format!("{hit}\n"));
            }
        }
        if !stale.is_empty() {
            if !msg.is_empty() {
                msg.push('\n');
            }
            msg.push_str("Stale allow-list entries:\n");
            for s in &stale {
                msg.push_str(&format!("{s}\n"));
            }
        }
        panic!("Public names check failed:\n{msg}");
    }
}
