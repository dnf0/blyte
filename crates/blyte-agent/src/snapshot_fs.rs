//! An [`Fs`] wrapper that saves the original bytes of every overwritten file before writing.

use std::collections::{BTreeMap, HashSet};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::fs::Fs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub existed: bool,
}

pub struct SnapshotFs<F: Fs> {
    inner: F,
    root: PathBuf,
    before_dir: PathBuf,
    manifest: PathBuf,
    recorded: Mutex<HashSet<String>>,
}

impl<F: Fs> SnapshotFs<F> {
    pub fn new(
        inner: F,
        root: PathBuf,
        before_dir: PathBuf,
        manifest: PathBuf,
    ) -> Result<Self, String> {
        let root = std::fs::canonicalize(&root)
            .map_err(|e| format!("cannot canonicalize {}: {e}", root.display()))?;
        std::fs::create_dir_all(&before_dir)
            .map_err(|e| format!("cannot create {}: {e}", before_dir.display()))?;
        let existing = read_manifest(&manifest)?;
        let recorded = existing.into_iter().map(|c| c.path).collect();
        Ok(Self {
            inner,
            root,
            before_dir,
            manifest,
            recorded: Mutex::new(recorded),
        })
    }

    fn resolve_inside_root(&self, path: &Path) -> Result<(PathBuf, PathBuf, String), String> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?
                .join(path)
        };
        let resolved = resolve_with_existing_ancestors(&abs, path)?;
        let rel = resolved.strip_prefix(&self.root).map_err(|_| {
            format!(
                "path {} resolves to {}, outside root {}",
                path.display(),
                resolved.display(),
                self.root.display()
            )
        })?;
        let mut parts = Vec::new();
        for comp in rel.components() {
            match comp {
                Component::Normal(s) => {
                    let part = s
                        .to_str()
                        .ok_or_else(|| format!("non-UTF-8 path: {}", path.display()))?;
                    parts.push(part);
                }
                _ => {
                    return Err(format!(
                        "path {} resolves outside root {}",
                        path.display(),
                        self.root.display()
                    ));
                }
            }
        }
        if parts.is_empty() {
            return Err(format!(
                "path {} refers to root directory {}",
                path.display(),
                self.root.display()
            ));
        }
        let rel_path = rel.to_path_buf();
        let rel_key = parts.join("/");
        Ok((resolved, rel_path, rel_key))
    }

    fn snapshot_before_first_write(
        &self,
        original_path: &Path,
        resolved: &Path,
        rel_path: &Path,
        rel_key: &str,
    ) -> Result<(), String> {
        let mut recorded = self.recorded.lock().map_err(|e| {
            format!(
                "cannot snapshot {}: snapshot state lock poisoned: {e}",
                original_path.display()
            )
        })?;
        if recorded.contains(rel_key) {
            return Ok(());
        }
        let existed = match std::fs::read(resolved) {
            Ok(bytes) => {
                let dest = self.before_dir.join(rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "cannot save original of {} in {}: {e}",
                            original_path.display(),
                            parent.display()
                        )
                    })?;
                }
                std::fs::write(&dest, &bytes).map_err(|e| {
                    format!(
                        "cannot save original of {} to {}: {e}",
                        original_path.display(),
                        dest.display()
                    )
                })?;
                true
            }
            Err(e) if e.kind() == ErrorKind::NotFound => false,
            Err(e) => {
                return Err(format!(
                    "cannot read original of {}: {e}",
                    original_path.display()
                ));
            }
        };
        let entry = Change {
            path: rel_key.to_string(),
            existed,
        };
        let line = serde_json::to_string(&entry).map_err(|e| {
            format!(
                "cannot serialize manifest entry for {}: {e}",
                original_path.display()
            )
        })?;
        if let Some(parent) = self.manifest.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "cannot record {} in {}: {e}",
                        original_path.display(),
                        self.manifest.display()
                    )
                })?;
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.manifest)
            .map_err(|e| {
                format!(
                    "cannot record {} in {}: {e}",
                    original_path.display(),
                    self.manifest.display()
                )
            })?;
        writeln!(file, "{line}").map_err(|e| {
            format!(
                "cannot record {} in {}: {e}",
                original_path.display(),
                self.manifest.display()
            )
        })?;
        file.flush().map_err(|e| {
            format!(
                "cannot record {} in {}: {e}",
                original_path.display(),
                self.manifest.display()
            )
        })?;
        recorded.insert(rel_key.to_string());
        Ok(())
    }
}

impl<F: Fs> Fs for SnapshotFs<F> {
    fn is_local(&self) -> bool {
        self.inner.is_local()
    }

    fn read(&self, path: &Path) -> Result<String, String> {
        self.inner.read(path)
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        let (resolved, rel_path, rel_key) = self.resolve_inside_root(path)?;
        self.snapshot_before_first_write(path, &resolved, &rel_path, &rel_key)?;
        self.inner.write(path, content)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        self.inner.canonicalize(path)
    }
}

