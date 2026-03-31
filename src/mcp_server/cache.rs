use std::error::Error;
use std::fs;
use std::path::Path;

use toml::{Table, Value};

use crate::config::server_is_enabled;
use crate::paths::{cache_file_path_from_home, home_dir};
use crate::types::{CachedTools, ToolSnapshot};

#[derive(Debug, Clone)]
pub(crate) struct CachedToolsetRecord {
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) tools: Vec<ToolSnapshot>,
}

pub(crate) fn load_cached_toolsets(
    config: &Table,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    load_cached_toolsets_from_home(config, &home_dir()?)
}

pub(crate) fn load_cached_toolsets_from_home(
    config: &Table,
    home: &Path,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    let Some(servers) = config.get("servers").and_then(Value::as_table) else {
        return Ok(Vec::new());
    };

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    let mut toolsets = Vec::new();
    for name in names {
        if !server_is_enabled(config, &name)? {
            continue;
        }

        let cache_path = cache_file_path_from_home(home, &name)?;
        if !cache_path.exists() {
            continue;
        }

        let cached: CachedTools = serde_json::from_str(&fs::read_to_string(cache_path)?)?;
        toolsets.push(CachedToolsetRecord {
            name,
            summary: cached.summary,
            tools: cached.tools,
        });
    }

    Ok(toolsets)
}
