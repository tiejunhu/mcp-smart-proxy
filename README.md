# mcp-smart-proxy

`mcp-smart-proxy` is a small Rust CLI that helps an AI work with multiple stdio MCP servers through one proxy server.

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
- An OpenAI-compatible API key for `reload`
- Any downstream MCP servers must use stdio transport

## Build

```bash
cargo build
```

Run the CLI during development with:

```bash
cargo run -- --help
```

## Configuration

The default config path is:

```text
~/.config/mcp-smart-proxy/config.toml
```

You can override it with `--config <PATH>`.

Example config:

```toml
[openai]
key = "sk-..."
model = "gpt-5.2"
# baseurl = "https://api.openai.com/v1"

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
- `openai.key` can also come from `OPENAI_API_KEY`.
- `openai.baseurl` can also come from `OPENAI_API_BASE`.
- If `openai.model` is missing, the default is `gpt-5.2`.

## Commands

### Add a server

```bash
mcp-smart-proxy add github npx -y @modelcontextprotocol/server-github
```

This writes the server definition into the config file.

Server names are normalized to lowercase kebab-case. For example, `GitHub Tools` becomes `github-tools`.

If the command passed to `add` is a single `http://` or `https://` URL, it is automatically converted to:

```bash
npx -y mcp-remote <URL>
```

Example:

```bash
mcp-smart-proxy add remote-demo https://example.com/mcp
```

### Configure OpenAI settings

```bash
mcp-smart-proxy config openai --key "$OPENAI_API_KEY" --model gpt-5.2
```

Optional fields:

```bash
mcp-smart-proxy config openai --baseurl https://api.openai.com/v1
```

### Reload cached tools

```bash
mcp-smart-proxy reload github
```

This command:

1. connects to the configured MCP server
2. fetches its tool list
3. asks the OpenAI-compatible model for a one-sentence summary
4. writes the cache file

The cache is stored at:

```text
~/.cache/mcp-smart-proxy/<server-name>.json
```

`reload` requires an OpenAI API key from config or `OPENAI_API_KEY`.

### Start the proxy MCP server

```bash
mcp-smart-proxy mcp
```

The server runs over stdio and loads every configured server that already has a cache file.

If a configured server has no cache yet, it is ignored until `reload` is run for that server.

## Typical Workflow

```bash
mcp-smart-proxy add github npx -y @modelcontextprotocol/server-github
mcp-smart-proxy config openai --key "$OPENAI_API_KEY"
mcp-smart-proxy reload github
mcp-smart-proxy mcp
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
