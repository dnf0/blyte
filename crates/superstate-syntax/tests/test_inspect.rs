use std::fs;
use std::path::Path;
use superstate_syntax::inspect::{
    detect_language, extract_symbol_source, extract_symbols_from_file, find_symbol, format_outline,
};
use superstate_syntax::symbols::{SupportedLanguage, SymbolInfo, SymbolKind, SyntaxError};
use tempfile::tempdir;

#[test]
fn test_detect_language_supported_and_unsupported() {
    assert_eq!(
        detect_language(Path::new("src/lib.rs")),
        Some(SupportedLanguage::Rust)
    );
    assert_eq!(
        detect_language(Path::new("src/MAIN.RS")),
        Some(SupportedLanguage::Rust)
    );
    assert_eq!(
        detect_language(Path::new("scripts/run.py")),
        Some(SupportedLanguage::Python)
    );
    assert_eq!(
        detect_language(Path::new("ext/native.pyx")),
        Some(SupportedLanguage::Python)
    );
    assert_eq!(
        detect_language(Path::new("frontend/index.ts")),
        Some(SupportedLanguage::TypeScript)
    );
    assert_eq!(
        detect_language(Path::new("frontend/app.tsx")),
        Some(SupportedLanguage::TypeScript)
    );
    assert_eq!(
        detect_language(Path::new("legacy/utils.js")),
        Some(SupportedLanguage::TypeScript)
    );
    assert_eq!(
        detect_language(Path::new("legacy/view.jsx")),
        Some(SupportedLanguage::TypeScript)
    );
    assert_eq!(
        detect_language(Path::new("backend/main.go")),
        Some(SupportedLanguage::Go)
    );

    // Unsupported
    assert_eq!(detect_language(Path::new("README.md")), None);
    assert_eq!(detect_language(Path::new("styles.css")), None);
    assert_eq!(detect_language(Path::new("Dockerfile")), None);
    assert_eq!(detect_language(Path::new("")), None);
}

#[test]
fn test_extract_symbols_from_file_success() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("service.rs");
    let content = r#"pub struct Service {
    name: String,
}

pub fn start() {
    println!("started");
}
"#;
    fs::write(&file_path, content).unwrap();

    let (lang, read_content, symbols) = extract_symbols_from_file(&file_path).unwrap();
    assert_eq!(lang, SupportedLanguage::Rust);
    assert_eq!(read_content, content);
    assert_eq!(symbols.len(), 2);

    let struct_sym = find_symbol(&symbols, "Service").expect("Service struct found");
    assert_eq!(struct_sym.kind, SymbolKind::Struct);

    let fn_sym = find_symbol(&symbols, "start").expect("start fn found");
    assert_eq!(fn_sym.kind, SymbolKind::Function);
}

#[test]
fn test_extract_symbols_from_file_not_found() {
    let non_existent = Path::new("/path/does/not/exist/test.rs");
    match extract_symbols_from_file(non_existent) {
        Err(SyntaxError::Io(err)) => {
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("Expected SyntaxError::Io(NotFound), got {:?}", other),
    }
}

#[test]
fn test_extract_symbols_from_file_unsupported_language() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("notes.txt");
    fs::write(&file_path, "Just some notes").unwrap();

    match extract_symbols_from_file(&file_path) {
        Err(SyntaxError::UnsupportedLanguage(msg)) => {
            assert!(msg.contains("notes.txt"));
        }
        other => panic!("Expected SyntaxError::UnsupportedLanguage, got {:?}", other),
    }
}

#[test]
fn test_extract_symbol_source_slicing() {
    let code = "pub struct Worker {\n    id: u64,\n}\n\npub fn execute() {\n    Worker {\n        id: 1,\n    };\n}\n";
    let symbols =
        superstate_syntax::symbols::extract_symbols(code, SupportedLanguage::Rust).unwrap();

    let worker_sym = find_symbol(&symbols, "Worker").unwrap();
    let worker_src = extract_symbol_source(code, worker_sym);
    assert_eq!(worker_src, "pub struct Worker {\n    id: u64,\n}");

    let exec_sym = find_symbol(&symbols, "execute").unwrap();
    let exec_src = extract_symbol_source(code, exec_sym);
    assert_eq!(
        exec_src,
        "pub fn execute() {\n    Worker {\n        id: 1,\n    };\n}"
    );
}

#[test]
fn test_extract_symbol_source_boundary_clamping() {
    let code = "line 1\nline 2\nline 3";

    // Symbol with out-of-bounds start line
    let sym_out_of_bounds = SymbolInfo {
        name: "test".to_string(),
        kind: SymbolKind::Function,
        start_line: 10,
        end_line: 12,
        signature: "".to_string(),
    };
    assert_eq!(extract_symbol_source(code, &sym_out_of_bounds), "");

    // Symbol with clamped end line
    let sym_clamped = SymbolInfo {
        name: "test".to_string(),
        kind: SymbolKind::Function,
        start_line: 2,
        end_line: 99,
        signature: "".to_string(),
    };
    assert_eq!(extract_symbol_source(code, &sym_clamped), "line 2\nline 3");

    // Symbol with start_line 0
    let sym_zero = SymbolInfo {
        name: "test".to_string(),
        kind: SymbolKind::Function,
        start_line: 0,
        end_line: 1,
        signature: "".to_string(),
    };
    assert_eq!(extract_symbol_source(code, &sym_zero), "line 1");

    // Empty source
    assert_eq!(extract_symbol_source("", &sym_zero), "");
}

#[test]
fn test_format_outline() {
    let symbols = vec![
        SymbolInfo {
            name: "StateEngine".to_string(),
            kind: SymbolKind::Class,
            start_line: 1,
            end_line: 15,
            signature: "class StateEngine:".to_string(),
        },
        SymbolInfo {
            name: "run_step".to_string(),
            kind: SymbolKind::Function,
            start_line: 17,
            end_line: 25,
            signature: "def run_step():".to_string(),
        },
    ];

    let outline = format_outline(&symbols);
    let expected = "- [Class] StateEngine [lines 1-15]\n- [Function] run_step [lines 17-25]";
    assert_eq!(outline, expected);

    // Empty symbols list
    assert_eq!(format_outline(&[]), "");
}

#[test]
fn test_find_symbol_case_insensitive() {
    let symbols = vec![
        SymbolInfo {
            name: "ProcessData".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 5,
            signature: "".to_string(),
        },
        SymbolInfo {
            name: "Config".to_string(),
            kind: SymbolKind::Struct,
            start_line: 7,
            end_line: 10,
            signature: "".to_string(),
        },
    ];

    assert!(find_symbol(&symbols, "processdata").is_some());
    assert!(find_symbol(&symbols, "PROCESSDATA").is_some());
    assert!(find_symbol(&symbols, "ProcessData").is_some());
    assert_eq!(
        find_symbol(&symbols, "config").unwrap().kind,
        SymbolKind::Struct
    );
    assert!(find_symbol(&symbols, "unknown").is_none());
}
