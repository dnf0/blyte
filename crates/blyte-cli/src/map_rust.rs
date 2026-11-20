use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{
    Attribute, GenericArgument, ImplItem, Item, Meta, PathArguments, ReturnType, Token, TraitItem,
    Type, TypeParamBound, UseTree, Visibility,
};

use crate::map::{Edge, Field, Group, Part, Structure, Unit, Unparsed, Unresolved};

struct CrateInfo {
    pkg_name: String,
    dir_rel: PathBuf,
    manifest: toml::Value,
}

struct ParsedUnit {
    id: String,
    root_id: String,
    is_lib_root: bool,
    file_rel: String,
    ast: syn::File,
}

struct ReexportTarget {
    unit: String,
    extra: Vec<String>,
}

struct StructEntry {
    structure: Structure,
    rel_path: Vec<String>,
}

struct UnitWalkCtx<'a> {
    root: &'a Path,
    group: &'a str,
    root_id: &'a str,
    units: &'a mut Vec<Unit>,
    unparsed: &'a mut Vec<Unparsed>,
    unresolved: &'a mut Vec<Unresolved>,
    parsed: &'a mut Vec<ParsedUnit>,
    visited: &'a mut HashSet<(String, PathBuf)>,
}

pub fn extract(root: &Path) -> Result<Part, String> {
    let root_manifest_path = root.join("Cargo.toml");
    let root_manifest = read_manifest(&root_manifest_path)?;

    let crate_dirs = discover_crate_dirs(root, &root_manifest)?;
    let mut crates = Vec::with_capacity(crate_dirs.len());
    let mut dir_to_pkg: HashMap<PathBuf, String> = HashMap::new();
    let mut groups = Vec::with_capacity(crate_dirs.len());

    for dir_rel in crate_dirs {
        let manifest_path = if dir_rel.as_os_str().is_empty() {
            root_manifest_path.clone()
        } else {
            root.join(&dir_rel).join("Cargo.toml")
        };
        let manifest = if dir_rel.as_os_str().is_empty() {
            root_manifest.clone()
        } else {
            read_manifest(&manifest_path)?
        };
        let pkg_name = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("missing [package].name in {}", manifest_path.display()))?
            .to_string();
        let path_str = rel_to_string(&dir_rel);
        groups.push(Group {
            id: pkg_name.clone(),
            kind: "crate".to_string(),
            path: path_str,
            lang: "rust".to_string(),
        });
        dir_to_pkg.insert(dir_rel.clone(), pkg_name.clone());
        crates.push(CrateInfo {
            pkg_name,
            dir_rel,
            manifest,
        });
    }

    let workspace_deps = root_manifest
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table());

    let mut edges = Vec::new();
    for info in &crates {
        collect_group_edges(info, workspace_deps, &dir_to_pkg, &mut edges);
    }

    let mut units = Vec::new();
    let mut unparsed = Vec::new();
    let mut unresolved = Vec::new();
    let mut parsed_units = Vec::new();
    let mut lib_roots: HashSet<String> = HashSet::new();
    let mut visited: HashSet<(String, PathBuf)> = HashSet::new();

    for info in &crates {
        let targets = crate_targets(root, info)?;
        for (unit_id, is_lib_root, target_rel) in targets {
            if is_lib_root {
                lib_roots.insert(unit_id.clone());
            }
            let mut ctx = UnitWalkCtx {
                root,
                group: &info.pkg_name,
                root_id: &unit_id,
                units: &mut units,
                unparsed: &mut unparsed,
                unresolved: &mut unresolved,
                parsed: &mut parsed_units,
                visited: &mut visited,
            };
            visit_unit_file(&mut ctx, &unit_id, is_lib_root, true, &target_rel)?;
        }
    }

    let unit_set: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let reexports = collect_reexports(&parsed_units, &unit_set, &lib_roots);

    let mut struct_entries: Vec<StructEntry> = Vec::new();
    let mut structs_by_unit: HashMap<String, Vec<usize>> = HashMap::new();
    for pu in &parsed_units {
        let mut inline_mods = Vec::new();
        collect_structures(
            &pu.ast.items,
            &pu.id,
            &pu.file_rel,
            &mut inline_mods,
            &mut struct_entries,
            &mut structs_by_unit,
        );
    }

    for pu in &parsed_units {
        let mut collector = PathCollector::default();
        collector.visit_file(&pu.ast);
        let unit_segs: Vec<String> = pu.id.split("::").map(|s| s.to_string()).collect();

        for (raw_segs, inline_mods) in collector.paths {
            let Some(resolved) =
                resolve_counted_path(&raw_segs, &unit_segs, &inline_mods, &pu.root_id, &lib_roots)
            else {
                continue;
            };
            let Some((mut target_unit, mut remaining)) = longest_unit_prefix(&resolved, &unit_set)
            else {
                continue;
            };

            if lib_roots.contains(&target_unit) {
                if let Some(first_rem) = remaining.first() {
                    if let Some(reexp) = reexports.get(&(target_unit.clone(), first_rem.clone())) {
                        target_unit = reexp.unit.clone();
                        let mut new_rem = reexp.extra.clone();
                        new_rem.extend_from_slice(&remaining[1..]);
                        remaining = new_rem;
                    }
                }
            }

            if target_unit != pu.id {
                edges.push(Edge {
                    from: pu.id.clone(),
                    to: target_unit.clone(),
                    level: "unit".to_string(),
                });
                if let Some(indices) = structs_by_unit.get(&target_unit) {
                    for &idx in indices {
                        if remaining.starts_with(&struct_entries[idx].rel_path) {
                            struct_entries[idx]
                                .structure
                                .referenced_by
                                .push(pu.id.clone());
                        }
                    }
                }
            }
        }
    }

    let structures = struct_entries.into_iter().map(|e| e.structure).collect();

    Ok(Part {
        groups,
        units,
        edges,
        structures,
        unparsed,
        unresolved,
    })
}

