//! A model-free benchmark of the harness's own time: a fixed trajectory of tool calls, replayed
//! at zero model latency (`examples/harness_bench.rs`). Every speed change is judged by it.

use serde_json::{json, Value};

/// A realistic mix of tool calls over `files`, deterministic, `turns` long: per 20 steps, 6 reads
/// (2 of them ranged), 3 greps, 1 glob, 2 writes, 2 edits (each of a file written just before,
/// with its exact text) and 6 shell commands.
pub fn trajectory(files: &[String], turns: usize) -> Vec<(String, Value)> {
    let file = |i: usize| files[(i * 7) % files.len().max(1)].clone();
    (0..turns)
        .map(|i| {
            let (name, args) = match i % 20 {
                0 | 8 | 11 | 18 => ("read", json!({ "path": file(i) })),
                3 | 13 => (
                    "read",
                    json!({ "path": file(i), "start_line": 1, "end_line": 40 }),
                ),
                1 => ("grep", json!({ "pattern": "fn " })),
                10 => ("grep", json!({ "pattern": "impl .* for" })),
                17 => ("grep", json!({ "pattern": "Message::" })),
                7 => ("glob", json!({ "pattern": "**/*.rs" })),
                4 | 14 => (
                    "write",
                    json!({ "path": format!("scratch/s{i}.txt"), "content": format!("alpha {i}\nsecond line\n") }),
                ),
                5 | 15 => (
                    "edit",
                    json!({ "path": format!("scratch/s{}.txt", i - 1), "old": format!("alpha {}", i - 1), "new": format!("beta {}", i - 1) }),
                ),
                2 => ("bash", json!({ "command": "ls" })),
                6 => ("bash", json!({ "command": "git status --short | head -20" })),
                9 => ("bash", json!({ "command": "wc -l $(git ls-files | head -50) | tail -1" })),
                12 => ("bash", json!({ "command": "head -30 Cargo.toml" })),
                16 => ("bash", json!({ "command": "echo done" })),
                _ => ("bash", json!({ "command": "find . -name '*.rs' | wc -l" })),
            };
            (name.to_string(), args)
        })
        .collect()
}

/// Nearest-rank percentile of `xs` (any order); 0 for an empty slice.
pub fn percentile(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}
