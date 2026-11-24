use std::process::Command;

fn run_help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .output()
        .expect("Failed to run blyte");
    assert!(
        output.status.success(),
        "blyte {:?} failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        args,
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn command_list_lines(help: &str) -> Vec<&str> {
    let mut in_commands = false;
    let mut lines = Vec::new();
    for line in help.lines() {
        if line.trim() == "Commands:" {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() {
                break;
            }
            lines.push(line);
        }
    }
    assert!(
        !lines.is_empty(),
        "expected a non-empty Commands section in:\n{help}"
    );
    lines
}

#[test]
fn the_help_does_not_list_internal_commands() {
    let help = run_help(&["--help"]);
    let commands = command_list_lines(&help);
    let hidden = [
        "comments",
        "instructions",
        "mock",
        "edit-start",
        "edit-finish",
    ];
    for line in &commands {
        let trimmed = line.trim_start();
        for name in hidden {
            assert!(
                !trimmed.starts_with(name),
                "internal command `{name}` should not appear in Commands list:\n{help}"
            );
        }
    }
}

#[test]
fn the_help_still_lists_the_commands_people_use() {
    let help = run_help(&["--help"]);
    let commands = command_list_lines(&help);
    let expected = [
        "dispatch", "review", "wait", "status", "plan", "cost", "clean", "init", "update",
    ];
    for name in expected {
        assert!(
            commands
                .iter()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "expected command `{name}` in Commands list:\n{help}"
        );
    }
}

#[test]
fn hidden_commands_still_run() {
    let hidden = [
        "edit-start",
        "edit-finish",
        "comments",
        "instructions",
        "mock",
    ];
    for name in hidden {
        let help = run_help(&[name, "--help"]);
        let usage = format!("Usage: blyte {name}");
        assert!(
            help.contains(&usage),
            "expected `{usage}` in `blyte {name} --help` output:\n{help}"
        );
    }
}
