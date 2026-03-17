use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn expand_tilde(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path_str = path.to_string_lossy();

    if path_str == "~" {
        return Ok(home_dir()?);
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

pub fn cache_file_path(server_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    cache_file_path_from_home(&home_dir()?, server_name)
}

pub fn cache_file_path_from_home(
    home: &Path,
    server_name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    Ok(home
        .join(".cache/mcp-smart-proxy")
        .join(format!("{server_name}.json")))
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

pub fn unix_epoch_ms() -> Result<u128, Box<dyn Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
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
}
