use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;

use crate::map::{Edge, Field, Group, Part, Structure, Unit, Unparsed, Unresolved};

const EXCLUDED_DIRS: &[&str] = &[
    "tests",
    "test",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
];

const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class",
    "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return",
    "try", "while", "with", "yield",
];

struct PyUnitInfo {
    id: String,
    group_id: String,
    file_rel: String,
    package_segs: Vec<String>,
    is_init: bool,
    abs_path: PathBuf,
}

struct LogicalLine {
    line: u32,
    indent: usize,
    text: String,
}

enum FromItem {
    Wildcard,
    Name { orig: String, local: String },
}

enum ParsedImport {
    Import {
        line: u32,
        module: String,
        alias: String,
    },
    From {
        line: u32,
        dots: usize,
        module: String,
        items: Vec<FromItem>,
    },
}

struct UnitScan {
    unit_id: String,
    stripped: String,
    direct_from_names: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
}

pub fn extract(root: &Path) -> Result<Part, String> {
    let py_files = discover_python_files(root);
    if py_files.is_empty() {
        return Ok(Part::default());
    }

    let mut unit_infos: Vec<PyUnitInfo> = Vec::with_capacity(py_files.len());
    let mut units: Vec<Unit> = Vec::with_capacity(py_files.len());
    let mut groups_map: HashMap<(String, String), Group> = HashMap::new();
    let mut unit_set: HashSet<String> = HashSet::new();
    let mut unit_to_file: HashMap<String, String> = HashMap::new();
    let mut unit_to_group: HashMap<String, String> = HashMap::new();
    let mut init_units: HashSet<String> = HashSet::new();
    let mut top_level_names: HashSet<String> = HashSet::new();

    for rel in py_files {
        let Some(info) = build_unit_info(root, &rel) else {
            continue;
        };

        if let Some(existing) = unit_to_file.insert(info.id.clone(), info.file_rel.clone()) {
            let (first, second) = if existing <= info.file_rel {
                (existing, info.file_rel)
            } else {
                (info.file_rel, existing)
            };
            return Err(format!(
                "two Python files have the module name {}: {} and {}",
                info.id, first, second
            ));
        }

        let group_path = dir_rel_to_string(rel.parent().unwrap_or_else(|| Path::new("")));
        groups_map
            .entry((info.group_id.clone(), group_path.clone()))
            .or_insert_with(|| Group {
                id: info.group_id.clone(),
                kind: "package".to_string(),
                path: group_path,
                lang: "python".to_string(),
            });

        if let Some(first) = info.id.split('.').next() {
            if !first.is_empty() {
                top_level_names.insert(first.to_string());
            }
        }
        if info.is_init {
            init_units.insert(info.id.clone());
        }
        unit_set.insert(info.id.clone());
        unit_to_group.insert(info.id.clone(), info.group_id.clone());

        units.push(Unit {
            id: info.id.clone(),
            group: info.group_id.clone(),
            file: info.file_rel.clone(),
            lang: "python".to_string(),
        });
        unit_infos.push(info);
    }

    let mut edges: Vec<Edge> = Vec::new();
    let mut structures: Vec<Structure> = Vec::new();
    let mut unparsed: Vec<Unparsed> = Vec::new();
    let mut unresolved: Vec<Unresolved> = Vec::new();
    let mut scans: Vec<UnitScan> = Vec::with_capacity(unit_infos.len());
    let mut init_reexports: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();

    for info in &unit_infos {
        let bytes = std::fs::read(&info.abs_path)
            .map_err(|e| format!("cannot read {}: {e}", info.abs_path.display()))?;
        let src = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                unparsed.push(Unparsed {
                    file: info.file_rel.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };

        let stripped = blank_strings_and_comments(&src);
        let logical_lines = join_logical_lines(&stripped);

        let file_structures = extract_structures(&logical_lines, &info.id, &info.file_rel);
        structures.extend(file_structures);

        let parsed_imports = parse_imports(&logical_lines);
        let mut direct_from_names: Vec<(String, String)> = Vec::new();
        let mut module_aliases: Vec<(String, String)> = Vec::new();

        for imp in parsed_imports {
            match imp {
                ParsedImport::Import {
                    line,
                    module,
                    alias,
                } => {
                    if let Some(target_unit) =
                        resolve_plain_import(&module, &info.package_segs, &unit_set)
                    {
                        emit_edges(&info.id, &target_unit, &unit_to_group, &mut edges);
                        module_aliases.push((target_unit, alias));
                    } else if first_segment_in(&module, &top_level_names) {
                        unresolved.push(Unresolved {
                            file: info.file_rel.clone(),
                            line,
                            reference: module,
                        });
                    }
                }
                ParsedImport::From {
                    line,
                    dots,
                    module,
                    items,
                } => {
                    resolve_from_import(
                        info,
                        line,
                        dots,
                        &module,
                        &items,
                        &unit_set,
                        &top_level_names,
                        &unit_to_group,
                        &mut edges,
                        &mut unresolved,
                        &mut direct_from_names,
                        &mut module_aliases,
                        &mut init_reexports,
                    );
                }
            }
        }

        scans.push(UnitScan {
            unit_id: info.id.clone(),
            stripped,
            direct_from_names,
            module_aliases,
        });
    }

    let mut struct_by_unit_name: HashMap<(String, String), usize> = HashMap::new();
    let mut structs_by_unit: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, s) in structures.iter().enumerate() {
        struct_by_unit_name.insert((s.unit.clone(), s.name.clone()), idx);
        structs_by_unit.entry(s.unit.clone()).or_default().push(idx);
    }

    for scan in &scans {
        for (target_unit, orig_name) in &scan.direct_from_names {
            if let Some(&idx) = struct_by_unit_name.get(&(target_unit.clone(), orig_name.clone())) {
                if structures[idx].unit != scan.unit_id {
                    structures[idx].referenced_by.push(scan.unit_id.clone());
                }
            }
            if init_units.contains(target_unit) {
                if let Some(targets) = init_reexports.get(&(target_unit.clone(), orig_name.clone()))
                {
                    for (real_unit, real_name) in targets {
                        if let Some(&idx) =
                            struct_by_unit_name.get(&(real_unit.clone(), real_name.clone()))
                        {
                            if structures[idx].unit != scan.unit_id {
                                structures[idx].referenced_by.push(scan.unit_id.clone());
                            }
                        }
                    }
                }
            }
        }

        for (target_unit, alias_name) in &scan.module_aliases {
            if let Some(indices) = structs_by_unit.get(target_unit) {
                for &idx in indices {
                    if structures[idx].unit != scan.unit_id
                        && code_contains_attr_ref(&scan.stripped, alias_name, &structures[idx].name)
                    {
                        structures[idx].referenced_by.push(scan.unit_id.clone());
                    }
                }
            }
        }
    }

    let groups: Vec<Group> = groups_map.into_values().collect();

    Ok(Part {
        groups,
        units,
        edges,
        structures,
        unparsed,
        unresolved,
    })
}

