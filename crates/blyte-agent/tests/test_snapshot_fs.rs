use std::path::PathBuf;

use blyte_agent::fs::{Fs, LocalFs};
use blyte_agent::snapshot_fs::{changed_files, read_manifest, unified_diff, Change, SnapshotFs};

struct TestFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    before_dir: PathBuf,
    manifest: PathBuf,
}

impl TestFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("folder");
        std::fs::create_dir_all(&root).unwrap();
        let before_dir = tmp.path().join("before");
        let manifest = tmp.path().join("manifest.jsonl");
        Self {
            _tmp: tmp,
            root,
            before_dir,
            manifest,
        }
    }

    fn snapshot_fs(&self) -> SnapshotFs<LocalFs> {
        SnapshotFs::new(
            LocalFs,
            self.root.clone(),
            self.before_dir.clone(),
            self.manifest.clone(),
        )
        .unwrap()
    }
}

#[test]
fn the_original_is_saved_before_the_first_overwrite() {
    let f = TestFixture::new();
    let target = f.root.join("notes.txt");
    std::fs::write(&target, "original content\n").unwrap();

    let sfs = f.snapshot_fs();
    assert!(sfs.is_local());
    assert_eq!(sfs.read(&target).unwrap(), "original content\n");
    sfs.write(&target, "updated content\n").unwrap();

    assert_eq!(
        std::fs::read(f.before_dir.join("notes.txt")).unwrap(),
        b"original content\n"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "updated content\n"
    );
    let raw = std::fs::read_to_string(&f.manifest).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines, vec![r#"{"path":"notes.txt","existed":true}"#]);
    assert_eq!(
        read_manifest(&f.manifest).unwrap(),
        vec![Change {
            path: "notes.txt".to_string(),
            existed: true,
        }]
    );
}

#[test]
fn a_new_file_is_recorded_as_not_existing() {
    let f = TestFixture::new();
    let target = f.root.join("sub/fresh.txt");

    let sfs = f.snapshot_fs();
    sfs.write(&target, "brand new\n").unwrap();

    assert!(!f.before_dir.join("sub/fresh.txt").exists());
    assert_eq!(std::fs::read_dir(&f.before_dir).unwrap().count(), 0);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "brand new\n");
    assert_eq!(
        read_manifest(&f.manifest).unwrap(),
        vec![Change {
            path: "sub/fresh.txt".to_string(),
            existed: false,
        }]
    );
}

#[test]
fn a_second_write_keeps_the_first_original() {
    let f = TestFixture::new();
    let target = f.root.join("doc.txt");
    std::fs::write(&target, "v1\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "v2\n").unwrap();
    sfs.write(&target, "v3\n").unwrap();

    assert_eq!(
        std::fs::read(f.before_dir.join("doc.txt")).unwrap(),
        b"v1\n"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "v3\n");
    assert_eq!(read_manifest(&f.manifest).unwrap().len(), 1);
}

#[test]
fn a_binary_original_is_saved_byte_for_byte() {
    let f = TestFixture::new();
    let target = f.root.join("data.bin");
    let bytes: [u8; 4] = [0, 159, 146, 150];
    std::fs::write(&target, bytes).unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "now text\n").unwrap();

    assert_eq!(std::fs::read(f.before_dir.join("data.bin")).unwrap(), bytes);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "now text\n");
}

