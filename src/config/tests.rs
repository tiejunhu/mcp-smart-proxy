use super::*;
use crate::cli::DEFAULT_CONFIG_PATH;
use crate::fs_util::acquire_sibling_lock;
use crate::paths::{cache_file_path_from_home, expand_tilde, sibling_backup_path};
use crate::types::{CachedTools, ConfiguredServer, ConfiguredTransport, ModelProviderConfig};
use serde_json::Value as JsonValue;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toml::{Table, Value};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_home_env<T>(home: &Path, test: impl FnOnce() -> T) -> T {
    let _guard = env_lock().lock().unwrap();
    let previous_home = env::var("HOME").ok();

    unsafe {
        env::set_var("HOME", home);
    }

    let result = test();

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    result
}

fn with_codex_home_env<T>(codex_home: &Path, test: impl FnOnce() -> T) -> T {
    let _guard = env_lock().lock().unwrap();
    let previous_codex_home = env::var(CODEX_HOME_ENV).ok();

    unsafe {
        env::set_var(CODEX_HOME_ENV, codex_home);
    }

    let result = test();

    match previous_codex_home {
        Some(value) => unsafe { env::set_var(CODEX_HOME_ENV, value) },
        None => unsafe { env::remove_var(CODEX_HOME_ENV) },
    }

    result
}

#[test]
fn expands_default_config_path() {
    let home = PathBuf::from("/tmp/mcp-smart-proxy-home");
    unsafe {
        env::set_var("HOME", &home);
    }

    let expanded = expand_tilde(Path::new(DEFAULT_CONFIG_PATH)).unwrap();

    assert_eq!(expanded, home.join(".config/mcp-smart-proxy/config.toml"));
}

#[test]
fn parses_arbitrary_toml_content() {
    let config: Table = toml::from_str(
        r#"
                listen_addr = "127.0.0.1:8080"

                [upstream]
                url = "https://example.com/mcp"
            "#,
    )
    .unwrap();

    assert_eq!(config["listen_addr"].as_str(), Some("127.0.0.1:8080"));
    assert_eq!(
        config["upstream"]
            .as_table()
            .and_then(|table| table["url"].as_str()),
        Some("https://example.com/mcp")
    );
}

#[test]
fn resolves_codex_config_path_from_codex_home() {
    let _guard = env_lock().lock().unwrap();
    let previous_codex_home = env::var(CODEX_HOME_ENV).ok();

    unsafe {
        env::set_var(CODEX_HOME_ENV, "/tmp/codex-home");
    }

    let path = codex_config_path().unwrap();

    assert_eq!(path, PathBuf::from("/tmp/codex-home/config.toml"));

    match previous_codex_home {
        Some(value) => unsafe { env::set_var(CODEX_HOME_ENV, value) },
        None => unsafe { env::remove_var(CODEX_HOME_ENV) },
    }
}

#[test]
fn installs_codex_mcp_server_when_missing() {
    let codex_home = unique_test_path("codex-install-home");
    fs::create_dir_all(&codex_home).unwrap();

    with_codex_home_env(&codex_home, || {
        let installed = install_codex_mcp_server().unwrap();

        assert_eq!(installed.name, "msp");
        assert_eq!(installed.status, InstallMcpServerStatus::Installed);
        assert_eq!(installed.config_path, codex_home.join("config.toml"));

        let config = load_config_table(&installed.config_path).unwrap();
        let server = config["mcp_servers"]["msp"].as_table().unwrap();
        assert_eq!(server["command"].as_str(), Some("msp"));
        assert_eq!(
            server["args"].as_array().unwrap(),
            &vec![
                Value::String("mcp".to_string()),
                Value::String("--provider".to_string()),
                Value::String("codex".to_string()),
            ]
        );
    });

    fs::remove_dir_all(codex_home).unwrap();
}

#[test]
fn updates_existing_codex_self_server_to_requested_provider() {
    let codex_home = unique_test_path("codex-update-home");
    fs::create_dir_all(&codex_home).unwrap();
    let config_path = codex_home.join("config.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp", "--provider", "opencode"]
            "#,
    )
    .unwrap();

    with_codex_home_env(&codex_home, || {
        let installed = install_codex_mcp_server().unwrap();

        assert_eq!(installed.name, "proxy");
        assert_eq!(installed.status, InstallMcpServerStatus::Updated);

        let config = load_config_table(&config_path).unwrap();
        let server = config["mcp_servers"]["proxy"].as_table().unwrap();
        assert_eq!(server["command"].as_str(), Some("msp"));
        assert_eq!(
            server["args"].as_array().unwrap(),
            &vec![
                Value::String("mcp".to_string()),
                Value::String("--provider".to_string()),
                Value::String("codex".to_string()),
            ]
        );
    });

    fs::remove_dir_all(codex_home).unwrap();
}

#[test]
fn installs_codex_mcp_server_with_numbered_name_when_msp_is_taken() {
    let codex_home = unique_test_path("codex-conflict-home");
    fs::create_dir_all(&codex_home).unwrap();
    let config_path = codex_home.join("config.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.msp]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    with_codex_home_env(&codex_home, || {
        let installed = install_codex_mcp_server().unwrap();

        assert_eq!(installed.name, "msp1");
        assert_eq!(installed.status, InstallMcpServerStatus::Installed);

        let config = load_config_table(&config_path).unwrap();
        let server = config["mcp_servers"]["msp1"].as_table().unwrap();
        assert_eq!(server["command"].as_str(), Some("msp"));
    });

    fs::remove_dir_all(codex_home).unwrap();
}

