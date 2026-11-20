use std::io::Write;

use crate::cli::{DashboardArgs, MockArgs};
use crate::dashboard::Sources;

pub fn route(path: &str, src: &Sources, now: u64, days: u64) -> (u16, &'static str, Vec<u8>) {
    let clean_path = match path.split_once('?') {
        Some((p, _)) => p,
        None => path,
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
    crate::dashboard::serve_http(listener, &["/api/replay"], move |path| {
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
