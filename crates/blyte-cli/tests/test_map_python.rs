use std::fs;
use std::path::Path;

use blyte_cli::map::{self, Edge, Field, Group, Unit, Unresolved};
use tempfile::tempdir;

fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn modules_packages_and_groups() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/__init__.py", "");
    write_file(dir.path(), "pkg/a.py", "");
    write_file(dir.path(), "pkg/sub/b.py", "");
    write_file(dir.path(), "tool.py", "");

    let m = map::build(dir.path(), "demo").unwrap();
    assert_eq!(
        m.units,
        vec![
            Unit {
                id: "pkg".to_string(),
                group: "pkg".to_string(),
                file: "pkg/__init__.py".to_string(),
                lang: "python".to_string(),
            },
            Unit {
                id: "pkg.a".to_string(),
                group: "pkg".to_string(),
                file: "pkg/a.py".to_string(),
                lang: "python".to_string(),
            },
            Unit {
                id: "pkg.sub.b".to_string(),
                group: "pkg.sub".to_string(),
                file: "pkg/sub/b.py".to_string(),
                lang: "python".to_string(),
            },
            Unit {
                id: "tool".to_string(),
                group: ".".to_string(),
                file: "tool.py".to_string(),
                lang: "python".to_string(),
            },
        ]
    );
    assert_eq!(
        m.groups,
        vec![
            Group {
                id: ".".to_string(),
                kind: "package".to_string(),
                path: ".".to_string(),
                lang: "python".to_string(),
            },
            Group {
                id: "pkg".to_string(),
                kind: "package".to_string(),
                path: "pkg".to_string(),
                lang: "python".to_string(),
            },
            Group {
                id: "pkg.sub".to_string(),
                kind: "package".to_string(),
                path: "pkg/sub".to_string(),
                lang: "python".to_string(),
            },
        ]
    );
}

#[test]
fn src_layout_drops_src() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "src/pkg/a.py", "");

    let m = map::build(dir.path(), "src_demo").unwrap();
    assert_eq!(
        m.units,
        vec![Unit {
            id: "pkg.a".to_string(),
            group: "pkg".to_string(),
            file: "src/pkg/a.py".to_string(),
            lang: "python".to_string(),
        }]
    );
    assert_eq!(
        m.groups,
        vec![Group {
            id: "pkg".to_string(),
            kind: "package".to_string(),
            path: "src/pkg".to_string(),
            lang: "python".to_string(),
        }]
    );
}

#[test]
fn tests_and_virtualenvs_are_left_out() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/core.py", "");
    write_file(dir.path(), "tests/test_a.py", "");
    write_file(dir.path(), "test/helper.py", "");
    write_file(dir.path(), "pkg/test_b.py", "");
    write_file(dir.path(), "pkg/c_test.py", "");
    write_file(dir.path(), "pkg/conftest.py", "");
    write_file(dir.path(), ".venv/lib/x.py", "");
    write_file(dir.path(), "venv/lib/y.py", "");
    write_file(dir.path(), "__pycache__/z.py", "");
    write_file(dir.path(), "node_modules/m/n.py", "");

    let m = map::build(dir.path(), "filtered").unwrap();
    let ids: Vec<&str> = m.units.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, vec!["pkg.core"]);
}

#[test]
fn absolute_and_relative_imports() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/__init__.py", "");
    write_file(dir.path(), "pkg/a.py", "def f():\n    pass\n");
    write_file(dir.path(), "pkg/other.py", "def g():\n    pass\n");
    write_file(dir.path(), "pkg/extra.py", "def h():\n    pass\n");
    write_file(dir.path(), "pkg/sub/__init__.py", "");
    write_file(dir.path(), "pkg/sub/b.py", "from ..other import g\n");
    write_file(dir.path(), "pkg/c.py", "from . import a\n");
    write_file(dir.path(), "app1.py", "import pkg.a\n");
    write_file(dir.path(), "app2.py", "from pkg import a\n");
    write_file(
        dir.path(),
        "app3.py",
        concat!(
            "from pkg.a import f\n",
            "from pkg import (\n",
            "    extra,\n",
            ")\n",
            "def run():\n",
            "    from pkg.other import g\n",
            "    return g()\n",
        ),
    );

    let m = map::build(dir.path(), "imports").unwrap();
    let unit_edges: Vec<(&str, &str)> = m
        .edges
        .iter()
        .filter(|e| e.level == "unit")
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(
        unit_edges,
        vec![
            ("app1", "pkg.a"),
            ("app2", "pkg.a"),
            ("app3", "pkg.a"),
            ("app3", "pkg.extra"),
            ("app3", "pkg.other"),
            ("pkg.c", "pkg.a"),
            ("pkg.sub.b", "pkg.other"),
        ]
    );
}

