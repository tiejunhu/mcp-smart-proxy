use std::error::Error;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use super::CURRENT_VERSION;
use super::compare::{compare_versions, should_restart_for_installed_version};
use super::install::{fetch_latest_release_version, try_auto_update};
use super::state::{
    load_installed_version_record, synchronize_current_installed_version_record,
    update_cached_version_notice,
};

pub(super) async fn run_self_update_cycle(cli_args: &[OsString]) -> Result<(), Box<dyn Error>> {
    synchronize_current_installed_version_record()?;
    restart_if_newer_installed_version_exists(cli_args)?;

    let latest_version = fetch_latest_release_version().await?;
    update_cached_version_notice(latest_version.as_deref())?;

    let Some(latest_version) = latest_version else {
        return Ok(());
    };

    if matches!(
        compare_versions(CURRENT_VERSION, &latest_version),
        Some(std::cmp::Ordering::Less)
    ) {
        let executable_path = std::env::current_exe()?;
        let _ = try_auto_update(&executable_path, &latest_version).await?;
        restart_if_newer_installed_version_exists(cli_args)?;
    }

    Ok(())
}

pub(super) fn restart_if_newer_installed_version_exists(
    cli_args: &[OsString],
) -> Result<(), Box<dyn Error>> {
    let executable_path = std::env::current_exe()?;
    let Some(record) = load_installed_version_record(&executable_path)? else {
        return Ok(());
    };

    if !should_restart_for_installed_version(CURRENT_VERSION, &record.latest_version) {
        return Ok(());
    }

    exec_current_process(&executable_path, cli_args)
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
