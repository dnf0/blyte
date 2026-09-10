//! Reflink-based copy-on-write directory cache sharing powered by Worktrunk.

use std::fs;
use std::path::Path;
use worktrunk::copy::copy_dir_recursive;
use worktrunk::progress::Progress;

use crate::error::WorktreeError;

/// Reflink-copies named cache directories (e.g. `.venv`, `target`, `node_modules`)
/// from `source_root` to `target_root`.
///
/// Uses copy-on-write extents when supported by the underlying filesystem (APFS, btrfs, XFS)
/// without consuming additional disk space, falling back to standard file copy.
pub fn reflink_cache_dirs(
    source_root: &Path,
    target_root: &Path,
    cache_names: &[&str],
) -> Result<u64, WorktreeError> {
    let progress = Progress::disabled();
    let mut total_copied = 0u64;

    for cache_name in cache_names {
        let src = source_root.join(cache_name);
        if !src.exists() || !src.is_dir() {
            continue;
        }

        let dst = target_root.join(cache_name);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| WorktreeError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        copy_dir_recursive(&src, &dst, Some(target_root), true, &progress).map_err(|e| {
            WorktreeError::CacheCopy {
                src_path: src.clone(),
                dst_path: dst.clone(),
                details: e.to_string(),
            }
        })?;

        total_copied += 1;
    }

    Ok(total_copied)
}
