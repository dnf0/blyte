use serde::{Deserialize, Serialize};
use thiserror::Error;
use tree_sitter::{Language, Parser};

#[derive(Error, Debug)]
pub enum SyntaxError {
    #[error("Tree-sitter language load error: {0}")]
    LanguageError(String),
    #[error("Failed to parse source code")]
    ParseFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportedLanguage {
    Rust,
    Python,
    TypeScript,
    Go,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Struct,
    Class,
    Interface,
    Enum,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
}

pub fn get_language(lang: SupportedLanguage) -> Language {
    match lang {
        SupportedLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
        SupportedLanguage::Python => tree_sitter_python::LANGUAGE.into(),
        SupportedLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        SupportedLanguage::Go => tree_sitter_go::LANGUAGE.into(),
    }
}

pub fn extract_symbols(
    source: &str,
    lang: SupportedLanguage,
) -> Result<Vec<SymbolInfo>, SyntaxError> {
    let mut parser = Parser::new();
    let language = get_language(lang);
    parser
        .set_language(&language)
        .map_err(|e| SyntaxError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or(SyntaxError::ParseFailure)?;
    let root_node = tree.root_node();
    let mut symbols = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    fn traverse(
        node: tree_sitter::Node,
        source: &str,
        lines: &[&str],
        lang: SupportedLanguage,
        symbols: &mut Vec<SymbolInfo>,
    ) {
        let kind_str = node.kind();
        let mut symbol: Option<(String, SymbolKind)> = None;

        match lang {
            SupportedLanguage::Rust => match kind_str {
                "function_item" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Function));
                    }
                }
                "struct_item" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Struct));
                    }
                }
                "enum_item" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Enum));
                    }
                }
                _ => {}
            },
            SupportedLanguage::Python => match kind_str {
                "function_definition" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Function));
                    }
                }
                "class_definition" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Class));
                    }
                }
                _ => {}
            },
            SupportedLanguage::TypeScript => match kind_str {
                "function_declaration" | "method_definition" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Function));
                    }
                }
                "interface_declaration" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Interface));
                    }
                }
                "class_declaration" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Class));
                    }
                }
                _ => {}
            },
            SupportedLanguage::Go => match kind_str {
                "function_declaration" | "method_declaration" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        symbol = Some((name.to_string(), SymbolKind::Function));
                    }
                }
                "type_spec" => {
                    if let Some(name_node) = node.child_by_field_name("name") {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        if let Some(type_node) = node.child_by_field_name("type") {
                            if type_node.kind() == "struct_type" {
                                symbol = Some((name.to_string(), SymbolKind::Struct));
                            } else if type_node.kind() == "interface_type" {
                                symbol = Some((name.to_string(), SymbolKind::Interface));
                            }
                        }
                    }
                }
                _ => {}
            },
        }

        if let Some((name, kind)) = symbol {
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            let first_line = lines
                .get(node.start_position().row)
                .unwrap_or(&"")
                .trim()
                .to_string();
            symbols.push(SymbolInfo {
                name,
                kind,
                start_line,
                end_line,
                signature: first_line,
            });
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            traverse(child, source, lines, lang, symbols);
        }
    }

    traverse(root_node, source, &lines, lang, &mut symbols);
    Ok(symbols)
}