fn read_manifest(path: &Path) -> Result<toml::Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str::<toml::Value>(&raw).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

fn has_glob_chars(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[') || pattern.contains(']')
}

fn discover_crate_dirs(root: &Path, root_manifest: &toml::Value) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    if let Some(workspace) = root_manifest.get("workspace") {
        let mut excluded: HashSet<PathBuf> = HashSet::new();
        if let Some(arr) = workspace.get("exclude").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    excluded.insert(normalize_rel(Path::new(s)));
                }
            }
        }

        if let Some(arr) = workspace.get("members").and_then(|v| v.as_array()) {
            for item in arr {
                let Some(pattern) = item.as_str() else {
                    continue;
                };
                if has_glob_chars(pattern) {
                    let Some(prefix) = pattern.strip_suffix("/*") else {
                        return Err(format!("unsupported workspace member pattern: {pattern}"));
                    };
                    if prefix.is_empty() || has_glob_chars(prefix) {
                        return Err(format!("unsupported workspace member pattern: {pattern}"));
                    }
                    let prefix_rel = normalize_rel(Path::new(prefix));
                    let parent_dir = root.join(&prefix_rel);
                    let entries = std::fs::read_dir(&parent_dir).map_err(|e| {
                        format!(
                            "cannot read workspace member directory {}: {e}",
                            parent_dir.display()
                        )
                    })?;
                    let mut children = Vec::new();
                    for entry in entries {
                        let entry = entry.map_err(|e| {
                            format!(
                                "cannot read workspace member directory {}: {e}",
                                parent_dir.display()
                            )
                        })?;
                        let path = entry.path();
                        if path.is_dir() && path.join("Cargo.toml").is_file() {
                            let child_rel = normalize_rel(&prefix_rel.join(entry.file_name()));
                            if !excluded.contains(&child_rel) {
                                children.push(child_rel);
                            }
                        }
                    }
                    children.sort();
                    for child in children {
                        if seen.insert(child.clone()) {
                            dirs.push(child);
                        }
                    }
                } else {
                    let rel = normalize_rel(Path::new(pattern));
                    if !excluded.contains(&rel) && seen.insert(rel.clone()) {
                        dirs.push(rel);
                    }
                }
            }
        }
    }

    if root_manifest.get("package").is_some() {
        let root_rel = PathBuf::new();
        if seen.insert(root_rel.clone()) {
            dirs.push(root_rel);
        }
    }

    if dirs.is_empty() {
        return Err(format!(
            "no crates found in {}",
            root.join("Cargo.toml").display()
        ));
    }

    Ok(dirs)
}

fn collect_group_edges(
    info: &CrateInfo,
    workspace_deps: Option<&toml::map::Map<String, toml::Value>>,
    dir_to_pkg: &HashMap<PathBuf, String>,
    edges: &mut Vec<Edge>,
) {
    let Some(deps) = info.manifest.get("dependencies").and_then(|d| d.as_table()) else {
        return;
    };
    for (dep_name, spec) in deps {
        let Some(table) = spec.as_table() else {
            continue;
        };
        let target_dir = if let Some(path_str) = table.get("path").and_then(|p| p.as_str()) {
            normalize_rel_checked(&info.dir_rel.join(path_str))
        } else if table.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
            workspace_deps
                .and_then(|ws| ws.get(dep_name))
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("path"))
                .and_then(|p| p.as_str())
                .and_then(|p| normalize_rel_checked(Path::new(p)))
        } else {
            None
        };

        if let Some(dir) = target_dir {
            if let Some(target_pkg) = dir_to_pkg.get(&dir) {
                if target_pkg != &info.pkg_name {
                    edges.push(Edge {
                        from: info.pkg_name.clone(),
                        to: target_pkg.clone(),
                        level: "group".to_string(),
                    });
                }
            }
        }
    }
}

