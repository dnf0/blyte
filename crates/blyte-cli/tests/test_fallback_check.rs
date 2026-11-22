use std::path::Path;
use std::process::Command;

use blyte_cli::fallback_check::{added_fallbacks, flag, Finding};

fn diff(path: &str, start: usize, lines: &[&str]) -> String {
    let mut out = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{start},{} +{start},{} @@\n",
        lines.len(),
        lines.len()
    );
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[test]
fn an_added_unwrap_or_default_is_found() {
    let d = diff(
        "src/a.rs",
        10,
        &[
            " let x = 1;",
            "+let region = cfg.region.clone().unwrap_or_default();",
        ],
    );
    assert_eq!(
        added_fallbacks(&d),
        vec![Finding {
            file: "src/a.rs".to_string(),
            line: 11,
            pattern: "unwrap_or_default",
            text: "let region = cfg.region.clone().unwrap_or_default();".to_string(),
        }]
    );
}

#[test]
fn a_removed_or_context_line_is_not_found() {
    let d = diff(
        "src/a.rs",
        10,
        &[
            "-let region = cfg.region.clone().unwrap_or_default();",
            " let region = cfg.region.clone().unwrap_or_default();",
        ],
    );
    assert!(added_fallbacks(&d).is_empty());
}

#[test]
fn rust_unwrap_or_and_unwrap_or_else_are_found() {
    let d = diff(
        "src/a.rs",
        1,
        &[
            "+let a = x.unwrap_or(0);",
            "+let b = y.unwrap_or_else(|| 1);",
        ],
    );
    assert_eq!(
        added_fallbacks(&d),
        vec![
            Finding {
                file: "src/a.rs".to_string(),
                line: 1,
                pattern: "unwrap_or",
                text: "let a = x.unwrap_or(0);".to_string(),
            },
            Finding {
                file: "src/a.rs".to_string(),
                line: 2,
                pattern: "unwrap_or_else",
                text: "let b = y.unwrap_or_else(|| 1);".to_string(),
            },
        ]
    );
}

#[test]
fn a_test_file_is_ignored() {
    for (p, line) in [
        ("tests/a.rs", "+let r = x.unwrap_or_default();"),
        ("crates/x/tests/t.rs", "+let r = x.unwrap_or_default();"),
        ("src/test_a.py", "+r = cfg.get(\"k\", \"v\")"),
        ("web/a.test.ts", "+const r = a ?? b;"),
    ] {
        let d = diff(p, 10, &[" let x = 1;", line]);
        assert!(added_fallbacks(&d).is_empty(), "expected empty for {p}");
    }
}

#[test]
fn a_rust_test_module_is_ignored() {
    let after = diff(
        "src/a.rs",
        1,
        &[
            " #[cfg(test)]",
            " mod tests {",
            "+    let x = a.unwrap_or(0);",
        ],
    );
    assert!(added_fallbacks(&after).is_empty());

    let before = diff(
        "src/a.rs",
        1,
        &["+let x = a.unwrap_or(0);", " #[cfg(test)]", " mod tests {"],
    );
    assert_eq!(
        added_fallbacks(&before),
        vec![Finding {
            file: "src/a.rs".to_string(),
            line: 1,
            pattern: "unwrap_or",
            text: "let x = a.unwrap_or(0);".to_string(),
        }]
    );
}

#[test]
fn a_comment_is_ignored() {
    let d = diff("src/a.rs", 1, &["+    // was .unwrap_or_default()"]);
    assert!(added_fallbacks(&d).is_empty());
}

#[test]
fn python_except_pass_is_found() {
    let d1 = diff("src/a.py", 5, &["+    except Exception:", "+        pass"]);
    assert_eq!(
        added_fallbacks(&d1),
        vec![Finding {
            file: "src/a.py".to_string(),
            line: 5,
            pattern: "except-pass",
            text: "except Exception:".to_string(),
        }]
    );

    let d2 = diff(
        "src/a.py",
        5,
        &["+    except ValueError:", "+        raise"],
    );
    assert!(added_fallbacks(&d2).is_empty());

    let d3 = diff("src/a.py", 5, &["+    except: pass"]);
    assert_eq!(
        added_fallbacks(&d3),
        vec![Finding {
            file: "src/a.py".to_string(),
            line: 5,
            pattern: "except-pass",
            text: "except: pass".to_string(),
        }]
    );
}

