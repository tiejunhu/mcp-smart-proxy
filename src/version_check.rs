use std::cmp::Ordering;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use tar::Archive;

use crate::console::print_app_warning;
use crate::paths::{
    home_dir, installed_version_record_path, sibling_lock_path, unix_epoch_ms,
    version_check_record_path, version_check_record_path_from_home,
};

const BINARY_NAME: &str = "msp";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_LATEST_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases/latest";
const RELEASES_PAGE_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases";
const VERSION_CHECK_STAGE: &str = "startup.version_check";
const SELF_UPDATE_STAGE: &str = "startup.self_update";
const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VersionUpdateRecord {
    checked_at: u128,
    current_version: String,
    latest_version: String,
    releases_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstalledVersionRecord {
    updated_at: u128,
    latest_version: String,
    release_url: String,
}

struct PathLockGuard {
    _file: File,
}

pub fn prepare_executable_for_background_update(cli_args: &[OsString]) {
    if let Err(error) = synchronize_current_installed_version_record() {
        print_app_warning(
            SELF_UPDATE_STAGE,
            format!("Failed to synchronize the installed-version record: {error}"),
        );
    }

    if let Err(error) = restart_if_newer_installed_version_exists(cli_args) {
        print_app_warning(
            SELF_UPDATE_STAGE,
            format!("Failed to restart after detecting a newer installed version: {error}"),
        );
    }
}

pub fn spawn_periodic_self_update(cli_args: Vec<OsString>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(VERSION_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = run_self_update_cycle(&cli_args).await {
                print_app_warning(
                    SELF_UPDATE_STAGE,
                    format!("Background self-update skipped: {error}"),
                );
            }
        }
    });
}

pub fn print_cached_update_notice() {
    let Ok(Some(record)) = load_version_update_record() else {
        return;
    };

    if !matches!(
        compare_versions(CURRENT_VERSION, &record.latest_version),
        Some(Ordering::Less)
    ) {
        return;
    }

    print_app_warning(
        VERSION_CHECK_STAGE,
        format!(
            "A newer msp release is available: v{} (current: v{}). See {}",
            record.latest_version, CURRENT_VERSION, record.releases_url
        ),
    );
}

async fn run_self_update_cycle(cli_args: &[OsString]) -> Result<(), Box<dyn Error>> {
    synchronize_current_installed_version_record()?;
    restart_if_newer_installed_version_exists(cli_args)?;

    let latest_version = fetch_latest_release_version().await?;
    update_cached_version_notice(latest_version.as_deref())?;

    let Some(latest_version) = latest_version else {
        return Ok(());
    };

    if matches!(
        compare_versions(CURRENT_VERSION, &latest_version),
        Some(Ordering::Less)
    ) {
        let executable_path = std::env::current_exe()?;
        let _ = try_auto_update(&executable_path, &latest_version).await?;
        restart_if_newer_installed_version_exists(cli_args)?;
    }

    Ok(())
}

async fn try_auto_update(
    executable_path: &Path,
    latest_version: &str,
) -> Result<bool, Box<dyn Error>> {
    let _update_lock = acquire_path_lock(executable_path)?;

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

fn synchronize_current_installed_version_record() -> Result<(), Box<dyn Error>> {
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

    if should_write {
        write_installed_version_record(
            &executable_path,
            &InstalledVersionRecord {
                updated_at: unix_epoch_ms()?,
                latest_version: CURRENT_VERSION.to_string(),
                release_url: RELEASES_PAGE_URL.to_string(),
            },
        )?;
    } else if !path.exists() {
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

fn restart_if_newer_installed_version_exists(cli_args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let executable_path = std::env::current_exe()?;
    let Some(record) = load_installed_version_record(&executable_path)? else {
        return Ok(());
    };

    if !should_restart_for_installed_version(CURRENT_VERSION, &record.latest_version) {
        return Ok(());
    }

    exec_current_process(&executable_path, cli_args)
}

fn should_restart_for_installed_version(current_version: &str, latest_version: &str) -> bool {
    matches!(
        compare_versions(current_version, latest_version),
        Some(Ordering::Less)
    )
}

#[cfg(unix)]
fn exec_current_process(
    executable_path: &Path,
    cli_args: &[OsString],
) -> Result<(), Box<dyn Error>> {
    let error = Command::new(executable_path)
        .args(cli_args.iter().skip(1))
        .exec();
    Err(Box::new(error))
}

#[cfg(not(unix))]
fn exec_current_process(
    _executable_path: &Path,
    _cli_args: &[OsString],
) -> Result<(), Box<dyn Error>> {
    Err("automatic restart is supported only on Unix-like platforms".into())
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

fn normalize_release_tag(version: &str) -> String {
    if version.starts_with('v') {
        return version.to_string();
    }

    format!("v{version}")
}

fn write_installed_version_record(
    executable_path: &Path,
    record: &InstalledVersionRecord,
) -> Result<(), Box<dyn Error>> {
    let path = installed_version_record_path(executable_path)?;
    write_json_record(path, record)
}

fn load_installed_version_record(
    executable_path: &Path,
) -> Result<Option<InstalledVersionRecord>, Box<dyn Error>> {
    let path = installed_version_record_path(executable_path)?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

fn update_cached_version_notice(latest_version: Option<&str>) -> Result<(), Box<dyn Error>> {
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

async fn fetch_latest_release_version() -> Result<Option<String>, Box<dyn Error>> {
    let client = github_client()?;
    let response = client.head(RELEASES_LATEST_URL).send().await?;
    Ok(parse_release_version_from_url(response.url()))
}

fn github_client() -> Result<Client, Box<dyn Error>> {
    Ok(Client::builder()
        .redirect(Policy::limited(10))
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), CURRENT_VERSION))
        .build()?)
}

fn load_version_update_record() -> Result<Option<VersionUpdateRecord>, Box<dyn Error>> {
    load_version_update_record_from_home(&home_dir()?)
}

fn load_version_update_record_from_home(
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

fn delete_version_update_record() -> Result<(), Box<dyn Error>> {
    let path = version_check_record_path()?;
    delete_file_with_lock(&path)
}

fn write_json_record<T: Serialize>(path: PathBuf, record: &T) -> Result<(), Box<dyn Error>> {
    let _guard = acquire_path_lock(&path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(record)?;
    write_file_atomically(&path, &bytes)?;
    Ok(())
}

fn delete_file_with_lock(path: &Path) -> Result<(), Box<dyn Error>> {
    let _guard = acquire_path_lock(path)?;
    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(path)?;
    Ok(())
}

fn write_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = temporary_path_for(path)?;
    fs::write(&temp_path, bytes)?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(Box::new(error));
    }

    Ok(())
}

fn acquire_path_lock(path: &Path) -> Result<PathLockGuard, Box<dyn Error>> {
    let lock_path = sibling_lock_path(path);
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
    Ok(PathLockGuard { _file: file })
}

fn parse_release_version_from_url(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("cybershape"), Some("mcp-smart-proxy"), Some("releases"), Some("tag")) => {
            segments.next().and_then(parse_release_tag)
        }
        _ => parse_release_tag(url.path().rsplit('/').next()?),
    }
}

fn parse_release_tag(tag: &str) -> Option<String> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if version.is_empty() || !version.split('.').all(is_numeric_component) {
        return None;
    }
    Some(version.to_string())
}

