# mcp-smart-proxy

`mcp-smart-proxy` is a small Rust CLI that helps an AI work with multiple stdio MCP servers through one proxy server.

The installed binary name is `msp`.

It does two things:

1. It connects to a configured MCP server and caches its tool metadata.
2. It starts a stdio MCP server that exposes the cached toolsets through a small proxy interface.

## What It Does

The proxy server currently exposes two tools:

- `activate_toolset`: returns the cached tool list for a named toolset.
- `call_tool_in_toolset`: calls one downstream tool by toolset name and tool name.

This lets another AI inspect cached toolsets first, then call only the downstream tool it needs.

## Requirements

- Rust toolchain
- An OpenAI-compatible API key for `reload` when using the `openai` provider
- The `codex` CLI for `reload` when using the `codex` provider
- Any downstream MCP servers must use stdio transport

## Install

Install the latest released build with Homebrew:

```bash
brew install tiejunhu/mcp-smart-proxy/msp
```

After installation, run:

```bash
msp --help
```

## Build

```bash
cargo build --bin msp
```

Run the CLI during development with:

```bash
cargo run -- --help
```

## Console Output

`msp` writes structured console output so another AI or operator can distinguish application events from external command output.

- Application success output uses `=== MSP APP EVENT BEGIN ===` and `=== MSP APP EVENT END ===`.
- Application failure output uses `=== MSP APP ERROR BEGIN ===` and `=== MSP APP ERROR END ===`.
- External command stderr is tagged with `[MSP][EXTERNAL][<stage>][<label>]...`.
- External command blocks include the stage, command line, stream, and raw content.

Example success output:

```text
=== MSP APP EVENT BEGIN ===
kind: app
level: info
stage: cli.reload
message: Reloaded MCP server `github` into /Users/example/.cache/mcp-smart-proxy/github.json
=== MSP APP EVENT END ===
```

Example failure output:

```text
=== MSP APP ERROR BEGIN ===
kind: app
level: error
stage: reload.fetch_tools.list_tools
summary: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
error_chain:
- cli.reload: failed to reload MCP server `github`
- reload.fetch_tools: failed to fetch tools from MCP server `github`
- reload.fetch_tools.list_tools: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
=== MSP APP ERROR END ===
```

## Release Binaries

Pushing a tag that starts with `v` publishes release binaries automatically on GitHub Releases and updates `Formula/msp.rb` for Homebrew.

Example:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Each release includes `tar.gz` archives for:

- macOS `arm64`
- macOS `x86_64`
- Linux `arm64`
- Linux `x86_64`

## Configuration

The default config path is:

```text
~/.config/mcp-smart-proxy/config.toml
```

You can override it with `--config <PATH>`.

Example config:

```toml
default_provider = "openai"

[openai]
key = "sk-..."
model = "gpt-5.2"
# baseurl = "https://api.openai.com/v1"

[codex]
model = "gpt-5.2"

[servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
```

Notes:

- Only stdio transport is supported.
- `default_provider` is required for commands that need an AI model, such as `reload`.
- `openai.key` can also come from `OPENAI_API_KEY`.
- `openai.baseurl` can also come from `OPENAI_API_BASE`.
- If `openai.model` or `codex.model` is missing, the default is `gpt-5.2`.

## Commands

### Add a server

```bash
msp add github npx -y @modelcontextprotocol/server-github
```

This command:

1. checks that a supported `default_provider` is already configured
2. writes the server definition into the config file
3. immediately runs the same refresh flow as `reload`

If the default provider is missing, `add` fails before changing the config file.

Server names are normalized to lowercase kebab-case. For example, `GitHub Tools` becomes `github-tools`.

If the command passed to `add` is a single `http://` or `https://` URL, it is automatically converted to:

```bash
npx -y mcp-remote <URL>
```

Example:

```bash
msp add remote-demo https://example.com/mcp
```

### Configure OpenAI settings

```bash
msp config openai --key "$OPENAI_API_KEY" --model gpt-5.2
```

Optional fields:

```bash
msp config openai --baseurl https://api.openai.com/v1
msp config openai --default
```

`--default` writes `default_provider = "openai"` into the config file. Model-backed commands fail fast if `default_provider` is missing.

### Configure Codex settings

```bash
msp config codex --model gpt-5.2
```

Optional fields:

```bash
msp config codex --default
```

`codex.model` is optional and defaults to `gpt-5.2`. When `default_provider = "codex"`, model-backed commands call `codex exec` to generate the same one-sentence toolset summary used by the OpenAI provider.

### Reload cached tools

```bash
msp reload github
```

This command:

1. connects to the configured MCP server
2. fetches its tool list
3. asks the configured default provider for a one-sentence summary
4. writes the cache file

The cache is stored at:

```text
~/.cache/mcp-smart-proxy/<server-name>.json
```

`reload` requires a supported `default_provider`.

- For `openai`, configure `openai.key` or `OPENAI_API_KEY`.
- For `codex`, install the `codex` CLI; `reload` runs `codex exec`.

### Start the proxy MCP server

```bash
msp mcp
```

The server runs over stdio and loads every configured server that already has a cache file.

If a configured server has no cache yet, it is ignored until `reload` is run for that server.

## Typical Workflow

```bash
msp add github npx -y @modelcontextprotocol/server-github
msp config openai --key "$OPENAI_API_KEY" --default
msp reload github
msp mcp
```

Using Codex:

```bash
msp add github npx -y @modelcontextprotocol/server-github
msp config codex --default
msp reload github
msp mcp
```

## Proxy Tool Contract

### `activate_toolset`

Input:

```json
{
  "name": "github"
}
```

Output:

```json
{
  "tools": [
    {
      "name": "example_tool",
      "title": "Example Tool",
      "description": "Example description",
      "input_schema": {}
    }
  ]
}
```

### `call_tool_in_toolset`

Input:

```json
{
  "toolset_name": "github",
  "tool_name": "example_tool",
  "args_in_json": "{\"owner\":\"octo-org\",\"repo\":\"demo\"}"
}
```

`args_in_json` must decode to a JSON object or `null`.

## Limitations

- Only stdio downstream MCP servers are supported.
- Tool discovery depends on cached metadata produced by `reload`.
- The proxy does not dynamically list downstream tools as first-class proxy tools; it exposes a fixed activation-and-call interface instead.
