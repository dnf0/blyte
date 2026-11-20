use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use blyte_cli::map::{self, Edge, Field, Group, Unit};
use tempfile::tempdir;

fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn workspace_members_become_groups() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/skip\"]\n",
    );
    write_file(
        dir.path(),
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "crates/alpha/src/lib.rs", "");
    write_file(
        dir.path(),
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "crates/beta/src/lib.rs", "");
    write_file(
        dir.path(),
        "crates/skip/Cargo.toml",
        "[package]\nname = \"skip\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "crates/skip/src/lib.rs", "");

    let m = map::build(dir.path(), "demo").unwrap();
    assert_eq!(
        m.groups,
        vec![
            Group {
                id: "alpha".to_string(),
                kind: "crate".to_string(),
                path: "crates/alpha".to_string(),
                lang: "rust".to_string(),
            },
            Group {
                id: "beta".to_string(),
                kind: "crate".to_string(),
                path: "crates/beta".to_string(),
                lang: "rust".to_string(),
            },
        ]
    );
}

#[test]
fn unsupported_member_pattern_is_an_error() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/**/x\"]\n",
    );

    let err = map::build(dir.path(), "demo").unwrap_err();
    assert!(
        err.contains("crates/**/x"),
        "expected error to name pattern, got: {err}"
    );
}

#[test]
fn single_package_at_root() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "src/lib.rs", "pub fn hi() {}\n");

    let m = map::build(dir.path(), "solo").unwrap();
    assert_eq!(
        m.groups,
        vec![Group {
            id: "solo".to_string(),
            kind: "crate".to_string(),
            path: ".".to_string(),
            lang: "rust".to_string(),
        }]
    );
    assert_eq!(
        m.units,
        vec![Unit {
            id: "solo".to_string(),
            group: "solo".to_string(),
            file: "src/lib.rs".to_string(),
            lang: "rust".to_string(),
        }]
    );
}

#[test]
fn module_files_resolve() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir.path(),
        "src/lib.rs",
        "mod a;\nmod b;\n#[path = \"odd/d.rs\"]\nmod d;\n",
    );
    write_file(dir.path(), "src/a.rs", "mod c;\n");
    write_file(dir.path(), "src/a/c.rs", "");
    write_file(dir.path(), "src/b/mod.rs", "");
    write_file(dir.path(), "src/odd/d.rs", "");

    let m = map::build(dir.path(), "x").unwrap();
    let got: Vec<(&str, &str)> = m
        .units
        .iter()
        .map(|u| (u.id.as_str(), u.file.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("x", "src/lib.rs"),
            ("x::a", "src/a.rs"),
            ("x::a::c", "src/a/c.rs"),
            ("x::b", "src/b/mod.rs"),
            ("x::d", "src/odd/d.rs"),
        ]
    );
    assert!(m.unresolved.is_empty());
}

#[test]
fn lib_name_and_bin_units() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"my-tool\"\nversion = \"0.1.0\"\n\n[lib]\nname = \"tool_lib\"\n",
    );
    write_file(dir.path(), "src/lib.rs", "pub fn run() {}\n");
    write_file(
        dir.path(),
        "src/main.rs",
        "fn main() { tool_lib::run(); }\n",
    );

    let m = map::build(dir.path(), "my-tool").unwrap();
    let unit_ids: Vec<&str> = m.units.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(unit_ids, vec!["bin:my-tool", "tool_lib"]);
    assert_eq!(
        m.edges,
        vec![Edge {
            from: "bin:my-tool".to_string(),
            to: "tool_lib".to_string(),
            level: "unit".to_string(),
        }]
    );
}

