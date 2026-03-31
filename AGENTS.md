# First in first

- Use English for all code, comments, and documentation, to ensure that it is accessible to the widest possible audience.
- Keep all usage of this project in README.md, update it as needed, and make sure it is clear and concise.
- Before making any changes, present the plan (in the same language as user's input) to the user and ask the user for approving by giving user options: 1. Approve 2. Reject 3. <input a new plan>
- Reduce complexity, during reading and editing the code, find complexity and try to reduce it.

# Code edit

- prefer small files, with a single responsibility, and a clear API
- always make code clean, readable, and maintainable, avoid unnecessary complexity, avoid over-engineering, avoid duplicate code, and follow Rust best practices and idioms
- when some function is not needed, don't hesitate to remove it, we can always add it back when we need it
- the original code architecture and design may not be perfect, improve it when you see an opportunity, but don't try to redesign the whole codebase in one go, make incremental improvements and refactorings as you go
- after editing, always run `cargo fmt` to ensure consistent code formatting, and `cargo clippy` to catch common mistakes and improve code quality, and `rust-analyzer diagnostics .` for language server diagnostics

# Documentation

- Keep README.md update to date and user facing, user frendly, and concise, and make sure it covers all the features and usage of the project.
- Doc about the code, design and decision should be updated in AGENTS.md

## Design notes

- Keep shared import/export workflow helpers split by responsibility: provider-specific parsing stays in `src/config/import_export/<provider>.rs`, while format-specific file operations live in shared helpers under `src/config/import_export/`.
- Keep CLI orchestration thin: `src/commands.rs` should focus on top-level dispatch, while grouped command workflows such as import/install or remote auth should live in `src/commands/*.rs`.
- Keep `add` side-effect free beyond config persistence: adding a server should only write config, while provider-dependent cache refresh belongs to `reload` and `mcp` startup.
- Keep `msp mcp` on a daemon/client split: the foreground `msp mcp` process should stay a thin stdio MCP facade, while the shared daemon owns downstream MCP communication, socket lifecycle, idle shutdown, and background self-update work.
- Keep self-update logic split by concern: version comparison, state-file persistence, binary installation, and runtime orchestration should not live in a single Rust module.
- Keep local config record construction centralized: adding or importing a server should go through shared draft builders instead of duplicating transport-to-table conversion logic.
- Keep MCP proxy logic split between cache loading, tool-schema helpers, downstream client lifecycle, and request dispatch so `src/mcp_server/` remains easy to extend without re-reading one large file.

# Packages

- rmcp, for mcp server and client
- clap, for command line parsing
- serde, for serialization and deserialization of messages
- tokio, for async runtime

# Console output

- Console output must clearly separate application output from external command output.
- External command output must include the stage, the command line, the stream (`stdout` or `stderr`), and clear start/end or block markers.
- Error output must explain which stage failed and preserve enough original external output that another AI model can diagnose the failing step from the console transcript alone.