#[test]
fn installs_opencode_mcp_server_when_missing() {
    let home = unique_test_path("opencode-install-home");
    fs::create_dir_all(&home).unwrap();

    with_home_env(&home, || {
        let installed = install_opencode_mcp_server().unwrap();

        assert_eq!(installed.name, "msp");
        assert_eq!(installed.status, InstallMcpServerStatus::Installed);
        assert_eq!(
            installed.config_path,
            home.join(".config/opencode/opencode.json")
        );

        let contents = fs::read_to_string(&installed.config_path).unwrap();
        let config: JsonValue = serde_json::from_str(&contents).unwrap();
        let server = config["mcp"]["msp"].as_object().unwrap();
        assert_eq!(server["type"].as_str(), Some("local"));
        assert_eq!(
            server["command"].as_array().unwrap(),
            &vec![
                JsonValue::String("msp".to_string()),
                JsonValue::String("mcp".to_string()),
                JsonValue::String("--provider".to_string()),
                JsonValue::String("opencode".to_string()),
            ]
        );
    });

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn installs_claude_mcp_server_when_missing() {
    let home = unique_test_path("claude-install-home");
    fs::create_dir_all(&home).unwrap();

    with_home_env(&home, || {
        let installed = install_claude_mcp_server().unwrap();

        assert_eq!(installed.name, "msp");
        assert_eq!(installed.status, InstallMcpServerStatus::Installed);
        assert_eq!(installed.config_path, home.join(".claude.json"));

        let contents = fs::read_to_string(&installed.config_path).unwrap();
        let config: JsonValue = serde_json::from_str(&contents).unwrap();
        let server = config["mcpServers"]["msp"].as_object().unwrap();
        assert_eq!(server["type"].as_str(), Some("stdio"));
        assert_eq!(server["command"].as_str(), Some("msp"));
        assert_eq!(
            server["args"].as_array().unwrap(),
            &vec![
                JsonValue::String("mcp".to_string()),
                JsonValue::String("--provider".to_string()),
                JsonValue::String("claude".to_string()),
            ]
        );
    });

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn replaces_codex_servers_after_merging_backup_without_duplicates() {
    let config_path = unique_test_path("codex-replace.toml");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "alpha-server"]

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
            "#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"
                [mcp_servers.beta]
                command = "old"
                args = ["beta-old"]

                [mcp_servers.gamma]
                command = "npx"
                args = ["-y", "gamma-server"]
            "#,
    )
    .unwrap();

    let replaced = replace_codex_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(replaced.config_path, config_path);
    assert_eq!(replaced.backup_path, backup_path);
    assert_eq!(replaced.backed_up_server_count, 2);
    assert_eq!(replaced.removed_server_count, 2);

    let config = load_config_table(&config_path).unwrap();
    assert!(config.get("mcp_servers").is_none());

    let backup = load_config_table(&backup_path).unwrap();
    let backup_servers = backup["mcp_servers"].as_table().unwrap();
    assert_eq!(backup_servers.len(), 3);
    assert_eq!(backup_servers["alpha"]["command"].as_str(), Some("npx"));
    assert_eq!(backup_servers["beta"]["command"].as_str(), Some("uvx"));
    assert_eq!(backup_servers["gamma"]["command"].as_str(), Some("npx"));

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn restores_codex_servers_from_backup_after_removing_self_servers() {
    let config_path = unique_test_path("codex-restore.toml");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.msp]
                command = "msp"
                args = ["mcp", "--provider", "codex"]

                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp", "--provider", "opencode"]
            "#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "alpha-server"]

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
            "#,
    )
    .unwrap();

    let restored = restore_codex_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(restored.config_path, config_path);
    assert_eq!(restored.backup_path, backup_path);
    assert_eq!(restored.removed_self_server_count, 2);
    assert_eq!(restored.restored_server_count, 2);

    let config = load_config_table(&config_path).unwrap();
    let servers = config["mcp_servers"].as_table().unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.get("msp").is_none());
    assert!(servers.get("proxy").is_none());
    assert_eq!(servers["alpha"]["command"].as_str(), Some("npx"));
    assert_eq!(servers["beta"]["command"].as_str(), Some("uvx"));

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn recognizes_existing_opencode_self_server_with_matching_provider() {
    let home = unique_test_path("opencode-existing-home");
    fs::create_dir_all(home.join(".config/opencode")).unwrap();
    let config_path = home.join(".config/opencode/opencode.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "proxy": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "opencode"]
                    }
                }
            }"#,
    )
    .unwrap();

    with_home_env(&home, || {
        let installed = install_opencode_mcp_server().unwrap();

        assert_eq!(installed.name, "proxy");
        assert_eq!(installed.status, InstallMcpServerStatus::AlreadyInstalled);
    });

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn replaces_opencode_servers_after_merging_backup_without_duplicates() {
    let config_path = unique_test_path("opencode-replace.json");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "alpha": {
                        "type": "local",
                        "command": ["npx", "-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "local",
                        "command": ["uvx", "beta-server"]
                    }
                }
            }"#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"{
                "mcp": {
                    "beta": {
                        "type": "local",
                        "command": ["old", "beta-old"]
                    },
                    "gamma": {
                        "type": "local",
                        "command": ["npx", "-y", "gamma-server"]
                    }
                }
            }"#,
    )
    .unwrap();

    let replaced = replace_opencode_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(replaced.config_path, config_path);
    assert_eq!(replaced.backup_path, backup_path);
    assert_eq!(replaced.backed_up_server_count, 2);
    assert_eq!(replaced.removed_server_count, 2);

    let config = load_opencode_config(&config_path).unwrap();
    assert!(config.get("mcp").is_none());

    let backup = load_opencode_config(&backup_path).unwrap();
    let backup_servers = backup["mcp"].as_object().unwrap();
    assert_eq!(backup_servers.len(), 3);
    assert_eq!(
        backup_servers["alpha"]["command"].as_array().unwrap(),
        &vec![
            JsonValue::String("npx".to_string()),
            JsonValue::String("-y".to_string()),
            JsonValue::String("alpha-server".to_string()),
        ]
    );
    assert_eq!(
        backup_servers["beta"]["command"].as_array().unwrap(),
        &vec![
            JsonValue::String("uvx".to_string()),
            JsonValue::String("beta-server".to_string()),
        ]
    );
    assert!(backup_servers.get("gamma").is_some());

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn restores_opencode_servers_from_backup_after_removing_self_servers() {
    let config_path = unique_test_path("opencode-restore.json");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "msp": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "opencode"]
                    },
                    "proxy": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "codex"]
                    }
                }
            }"#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"{
                "mcp": {
                    "alpha": {
                        "type": "local",
                        "command": ["npx", "-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "local",
                        "command": ["uvx", "beta-server"]
                    }
                }
            }"#,
    )
    .unwrap();

    let restored = restore_opencode_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(restored.config_path, config_path);
    assert_eq!(restored.backup_path, backup_path);
    assert_eq!(restored.removed_self_server_count, 2);
    assert_eq!(restored.restored_server_count, 2);

    let config = load_opencode_config(&config_path).unwrap();
    let servers = config["mcp"].as_object().unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.get("msp").is_none());
    assert!(servers.get("proxy").is_none());
    assert_eq!(
        servers["alpha"]["command"].as_array().unwrap(),
        &vec![
            JsonValue::String("npx".to_string()),
            JsonValue::String("-y".to_string()),
            JsonValue::String("alpha-server".to_string()),
        ]
    );
    assert_eq!(
        servers["beta"]["command"].as_array().unwrap(),
        &vec![
            JsonValue::String("uvx".to_string()),
            JsonValue::String("beta-server".to_string()),
        ]
    );

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn replaces_claude_servers_after_merging_backup_without_duplicates() {
    let config_path = unique_test_path("claude-replace.json");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "alpha": {
                        "type": "stdio",
                        "command": "npx",
                        "args": ["-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "stdio",
                        "command": "uvx",
                        "args": ["beta-server"]
                    }
                }
            }"#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"{
                "mcpServers": {
                    "beta": {
                        "type": "stdio",
                        "command": "old",
                        "args": ["beta-old"]
                    },
                    "gamma": {
                        "type": "stdio",
                        "command": "npx",
                        "args": ["-y", "gamma-server"]
                    }
                }
            }"#,
    )
    .unwrap();

    let replaced = replace_claude_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(replaced.config_path, config_path);
    assert_eq!(replaced.backup_path, backup_path);
    assert_eq!(replaced.backed_up_server_count, 2);
    assert_eq!(replaced.removed_server_count, 2);

    let config = load_claude_config(&config_path).unwrap();
    assert!(config.get("mcpServers").is_none());

    let backup = load_claude_config(&backup_path).unwrap();
    let backup_servers = backup["mcpServers"].as_object().unwrap();
    assert_eq!(backup_servers.len(), 3);
    assert_eq!(backup_servers["alpha"]["command"].as_str(), Some("npx"));
    assert_eq!(backup_servers["beta"]["command"].as_str(), Some("uvx"));
    assert!(backup_servers.get("gamma").is_some());

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn restores_claude_servers_from_backup_after_removing_self_servers() {
    let config_path = unique_test_path("claude-restore.json");
    let backup_path = sibling_backup_path(&config_path, "msp-backup");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "msp": {
                        "type": "stdio",
                        "command": "msp",
                        "args": ["mcp", "--provider", "claude"]
                    },
                    "proxy": {
                        "type": "stdio",
                        "command": "msp",
                        "args": ["mcp", "--provider", "codex"]
                    }
                }
            }"#,
    )
    .unwrap();
    fs::write(
        &backup_path,
        r#"{
                "mcpServers": {
                    "alpha": {
                        "type": "stdio",
                        "command": "npx",
                        "args": ["-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "stdio",
                        "command": "uvx",
                        "args": ["beta-server"]
                    }
                }
            }"#,
    )
    .unwrap();

    let restored = restore_claude_mcp_servers_from_path(&config_path).unwrap();

    assert_eq!(restored.config_path, config_path);
    assert_eq!(restored.backup_path, backup_path);
    assert_eq!(restored.removed_self_server_count, 2);
    assert_eq!(restored.restored_server_count, 2);

    let config = load_claude_config(&config_path).unwrap();
    let servers = config["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.get("msp").is_none());
    assert!(servers.get("proxy").is_none());
    assert_eq!(servers["alpha"]["command"].as_str(), Some("npx"));
    assert_eq!(servers["beta"]["command"].as_str(), Some("uvx"));

    fs::remove_file(config_path).unwrap();
    fs::remove_file(backup_path).unwrap();
}

