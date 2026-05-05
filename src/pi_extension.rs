use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fs_util::{acquire_lock, write_file_atomically};
use crate::paths::{
    home_dir, pi_extension_lock_path, pi_extension_lock_path_from_home, pi_extension_path,
    pi_extension_path_from_home,
};

const BUNDLED_PI_EXTENSION_SOURCE: &str = include_str!("../pi-extension/msp.ts");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiExtensionInstallStatus {
    Installed,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiExtensionInstallResult {
    pub path: PathBuf,
    pub status: PiExtensionInstallStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiExtensionRestoreResult {
    pub path: PathBuf,
    pub removed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiExtensionSyncStatus {
    NotInstalled,
    AlreadyCurrent,
    Updated,
}

pub fn bundled_pi_extension_source() -> &'static str {
    BUNDLED_PI_EXTENSION_SOURCE
}

pub fn install_global_pi_extension() -> Result<PiExtensionInstallResult, Box<dyn Error>> {
    let path = pi_extension_path()?;
    let lock_path = pi_extension_lock_path()?;
    install_global_pi_extension_at(&path, &lock_path)
}

pub fn restore_global_pi_extension() -> Result<PiExtensionRestoreResult, Box<dyn Error>> {
    let path = pi_extension_path()?;
    let lock_path = pi_extension_lock_path()?;
    restore_global_pi_extension_at(&path, &lock_path)
}

pub fn synchronize_global_pi_extension_if_installed()
-> Result<PiExtensionSyncStatus, Box<dyn Error>> {
    let Ok(home) = home_dir() else {
        return Ok(PiExtensionSyncStatus::NotInstalled);
    };

    let path = pi_extension_path_from_home(&home)?;
    let lock_path = pi_extension_lock_path_from_home(&home)?;
    synchronize_global_pi_extension_if_installed_at(&path, &lock_path)
}

fn install_global_pi_extension_at(
    path: &Path,
    lock_path: &Path,
) -> Result<PiExtensionInstallResult, Box<dyn Error>> {
    let _guard = acquire_lock(lock_path)?;
    let status = if path.exists() {
        PiExtensionInstallStatus::Updated
    } else {
        PiExtensionInstallStatus::Installed
    };
    write_file_atomically(path, bundled_pi_extension_source().as_bytes())?;

    Ok(PiExtensionInstallResult {
        path: path.to_path_buf(),
        status,
    })
}

fn restore_global_pi_extension_at(
    path: &Path,
    lock_path: &Path,
) -> Result<PiExtensionRestoreResult, Box<dyn Error>> {
    if !path.exists() {
        return Ok(PiExtensionRestoreResult {
            path: path.to_path_buf(),
            removed: false,
        });
    }

    let _guard = acquire_lock(lock_path)?;
    let removed = if path.exists() {
        fs::remove_file(path)?;
        true
    } else {
        false
    };

    Ok(PiExtensionRestoreResult {
        path: path.to_path_buf(),
        removed,
    })
}

fn synchronize_global_pi_extension_if_installed_at(
    path: &Path,
    lock_path: &Path,
) -> Result<PiExtensionSyncStatus, Box<dyn Error>> {
    if !path.exists() {
        return Ok(PiExtensionSyncStatus::NotInstalled);
    }

    let _guard = acquire_lock(lock_path)?;
    if !path.exists() {
        return Ok(PiExtensionSyncStatus::NotInstalled);
    }

    let installed_bytes = fs::read(path)?;
    if installed_bytes == bundled_pi_extension_source().as_bytes() {
        return Ok(PiExtensionSyncStatus::AlreadyCurrent);
    }

    write_file_atomically(path, bundled_pi_extension_source().as_bytes())?;
    Ok(PiExtensionSyncStatus::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{pi_extension_lock_path_from_home, pi_extension_path_from_home};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_home(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn sibling_lock_path_for(path: &Path) -> PathBuf {
        path.with_file_name("msp.ts.lock")
    }

    #[test]
    fn installs_bundled_pi_extension_into_global_path() {
        let home = unique_test_home("msp-pi-install");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();

        let result = install_global_pi_extension_at(&path, &lock_path).unwrap();

        assert_eq!(result.status, PiExtensionInstallStatus::Installed);
        assert_eq!(result.path, path);
        assert_eq!(
            fs::read_to_string(&result.path).unwrap(),
            bundled_pi_extension_source()
        );
        assert!(!sibling_lock_path_for(&path).exists());
        assert!(lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn install_overwrites_existing_global_pi_extension() {
        let home = unique_test_home("msp-pi-install-overwrite");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "old content").unwrap();

        let result = install_global_pi_extension_at(&path, &lock_path).unwrap();

        assert_eq!(result.status, PiExtensionInstallStatus::Updated);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            bundled_pi_extension_source()
        );
        assert!(!sibling_lock_path_for(&path).exists());
        assert!(lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn restore_removes_installed_global_pi_extension() {
        let home = unique_test_home("msp-pi-restore");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bundled_pi_extension_source()).unwrap();

        let result = restore_global_pi_extension_at(&path, &lock_path).unwrap();

        assert!(result.removed);
        assert!(!path.exists());
        assert!(!sibling_lock_path_for(&path).exists());
        assert!(lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn restore_skips_missing_global_pi_extension() {
        let home = unique_test_home("msp-pi-restore-missing");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();

        let result = restore_global_pi_extension_at(&path, &lock_path).unwrap();

        assert!(!result.removed);
        assert_eq!(result.path, path);
        assert!(!path.parent().unwrap().exists());
        assert!(!lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn sync_updates_existing_outdated_global_pi_extension() {
        let home = unique_test_home("msp-pi-sync-update");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "outdated").unwrap();

        let result = synchronize_global_pi_extension_if_installed_at(&path, &lock_path).unwrap();

        assert_eq!(result, PiExtensionSyncStatus::Updated);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            bundled_pi_extension_source()
        );
        assert!(!sibling_lock_path_for(&path).exists());
        assert!(lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn sync_skips_missing_global_pi_extension() {
        let home = unique_test_home("msp-pi-sync-missing");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();

        let result = synchronize_global_pi_extension_if_installed_at(&path, &lock_path).unwrap();

        assert_eq!(result, PiExtensionSyncStatus::NotInstalled);
        assert!(!path.parent().unwrap().exists());
        assert!(!lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn sync_skips_when_global_pi_extension_is_current() {
        let home = unique_test_home("msp-pi-sync-current");
        let path = pi_extension_path_from_home(&home).unwrap();
        let lock_path = pi_extension_lock_path_from_home(&home).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bundled_pi_extension_source()).unwrap();

        let result = synchronize_global_pi_extension_if_installed_at(&path, &lock_path).unwrap();

        assert_eq!(result, PiExtensionSyncStatus::AlreadyCurrent);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            bundled_pi_extension_source()
        );
        assert!(!sibling_lock_path_for(&path).exists());
        assert!(lock_path.exists());
        fs::remove_dir_all(home).unwrap();
    }
}
