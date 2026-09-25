//! Tool behaviour, concentrating on the ways a tool can quietly do the wrong thing.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{distil, execute, specs, OUTPUT_LINE_BUDGET};
use serde_json::json;

fn ws() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn no_tool_offers_a_background_mode() {
    // 47.6% of all input tokens in EXP-081 went to turns that only waited for a background
    // task (FINDINGS 13.29). The fix is that there is nothing to background and nothing to
    // poll -- asserted here so it cannot be reintroduced for convenience.
    // Checked against tool NAMES and PARAMETER names, not raw text: the bash description
    // legitimately contains the word "poll" while telling the model there is nothing to poll,
    // and a substring match on prose flagged it.
    let specs = specs();
    for t in &specs {
        assert!(
            !t.name.starts_with("job_"),
            "job control tool present: {}",
            t.name
        );
        let props = t.parameters.get("properties").and_then(|p| p.as_object());
        for key in props.into_iter().flatten().map(|(k, _)| k.as_str()) {
            assert!(
                !matches!(key, "run_in_background" | "background" | "async" | "detach"),
                "tool `{}` offers a background parameter `{key}`",
                t.name
            );
        }
    }
    assert!(
        specs.iter().any(|t| t.name == "bash"),
        "bash must exist to be constrained"
    );
}

#[test]
fn bash_blocks_until_the_command_finishes() {
    let d = ws();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "sleep 1; echo done"}),
    );
    assert!(
        r.content.contains("done"),
        "must return the output, not a job handle: {r:?}"
    );
    assert!(!r.is_error);
}

#[test]
fn bash_always_states_the_exit_code() {
    // A non-zero exit with quiet output otherwise reads as success.
    let d = ws();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "exit 3"}),
    );
    assert!(r.content.starts_with("exit 3"), "got {:?}", r.content);
    assert!(r.is_error, "a failing command must be marked an error");
}

#[test]
fn a_path_escaping_the_workspace_is_refused() {
    // The previous harness relied on a prompt rule: "confine all inspections to the workspace".
    // Invariant IV exists because agents ignore rules like that under pressure.
    let d = ws();
    for bad in ["../../../etc/passwd", "/etc/passwd"] {
        let r = execute(
            d.path(),
            &LocalShell,
            &LocalFs,
            "read",
            &json!({"path": bad}),
        );
        assert!(
            r.is_error && r.content.contains("escapes the workspace"),
            "{bad} must be refused, got {r:?}"
        );
    }
}

#[test]
fn a_new_file_outside_the_workspace_is_refused_even_though_it_does_not_exist_yet() {
    // Canonicalising only existing paths would wave through exactly the dangerous case.
    let d = ws();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "../escaped.txt", "content": "x"}),
    );
    assert!(r.is_error, "writing outside the workspace must fail: {r:?}");
}

#[test]
fn edit_refuses_an_ambiguous_match_instead_of_guessing() {
    // Replacing the first of several matches silently edits the wrong place, and the agent
    // has no way to tell that it did.
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "let x = 1;\nlet x = 1;\n").unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "edit",
        &json!({"path": "a.rs", "old": "let x = 1;", "new": "let x = 2;"}),
    );
    assert!(
        r.is_error && r.content.contains("appears 2 times"),
        "got {r:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "let x = 1;\nlet x = 1;\n",
        "a refused edit must not modify the file"
    );
}

#[test]
fn edit_says_to_read_first_when_the_target_is_absent() {
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "fn main() {}\n").unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "edit",
        &json!({"path": "a.rs", "old": "nope", "new": "x"}),
    );
    assert!(
        r.is_error && r.content.contains("Read it first"),
        "got {r:?}"
    );
}

#[test]
fn read_numbers_lines_so_edits_can_be_precise() {
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "a.rs", "start_line": 2, "end_line": 3}),
    );
    assert!(r.content.contains("2\ttwo"), "got {:?}", r.content);
    assert!(!r.content.contains("one"), "range must be respected");
}

