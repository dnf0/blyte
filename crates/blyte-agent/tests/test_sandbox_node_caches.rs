//! A worktree inside a Node repository may write the enclosing node_modules' tool caches (vitest's
//! .vite-temp), never its packages: `npm test` failed with EPERM under the sandbox (2026-10-01).

use blyte_agent::sandbox::default_allowed;

#[test]
fn enclosing_node_modules_caches_are_writable_and_packages_are_not() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repo");
    let nm = repo.join("node_modules");
    std::fs::create_dir_all(nm.join("vitest")).unwrap();
    let wt = repo.join(".wt").join("T1");
    std::fs::create_dir_all(&wt).unwrap();

    let allowed = default_allowed(&wt);
    for name in [".vite-temp", ".vite", ".cache"] {
        assert!(
            allowed.contains(&nm.join(name)),
            "{name} missing from {allowed:?}"
        );
        assert!(nm.join(name).is_dir(), "{name} not created");
    }
    assert!(
        !allowed.contains(&nm),
        "node_modules itself must not be writable"
    );
    assert!(!allowed.iter().any(|p| p == &nm.join("vitest")));

    // A repository without node_modules gets nothing extra.
    let plain = t.path().join("plain").join(".wt").join("T2");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(!default_allowed(&plain)
        .iter()
        .any(|p| p.to_string_lossy().contains("node_modules")));
}