fn crate_targets(root: &Path, info: &CrateInfo) -> Result<Vec<(String, bool, PathBuf)>, String> {
    let mut targets = Vec::new();
    let crate_abs = if info.dir_rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(&info.dir_rel)
    };
    let manifest_path = crate_abs.join("Cargo.toml");

    let lib_table = info.manifest.get("lib").and_then(|v| v.as_table());
    let lib_name = lib_table
        .and_then(|t| t.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| info.pkg_name.replace('-', "_"));
    let lib_path_rel = if let Some(p) = lib_table
        .and_then(|t| t.get("path"))
        .and_then(|p| p.as_str())
    {
        let rel = normalize_rel(&info.dir_rel.join(p));
        if !root.join(&rel).is_file() {
            return Err(format!(
                "missing [lib] path {} referenced by {}",
                root.join(&rel).display(),
                manifest_path.display()
            ));
        }
        Some(rel)
    } else {
        let default_rel = normalize_rel(&info.dir_rel.join("src/lib.rs"));
        if root.join(&default_rel).is_file() {
            Some(default_rel)
        } else {
            None
        }
    };
    if let Some(rel) = lib_path_rel {
        targets.push((lib_name, true, rel));
    }

    if let Some(bins) = info.manifest.get("bin").and_then(|v| v.as_array()) {
        for bin in bins {
            let Some(table) = bin.as_table() else {
                continue;
            };
            let bin_name = table
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| format!("missing [[bin]].name in {}", manifest_path.display()))?;
            let bin_rel = if let Some(p) = table.get("path").and_then(|p| p.as_str()) {
                let rel = normalize_rel(&info.dir_rel.join(p));
                if !root.join(&rel).is_file() {
                    return Err(format!(
                        "missing [[bin]] path {} referenced by {}",
                        root.join(&rel).display(),
                        manifest_path.display()
                    ));
                }
                Some(rel)
            } else {
                let candidates = [
                    info.dir_rel.join(format!("src/bin/{bin_name}.rs")),
                    info.dir_rel.join(format!("src/bin/{bin_name}/main.rs")),
                    info.dir_rel.join("src/main.rs"),
                ];
                candidates
                    .into_iter()
                    .map(|c| normalize_rel(&c))
                    .find(|c| root.join(c).is_file())
            };
            if let Some(rel) = bin_rel {
                targets.push((format!("bin:{bin_name}"), false, rel));
            }
        }
    } else {
        let default_main = normalize_rel(&info.dir_rel.join("src/main.rs"));
        if root.join(&default_main).is_file() {
            targets.push((format!("bin:{}", info.pkg_name), false, default_main));
        }
    }

    Ok(targets)
}

fn visit_unit_file(
    ctx: &mut UnitWalkCtx<'_>,
    unit_id: &str,
    is_lib_root: bool,
    is_root: bool,
    file_rel: &Path,
) -> Result<(), String> {
    let key = (unit_id.to_string(), file_rel.to_path_buf());
    if !ctx.visited.insert(key) {
        return Ok(());
    }

    let file_rel_str = rel_to_string(file_rel);
    ctx.units.push(Unit {
        id: unit_id.to_string(),
        group: ctx.group.to_string(),
        file: file_rel_str.clone(),
        lang: "rust".to_string(),
    });

    let abs_path = ctx.root.join(file_rel);
    let src = std::fs::read_to_string(&abs_path)
        .map_err(|e| format!("cannot read {}: {e}", abs_path.display()))?;

    let ast = match syn::parse_file(&src) {
        Ok(f) => f,
        Err(e) => {
            ctx.unparsed.push(Unparsed {
                file: file_rel_str,
                reason: e.to_string(),
            });
            return Ok(());
        }
    };

    let mut child_mods = Vec::new();
    let mut inline_mods = Vec::new();
    collect_mod_decls(&ast.items, &mut inline_mods, &mut child_mods);

    ctx.parsed.push(ParsedUnit {
        id: unit_id.to_string(),
        root_id: ctx.root_id.to_string(),
        is_lib_root,
        file_rel: file_rel_str.clone(),
        ast,
    });

    for (enclosing, mod_name, path_attr, line) in child_mods {
        let child_unit_id = if enclosing.is_empty() {
            format!("{unit_id}::{mod_name}")
        } else {
            format!("{unit_id}::{}::{mod_name}", enclosing.join("::"))
        };
        if let Some(child_rel) = resolve_mod_file(
            ctx.root,
            file_rel,
            is_root,
            &enclosing,
            &mod_name,
            path_attr.as_deref(),
        ) {
            visit_unit_file(ctx, &child_unit_id, false, false, &child_rel)?;
        } else {
            ctx.unresolved.push(Unresolved {
                file: file_rel_str.clone(),
                line,
                reference: format!("mod {mod_name}"),
            });
        }
    }

    Ok(())
}

