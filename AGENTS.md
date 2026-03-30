# First in first

- Use English for all code, comments, and documentation, to ensure that it is accessible to the widest possible audience.
- Keep all usage of this project in README.md, update it as needed, and make sure it is clear and concise.
- Before making any changes, present the plan (in the same language as user's input) to the user and ask the user for approving.
- Reduce complexity, during reading and editing the code, find complexity and try to reduce it.

# Code edit

- prefer small files, with a single responsibility, and a clear API
- when some function is not needed, don't hesitate to remove it, we can always add it back when we need it
- after editing, always run `cargo fmt` to ensure consistent code formatting, and `cargo clippy` to catch common mistakes and improve code quality, and `rust-analyzer diagnostics .` for language server diagnostics
- always make code clean, readable, and maintainable, avoid unnecessary complexity, avoid over-engineering, avoid duplicate code, and follow Rust best practices and idioms

# Documentation

- Keep README.md update to date and user facing, user frendly, and concise, and make sure it covers all the features and usage of the project.
- Doc about the code, design and decision should be updated in AGENTS.md

# packages

- rmcp, for mcp server and client
- clap, for command line parsing
- serde, for serialization and deserialization of messages
- tokio, for async runtime

# Console output

- Console output must clearly separate application output from external command output.
- External command output must include the stage, the command line, the stream (`stdout` or `stderr`), and clear start/end or block markers.
- Error output must explain which stage failed and preserve enough original external output that another AI model can diagnose the failing step from the console transcript alone.
