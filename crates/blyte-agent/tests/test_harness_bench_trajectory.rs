//! The benchmark's trajectory is deterministic, realistic in mix, and every step is valid
//! against its fixture, so a benchmark run measures tools, not errors.

use blyte_agent::bench::{percentile, trajectory};

fn files() -> Vec<String> {
    (0..10).map(|i| format!("crates/x/src/f{i}.rs")).collect()
}

#[test]
fn the_trajectory_is_deterministic_and_has_the_intended_mix() {
    let a = trajectory(&files(), 200);
    let b = trajectory(&files(), 200);
    assert_eq!(a, b);
    assert_eq!(a.len(), 200);
    let count = |n: &str| a.iter().filter(|(name, _)| name == n).count();
    assert_eq!(count("read"), 60);
    assert_eq!(count("grep"), 30);
    assert_eq!(count("glob"), 10);
    assert_eq!(count("write"), 20);
    assert_eq!(count("edit"), 20);
    assert_eq!(count("bash"), 60);
}

#[test]
fn every_edit_targets_a_file_written_earlier_with_its_exact_text() {
    let t = trajectory(&files(), 200);
    for (i, (name, args)) in t.iter().enumerate() {
        if name != "edit" {
            continue;
        }
        let path = args["path"].as_str().unwrap();
        let old = args["old"].as_str().unwrap();
        let written = t[..i].iter().any(|(n, a)| {
            n == "write"
                && a["path"].as_str() == Some(path)
                && a["content"].as_str().is_some_and(|c| c.contains(old))
        });
        assert!(
            written,
            "edit at step {i} has no prior write of {path} containing {old:?}"
        );
    }
}

#[test]
fn reads_use_fixture_files_and_percentiles_are_nearest_rank() {
    let t = trajectory(&files(), 40);
    for (name, args) in &t {
        if name == "read" {
            assert!(files()
                .iter()
                .any(|f| Some(f.as_str()) == args["path"].as_str()));
        }
    }
    let xs = [5.0, 1.0, 3.0, 2.0, 4.0];
    assert_eq!(percentile(&xs, 50.0), 3.0);
    assert_eq!(percentile(&xs, 95.0), 5.0);
    assert_eq!(percentile(&[], 50.0), 0.0);
}