fn compare_versions(current: &str, latest: &str) -> Option<Ordering> {
    let current = parse_version_components(current)?;
    let latest = parse_version_components(latest)?;

    let max_len = current.len().max(latest.len());
    for index in 0..max_len {
        let current_part = current.get(index).copied().unwrap_or_default();
        let latest_part = latest.get(index).copied().unwrap_or_default();
        match current_part.cmp(&latest_part) {
            Ordering::Equal => continue,
            ordering => return Some(ordering),
        }
    }

    Some(Ordering::Equal)
}

fn parse_version_components(version: &str) -> Option<Vec<u64>> {
    version
        .strip_prefix('v')
        .unwrap_or(version)
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

fn is_numeric_component(component: &str) -> bool {
    !component.is_empty() && component.chars().all(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            unix_epoch_ms().unwrap()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parses_release_version_from_latest_redirect_url() {
        let url = Url::parse("https://github.com/cybershape/mcp-smart-proxy/releases/tag/v0.0.16")
            .unwrap();

        assert_eq!(
            parse_release_version_from_url(&url).as_deref(),
            Some("0.0.16")
        );
    }

    #[test]
    fn rejects_non_numeric_release_tags() {
        assert_eq!(parse_release_tag("nightly"), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert_eq!(compare_versions("0.0.15", "0.0.16"), Some(Ordering::Less));
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("0.0.15", "0.0.15"), Some(Ordering::Equal));
    }

    #[test]
    fn treats_missing_components_as_zero_for_comparison() {
        assert_eq!(compare_versions("0.1", "0.1.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("0.1", "0.1.1"), Some(Ordering::Less));
    }

    #[test]
    fn loads_missing_version_update_record_as_none() {
        let home = unique_test_dir("msp-version-check-missing");

        assert!(
            load_version_update_record_from_home(&home)
                .unwrap()
                .is_none()
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn writes_and_reads_version_update_record() {
        let home = unique_test_dir("msp-version-check-record");
        let path = version_check_record_path_from_home(&home).unwrap();
        let record = VersionUpdateRecord {
            checked_at: 1_742_103_456_000,
            current_version: "0.0.15".into(),
            latest_version: "0.0.16".into(),
            releases_url: RELEASES_PAGE_URL.into(),
        };

        write_json_record(path, &record).unwrap();

        assert_eq!(
            load_version_update_record_from_home(&home).unwrap(),
            Some(record)
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn writes_and_reads_installed_version_record() {
        let home = unique_test_dir("msp-installed-version-record");
        let executable_path = home.join("msp");
        fs::write(&executable_path, b"bin").unwrap();

        let record = InstalledVersionRecord {
            updated_at: 1_742_103_456_000,
            latest_version: "0.0.21".into(),
            release_url: RELEASES_PAGE_URL.into(),
        };

        write_installed_version_record(&executable_path, &record).unwrap();

        assert_eq!(
            load_installed_version_record(&executable_path).unwrap(),
            Some(record)
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn restart_is_required_only_when_record_is_newer() {
        assert!(should_restart_for_installed_version("0.0.20", "0.0.21"));
        assert!(!should_restart_for_installed_version("0.0.21", "0.0.21"));
        assert!(!should_restart_for_installed_version("0.0.22", "0.0.21"));
    }

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(normalize_release_tag("0.0.20"), "v0.0.20");
        assert_eq!(normalize_release_tag("v0.0.20"), "v0.0.20");
    }
}
