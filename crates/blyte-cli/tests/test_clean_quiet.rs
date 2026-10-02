use std::io::Write;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@test.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn init_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q", "-b", "main"]);
    write_file(&t.path().join("README.md"), "initial\n");
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-qm", "initial", "--no-gpg-sign"]);
    t
}

#[test]
fn test_clean_quiet_no_merge_tree_and_timing() {
    let repo = init_repo();
    let root = repo.path();

    write_file(&root.join(".gitignore"), ".wt/\n");
    git(root, &["add", ".gitignore"]);
    git(root, &["commit", "-qm", "ignore .wt", "--no-gpg-sign"]);

    let wt_dir = root.join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    // Create an unmerged commit on a temporary branch so we can create unmerged wip branches
    git(root, &["checkout", "-b", "unmerged-base"]);
    write_file(&root.join("unmerged.txt"), "unmerged content\n");
    git(root, &["add", "unmerged.txt"]);
    git(root, &["commit", "-qm", "unmerged commit", "--no-gpg-sign"]);
    let unmerged_sha = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "main"]);

    // Create 40 fake finished runs (20 merged, 20 unmerged)
    let head = git(root, &["rev-parse", "HEAD"]);
    let mut update_ref = String::new();
    for i in 0..20 {
        let rid = format!("M{i:02}");
        update_ref.push_str(&format!("create refs/heads/wip/{rid} {head}\n"));
    }
    for i in 0..20 {
        let rid = format!("U{i:02}");
        update_ref.push_str(&format!("create refs/heads/wip/{rid} {unmerged_sha}\n"));
    }

    let mut child = Command::new("git")
        .args(["update-ref", "--stdin"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(update_ref.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());

    for i in 0..20 {
        let rid = format!("M{i:02}");
        let r_path = wt_dir.join(&rid);
        std::fs::create_dir_all(&r_path).unwrap();
        write_file(
            &wt_dir.join(format!("{rid}.start.json")),
            &serde_json::json!({ "id": rid, "pid": 999999, "started_at": 1000 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{rid}.verdict.json")),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }

    for i in 0..20 {
        let rid = format!("U{i:02}");
        let r_path = wt_dir.join(&rid);
        std::fs::create_dir_all(&r_path).unwrap();
        write_file(
            &wt_dir.join(format!("{rid}.start.json")),
            &serde_json::json!({ "id": rid, "pid": 999999, "started_at": 1000 }).to_string(),
        );
        write_file(
            &wt_dir.join(format!("{rid}.verdict.json")),
            &serde_json::json!({ "verdict": "accepted" }).to_string(),
        );
    }

    let trace_file = root.join("git_trace.log");

    std::env::set_var("GIT_TRACE", &trace_file);
    let start = std::time::Instant::now();
    blyte_cli::clean::clean_quiet(root, None);
    let elapsed = start.elapsed();
    std::env::remove_var("GIT_TRACE");

    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "quiet clean took {:?}, expected < 2 s",
        elapsed
    );

    let trace_content = std::fs::read_to_string(&trace_file).unwrap_or_default();
    let merge_tree_count = trace_content
        .lines()
        .filter(|l| l.contains("merge-tree"))
        .count();
    assert_eq!(
        merge_tree_count, 0,
        "quiet clean should make 0 merge-tree calls, found {merge_tree_count}:\n{trace_content}"
    );
}
