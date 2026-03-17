use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

pub const DEFAULT_CONFIG_PATH: &str = "~/.config/mcp-smart-proxy/config.toml";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "A smart MCP proxy",
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
    /// Add a stdio MCP server and refresh its cached tools.
    Add {
        #[arg(long, value_enum)]
        provider: Option<ProviderName>,
        name: String,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// List configured stdio MCP servers.
    List,
    /// Import MCP servers from another tool's config and refresh their cached tools.
    #[command(arg_required_else_help = true)]
    Import {
        #[arg(long, value_enum)]
        provider: Option<ProviderName>,
        source: ImportSource,
    },
    /// Install this proxy as an MCP server in another tool's config.
    Install { target: InstallTarget },
    /// Remove a configured MCP server and its cached tools.
    Remove { name: String },
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
    /// Update application configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ImportSource {
    Codex,
    Opencode,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum InstallTarget {
    Codex,
    Opencode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderName {
    Openai,
    Codex,
    Opencode,
}

impl ProviderName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Update OpenAI settings.
    #[command(arg_required_else_help = true)]
    Openai {
        #[arg(long)]
        baseurl: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long = "default")]
        make_default: bool,
    },
    /// Update Codex settings.
    #[command(arg_required_else_help = true)]
    Codex {
        #[arg(long)]
        model: Option<String>,
        #[arg(long = "default")]
        make_default: bool,
    },
    /// Update OpenCode settings.
    #[command(arg_required_else_help = true)]
    Opencode {
        #[arg(long)]
        model: Option<String>,
        #[arg(long = "default")]
        make_default: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

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
    fn parses_add_with_provider_override() {
        let cli = Cli::parse_from([
            "msp",
            "add",
            "--provider",
            "codex",
            "github",
            "npx",
            "-y",
            "@modelcontextprotocol/server-github",
        ]);

        match cli.command {
            Some(Command::Add {
                provider,
                name,
                command,
            }) => {
                assert!(matches!(provider, Some(ProviderName::Codex)));
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
    fn parses_import_with_provider_override() {
        let cli = Cli::parse_from(["msp", "import", "--provider", "openai", "codex"]);

        match cli.command {
            Some(Command::Import { provider, source }) => {
                assert!(matches!(provider, Some(ProviderName::Openai)));
                assert!(matches!(source, ImportSource::Codex));
            }
            other => panic!("expected import command, got {other:?}"),
        }
    }

    #[test]
    fn parses_install_codex_target() {
        let cli = Cli::parse_from(["msp", "install", "codex"]);

        match cli.command {
            Some(Command::Install { target }) => {
                assert!(matches!(target, InstallTarget::Codex));
            }
            other => panic!("expected install command, got {other:?}"),
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
    fn config_openai_without_flags_shows_help() {
        let error = Cli::try_parse_from(["msp", "config", "openai"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn config_codex_without_flags_shows_help() {
        let error = Cli::try_parse_from(["msp", "config", "codex"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn config_opencode_without_flags_shows_help() {
        let error = Cli::try_parse_from(["msp", "config", "opencode"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
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
}
