use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

pub const DEFAULT_CONFIG_PATH: &str = "~/.config/mcp-smart-proxy/config.toml";
const CLI_ABOUT: &str = concat!("A smart MCP proxy ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Parser)]
#[command(
    about = CLI_ABOUT,
    disable_version_flag = true,
    arg_required_else_help = true,
    subcommand_required = true
)]
pub struct Cli {
    /// Override the config file path.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_CONFIG_PATH)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add a managed MCP server to the local config.
    Add {
        name: String,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// List configured MCP servers.
    List,
    /// Enable a configured MCP server.
    Enable { name: String },
    /// Disable a configured MCP server.
    Disable { name: String },
    /// Show or update one configured MCP server.
    Config {
        name: String,
        #[arg(long = "transport", value_enum)]
        transport: Option<ServerTransport>,
        #[arg(long = "cmd", alias = "command", value_name = "COMMAND")]
        command: Option<String>,
        #[arg(long = "arg", value_name = "ARG")]
        args: Vec<String>,
        #[arg(long)]
        clear_args: bool,
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long = "header", value_name = "KEY=VALUE")]
        headers: Vec<String>,
        #[arg(long = "unset-header", value_name = "KEY")]
        unset_headers: Vec<String>,
        #[arg(long)]
        clear_headers: bool,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        #[arg(long = "unset-env", value_name = "KEY")]
        unset_env: Vec<String>,
        #[arg(long)]
        clear_env: bool,
        #[arg(long = "env-var", value_name = "NAME")]
        env_vars: Vec<String>,
        #[arg(long = "unset-env-var", value_name = "NAME")]
        unset_env_vars: Vec<String>,
        #[arg(long)]
        clear_env_vars: bool,
    },
    /// Import MCP servers from another tool's config and refresh their cached tools.
    #[command(arg_required_else_help = true)]
    Import {
        #[arg(long, value_enum)]
        provider: Option<ProviderName>,
        source: ImportSource,
    },
    /// Install this proxy as an MCP server in another tool's config.
    Install {
        /// Import target MCP servers into msp, back them up, remove them, then install msp.
        #[arg(long)]
        replace: bool,
        target: InstallTarget,
    },
    /// Remove installed msp MCP servers from another tool's config and restore backed up MCP servers.
    Restore { target: InstallTarget },
    /// Remove a configured MCP server and its cached tools.
    Remove { name: String },
    /// Complete OAuth login for one remote MCP server.
    Login { name: String },
    /// Clear stored OAuth credentials for one remote MCP server.
    Logout { name: String },
    /// Update the running msp binary to the latest released version.
    Update,
    /// Refresh cached tool metadata for one configured MCP server, or all servers when omitted.
    Reload {
        #[arg(long, value_enum)]
        provider: Option<ProviderName>,
        name: Option<String>,
    },
    /// Start a stdio MCP server that exposes cached toolset activation.
    Mcp {
        #[arg(long, value_enum)]
        provider: Option<ProviderName>,
    },
    #[command(hide = true)]
    Daemon {
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
        #[command(subcommand)]
        command: DaemonCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum DaemonCommand {
    Run,
    Status,
    Exit,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ImportSource {
    Codex,
    Opencode,
    Claude,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum InstallTarget {
    Codex,
    Opencode,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderName {
    Codex,
    Opencode,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServerTransport {
    Stdio,
    Remote,
}

impl ServerTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Remote => "remote",
        }
    }
}

impl ProviderName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Claude => "claude",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, error::ErrorKind};

    #[test]
    fn parses_reload_without_name() {
        let cli = Cli::parse_from(["msp", "reload"]);

        match cli.command {
            Some(Command::Reload { provider, name }) => {
                assert_eq!(provider, None);
                assert_eq!(name, None);
            }
            other => panic!("expected reload command, got {other:?}"),
        }
    }

    #[test]
    fn parses_reload_with_name() {
        let cli = Cli::parse_from(["msp", "reload", "github"]);

        match cli.command {
            Some(Command::Reload { provider, name }) => {
                assert_eq!(provider, None);
                assert_eq!(name.as_deref(), Some("github"));
            }
            other => panic!("expected reload command, got {other:?}"),
        }
    }

    #[test]
    fn parses_add_command() {
        let cli = Cli::parse_from([
            "msp",
            "add",
            "github",
            "npx",
            "-y",
            "@modelcontextprotocol/server-github",
        ]);

        match cli.command {
            Some(Command::Add { name, command }) => {
                assert_eq!(name, "github");
                assert_eq!(
                    command,
                    vec![
                        "npx".to_string(),
                        "-y".to_string(),
                        "@modelcontextprotocol/server-github".to_string(),
                    ]
                );
            }
            other => panic!("expected add command, got {other:?}"),
        }
    }

    #[test]
    fn rejects_add_provider_flag() {
        let error = Cli::try_parse_from(["msp", "add", "--provider", "codex", "github", "npx"])
            .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_import_with_provider_override() {
        let cli = Cli::parse_from(["msp", "import", "--provider", "opencode", "codex"]);

        match cli.command {
            Some(Command::Import { provider, source }) => {
                assert!(matches!(provider, Some(ProviderName::Opencode)));
                assert!(matches!(source, ImportSource::Codex));
            }
            other => panic!("expected import command, got {other:?}"),
        }
    }

    #[test]
    fn parses_import_claude_source() {
        let cli = Cli::parse_from(["msp", "import", "claude"]);

        match cli.command {
            Some(Command::Import { provider, source }) => {
                assert!(provider.is_none());
                assert!(matches!(source, ImportSource::Claude));
            }
            other => panic!("expected import command, got {other:?}"),
        }
    }

    #[test]
    fn parses_install_codex_target() {
        let cli = Cli::parse_from(["msp", "install", "codex"]);

        match cli.command {
            Some(Command::Install { replace, target }) => {
                assert!(!replace);
                assert!(matches!(target, InstallTarget::Codex));
            }
            other => panic!("expected install command, got {other:?}"),
        }
    }

    #[test]
    fn parses_install_with_replace_flag() {
        let cli = Cli::parse_from(["msp", "install", "opencode", "--replace"]);

        match cli.command {
            Some(Command::Install { replace, target }) => {
                assert!(replace);
                assert!(matches!(target, InstallTarget::Opencode));
            }
            other => panic!("expected install command, got {other:?}"),
        }
    }

    #[test]
    fn parses_restore_opencode_target() {
        let cli = Cli::parse_from(["msp", "restore", "opencode"]);

        match cli.command {
            Some(Command::Restore { target }) => {
                assert!(matches!(target, InstallTarget::Opencode));
            }
            other => panic!("expected restore command, got {other:?}"),
        }
    }

    #[test]
    fn parses_install_claude_target() {
        let cli = Cli::parse_from(["msp", "install", "claude"]);

        match cli.command {
            Some(Command::Install { replace, target }) => {
                assert!(!replace);
                assert!(matches!(target, InstallTarget::Claude));
            }
            other => panic!("expected install command, got {other:?}"),
        }
    }

    #[test]
    fn parses_restore_claude_target() {
        let cli = Cli::parse_from(["msp", "restore", "claude"]);

        match cli.command {
            Some(Command::Restore { target }) => {
                assert!(matches!(target, InstallTarget::Claude));
            }
            other => panic!("expected restore command, got {other:?}"),
        }
    }

    #[test]
    fn parses_reload_with_provider_override() {
        let cli = Cli::parse_from(["msp", "reload", "--provider", "opencode", "github"]);

        match cli.command {
            Some(Command::Reload { provider, name }) => {
                assert!(matches!(provider, Some(ProviderName::Opencode)));
                assert_eq!(name.as_deref(), Some("github"));
            }
            other => panic!("expected reload command, got {other:?}"),
        }
    }

    #[test]
    fn parses_update_command() {
        let cli = Cli::parse_from(["msp", "update"]);

        match cli.command {
            Some(Command::Update) => {}
            other => panic!("expected update command, got {other:?}"),
        }
    }

    #[test]
    fn parses_mcp_with_provider_override() {
        let cli = Cli::parse_from(["msp", "mcp", "--provider", "codex"]);

        match cli.command {
            Some(Command::Mcp { provider }) => {
                assert!(matches!(provider, Some(ProviderName::Codex)));
            }
            other => panic!("expected mcp command, got {other:?}"),
        }
    }

    #[test]
    fn parses_hidden_daemon_run_subcommand() {
        let cli = Cli::parse_from(["msp", "daemon", "run"]);

        match cli.command {
            Some(Command::Daemon { socket, command }) => {
                assert!(socket.is_none());
                assert!(matches!(command, DaemonCommand::Run));
            }
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn parses_hidden_daemon_socket_override() {
        let cli = Cli::parse_from(["msp", "daemon", "--socket", "/tmp/msp.sock", "status"]);

        match cli.command {
            Some(Command::Daemon { socket, command }) => {
                assert_eq!(socket, Some(PathBuf::from("/tmp/msp.sock")));
                assert!(matches!(command, DaemonCommand::Status));
            }
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn import_without_source_shows_help() {
        let error = Cli::try_parse_from(["msp", "import"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn top_level_without_subcommand_shows_help() {
        let error = Cli::try_parse_from(["msp"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn parses_enable_server() {
        let cli = Cli::parse_from(["msp", "enable", "github"]);

        match cli.command {
            Some(Command::Enable { name }) => {
                assert_eq!(name, "github");
            }
            other => panic!("expected enable command, got {other:?}"),
        }
    }

    #[test]
    fn parses_disable_server() {
        let cli = Cli::parse_from(["msp", "disable", "server1"]);

        match cli.command {
            Some(Command::Disable { name }) => {
                assert_eq!(name, "server1");
            }
            other => panic!("expected disable command, got {other:?}"),
        }
    }

    #[test]
    fn parses_login_server() {
        let cli = Cli::parse_from(["msp", "login", "remote-demo"]);

        match cli.command {
            Some(Command::Login { name }) => assert_eq!(name, "remote-demo"),
            other => panic!("expected login command, got {other:?}"),
        }
    }

    #[test]
    fn parses_logout_server() {
        let cli = Cli::parse_from(["msp", "logout", "remote-demo"]);

        match cli.command {
            Some(Command::Logout { name }) => assert_eq!(name, "remote-demo"),
            other => panic!("expected logout command, got {other:?}"),
        }
    }

    #[test]
    fn parses_config_read_command() {
        let cli = Cli::parse_from(["msp", "config", "github"]);

        match cli.command {
            Some(Command::Config {
                name,
                transport,
                command,
                args,
                clear_args,
                url,
                enabled,
                headers,
                unset_headers,
                clear_headers,
                env,
                unset_env,
                clear_env,
                env_vars,
                unset_env_vars,
                clear_env_vars,
            }) => {
                assert_eq!(name, "github");
                assert_eq!(transport, None);
                assert_eq!(command, None);
                assert!(args.is_empty());
                assert!(!clear_args);
                assert_eq!(url, None);
                assert_eq!(enabled, None);
                assert!(headers.is_empty());
                assert!(unset_headers.is_empty());
                assert!(!clear_headers);
                assert!(env.is_empty());
                assert!(unset_env.is_empty());
                assert!(!clear_env);
                assert!(env_vars.is_empty());
                assert!(unset_env_vars.is_empty());
                assert!(!clear_env_vars);
            }
            other => panic!("expected config command, got {other:?}"),
        }
    }

    #[test]
    fn parses_config_update_command() {
        let cli = Cli::parse_from([
            "msp",
            "config",
            "github",
            "--transport",
            "stdio",
            "--cmd",
            "uvx",
            "--clear-args",
            "--arg",
            "demo-server",
            "--url",
            "https://example.com/mcp",
            "--enabled",
            "false",
            "--header",
            "Authorization=Bearer ${DEMO_TOKEN}",
            "--unset-header",
            "X-Old",
            "--clear-headers",
            "--env",
            "DEMO_REGION=global",
            "--unset-env",
            "OLD_KEY",
            "--clear-env",
            "--env-var",
            "DEMO_TOKEN",
            "--unset-env-var",
            "OLD_TOKEN",
            "--clear-env-vars",
        ]);

        match cli.command {
            Some(Command::Config {
                name,
                transport,
                command,
                args,
                clear_args,
                url,
                enabled,
                headers,
                unset_headers,
                clear_headers,
                env,
                unset_env,
                clear_env,
                env_vars,
                unset_env_vars,
                clear_env_vars,
            }) => {
                assert_eq!(name, "github");
                assert_eq!(transport, Some(ServerTransport::Stdio));
                assert_eq!(command.as_deref(), Some("uvx"));
                assert_eq!(args, vec!["demo-server".to_string()]);
                assert!(clear_args);
                assert_eq!(url.as_deref(), Some("https://example.com/mcp"));
                assert_eq!(enabled, Some(false));
                assert_eq!(
                    headers,
                    vec!["Authorization=Bearer ${DEMO_TOKEN}".to_string()]
                );
                assert_eq!(unset_headers, vec!["X-Old".to_string()]);
                assert!(clear_headers);
                assert_eq!(env, vec!["DEMO_REGION=global".to_string()]);
                assert_eq!(unset_env, vec!["OLD_KEY".to_string()]);
                assert!(clear_env);
                assert_eq!(env_vars, vec!["DEMO_TOKEN".to_string()]);
                assert_eq!(unset_env_vars, vec!["OLD_TOKEN".to_string()]);
                assert!(clear_env_vars);
            }
            other => panic!("expected config command, got {other:?}"),
        }
    }

    #[test]
    fn help_includes_version_in_about_text() {
        let mut command = Cli::command();
        let help = command.render_help().to_string();

        assert!(
            help.contains(CLI_ABOUT),
            "help did not contain `{CLI_ABOUT}`:\n{help}"
        );
    }

    #[test]
    fn help_does_not_include_version_flag() {
        let mut command = Cli::command();
        let help = command.render_help().to_string();

        assert!(
            !help.contains("--version"),
            "help unexpectedly included --version:\n{help}"
        );
        assert!(
            !help.contains("-V"),
            "help unexpectedly included -V:\n{help}"
        );
    }
}
