use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};

use crate::console::print_app_warning;
use crate::paths::unix_epoch_ms;
use crate::paths::version_check_record_path;
use crate::paths::version_check_record_path_from_home;

const RELEASES_LATEST_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases/latest";
const RELEASES_PAGE_URL: &str = "https://github.com/cybershape/mcp-smart-proxy/releases";
const VERSION_CHECK_STAGE: &str = "startup.version_check";
const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VersionUpdateRecord {
    checked_at: u128,
    current_version: String,
    latest_version: String,
    releases_url: String,
}

pub fn spawn_periodic_version_check_record_refresh() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(VERSION_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            let _ = refresh_version_check_record().await;
        }
    });
}

pub fn print_cached_update_notice() {
    let Ok(Some(record)) = load_version_update_record() else {
        return;
    };

    if !matches!(
        compare_versions(env!("CARGO_PKG_VERSION"), &record.latest_version),
        Some(Ordering::Less)
    ) {
        return;
    }

    print_app_warning(
        VERSION_CHECK_STAGE,
        format!(
            "A newer msp release is available: v{} (current: v{}). See {}",
            record.latest_version,
            env!("CARGO_PKG_VERSION"),
            record.releases_url
        ),
    );
}

async fn refresh_version_check_record() -> Result<(), Box<dyn Error>> {
    let Some(latest_version) = fetch_latest_release_version().await? else {
        return Ok(());
    };

    match compare_versions(env!("CARGO_PKG_VERSION"), &latest_version) {
        Some(Ordering::Less) => {
            write_version_update_record(&VersionUpdateRecord {
                checked_at: unix_epoch_ms()?,
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                latest_version,
                releases_url: RELEASES_PAGE_URL.to_string(),
            })?;
        }
        Some(Ordering::Equal | Ordering::Greater) => {
            delete_version_update_record()?;
        }
        None => {}
    }

    Ok(())
}

async fn fetch_latest_release_version() -> Result<Option<String>, Box<dyn Error>> {
    let client = Client::builder()
        .redirect(Policy::limited(10))
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!(
            "{}/{}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    let response = client.head(RELEASES_LATEST_URL).send().await?;
    Ok(parse_release_version_from_url(response.url()))
}

fn load_version_update_record() -> Result<Option<VersionUpdateRecord>, Box<dyn Error>> {
    load_version_update_record_from_home(&crate::paths::home_dir()?)
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
    write_version_update_record_at_path(&path, record)
}

fn write_version_update_record_at_path(
    path: &Path,
    record: &VersionUpdateRecord,
) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(record)?)?;
    Ok(())
}

fn delete_version_update_record() -> Result<(), Box<dyn Error>> {
    let path = version_check_record_path()?;
    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(path)?;
    Ok(())
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

    #[test]
    fn parses_release_version_from_latest_redirect_url() {
        let url =
            Url::parse("https://github.com/cybershape/mcp-smart-proxy/releases/tag/v0.0.16")
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
        let home =
            std::env::temp_dir().join(format!("msp-version-check-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();

        assert!(
            load_version_update_record_from_home(&home)
                .unwrap()
                .is_none()
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn writes_and_reads_version_update_record() {
        let home = std::env::temp_dir().join(format!(
            "msp-version-check-record-{}-{}",
            std::process::id(),
            unix_epoch_ms().unwrap()
        ));
        let path = version_check_record_path_from_home(&home).unwrap();
        let record = VersionUpdateRecord {
            checked_at: 1_742_103_456_000,
            current_version: "0.0.15".into(),
            latest_version: "0.0.16".into(),
            releases_url: RELEASES_PAGE_URL.into(),
        };

        write_version_update_record_at_path(&path, &record).unwrap();

        assert_eq!(
            load_version_update_record_from_home(&home).unwrap(),
            Some(record)
        );

        fs::remove_dir_all(home).unwrap();
    }
}
