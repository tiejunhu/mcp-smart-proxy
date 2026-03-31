use std::cmp::Ordering;
use std::error::Error;

use reqwest::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpdateDecision {
    UpdateRequired,
    AlreadyUpToDate,
}

pub(super) fn should_restart_for_installed_version(
    current_version: &str,
    latest_version: &str,
) -> bool {
    matches!(
        compare_versions(current_version, latest_version),
        Some(Ordering::Less)
    )
}

pub(super) fn decide_update(
    current_version: &str,
    latest_version: &str,
) -> Result<UpdateDecision, Box<dyn Error>> {
    match compare_versions(current_version, latest_version) {
        Some(Ordering::Less) => Ok(UpdateDecision::UpdateRequired),
        Some(Ordering::Equal | Ordering::Greater) => Ok(UpdateDecision::AlreadyUpToDate),
        None => Err(format!(
            "failed to compare current version `{current_version}` with latest release `{latest_version}`"
        )
        .into()),
    }
}

pub(super) fn parse_release_version_from_url(url: &Url) -> Option<String> {
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

pub(super) fn parse_release_tag(tag: &str) -> Option<String> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if version.is_empty() || !version.split('.').all(is_numeric_component) {
        return None;
    }
    Some(version.to_string())
}

pub(super) fn compare_versions(current: &str, latest: &str) -> Option<Ordering> {
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

pub(super) fn normalize_release_tag(version: &str) -> String {
    if version.starts_with('v') {
        return version.to_string();
    }

    format!("v{version}")
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
