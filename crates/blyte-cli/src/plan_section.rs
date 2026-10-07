//! Extract a task section from a plan file.

#[derive(Copy, Clone, PartialEq, Eq)]
enum Fence {
    None,
    Backticks(usize),
    Tildes(usize),
}

impl Fence {
    fn from_opening(trimmed_start: &str) -> Option<Fence> {
        let bt = trimmed_start.chars().take_while(|&c| c == '`').count();
        if bt >= 3 && !trimmed_start[bt..].contains('`') {
            return Some(Fence::Backticks(bt));
        }
        let tl = trimmed_start.chars().take_while(|&c| c == '~').count();
        if tl >= 3 {
            return Some(Fence::Tildes(tl));
        }
        None
    }

    fn is_closing(&self, trimmed_start: &str) -> bool {
        match *self {
            Fence::None => false,
            Fence::Backticks(min) => {
                let bt = trimmed_start.chars().take_while(|&c| c == '`').count();
                bt >= min && trimmed_start[bt..].trim().is_empty()
            }
            Fence::Tildes(min) => {
                let tl = trimmed_start.chars().take_while(|&c| c == '~').count();
                tl >= min && trimmed_start[tl..].trim().is_empty()
            }
        }
    }
}

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

        if fence == Fence::None {
            if let Some(opened) = Fence::from_opening(trimmed_start) {
                fence = opened;
                continue;
            }

            if let Some((hashes, heading_text)) = parse_atx_heading(trimmed_start) {
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
        } else if fence.is_closing(trimmed_start) {
            fence = Fence::None;
        }
    }

    matched_start.map(|start| {
        let end = matched_end.unwrap_or(plan.len());
        plan[start..end].to_string()
    })
}

/// Explain why a task section was not found in `plan`, if due to code block fence issues.
/// Used only after `extract` returned `None`.
pub fn explain_missing(plan: &str, task: &str) -> Option<String> {
    let task_trimmed = task.trim();
    if task_trimmed.is_empty() {
        return None;
    }

    let mut fence = Fence::None;
    let mut fence_open_line: usize = 0;

    for (idx, line) in plan.split_inclusive('\n').enumerate() {
        let line_num = idx + 1;
        let line_content = line.trim_end_matches(['\r', '\n']);
        let trimmed_start = line_content.trim_start();

        if fence == Fence::None {
            if let Some(opened) = Fence::from_opening(trimmed_start) {
                fence = opened;
                fence_open_line = line_num;
                continue;
            }
        } else if fence.is_closing(trimmed_start) {
            fence = Fence::None;
        } else if let Some((_, heading_text)) = parse_atx_heading(trimmed_start) {
            if heading_matches(heading_text, task_trimmed) {
                return Some(format!(
                    "the heading at line {line_num} is inside a code block that opens at line {fence_open_line}; an inner fence may have closed an outer one early (nest with a longer fence such as ```` around ```)"
                ));
            }
        }
    }

    if fence != Fence::None {
        Some(format!(
            "the code block that opens at line {fence_open_line} is never closed, so headings after it are not read"
        ))
    } else {
        None
    }
}

fn parse_atx_heading(trimmed_start: &str) -> Option<(usize, &str)> {
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
            return Some((hashes, heading_text));
        }
    }
    None
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

    #[test]
    fn explain_missing_oc5_shape_unclosed_fence() {
        let plan = "\
# Intro
Some text.
```markdown
Line 4
```bash
Line 6
```
Line 8
```
Line 10
### Task X: y
Line 12
";
        assert_eq!(extract(plan, "Task X"), None);
        assert_eq!(
            explain_missing(plan, "Task X"),
            Some(
                "the heading at line 11 is inside a code block that opens at line 9; an inner fence may have closed an outer one early (nest with a longer fence such as ```` around ```)".to_string()
            )
        );
    }

    #[test]
    fn explain_missing_heading_inside_closed_fence() {
        let plan = "\
# Intro
```
### Task X
```
Other text.
";
        assert_eq!(extract(plan, "Task X"), None);
        assert_eq!(
            explain_missing(plan, "Task X"),
            Some(
                "the heading at line 3 is inside a code block that opens at line 2; an inner fence may have closed an outer one early (nest with a longer fence such as ```` around ```)".to_string()
            )
        );
    }

    #[test]
    fn explain_missing_unclosed_fence_without_matching_heading() {
        let plan = "\
# Intro
```
Some code that never closes.
";
        assert_eq!(extract(plan, "Task X"), None);
        assert_eq!(
            explain_missing(plan, "Task X"),
            Some(
                "the code block that opens at line 2 is never closed, so headings after it are not read".to_string()
            )
        );
    }

    #[test]
    fn explain_missing_no_fences_and_no_matching_heading() {
        let plan = "\
# Intro
Some text.
### Other Task
More text.
";
        assert_eq!(extract(plan, "Task X"), None);
        assert_eq!(explain_missing(plan, "Task X"), None);
    }

    #[test]
    fn explain_missing_heading_inside_tilde_fence() {
        let plan = "\
# Intro
~~~
### Task X
~~~
Other text.
";
        assert_eq!(extract(plan, "Task X"), None);
        assert_eq!(
            explain_missing(plan, "Task X"),
            Some(
                "the heading at line 3 is inside a code block that opens at line 2; an inner fence may have closed an outer one early (nest with a longer fence such as ```` around ```)".to_string()
            )
        );
    }
}