#[test]
fn loads_codex_servers_for_import_from_path() {
    let config_path = unique_test_path("codex-import.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]

                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![
            ImportableServer {
                name: "alpha".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
            ImportableServer {
                name: "beta".to_string(),
                command: vec!["uvx".to_string(), "beta-server".to_string()],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
        ]
    );
    assert!(plan.skipped_self_servers.is_empty());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn preserves_codex_enabled_state_when_loading_import_plan() {
    let config_path = unique_test_path("codex-import-enabled.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
                enabled = false

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
                enabled = true
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 2);
    assert!(!plan.servers[0].enabled);
    assert!(plan.servers[1].enabled);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_codex_server_env_and_env_vars_for_import() {
    let config_path = unique_test_path("codex-import-env.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                command = "npx"
                args = ["-y", "demo-server"]
                env_vars = ["DEMO_TOKEN"]

                [mcp_servers.demo.env]
                DEMO_REGION = "global"
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(
        plan.servers[0].env.get("DEMO_REGION"),
        Some(&"global".to_string())
    );
    assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_codex_remote_server_http_headers_for_import() {
    let config_path = unique_test_path("codex-import-remote.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"

                [mcp_servers.demo.http_headers]
                Authorization = "Bearer secret"
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(plan.servers[0].command, Vec::<String>::new());
    assert_eq!(
        plan.servers[0].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        plan.servers[0].headers,
        BTreeMap::from([("Authorization".to_string(), "Bearer secret".to_string(),)])
    );
    assert!(plan.servers[0].env.is_empty());
    assert!(plan.servers[0].env_vars.is_empty());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_codex_remote_server_bearer_token_env_var_for_import() {
    let config_path = unique_test_path("codex-import-remote-bearer.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"
                bearer_token_env_var = "DEMO_TOKEN"
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(plan.servers[0].command, Vec::<String>::new());
    assert_eq!(
        plan.servers[0].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        plan.servers[0].headers,
        BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer {env:DEMO_TOKEN}".to_string(),
        )])
    );
    assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_codex_remote_server_env_http_headers_for_import() {
    let config_path = unique_test_path("codex-import-remote-env-headers.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"

                [mcp_servers.demo.env_http_headers]
                X-Workspace = "DEMO_WORKSPACE"
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(plan.servers[0].command, Vec::<String>::new());
    assert_eq!(
        plan.servers[0].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        plan.servers[0].headers,
        BTreeMap::from([(
            "X-Workspace".to_string(),
            "{env:DEMO_WORKSPACE}".to_string(),
        )])
    );
    assert_eq!(plan.servers[0].env_vars, vec!["DEMO_WORKSPACE".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_opencode_servers_for_import_from_path() {
    let config_path = unique_test_path("opencode-import.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "beta": {
                        "command": ["uvx", "beta-server"],
                        "type": "local"
                    },
                    "alpha": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local"
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![
            ImportableServer {
                name: "alpha".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
            ImportableServer {
                name: "beta".to_string(),
                command: vec!["uvx".to_string(), "beta-server".to_string()],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
        ]
    );
    assert!(plan.skipped_self_servers.is_empty());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_opencode_server_environment_for_import() {
    let config_path = unique_test_path("opencode-import-environment.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "local",
                        "environment": {
                            "DEMO_REGION": "global"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(
        plan.servers[0].env.get("DEMO_REGION"),
        Some(&"global".to_string())
    );
    assert!(plan.servers[0].env_vars.is_empty());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_opencode_remote_headers_for_import() {
    let config_path = unique_test_path("opencode-import-remote.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "demo": {
                        "type": "remote",
                        "url": "https://example.com/mcp",
                        "headers": {
                            "Authorization": "Bearer {env:DEMO_TOKEN}"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(plan.servers[0].command, Vec::<String>::new());
    assert_eq!(
        plan.servers[0].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        plan.servers[0].headers,
        BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer {env:DEMO_TOKEN}".to_string(),
        )])
    );
    assert!(plan.servers[0].env.is_empty());
    assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_claude_stdio_servers_for_import_from_path() {
    let config_path = unique_test_path("claude-import.json");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "beta": {
                        "type": "stdio",
                        "command": "uvx",
                        "args": ["beta-server"]
                    },
                    "alpha": {
                        "command": "npx",
                        "args": ["-y", "@modelcontextprotocol/server-github"],
                        "env": {
                            "GITHUB_API_URL": "https://api.github.com"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_claude_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![
            ImportableServer {
                name: "alpha".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::from([(
                    "GITHUB_API_URL".to_string(),
                    "https://api.github.com".to_string(),
                )]),
                env_vars: Vec::new(),
            },
            ImportableServer {
                name: "beta".to_string(),
                command: vec!["uvx".to_string(), "beta-server".to_string()],
                url: None,
                headers: BTreeMap::new(),
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
        ]
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_claude_remote_headers_for_import() {
    let config_path = unique_test_path("claude-import-remote.json");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "demo": {
                        "type": "http",
                        "url": "https://example.com/mcp",
                        "headers": {
                            "Authorization": "Bearer ${DEMO_TOKEN}"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_claude_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 1);
    assert_eq!(plan.servers[0].command, Vec::<String>::new());
    assert_eq!(
        plan.servers[0].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        plan.servers[0].headers,
        BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer ${DEMO_TOKEN}".to_string(),
        )])
    );
    assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn preserves_opencode_enabled_state_when_loading_import_plan() {
    let config_path = unique_test_path("opencode-import-enabled.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "alpha": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local",
                        "enabled": false
                    },
                    "beta": {
                        "command": ["uvx", "beta-server"],
                        "type": "local",
                        "enabled": true
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(plan.servers.len(), 2);
    assert!(!plan.servers[0].enabled);
    assert!(plan.servers[1].enabled);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn skips_self_server_when_loading_opencode_import_plan() {
    let config_path = unique_test_path("opencode-import-self.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "proxy": {
                        "command": ["msp", "mcp"],
                        "type": "local"
                    },
                    "github": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local"
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![ImportableServer {
            name: "github".to_string(),
            command: vec![
                "npx".to_string(),
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }]
    );
    assert_eq!(plan.skipped_self_servers, vec!["proxy".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_opencode_import_when_server_uses_unsupported_fields() {
    let config_path = unique_test_path("opencode-import-unsupported.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "local",
                        "env": {
                            "DEMO_TOKEN": "secret"
                        }
                    }
                }
            }"#,
    )
    .unwrap();

    let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "OpenCode MCP server `demo` uses unsupported settings `env`; only `command` and optional `type`, `enabled`, and `environment` can be imported"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_opencode_import_when_server_type_is_not_local() {
    let config_path = unique_test_path("opencode-import-invalid-type.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "stdio"
                    }
                }
            }"#,
    )
    .unwrap();

    let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "OpenCode MCP server `demo` uses unsupported type `stdio`, only `local` and `remote` can be imported"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_opencode_import_when_command_is_not_a_string_array() {
    let config_path = unique_test_path("opencode-import-invalid-command.json");
    fs::write(
        &config_path,
        r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", 1],
                        "type": "local"
                    }
                }
            }"#,
    )
    .unwrap();

    let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "OpenCode MCP server `demo` contains a non-string command part"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_opencode_import_when_no_servers_are_configured() {
    let config_path = unique_test_path("opencode-import-empty.json");
    fs::write(&config_path, "{}").unwrap();

    let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "no `mcp` object found in OpenCode config {}",
            config_path.display()
        )
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_claude_import_when_server_uses_unsupported_fields() {
    let config_path = unique_test_path("claude-import-unsupported.json");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "demo": {
                        "command": "npx",
                        "args": ["-y", "demo-server"],
                        "env_vars": ["DEMO_TOKEN"]
                    }
                }
            }"#,
    )
    .unwrap();

    let error = load_claude_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Claude Code MCP server `demo` uses unsupported settings `env_vars`; only `command`, optional `args`, optional `env`, and optional `type` can be imported"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn skips_self_server_when_loading_claude_import_plan() {
    let config_path = unique_test_path("claude-import-self.json");
    fs::write(
        &config_path,
        r#"{
                "mcpServers": {
                    "proxy": {
                        "type": "stdio",
                        "command": "msp",
                        "args": ["mcp"]
                    },
                    "github": {
                        "command": "npx",
                        "args": ["-y", "@modelcontextprotocol/server-github"]
                    }
                }
            }"#,
    )
    .unwrap();

    let plan = load_claude_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![ImportableServer {
            name: "github".to_string(),
            command: vec![
                "npx".to_string(),
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }]
    );
    assert_eq!(plan.skipped_self_servers, vec!["proxy".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_claude_import_when_no_servers_are_configured() {
    let config_path = unique_test_path("claude-import-empty.json");
    fs::write(&config_path, "{}").unwrap();

    let error = load_claude_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "no `mcpServers` object found in Claude Code config {}",
            config_path.display()
        )
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn skips_self_server_when_loading_codex_import_plan() {
    let config_path = unique_test_path("codex-import-self.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp"]

                [mcp_servers.github]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

    assert_eq!(
        plan.servers,
        vec![ImportableServer {
            name: "github".to_string(),
            command: vec![
                "npx".to_string(),
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }]
    );
    assert_eq!(plan.skipped_self_servers, vec!["proxy".to_string()]);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_codex_import_when_server_uses_unsupported_fields() {
    let config_path = unique_test_path("codex-import-unsupported.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                command = "npx"
                args = ["-y", "demo-server"]
                cwd = "/tmp/demo"
            "#,
    )
    .unwrap();

    let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Codex MCP server `demo` uses unsupported settings `cwd`; only `command`, `args`, optional `enabled`, `env`, `env_vars`, or remote `url` with optional `http_headers`, `bearer_token_env_var`, and `env_http_headers` can be imported"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_codex_import_when_args_is_not_an_array() {
    let config_path = unique_test_path("codex-import-invalid-args.toml");
    fs::write(
        &config_path,
        r#"
                [mcp_servers.demo]
                command = "npx"
                args = "demo-server"
            "#,
    )
    .unwrap();

    let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Codex MCP server `demo` has a non-array `args` field"
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_codex_import_when_no_servers_are_configured() {
    let config_path = unique_test_path("codex-import-empty.toml");
    fs::write(&config_path, "").unwrap();

    let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "no `mcp_servers` table found in Codex config {}",
            config_path.display()
        )
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn writes_remote_url_server_to_config() {
    let config_path = unique_test_path("write-remote-server-config.toml");
    let server_name = add_server(
        &config_path,
        "ones",
        vec!["https://ones.com/mcp".to_string()],
    )
    .unwrap();
    let config = load_config_table(&config_path).unwrap();

    let saved = config["servers"][&server_name].as_table().unwrap();
    assert!(saved.get("transport").is_none());
    assert_eq!(saved["url"].as_str(), Some("https://ones.com/mcp"));
    assert!(saved.get("command").is_none());
    assert!(saved.get("args").is_none());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn writes_imported_disabled_server_to_config() {
    let config_path = unique_test_path("write-imported-disabled-server-config.toml");
    let server_name = import_server(
        &config_path,
        &ImportableServer {
            name: "ones".to_string(),
            command: Vec::new(),
            url: Some("https://ones.com/mcp".to_string()),
            headers: BTreeMap::new(),
            enabled: false,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        },
    )
    .unwrap();
    let config = load_config_table(&config_path).unwrap();

    let saved = config["servers"][&server_name].as_table().unwrap();
    assert_eq!(saved["enabled"].as_bool(), Some(false));

    fs::remove_file(config_path).unwrap();
}

#[test]
fn writes_imported_server_env_and_env_vars_to_config() {
    let config_path = unique_test_path("write-imported-server-env-config.toml");
    let server_name = import_server(
        &config_path,
        &ImportableServer {
            name: "demo".to_string(),
            command: vec![
                "npx".to_string(),
                "-y".to_string(),
                "demo-server".to_string(),
            ],
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
            env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
            env_vars: vec!["DEMO_TOKEN".to_string()],
        },
    )
    .unwrap();
    let config = load_config_table(&config_path).unwrap();

    let saved = config["servers"][&server_name].as_table().unwrap();
    assert_eq!(saved["env"]["DEMO_REGION"].as_str(), Some("global"));
    assert_eq!(
        saved["env_vars"].as_array().unwrap(),
        &vec![Value::String("DEMO_TOKEN".to_string())]
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_server_config_snapshot() {
    let config_path = unique_test_path("load-server-config.toml");
    fs::write(
        &config_path,
        r#"
                [servers.demo]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                enabled = false
                env_vars = ["DEMO_TOKEN"]

                [servers.demo.env]
                DEMO_REGION = "global"
            "#,
    )
    .unwrap();

    let snapshot = load_server_config(&config_path, "demo").unwrap();

    assert_eq!(
        snapshot,
        ServerConfigSnapshot {
            name: "demo".to_string(),
            transport: "stdio".to_string(),
            enabled: false,
            command: Some("uvx".to_string()),
            args: vec!["demo-server".to_string()],
            url: None,
            headers: BTreeMap::new(),
            env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
            env_vars: vec!["DEMO_TOKEN".to_string()],
        }
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn loads_server_config_snapshot_without_transport_field() {
    let config_path = unique_test_path("load-server-config-without-transport.toml");
    fs::write(
        &config_path,
        r#"
                [servers.demo]
                url = "https://example.com/mcp"

                [servers.demo.headers]
                Authorization = "Bearer ${DEMO_TOKEN}"
            "#,
    )
    .unwrap();

    let snapshot = load_server_config(&config_path, "demo").unwrap();

    assert_eq!(snapshot.transport, "remote");
    assert_eq!(snapshot.url.as_deref(), Some("https://example.com/mcp"));
    assert_eq!(
        snapshot.headers,
        BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer ${DEMO_TOKEN}".to_string(),
        )])
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn updates_server_config_fields() {
    let config_path = unique_test_path("update-server-config.toml");
    fs::write(
        &config_path,
        r#"
                [servers.demo]
                transport = "stdio"
                command = "npx"
                args = ["-y", "demo-server"]
                enabled = false
                env_vars = ["OLD_TOKEN"]

                [servers.demo.env]
                OLD_REGION = "legacy"
            "#,
    )
    .unwrap();

    let updated = update_server_config(
        &config_path,
        "demo",
        &UpdateServerConfig {
            transport: Some("stdio".to_string()),
            command: Some("uvx".to_string()),
            clear_args: true,
            add_args: vec!["new-server".to_string()],
            url: None,
            enabled: Some(true),
            clear_headers: false,
            set_headers: BTreeMap::new(),
            unset_headers: Vec::new(),
            clear_env: true,
            set_env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
            unset_env: vec!["OLD_REGION".to_string()],
            clear_env_vars: true,
            add_env_vars: vec!["DEMO_TOKEN".to_string()],
            unset_env_vars: vec!["OLD_TOKEN".to_string()],
        },
    )
    .unwrap();

    assert_eq!(
        updated,
        ServerConfigSnapshot {
            name: "demo".to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            command: Some("uvx".to_string()),
            args: vec!["new-server".to_string()],
            url: None,
            headers: BTreeMap::new(),
            env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
            env_vars: vec!["DEMO_TOKEN".to_string()],
        }
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn implicit_transport_switch_removes_stale_transport_override() {
    let config_path = unique_test_path("update-server-config-remove-transport.toml");
    fs::write(
        &config_path,
        r#"
                [servers.demo]
                transport = "stdio"
                command = "npx"
                args = ["-y", "demo-server"]
            "#,
    )
    .unwrap();

    let updated = update_server_config(
        &config_path,
        "demo",
        &UpdateServerConfig {
            url: Some("https://example.com/mcp".to_string()),
            ..UpdateServerConfig::default()
        },
    )
    .unwrap();

    assert_eq!(updated.transport, "remote");
    let config = load_config_table(&config_path).unwrap();
    let saved = config["servers"]["demo"].as_table().unwrap();
    assert!(saved.get("transport").is_none());
    assert_eq!(saved["url"].as_str(), Some("https://example.com/mcp"));
    assert!(saved.get("command").is_none());

    fs::remove_file(config_path).unwrap();
}

#[test]
fn appends_server_args_and_env_vars_without_clearing() {
    let config_path = unique_test_path("append-server-config.toml");
    fs::write(
        &config_path,
        r#"
                [servers.demo]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                env_vars = ["DEMO_TOKEN"]
            "#,
    )
    .unwrap();

    let updated = update_server_config(
        &config_path,
        "demo",
        &UpdateServerConfig {
            add_args: vec!["--verbose".to_string()],
            add_env_vars: vec!["DEMO_TOKEN".to_string(), "DEMO_REGION".to_string()],
            ..UpdateServerConfig::default()
        },
    )
    .unwrap();

    assert_eq!(
        updated.args,
        vec!["demo-server".to_string(), "--verbose".to_string()]
    );
    assert_eq!(
        updated.env_vars,
        vec!["DEMO_TOKEN".to_string(), "DEMO_REGION".to_string()]
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn collects_codex_style_remote_header_env_placeholders() {
    let env_vars = collect_remote_header_value_env_vars("Bearer {env:DEMO_TOKEN}");

    assert_eq!(env_vars, vec!["DEMO_TOKEN".to_string()]);
}

#[test]
fn collects_claude_style_remote_header_env_placeholders() {
    let env_vars = collect_remote_header_value_env_vars("Bearer ${DEMO_TOKEN:-fallback}");

    assert_eq!(env_vars, vec!["DEMO_TOKEN".to_string()]);
}

#[test]
fn detects_existing_server_name_after_normalization() {
    let config: Table = toml::from_str(
        r#"
                [servers.github-tools]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    assert!(contains_server_name(&config, "GitHub Tools"));
    assert!(!contains_server_name(&config, "filesystem"));
}

#[test]
fn lists_configured_servers_sorted_by_name() {
    let config_path = unique_test_path("list-servers.toml");
    let cache_home = unique_test_path("list-servers-home");

    fs::create_dir_all(&cache_home).unwrap();
    fs::write(
        &config_path,
        r#"
                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]

                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    let servers = with_home_env(&cache_home, || list_servers(&config_path).unwrap());

    assert_eq!(
        servers,
        vec![
            ListedServer {
                name: "alpha".to_string(),
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                enabled: true,
                last_updated_at: None,
            },
            ListedServer {
                name: "beta".to_string(),
                command: "uvx".to_string(),
                args: vec!["beta-server".to_string()],
                enabled: true,
                last_updated_at: None,
            },
        ]
    );

    fs::remove_file(config_path).unwrap();
    fs::remove_dir_all(cache_home).unwrap();
}

#[test]
fn lists_cached_reload_timestamp_when_cache_exists() {
    let config_path = unique_test_path("list-servers-with-cache.toml");
    let cache_home = unique_test_path("list-servers-with-cache-home");

    fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
    fs::write(
        &config_path,
        r#"
                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    let cache_path = cache_file_path_from_home(&cache_home, "alpha").unwrap();
    fs::write(
        &cache_path,
        serde_json::to_string(&CachedTools {
            server: "alpha".to_string(),
            summary: "summary".to_string(),
            fetched_at_epoch_ms: 1_742_103_456_000,
            tools: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();

    let servers = with_home_env(&cache_home, || list_servers(&config_path).unwrap());

    assert_eq!(servers.len(), 1);
    assert!(servers[0].enabled);
    assert_eq!(servers[0].last_updated_at, Some(1_742_103_456_000));

    fs::remove_file(config_path).unwrap();
    fs::remove_dir_all(cache_home).unwrap();
}

#[test]
fn remove_server_deletes_config_entry_and_cache_file() {
    let config_path = unique_test_path("remove-server.toml");
    let cache_home = unique_test_path("remove-server-home");

    fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
    fs::write(
        &config_path,
        r#"
                [servers.github-tools]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]

                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]
            "#,
    )
    .unwrap();
    let cache_path = cache_file_path_from_home(&cache_home, "github-tools").unwrap();
    fs::write(&cache_path, "{}").unwrap();

    let removed = with_home_env(&cache_home, || {
        remove_server(&config_path, "GitHub Tools").unwrap()
    });

    assert_eq!(removed.name, "github-tools");
    assert_eq!(removed.cache_path, cache_path);
    assert!(removed.cache_deleted);
    assert!(!cache_path.exists());

    let config = load_config_table(&config_path).unwrap();
    assert!(
        config["servers"]
            .as_table()
            .unwrap()
            .get("github-tools")
            .is_none()
    );
    assert!(config["servers"].as_table().unwrap().get("beta").is_some());

    fs::remove_file(config_path).unwrap();
    fs::remove_dir_all(cache_home).unwrap();
}

#[test]
fn remove_server_drops_servers_table_when_last_entry_is_removed() {
    let config_path = unique_test_path("remove-last-server.toml");
    let cache_home = unique_test_path("remove-last-server-home");

    fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
    fs::write(
        &config_path,
        r#"
                [servers.github]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();

    let removed = with_home_env(&cache_home, || {
        remove_server(&config_path, "github").unwrap()
    });

    assert_eq!(removed.name, "github");
    assert!(!removed.cache_deleted);

    let config = load_config_table(&config_path).unwrap();
    assert!(config.get("servers").is_none());

    fs::remove_file(config_path).unwrap();
    fs::remove_dir_all(cache_home).unwrap();
}

#[test]
fn remove_server_waits_for_cache_lock_before_updating_state() {
    let _guard = env_lock().lock().unwrap();
    let previous_home = env::var("HOME").ok();
    let config_path = unique_test_path("remove-server-lock.toml");
    let cache_home = unique_test_path("remove-server-lock-home");

    unsafe {
        env::set_var("HOME", &cache_home);
    }

    fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
    fs::write(
        &config_path,
        r#"
                [servers.github]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
    )
    .unwrap();
    let cache_path = cache_file_path_from_home(&cache_home, "github").unwrap();
    fs::write(&cache_path, "{}").unwrap();

    let cache_lock = acquire_sibling_lock(&cache_path).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let config_path_for_thread = config_path.clone();
    let worker = thread::spawn(move || {
        let removed = remove_server(&config_path_for_thread, "github").unwrap();
        done_tx.send(removed).unwrap();
    });

    thread::sleep(Duration::from_millis(150));
    assert!(done_rx.try_recv().is_err());
    assert!(cache_path.exists());
    let config = load_config_table(&config_path).unwrap();
    assert!(config["servers"].as_table().unwrap().contains_key("github"));

    drop(cache_lock);

    let removed = done_rx.recv().unwrap();
    worker.join().unwrap();

    assert_eq!(removed.name, "github");
    assert!(removed.cache_deleted);
    assert!(!cache_path.exists());
    let config = load_config_table(&config_path).unwrap();
    assert!(config.get("servers").is_none());

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    fs::remove_file(config_path).unwrap();
    fs::remove_dir_all(cache_home).unwrap();
}

#[test]
fn rejects_duplicate_server_name() {
    let config_path = unique_test_path("duplicate-server-config.toml");
    add_server(
        &config_path,
        "ones",
        vec!["https://ones.com/mcp".to_string()],
    )
    .unwrap();

    let error = add_server(
        &config_path,
        "ones",
        vec!["https://example.com/mcp".to_string()],
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "server `ones` already exists");
    fs::remove_file(config_path).unwrap();
}

#[test]
fn writes_server_without_provider_configuration() {
    let config_path = unique_test_path("server-without-provider-config.toml");

    let server_name = add_server(
        &config_path,
        "ones",
        vec!["https://ones.com/mcp".to_string()],
    )
    .unwrap();
    let config = load_config_table(&config_path).unwrap();

    assert_eq!(server_name, "ones");
    assert!(config["servers"]["ones"].get("transport").is_none());
    assert_eq!(
        config["servers"]["ones"]["url"].as_str(),
        Some("https://ones.com/mcp")
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn rejects_adding_self_as_server() {
    let config_path = unique_test_path("self-server-config.toml");
    let error = add_server(
        &config_path,
        "proxy",
        vec!["msp".to_string(), "mcp".to_string()],
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "cannot add `msp mcp` as a managed server"
    );

    assert!(!config_path.exists());
}

#[test]
fn rejects_unsupported_provider_for_model_backed_runtime() {
    let error = load_model_provider_config("anthropic").unwrap_err();

    assert_eq!(
        error.to_string(),
        "unsupported provider `anthropic`; supported providers are `codex`, `opencode`, and `claude`"
    );
}

#[test]
fn loads_codex_provider_runtime_with_default_model() {
    let runtime = load_model_provider_config("codex").unwrap();

    match runtime {
        ModelProviderConfig::Codex(codex) => {
            assert_eq!(codex.model, DEFAULT_MODEL);
        }
        ModelProviderConfig::Opencode(_) => {
            panic!("expected codex provider")
        }
        ModelProviderConfig::Claude(_) => {
            panic!("expected codex provider")
        }
    }
}

#[test]
fn loads_opencode_provider_runtime_with_default_model() {
    let runtime = load_model_provider_config("opencode").unwrap();

    match runtime {
        ModelProviderConfig::Opencode(opencode) => {
            assert_eq!(opencode.model, DEFAULT_OPENCODE_MODEL);
        }
        ModelProviderConfig::Codex(_) => {
            panic!("expected opencode provider")
        }
        ModelProviderConfig::Claude(_) => {
            panic!("expected opencode provider")
        }
    }
}

#[test]
fn loads_claude_provider_runtime_with_default_model() {
    let runtime = load_model_provider_config("claude").unwrap();

    match runtime {
        ModelProviderConfig::Claude(claude) => {
            assert_eq!(claude.model, DEFAULT_CLAUDE_MODEL);
        }
        ModelProviderConfig::Codex(_) => {
            panic!("expected claude provider")
        }
        ModelProviderConfig::Opencode(_) => {
            panic!("expected claude provider")
        }
    }
}

#[test]
fn finds_server_by_exact_or_sanitized_name() {
    let config: Table = toml::from_str(
        r#"
                [servers.my-server]
                command = "uvx"
                args = ["mcp-server"]
            "#,
    )
    .unwrap();

    let (exact_name, exact_server) = configured_server(&config, "my-server").unwrap();
    assert_eq!(exact_name, "my-server");
    assert_eq!(
        exact_server,
        ConfiguredServer {
            transport: ConfiguredTransport::Stdio {
                command: "uvx".to_string(),
                args: vec!["mcp-server".to_string()],
            },
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }
    );

    let (sanitized_name, _) = configured_server(&config, "My Server").unwrap();
    assert_eq!(sanitized_name, "my-server");
}

#[test]
fn infers_remote_transport_without_transport_field() {
    let config: Table = toml::from_str(
        r#"
                [servers.remote-demo]
                url = "https://example.com/mcp"

                [servers.remote-demo.headers]
                Authorization = "Bearer ${DEMO_TOKEN}"
            "#,
    )
    .unwrap();

    let (_, server) = configured_server(&config, "remote-demo").unwrap();

    assert_eq!(
        server,
        ConfiguredServer {
            transport: ConfiguredTransport::Remote {
                url: "https://example.com/mcp".to_string(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    "Bearer ${DEMO_TOKEN}".to_string(),
                )]),
            },
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }
    );
}

#[test]
fn infers_stdio_transport_when_command_and_url_are_both_present() {
    let config: Table = toml::from_str(
        r#"
                [servers.demo]
                command = "uvx"
                args = ["demo-server"]
                url = "https://example.com/mcp"
            "#,
    )
    .unwrap();

    let (_, server) = configured_server(&config, "demo").unwrap();

    assert_eq!(
        server.stdio_transport(),
        Some(("uvx", ["demo-server".to_string()].as_slice()))
    );
    assert!(matches!(
        server.transport,
        ConfiguredTransport::Stdio {
            command,
            args
        } if command == "uvx" && args == vec!["demo-server".to_string()]
    ));
}

#[test]
fn explicit_transport_overrides_inferred_fields() {
    let config: Table = toml::from_str(
        r#"
                [servers.demo]
                transport = "remote"
                command = "uvx"
                args = ["demo-server"]
                url = "https://example.com/mcp"
            "#,
    )
    .unwrap();

    let (_, server) = configured_server(&config, "demo").unwrap();

    assert_eq!(
        server.transport,
        ConfiguredTransport::Remote {
            url: "https://example.com/mcp".to_string(),
            headers: BTreeMap::new(),
        }
    );
    assert_eq!(
        server.remote_transport(),
        Some(("https://example.com/mcp", &BTreeMap::new()))
    );
    assert!(server.stdio_transport().is_none());
}

#[test]
fn lists_disabled_servers() {
    let config_path = unique_test_path("list-disabled-servers.toml");
    fs::write(
        &config_path,
        r#"
                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
                enabled = false
            "#,
    )
    .unwrap();

    let servers = list_servers(&config_path).unwrap();

    assert_eq!(servers.len(), 1);
    assert!(!servers[0].enabled);

    fs::remove_file(config_path).unwrap();
}

#[test]
fn enables_server_by_sanitized_name() {
    let config_path = unique_test_path("enable-server.toml");
    fs::write(
        &config_path,
        r#"
                [servers.my-server]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                enabled = false
            "#,
    )
    .unwrap();

    let updated = set_server_enabled(&config_path, "My Server", true).unwrap();
    let config = load_config_table(&config_path).unwrap();

    assert_eq!(
        updated,
        SetServerEnabledResult {
            name: "my-server".to_string(),
            enabled: true,
        }
    );
    assert_eq!(
        config["servers"]["my-server"]["enabled"].as_bool(),
        Some(true)
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn disables_server_by_exact_name() {
    let config_path = unique_test_path("disable-server.toml");
    fs::write(
        &config_path,
        r#"
                [servers.server1]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
            "#,
    )
    .unwrap();

    let updated = set_server_enabled(&config_path, "server1", false).unwrap();
    let config = load_config_table(&config_path).unwrap();

    assert_eq!(
        updated,
        SetServerEnabledResult {
            name: "server1".to_string(),
            enabled: false,
        }
    );
    assert_eq!(
        config["servers"]["server1"]["enabled"].as_bool(),
        Some(false)
    );

    fs::remove_file(config_path).unwrap();
}

#[test]
fn reads_enabled_state_with_default_true() {
    let config: Table = toml::from_str(
        r#"
                [servers.alpha]
                transport = "stdio"
                command = "uvx"
                args = ["alpha-server"]

                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]
                enabled = false
            "#,
    )
    .unwrap();

    assert!(server_is_enabled(&config, "alpha").unwrap());
    assert!(!server_is_enabled(&config, "beta").unwrap());
}

#[test]
fn builds_cache_file_path_under_default_cache_dir() {
    let home = PathBuf::from("/tmp/mcp-smart-proxy-cache-home");
    let path = cache_file_path_from_home(&home, "demo-server").unwrap();

    assert_eq!(path, home.join(".cache/mcp-smart-proxy/demo-server.json"));
}

fn unique_test_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    env::temp_dir().join(format!("mcp-smart-proxy-{unique}-{name}"))
}