#[test]
fn edges_from_use_and_inline_paths() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\npub mod d;\npub mod e;\n",
    );
    write_file(dir.path(), "src/a.rs", "pub mod c;\npub struct Thing;\n");
    write_file(dir.path(), "src/a/c.rs", "pub struct Other;\n");
    write_file(dir.path(), "src/b.rs", "pub fn x() {}\n");
    write_file(dir.path(), "src/d.rs", "pub fn go() {}\n");
    write_file(
        dir.path(),
        "src/e.rs",
        "use crate::a::Thing;\n\
         use crate::{b, a::c::{Other, self}};\n\
         use crate::e::local;\n\
         pub fn local() {}\n\
         pub fn run() {\n\
             let _ = (Thing, Other);\n\
             crate::d::go();\n\
             super::b::x();\n\
             b::x();\n\
         }\n",
    );

    let m = map::build(dir.path(), "app").unwrap();
    let e_edges: Vec<&Edge> = m.edges.iter().filter(|e| e.from == "app::e").collect();
    assert_eq!(
        e_edges,
        vec![
            &Edge {
                from: "app::e".to_string(),
                to: "app::a".to_string(),
                level: "unit".to_string(),
            },
            &Edge {
                from: "app::e".to_string(),
                to: "app::a::c".to_string(),
                level: "unit".to_string(),
            },
            &Edge {
                from: "app::e".to_string(),
                to: "app::b".to_string(),
                level: "unit".to_string(),
            },
            &Edge {
                from: "app::e".to_string(),
                to: "app::d".to_string(),
                level: "unit".to_string(),
            },
        ]
    );
}

#[test]
fn cross_crate_edge_and_reexport() {
    let dir1 = tempdir().unwrap();
    write_file(
        dir1.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/one\", \"crates/two\"]\n",
    );
    write_file(
        dir1.path(),
        "crates/one/Cargo.toml",
        "[package]\nname = \"one\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir1.path(),
        "crates/one/src/lib.rs",
        "pub mod inner;\npub use inner::deep::Item;\n",
    );
    write_file(dir1.path(), "crates/one/src/inner.rs", "pub mod deep;\n");
    write_file(
        dir1.path(),
        "crates/one/src/inner/deep.rs",
        "pub struct Item;\nimpl Item { pub fn new() -> Self { Self } }\n",
    );
    write_file(
        dir1.path(),
        "crates/two/Cargo.toml",
        "[package]\nname = \"two\"\nversion = \"0.1.0\"\n\n[dependencies]\none = { path = \"../one\" }\n",
    );
    write_file(
        dir1.path(),
        "crates/two/src/lib.rs",
        "pub fn make() { let _ = one::Item::new(); }\n",
    );

    let m1 = map::build(dir1.path(), "ws").unwrap();
    assert!(m1.edges.contains(&Edge {
        from: "two".to_string(),
        to: "one::inner::deep".to_string(),
        level: "unit".to_string(),
    }));
    assert!(m1.edges.contains(&Edge {
        from: "two".to_string(),
        to: "one".to_string(),
        level: "group".to_string(),
    }));
    let item = m1
        .structures
        .iter()
        .find(|s| s.id == "one::inner::deep::Item")
        .unwrap();
    assert_eq!(item.referenced_by, vec!["two".to_string()]);

    let dir2 = tempdir().unwrap();
    write_file(
        dir2.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/one\", \"crates/two\"]\n\n[workspace.dependencies]\none = { path = \"crates/one\" }\n",
    );
    write_file(
        dir2.path(),
        "crates/one/Cargo.toml",
        "[package]\nname = \"one\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir2.path(), "crates/one/src/lib.rs", "");
    write_file(
        dir2.path(),
        "crates/two/Cargo.toml",
        "[package]\nname = \"two\"\nversion = \"0.1.0\"\n\n[dependencies]\none.workspace = true\n",
    );
    write_file(dir2.path(), "crates/two/src/lib.rs", "");

    let m2 = map::build(dir2.path(), "ws2").unwrap();
    assert!(m2.edges.contains(&Edge {
        from: "two".to_string(),
        to: "one".to_string(),
        level: "group".to_string(),
    }));
}

#[test]
fn cfg_test_is_ignored() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir.path(),
        "src/lib.rs",
        "pub mod z;\n\
         #[cfg(test)]\n\
         mod tests {\n\
             use crate::z::Y;\n\
             struct Local;\n\
             fn check() { let _ = (Y, Local); }\n\
         }\n\
         pub struct After;\n",
    );
    write_file(dir.path(), "src/z.rs", "pub struct Y;\n");

    let m = map::build(dir.path(), "p").unwrap();
    assert!(m.edges.is_empty());
    let ids: Vec<&str> = m.structures.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["p::After", "p::z::Y"]);
}

