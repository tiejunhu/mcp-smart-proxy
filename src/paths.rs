use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn expand_tilde(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path_str = path.to_string_lossy();

    if path_str == "~" {
        return home_dir();
    }

    if let Some(stripped) = path_str.strip_prefix("~/") {
        return Ok(home_dir()?.join(stripped));
    }

    Ok(path.to_path_buf())
}

pub fn home_dir() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".into())
}

pub fn format_path_for_display(path: &Path) -> String {
    match home_dir() {
        Ok(home) => format_path_for_display_from_home(&home, path),
        Err(_) => path.display().to_string(),
    }
}

pub fn cache_file_path(server_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    cache_file_path_from_home(&home_dir()?, server_name)
}

pub fn oauth_credentials_path(server_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    oauth_credentials_path_from_home(&home_dir()?, server_name)
}

pub fn cache_dir_path_from_home(home: &Path) -> Result<PathBuf, Box<dyn Error>> {
    Ok(home.join(".cache/mcp-smart-proxy"))
}

pub fn oauth_credentials_path_from_home(
    home: &Path,
    server_name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    Ok(cache_dir_path_from_home(home)?
        .join("oauth")
        .join(format!("{server_name}.json")))
}

pub fn cache_file_path_from_home(
    home: &Path,
    server_name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    Ok(cache_dir_path_from_home(home)?.join(format!("{server_name}.json")))
}

pub fn daemon_socket_path(config_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    daemon_socket_path_from_home(&home_dir()?, config_path)
}

pub fn daemon_socket_path_from_home(
    home: &Path,
    config_path: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let scope = daemon_scope_component(config_path);
    Ok(cache_dir_path_from_home(home)?
        .join("daemon")
        .join(format!("daemon-{scope}.sock")))
}

pub fn version_check_record_path() -> Result<PathBuf, Box<dyn Error>> {
    version_check_record_path_from_home(&home_dir()?)
}

pub fn version_check_record_path_from_home(home: &Path) -> Result<PathBuf, Box<dyn Error>> {
    Ok(cache_dir_path_from_home(home)?.join("version-update.json"))
}

pub fn installed_version_record_path(executable_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = executable_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "failed to derive executable file name".to_string())?;
    Ok(executable_path.with_file_name(format!("{file_name}.latest-version.json")))
}

pub fn sibling_lock_path(path: &Path) -> PathBuf {
    let mut file_name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    file_name.push(".lock");
    path.with_file_name(file_name)
}

pub fn sibling_backup_path(path: &Path, suffix: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("config");

    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) if !extension.is_empty() => {
            parent.join(format!("{stem}.{suffix}.{extension}"))
        }
        _ => parent.join(format!("{stem}.{suffix}")),
    }
}

pub fn sanitize_name(value: &str) -> String {
    let mut result = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            result.push('-');
            previous_dash = true;
        }
    }

    result.trim_matches('-').to_string()
}

pub fn daemon_scope_component(config_path: &Path) -> String {
    let normalized = config_path.to_string_lossy().to_string();
    let sanitized = sanitize_name(&normalized);
    let readable = if sanitized.is_empty() {
        "config".to_string()
    } else {
        sanitized.chars().take(48).collect::<String>()
    };
    format!("{readable}-{:016x}", fnv1a64(normalized.as_bytes()))
}

pub fn unix_epoch_ms() -> Result<u128, Box<dyn Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn format_path_for_display_from_home(home: &Path, path: &Path) -> String {
    if path == home {
        return "~".to_string();
    }

    if let Ok(relative_path) = path.strip_prefix(home) {
        if relative_path.as_os_str().is_empty() {
            return "~".to_string();
        }

        return format!("~/{}", relative_path.display());
    }

    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_non_tilde_paths() {
        let path = PathBuf::from("/tmp/config.toml");

        let expanded = expand_tilde(&path).unwrap();

        assert_eq!(expanded, path);
    }

    #[test]
    fn sanitizes_server_name() {
        assert_eq!(sanitize_name("Ones MCP"), "ones-mcp");
    }

    #[test]
    fn builds_sibling_backup_path_with_extension() {
        let path = Path::new("/tmp/config.toml");

        let backup_path = sibling_backup_path(path, "msp-backup");

        assert_eq!(backup_path, PathBuf::from("/tmp/config.msp-backup.toml"));
    }

    #[test]
    fn builds_cache_dir_path_under_default_cache_dir() {
        let home = PathBuf::from("/tmp/example-home");

        let path = cache_dir_path_from_home(&home).unwrap();

        assert_eq!(path, home.join(".cache/mcp-smart-proxy"));
    }

    #[test]
    fn builds_oauth_credentials_path_under_cache_dir() {
        let home = PathBuf::from("/tmp/example-home");

        let path = oauth_credentials_path_from_home(&home, "demo").unwrap();

        assert_eq!(path, home.join(".cache/mcp-smart-proxy/oauth/demo.json"));
    }

    #[test]
    fn builds_daemon_socket_path_from_config_scope() {
        let home = PathBuf::from("/tmp/example-home");
        let config_path = Path::new("/tmp/demo-config.toml");

        let path = daemon_socket_path_from_home(&home, config_path).unwrap();

        assert!(path.starts_with(home.join(".cache/mcp-smart-proxy/daemon")));
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("sock")
        );
    }

    #[test]
    fn builds_installed_version_record_next_to_executable() {
        let path = installed_version_record_path(Path::new("/usr/local/bin/msp")).unwrap();

        assert_eq!(
            path,
            PathBuf::from("/usr/local/bin/msp.latest-version.json")
        );
    }

    #[test]
    fn builds_sibling_lock_path() {
        let path = sibling_lock_path(Path::new("/tmp/version.json"));

        assert_eq!(path, PathBuf::from("/tmp/version.json.lock"));
    }

    #[test]
    fn formats_home_subpath_for_display() {
        let home = PathBuf::from("/Users/example");

        assert_eq!(
            format_path_for_display_from_home(&home, Path::new("/Users/example/.config/test.toml")),
            "~/.config/test.toml"
        );
    }

    #[test]
    fn formats_home_root_for_display() {
        let home = PathBuf::from("/Users/example");

        assert_eq!(format_path_for_display_from_home(&home, &home), "~");
    }

    #[test]
    fn keeps_non_home_path_for_display() {
        let home = PathBuf::from("/Users/example");

        assert_eq!(
            format_path_for_display_from_home(&home, Path::new("/tmp/test.toml")),
            "/tmp/test.toml"
        );
    }
}
