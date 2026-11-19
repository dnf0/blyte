use blyte_cli::comment_lint::{
    added_comments, added_lines, comments_in, flag, is_supported, Comment,
};
use std::collections::BTreeSet;

fn lines(comments: &[Comment]) -> Vec<usize> {
    comments.iter().map(|c| c.line).collect()
}

#[test]
fn test_01_rust_line_comments() {
    let src = "// a\nlet x = 1; // b\n";
    let got = comments_in("a.rs", src);
    assert_eq!(lines(&got), vec![1, 2]);
    assert_eq!(got[0].text, "// a");
    assert_eq!(got[1].text, "// b");
}

#[test]
fn test_02_rust_string_with_slashes() {
    let src = "let s = \"http://x // y\";\n";
    assert_eq!(lines(&comments_in("a.rs", src)), Vec::<usize>::new());
}

#[test]
fn test_03_rust_raw_strings() {
    let src_one_hash = "let s = r#\"a // b \" // c\"#;";
    assert_eq!(
        lines(&comments_in("a.rs", src_one_hash)),
        Vec::<usize>::new()
    );
    let src_one_hash_real = "let s = r#\"a // b \" // c\"#; // real";
    assert_eq!(lines(&comments_in("a.rs", src_one_hash_real)), vec![1]);

    let src_two_hash = "let s = r##\"a // b \"# // c\"##;";
    assert_eq!(
        lines(&comments_in("a.rs", src_two_hash)),
        Vec::<usize>::new()
    );
    let src_two_hash_real = "let s = r##\"a // b \"# // c\"##; // real";
    assert_eq!(lines(&comments_in("a.rs", src_two_hash_real)), vec![1]);
}

#[test]
fn test_04_rust_lifetimes_and_char_literals() {
    let s1 = "fn f<'a>(x: &'a str) {} // c";
    assert_eq!(lines(&comments_in("a.rs", s1)), vec![1]);

    let s2 = "let c = '\"'; // c";
    assert_eq!(lines(&comments_in("a.rs", s2)), vec![1]);

    let s3 = "let c = '\\''; // c";
    assert_eq!(lines(&comments_in("a.rs", s3)), vec![1]);
}

#[test]
fn test_05_rust_nested_block_comments() {
    let src = "/* a /* b */ c */ let x = 1;\n// d\n";
    let got = comments_in("a.rs", src);
    assert_eq!(lines(&got), vec![1, 2]);
    assert_eq!(got[0].text, "/* a /* b */ c */");
    assert_eq!(got[1].text, "// d");
}

#[test]
fn test_06_rust_doc_comments_and_non_doc_variants() {
    for doc in ["/// doc", "//! doc", "/** doc */", "/*! doc */"] {
        assert_eq!(
            lines(&comments_in("a.rs", doc)),
            Vec::<usize>::new(),
            "expected no finding for {doc}"
        );
    }
    assert_eq!(lines(&comments_in("a.rs", "//// four")), vec![1]);
    assert_eq!(lines(&comments_in("a.rs", "/**/")), vec![1]);
}

#[test]
fn test_07_rust_directives() {
    let src = "// SAFETY: the pointer is valid\n// clippy::foo\n";
    assert_eq!(lines(&comments_in("a.rs", src)), Vec::<usize>::new());
}

#[test]
fn test_08_python_comments_and_strings() {
    let src = "# a\nx = 1  # b\ns = \"# no\"\ns = '# no'\n";
    let got = comments_in("a.py", src);
    assert_eq!(lines(&got), vec![1, 2]);
    assert_eq!(got[0].text, "# a");
    assert_eq!(got[1].text, "# b");
}

#[test]
fn test_09_python_triple_quoted_multiline() {
    let src = "s = \"\"\"line 1\n# no\nline 3\"\"\"\n# yes\n";
    let got = comments_in("a.py", src);
    assert_eq!(lines(&got), vec![4]);
    assert_eq!(got[0].text, "# yes");
}

#[test]
fn test_10_python_shebang_line_1_vs_line_3() {
    let line1 = "#!/usr/bin/env python\nx = 1\n";
    assert_eq!(lines(&comments_in("a.py", line1)), Vec::<usize>::new());

    let line3 = "x = 1\ny = 2\n#!/usr/bin/env python\n";
    assert_eq!(lines(&comments_in("a.py", line3)), vec![3]);
}

