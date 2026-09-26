//! A `bash` result is the command's exit code and output, nothing more: the #84 commit footer did
//! not reduce git-inspection turns (4 and 6 calls in the next two runs, against 4-6 before) and
//! was removed.

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::{LocalShell, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

#[test]
fn a_commit_result_is_just_the_command_output() {
    let d = tempfile::tempdir().unwrap();
    LocalShell
        .exec(
            "git init -q && git config user.email a@b && git config user.name t && \
             echo 0 > a.txt && git add -A && git commit -qm base",
            d.path(),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
    let out = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "echo 1 > a.txt && git commit -qam 'feat: change'"}),
    )
    .content;
    assert_eq!(out.trim_end(), "exit 0", "{out}");
}