fn discover_python_files(root: &Path) -> Vec<PathBuf> {
    let mut walk = WalkBuilder::new(root);
    walk.require_git(false);
    walk.parents(false);
    walk.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            if let Some(name) = entry.file_name().to_str() {
                if EXCLUDED_DIRS.contains(&name) {
                    return false;
                }
            }
        }
        true
    });
    walk.sort_by_file_path(|a, b| a.cmp(b));

    let mut files = Vec::new();
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension() != Some(OsStr::new("py")) {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_excluded_py_file(file_name) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if has_excluded_dir_component(rel) {
            continue;
        }
        files.push(rel.to_path_buf());
    }
    files
}

fn is_excluded_py_file(name: &str) -> bool {
    name == "conftest.py"
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_test.py")
}

fn has_excluded_dir_component(rel: &Path) -> bool {
    let Some(parent) = rel.parent() else {
        return false;
    };
    for comp in parent.components() {
        if let Component::Normal(s) = comp {
            if let Some(name) = s.to_str() {
                if EXCLUDED_DIRS.contains(&name) {
                    return true;
                }
            }
        }
    }
    false
}

fn build_unit_info(root: &Path, rel: &Path) -> Option<PyUnitInfo> {
    let stem = rel.file_stem()?.to_str()?.to_string();
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    let all_dir_segs: Vec<String> = parent
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();

    let dir_segs: Vec<String> = match all_dir_segs.iter().rposition(|s| s == "src") {
        Some(idx) => all_dir_segs[idx + 1..].to_vec(),
        None => all_dir_segs.clone(),
    };

    let group_id = if !dir_segs.is_empty() {
        dir_segs.join(".")
    } else if !all_dir_segs.is_empty() {
        all_dir_segs.join(".")
    } else {
        ".".to_string()
    };

    let is_init = stem == "__init__";
    let id = if is_init {
        if dir_segs.is_empty() {
            stem
        } else {
            dir_segs.join(".")
        }
    } else if dir_segs.is_empty() {
        stem
    } else {
        format!("{}.{stem}", dir_segs.join("."))
    };

    let file_rel = path_rel_to_string(rel);

    Some(PyUnitInfo {
        id,
        group_id,
        file_rel,
        package_segs: dir_segs,
        is_init,
        abs_path: root.join(rel),
    })
}

