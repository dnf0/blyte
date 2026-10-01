//! `blyte review` flags reads of other terminals and of files outside the repository: a research run
//! read another session's terminal for its last turns (2026-10-01).

use std::path::{Path, PathBuf};

use blyte_cli::dispatch::outside_reads;

#[test]
fn terminal_reads_and_files_outside_the_repository_are_flagged() {
    let home = Path::new("/Users/u");
    let allowed = vec![
        PathBuf::from("/Users/u/repos/app"),
        PathBuf::from("/Users/u/repos/app/.wt/T1"),
    ];
    let log = "\
[Turn 1] \n● bash cat /Users/u/repos/app/src/main.rs -> exit 0
● bash ls ~/.cargo/registry -> exit 0
● bash sleep 3 && /Users/u/.local/bin/herdr pane read w3N:p1 --source -> exit 0
● bash grep -rn token ~/repos/other/config.py -> exit 0
● bash cat $HOME/.zshrc -> exit 0
● bash tmux capture-pane -p -t 2 -> exit 0
● bash cat /etc/hosts -> exit 0
● bash grep -rn token ~/repos/other/config.py -> exit 0
";
    assert_eq!(
        outside_reads(log, &allowed, home),
        vec![
            "sleep 3 && /Users/u/.local/bin/herdr pane read w3N:p1 --source",
            "grep -rn token ~/repos/other/config.py",
            "cat $HOME/.zshrc",
            "tmux capture-pane -p -t 2",
        ]
    );
}
