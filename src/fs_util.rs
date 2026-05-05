use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::sibling_lock_path;

pub struct FileLockGuard {
    _file: File,
}

pub fn acquire_lock(lock_path: &Path) -> io::Result<FileLockGuard> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock()?;
    Ok(FileLockGuard { _file: file })
}

pub fn acquire_sibling_lock(path: &Path) -> io::Result<FileLockGuard> {
    acquire_lock(&sibling_lock_path(path))
}

pub fn write_file_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = temporary_path_for(path)?;
    fs::write(&temp_path, bytes)?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    Ok(())
}

fn temporary_path_for(path: &Path) -> io::Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("failed to derive file name"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::other(error.to_string()))?
        .as_millis();
    Ok(path.with_file_name(format!(
        ".{file_name}.tmp-{}-{timestamp}",
        std::process::id(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquires_exact_lock_path() {
        let path = std::env::temp_dir().join(format!(
            "msp-fs-util-lock-{}-{}.lock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let _guard = acquire_lock(&path).unwrap();

        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn acquires_sibling_lock_next_to_target() {
        let path = std::env::temp_dir().join(format!(
            "msp-fs-util-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let _guard = acquire_sibling_lock(&path).unwrap();

        assert!(sibling_lock_path(&path).exists());
        let _ = fs::remove_file(sibling_lock_path(&path));
    }

    #[test]
    fn writes_file_atomically_to_target_path() {
        let dir = std::env::temp_dir().join(format!(
            "msp-fs-util-write-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("target.json");

        write_file_atomically(&path, br#"{"ok":true}"#).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"ok":true}"#);
        let _ = fs::remove_dir_all(dir);
    }
}