type ModDecl = (Vec<String>, String, Option<String>, u32);

fn collect_mod_decls(items: &[Item], inline_mods: &mut Vec<String>, out: &mut Vec<ModDecl>) {
    for item in items {
        if is_cfg_test(item_attrs(item)) {
            continue;
        }
        if let Item::Mod(m) = item {
            if let Some((_, sub_items)) = &m.content {
                inline_mods.push(m.ident.to_string());
                collect_mod_decls(sub_items, inline_mods, out);
                inline_mods.pop();
            } else {
                let path_attr = extract_path_attr(&m.attrs);
                let line = m.ident.span().start().line as u32;
                out.push((inline_mods.clone(), m.ident.to_string(), path_attr, line));
            }
        }
    }
}

fn resolve_mod_file(
    root: &Path,
    decl_rel: &Path,
    is_root: bool,
    enclosing_inline: &[String],
    mod_name: &str,
    path_attr: Option<&str>,
) -> Option<PathBuf> {
    let decl_dir = decl_rel.parent().unwrap_or_else(|| Path::new(""));
    if let Some(p) = path_attr {
        let candidate = normalize_rel(&decl_dir.join(p));
        return root.join(&candidate).is_file().then_some(candidate);
    }

    let mut base_dir = if is_root || decl_rel.file_name() == Some(OsStr::new("mod.rs")) {
        decl_dir.to_path_buf()
    } else {
        let stem = decl_rel.file_stem()?;
        decl_dir.join(stem)
    };
    for im in enclosing_inline {
        base_dir = base_dir.join(im);
    }

    let file_candidate = normalize_rel(&base_dir.join(format!("{mod_name}.rs")));
    if root.join(&file_candidate).is_file() {
        return Some(file_candidate);
    }
    let dir_candidate = normalize_rel(&base_dir.join(mod_name).join("mod.rs"));
    if root.join(&dir_candidate).is_file() {
        return Some(dir_candidate);
    }
    None
}

fn collect_reexports(
    parsed_units: &[ParsedUnit],
    unit_set: &HashSet<String>,
    lib_roots: &HashSet<String>,
) -> HashMap<(String, String), ReexportTarget> {
    let mut map = HashMap::new();
    for pu in parsed_units {
        if !pu.is_lib_root {
            continue;
        }
        let inline_mod_names: HashSet<String> = pu
            .ast
            .items
            .iter()
            .filter_map(|item| {
                if is_cfg_test(item_attrs(item)) {
                    return None;
                }
                if let Item::Mod(m) = item {
                    if m.content.is_some() {
                        return Some(m.ident.to_string());
                    }
                }
                None
            })
            .collect();

        for item in &pu.ast.items {
            if is_cfg_test(item_attrs(item)) {
                continue;
            }
            let Item::Use(u) = item else {
                continue;
            };
            if !matches!(u.vis, Visibility::Public(_)) {
                continue;
            }
            let mut entries = Vec::new();
            let mut prefix = Vec::new();
            expand_use_tree_for_reexport(&u.tree, &mut prefix, &mut entries);
            for (raw_segs, exported_name) in entries {
                if raw_segs.is_empty() {
                    continue;
                }
                let resolved = match raw_segs[0].as_str() {
                    "crate" | "self" => {
                        let mut v = Vec::with_capacity(raw_segs.len());
                        v.push(pu.id.clone());
                        v.extend_from_slice(&raw_segs[1..]);
                        v
                    }
                    first if lib_roots.contains(first) => raw_segs,
                    first => {
                        let candidate_child = format!("{}::{first}", pu.id);
                        if unit_set.contains(&candidate_child) || inline_mod_names.contains(first) {
                            let mut v = Vec::with_capacity(raw_segs.len() + 1);
                            v.push(pu.id.clone());
                            v.extend(raw_segs);
                            v
                        } else {
                            continue;
                        }
                    }
                };
                if let Some((target_unit, extra)) = longest_unit_prefix(&resolved, unit_set) {
                    if target_unit != pu.id || !extra.is_empty() {
                        map.insert(
                            (pu.id.clone(), exported_name),
                            ReexportTarget {
                                unit: target_unit,
                                extra,
                            },
                        );
                    }
                }
            }
        }
    }
    map
}

