use superstate_syntax::symbols::{extract_symbols, SupportedLanguage, SymbolKind};

#[test]
fn test_extract_rust_symbols() {
    let code = r#"
pub struct StateManager {
    id: usize,
}

impl StateManager {
    pub fn new() -> Self {
        Self { id: 0 }
    }
}
"#;
    let symbols = extract_symbols(code, SupportedLanguage::Rust).unwrap();
    assert!(symbols
        .iter()
        .any(|s| s.name == "StateManager" && s.kind == SymbolKind::Struct));
    assert!(symbols
        .iter()
        .any(|s| s.name == "new" && s.kind == SymbolKind::Function));
}

#[test]
fn test_extract_python_symbols() {
    let code = r#"
class StateEngine:
    def __init__(self):
        self.state = None

def run_pipeline():
    pass
"#;
    let symbols = extract_symbols(code, SupportedLanguage::Python).unwrap();
    assert!(symbols
        .iter()
        .any(|s| s.name == "StateEngine" && s.kind == SymbolKind::Class));
    assert!(symbols
        .iter()
        .any(|s| s.name == "run_pipeline" && s.kind == SymbolKind::Function));
}

#[test]
fn test_extract_typescript_symbols() {
    let code = r#"
export interface Config {
    apiKey: string;
}

export function initialize(): void {
    console.log("ready");
}
"#;
    let symbols = extract_symbols(code, SupportedLanguage::TypeScript).unwrap();
    assert!(symbols
        .iter()
        .any(|s| s.name == "Config" && s.kind == SymbolKind::Interface));
    assert!(symbols
        .iter()
        .any(|s| s.name == "initialize" && s.kind == SymbolKind::Function));
}

#[test]
fn test_extract_go_symbols() {
    let code = r#"
package main

type Runner struct {
    Name string
}

func Execute() error {
    return nil
}
"#;
    let symbols = extract_symbols(code, SupportedLanguage::Go).unwrap();
    assert!(symbols
        .iter()
        .any(|s| s.name == "Runner" && s.kind == SymbolKind::Struct));
    assert!(symbols
        .iter()
        .any(|s| s.name == "Execute" && s.kind == SymbolKind::Function));
}