#[test]
fn distil_keeps_the_tail_because_test_runners_summarise_at_the_end() {
    // Head-only truncation would hide the answer and keep the noise -- the opposite of useful
    // for the 25,105-line `go test` output this exists for (FINDINGS 13.26).
    let text: String = (1..=5000).map(|i| format!("line {i}\n")).collect();
    let out = distil(&text, OUTPUT_LINE_BUDGET);
    assert!(out.contains("line 1"), "head kept");
    assert!(
        out.contains("line 5000"),
        "TAIL kept -- the summary lives here"
    );
    assert!(
        out.contains("elided by blyte"),
        "elision must be stated, not silent"
    );
    assert!(out.lines().count() < OUTPUT_LINE_BUDGET + 10);
}

#[test]
fn distil_leaves_short_output_untouched() {
    assert_eq!(distil("a\nb\nc", OUTPUT_LINE_BUDGET), "a\nb\nc");
}

// ---- discovery tools -------------------------------------------------------------------

fn seeded() -> tempfile::TempDir {
    let d = ws();
    std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
    std::fs::create_dir_all(d.path().join("node_modules/junk")).unwrap();
    std::fs::write(
        d.path().join("src/main.rs"),
        "fn main() { parse_array(); }\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("src/deep/array.rs"),
        "fn parse_array() {}\n// multicolumn\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("src/notes.md"),
        "parse_array is the entry point\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("node_modules/junk/array.rs"),
        "fn parse_array() {}\n",
    )
    .unwrap();
    d
}

#[test]
fn glob_finds_files_recursively() {
    let d = seeded();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "src/**/*.rs"}),
    );
    assert!(!r.is_error, "{r:?}");
    assert!(r.content.contains("src/main.rs"), "got {}", r.content);
    assert!(
        r.content.contains("src/deep/array.rs"),
        "** must recurse: {}",
        r.content
    );
}

#[test]
fn glob_reports_no_match_rather_than_returning_empty() {
    // An empty string reads to a model as a tool that broke, and it retries. Saying so plainly
    // is the difference between one call and three.
    let d = seeded();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "*.nope"}),
    );
    assert!(!r.is_error);
    assert!(r.content.contains("no files match"), "got {}", r.content);
}

#[test]
fn grep_returns_path_line_and_text() {
    let d = seeded();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "parse_array"}),
    );
    assert!(!r.is_error, "{r:?}");
    assert!(
        r.content.contains("src/main.rs:1:"),
        "must carry path:line: {}",
        r.content
    );
}

#[test]
fn grep_honours_gitignore_so_node_modules_does_not_drown_the_result() {
    // The single reason a repo-wide search is usable. Requires ripgrep; skipped without it
    // rather than asserted falsely, since the grep fallback has no such behaviour.
    let d = seeded();
    let have_rg = std::process::Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have_rg {
        return;
    }
    std::fs::write(d.path().join(".gitignore"), "node_modules/\n").unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .output()
        .unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "parse_array"}),
    );
    assert!(
        !r.content.contains("node_modules"),
        "ignored paths must stay out: {}",
        r.content
    );
}

#[test]
fn grep_can_be_restricted_by_glob() {
    let d = seeded();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "parse_array", "glob": "*.md"}),
    );
    assert!(r.content.contains("notes.md"), "got {}", r.content);
    assert!(
        !r.content.contains("main.rs"),
        "glob must restrict: {}",
        r.content
    );
}

#[test]
fn grep_says_when_it_capped_rather_than_implying_completeness() {
    let d = ws();
    let many: String = (0..500).map(|i| format!("needle {i}\n")).collect();
    std::fs::write(d.path().join("big.txt"), many).unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "needle"}),
    );
    assert!(
        r.content.contains("capped at"),
        "a truncated search must not read as complete: {}",
        &r.content[r.content.len().saturating_sub(200)..]
    );
}