fn expand_use_tree_for_reexport(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, String)>,
) {
    match tree {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            expand_use_tree_for_reexport(&p.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last().cloned() {
                    out.push((prefix.clone(), last));
                }
            } else {
                prefix.push(name.clone());
                out.push((prefix.clone(), name));
                prefix.pop();
            }
        }
        UseTree::Rename(r) => {
            let orig = r.ident.to_string();
            let alias = r.rename.to_string();
            if orig == "self" {
                if !prefix.is_empty() {
                    out.push((prefix.clone(), alias));
                }
            } else {
                prefix.push(orig);
                out.push((prefix.clone(), alias));
                prefix.pop();
            }
        }
        UseTree::Glob(_) => {}
        UseTree::Group(g) => {
            for item in &g.items {
                expand_use_tree_for_reexport(item, prefix, out);
            }
        }
    }
}

fn expand_use_tree_paths(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            expand_use_tree_paths(&p.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if !prefix.is_empty() {
                    out.push(prefix.clone());
                }
            } else {
                prefix.push(name);
                out.push(prefix.clone());
                prefix.pop();
            }
        }
        UseTree::Rename(r) => {
            let orig = r.ident.to_string();
            if orig == "self" {
                if !prefix.is_empty() {
                    out.push(prefix.clone());
                }
            } else {
                prefix.push(orig);
                out.push(prefix.clone());
                prefix.pop();
            }
        }
        UseTree::Glob(_) => {
            if !prefix.is_empty() {
                out.push(prefix.clone());
            }
        }
        UseTree::Group(g) => {
            for item in &g.items {
                expand_use_tree_paths(item, prefix, out);
            }
        }
    }
}

#[derive(Default)]
struct PathCollector {
    inline_mods: Vec<String>,
    paths: Vec<(Vec<String>, Vec<String>)>,
}

