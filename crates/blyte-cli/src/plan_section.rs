//! Extract a task section from a plan file.

/// The section of `plan` whose heading matches `task`, including the heading line, up to (not
/// including) the next heading of the same or a higher level (fewer or equal `#`), or the end.
/// A heading matches when its text after the `#`s, trimmed, equals `task` trimmed, or starts with
/// `task` followed by `:`, ` -`, ` —` or ` (`. Example: task "Task B2" matches
/// "### Task B2: the parser" and "### Task B2", but not "### Task B20".
/// Headings inside fenced code blocks (``` or ~~~) are ignored.
/// Returns None if no heading matches; if several match, the first.
pub fn extract(plan: &str, task: &str) -> Option<String> {
    let task_trimmed = task.trim();
    if task_trimmed.is_empty() {
        return None;
    }

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Fence {
        None,
        Backticks(usize),
        Tildes(usize),
    }

    let mut fence = Fence::None;
    let mut matched_start: Option<usize> = None;
    let mut matched_level: usize = 0;
    let mut matched_end: Option<usize> = None;

    let mut offset = 0;
    for line in plan.split_inclusive('\n') {
        let line_offset = offset;
        offset += line.len();

        let line_content = line.trim_end_matches(['\r', '\n']);
        let trimmed_start = line_content.trim_start();

        match fence {
            Fence::None => {
                let bt = trimmed_start.chars().take_while(|&c| c == '`').count();
                if bt >= 3 && !trimmed_start[bt..].contains('`') {
                    fence = Fence::Backticks(bt);
                    continue;
                }
                let tl = trimmed_start.chars().take_while(|&c| c == '~').count();
                if tl >= 3 {
                    fence = Fence::Tildes(tl);
                    continue;
                }

                // Check if this line is an ATX heading
                let hashes = trimmed_start.chars().take_while(|&c| c == '#').count();
                if hashes > 0 {
                    let after = &trimmed_start[hashes..];
                    if after.starts_with(' ') || after.starts_with('\t') || after.is_empty() {
                        let heading_raw = after.trim();
                        let heading_text = if let Some(stripped) = heading_raw.strip_suffix('#') {
                            let stripped_all = stripped.trim_end_matches('#');
                            if stripped_all.ends_with(' ')
                                || stripped_all.ends_with('\t')
                                || stripped_all.is_empty()
                            {
                                stripped_all.trim()
                            } else {
                                heading_raw
                            }
                        } else {
                            heading_raw
                        };

                        if matched_start.is_none() {
                            if heading_matches(heading_text, task_trimmed) {
                                matched_start = Some(line_offset);
                                matched_level = hashes;
                            }
                        } else if hashes <= matched_level {
                            matched_end = Some(line_offset);
                            break;
                        }
                    }
                }
            }
            Fence::Backticks(min) => {
                let bt = trimmed_start.chars().take_while(|&c| c == '`').count();
                if bt >= min && trimmed_start[bt..].trim().is_empty() {
                    fence = Fence::None;
                }
            }
            Fence::Tildes(min) => {
                let tl = trimmed_start.chars().take_while(|&c| c == '~').count();
                if tl >= min && trimmed_start[tl..].trim().is_empty() {
                    fence = Fence::None;
                }
            }
        }
    }

    matched_start.map(|start| {
        let end = matched_end.unwrap_or(plan.len());
        plan[start..end].to_string()
    })
}

