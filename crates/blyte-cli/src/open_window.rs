use std::path::{Path, PathBuf};
use std::process::Command;

pub const OPEN_IN_ENV: &str = "BLYTE_OPEN_IN";

pub const APP_FALLBACK_NOTICE: &str = "blyte: no Chrome, Chromium, Edge or Brave found for an app window; opening in the default browser (--open-in browser stops this notice)";

pub const MACOS_APP_BROWSERS: &[&str] = &[
    "Google Chrome",
    "Chromium",
    "Microsoft Edge",
    "Brave Browser",
];

pub const LINUX_APP_BROWSERS: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "brave-browser",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenIn {
    #[default]
    App,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPlan {
    pub program: String,
    pub args: Vec<String>,
    pub notice: Option<String>,
}

pub fn parse_open_in(raw: &str, source: &str) -> Result<OpenIn, String> {
    match raw {
        "app" => Ok(OpenIn::App),
        "browser" => Ok(OpenIn::Browser),
        _ => Err(format!(
            "invalid value `{raw}` for {source}; expected `app` or `browser`"
        )),
    }
}

pub fn resolve_open_in(flag: Option<&str>, env: Option<&str>) -> Result<OpenIn, String> {
    if let Some(raw) = flag {
        return parse_open_in(raw, "--open-in");
    }
    if let Some(raw) = env {
        return parse_open_in(raw, OPEN_IN_ENV);
    }
    Ok(OpenIn::App)
}

pub fn resolve_open_in_from_env(flag: Option<&str>) -> Result<OpenIn, String> {
    let env_val = std::env::var(OPEN_IN_ENV).ok();
    resolve_open_in(flag, env_val.as_deref())
}

pub fn open_command(
    open_in: OpenIn,
    url: &str,
    os: &str,
    installed: &dyn Fn(&str) -> bool,
) -> Result<OpenPlan, String> {
    match (os, open_in) {
        ("macos", OpenIn::App) => {
            for &name in MACOS_APP_BROWSERS {
                if installed(name) {
                    return Ok(OpenPlan {
                        program: "open".to_string(),
                        args: vec![
                            "-na".to_string(),
                            name.to_string(),
                            "--args".to_string(),
                            format!("--app={url}"),
                        ],
                        notice: None,
                    });
                }
            }
            Ok(OpenPlan {
                program: "open".to_string(),
                args: vec![url.to_string()],
                notice: Some(APP_FALLBACK_NOTICE.to_string()),
            })
        }
        ("macos", OpenIn::Browser) => Ok(OpenPlan {
            program: "open".to_string(),
            args: vec![url.to_string()],
            notice: None,
        }),
        ("linux", OpenIn::App) => {
            for &program in LINUX_APP_BROWSERS {
                if installed(program) {
                    return Ok(OpenPlan {
                        program: program.to_string(),
                        args: vec![format!("--app={url}")],
                        notice: None,
                    });
                }
            }
            Ok(OpenPlan {
                program: "xdg-open".to_string(),
                args: vec![url.to_string()],
                notice: Some(APP_FALLBACK_NOTICE.to_string()),
            })
        }
        ("linux", OpenIn::Browser) => Ok(OpenPlan {
            program: "xdg-open".to_string(),
            args: vec![url.to_string()],
            notice: None,
        }),
        _ => Err(format!("opening a surface window is not supported on {os}")),
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_macos_app_installed(name: &str) -> bool {
    let bundle = format!("{name}.app");
    if Path::new("/Applications").join(&bundle).exists() {
        return true;
    }
    if let Some(home) = std::env::var_os("HOME") {
        if PathBuf::from(home)
            .join("Applications")
            .join(&bundle)
            .exists()
        {
            return true;
        }
    }
    false
}

fn is_on_path(program: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_var) {
        if is_executable_file(&dir.join(program)) {
            return true;
        }
    }
    false
}

pub fn is_browser_installed(name: &str, os: &str) -> bool {
    match os {
        "macos" => is_macos_app_installed(name),
        "linux" => is_on_path(name),
        _ => false,
    }
}

pub fn open_url(open_in: OpenIn, url: &str) {
    let os = std::env::consts::OS;
    let plan = match open_command(open_in, url, os, &|name| is_browser_installed(name, os)) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("blyte: {e}; open {url} yourself");
            return;
        }
    };
    if let Some(notice) = plan.notice {
        eprintln!("{notice}");
    }
    if let Err(e) = Command::new(&plan.program).args(&plan.args).spawn() {
        eprintln!(
            "blyte: could not open a browser with {}: {e}; open {url} yourself",
            plan.program
        );
    }
}