#[test]
fn a_write_outside_the_root_is_refused() {
    let f = TestFixture::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("escaped.txt");
    let dotdot = f.root.join("../escaped2.txt");

    let sfs = f.snapshot_fs();
    let err = sfs.write(&target, "nope\n").unwrap_err();
    assert!(err.contains("escaped.txt"), "{err}");
    assert!(!target.exists());

    let err2 = sfs.write(&dotdot, "nope\n").unwrap_err();
    assert!(err2.contains("escaped2.txt"), "{err2}");
    assert!(!dotdot.exists());
    assert!(read_manifest(&f.manifest).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_write_through_a_symlinked_folder_out_of_the_root_is_refused() {
    let f = TestFixture::new();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), f.root.join("link_out")).unwrap();

    let sfs = f.snapshot_fs();
    let target = f.root.join("link_out/secret.txt");
    let err = sfs.write(&target, "nope\n").unwrap_err();
    assert!(err.contains("secret.txt"), "{err}");
    assert!(!outside.path().join("secret.txt").exists());

    let nested = f.root.join("link_out/deep/secret2.txt");
    let err2 = sfs.write(&nested, "nope\n").unwrap_err();
    assert!(err2.contains("secret2.txt"), "{err2}");
    assert!(!outside.path().join("deep/secret2.txt").exists());
    assert!(read_manifest(&f.manifest).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn when_the_original_cannot_be_saved_the_file_is_untouched() {
    use std::os::unix::fs::PermissionsExt;

    struct RestoreMode(PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }

    let f = TestFixture::new();
    let target = f.root.join("keep.txt");
    std::fs::write(&target, "untouched\n").unwrap();

    let sfs = f.snapshot_fs();
    std::fs::set_permissions(&f.before_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let _restore = RestoreMode(f.before_dir.clone());

    let err = sfs.write(&target, "overwritten\n").unwrap_err();
    assert!(err.contains("keep.txt"), "{err}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
    assert!(read_manifest(&f.manifest).unwrap().is_empty());
}

#[test]
fn a_reopened_manifest_does_not_snapshot_again() {
    let f = TestFixture::new();
    let target = f.root.join("resume.txt");
    std::fs::write(&target, "first\n").unwrap();

    let first = f.snapshot_fs();
    first.write(&target, "second\n").unwrap();
    drop(first);

    let second = f.snapshot_fs();
    second.write(&target, "third\n").unwrap();

    assert_eq!(
        std::fs::read(f.before_dir.join("resume.txt")).unwrap(),
        b"first\n"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "third\n");
    assert_eq!(read_manifest(&f.manifest).unwrap().len(), 1);
}

#[test]
fn changed_files_leaves_out_an_identical_rewrite() {
    let f = TestFixture::new();
    let same = f.root.join("same.txt");
    let reverted = f.root.join("reverted.txt");
    std::fs::write(&same, "unchanged\n").unwrap();
    std::fs::write(&reverted, "alpha\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&same, "unchanged\n").unwrap();
    sfs.write(&reverted, "beta\n").unwrap();
    sfs.write(&reverted, "alpha\n").unwrap();

    assert!(changed_files(&f.root, &f.before_dir, &f.manifest)
        .unwrap()
        .is_empty());
}

#[test]
fn changed_files_lists_created_and_deleted_files() {
    let f = TestFixture::new();
    let existing = f.root.join("b_mod.txt");
    let deleted = f.root.join("a_del.txt");
    let created = f.root.join("c_new.txt");
    let ephemeral = f.root.join("d_gone.txt");
    std::fs::write(&existing, "before\n").unwrap();
    std::fs::write(&deleted, "gone soon\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&existing, "after\n").unwrap();
    sfs.write(&deleted, "edited then deleted\n").unwrap();
    std::fs::remove_file(&deleted).unwrap();
    sfs.write(&created, "hello\n").unwrap();
    sfs.write(&ephemeral, "temp\n").unwrap();
    std::fs::remove_file(&ephemeral).unwrap();

    assert_eq!(
        changed_files(&f.root, &f.before_dir, &f.manifest).unwrap(),
        vec!["a_del.txt", "b_mod.txt", "c_new.txt"]
    );
}

#[test]
fn a_missing_saved_original_is_an_error() {
    let f = TestFixture::new();
    let target = f.root.join("lost.txt");
    std::fs::write(&target, "before\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "after\n").unwrap();
    std::fs::remove_file(f.before_dir.join("lost.txt")).unwrap();

    let err = changed_files(&f.root, &f.before_dir, &f.manifest).unwrap_err();
    assert!(err.contains("lost.txt"), "{err}");

    let diff_err = unified_diff(&f.root, &f.before_dir, &f.manifest).unwrap_err();
    assert!(diff_err.contains("lost.txt"), "{diff_err}");
}

#[test]
fn a_corrupt_manifest_line_is_an_error_naming_the_line() {
    let f = TestFixture::new();
    std::fs::write(
        &f.manifest,
        "{\"path\":\"ok.txt\",\"existed\":true}\nnot valid json\n",
    )
    .unwrap();

    let err = read_manifest(&f.manifest).unwrap_err();
    assert!(err.contains("2"), "{err}");
}

#[test]
fn the_diff_shows_old_and_new_lines() {
    let f = TestFixture::new();
    let target = f.root.join("notes.txt");
    std::fs::write(&target, "old\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "new\n").unwrap();

    let diff = unified_diff(&f.root, &f.before_dir, &f.manifest).unwrap();
    assert!(diff.contains("--- a/notes.txt"), "{diff}");
    assert!(diff.contains("+++ b/notes.txt"), "{diff}");
    assert!(diff.contains("-old"), "{diff}");
    assert!(diff.contains("+new"), "{diff}");
}

#[test]
fn the_diff_of_a_created_file_is_against_dev_null() {
    let f = TestFixture::new();
    let target = f.root.join("added.txt");

    let sfs = f.snapshot_fs();
    sfs.write(&target, "fresh line\n").unwrap();

    let diff = unified_diff(&f.root, &f.before_dir, &f.manifest).unwrap();
    assert!(diff.contains("--- /dev/null"), "{diff}");
    assert!(diff.contains("+++ b/added.txt"), "{diff}");
    assert!(diff.contains("+fresh line"), "{diff}");
}

#[test]
fn a_binary_change_is_one_line() {
    let f = TestFixture::new();
    let target = f.root.join("blob.bin");
    std::fs::write(&target, [0u8, 159, 146, 150]).unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "replacement\n").unwrap();

    let diff = unified_diff(&f.root, &f.before_dir, &f.manifest).unwrap();
    assert_eq!(
        diff.lines().collect::<Vec<_>>(),
        vec!["Binary files a/blob.bin and b/blob.bin differ"]
    );
}

#[test]
fn the_diff_of_a_deleted_file_is_against_dev_null() {
    let f = TestFixture::new();
    let target = f.root.join("removed.txt");
    std::fs::write(&target, "goodbye\n").unwrap();

    let sfs = f.snapshot_fs();
    sfs.write(&target, "temporary\n").unwrap();
    std::fs::remove_file(&target).unwrap();

    let diff = unified_diff(&f.root, &f.before_dir, &f.manifest).unwrap();
    assert!(diff.contains("--- a/removed.txt"), "{diff}");
    assert!(diff.contains("+++ /dev/null"), "{diff}");
    assert!(diff.contains("-goodbye"), "{diff}");
}
