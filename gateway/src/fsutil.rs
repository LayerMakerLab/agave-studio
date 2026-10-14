//! File helpers shared by the key store and the user store.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Writes `bytes` to `path` atomically with 0600 permissions: a uniquely named temp file in the
/// same directory (O_EXCL, so a planted file or symlink is never followed), fsync, then rename.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(".{}.tmp", crate::media::random_hex(8)));
    let tmp = PathBuf::from(tmp_name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `dir` (and missing parents) as owner-only (0700). An existing folder that others can read is tightened
/// to 0700; if that fails (another owner, a read-only or chmod-less volume) a warning is printed and startup goes on.
/// A symlinked folder is left as the user set it up. Only a failure to create the folder is an error.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || meta.permissions().mode() & 0o077 == 0 {
        return Ok(());
    }
    if let Err(e) = fs::set_permissions(dir, fs::Permissions::from_mode(0o700)) {
        eprintln!(
            "could not make {} owner-only ({e}); other accounts on this computer may be able to read it",
            dir.display()
        );
    }
    Ok(())
}

/// Removes work folders and temp files a crash or kill left behind in `dir` (.edit-*, .speech-*, .warm-*,
/// and *.tmp from write_private). Only called once the gateway owns the port, so no other copy is using them.
pub fn remove_leftovers(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let work = [".edit-", ".speech-", ".warm-"].iter().any(|p| name.starts_with(p));
        if !work && !name.ends_with(".tmp") {
            continue;
        }
        let path = e.path();
        let _ = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    }
}

/// Empty scratch directory under target/ for tests (git-ignored).
#[cfg(test)]
pub fn test_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftovers_go_and_real_files_stay() {
        let dir = test_dir("fsutil_leftovers");
        fs::create_dir_all(dir.join(".edit-abc123/x")).unwrap();
        fs::write(dir.join(".warm-1a2b.wav"), b"x").unwrap();
        fs::write(dir.join("jobs.json.0011aabb.tmp"), b"x").unwrap();
        fs::write(dir.join("a1b2c3.mp4"), b"x").unwrap();
        remove_leftovers(&dir);
        let left: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["a1b2c3.mp4".to_string()]);
    }

    #[test]
    fn private_dirs_are_owner_only_even_if_they_existed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("fsutil_private").join("data/outputs");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let fresh = test_dir("fsutil_private_new").join("made");
        ensure_private_dir(&fresh).unwrap();
        assert_eq!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o700);
    }
}
