use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use reqwest::{Client, redirect::Policy};
use tar::Archive;

use crate::fs_util::acquire_sibling_lock;
use crate::paths::unix_epoch_ms;

use super::compare::{compare_versions, normalize_release_tag, parse_release_version_from_url};
use super::state::{
    InstalledVersionRecord, load_installed_version_record, write_installed_version_record,
};
use super::{BINARY_NAME, CURRENT_VERSION, RELEASES_LATEST_URL, RELEASES_PAGE_URL};

pub(super) async fn try_auto_update(
    executable_path: &Path,
    latest_version: &str,
) -> Result<bool, Box<dyn Error>> {
    let _update_lock = acquire_sibling_lock(executable_path)?;

    if let Some(record) = load_installed_version_record(executable_path)? {
        match compare_versions(&record.latest_version, latest_version) {
            Some(Ordering::Equal | Ordering::Greater) => return Ok(false),
            Some(Ordering::Less) | None => {}
        }
    }

    let release_tag = normalize_release_tag(latest_version);
    let target = detect_release_target()?;
    let asset_name = format!("{BINARY_NAME}-{release_tag}-{target}.tar.gz");
    let download_url = format!("{RELEASES_PAGE_URL}/download/{release_tag}/{asset_name}");
    let archive_bytes = download_release_asset(&download_url).await?;
    let binary_bytes = extract_binary_from_archive(&archive_bytes)?;
    replace_executable_atomically(executable_path, &binary_bytes)?;
    write_installed_version_record(
        executable_path,
        &InstalledVersionRecord {
            updated_at: unix_epoch_ms()?,
            latest_version: latest_version.to_string(),
            release_url: download_url,
        },
    )?;
    Ok(true)
}

pub(super) async fn fetch_latest_release_version() -> Result<Option<String>, Box<dyn Error>> {
    let client = github_client()?;
    let response = client.head(RELEASES_LATEST_URL).send().await?;
    Ok(parse_release_version_from_url(response.url()))
}

async fn download_release_asset(url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let client = github_client()?;
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

fn extract_binary_from_archive(archive_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let entry_path = entry.path()?;
        let Some(file_name) = entry_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name != BINARY_NAME {
            continue;
        }

        let mut binary_bytes = Vec::new();
        entry.read_to_end(&mut binary_bytes)?;
        return Ok(binary_bytes);
    }

    Err(format!("release archive did not contain `{BINARY_NAME}`").into())
}

fn replace_executable_atomically(
    executable_path: &Path,
    binary_bytes: &[u8],
) -> Result<(), Box<dyn Error>> {
    let parent = executable_path
        .parent()
        .ok_or_else(|| "failed to resolve executable parent directory".to_string())?;
    fs::create_dir_all(parent)?;

    let temp_path = temporary_path_for(executable_path)?;
    fs::write(&temp_path, binary_bytes)?;
    #[cfg(unix)]
    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))?;

    if let Err(error) = fs::rename(&temp_path, executable_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(Box::new(error));
    }

    Ok(())
}

fn temporary_path_for(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "failed to derive file name".to_string())?;
    Ok(path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

fn detect_release_target() -> Result<&'static str, Box<dyn Error>> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("unsupported architecture: {other}").into()),
    };
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        other => return Err(format!("unsupported operating system: {other}").into()),
    };
    Ok(match (arch, os) {
        ("x86_64", "unknown-linux-gnu") => "x86_64-unknown-linux-gnu",
        ("aarch64", "unknown-linux-gnu") => "aarch64-unknown-linux-gnu",
        ("x86_64", "apple-darwin") => "x86_64-apple-darwin",
        ("aarch64", "apple-darwin") => "aarch64-apple-darwin",
        _ => unreachable!(),
    })
}

fn github_client() -> Result<Client, Box<dyn Error>> {
    Ok(Client::builder()
        .redirect(Policy::limited(10))
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), CURRENT_VERSION))
        .build()?)
}
