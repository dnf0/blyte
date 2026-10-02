//! One-line summaries are plain text, not markdown (2026-09-30).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect, Sources};

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn markdown_is_stripped_from_the_summary_line() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs/repo-f");
    for (id, msg) in [
        (
            "M1",
            "### Done\n\n1. **Added brand test suite** in `tests/x.rs`:\n",
        ),
        ("M2", "- **app/lookup/rasters.py**: routing\n"),
    ] {
        write(&runs.join(format!("{id}.log")), "[Turn 1] \n");
        write(
            &runs.join(format!("{id}.report.json")),
            &serde_json::json!({"outcome": "verified", "final_message": msg}).to_string(),
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let v = collect(
        &Sources {
            roots: vec![],
            runs_dirs: vec![tmp.path().join("runs")],
            hours: 24,
        },
        now,
    );
    let now_of = |k: &str| {
        v["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["key"] == k)
            .unwrap()["now"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(now_of("repo-f/M1"), "Added brand test suite in tests/x.rs:");
    assert_eq!(now_of("repo-f/M2"), "app/lookup/rasters.py: routing");
}