#[test]
fn script_directory_import() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "scripts/helper.py", "def h():\n    return 1\n");
    write_file(dir.path(), "scripts/run.py", "from helper import h\n");

    let m = map::build(dir.path(), "scripts_repo").unwrap();
    assert_eq!(
        m.edges,
        vec![Edge {
            from: "scripts.run".to_string(),
            to: "scripts.helper".to_string(),
            level: "unit".to_string(),
        }]
    );
    assert!(m.unresolved.is_empty());
}

#[test]
fn outside_imports_ignored_inside_unresolved() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/__init__.py", "");
    write_file(dir.path(), "pkg/real.py", "");
    write_file(
        dir.path(),
        "tool.py",
        concat!(
            "import os\n",
            "from requests import get\n",
            "from pkg.nothere import x\n",
            "from ... import x\n",
        ),
    );

    let m = map::build(dir.path(), "unres").unwrap();
    assert!(m.edges.is_empty());
    assert_eq!(
        m.unresolved,
        vec![
            Unresolved {
                file: "tool.py".to_string(),
                line: 3,
                reference: "pkg.nothere".to_string(),
            },
            Unresolved {
                file: "tool.py".to_string(),
                line: 4,
                reference: "...".to_string(),
            },
        ]
    );
}

#[test]
fn imports_in_strings_and_comments_are_ignored() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/__init__.py", "");
    write_file(dir.path(), "pkg/a.py", "");
    write_file(
        dir.path(),
        "app.py",
        concat!(
            "\"\"\"\n",
            "from pkg import a\n",
            "\"\"\"\n",
            "# import pkg.a\n",
            "s = \"import pkg.a\"\n",
            "r_str = r'from pkg import a'\n",
            "f_str = f\"import pkg.a\"\n",
        ),
    );

    let m = map::build(dir.path(), "strings").unwrap();
    assert!(m.edges.is_empty());
    assert!(m.unresolved.is_empty());
}