#[test]
fn python_get_with_a_default_is_found() {
    let d1 = diff("src/a.py", 1, &["+region = cfg.get(\"region\", \"EUR\")"]);
    assert_eq!(
        added_fallbacks(&d1),
        vec![Finding {
            file: "src/a.py".to_string(),
            line: 1,
            pattern: "get-default",
            text: "region = cfg.get(\"region\", \"EUR\")".to_string(),
        }]
    );

    let d2 = diff("src/a.py", 1, &["+region = cfg.get(\"region\")"]);
    assert!(added_fallbacks(&d2).is_empty());

    let d3 = diff("src/a.py", 1, &["+v = d.get(key(a, b))"]);
    assert!(added_fallbacks(&d3).is_empty());

    let d4 = diff("src/a.py", 1, &["+home = os.getenv(\"HOME\", \"/tmp\")"]);
    assert_eq!(
        added_fallbacks(&d4),
        vec![Finding {
            file: "src/a.py".to_string(),
            line: 1,
            pattern: "get-default",
            text: "home = os.getenv(\"HOME\", \"/tmp\")".to_string(),
        }]
    );
}

#[test]
fn python_or_literal_is_found() {
    let d1 = diff("src/a.py", 1, &["+name = name or \"unknown\""]);
    assert_eq!(
        added_fallbacks(&d1),
        vec![Finding {
            file: "src/a.py".to_string(),
            line: 1,
            pattern: "or-literal",
            text: "name = name or \"unknown\"".to_string(),
        }]
    );

    let d2 = diff("src/a.py", 1, &["+ok = a or b"]);
    assert!(added_fallbacks(&d2).is_empty());
}

#[test]
fn typescript_patterns_are_found() {
    let d = diff(
        "web/a.ts",
        1,
        &[
            "+const r = cfg.region ?? \"EUR\";",
            "+const n = count || 0;",
            "+if (a || b) {",
            "+} catch (e) {}",
        ],
    );
    assert_eq!(
        added_fallbacks(&d),
        vec![
            Finding {
                file: "web/a.ts".to_string(),
                line: 1,
                pattern: "nullish",
                text: "const r = cfg.region ?? \"EUR\";".to_string(),
            },
            Finding {
                file: "web/a.ts".to_string(),
                line: 2,
                pattern: "or-literal",
                text: "const n = count || 0;".to_string(),
            },
            Finding {
                file: "web/a.ts".to_string(),
                line: 4,
                pattern: "empty-catch",
                text: "} catch (e) {}".to_string(),
            },
        ]
    );
}

#[test]
fn shell_patterns_are_found() {
    let d = diff(
        "scripts/run.sh",
        1,
        &["+rm -f x || true", "+REGION=\"${REGION:-eu-west-1}\""],
    );
    assert_eq!(
        added_fallbacks(&d),
        vec![
            Finding {
                file: "scripts/run.sh".to_string(),
                line: 1,
                pattern: "or-true",
                text: "rm -f x || true".to_string(),
            },
            Finding {
                file: "scripts/run.sh".to_string(),
                line: 2,
                pattern: "param-default",
                text: "REGION=\"${REGION:-eu-west-1}\"".to_string(),
            },
        ]
    );
}

#[test]
fn a_deleted_file_and_an_unknown_extension_are_ignored() {
    let raw = "\
diff --git a/src/old.rs b/src/old.rs
--- a/src/old.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-let a = x.unwrap_or_default();
+let b = x.unwrap_or_default();
diff --git a/notes.txt b/notes.txt
--- a/notes.txt
+++ b/notes.txt
@@ -1,1 +1,2 @@
+a ?? b
";
    assert!(added_fallbacks(raw).is_empty());
}