#[test]
fn test_11_python_directives() {
    let src = "x = f()  # noqa: E501\n# type: ignore\n# pragma: no cover\n";
    assert_eq!(lines(&comments_in("a.py", src)), Vec::<usize>::new());
}

#[test]
fn test_12_typescript_templates_directives_and_jsdoc() {
    let t1 = "const s = `a // ${b} // c`;";
    assert_eq!(lines(&comments_in("a.ts", t1)), Vec::<usize>::new());

    let t2 = "const s = `line 1\n// line 2`;";
    assert_eq!(lines(&comments_in("a.ts", t2)), Vec::<usize>::new());

    let directives = "// @ts-expect-error\n// eslint-disable-next-line x\n";
    assert_eq!(lines(&comments_in("a.ts", directives)), Vec::<usize>::new());

    assert_eq!(
        lines(&comments_in("a.ts", "/** jsdoc */")),
        Vec::<usize>::new()
    );
    assert_eq!(lines(&comments_in("a.ts", "/* plain */")), vec![1]);
}

#[test]
fn test_13_javascript_regex_escaped_slash() {
    let src = "const r = /a\\//; // c";
    let got = comments_in("a.js", src);
    assert_eq!(lines(&got), vec![1]);
    assert_eq!(got[0].text, "// c");
}

#[test]
fn test_14_is_supported() {
    for p in ["a/b.rs", "x.py", "x.tsx", "x.cjs"] {
        assert!(is_supported(p), "expected {p} to be supported");
    }
    for p in ["x.sh", "x.yml", "x.md", "Makefile"] {
        assert!(!is_supported(p), "expected {p} to be unsupported");
    }
}

#[test]
fn test_15_added_lines() {
    let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 line1
+line2
 line3
@@ -10,1 +11,2 @@
 line10
+line12
diff --git a/b.py b/b.py
--- a/b.py
+++ b/b.py
@@ -0,0 +1,2 @@
+first
+second
diff --git a/old.rs b/old.rs
--- a/old.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-gone1
-gone2
";
    let got = added_lines(diff);
    assert_eq!(got.len(), 2);
    assert_eq!(got.get("a.rs"), Some(&BTreeSet::from([2, 12])));
    assert_eq!(got.get("b.py"), Some(&BTreeSet::from([1, 2])));
    assert!(!got.contains_key("old.rs"));
}

#[test]
fn test_16_added_comments_filters_lines_and_skips_unsupported() {
    let diff = "\
+++ b/a.rs
@@ -1,3 +1,5 @@
 // old on line 1
+// added on line 2
 let a = 1;
 let b = 2;
+// added on line 5
+++ b/notes.md
@@ -0,0 +1,1 @@
+// in markdown
";
    let file_a =
        "// old on line 1\n// added on line 2\nlet a = 1;\nlet b = 2;\n// added on line 5\n";
    let got = added_comments(diff, &|path| {
        if path == "a.rs" {
            Ok(file_a.to_string())
        } else {
            panic!("unexpected read for {path}");
        }
    })
    .unwrap();
    assert_eq!(lines(&got), vec![2, 5]);
}

#[test]
fn test_17_added_comments_propagates_read_error() {
    let diff = "\
+++ b/a.rs
@@ -0,0 +1,1 @@
+// a
";
    let err = added_comments(diff, &|_| Err("gone".to_string())).unwrap_err();
    assert_eq!(err, "cannot read a.rs: gone");
}

#[test]
fn test_18_flag_formatting() {
    assert_eq!(flag(&[]), None);

    let two = vec![
        Comment {
            file: "a.rs".to_string(),
            line: 2,
            text: "// a".to_string(),
        },
        Comment {
            file: "b.py".to_string(),
            line: 7,
            text: "# b".to_string(),
        },
    ];
    assert_eq!(
        flag(&two),
        Some(
            "comments added (2): a.rs:2, b.py:7: remove them, and check what each one was covering for"
                .to_string()
        )
    );

    let five: Vec<Comment> = (1..=5)
        .map(|i| Comment {
            file: "a.rs".to_string(),
            line: i,
            text: "// x".to_string(),
        })
        .collect();
    assert_eq!(
        flag(&five),
        Some(
            "comments added (5): a.rs:1, a.rs:2, a.rs:3 and 2 more: remove them, and check what each one was covering for"
                .to_string()
        )
    );
}