#[test]
fn structures_and_fields() {
    let dir = tempdir().unwrap();
    write_file(
        dir.path(),
        "models.py",
        concat!(
            "import dataclasses\n",
            "from dataclasses import dataclass\n",
            "from typing import ClassVar, TypedDict\n",
            "from pydantic import BaseModel\n",
            "\n",
            "@dataclass\n",
            "class Item:\n",
            "    name: str\n",
            "    count: int = 0\n",
            "    tag: ClassVar[str] = \"x\"\n",
            "\n",
            "@dataclasses.dataclass(frozen=True)\n",
            "class Frozen:\n",
            "    key: str\n",
            "\n",
            "class M(BaseModel):\n",
            "    id: int\n",
            "    title: str = \"untitled\"\n",
            "\n",
            "class T(TypedDict):\n",
            "    ok: bool\n",
            "    msg: str\n",
            "\n",
            "class Plain:\n",
            "    x: int\n",
            "\n",
            "def make():\n",
            "    @dataclass\n",
            "    class Nested:\n",
            "        y: int\n",
            "    return Nested\n",
        ),
    );

    let m = map::build(dir.path(), "structs").unwrap();
    let names: Vec<&str> = m.structures.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Frozen", "Item", "M", "T"]);

    let item = m.structures.iter().find(|s| s.name == "Item").unwrap();
    assert_eq!(item.id, "models.Item");
    assert_eq!(item.kind, "dataclass");
    assert_eq!(item.line, 7);
    assert!(item.serialisable);
    assert_eq!(
        item.fields,
        vec![
            Field {
                name: "name".to_string(),
                ty: "str".to_string(),
            },
            Field {
                name: "count".to_string(),
                ty: "int".to_string(),
            },
        ]
    );

    let frozen = m.structures.iter().find(|s| s.name == "Frozen").unwrap();
    assert_eq!(frozen.id, "models.Frozen");
    assert_eq!(frozen.kind, "dataclass");
    assert_eq!(frozen.line, 13);
    assert!(frozen.serialisable);
    assert_eq!(
        frozen.fields,
        vec![Field {
            name: "key".to_string(),
            ty: "str".to_string(),
        }]
    );

    let model = m.structures.iter().find(|s| s.name == "M").unwrap();
    assert_eq!(model.id, "models.M");
    assert_eq!(model.kind, "model");
    assert_eq!(model.line, 16);
    assert!(model.serialisable);
    assert_eq!(
        model.fields,
        vec![
            Field {
                name: "id".to_string(),
                ty: "int".to_string(),
            },
            Field {
                name: "title".to_string(),
                ty: "str".to_string(),
            },
        ]
    );

    let td = m.structures.iter().find(|s| s.name == "T").unwrap();
    assert_eq!(td.id, "models.T");
    assert_eq!(td.kind, "typeddict");
    assert_eq!(td.line, 20);
    assert!(td.serialisable);
    assert_eq!(
        td.fields,
        vec![
            Field {
                name: "ok".to_string(),
                ty: "bool".to_string(),
            },
            Field {
                name: "msg".to_string(),
                ty: "str".to_string(),
            },
        ]
    );
}

#[test]
fn references_direct_reexported_and_by_module() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "pkg/__init__.py", "from .models import Rec\n");
    write_file(
        dir.path(),
        "pkg/models.py",
        concat!(
            "from dataclasses import dataclass\n",
            "\n",
            "@dataclass\n",
            "class Rec:\n",
            "    x: int\n",
            "\n",
            "def other():\n",
            "    return 1\n",
        ),
    );
    write_file(dir.path(), "direct.py", "from pkg.models import Rec\n");
    write_file(dir.path(), "reexp.py", "from pkg import Rec\n");
    write_file(
        dir.path(),
        "bymod.py",
        concat!("import pkg.models as m\n", "val = m.Rec(1)\n",),
    );
    write_file(dir.path(), "unrelated.py", "from pkg.models import other\n");

    let m = map::build(dir.path(), "refs").unwrap();
    let rec = m
        .structures
        .iter()
        .find(|s| s.id == "pkg.models.Rec")
        .unwrap();
    assert!(rec.referenced_by.contains(&"direct".to_string()));
    assert!(rec.referenced_by.contains(&"reexp".to_string()));
    assert!(rec.referenced_by.contains(&"bymod".to_string()));
    assert!(!rec.referenced_by.contains(&"unrelated".to_string()));
}

#[test]
fn group_edges_cross_packages_only() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "alpha/__init__.py", "");
    write_file(dir.path(), "alpha/a.py", "");
    write_file(dir.path(), "alpha/b.py", "from . import a\n");
    write_file(dir.path(), "beta/__init__.py", "");
    write_file(dir.path(), "beta/c.py", "from alpha import a\n");

    let m = map::build(dir.path(), "groups").unwrap();
    let group_edges: Vec<&Edge> = m.edges.iter().filter(|e| e.level == "group").collect();
    assert_eq!(
        group_edges,
        vec![&Edge {
            from: "beta".to_string(),
            to: "alpha".to_string(),
            level: "group".to_string(),
        }]
    );
}