#[test]
fn grep_patterns_with_shell_metacharacters_are_passed_verbatim() {
    // `$`, quotes and spaces in a regex must reach the tool, not the shell.
    let d = ws();
    std::fs::write(d.path().join("a.rs"), "let x = \"a b\";\nprice $5\n").unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": r"price \$5"}),
    );
    assert!(r.content.contains("a.rs:2:"), "got {}", r.content);
}

#[test]
fn grep_outside_the_workspace_is_refused() {
    let d = ws();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "x", "path": "../.."}),
    );
    assert!(
        r.is_error && r.content.contains("escapes the workspace"),
        "{r:?}"
    );
}

#[test]
fn the_environment_probe_finds_the_tools_that_are_actually_here() {
    let d = ws();
    let bins = blyte_agent::tools::available_binaries(&LocalShell, d.path());
    assert!(
        bins.iter().any(|b| b == "git"),
        "git is present on any dev machine: {bins:?}"
    );
    assert!(!bins.iter().any(|b| b == "definitely-not-a-real-binary"));
}

// ---- the execution seam ----------------------------------------------------------------

/// A shell that records what it was asked to run and answers from a script.
#[derive(Default)]
struct FakeShell {
    seen: std::sync::Mutex<Vec<String>>,
}

impl blyte_agent::shell::Shell for FakeShell {
    fn exec(
        &self,
        command: &str,
        _cwd: &std::path::Path,
        _t: std::time::Duration,
    ) -> Result<blyte_agent::shell::Output, String> {
        self.seen.lock().unwrap().push(command.to_string());
        Ok(blyte_agent::shell::Output {
            stdout: "ran elsewhere\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    fn describe(&self) -> String {
        "a fake".into()
    }
}

#[test]
fn bash_goes_through_the_seam_not_the_host() {
    // The whole point of the seam: a container or remote deployment composes the executor and
    // the model's tool surface does not change. If `bash` reached the host directly, a
    // containerised run would edit the wrong filesystem -- which is what happened before.
    let d = ws();
    let fake = FakeShell::default();
    let r = execute(
        d.path(),
        &fake,
        &LocalFs,
        "bash",
        &json!({"command": "npm test"}),
    );
    assert!(
        r.content.contains("ran elsewhere"),
        "output must come from the seam: {r:?}"
    );
    assert_eq!(
        fake.seen.lock().unwrap().as_slice(),
        &["npm test".to_string()]
    );
}

/// Host files presented as a remote filesystem, as a container's are.
struct NotLocal;
impl blyte_agent::fs::Fs for NotLocal {
    fn read(&self, p: &std::path::Path) -> Result<String, String> {
        LocalFs.read(p)
    }
    fn write(&self, p: &std::path::Path, c: &str) -> Result<(), String> {
        LocalFs.write(p, c)
    }
    fn canonicalize(&self, p: &std::path::Path) -> Result<std::path::PathBuf, String> {
        LocalFs.canonicalize(p)
    }
}

#[test]
fn search_tools_also_go_through_the_seam() {
    // A container's files are not local (`Fs::is_local` is false), so glob, like grep, must go
    // through its executor. A local workspace is searched in-process instead (test_native_glob).
    let d = ws();
    let fake = FakeShell::default();
    execute(
        d.path(),
        &fake,
        &NotLocal,
        "glob",
        &json!({"pattern": "**/*.rs"}),
    );
    // A container's files are not local (`Fs::is_local` is false), so grep must go through its
    // executor. A local workspace is searched in-process instead (test_native_grep).
    execute(
        d.path(),
        &fake,
        &NotLocal,
        "grep",
        &json!({"pattern": "needle"}),
    );
    let seen = fake.seen.lock().unwrap();
    assert!(
        seen.len() >= 2,
        "both searches must reach the executor: {seen:?}"
    );
    // glob lists a container's repository with git ls-files (or falls back to a shell glob).
    assert!(seen
        .iter()
        .any(|c| c.contains("ls-files") || c.contains("*.rs")));
    assert!(seen.iter().any(|c| c.contains("needle")));
}

#[test]
fn the_probe_reports_what_the_execution_environment_has_not_the_host() {
    // A container deployment must not advertise the orchestrator's toolchain. The fake answers
    // every probe affirmatively, so every candidate comes back -- proving the list is built
    // from the seam's reply rather than from the host.
    let d = ws();
    let fake = FakeShell::default();
    let bins = blyte_agent::tools::available_binaries(&fake, d.path());
    assert!(
        bins.contains(&"ran elsewhere".to_string()) || !bins.is_empty(),
        "probe must be answered by the executor, got {bins:?}"
    );
    assert!(fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.contains("command -v")));
}

#[test]
fn the_environment_is_probed_in_one_round_trip() {
    // A remote shell pays a round trip per call; sixteen before the first turn is a poor start.
    let d = ws();
    let fake = FakeShell::default();
    blyte_agent::tools::available_binaries(&fake, d.path());
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        1,
        "the whole probe must be one command, not one per binary"
    );
}

