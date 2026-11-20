use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::{DashboardArgs, MockArgs};
use crate::dashboard::Sources;

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

fn json_error(message: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "error": message })).unwrap_or_default()
}

fn pick_root<'a>(roots: &'a [PathBuf], name: &str) -> Option<&'a Path> {
    let mut candidates: Vec<&'a Path> = roots
        .iter()
        .map(PathBuf::as_path)
        .filter(|root| crate::dashboard::repo_name(root) == name)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let git_dirs: Vec<&'a Path> = candidates
        .iter()
        .copied()
        .filter(|root| root.join(".git").is_dir())
        .collect();
    if git_dirs.len() == 1 {
        return Some(git_dirs[0]);
    }
    candidates.sort();
    Some(candidates[0])
}

pub fn route(path: &str, src: &Sources, now: u64, days: u64) -> (u16, &'static str, Vec<u8>) {
    let (clean_path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    if let Some(asset) = crate::dashboard::shared_asset(clean_path) {
        return asset;
    }
    match clean_path {
        "/" | "/index.html" => {
            let html = include_str!("../assets/mock/index.html");
            (200, "text/html; charset=utf-8", html.as_bytes().to_vec())
        }
        "/api/replay" => {
            let data = crate::replay::collect(src, now, days);
            match serde_json::to_vec(&data) {
                Ok(body) => (200, "application/json", body),
                Err(e) => (
                    500,
                    "text/plain; charset=utf-8",
                    format!("failed to serialise replay data: {e}\n").into_bytes(),
                ),
            }
        }
        "/api/map" => {
            let name = query
                .and_then(|q| query_param(q, "repo"))
                .unwrap_or_default();
            if name.is_empty() {
                return (
                    400,
                    "application/json",
                    json_error("missing query parameter: repo"),
                );
            }
            let Some(root) = pick_root(&src.roots, &name) else {
                return (
                    404,
                    "application/json",
                    json_error(&format!(
                        "no repository named {name} under the mock's roots"
                    )),
                );
            };
            match crate::map::build(root, &name) {
                Ok(data) => match serde_json::to_vec(&data) {
                    Ok(body) => (200, "application/json", body),
                    Err(e) => (
                        500,
                        "text/plain; charset=utf-8",
                        format!("failed to serialise map data: {e}\n").into_bytes(),
                    ),
                },
                Err(e) => (422, "application/json", json_error(&e)),
            }
        }
        _ => (404, "text/plain; charset=utf-8", b"Not Found\n".to_vec()),
    }
}

pub fn run(args: &MockArgs) -> i32 {
    if !(1..=31).contains(&args.days) {
        eprintln!(
            "blyte mock: --days must be between 1 and 31, got {}",
            args.days
        );
        return 1;
    }

    let dash_args = DashboardArgs {
        port: args.port,
        roots: args.roots.clone(),
        runs_dirs: Vec::new(),
        hours: args.days.saturating_mul(24),
        open: false,
    };
    let (sources, where_from) = Sources::from_args_with_where(&dash_args);

    let listener = match std::net::TcpListener::bind(format!("127.0.0.1:{}", args.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error binding 127.0.0.1:{}: {e}", args.port);
            return 1;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error getting local address: {e}");
            return 1;
        }
    };
    let port = local_addr.port();
    let url = format!("http://127.0.0.1:{port}");
    if sources.roots.is_empty() && sources.runs_dirs.is_empty() {
        eprintln!(
            "blyte mock: nothing to watch yet: run blyte dispatch in a repository, or list roots in ~/.config/blyte/dashboard.toml"
        );
    } else {
        eprintln!(
            "blyte mock: watching {} repositories and {} run directories ({where_from})",
            sources.roots.len(),
            sources.runs_dirs.len()
        );
    }
    println!("blyte mock: {url}");
    let _ = std::io::stdout().flush();

    let live = args.roots.is_empty();
    let days = args.days;
    crate::dashboard::serve_http(listener, &["/api/replay", "/api/map"], move |path| {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(e) => {
                return (
                    500,
                    "text/plain; charset=utf-8",
                    format!("system clock error: {e}\n").into_bytes(),
                );
            }
        };
        if live {
            let src = Sources::from_args(&dash_args);
            route(path, &src, now, days)
        } else {
            route(path, &sources, now, days)
        }
    });

    0
}
