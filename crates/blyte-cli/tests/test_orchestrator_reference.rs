use std::collections::HashSet;

use blyte_cli::cli::Cli;
use blyte_cli::orchestrator_text::{DEFAULT_PROCESS, REFERENCE};
use clap::CommandFactory;

enum Snippet<'a> {
    FencedLine(&'a str),
    Backtick(&'a str),
}

fn extract_snippets(text: &str) -> Vec<Snippet<'_>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(Snippet::FencedLine(line.trim()));
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                let start_ticks = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                let tick_len = i - start_ticks;
                let content_start = i;
                let mut found_end = None;
                while i < bytes.len() {
                    if bytes[i] == b'`' {
                        let close_start = i;
                        while i < bytes.len() && bytes[i] == b'`' {
                            i += 1;
                        }
                        if i - close_start == tick_len {
                            found_end = Some(close_start);
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                if let Some(end) = found_end {
                    out.push(Snippet::Backtick(line[content_start..end].trim()));
                }
            } else {
                i += 1;
            }
        }
    }
    out
}

fn extract_flags(s: &str) -> Vec<String> {
    let mut flags = Vec::new();
    for raw in s.split_whitespace() {
        let token = raw.trim_start_matches(['(', '[']);
        let Some(rest) = token.strip_prefix("--") else {
            continue;
        };
        if !rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !name.is_empty() {
            flags.push(name);
        }
    }
    flags
}

fn long_flags_of(cmd: &clap::Command, global_only: bool) -> HashSet<String> {
    let mut set = HashSet::new();
    for arg in cmd.get_arguments() {
        if global_only && !arg.is_global_set() {
            continue;
        }
        if let Some(long) = arg.get_long() {
            set.insert(long.to_string());
        }
        if let Some(aliases) = arg.get_all_aliases() {
            for alias in aliases {
                set.insert(alias.to_string());
            }
        }
    }
    set
}

fn collect_all_subcommand_flags(cmd: &clap::Command, out: &mut HashSet<String>) {
    for sub in cmd.get_subcommands() {
        out.extend(long_flags_of(sub, false));
        collect_all_subcommand_flags(sub, out);
    }
}

fn find_subcommand<'a>(cmd: &'a clap::Command, name: &str) -> Option<&'a clap::Command> {
    cmd.get_subcommands()
        .find(|sub| sub.get_name() == name || sub.get_all_aliases().any(|a| a == name))
}

fn check_subcommands(text: &str, cli: &clap::Command) -> Vec<String> {
    let mut errors = Vec::new();
    for snippet in extract_snippets(text) {
        let s = match snippet {
            Snippet::FencedLine(s) | Snippet::Backtick(s) => s,
        };
        let Some(rest) = s.strip_prefix("blyte ") else {
            continue;
        };
        let Some(word) = rest.split_whitespace().next() else {
            continue;
        };
        if find_subcommand(cli, word).is_none() {
            errors.push(format!("unknown subcommand `{word}` in `{s}`"));
        }
    }
    errors
}

fn check_flags(text: &str, cli: &clap::Command) -> Vec<String> {
    let global_flags = long_flags_of(cli, true);
    let mut all_sub_flags = HashSet::new();
    collect_all_subcommand_flags(cli, &mut all_sub_flags);
    all_sub_flags.extend(global_flags.iter().cloned());

    let mut errors = Vec::new();
    for snippet in extract_snippets(text) {
        match snippet {
            Snippet::FencedLine(s) | Snippet::Backtick(s) if s.starts_with("blyte ") => {
                let rest = &s["blyte ".len()..];
                let mut words = rest.split_whitespace();
                let Some(sub_name) = words.next() else {
                    continue;
                };
                let Some(sub) = find_subcommand(cli, sub_name) else {
                    continue;
                };
                let mut valid = long_flags_of(sub, false);
                valid.extend(global_flags.iter().cloned());
                if let Some(next_word) = words.next() {
                    if let Some(nested) = find_subcommand(sub, next_word) {
                        valid.extend(long_flags_of(nested, false));
                    }
                }
                for flag in extract_flags(rest) {
                    if !valid.contains(&flag) {
                        errors.push(format!(
                            "unknown flag `--{flag}` on `blyte {sub_name}` in `{s}`"
                        ));
                    }
                }
            }
            Snippet::Backtick(s) if !s.contains("blyte") => {
                for flag in extract_flags(s) {
                    if !all_sub_flags.contains(&flag) {
                        errors.push(format!("unknown flag `--{flag}` in `{s}`"));
                    }
                }
            }
            _ => {}
        }
    }
    errors
}

fn built_cli() -> clap::Command {
    let mut cmd = <Cli as CommandFactory>::command();
    cmd.build();
    cmd
}

#[test]
fn every_blyte_subcommand_named_exists() {
    let cli = built_cli();
    let mut errors = check_subcommands(REFERENCE, &cli);
    errors.extend(check_subcommands(DEFAULT_PROCESS, &cli));
    assert!(
        errors.is_empty(),
        "unknown blyte subcommands:\n{}",
        errors.join("\n")
    );
}

#[test]
fn every_flag_named_exists_on_its_subcommand() {
    let cli = built_cli();
    let mut errors = check_flags(REFERENCE, &cli);
    errors.extend(check_flags(DEFAULT_PROCESS, &cli));
    assert!(
        errors.is_empty(),
        "unknown blyte flags:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_check_catches_an_unknown_flag() {
    let cli = built_cli();
    let errors = check_flags("`blyte review T1 --no-such-flag`", &cli);
    assert!(
        errors.iter().any(|e| e.contains("--no-such-flag")),
        "expected `--no-such-flag` to be reported, got: {errors:?}"
    );
}

#[test]
fn reference_describes_unsandboxed_and_no_dev_null_redirect() {
    assert!(blyte_cli::orchestrator_text::SANDBOX_ON_PARAGRAPH.contains("unsandboxed"));
    assert!(!REFERENCE.contains("> /dev/null"));
}

#[test]
fn default_process_hands_a_request_to_one_run_and_ends_the_turn() {
    let normalized = DEFAULT_PROCESS
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        normalized.contains("end your turn. Do not call `blyte wait`."),
        "{DEFAULT_PROCESS}"
    );
    assert!(
        DEFAULT_PROCESS.contains("--base blyte/<request>"),
        "{DEFAULT_PROCESS}"
    );
    assert!(
        normalized.contains("Split work across several runs only when the user asks for that, or when a run stopped because its budget ran out and the user agrees to a split."),
        "{DEFAULT_PROCESS}"
    );
    assert!(REFERENCE.contains("blyte wait --any <ids>"), "{REFERENCE}");
    assert!(!REFERENCE.contains(r#"{"args": ["wait""#), "{REFERENCE}");
}
