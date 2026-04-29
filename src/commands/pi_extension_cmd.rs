use std::error::Error;

use crate::console::{message_error, operation_error, print_app_event, print_app_warning};
use crate::paths::format_path_for_display;
use crate::pi_extension::{
    PiExtensionInstallStatus, install_global_pi_extension, restore_global_pi_extension,
    synchronize_global_pi_extension_if_installed,
};

const INSTALL_STAGE: &str = "cli.install.pi";
const RESTORE_STAGE: &str = "cli.restore.pi";
const SYNC_STAGE: &str = "startup.pi_extension";

pub(super) fn run_install_pi_command(replace: bool) -> Result<(), Box<dyn Error>> {
    if replace {
        return Err(message_error(
            INSTALL_STAGE,
            "`--replace` is not supported for `msp install pi`",
        ));
    }

    let installed = install_global_pi_extension().map_err(|error| {
        operation_error(
            INSTALL_STAGE,
            "failed to install the bundled pi extension",
            error,
        )
    })?;
    let verb = match installed.status {
        PiExtensionInstallStatus::Installed => "Installed",
        PiExtensionInstallStatus::Updated => "Updated",
    };

    print_app_event(
        INSTALL_STAGE,
        format!(
            "{verb} bundled pi extension at {}",
            format_path_for_display(&installed.path)
        ),
    );
    Ok(())
}

pub(super) fn run_restore_pi_command() -> Result<(), Box<dyn Error>> {
    let restored = restore_global_pi_extension().map_err(|error| {
        operation_error(
            RESTORE_STAGE,
            "failed to remove the bundled pi extension",
            error,
        )
    })?;
    if restored.removed {
        print_app_event(
            RESTORE_STAGE,
            format!(
                "Removed bundled pi extension from {}",
                format_path_for_display(&restored.path)
            ),
        );
    } else {
        print_app_event(
            RESTORE_STAGE,
            format!(
                "Bundled pi extension not found at {}",
                format_path_for_display(&restored.path)
            ),
        );
    }
    Ok(())
}

pub(super) fn synchronize_installed_pi_extension_on_startup() {
    if let Err(error) = synchronize_global_pi_extension_if_installed() {
        print_app_warning(
            SYNC_STAGE,
            format!("Failed to update the installed bundled pi extension automatically: {error}"),
        );
    }
}