fn heading_matches(heading_text: &str, task_trimmed: &str) -> bool {
    if heading_text == task_trimmed {
        return true;
    }
    if let Some(rest) = heading_text.strip_prefix(task_trimmed) {
        if rest.starts_with(':')
            || rest.starts_with(" -")
            || rest.starts_with(" —")
            || rest.starts_with(" (")
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_with_colon_returns_through_line_before_next_heading() {
        let plan = "\
# Intro
Some overview.

### Task B2: the parser
Parse the input tokens.
Handle error cases.

### Task B3: code generation
Generate bytecode.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some(
                "### Task B2: the parser\nParse the input tokens.\nHandle error cases.\n\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn exact_task_name_match() {
        let plan = "\
### Task B2
Parse the input tokens.

### Task B3
Other.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some("### Task B2\nParse the input tokens.\n\n".to_string())
        );
    }

    #[test]
    fn variations_of_matching_delimiters() {
        let hyphen = extract("### Task B2 - the parser\nbody\n### End\n", "Task B2");
        assert_eq!(hyphen, Some("### Task B2 - the parser\nbody\n".to_string()));

        let em_dash = extract("### Task B2 — the parser\nbody\n### End\n", "Task B2");
        assert_eq!(
            em_dash,
            Some("### Task B2 — the parser\nbody\n".to_string())
        );

        let paren = extract("### Task B2 (the parser)\nbody\n### End\n", "Task B2");
        assert_eq!(paren, Some("### Task B2 (the parser)\nbody\n".to_string()));
    }

    #[test]
    fn task_b2_does_not_match_task_b20_and_returns_b2_when_both_exist() {
        let plan = "\
### Task B20: twenty
This is task 20.

### Task B2: two
This is task 2.

### Task B3: three
This is task 3.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some("### Task B2: two\nThis is task 2.\n\n".to_string())
        );
    }

    #[test]
    fn higher_level_heading_ends_section_subheading_does_not() {
        let plan = "\
### Task B2: the parser
Parser description.

#### Implementation notes
Some detailed steps here.

##### Deep note
Even deeper.

## Architecture
Next high-level section.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some(
                "### Task B2: the parser\nParser description.\n\n#### Implementation notes\nSome detailed steps here.\n\n##### Deep note\nEven deeper.\n\n".to_string()
            )
        );
    }

    #[test]
    fn heading_inside_backtick_fence_is_not_a_heading() {
        let plan = "\
Here is a code example:
```markdown
### Task B2: fake
Inside fence.
```

### Task B2: real
Real body.

### Task B3
After.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some("### Task B2: real\nReal body.\n\n".to_string())
        );
    }

    #[test]
    fn heading_inside_tilde_fence_is_not_a_heading() {
        let plan = "\
~~~
### Task B2: fake
Inside tilde fence.
~~~
";
        assert_eq!(extract(plan, "Task B2"), None);
    }

    #[test]
    fn fence_inside_section_does_not_end_section_on_nested_heading() {
        let plan = "\
### Task B2
```markdown
## Not a real section break
```
Still in Task B2.

### Task B3
End.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some(
                "### Task B2\n```markdown\n## Not a real section break\n```\nStill in Task B2.\n\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn section_at_end_of_file_returns_to_the_end() {
        let plan = "\
# Overview
Some text.

### Task B2: the parser
All the way to EOF";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some("### Task B2: the parser\nAll the way to EOF".to_string())
        );

        let plan_with_nl = "\
# Overview

### Task B2: the parser
All the way to EOF with newline.
";
        let extracted2 = extract(plan_with_nl, "Task B2");
        assert_eq!(
            extracted2,
            Some("### Task B2: the parser\nAll the way to EOF with newline.\n".to_string())
        );
    }

    #[test]
    fn no_match_returns_none() {
        let plan = "\
# Overview
### Task A1
### Task A2
";
        assert_eq!(extract(plan, "Task B2"), None);
        assert_eq!(extract("", "Task B2"), None);
        assert_eq!(extract(plan, ""), None);
    }

    #[test]
    fn returns_first_match_if_several_match() {
        let plan = "\
### Task B2: first
First implementation.

### Task B2: second
Second implementation.
";
        let extracted = extract(plan, "Task B2");
        assert_eq!(
            extracted,
            Some("### Task B2: first\nFirst implementation.\n\n".to_string())
        );
    }
}
