use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PRIVATE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct GrantRules<'a> {
    /// Paths the shell must not read (from `protected_read_paths()`).
    pub protected: &'a [PathBuf],
    /// The subset blyte never grants, even with `--allow-read`: `$HOME/.config/blyte/env` and the surface token folder.
    pub own_protected: &'a [PathBuf],
    /// Temp folders that may be dropped and replaced by a private one.
    pub temp_dirs: &'a [PathBuf],
    /// Folders granted by their children when they contain a protected path (`$HOME/.cargo`).
    pub carve_dirs: &'a [PathBuf],
    /// Child names of a carved folder that are not granted for writes.
    pub carve_write_excluded: &'a [&'a str],
    /// The user's `--allow-read` paths.
    pub user_allow_read: &'a [PathBuf],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinedGrants {
    pub write: Vec<PathBuf>,
    pub read: Vec<PathBuf>,
    /// Temp folders removed from the lists, in the order first met.
    pub dropped_temp: Vec<PathBuf>,
}

struct ResolvedRules<'a> {
    protected: Vec<(&'a PathBuf, PathBuf)>,
    own_protected: Vec<(&'a PathBuf, PathBuf)>,
    temp_dirs: Vec<PathBuf>,
    carve_dirs: Vec<PathBuf>,
    carve_write_excluded: &'a [&'a str],
    user_allow_read: Vec<PathBuf>,
}

fn resolve_pairs(paths: &[PathBuf]) -> Result<Vec<(&PathBuf, PathBuf)>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push((p, super::resolve_protected_prefix(p)?));
    }
    Ok(out)
}

fn resolve_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(super::resolve_protected_prefix(p)?);
    }
    Ok(out)
}

fn confine_list(
    grants: Vec<PathBuf>,
    is_write: bool,
    rules: &ResolvedRules<'_>,
    dropped_temp: &mut Vec<PathBuf>,
    dropped_temp_resolved: &mut Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::with_capacity(grants.len());
    for g in grants {
        let g_res = super::resolve_protected_prefix(&g)?;
        let contained = rules
            .protected
            .iter()
            .find(|(_, p_res)| p_res.starts_with(&g_res));
        let enclosing = rules
            .protected
            .iter()
            .find(|(_, p_res)| g_res.starts_with(p_res));

        if contained.is_none() && enclosing.is_none() {
            out.push(g);
            continue;
        }

        if enclosing.is_some() && rules.user_allow_read.contains(&g_res) {
            if let Some((o_raw, _)) = rules
                .own_protected
                .iter()
                .find(|(_, o_res)| g_res.starts_with(o_res))
            {
                return Err(format!(
                    "--allow-read {} is refused: it is inside {}, which a run may never read",
                    g.display(),
                    o_raw.display()
                ));
            }
            out.push(g);
            continue;
        }

        if contained.is_some() && rules.temp_dirs.contains(&g_res) {
            if !dropped_temp_resolved.contains(&g_res) {
                dropped_temp_resolved.push(g_res);
                dropped_temp.push(g);
            }
            continue;
        }

        let strictly_contains_protected = rules
            .protected
            .iter()
            .any(|(_, p_res)| p_res.starts_with(&g_res) && *p_res != g_res);
        if enclosing.is_none() && strictly_contains_protected && rules.carve_dirs.contains(&g_res) {
            let entries = match std::fs::read_dir(&g) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("cannot read {}: {e}", g.display())),
            };
            let mut children = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| format!("cannot read {}: {e}", g.display()))?;
                children.push(entry.path());
            }
            children.sort();
            for child in children {
                let child_res = super::resolve_protected_prefix(&child)?;
                let touches_protected = rules.protected.iter().any(|(_, p_res)| {
                    p_res.starts_with(&child_res) || child_res.starts_with(p_res)
                });
                if touches_protected {
                    continue;
                }
                if is_write
                    && rules
                        .carve_write_excluded
                        .iter()
                        .any(|ex| child.file_name() == Some(OsStr::new(ex)))
                {
                    continue;
                }
                out.push(child);
            }
            continue;
        }

        if let Some((p_raw, _)) = contained {
            return Err(format!(
                "on Linux the sandbox cannot protect {}: granted path {} contains it",
                p_raw.display(),
                g.display()
            ));
        }
        if let Some((p_raw, _)) = enclosing {
            return Err(format!(
                "on Linux the sandbox cannot protect {}: granted path {} is inside it",
                p_raw.display(),
                g.display()
            ));
        }
    }
    Ok(out)
}

