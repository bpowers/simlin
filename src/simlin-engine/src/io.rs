// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;

/// Write `contents` to `path` atomically via a sibling temp file.
///
/// Writes to `{path}.new`, fsyncs the file, renames over the target,
/// then best-effort fsyncs the parent directory for durability.
/// Cleans up the temp file on any error.
///
/// On Unix the rename is fully atomic. On Windows `fs::rename` does not
/// overwrite an existing file, so the target is removed first; there is a
/// brief window where neither file exists. This is a known Windows
/// limitation -- true atomic replacement requires `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING`, which std does not expose.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut tmp_name = OsString::from(path.as_os_str());
    tmp_name.push(".new");
    let tmp_path = Path::new(&tmp_name);

    let result = write_and_rename(tmp_path, path, contents);
    if result.is_err() {
        let _ = fs::remove_file(tmp_path);
    }
    result
}

fn write_and_rename(tmp: &Path, target: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(tmp, contents)?;

    let file = fs::File::open(tmp)?;
    file.sync_all()?;
    drop(file);

    // On Windows, rename does not atomically replace an existing file.
    // Remove the target first so rename succeeds.
    #[cfg(target_os = "windows")]
    {
        let _ = fs::remove_file(target);
    }

    fs::rename(tmp, target)?;

    // Best-effort fsync on parent directory for rename durability.
    if let Some(parent) = target.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn writes_correct_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.txt");
        let content = b"hello, world";

        atomic_write(&target, content).unwrap();

        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn temp_file_cleaned_up_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.txt");

        atomic_write(&target, b"data").unwrap();

        let mut tmp_name = OsString::from(target.as_os_str());
        tmp_name.push(".new");
        let tmp_path = Path::new(&tmp_name);
        assert!(
            !tmp_path.exists(),
            "temp file should be removed after successful write"
        );
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.txt");

        fs::write(&target, b"old content").unwrap();
        atomic_write(&target, b"new content").unwrap();

        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back, b"new content");
    }

    #[test]
    fn returns_error_when_parent_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nonexistent").join("output.txt");

        let result = atomic_write(&target, b"data");
        assert!(
            result.is_err(),
            "should fail when parent directory does not exist"
        );
    }

    #[test]
    fn succeeds_on_normal_directory() {
        // Exercises the best-effort parent dir fsync path: on a normal
        // directory the fsync succeeds silently (no error propagated).
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fsync_test.txt");

        atomic_write(&target, b"fsync content").unwrap();

        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back, b"fsync content");
    }
}