fn path_rel_to_string(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.join("/")
}

fn dir_rel_to_string(rel: &Path) -> String {
    let s = path_rel_to_string(rel);
    if s.is_empty() {
        ".".to_string()
    } else {
        s
    }
}

fn blank_strings_and_comments(src: &str) -> String {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let chars: Vec<char> = normalized.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(normalized.len());
    let mut i = 0;

    while i < n {
        let c = chars[i];
        if c == '#' {
            while i < n && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }

        if let Some((prefix_len, quote)) = string_start(&chars, i) {
            for _ in 0..prefix_len {
                out.push(' ');
            }
            i += prefix_len;

            let is_triple = i + 2 < n && chars[i + 1] == quote && chars[i + 2] == quote;
            if is_triple {
                out.push_str("   ");
                i += 3;
                while i < n {
                    if chars[i] == '\\' {
                        out.push(' ');
                        i += 1;
                        if i < n {
                            if chars[i] == '\n' {
                                out.push('\n');
                            } else {
                                out.push(' ');
                            }
                            i += 1;
                        }
                    } else if i + 2 < n
                        && chars[i] == quote
                        && chars[i + 1] == quote
                        && chars[i + 2] == quote
                    {
                        out.push_str("   ");
                        i += 3;
                        break;
                    } else if chars[i] == '\n' {
                        out.push('\n');
                        i += 1;
                    } else {
                        out.push(' ');
                        i += 1;
                    }
                }
            } else {
                out.push(quote);
                i += 1;
                let mut multiline = false;
                while i < n {
                    if chars[i] == '\\' {
                        out.push(' ');
                        i += 1;
                        if i < n {
                            if chars[i] == '\n' {
                                out.push('\n');
                                multiline = true;
                            } else {
                                out.push(' ');
                            }
                            i += 1;
                        }
                    } else if chars[i] == quote {
                        if multiline {
                            out.push(' ');
                        } else {
                            out.push(quote);
                        }
                        i += 1;
                        break;
                    } else if chars[i] == '\n' {
                        out.push('\n');
                        i += 1;
                        break;
                    } else {
                        out.push(' ');
                        i += 1;
                    }
                }
            }
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}

fn string_start(chars: &[char], i: usize) -> Option<(usize, char)> {
    let n = chars.len();
    let c = chars[i];
    if c == '"' || c == '\'' {
        return Some((0, c));
    }
    if i > 0 && is_ident_continue(chars[i - 1]) {
        return None;
    }
    if i + 2 < n
        && (chars[i + 2] == '"' || chars[i + 2] == '\'')
        && is_two_char_string_prefix(c, chars[i + 1])
    {
        return Some((2, chars[i + 2]));
    }
    if i + 1 < n && (chars[i + 1] == '"' || chars[i + 1] == '\'') && is_one_char_string_prefix(c) {
        return Some((1, chars[i + 1]));
    }
    None
}

fn is_one_char_string_prefix(c: char) -> bool {
    matches!(c, 'r' | 'R' | 'u' | 'U' | 'b' | 'B' | 'f' | 'F')
}

fn is_two_char_string_prefix(a: char, b: char) -> bool {
    matches!(
        (a.to_ascii_lowercase(), b.to_ascii_lowercase()),
        ('r', 'b') | ('b', 'r') | ('r', 'f') | ('f', 'r')
    )
}

fn measure_indent(line: &str) -> usize {
    let mut width = 0;
    for ch in line.chars() {
        match ch {
            ' ' => width += 1,
            '\t' => width += 4,
            _ => break,
        }
    }
    width
}

fn join_logical_lines(stripped: &str) -> Vec<LogicalLine> {
    let mut result = Vec::new();
    let mut cur_line: u32 = 0;
    let mut cur_indent: usize = 0;
    let mut cur_text = String::new();
    let mut depth: usize = 0;
    let mut continuing = false;

    for (idx, raw_line) in stripped.split('\n').enumerate() {
        let line_no = (idx + 1) as u32;
        if !continuing {
            if raw_line.trim().is_empty() {
                continue;
            }
            cur_line = line_no;
            cur_indent = measure_indent(raw_line);
            cur_text.clear();
            depth = 0;
        }

        let trimmed_end = raw_line.trim_end();
        let has_backslash = trimmed_end.ends_with('\\');
        let piece = if has_backslash {
            trimmed_end[..trimmed_end.len() - 1].trim()
        } else {
            raw_line.trim()
        };

        if !piece.is_empty() {
            if !cur_text.is_empty() {
                cur_text.push(' ');
            }
            cur_text.push_str(piece);
            for ch in piece.chars() {
                match ch {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }

        if depth > 0 || has_backslash {
            continuing = true;
        } else {
            continuing = false;
            if !cur_text.is_empty() {
                result.push(LogicalLine {
                    line: cur_line,
                    indent: cur_indent,
                    text: std::mem::take(&mut cur_text),
                });
            }
        }
    }

    if continuing && !cur_text.is_empty() {
        result.push(LogicalLine {
            line: cur_line,
            indent: cur_indent,
            text: cur_text,
        });
    }

    result
}

fn split_top_level(s: &str, delim: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            c if c == delim && depth == 0 => {
                parts.push(&s[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn find_top_level_char(s: &str, target: char) -> Option<usize> {
    let mut depth: usize = 0;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            c if c == target && depth == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

fn parse_imports(lines: &[LogicalLine]) -> Vec<ParsedImport> {
    let mut out = Vec::new();
    for logical in lines {
        for part in split_top_level(&logical.text, ';') {
            let stmt = part.trim();
            if stmt.is_empty() {
                continue;
            }
            if stmt.starts_with("import") {
                out.extend(parse_plain_import_stmt(stmt, logical.line));
            } else if stmt.starts_with("from") {
                if let Some(parsed) = parse_from_import_stmt(stmt, logical.line) {
                    out.push(parsed);
                }
            }
        }
    }
    out
}

fn parse_plain_import_stmt(stmt: &str, line: u32) -> Vec<ParsedImport> {
    let Some(rest) = stmt.strip_prefix("import") else {
        return Vec::new();
    };
    if !rest.chars().next().is_some_and(|c| c.is_ascii_whitespace()) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in rest.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = item.split_whitespace().collect();
        match tokens.as_slice() {
            [module] if is_valid_dotted_name(module) => {
                out.push(ParsedImport::Import {
                    line,
                    module: (*module).to_string(),
                    alias: (*module).to_string(),
                });
            }
            [module, "as", alias] if is_valid_dotted_name(module) && is_valid_ident(alias) => {
                out.push(ParsedImport::Import {
                    line,
                    module: (*module).to_string(),
                    alias: (*alias).to_string(),
                });
            }
            _ => {}
        }
    }
    out
}

fn parse_from_import_stmt(stmt: &str, line: u32) -> Option<ParsedImport> {
    let rest = stmt.strip_prefix("from")?;
    let first = rest.chars().next()?;
    if !first.is_ascii_whitespace() && first != '.' {
        return None;
    }
    let rest = rest.trim_start();
    let dots = rest.bytes().take_while(|&b| b == b'.').count();
    let after_dots = &rest[dots..];

    let (module, after_import) = if dots > 0 {
        if after_dots.is_empty() {
            return None;
        }
        if after_dots
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_whitespace())
        {
            (
                String::new(),
                strip_import_keyword(after_dots.trim_start())?,
            )
        } else if let Some(after_kw) = strip_import_keyword(after_dots) {
            (String::new(), after_kw)
        } else {
            let mod_end = after_dots
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(after_dots.len());
            let mod_part = &after_dots[..mod_end];
            if !is_valid_dotted_name(mod_part) {
                return None;
            }
            let remainder = after_dots[mod_end..].trim_start();
            (mod_part.to_string(), strip_import_keyword(remainder)?)
        }
    } else {
        let mod_end = after_dots
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(after_dots.len());
        let mod_part = &after_dots[..mod_end];
        if mod_part == "import" || !is_valid_dotted_name(mod_part) {
            return None;
        }
        let remainder = after_dots[mod_end..].trim_start();
        (mod_part.to_string(), strip_import_keyword(remainder)?)
    };

    let mut list_text = after_import.trim();
    if let Some(inner) = list_text
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
    {
        list_text = inner.trim();
    }

    let mut items = Vec::new();
    for piece in list_text.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if piece == "*" {
            items.push(FromItem::Wildcard);
            continue;
        }
        let tokens: Vec<&str> = piece.split_whitespace().collect();
        match tokens.as_slice() {
            [name] if is_valid_ident(name) => {
                items.push(FromItem::Name {
                    orig: (*name).to_string(),
                    local: (*name).to_string(),
                });
            }
            [name, "as", alias] if is_valid_ident(name) && is_valid_ident(alias) => {
                items.push(FromItem::Name {
                    orig: (*name).to_string(),
                    local: (*alias).to_string(),
                });
            }
            _ => {}
        }
    }

    if items.is_empty() {
        return None;
    }

    Some(ParsedImport::From {
        line,
        dots,
        module,
        items,
    })
}

fn strip_import_keyword(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("import")?;
    let next = rest.chars().next()?;
    if next.is_ascii_whitespace() || next == '(' || next == '*' {
        Some(rest.trim())
    } else {
        None
    }
}

fn resolve_plain_import(
    module: &str,
    package_segs: &[String],
    unit_set: &HashSet<String>,
) -> Option<String> {
    let segs: Vec<&str> = module.split('.').collect();
    if let Some(found) = longest_prefix_match(&segs, 1, unit_set) {
        return Some(found);
    }
    if !package_segs.is_empty() {
        let mut combined: Vec<&str> = package_segs.iter().map(|s| s.as_str()).collect();
        combined.extend(segs);
        if let Some(found) = longest_prefix_match(&combined, package_segs.len() + 1, unit_set) {
            return Some(found);
        }
    }
    None
}

fn longest_prefix_match(
    segs: &[&str],
    min_len: usize,
    unit_set: &HashSet<String>,
) -> Option<String> {
    if segs.len() < min_len {
        return None;
    }
    for len in (min_len..=segs.len()).rev() {
        let candidate = segs[..len].join(".");
        if unit_set.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_from_import(
    info: &PyUnitInfo,
    line: u32,
    dots: usize,
    module: &str,
    items: &[FromItem],
    unit_set: &HashSet<String>,
    top_level_names: &HashSet<String>,
    unit_to_group: &HashMap<String, String>,
    edges: &mut Vec<Edge>,
    unresolved: &mut Vec<Unresolved>,
    direct_from_names: &mut Vec<(String, String)>,
    module_aliases: &mut Vec<(String, String)>,
    init_reexports: &mut HashMap<(String, String), Vec<(String, String)>>,
) {
    let written = format!("{}{}", ".".repeat(dots), module);

    if dots > 0 {
        let up = dots - 1;
        if up > info.package_segs.len() {
            unresolved.push(Unresolved {
                file: info.file_rel.clone(),
                line,
                reference: written,
            });
            return;
        }
        let mut base_segs: Vec<String> = info.package_segs[..info.package_segs.len() - up].to_vec();
        if !module.is_empty() {
            base_segs.extend(module.split('.').map(|s| s.to_string()));
        }
        let base_mod = base_segs.join(".");
        let mut any_unresolved = false;

        for item in items {
            match item {
                FromItem::Wildcard => {
                    if !base_mod.is_empty() && unit_set.contains(&base_mod) {
                        emit_edges(&info.id, &base_mod, unit_to_group, edges);
                    } else {
                        any_unresolved = true;
                    }
                }
                FromItem::Name { orig, local } => {
                    let sub_candidate = if base_mod.is_empty() {
                        orig.clone()
                    } else {
                        format!("{base_mod}.{orig}")
                    };
                    if unit_set.contains(&sub_candidate) {
                        emit_edges(&info.id, &sub_candidate, unit_to_group, edges);
                        module_aliases.push((sub_candidate, local.clone()));
                    } else if !base_mod.is_empty() && unit_set.contains(&base_mod) {
                        emit_edges(&info.id, &base_mod, unit_to_group, edges);
                        direct_from_names.push((base_mod.clone(), orig.clone()));
                        if info.is_init {
                            init_reexports
                                .entry((info.id.clone(), local.clone()))
                                .or_default()
                                .push((base_mod.clone(), orig.clone()));
                        }
                    } else {
                        any_unresolved = true;
                    }
                }
            }
        }

        if any_unresolved {
            unresolved.push(Unresolved {
                file: info.file_rel.clone(),
                line,
                reference: written,
            });
        }
        return;
    }

    let mut any_unresolved = false;
    for item in items {
        match item {
            FromItem::Wildcard => {
                if unit_set.contains(module) {
                    emit_edges(&info.id, module, unit_to_group, edges);
                } else if let Some(local_mod) = prefixed_unit(&info.package_segs, module, unit_set)
                {
                    emit_edges(&info.id, &local_mod, unit_to_group, edges);
                } else if first_segment_in(module, top_level_names) {
                    any_unresolved = true;
                }
            }
            FromItem::Name { orig, local } => {
                let root_sub = format!("{module}.{orig}");
                if unit_set.contains(&root_sub) {
                    emit_edges(&info.id, &root_sub, unit_to_group, edges);
                    module_aliases.push((root_sub, local.clone()));
                    continue;
                }
                if unit_set.contains(module) {
                    emit_edges(&info.id, module, unit_to_group, edges);
                    direct_from_names.push((module.to_string(), orig.clone()));
                    if info.is_init {
                        init_reexports
                            .entry((info.id.clone(), local.clone()))
                            .or_default()
                            .push((module.to_string(), orig.clone()));
                    }
                    continue;
                }
                if !info.package_segs.is_empty() {
                    let pkg_prefix = info.package_segs.join(".");
                    let local_sub = format!("{pkg_prefix}.{module}.{orig}");
                    if unit_set.contains(&local_sub) {
                        emit_edges(&info.id, &local_sub, unit_to_group, edges);
                        module_aliases.push((local_sub, local.clone()));
                        continue;
                    }
                    let local_mod = format!("{pkg_prefix}.{module}");
                    if unit_set.contains(&local_mod) {
                        emit_edges(&info.id, &local_mod, unit_to_group, edges);
                        direct_from_names.push((local_mod.clone(), orig.clone()));
                        if info.is_init {
                            init_reexports
                                .entry((info.id.clone(), local.clone()))
                                .or_default()
                                .push((local_mod, orig.clone()));
                        }
                        continue;
                    }
                }
                if first_segment_in(module, top_level_names) {
                    any_unresolved = true;
                }
            }
        }
    }

    if any_unresolved {
        unresolved.push(Unresolved {
            file: info.file_rel.clone(),
            line,
            reference: written,
        });
    }
}

fn prefixed_unit(
    package_segs: &[String],
    module: &str,
    unit_set: &HashSet<String>,
) -> Option<String> {
    if package_segs.is_empty() {
        return None;
    }
    let candidate = format!("{}.{module}", package_segs.join("."));
    unit_set.contains(&candidate).then_some(candidate)
}

fn first_segment_in(module: &str, top_level_names: &HashSet<String>) -> bool {
    module
        .split('.')
        .next()
        .is_some_and(|first| top_level_names.contains(first))
}

fn emit_edges(
    from_unit: &str,
    to_unit: &str,
    unit_to_group: &HashMap<String, String>,
    edges: &mut Vec<Edge>,
) {
    if from_unit == to_unit {
        return;
    }
    edges.push(Edge {
        from: from_unit.to_string(),
        to: to_unit.to_string(),
        level: "unit".to_string(),
    });
    if let (Some(from_group), Some(to_group)) =
        (unit_to_group.get(from_unit), unit_to_group.get(to_unit))
    {
        if from_group != to_group {
            edges.push(Edge {
                from: from_group.clone(),
                to: to_group.clone(),
                level: "group".to_string(),
            });
        }
    }
}

fn extract_structures(lines: &[LogicalLine], unit_id: &str, file_rel: &str) -> Vec<Structure> {
    let mut structures = Vec::new();
    let mut pending_decorators: Vec<&str> = Vec::new();

    for (idx, logical) in lines.iter().enumerate() {
        if logical.indent != 0 {
            continue;
        }
        let text = logical.text.as_str();
        if text.starts_with('@') {
            pending_decorators.push(text);
            continue;
        }

        if let Some((class_name, bases)) = parse_class_header(text) {
            let kind = classify_structure(&pending_decorators, &bases);
            pending_decorators.clear();

            if let Some(kind) = kind {
                let fields = extract_class_fields(&lines[idx + 1..]);
                structures.push(Structure {
                    id: format!("{unit_id}.{class_name}"),
                    name: class_name,
                    kind: kind.to_string(),
                    unit: unit_id.to_string(),
                    file: file_rel.to_string(),
                    line: logical.line,
                    serialisable: true,
                    fields,
                    referenced_by: Vec::new(),
                });
            }
        } else {
            pending_decorators.clear();
        }
    }

    structures
}

fn parse_class_header(text: &str) -> Option<(String, Vec<String>)> {
    let rest = text.strip_prefix("class")?;
    if !rest.chars().next().is_some_and(|c| c.is_ascii_whitespace()) {
        return None;
    }
    let rest = rest.trim_start();
    let colon_idx = find_top_level_char(rest, ':')?;
    let header = rest[..colon_idx].trim();

    if let Some(open_paren) = header.find('(') {
        let name = header[..open_paren].trim();
        if !is_valid_ident(name) {
            return None;
        }
        let close_paren = header.rfind(')')?;
        if close_paren <= open_paren {
            return None;
        }
        let bases_str = &header[open_paren + 1..close_paren];
        let bases = split_top_level(bases_str, ',')
            .into_iter()
            .filter_map(|b| parse_base_last_name(b.trim()))
            .collect();
        Some((name.to_string(), bases))
    } else {
        if !is_valid_ident(header) {
            return None;
        }
        Some((header.to_string(), Vec::new()))
    }
}

fn parse_base_last_name(base: &str) -> Option<String> {
    if base.is_empty() || find_top_level_char(base, '=').is_some() {
        return None;
    }
    let without_generic = base.split('[').next().unwrap_or(base).trim();
    let last = without_generic.split('.').next_back()?.trim();
    if is_valid_ident(last) {
        Some(last.to_string())
    } else {
        None
    }
}

fn classify_structure(decorators: &[&str], bases: &[String]) -> Option<&'static str> {
    if decorators.iter().any(|d| is_dataclass_decorator(d)) {
        return Some("dataclass");
    }
    if bases.iter().any(|b| b == "BaseModel") {
        return Some("model");
    }
    if bases.iter().any(|b| b == "TypedDict") {
        return Some("typeddict");
    }
    None
}

fn is_dataclass_decorator(decorator: &str) -> bool {
    let Some(rest) = decorator.strip_prefix('@') else {
        return false;
    };
    let rest = rest.trim();
    let name_part = match rest.find('(') {
        Some(pos) => {
            if !rest.ends_with(')') {
                return false;
            }
            rest[..pos].trim()
        }
        None => rest,
    };
    name_part == "dataclass" || name_part == "dataclasses.dataclass"
}

fn extract_class_fields(body_lines: &[LogicalLine]) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut body_indent: Option<usize> = None;

    for line in body_lines {
        if line.indent == 0 {
            break;
        }
        let target_indent = *body_indent.get_or_insert(line.indent);
        if line.indent != target_indent {
            continue;
        }
        if let Some(field) = parse_field_line(&line.text) {
            fields.push(field);
        }
    }

    fields
}

fn parse_field_line(text: &str) -> Option<Field> {
    let colon_idx = text.find(':')?;
    let name = text[..colon_idx].trim();
    if !is_valid_ident(name) || PYTHON_KEYWORDS.contains(&name) {
        return None;
    }
    let rhs = &text[colon_idx + 1..];
    if rhs.starts_with('=') {
        return None;
    }
    let type_part = match find_top_level_char(rhs, '=') {
        Some(eq_idx) => rhs[..eq_idx].trim(),
        None => rhs.trim(),
    };
    if type_part.is_empty() || is_classvar_type(type_part) {
        return None;
    }
    Some(Field {
        name: name.to_string(),
        ty: type_part.to_string(),
    })
}

fn is_classvar_type(ty: &str) -> bool {
    let base = ty.split('[').next().unwrap_or(ty).trim();
    let last = base.split('.').next_back().unwrap_or("").trim();
    last == "ClassVar"
}

fn code_contains_attr_ref(code: &str, alias: &str, struct_name: &str) -> bool {
    let needle = format!("{alias}.{struct_name}");
    let mut offset = 0;
    while let Some(pos) = code[offset..].find(&needle) {
        let start = offset + pos;
        let end = start + needle.len();
        let before_ok = if start == 0 {
            true
        } else {
            let prev = code[..start].chars().next_back().unwrap();
            !is_ident_continue(prev) && prev != '.'
        };
        let after_ok = if end == code.len() {
            true
        } else {
            let next = code[end..].chars().next().unwrap();
            !is_ident_continue(next)
        };
        if before_ok && after_ok {
            return true;
        }
        offset = start + 1;
        while offset < code.len() && !code.is_char_boundary(offset) {
            offset += 1;
        }
    }
    false
}

fn is_valid_dotted_name(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_valid_ident)
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_ident_start(first) && chars.all(is_ident_continue)
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}
