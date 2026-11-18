use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ directory")
        .parent()
        .expect("repository root")
        .to_path_buf()
}

fn read_toolchain_channel(root: &Path) -> String {
    let path = root.join("rust-toolchain.toml");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let table: toml::Value = toml::from_str(&content)
        .unwrap_or_else(|e| panic!("invalid TOML in {}: {e}", path.display()));
    table
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| panic!("missing toolchain.channel in {}", path.display()))
        .to_string()
}

fn workflow_files(root: &Path) -> Vec<PathBuf> {
    let dir = root.join(".github/workflows");
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()));
    let mut files: Vec<PathBuf> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("failed to read entry in {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yml" | "yaml")))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no workflow files found in {}",
        dir.display()
    );
    files
}

#[test]
fn the_toolchain_file_names_an_exact_version() {
    let root = repo_root();
    let channel = read_toolchain_channel(&root);
    let parts: Vec<&str> = channel.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "rust-toolchain.toml channel {channel:?} must have three numeric parts (X.Y.Z)"
    );
    for part in &parts {
        assert!(
            !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()),
            "rust-toolchain.toml channel {channel:?} has non-numeric part {part:?}"
        );
    }
}

#[test]
fn no_workflow_installs_a_floating_toolchain() {
    let root = repo_root();
    for path in workflow_files(&root) {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        for floating in [
            "dtolnay/rust-toolchain@stable",
            "dtolnay/rust-toolchain@beta",
            "dtolnay/rust-toolchain@nightly",
        ] {
            assert!(
                !content.contains(floating),
                "{} contains floating toolchain {floating}",
                path.display()
            );
        }
    }
}

#[test]
fn every_workflow_toolchain_matches_the_file() {
    let root = repo_root();
    let expected = read_toolchain_channel(&root);
    let mut found = 0usize;

    for path in workflow_files(&root) {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.contains("dtolnay/rust-toolchain@") {
                found += 1;
                let window_end = (i + 6).min(lines.len());
                let toolchain_line = lines[i + 1..window_end]
                    .iter()
                    .find_map(|l| l.trim().strip_prefix("toolchain:"));
                let raw_value = toolchain_line.unwrap_or_else(|| {
                    panic!(
                        "{}:{} has dtolnay/rust-toolchain@ without a `toolchain:` line in the next five lines",
                        path.display(),
                        i + 1
                    )
                });
                let value = raw_value.trim().trim_matches('"').trim_matches('\'');
                assert_eq!(
                    value,
                    expected,
                    "{}:{} toolchain {value:?} does not match rust-toolchain.toml channel {expected:?}",
                    path.display(),
                    i + 1
                );
            }
        }
    }

    assert!(
        found >= 4,
        "expected at least 4 dtolnay/rust-toolchain@ steps across workflows, found {found}"
    );
}