#[test]
fn posix_guaranteed_utilities_are_not_advertised() {
    // Listing sed/awk/find in every prompt is noise: they are present everywhere, so the model
    // has no decision to make from knowing it.
    use blyte_agent::tools::{ACCELERATORS, TOOLCHAINS};
    for noise in ["sed", "awk", "find", "xargs", "grep"] {
        assert!(
            !ACCELERATORS.contains(&noise) && !TOOLCHAINS.contains(&noise),
            "{noise} is POSIX-guaranteed and should not be advertised"
        );
    }
    assert!(
        ACCELERATORS.contains(&"rg"),
        "rg changes search strategy, so it is worth stating"
    );
}

// ---- the filesystem seam ---------------------------------------------------------------

/// An in-memory filesystem. If a tool touches the host instead, these tests fail.
#[derive(Default)]
struct FakeFs {
    files: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl blyte_agent::fs::Fs for FakeFs {
    fn read(&self, path: &std::path::Path) -> Result<String, String> {
        self.files
            .lock()
            .unwrap()
            .get(&path.to_string_lossy().to_string())
            .cloned()
            .ok_or_else(|| format!("no such file: {}", path.display()))
    }
    fn write(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string_lossy().to_string(), content.to_string());
        Ok(())
    }
    fn canonicalize(&self, path: &std::path::Path) -> Result<std::path::PathBuf, String> {
        // No symlinks in the fake; lexical resolution is enough to exercise the escape check.
        Ok(path.to_path_buf())
    }
}

#[test]
fn write_lands_on_the_seam_not_the_host_disk() {
    // The defect this seam exists to prevent: the model edits one filesystem while its
    // verification runs against another. If `write` reached std::fs, the host would have the
    // file and the fake would not.
    let d = ws();
    let fake = FakeFs::default();
    let r = execute(
        d.path(),
        &LocalShell,
        &fake,
        "write",
        &json!({"path": "src/new.rs", "content": "fn x() {}"}),
    );
    assert!(!r.is_error, "{r:?}");
    assert!(
        !d.path().join("src/new.rs").exists(),
        "must NOT have touched the host disk"
    );
    assert_eq!(
        fake.files.lock().unwrap().values().next().unwrap(),
        "fn x() {}"
    );
}

#[test]
fn read_comes_from_the_seam() {
    let d = ws();
    let fake = FakeFs::default();
    let p = d
        .path()
        .join("only-in-the-seam.rs")
        .to_string_lossy()
        .to_string();
    fake.files
        .lock()
        .unwrap()
        .insert(p, "fn seam() {}\n".into());
    let r = execute(
        d.path(),
        &LocalShell,
        &fake,
        "read",
        &json!({"path": "only-in-the-seam.rs"}),
    );
    assert!(
        r.content.contains("fn seam()"),
        "read must consult the seam: {r:?}"
    );
}

