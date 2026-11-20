use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Map {
    pub repo: String,
    pub languages: Vec<String>,
    pub groups: Vec<Group>,
    pub units: Vec<Unit>,
    pub edges: Vec<Edge>,
    pub structures: Vec<Structure>,
    pub unparsed: Vec<Unparsed>,
    pub unresolved: Vec<Unresolved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub lang: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub id: String,
    pub group: String,
    pub file: String,
    pub lang: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub level: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Structure {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub unit: String,
    pub file: String,
    pub line: u32,
    pub serialisable: bool,
    pub fields: Vec<Field>,
    pub referenced_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unparsed {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unresolved {
    pub file: String,
    pub line: u32,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Part {
    pub groups: Vec<Group>,
    pub units: Vec<Unit>,
    pub edges: Vec<Edge>,
    pub structures: Vec<Structure>,
    pub unparsed: Vec<Unparsed>,
    pub unresolved: Vec<Unresolved>,
}

pub fn build(root: &Path, repo: &str) -> Result<Map, String> {
    let mut languages = Vec::new();
    let mut parts = Vec::new();

    if root.join("Cargo.toml").exists() {
        let part = crate::map_rust::extract(root)?;
        languages.push("rust".to_string());
        parts.push(part);
    }

    let py_part = crate::map_python::extract(root)?;
    if !py_part.units.is_empty() {
        languages.push("python".to_string());
        parts.push(py_part);
    }

    if languages.is_empty() {
        return Err(format!(
            "no supported language in {}: no Cargo.toml at the root and no Python modules",
            root.display()
        ));
    }

    Ok(finish(repo, languages, parts))
}

pub fn finish(repo: &str, languages: Vec<String>, parts: Vec<Part>) -> Map {
    let mut groups = Vec::new();
    let mut units = Vec::new();
    let mut edges = Vec::new();
    let mut structures = Vec::new();
    let mut unparsed = Vec::new();
    let mut unresolved = Vec::new();

    for part in parts {
        groups.extend(part.groups);
        units.extend(part.units);
        edges.extend(part.edges);
        structures.extend(part.structures);
        unparsed.extend(part.unparsed);
        unresolved.extend(part.unresolved);
    }

    groups.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.lang.cmp(&b.lang))
    });
    groups.dedup();

    units.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.lang.cmp(&b.lang))
    });
    units.dedup();

    edges.retain(|e| e.from != e.to);
    edges.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    edges.dedup();

    for structure in &mut structures {
        structure.referenced_by.retain(|u| u != &structure.unit);
        structure.referenced_by.sort();
        structure.referenced_by.dedup();
    }
    structures.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    structures.dedup();

    unparsed.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.reason.cmp(&b.reason)));
    unparsed.dedup();

    unresolved.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.reference.cmp(&b.reference))
    });
    unresolved.dedup();

    Map {
        repo: repo.to_string(),
        languages,
        groups,
        units,
        edges,
        structures,
        unparsed,
        unresolved,
    }
}