#[test]
fn line_numbers_follow_the_hunks() {
    let raw = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 let a = 1;
+let b = x.unwrap_or(0);
 let c = 2;
@@ -40,2 +41,3 @@
 let d = 3;
+let e = y.unwrap_or_default();
 let f = 4;
";
    assert_eq!(
        added_fallbacks(raw),
        vec![
            Finding {
                file: "src/a.rs".to_string(),
                line: 2,
                pattern: "unwrap_or",
                text: "let b = x.unwrap_or(0);".to_string(),
            },
            Finding {
                file: "src/a.rs".to_string(),
                line: 42,
                pattern: "unwrap_or_default",
                text: "let e = y.unwrap_or_default();".to_string(),
            },
        ]
    );
}

#[test]
fn the_flag_lists_three_and_counts_the_rest() {
    assert_eq!(flag(&[]), None);

    let findings = vec![
        Finding {
            file: "src/a.rs".to_string(),
            line: 1,
            pattern: "unwrap_or",
            text: "let a = x.unwrap_or(0);".to_string(),
        },
        Finding {
            file: "src/b.rs".to_string(),
            line: 2,
            pattern: "unwrap_or_default",
            text: "let b = x.unwrap_or_default();".to_string(),
        },
        Finding {
            file: "src/c.py".to_string(),
            line: 3,
            pattern: "get-default",
            text: "c = d.get('k', 0)".to_string(),
        },
        Finding {
            file: "src/d.ts".to_string(),
            line: 4,
            pattern: "nullish",
            text: "const d = a ?? b;".to_string(),
        },
        Finding {
            file: "src/e.sh".to_string(),
            line: 5,
            pattern: "or-true",
            text: "cmd || true".to_string(),
        },
    ];
    let f = flag(&findings).expect("expected Some flag");
    assert_eq!(f, "possible fallbacks added: 5 (not judged: no review)");
}

#[test]
fn moved_lines_are_still_candidates() {
    let moved_across_files = "\
diff --git a/src/old.rs b/src/old.rs
--- a/src/old.rs
+++ b/src/old.rs
@@ -1,2 +1,1 @@
-    let x = a.unwrap_or(b);
 let keep = 1;
diff --git a/src/new.rs b/src/new.rs
--- a/src/new.rs
+++ b/src/new.rs
@@ -1,1 +1,2 @@
 let keep = 1;
+let x = a.unwrap_or(b);
";
    assert_eq!(
        added_fallbacks(moved_across_files),
        vec![Finding {
            file: "src/new.rs".to_string(),
            line: 2,
            pattern: "unwrap_or",
            text: "let x = a.unwrap_or(b);".to_string(),
        }]
    );

    let long_val = "x".repeat(250);
    let long_line = format!("+    let v = opt.unwrap_or(\"{long_val}\");");
    let d = diff("src/long.rs", 1, &[&long_line]);
    let found = added_fallbacks(&d);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text.chars().count(), 200);
    assert!(found[0].text.starts_with("let v = opt.unwrap_or(\""));
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn review_flags_a_fallback_the_run_added() {
    let r = repo();
    let base_sha = rev_parse(r.path(), "HEAD");
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/FB1", ".wt/FB1"],
    );
    std::fs::write(
        r.path().join("task.md"),
        "Files: `src/a.rs`\nCommit: `feat: add a`\n",
    )
    .unwrap();
    std::fs::write(
        r.path().join(".wt/FB1.start.json"),
        serde_json::json!({
            "id": "FB1",
            "base_sha": base_sha,
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();

    let wt = r.path().join(".wt/FB1");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(wt.join("src/a.rs"), "let r = x.unwrap_or_default();\n").unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    let out = blyte(r.path(), &["review", "FB1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let checks_section = text
        .split("== checks\n")
        .nth(1)
        .unwrap()
        .split("\n== ")
        .next()
        .unwrap();
    assert!(
        checks_section.lines().any(|l| {
            let stripped = l.strip_prefix("! ").unwrap_or(l);
            stripped == "possible fallbacks added: 1 (not judged: no review)"
        }),
        "{text}"
    );
    assert!(!text.contains("== diff"), "{text}");
}
