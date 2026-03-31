use std::cmp::Ordering;
use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::console::print_app_warning;
#[cfg(test)]
use crate::paths::{unix_epoch_ms, version_check_record_path_from_home};

mod compare;
mod install;
mod runtime;
mod state;

use compare::{compare_versions, decide_update};
use install::{fetch_latest_release_version, try_auto_update};
use runtime::{restart_if_newer_installed_version_exists, run_self_update_cycle};
#[cfg(test)]
use state::{
    VersionUpdateRecord, load_installed_version_record, load_version_update_record_from_home,
    write_installed_version_record, write_json_record,
};
use state::{
    delete_version_update_record, load_version_update_record,
    synchronize_current_installed_version_record, update_cached_version_notice,
};

const BINARY_NAME: &str = "msp";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_LATEST_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases/latest";
const RELEASES_PAGE_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases";
const VERSION_CHECK_STAGE: &str = "startup.version_check";
const SELF_UPDATE_STAGE: &str = "startup.self_update";
const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualUpdateResult {
    pub executable_path: PathBuf,
    pub latest_version: String,
    pub updated: bool,
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

pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

pub async fn run_manual_self_update() -> Result<ManualUpdateResult, Box<dyn Error>> {
    synchronize_current_installed_version_record()?;

    let executable_path = std::env::current_exe()?;
    let latest_version = fetch_latest_release_version()
        .await?
        .ok_or_else(|| "failed to resolve the latest release version".to_string())?;

    match decide_update(CURRENT_VERSION, &latest_version)? {
        compare::UpdateDecision::UpdateRequired => {
            update_cached_version_notice(Some(&latest_version))?;
            let updated = try_auto_update(&executable_path, &latest_version).await?;
            delete_version_update_record()?;
            Ok(ManualUpdateResult {
                executable_path,
                latest_version,
                updated,
            })
        }
        compare::UpdateDecision::AlreadyUpToDate => {
            delete_version_update_record()?;
            Ok(ManualUpdateResult {
                executable_path,
                latest_version,
                updated: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::compare::{
        UpdateDecision, compare_versions, decide_update, normalize_release_tag, parse_release_tag,
        parse_release_version_from_url, should_restart_for_installed_version,
    };
    use super::state::InstalledVersionRecord;
    use super::*;
    use reqwest::Url;
    use std::fs;

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

    #[test]
    fn decides_when_manual_update_is_required() {
        assert_eq!(
            decide_update("0.0.22", "0.0.23").unwrap(),
            UpdateDecision::UpdateRequired
        );
    }

    #[test]
    fn decides_when_manual_update_is_not_required() {
        assert_eq!(
            decide_update("0.0.23", "0.0.23").unwrap(),
            UpdateDecision::AlreadyUpToDate
        );
        assert_eq!(
            decide_update("0.0.24", "0.0.23").unwrap(),
            UpdateDecision::AlreadyUpToDate
        );
    }
}
