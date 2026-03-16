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
        name: String,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// List configured stdio MCP servers.
    List,
    /// Import MCP servers from another tool's config and refresh their cached tools.
    Import { source: ImportSource },
    /// Remove a configured MCP server and its cached tools.
    Remove { name: String },
    /// Refresh cached tool metadata for one configured MCP server, or all servers when omitted.
    Reload { name: Option<String> },
    /// Start a stdio MCP server that exposes cached toolset activation.
    Mcp,
    /// Update application configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ImportSource {
    Codex,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn parses_reload_without_name() {
        let cli = Cli::parse_from(["msp", "reload"]);

        match cli.command {
            Some(Command::Reload { name }) => assert_eq!(name, None),
            other => panic!("expected reload command, got {other:?}"),
        }
    }

    #[test]
    fn parses_reload_with_name() {
        let cli = Cli::parse_from(["msp", "reload", "github"]);

        match cli.command {
            Some(Command::Reload { name }) => assert_eq!(name.as_deref(), Some("github")),
            other => panic!("expected reload command, got {other:?}"),
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
    fn top_level_without_subcommand_shows_help() {
        let error = Cli::try_parse_from(["msp"]).unwrap_err();

        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