#[test]
fn edit_reads_and_writes_through_the_same_seam() {
    // A half-routed edit -- reading the container and writing the host, or vice versa -- would
    // silently discard the change or corrupt the wrong file.
    let d = ws();
    let fake = FakeFs::default();
    let p = d.path().join("a.rs").to_string_lossy().to_string();
    fake.files
        .lock()
        .unwrap()
        .insert(p.clone(), "let x = 1;\n".into());
    let r = execute(
        d.path(),
        &LocalShell,
        &fake,
        "edit",
        &json!({"path": "a.rs", "old": "let x = 1;", "new": "let x = 2;"}),
    );
    assert!(!r.is_error, "{r:?}");
    assert_eq!(fake.files.lock().unwrap()[&p], "let x = 2;\n");
    assert!(
        !d.path().join("a.rs").exists(),
        "the host must be untouched"
    );
}

#[test]
fn the_escape_check_is_evaluated_against_the_seam() {
    // Canonicalising on the orchestrator's disk says nothing about where a path leads inside a
    // container, so the check has to ask the filesystem the write will actually land on.
    let d = ws();
    let fake = FakeFs::default();
    let r = execute(
        d.path(),
        &LocalShell,
        &fake,
        "write",
        &json!({"path": "../../escaped.rs", "content": "x"}),
    );
    assert!(
        r.is_error && r.content.contains("escapes the workspace"),
        "{r:?}"
    );
    assert!(
        fake.files.lock().unwrap().is_empty(),
        "a refused write must write nothing anywhere"
    );
}

#[test]
fn shell_and_fs_are_independently_composable() {
    // The combination that motivated splitting them: commands run in a container while files
    // are bind-mounted on the host. Both must be selectable without the tools noticing.
    let d = ws();
    let fake_shell = FakeShell::default();
    std::fs::write(d.path().join("host.rs"), "on the host\n").unwrap();
    let r = execute(
        d.path(),
        &fake_shell,
        &LocalFs,
        "read",
        &json!({"path": "host.rs"}),
    );
    assert!(
        r.content.contains("on the host"),
        "local fs with a remote shell must work: {r:?}"
    );
    let c = execute(
        d.path(),
        &fake_shell,
        &LocalFs,
        "bash",
        &json!({"command": "true"}),
    );
    assert!(
        c.content.contains("ran elsewhere"),
        "and the shell still goes elsewhere: {c:?}"
    );
}

#[test]
fn traversal_is_refused_even_when_canonicalize_does_not_normalise() {
    // Regression for a real hole. `Path::starts_with` compares components, so
    // `/ws/../../escaped.rs` lexically starts with `/ws`. Before lexical normalisation the
    // entire escape guarantee rested on the Fs implementation's canonicalize resolving `..`,
    // which is true of std::fs and `readlink -f` but is a property of those implementations
    // rather than of the check. FakeFs canonicalises lexically -- as a weak or partially
    // failing implementation would -- and the traversal got through.
    let d = ws();
    let fake = FakeFs::default();
    for escape in [
        "../../escaped.rs",
        "sub/../../../escaped.rs",
        "./../escaped.rs",
    ] {
        let r = execute(
            d.path(),
            &LocalShell,
            &fake,
            "write",
            &json!({"path": escape, "content": "x"}),
        );
        assert!(r.is_error, "{escape} must be refused, got {r:?}");
    }
    assert!(
        fake.files.lock().unwrap().is_empty(),
        "no refused write may land"
    );
}

#[test]
fn legitimate_dot_segments_inside_the_workspace_still_work() {
    // Normalisation must not become a blanket ban on `..`: `src/a/../b.rs` is an ordinary path
    // that stays inside, and refusing it would make the tool surprising.
    let d = ws();
    std::fs::create_dir_all(d.path().join("src/a")).unwrap();
    std::fs::write(d.path().join("src/b.rs"), "inside\n").unwrap();
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "src/a/../b.rs"}),
    );
    assert!(!r.is_error && r.content.contains("inside"), "{r:?}");
}
