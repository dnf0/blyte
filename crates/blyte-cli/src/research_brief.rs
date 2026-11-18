use std::path::{Path, PathBuf};

/// The first words of the line blyte puts at the top of a research report that did not pass its checks.
pub const NOT_VERIFIED_PREFIX: &str = "> **Not verified.**";

/// The sections a research question must have, in order.
pub const SECTIONS: [&str; 4] = ["Objective", "Start from", "Out of scope", "Answer format"];

/// A question to fill in: a title line and the four sections, each with a one-line prompt.
pub const TEMPLATE: &str = "\
# Question: <one line>

## Objective
<what you need to know, and what you will do with the answer>

## Start from
<the files or directories to read first, as absolute paths>

## Out of scope
<what not to read or answer; other questions being asked separately>

## Answer format
<what the report must contain: a table, paths with line numbers, a recommendation; and a length limit>
";

/// The sections of `SECTIONS` that `question` has no heading for, in `SECTIONS` order.
pub fn missing_sections(question: &str) -> Vec<&'static str> {
    let mut found = [false; SECTIONS.len()];
    let mut in_fence = false;

    for line in question.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if !(1..=6).contains(&hashes) {
            continue;
        }
        let after_hashes = &trimmed[hashes..];
        if !after_hashes.starts_with(' ') {
            continue;
        }
        let heading = after_hashes.trim().trim_end_matches(':').trim();
        for (i, section) in SECTIONS.iter().enumerate() {
            if heading
                .get(..section.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(section))
            {
                found[i] = true;
            }
        }
    }

    SECTIONS
        .iter()
        .enumerate()
        .filter_map(|(i, &section)| (!found[i]).then_some(section))
        .collect()
}

/// Markdown files named in `question` that exist and start with `NOT_VERIFIED_PREFIX`.
pub fn unverified_inputs(question: &str, base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    for piece in question.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
            )
    }) {
        let cleaned = piece.trim_end_matches(['.', ',', ';', ':']);
        if !cleaned.ends_with(".md") {
            continue;
        }
        let path = Path::new(cleaned);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };
        if out.contains(&resolved) {
            continue;
        }
        if std::fs::read_to_string(&resolved).is_ok_and(|c| c.starts_with(NOT_VERIFIED_PREFIX)) {
            out.push(resolved);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_has_every_section() {
        assert!(missing_sections(TEMPLATE).is_empty());
    }

    #[test]
    fn missing_sections_are_named_in_order() {
        let partial = "## Objective\nfind X\n\n## Answer format\na table\n";
        assert_eq!(
            missing_sections(partial),
            vec!["Start from", "Out of scope"]
        );
        assert_eq!(missing_sections(""), SECTIONS.to_vec());
    }

    #[test]
    fn headings_match_without_case_and_with_extras() {
        let q = "\
### objective:
find X

# START FROM (files)
/tmp/a

## Out of scope
none

#### Answer format: a table
10 lines
";
        assert!(missing_sections(q).is_empty());
    }

    #[test]
    fn a_heading_in_a_code_block_does_not_count() {
        let q = "\
```markdown
## Objective
inside fence
```
## Start from
/tmp/a
## Out of scope
none
## Answer format
short
";
        assert_eq!(missing_sections(q), vec!["Objective"]);
    }

    #[test]
    fn a_line_that_is_not_a_heading_does_not_count() {
        let q = "\
Objective: find X
**Objective**
####### Objective
##Objective
";
        assert_eq!(missing_sections(q), SECTIONS.to_vec());
    }

    #[test]
    fn an_unverified_report_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.report.md");
        let b = dir.path().join("b.report.md");
        std::fs::write(&a, "> **Not verified.** This research run ended as\n").unwrap();
        std::fs::write(&b, "# Findings\nAll good.\n").unwrap();

        let q = format!("Read `{}` and also {}.", a.display(), b.display());
        assert_eq!(unverified_inputs(&q, dir.path()), vec![a]);
    }

    #[test]
    fn a_relative_path_resolves_against_base() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let a = notes.join("a.md");
        std::fs::write(&a, "> **Not verified.** incomplete\n").unwrap();

        let q = "See notes/a.md for background.";
        assert_eq!(unverified_inputs(q, dir.path()), vec![a]);
    }

    #[test]
    fn a_missing_file_and_a_repeat_are_not_reported_twice() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        std::fs::write(&a, "> **Not verified.** incomplete\n").unwrap();

        let q = "Check missing.md, then a.md, and again (a.md).";
        assert_eq!(unverified_inputs(q, dir.path()), vec![a]);
    }
}
