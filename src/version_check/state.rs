use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs_util::{acquire_sibling_lock, write_file_atomically};
use crate::paths::{
    home_dir, installed_version_record_path, unix_epoch_ms, version_check_record_path,
    version_check_record_path_from_home,
};

use super::compare::compare_versions;
use super::{CURRENT_VERSION, RELEASES_PAGE_URL};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct VersionUpdateRecord {
    pub(super) checked_at: u128,
    pub(super) current_version: String,
    pub(super) latest_version: String,
    pub(super) releases_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct InstalledVersionRecord {
    pub(super) updated_at: u128,
    pub(super) latest_version: String,
    pub(super) release_url: String,
}

pub(super) fn synchronize_current_installed_version_record() -> Result<(), Box<dyn Error>> {
    let executable_path = std::env::current_exe()?;
    let path = installed_version_record_path(&executable_path)?;
    let record = load_installed_version_record(&executable_path)?;
    let should_write = match record {
        Some(record) => matches!(
            compare_versions(&record.latest_version, CURRENT_VERSION),
            Some(Ordering::Less)
        ),
        None => true,
    };

    if should_write || !path.exists() {
        write_installed_version_record(
            &executable_path,
            &InstalledVersionRecord {
                updated_at: unix_epoch_ms()?,
                latest_version: CURRENT_VERSION.to_string(),
                release_url: RELEASES_PAGE_URL.to_string(),
            },
        )?;
    }

    Ok(())
}

pub(super) fn write_installed_version_record(
    executable_path: &Path,
    record: &InstalledVersionRecord,
) -> Result<(), Box<dyn Error>> {
    let path = installed_version_record_path(executable_path)?;
    write_json_record(path, record)
}

pub(super) fn load_installed_version_record(
    executable_path: &Path,
) -> Result<Option<InstalledVersionRecord>, Box<dyn Error>> {
    let path = installed_version_record_path(executable_path)?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

pub(super) fn update_cached_version_notice(
    latest_version: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let Some(latest_version) = latest_version else {
        return Ok(());
    };

    match compare_versions(CURRENT_VERSION, latest_version) {
        Some(Ordering::Less) => write_version_update_record(&VersionUpdateRecord {
            checked_at: unix_epoch_ms()?,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: latest_version.to_string(),
            releases_url: RELEASES_PAGE_URL.to_string(),
        })?,
        Some(Ordering::Equal | Ordering::Greater) => delete_version_update_record()?,
        None => {}
    }

    Ok(())
}

pub(super) fn load_version_update_record() -> Result<Option<VersionUpdateRecord>, Box<dyn Error>> {
    load_version_update_record_from_home(&home_dir()?)
}

pub(super) fn load_version_update_record_from_home(
    home: &Path,
) -> Result<Option<VersionUpdateRecord>, Box<dyn Error>> {
    let path = version_check_record_path_from_home(home)?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

fn write_version_update_record(record: &VersionUpdateRecord) -> Result<(), Box<dyn Error>> {
    let path = version_check_record_path()?;
    write_json_record(path, record)
}

pub(super) fn delete_version_update_record() -> Result<(), Box<dyn Error>> {
    let path = version_check_record_path()?;
    delete_file_with_lock(&path)
}

pub(super) fn write_json_record<T: Serialize>(
    path: PathBuf,
    record: &T,
) -> Result<(), Box<dyn Error>> {
    let _guard = acquire_sibling_lock(&path)?;
    let bytes = serde_json::to_vec_pretty(record)?;
    write_file_atomically(&path, &bytes)?;
    Ok(())
}

fn delete_file_with_lock(path: &Path) -> Result<(), Box<dyn Error>> {
    let _guard = acquire_sibling_lock(path)?;
    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(path)?;
    Ok(())
}
