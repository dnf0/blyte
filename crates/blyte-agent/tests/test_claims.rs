use std::path::{Path, PathBuf};

use blyte_agent::research_check::{claims, Sources};

fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, text).unwrap();
    p
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| e.to_string())
}

#[test]
fn extracts_paragraph_and_list_claims_with_quotes_and_skips_noise() {
    let tmp = tempfile::tempdir().unwrap();
    let a_body: String = (1..=20).map(|n| format!("line_{n}\n")).collect();
    let b_body: String = (1..=5).map(|n| format!("b_{n}\n")).collect();
    let f1 = write(tmp.path(), "src/a.py", &a_body);
    let f2 = write(tmp.path(), "src/b.py", &b_body);
    let listing = vec![f1, f2];
    let src = Sources {
        workspace: tmp.path(),
        read_roots: &[],
        listing: &listing,
        read: &read,
    };

    let report = "\
# Heading citing src/a.py:10 is not a claim

This paragraph has no citations at all and must be skipped.

```python
orphan_code()
```

First claim in `src/a.py:10-12` with a following code block:

```python
line_10
line_11
```

Paragraph citing only missing `src/nope.py:1-3` is skipped.

- First list item cites `src/a.py:2` near start of file and missing `src/missing.py:4`.
- Second list item has no citation.
- Third list item cites `src/b.py:4-5` near end of file:

```python
b_4
b_5
```
";

    let got = claims(report, &src);
    assert_eq!(got.len(), 3, "got: {got:#?}");

    // Claim 1: paragraph with attached code block, widened by 5 on each side (10-5=5..=12+5=17).
    assert_eq!(got[0].id, 1);
    assert_eq!(got[0].line, 9);
    assert_eq!(
        got[0].text,
        "First claim in `src/a.py:10-12` with a following code block:"
    );
    assert_eq!(got[0].quoted.as_deref(), Some("line_10\nline_11"));
    assert_eq!(got[0].evidence.len(), 1);
    assert_eq!(got[0].evidence[0].path, "src/a.py");
    assert_eq!(got[0].evidence[0].start, 5);
    assert_eq!(got[0].evidence[0].end, 17);
    let expected_ev0: String = (5..=17)
        .map(|n| format!("{n}\tline_{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(got[0].evidence[0].lines, expected_ev0);

    // Claim 2: first list item; missing citation skipped, existing clamped at file start (1..=7).
    assert_eq!(got[1].id, 2);
    assert_eq!(got[1].line, 18);
    assert!(got[1].text.starts_with("- First list item"));
    assert_eq!(got[1].quoted, None);
    assert_eq!(got[1].evidence.len(), 1);
    assert_eq!(got[1].evidence[0].path, "src/a.py");
    assert_eq!(got[1].evidence[0].start, 1);
    assert_eq!(got[1].evidence[0].end, 7);
    assert!(got[1].evidence[0].lines.starts_with("1\tline_1\n2\tline_2"));

    // Claim 3: third list item with attached quote; clamped at file end (1..=5).
    assert_eq!(got[2].id, 3);
    assert_eq!(got[2].line, 20);
    assert!(got[2].text.starts_with("- Third list item"));
    assert_eq!(got[2].quoted.as_deref(), Some("b_4\nb_5"));
    assert_eq!(got[2].evidence.len(), 1);
    assert_eq!(got[2].evidence[0].path, "src/b.py");
    assert_eq!(got[2].evidence[0].start, 1);
    assert_eq!(got[2].evidence[0].end, 5);
    assert_eq!(
        got[2].evidence[0].lines,
        "1\tb_1\n2\tb_2\n3\tb_3\n4\tb_4\n5\tb_5"
    );
}

#[test]
fn caps_evidence_at_120_lines_and_4_blocks_per_claim() {
    let tmp = tempfile::tempdir().unwrap();
    let long_body: String = (1..=200).map(|n| format!("row_{n}\n")).collect();
    let f = write(tmp.path(), "src/big.rs", &long_body);
    let listing = vec![f];
    let src = Sources {
        workspace: tmp.path(),
        read_roots: &[],
        listing: &listing,
        read: &read,
    };

    // First claim cites 116 lines (10..=125): widening by 5 would be 126 lines, so context is trimmed to 120 while keeping 10..=125.
    // Second claim cites 150 lines (1..=150) and 5 separate ranges: capped at 120 lines and 4 blocks.
    let report = "\
First claim cites `src/big.rs:10-125` which needs context trimming to 120 lines.

Second claim cites `src/big.rs:1-150`, `src/big.rs:10`, `src/big.rs:30`, `src/big.rs:50`, and `src/big.rs:70`.
";

    let got = claims(report, &src);
    assert_eq!(got.len(), 2);

    let ev0 = &got[0].evidence[0];
    assert_eq!(ev0.end - ev0.start + 1, 120);
    assert_eq!(ev0.lines.lines().count(), 120);
    assert!(ev0.start <= 10 && ev0.end >= 125);

    assert_eq!(got[1].evidence.len(), 4);
    let big_ev = &got[1].evidence[0];
    assert_eq!(big_ev.start, 1);
    assert_eq!(big_ev.end, 120);
    assert_eq!(big_ev.lines.lines().count(), 120);
}