impl<'ast> Visit<'ast> for PathCollector {
    fn visit_attribute(&mut self, _attr: &'ast Attribute) {}

    fn visit_item(&mut self, item: &'ast Item) {
        if is_cfg_test(item_attrs(item)) {
            return;
        }
        match item {
            Item::Mod(m) => {
                if let Some((_, sub_items)) = &m.content {
                    self.inline_mods.push(m.ident.to_string());
                    for sub in sub_items {
                        self.visit_item(sub);
                    }
                    self.inline_mods.pop();
                }
            }
            Item::Use(u) => {
                let mut prefix = Vec::new();
                let mut expanded = Vec::new();
                expand_use_tree_paths(&u.tree, &mut prefix, &mut expanded);
                for segs in expanded {
                    self.paths.push((segs, self.inline_mods.clone()));
                }
            }
            other => syn::visit::visit_item(self, other),
        }
    }

    fn visit_impl_item(&mut self, i: &'ast ImplItem) {
        if is_cfg_test(impl_item_attrs(i)) {
            return;
        }
        syn::visit::visit_impl_item(self, i);
    }

    fn visit_trait_item(&mut self, i: &'ast TraitItem) {
        if is_cfg_test(trait_item_attrs(i)) {
            return;
        }
        syn::visit::visit_trait_item(self, i);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if !segs.is_empty() {
            self.paths.push((segs, self.inline_mods.clone()));
        }
        syn::visit::visit_path(self, path);
    }
}

fn resolve_counted_path(
    raw_segs: &[String],
    unit_segs: &[String],
    inline_mods: &[String],
    root_id: &str,
    lib_roots: &HashSet<String>,
) -> Option<Vec<String>> {
    let first = raw_segs.first()?;
    match first.as_str() {
        "crate" => {
            let mut v = Vec::with_capacity(raw_segs.len());
            v.push(root_id.to_string());
            v.extend_from_slice(&raw_segs[1..]);
            Some(v)
        }
        "self" | "super" => {
            if first == "self" && raw_segs.len() == 1 {
                return None;
            }
            let mut base = Vec::with_capacity(unit_segs.len() + inline_mods.len());
            base.extend_from_slice(unit_segs);
            base.extend_from_slice(inline_mods);
            let mut idx = 0;
            while idx < raw_segs.len() {
                match raw_segs[idx].as_str() {
                    "self" => idx += 1,
                    "super" => {
                        if base.len() <= 1 {
                            return None;
                        }
                        base.pop();
                        idx += 1;
                    }
                    _ => break,
                }
            }
            base.extend_from_slice(&raw_segs[idx..]);
            Some(base)
        }
        name if lib_roots.contains(name) => Some(raw_segs.to_vec()),
        _ => None,
    }
}

fn longest_unit_prefix(
    resolved: &[String],
    unit_set: &HashSet<String>,
) -> Option<(String, Vec<String>)> {
    for len in (1..=resolved.len()).rev() {
        let candidate = resolved[..len].join("::");
        if unit_set.contains(&candidate) {
            return Some((candidate, resolved[len..].to_vec()));
        }
    }
    None
}

fn collect_structures(
    items: &[Item],
    unit_id: &str,
    file_rel: &str,
    inline_mods: &mut Vec<String>,
    out: &mut Vec<StructEntry>,
    by_unit: &mut HashMap<String, Vec<usize>>,
) {
    for item in items {
        if is_cfg_test(item_attrs(item)) {
            continue;
        }
        match item {
            Item::Mod(m) => {
                if let Some((_, sub_items)) = &m.content {
                    inline_mods.push(m.ident.to_string());
                    collect_structures(sub_items, unit_id, file_rel, inline_mods, out, by_unit);
                    inline_mods.pop();
                }
            }
            Item::Struct(s) => {
                let name = s.ident.to_string();
                let id = structure_id(unit_id, inline_mods, &name);
                let line = s.ident.span().start().line as u32;
                let serialisable = has_serde_derive(&s.attrs);
                let fields = extract_struct_fields(&s.fields);
                let mut rel_path = inline_mods.clone();
                rel_path.push(name.clone());
                let idx = out.len();
                out.push(StructEntry {
                    structure: Structure {
                        id,
                        name,
                        kind: "struct".to_string(),
                        unit: unit_id.to_string(),
                        file: file_rel.to_string(),
                        line,
                        serialisable,
                        fields,
                        referenced_by: Vec::new(),
                    },
                    rel_path,
                });
                by_unit.entry(unit_id.to_string()).or_default().push(idx);
            }
            Item::Enum(e) => {
                let name = e.ident.to_string();
                let id = structure_id(unit_id, inline_mods, &name);
                let line = e.ident.span().start().line as u32;
                let serialisable = has_serde_derive(&e.attrs);
                let fields = e
                    .variants
                    .iter()
                    .filter(|v| !is_cfg_test(&v.attrs))
                    .map(|v| Field {
                        name: v.ident.to_string(),
                        ty: format_variant_fields(&v.fields),
                    })
                    .collect();
                let mut rel_path = inline_mods.clone();
                rel_path.push(name.clone());
                let idx = out.len();
                out.push(StructEntry {
                    structure: Structure {
                        id,
                        name,
                        kind: "enum".to_string(),
                        unit: unit_id.to_string(),
                        file: file_rel.to_string(),
                        line,
                        serialisable,
                        fields,
                        referenced_by: Vec::new(),
                    },
                    rel_path,
                });
                by_unit.entry(unit_id.to_string()).or_default().push(idx);
            }
            _ => {}
        }
    }
}

fn structure_id(unit_id: &str, inline_mods: &[String], name: &str) -> String {
    if inline_mods.is_empty() {
        format!("{unit_id}::{name}")
    } else {
        format!("{unit_id}::{}::{name}", inline_mods.join("::"))
    }
}

fn has_serde_derive(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let Ok(paths) = attr.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
        else {
            continue;
        };
        for path in paths {
            if let Some(last) = path.segments.last() {
                if last.ident == "Serialize" || last.ident == "Deserialize" {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_struct_fields(fields: &syn::Fields) -> Vec<Field> {
    match fields {
        syn::Fields::Named(named) => named
            .named
            .iter()
            .filter(|f| !is_cfg_test(&f.attrs))
            .map(|f| Field {
                name: f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                ty: format_type(&f.ty),
            })
            .collect(),
        syn::Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .filter(|f| !is_cfg_test(&f.attrs))
            .enumerate()
            .map(|(i, f)| Field {
                name: i.to_string(),
                ty: format_type(&f.ty),
            })
            .collect(),
        syn::Fields::Unit => Vec::new(),
    }
}

fn format_variant_fields(fields: &syn::Fields) -> String {
    match fields {
        syn::Fields::Unit => String::new(),
        syn::Fields::Unnamed(u) => {
            let parts: Vec<String> = u.unnamed.iter().map(|f| format_type(&f.ty)).collect();
            format!("({})", parts.join(", "))
        }
        syn::Fields::Named(n) => {
            let parts: Vec<String> = n
                .named
                .iter()
                .map(|f| {
                    let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
                    format!("{name}: {}", format_type(&f.ty))
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn format_type(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => {
            if let Some(qself) = &tp.qself {
                let self_ty = format_type(&qself.ty);
                if qself.position == 0 {
                    format!("<{self_ty}>::{}", format_path(&tp.path))
                } else {
                    let segs: Vec<String> = tp.path.segments.iter().map(format_segment).collect();
                    let prefix = if tp.path.leading_colon.is_some() {
                        "::"
                    } else {
                        ""
                    };
                    let trait_part = segs[..qself.position].join("::");
                    let rest = segs[qself.position..].join("::");
                    format!("<{self_ty} as {prefix}{trait_part}>::{rest}")
                }
            } else {
                format_path(&tp.path)
            }
        }
        Type::Reference(tr) => {
            let mut s = String::from("&");
            if let Some(lt) = &tr.lifetime {
                s.push('\'');
                s.push_str(&lt.ident.to_string());
                s.push(' ');
            }
            if tr.mutability.is_some() {
                s.push_str("mut ");
            }
            s.push_str(&format_type(&tr.elem));
            s
        }
        Type::Slice(ts) => format!("[{}]", format_type(&ts.elem)),
        Type::Array(ta) => format!("[{}; {}]", format_type(&ta.elem), format_tokens(&ta.len)),
        Type::Tuple(tt) => {
            if tt.elems.is_empty() {
                "()".to_string()
            } else if tt.elems.len() == 1 {
                format!("({},)", format_type(&tt.elems[0]))
            } else {
                let parts: Vec<String> = tt.elems.iter().map(format_type).collect();
                format!("({})", parts.join(", "))
            }
        }
        Type::Ptr(tp) => {
            let qual = if tp.const_token.is_some() {
                "const"
            } else {
                "mut"
            };
            format!("*{qual} {}", format_type(&tp.elem))
        }
        Type::BareFn(bf) => {
            let mut s = String::new();
            if let Some(lt) = &bf.lifetimes {
                s.push_str(&format_bound_lifetimes(lt));
                s.push(' ');
            }
            if bf.unsafety.is_some() {
                s.push_str("unsafe ");
            }
            if let Some(abi) = &bf.abi {
                s.push_str("extern ");
                if let Some(name) = &abi.name {
                    s.push_str(&format_tokens(name));
                    s.push(' ');
                }
            }
            s.push_str("fn(");
            let mut inputs: Vec<String> = bf
                .inputs
                .iter()
                .map(|arg| {
                    if let Some((name, _)) = &arg.name {
                        format!("{name}: {}", format_type(&arg.ty))
                    } else {
                        format_type(&arg.ty)
                    }
                })
                .collect();
            if bf.variadic.is_some() {
                inputs.push("...".to_string());
            }
            s.push_str(&inputs.join(", "));
            s.push(')');
            if let ReturnType::Type(_, ret) = &bf.output {
                s.push_str(" -> ");
                s.push_str(&format_type(ret));
            }
            s
        }
        Type::Never(_) => "!".to_string(),
        Type::Infer(_) => "_".to_string(),
        Type::Paren(tp) => format!("({})", format_type(&tp.elem)),
        Type::Group(tg) => format_type(&tg.elem),
        Type::TraitObject(to) => {
            let prefix = if to.dyn_token.is_some() { "dyn " } else { "" };
            format!("{prefix}{}", format_bounds(&to.bounds))
        }
        Type::ImplTrait(it) => format!("impl {}", format_bounds(&it.bounds)),
        other => format_tokens(other),
    }
}

fn format_path(path: &syn::Path) -> String {
    let prefix = if path.leading_colon.is_some() {
        "::"
    } else {
        ""
    };
    let segs: Vec<String> = path.segments.iter().map(format_segment).collect();
    format!("{prefix}{}", segs.join("::"))
}

fn format_segment(seg: &syn::PathSegment) -> String {
    let mut s = seg.ident.to_string();
    match &seg.arguments {
        PathArguments::None => {}
        PathArguments::AngleBracketed(ab) => {
            let args: Vec<String> = ab.args.iter().map(format_generic_arg).collect();
            s.push('<');
            s.push_str(&args.join(", "));
            s.push('>');
        }
        PathArguments::Parenthesized(p) => {
            let inputs: Vec<String> = p.inputs.iter().map(format_type).collect();
            s.push('(');
            s.push_str(&inputs.join(", "));
            s.push(')');
            if let ReturnType::Type(_, ret) = &p.output {
                s.push_str(" -> ");
                s.push_str(&format_type(ret));
            }
        }
    }
    s
}

fn format_generic_arg(arg: &GenericArgument) -> String {
    match arg {
        GenericArgument::Lifetime(lt) => format!("'{}", lt.ident),
        GenericArgument::Type(ty) => format_type(ty),
        GenericArgument::Const(expr) => format_tokens(expr),
        GenericArgument::AssocType(at) => {
            let generics = at
                .generics
                .as_ref()
                .map(|g| {
                    let args: Vec<String> = g.args.iter().map(format_generic_arg).collect();
                    format!("<{}>", args.join(", "))
                })
                .unwrap_or_default();
            format!("{}{generics} = {}", at.ident, format_type(&at.ty))
        }
        GenericArgument::AssocConst(ac) => {
            let generics = ac
                .generics
                .as_ref()
                .map(|g| {
                    let args: Vec<String> = g.args.iter().map(format_generic_arg).collect();
                    format!("<{}>", args.join(", "))
                })
                .unwrap_or_default();
            format!("{}{generics} = {}", ac.ident, format_tokens(&ac.value))
        }
        GenericArgument::Constraint(c) => {
            let generics = c
                .generics
                .as_ref()
                .map(|g| {
                    let args: Vec<String> = g.args.iter().map(format_generic_arg).collect();
                    format!("<{}>", args.join(", "))
                })
                .unwrap_or_default();
            format!("{}{generics}: {}", c.ident, format_bounds(&c.bounds))
        }
        other => format_tokens(other),
    }
}

fn format_bounds(bounds: &Punctuated<TypeParamBound, Token![+]>) -> String {
    let parts: Vec<String> = bounds
        .iter()
        .map(|b| match b {
            TypeParamBound::Trait(tb) => {
                let mut s = String::new();
                if matches!(tb.modifier, syn::TraitBoundModifier::Maybe(_)) {
                    s.push('?');
                }
                if let Some(lt) = &tb.lifetimes {
                    s.push_str(&format_bound_lifetimes(lt));
                    s.push(' ');
                }
                s.push_str(&format_path(&tb.path));
                if tb.paren_token.is_some() {
                    format!("({s})")
                } else {
                    s
                }
            }
            TypeParamBound::Lifetime(lt) => format!("'{}", lt.ident),
            other => format_tokens(other),
        })
        .collect();
    parts.join(" + ")
}

fn format_bound_lifetimes(lt: &syn::BoundLifetimes) -> String {
    let params: Vec<String> = lt
        .lifetimes
        .iter()
        .map(|p| match p {
            syn::GenericParam::Lifetime(l) => format!("'{}", l.lifetime.ident),
            other => format_tokens(other),
        })
        .collect();
    format!("for<{}>", params.join(", "))
}

fn format_tokens<T: ToTokens>(t: &T) -> String {
    t.to_token_stream().to_string()
}

fn consume_nested_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in meta.input);
        let _: proc_macro2::TokenStream = content.parse()?;
    }
    Ok(())
}

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("cfg") {
            continue;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test")
                && !meta.input.peek(Token![=])
                && !meta.input.peek(syn::token::Paren)
            {
                found = true;
                return Ok(());
            }
            if meta.path.is_ident("all") && meta.input.peek(syn::token::Paren) {
                let _ = meta.parse_nested_meta(|inner| {
                    if inner.path.is_ident("test")
                        && !inner.input.peek(Token![=])
                        && !inner.input.peek(syn::token::Paren)
                    {
                        found = true;
                        return Ok(());
                    }
                    consume_nested_value(&inner)
                });
                return Ok(());
            }
            consume_nested_value(&meta)
        });
        if found {
            return true;
        }
    }
    false
}

fn extract_path_attr(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("path") {
            continue;
        }
        if let Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(expr_lit) = &nv.value {
                if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                    return Some(lit_str.value());
                }
            }
        }
    }
    None
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn impl_item_attrs(item: &ImplItem) -> &[Attribute] {
    match item {
        ImplItem::Const(i) => &i.attrs,
        ImplItem::Fn(i) => &i.attrs,
        ImplItem::Type(i) => &i.attrs,
        ImplItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(item: &TraitItem) -> &[Attribute] {
    match item {
        TraitItem::Const(i) => &i.attrs,
        TraitItem::Fn(i) => &i.attrs,
        TraitItem::Type(i) => &i.attrs,
        TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn normalize_rel_checked(path: &Path) -> Option<PathBuf> {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(s) => parts.push(s.to_os_string()),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    Some(out)
}

fn normalize_rel(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    out
}

fn rel_to_string(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}
