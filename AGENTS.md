# First in first

- Use English for all code, comments, and documentation, to ensure that it is accessible to the widest possible audience.
- Keep all usage of this project in README.md, update it as needed, and make sure it is clear and concise.
- Before making any changes, present the plan (in the same language as user's input) to the user and ask the user for approving.

# File edit

- prefer small files, with a single responsibility, and a clear API
- when some function is not needed, don't hesitate to remove it, we can always add it back when we need it
- refresh operations that write cache files must use a sibling `.lock` file so concurrent refreshes for the same target serialize safely

# packages

- rmcp, for mcp server and client
- clap, for command line parsing
- serde, for serialization and deserialization of messages
- tokio, for async runtime

# Console output

- Console output must clearly separate application output from external command output.
- External command output must include the stage, the command line, the stream (`stdout` or `stderr`), and clear start/end or block markers.
- Error output must explain which stage failed and preserve enough original external output that another AI model can diagnose the failing step from the console transcript alone.
- When changing console behavior, update README.md examples or usage notes in the same change.