fn resolve_with_existing_ancestors(abs: &Path, display_path: &Path) -> Result<PathBuf, String> {
    let mut cur = PathBuf::new();
    let mut existing = true;
    for comp in abs.components() {
        match comp {
            Component::Prefix(p) => cur.push(p.as_os_str()),
            Component::RootDir => cur.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                cur.pop();
                if !existing {
                    if let Ok(canon) = std::fs::canonicalize(&cur) {
                        cur = canon;
                        existing = true;
                    }
                }
            }
            Component::Normal(part) => {
                cur.push(part);
                if existing {
                    match std::fs::canonicalize(&cur) {
                        Ok(canon) => cur = canon,
                        Err(e) => match std::fs::symlink_metadata(&cur) {
                            Err(meta_err) if meta_err.kind() == ErrorKind::NotFound => {
                                existing = false;
                            }
                            Ok(_) => {
                                return Err(format!(
                                    "cannot resolve {}: {e}",
                                    display_path.display()
                                ));
                            }
                            Err(meta_err) => {
                                return Err(format!(
                                    "cannot resolve {}: {meta_err}",
                                    display_path.display()
                                ));
                            }
                        },
                    }
                }
            }
        }
    }
    Ok(cur)
}

pub fn read_manifest(manifest: &Path) -> Result<Vec<Change>, String> {
    let text = match std::fs::read_to_string(manifest) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read manifest {}: {e}", manifest.display())),
    };
    let mut changes = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let change: Change = serde_json::from_str(line).map_err(|e| {
            format!(
                "cannot parse manifest {} line {line_no}: {e}",
                manifest.display()
            )
        })?;
        changes.push(change);
    }
    Ok(changes)
}

struct ChangedEntry {
    path: String,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn collect_changed(
    root: &Path,
    before_dir: &Path,
    manifest: &Path,
) -> Result<Vec<ChangedEntry>, String> {
    let manifest_entries = read_manifest(manifest)?;
    let mut unique: BTreeMap<String, bool> = BTreeMap::new();
    for change in manifest_entries {
        unique.entry(change.path).or_insert(change.existed);
    }
    let mut out = Vec::new();
    for (rel_path, existed) in unique {
        let current_path = root.join(&rel_path);
        let after = read_optional_bytes(&current_path)?;
        if existed {
            let before_path = before_dir.join(&rel_path);
            let before = std::fs::read(&before_path).map_err(|e| {
                format!(
                    "missing saved original for {rel_path} at {}: {e}",
                    before_path.display()
                )
            })?;
            let changed = match &after {
                Some(cur) => cur != &before,
                None => true,
            };
            if changed {
                out.push(ChangedEntry {
                    path: rel_path,
                    before: Some(before),
                    after,
                });
            }
        } else if after.is_some() {
            out.push(ChangedEntry {
                path: rel_path,
                before: None,
                after,
            });
        }
    }
    Ok(out)
}

pub fn changed_files(
    root: &Path,
    before_dir: &Path,
    manifest: &Path,
) -> Result<Vec<String>, String> {
    let entries = collect_changed(root, before_dir, manifest)?;
    Ok(entries.into_iter().map(|e| e.path).collect())
}

pub fn unified_diff(root: &Path, before_dir: &Path, manifest: &Path) -> Result<String, String> {
    let entries = collect_changed(root, before_dir, manifest)?;
    let mut out = String::new();
    for entry in entries {
        let before_utf8 = entry
            .before
            .as_deref()
            .is_none_or(|b| std::str::from_utf8(b).is_ok());
        let after_utf8 = entry
            .after
            .as_deref()
            .is_none_or(|b| std::str::from_utf8(b).is_ok());
        if !before_utf8 || !after_utf8 {
            out.push_str(&format!(
                "Binary files a/{} and b/{} differ\n",
                entry.path, entry.path
            ));
            continue;
        }
        let (old_label, old_path) = if entry.before.is_some() {
            (format!("a/{}", entry.path), before_dir.join(&entry.path))
        } else {
            ("/dev/null".to_string(), PathBuf::from("/dev/null"))
        };
        let (new_label, new_path) = if entry.after.is_some() {
            (format!("b/{}", entry.path), root.join(&entry.path))
        } else {
            ("/dev/null".to_string(), PathBuf::from("/dev/null"))
        };
        let output = Command::new("diff")
            .arg("-u")
            .arg("--label")
            .arg(&old_label)
            .arg("--label")
            .arg(&new_label)
            .arg("--")
            .arg(&old_path)
            .arg(&new_path)
            .output()
            .map_err(|e| format!("cannot run diff for {}: {e}", entry.path))?;
        match output.status.code() {
            Some(0 | 1) => {
                let chunk = String::from_utf8(output.stdout).map_err(|e| {
                    format!("diff output for {} is not valid UTF-8: {e}", entry.path)
                })?;
                out.push_str(&chunk);
                if !chunk.is_empty() && !chunk.ends_with('\n') {
                    out.push('\n');
                }
            }
            code => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "diff failed for {} (exit {:?}): {}",
                    entry.path,
                    code,
                    stderr.trim()
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::LocalFs;

    #[test]
    fn a_poisoned_recorded_mutex_refuses_writes_and_leaves_the_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("folder");
        std::fs::create_dir_all(&root).unwrap();
        let before_dir = tmp.path().join("before");
        let manifest = tmp.path().join("manifest.jsonl");
        let target = root.join("notes.txt");
        std::fs::write(&target, "original\n").unwrap();

        let sfs = SnapshotFs::new(LocalFs, root, before_dir.clone(), manifest.clone()).unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = sfs.recorded.lock().unwrap();
            panic!("poison snapshot lock");
        }));

        let err = sfs.write(&target, "updated\n").unwrap_err();
        assert!(err.contains("notes.txt"), "{err}");
        assert!(err.contains("poison"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original\n");
        assert!(!before_dir.join("notes.txt").exists());
        assert!(read_manifest(&manifest).unwrap().is_empty());
    }
}
