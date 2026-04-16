use std::error::Error;

mod cli;
mod commands;
mod config;
mod console;
mod daemon;
mod downstream_client;
mod env_template;
mod fs_util;
mod input_popup;
mod mcp_server;
mod paths;
mod reload;
mod remote;
mod toon;
mod types;
mod version_check;

use console::print_app_error;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        print_app_error(error.as_ref());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    commands::run().await
}
