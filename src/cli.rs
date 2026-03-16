use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

pub const DEFAULT_CONFIG_PATH: &str = "~/.config/mcp-smart-proxy/config.toml";

#[derive(Debug, Parser)]
#[command(version, about = "A smart MCP proxy")]
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
    /// Import MCP servers from another tool's config and refresh their cached tools.
    Import { source: ImportSource },
    /// Refresh cached tool metadata for a configured MCP server.
    Reload { name: String },
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
    Codex {
        #[arg(long)]
        model: Option<String>,
        #[arg(long = "default")]
        make_default: bool,
    },
}