/// Filters and validates Landlock write and read grant lists against protected paths.
pub fn confine_grants(
    write: Vec<PathBuf>,
    read: Vec<PathBuf>,
    rules: &GrantRules<'_>,
) -> Result<ConfinedGrants, String> {
    let resolved = ResolvedRules {
        protected: resolve_pairs(rules.protected)?,
        own_protected: resolve_pairs(rules.own_protected)?,
        temp_dirs: resolve_paths(rules.temp_dirs)?,
        carve_dirs: resolve_paths(rules.carve_dirs)?,
        carve_write_excluded: rules.carve_write_excluded,
        user_allow_read: resolve_paths(rules.user_allow_read)?,
    };
    let mut dropped_temp = Vec::new();
    let mut dropped_temp_resolved = Vec::new();
    let write = confine_list(
        write,
        true,
        &resolved,
        &mut dropped_temp,
        &mut dropped_temp_resolved,
    )?;
    let read = confine_list(
        read,
        false,
        &resolved,
        &mut dropped_temp,
        &mut dropped_temp_resolved,
    )?;
    Ok(ConfinedGrants {
        write,
        read,
        dropped_temp,
    })
}

/// Chooses a unique private temp folder path under the first usable candidate directory.
pub fn choose_private_tmp(
    candidates: &[PathBuf],
    dropped: &[PathBuf],
    protected: &[PathBuf],
) -> Result<PathBuf, String> {
    let dropped_pairs = resolve_pairs(dropped)?;
    let protected_pairs = resolve_pairs(protected)?;

    for candidate in candidates {
        let c_res = super::resolve_protected_prefix(candidate)?;
        if dropped_pairs.iter().any(|(_, d_res)| *d_res == c_res) {
            continue;
        }
        if protected_pairs
            .iter()
            .any(|(_, p_res)| p_res.starts_with(&c_res) || c_res.starts_with(p_res))
        {
            continue;
        }
        if !Path::new(candidate).is_dir() {
            continue;
        }
        let n = PRIVATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        return Ok(candidate.join(format!("blyte-{}-{n}", std::process::id())));
    }

    if let Some((first_dropped_raw, first_dropped_res)) = dropped_pairs.first() {
        if let Some((p_raw, _)) = protected_pairs
            .iter()
            .find(|(_, p_res)| p_res.starts_with(first_dropped_res))
        {
            return Err(format!(
                "cannot choose a private temp folder: {} contains {} and no other temp folder is usable",
                first_dropped_raw.display(),
                p_raw.display()
            ));
        }
        return Err(format!(
            "cannot choose a private temp folder: {} was dropped and no other temp folder is usable",
            first_dropped_raw.display()
        ));
    }

    Err("cannot choose a private temp folder: no candidate temp folder is usable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_rules<'a>(protected: &'a [PathBuf]) -> GrantRules<'a> {
        GrantRules {
            protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &[],
            carve_write_excluded: &[],
            user_allow_read: &[],
        }
    }

    #[test]
    fn keeps_grants_unrelated_to_protected_paths() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("ws");
        let cache = root.path().join("cache");
        let sys = root.path().join("usr");
        let secret = root.path().join("home/.aws");
        let protected = [secret];
        let rules = empty_rules(&protected);

        let got = confine_grants(
            vec![ws.clone(), cache.clone()],
            vec![sys.clone(), ws.clone()],
            &rules,
        )
        .unwrap();

        assert_eq!(got.write, vec![ws.clone(), cache]);
        assert_eq!(got.read, vec![sys, ws]);
        assert!(got.dropped_temp.is_empty());
    }

    #[test]
    fn drops_temp_dir_containing_protected_path_from_both_lists() {
        let root = tempfile::tempdir().unwrap();
        let tmp = root.path().join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let ws = root.path().join("ws");
        let sys = root.path().join("usr");
        let protected = [tmp.join("home/.aws")];
        let temp_dirs = [tmp.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &temp_dirs,
            carve_dirs: &[],
            carve_write_excluded: &[],
            user_allow_read: &[],
        };

        let got = confine_grants(
            vec![ws.clone(), tmp.clone(), root.path().join("cache")],
            vec![sys.clone(), tmp.clone(), ws.clone()],
            &rules,
        )
        .unwrap();

        assert_eq!(got.write, vec![ws.clone(), root.path().join("cache")]);
        assert_eq!(got.read, vec![sys, ws]);
        assert_eq!(got.dropped_temp, vec![tmp]);
    }

    #[test]
    fn errors_when_non_temp_grant_contains_protected_path() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let aws = home.join(".aws");
        let protected = [aws.clone()];
        let rules = empty_rules(&protected);

        let err = confine_grants(vec![home.clone()], vec![], &rules).unwrap_err();
        assert!(err.contains("cannot protect"), "{err}");
        assert!(err.contains(&aws.display().to_string()), "{err}");
        assert!(err.contains(&home.display().to_string()), "{err}");
        assert!(err.ends_with("contains it"), "{err}");
    }

    #[test]
    fn errors_when_grant_equals_protected_path() {
        let root = tempfile::tempdir().unwrap();
        let aws = root.path().join("home/.aws");
        let protected = [aws.clone()];
        let rules = empty_rules(&protected);

        let err = confine_grants(vec![], vec![aws], &rules).unwrap_err();
        assert!(err.contains("cannot protect"), "{err}");
    }

    #[test]
    fn errors_when_grant_is_inside_protected_path() {
        let root = tempfile::tempdir().unwrap();
        let ssh = root.path().join("home/.ssh");
        let keys = ssh.join("keys");
        let protected = [ssh];
        let rules = empty_rules(&protected);

        let err = confine_grants(vec![], vec![keys], &rules).unwrap_err();
        assert!(err.ends_with("is inside it"), "{err}");
    }

    #[test]
    fn allow_read_of_protected_path_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let aws = root.path().join("home/.aws");
        let protected = [aws.clone()];
        let allow = [aws.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &[],
            carve_write_excluded: &[],
            user_allow_read: &allow,
        };

        let got = confine_grants(vec![], vec![aws.clone()], &rules).unwrap();
        assert_eq!(got.read, vec![aws]);
    }

    #[test]
    fn allow_read_inside_own_protected_path_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let env = root.path().join("home/.config/blyte/env");
        let protected = [env.clone()];
        let own = [env.clone()];
        let allow = [env.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &own,
            temp_dirs: &[],
            carve_dirs: &[],
            carve_write_excluded: &[],
            user_allow_read: &allow,
        };

        let err = confine_grants(vec![], vec![env], &rules).unwrap_err();
        assert!(err.contains("--allow-read"), "{err}");
        assert!(err.contains("may never read"), "{err}");
    }

    #[test]
    fn allow_read_of_ancestor_of_protected_path_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let aws = home.join(".aws");
        let protected = [aws];
        let allow = [home.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &[],
            carve_write_excluded: &[],
            user_allow_read: &allow,
        };

        let err = confine_grants(vec![], vec![home], &rules).unwrap_err();
        assert!(err.ends_with("contains it"), "{err}");
    }

    #[test]
    fn carves_folder_into_children_without_protected_ones() {
        let root = tempfile::tempdir().unwrap();
        let cargo = root.path().join(".cargo");
        std::fs::create_dir_all(cargo.join("registry")).unwrap();
        std::fs::create_dir_all(cargo.join("git")).unwrap();
        std::fs::create_dir_all(cargo.join("bin")).unwrap();
        std::fs::write(cargo.join("config.toml"), "").unwrap();
        std::fs::write(cargo.join("credentials.toml"), "secret").unwrap();
        std::fs::write(cargo.join("credentials"), "secret").unwrap();

        let protected = [cargo.join("credentials.toml"), cargo.join("credentials")];
        let carve_dirs = [cargo.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &carve_dirs,
            carve_write_excluded: &["bin", "config.toml"],
            user_allow_read: &[],
        };

        let got = confine_grants(vec![cargo.clone()], vec![cargo.clone()], &rules).unwrap();
        assert_eq!(
            got.read,
            vec![
                cargo.join("bin"),
                cargo.join("config.toml"),
                cargo.join("git"),
                cargo.join("registry"),
            ]
        );
        assert_eq!(got.write, vec![cargo.join("git"), cargo.join("registry")]);
    }

    #[test]
    fn carved_folder_that_does_not_exist_is_dropped() {
        let root = tempfile::tempdir().unwrap();
        let cargo = root.path().join(".cargo");
        let protected = [cargo.join("credentials.toml")];
        let carve_dirs = [cargo.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &carve_dirs,
            carve_write_excluded: &[],
            user_allow_read: &[],
        };

        let got = confine_grants(vec![cargo.clone()], vec![cargo], &rules).unwrap();
        assert!(got.write.is_empty());
        assert!(got.read.is_empty());
        assert!(got.dropped_temp.is_empty());
    }

    #[test]
    fn carve_dir_not_containing_protected_path_is_kept_whole() {
        let root = tempfile::tempdir().unwrap();
        let cargo = root.path().join(".cargo");
        std::fs::create_dir_all(&cargo).unwrap();
        let protected = [root.path().join("home/.aws")];
        let carve_dirs = [cargo.clone()];
        let rules = GrantRules {
            protected: &protected,
            own_protected: &[],
            temp_dirs: &[],
            carve_dirs: &carve_dirs,
            carve_write_excluded: &[],
            user_allow_read: &[],
        };

        let got = confine_grants(vec![cargo.clone()], vec![cargo.clone()], &rules).unwrap();
        assert_eq!(got.write, vec![cargo.clone()]);
        assert_eq!(got.read, vec![cargo]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_grant_to_protected_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let aws = root.path().join("home/.aws");
        std::fs::create_dir_all(&aws).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&aws, &link).unwrap();
        let protected = [aws];
        let rules = empty_rules(&protected);

        let err = confine_grants(vec![], vec![link], &rules).unwrap_err();
        assert!(err.contains("cannot protect"), "{err}");
    }

    #[test]
    fn choose_private_tmp_skips_dropped_and_protected_candidates() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let c = root.path().join("c");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();

        let dropped = [a.clone()];
        let protected = [b.join("home/.aws")];
        let chosen = choose_private_tmp(&[a, b, c.clone()], &dropped, &protected).unwrap();

        assert_eq!(chosen.parent(), Some(c.as_path()));
        let name = chosen.file_name().unwrap().to_str().unwrap();
        let prefix = format!("blyte-{}-", std::process::id());
        assert!(name.starts_with(&prefix), "{name}");
        assert!(!chosen.exists());
    }

    #[test]
    fn choose_private_tmp_returns_distinct_paths() {
        let root = tempfile::tempdir().unwrap();
        let c = root.path().to_path_buf();
        let first = choose_private_tmp(std::slice::from_ref(&c), &[], &[]).unwrap();
        let second = choose_private_tmp(std::slice::from_ref(&c), &[], &[]).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn choose_private_tmp_errors_when_no_candidate_is_usable() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        let prot = a.join("home/.aws");
        let err = choose_private_tmp(
            std::slice::from_ref(&a),
            std::slice::from_ref(&a),
            std::slice::from_ref(&prot),
        )
        .unwrap_err();
        assert!(err.contains(&a.display().to_string()), "{err}");
        assert!(err.contains(&prot.display().to_string()), "{err}");
        assert!(
            err.contains("cannot choose a private temp folder:"),
            "{err}"
        );
    }

    #[test]
    fn choose_private_tmp_skips_missing_candidate() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        let present = root.path().join("present");
        std::fs::create_dir_all(&present).unwrap();

        let chosen = choose_private_tmp(&[missing, present.clone()], &[], &[]).unwrap();
        assert_eq!(chosen.parent(), Some(present.as_path()));
    }
}