#[test]
fn structures_fields_and_derives() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"data\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir.path(),
        "src/lib.rs",
        "use std::collections::HashMap;\n\
         use std::path::PathBuf;\n\
         \n\
         #[derive(Debug, serde::Serialize)]\n\
         pub struct Record {\n\
             pub names: Vec<String>,\n\
             pub path: Option<PathBuf>,\n\
             pub counts: HashMap<String, u64>,\n\
             pub tag: &'static str,\n\
             pub check: Box<dyn Fn(u32) -> bool>,\n\
         }\n\
         \n\
         pub struct Pair(String, u32);\n\
         \n\
         #[derive(Deserialize)]\n\
         pub enum Choice {\n\
             Empty,\n\
             Pair(String, u64),\n\
             Named { a: u32, b: Option<String> },\n\
         }\n\
         \n\
         pub fn helper() {\n\
             struct Hidden { x: u8 }\n\
             let _ = Hidden { x: 1 };\n\
         }\n",
    );

    let m = map::build(dir.path(), "data").unwrap();
    let names: Vec<&str> = m.structures.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Choice", "Pair", "Record"]);

    let record = m.structures.iter().find(|s| s.name == "Record").unwrap();
    assert_eq!(record.kind, "struct");
    assert_eq!(record.line, 5);
    assert!(record.serialisable);
    assert_eq!(
        record.fields,
        vec![
            Field {
                name: "names".to_string(),
                ty: "Vec<String>".to_string(),
            },
            Field {
                name: "path".to_string(),
                ty: "Option<PathBuf>".to_string(),
            },
            Field {
                name: "counts".to_string(),
                ty: "HashMap<String, u64>".to_string(),
            },
            Field {
                name: "tag".to_string(),
                ty: "&'static str".to_string(),
            },
            Field {
                name: "check".to_string(),
                ty: "Box<dyn Fn(u32) -> bool>".to_string(),
            },
        ]
    );

    let pair = m.structures.iter().find(|s| s.name == "Pair").unwrap();
    assert_eq!(pair.kind, "struct");
    assert_eq!(pair.line, 13);
    assert!(!pair.serialisable);
    assert_eq!(
        pair.fields,
        vec![
            Field {
                name: "0".to_string(),
                ty: "String".to_string(),
            },
            Field {
                name: "1".to_string(),
                ty: "u32".to_string(),
            },
        ]
    );

    let choice = m.structures.iter().find(|s| s.name == "Choice").unwrap();
    assert_eq!(choice.kind, "enum");
    assert_eq!(choice.line, 16);
    assert!(choice.serialisable);
    assert_eq!(
        choice.fields,
        vec![
            Field {
                name: "Empty".to_string(),
                ty: String::new(),
            },
            Field {
                name: "Pair".to_string(),
                ty: "(String, u64)".to_string(),
            },
            Field {
                name: "Named".to_string(),
                ty: "{a: u32, b: Option<String>}".to_string(),
            },
        ]
    );
}

#[test]
fn same_name_in_two_units() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"dup\"\nversion = \"0.1.0\"\n",
    );
    write_file(
        dir.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\npub mod c;\n",
    );
    write_file(dir.path(), "src/a.rs", "pub struct Sources;\n");
    write_file(dir.path(), "src/b.rs", "pub struct Sources;\n");
    write_file(
        dir.path(),
        "src/c.rs",
        "use crate::a::Sources;\npub fn f(_: Sources) {}\n",
    );

    let m = map::build(dir.path(), "dup").unwrap();
    let a_sources = m
        .structures
        .iter()
        .find(|s| s.id == "dup::a::Sources")
        .unwrap();
    let b_sources = m
        .structures
        .iter()
        .find(|s| s.id == "dup::b::Sources")
        .unwrap();
    assert_eq!(a_sources.referenced_by, vec!["dup::c".to_string()]);
    assert!(b_sources.referenced_by.is_empty());
}

#[test]
fn missing_module_file_is_unresolved() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "src/lib.rs", "mod gone;\n");

    let m = map::build(dir.path(), "x").unwrap();
    assert!(m.units.iter().all(|u| u.id != "x::gone"));
    assert_eq!(m.unresolved.len(), 1);
    assert_eq!(m.unresolved[0].file, "src/lib.rs");
    assert_eq!(m.unresolved[0].line, 1);
    assert_eq!(m.unresolved[0].reference, "mod gone");
}