#[test]
fn build_python_only_and_mixed() {
    let py_dir = tempdir().unwrap();
    write_file(py_dir.path(), "pkg/a.py", "x = 1\n");
    let py_map = map::build(py_dir.path(), "pyonly").unwrap();
    assert_eq!(py_map.languages, vec!["python".to_string()]);
    assert_eq!(py_map.units.len(), 1);
    assert_eq!(py_map.units[0].id, "pkg.a");

    let mixed_dir = tempdir().unwrap();
    write_file(
        mixed_dir.path(),
        "Cargo.toml",
        "[package]\nname = \"hybrid\"\nversion = \"0.1.0\"\n",
    );
    write_file(mixed_dir.path(), "src/lib.rs", "pub fn hi() {}\n");
    write_file(mixed_dir.path(), "scripts/tool.py", "x = 1\n");
    let mixed_map = map::build(mixed_dir.path(), "hybrid").unwrap();
    assert_eq!(
        mixed_map.languages,
        vec!["rust".to_string(), "python".to_string()]
    );
    assert!(mixed_map
        .units
        .iter()
        .any(|u| u.id == "hybrid" && u.lang == "rust"));
    assert!(mixed_map
        .units
        .iter()
        .any(|u| u.id == "scripts.tool" && u.lang == "python"));

    let empty_dir = tempdir().unwrap();
    let err = map::build(empty_dir.path(), "empty").unwrap_err();
    assert_eq!(
        err,
        format!(
            "no supported language in {}: no Cargo.toml at the root and no Python modules",
            empty_dir.path().display()
        )
    );
}

#[test]
fn nested_src_directories_name_modules() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "service-a/src/service_a/__init__.py", "");
    write_file(dir.path(), "service-a/src/service_a/api.py", "");
    write_file(dir.path(), "shared/src/shared_utils/geo.py", "");

    let m = map::build(dir.path(), "multi").unwrap();
    let ids: Vec<&str> = m.units.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, vec!["service_a", "service_a.api", "shared_utils.geo"]);

    let svc_group = m.groups.iter().find(|g| g.id == "service_a").unwrap();
    assert_eq!(svc_group.path, "service-a/src/service_a");
}

#[test]
fn imports_resolve_across_projects() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "service-a/src/service_a/__init__.py", "");
    write_file(
        dir.path(),
        "service-a/src/service_a/api.py",
        concat!(
            "from shared_utils.geo import area\n",
            "from shared_utils import geo\n",
        ),
    );
    write_file(
        dir.path(),
        "shared/src/shared_utils/geo.py",
        "def area():\n    return 0\n",
    );

    let m = map::build(dir.path(), "cross").unwrap();
    let unit_edges: Vec<&Edge> = m.edges.iter().filter(|e| e.level == "unit").collect();
    assert_eq!(
        unit_edges,
        vec![&Edge {
            from: "service_a.api".to_string(),
            to: "shared_utils.geo".to_string(),
            level: "unit".to_string(),
        }]
    );
    let group_edges: Vec<&Edge> = m.edges.iter().filter(|e| e.level == "group").collect();
    assert_eq!(
        group_edges,
        vec![&Edge {
            from: "service_a".to_string(),
            to: "shared_utils".to_string(),
            level: "group".to_string(),
        }]
    );
    assert!(m.unresolved.is_empty());
}

#[test]
fn file_directly_in_nested_src() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "svc/src/main.py", "x = 1\n");

    let m = map::build(dir.path(), "direct_src").unwrap();
    assert_eq!(
        m.units,
        vec![Unit {
            id: "main".to_string(),
            group: "svc.src".to_string(),
            file: "svc/src/main.py".to_string(),
            lang: "python".to_string(),
        }]
    );
    assert_eq!(
        m.groups,
        vec![Group {
            id: "svc.src".to_string(),
            kind: "package".to_string(),
            path: "svc/src".to_string(),
            lang: "python".to_string(),
        }]
    );
}

#[test]
fn duplicate_module_names_are_an_error() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "a/src/pkg/x.py", "");
    write_file(dir.path(), "b/src/pkg/x.py", "");

    let err = map::build(dir.path(), "dup").unwrap_err();
    assert_eq!(
        err,
        "two Python files have the module name pkg.x: a/src/pkg/x.py and b/src/pkg/x.py"
    );
}

#[test]
fn root_src_and_no_src_unchanged() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "src/pkg/a.py", "");
    write_file(dir.path(), "tool.py", "");

    let m = map::build(dir.path(), "mixed_layout").unwrap();
    let ids: Vec<&str> = m.units.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, vec!["pkg.a", "tool"]);
}