#[test]
fn unparsable_file_is_listed() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "src/lib.rs", "mod broken;\n");
    write_file(dir.path(), "src/broken.rs", "fn (\n");

    let m = map::build(dir.path(), "x").unwrap();
    assert!(m.units.iter().any(|u| u.id == "x::broken"));
    assert!(m.edges.iter().all(|e| e.from != "x::broken"));
    assert_eq!(m.unparsed.len(), 1);
    assert_eq!(m.unparsed[0].file, "src/broken.rs");
    assert!(!m.unparsed[0].reason.is_empty());
}

#[test]
fn build_sorts_and_names_languages() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/zeta\", \"crates/alpha\"]\n",
    );
    write_file(
        dir.path(),
        "crates/zeta/Cargo.toml",
        "[package]\nname = \"zeta\"\nversion = \"0.1.0\"\n\n[dependencies]\nalpha = { path = \"../alpha\" }\n",
    );
    write_file(
        dir.path(),
        "crates/zeta/src/lib.rs",
        "pub mod b;\npub mod a;\npub struct Z;\n",
    );
    write_file(
        dir.path(),
        "crates/zeta/src/b.rs",
        "use alpha::A;\npub fn f(_: A) {}\n",
    );
    write_file(
        dir.path(),
        "crates/zeta/src/a.rs",
        "use crate::b;\npub fn g() { let _ = b::f; }\n",
    );
    write_file(
        dir.path(),
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\n",
    );
    write_file(dir.path(), "crates/alpha/src/lib.rs", "pub struct A;\n");

    let m = map::build(dir.path(), "two").unwrap();
    assert_eq!(m.languages, vec!["rust".to_string()]);

    let group_ids: Vec<&str> = m.groups.iter().map(|g| g.id.as_str()).collect();
    let mut sorted_groups = group_ids.clone();
    sorted_groups.sort();
    assert_eq!(group_ids, sorted_groups);

    let unit_ids: Vec<&str> = m.units.iter().map(|u| u.id.as_str()).collect();
    let mut sorted_units = unit_ids.clone();
    sorted_units.sort();
    assert_eq!(unit_ids, sorted_units);

    let struct_ids: Vec<&str> = m.structures.iter().map(|s| s.id.as_str()).collect();
    let mut sorted_structs = struct_ids.clone();
    sorted_structs.sort();
    assert_eq!(struct_ids, sorted_structs);

    let edge_keys: Vec<(&str, &str, &str)> = m
        .edges
        .iter()
        .map(|e| (e.level.as_str(), e.from.as_str(), e.to.as_str()))
        .collect();
    let mut sorted_edges = edge_keys.clone();
    sorted_edges.sort();
    assert_eq!(edge_keys, sorted_edges);

    let json = serde_json::to_value(&m).unwrap();
    let keys: BTreeSet<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let expected: BTreeSet<&str> = [
        "repo",
        "languages",
        "groups",
        "units",
        "edges",
        "structures",
        "unparsed",
        "unresolved",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);
}

#[test]
fn build_without_cargo_toml_is_an_error() {
    let dir = tempdir().unwrap();
    let err = map::build(dir.path(), "empty").unwrap_err();
    assert_eq!(
        err,
        format!(
            "no supported language in {}: no Cargo.toml at the root",
            dir.path().display()
        )
    );
}

#[test]
fn this_repository_maps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let m = map::build(root, "blyte").unwrap();

    assert_eq!(m.groups.len(), 3);
    assert!(m
        .units
        .iter()
        .any(|u| u.id == "blyte_cli::replay" && u.file == "crates/blyte-cli/src/replay.rs"));
    assert!(m.edges.contains(&Edge {
        from: "blyte_cli::replay".to_string(),
        to: "blyte_cli::dashboard".to_string(),
        level: "unit".to_string(),
    }));
    let replay = m
        .structures
        .iter()
        .find(|s| s.id == "blyte_cli::replay::Replay")
        .unwrap();
    assert!(replay.serialisable);
    assert!(m.unparsed.is_empty());
    assert!(m.unresolved.is_empty());
}
