# MSP (MCP Smart Proxy)

`msp` is a small Rust CLI that lets an AI work with many MCP servers through one proxy server.

Instead of exposing every downstream MCP tool directly, `msp` exposes only three proxy tools. This keeps the upstream tool list small, reduces prompt noise, and avoids wasting tokens on tools the agent will never use.

The installed binary name is `msp`. Running `msp` without arguments shows the top-level help.

## Why use it

- Reduce the number of tools your agent sees.
- Cache downstream MCP tool metadata and summaries.
- Proxy both local stdio MCP servers and remote Streamable HTTP MCP servers.
- Reuse your existing Codex, OpenCode, or Claude Code MCP setup instead of rebuilding everything from scratch.

## How it works

`msp` does three things:

1. Connects to each configured MCP server and caches its tool metadata.
2. Generates a short summary for each server by using a configured provider: `codex`, `opencode`, or `claude`.
3. Starts a stdio MCP proxy that exposes only these three tools:
   - `activate_external_mcp`
   - `activate_external_mcp_tool`
   - `call_tool_in_external_mcp`

Agents first inspect the cached server index, optionally inspect one tool definition, and then call the downstream tool through the proxy.

## Requirements

- `curl` or `wget`, plus `tar`, for installation
- The `codex` CLI when using `--provider codex`
- The `opencode` CLI when using `--provider opencode`
- The `claude` CLI when using `--provider claude`
- A browser session for remote MCP servers that require OAuth login

## Install

Install the latest release:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | bash
```

Install to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | INSTALL_DIR=/tmp/msp/bin bash
```

Install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | VERSION=v0.0.19 bash
```

By default the installer writes `msp` to:

- macOS on Apple Silicon: `/opt/homebrew/bin`
- macOS on Intel: `/usr/local/bin`
- Linux as root: `/usr/local/bin`
- Linux as a regular user: `~/.local/bin`

After installation:

```bash
msp
```

## Quick Start

### Fastest path for Codex users

Import your existing Codex MCP servers into `msp`, replace Codex's MCP entries with the proxy, and keep a backup:

```bash
msp import codex --replace
```

If you want to restore the original Codex MCP servers later:

```bash
msp restore codex
```

To add a new server after that:

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

### Fastest path for OpenCode or Claude Code

Import existing servers:

```bash
msp import opencode
msp import claude
```

Install the proxy into the host:

```bash
msp install opencode
msp install claude
```

Replace existing host MCP entries and keep a backup:

```bash
msp install opencode --replace
msp install claude --replace
```

Restore the original host config later if needed:

```bash
msp restore opencode
msp restore claude
```

### Start from scratch

Add a server:

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

Install `msp` into your host:

```bash
msp install codex
```

From that point, the host launches `msp mcp --provider <provider>` as its MCP server entrypoint.

## Common Tasks

### Add a server

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

If the command is a single `http://` or `https://` URL, `msp` stores it as a native remote server:

```bash
msp add remote-demo https://example.com/mcp
```

`add` immediately refreshes the new server. If refresh fails, the new config entry is rolled back.

### List servers

```bash
msp list
```

`msp list` shows each configured server, whether it is enabled, and when its cache was last refreshed.

### Enable or disable a server

```bash
msp disable github
msp enable github
```

Disabled servers stay in the config and keep their cache files, but bulk `reload` and `mcp` startup skip them.

### Show or update one server

Show current config:

```bash
msp config github
```

Update a stdio server:

```bash
msp config github --cmd uvx --clear-args --arg demo-server --env DEMO_REGION=global --env-var DEMO_TOKEN --enabled false
```

Update a remote server:

```bash
msp config remote-demo --url https://example.com/mcp --clear-headers --header Authorization='Bearer ${DEMO_TOKEN}'
```

`msp config` can update `transport`, `enabled`, command or URL fields, headers, static env values, and forwarded env var names.

### Reload cached tools

Reload one server:

```bash
msp reload --provider codex github
```

Reload every enabled server:

```bash
msp reload --provider codex
```

`reload` fetches the downstream tool list, compares it to the cache, and only regenerates the summary when the tool list changed.

### Log in or out of a remote server

Start OAuth login:

```bash
msp login remote-demo
```

Clear cached OAuth credentials:

```bash
msp logout remote-demo
```

OAuth metadata is discovered from the remote MCP server at runtime. Credentials are cached under `~/.cache/mcp-smart-proxy/oauth/`.

### Remove a server

```bash
msp remove github
```

This removes the server from the config and deletes its cached tool file if one exists.

### Update `msp` itself

```bash
msp update
```

This checks GitHub Releases, downloads the newest build for the current platform, and replaces the current executable in place when a newer release exists.

## Install Into a Host

Install the proxy into Codex, OpenCode, or Claude Code:

```bash
msp install codex
msp install opencode
msp install claude
```

With `--replace`, `msp` first imports the host's current MCP servers into `msp`, backs them up, removes them from the host config, and then installs the proxy:

```bash
msp install codex --replace
msp install opencode --replace
msp install claude --replace
```

Backup files:

- Codex: `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`
- OpenCode: `~/.config/opencode/opencode.msp-backup.json`
- Claude Code: `~/.claude.msp-backup.json`

Restore from backup:

```bash
msp restore codex
msp restore opencode
msp restore claude
```

## Import Existing Servers

`msp` can import MCP servers from:

- Codex: `msp import codex`
- OpenCode: `msp import opencode`
- Claude Code: `msp import claude`

Provider selection works like this:

- `import codex` defaults to provider `codex`
- `import opencode` defaults to provider `opencode`
- `import claude` defaults to provider `claude`
- `--provider ...` overrides the default summary provider

Examples:

```bash
msp import codex
msp import --provider opencode codex
msp import opencode
msp import claude
```

Import behavior:

- Existing names are skipped after normalization.
- Entries that launch `msp mcp` are skipped.
- Only supported MCP config shapes are imported.
- If refresh fails during the import batch, `msp` rolls back the servers added in that run.

## Configuration

Default config path:

```text
~/.config/mcp-smart-proxy/config.toml
```

Override it with `--config <PATH>`.

Example:

```toml
[servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
enabled = true
env_vars = ["GITHUB_TOKEN"]

[servers.github.env]
GITHUB_API_URL = "https://api.github.com"
GITHUB_ENTERPRISE_MODE = "false"

[servers.test]
transport = "remote"
url = "https://example.com/mcp"
enabled = false
env_vars = ["DEMO_TOKEN", "DEMO_REGION"]

[servers.test.headers]
Authorization = "Bearer ${DEMO_TOKEN}"
X-Region = "${DEMO_REGION:-global}"

[servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
```

Notes:

- The config file stores managed `servers` only.
- Servers are enabled by default unless `enabled = false`.
- `transport` is optional. If omitted, `msp` infers it from `command` or `url`.
- `stdio` servers use `command` plus `args`.
- `remote` servers use a Streamable HTTP `url` plus optional `headers`.
- `env` stores static variables for the downstream server.
- `env_vars` lists variables that `msp` forwards from its own environment.

## Run the Proxy

```bash
msp mcp --provider codex
```

`msp mcp` is a stdio MCP server entrypoint, not an interactive shell command. Start it from an MCP host such as Codex, OpenCode, or Claude Code, or install it with `msp install ...`.

Before the proxy starts, `msp mcp` reloads every enabled configured server. If any reload fails, the proxy does not report ready upstream.

## Background Self-Update

When `msp mcp` is running, it checks GitHub Releases every 30 minutes.

- If a newer build exists for the current platform, it downloads and atomically replaces the current `msp` binary.
- It writes a sibling latest-version record next to the binary.
- If the running process sees that it is older than that record, it restarts itself into the updated binary.
- Lock files prevent concurrent updates from racing on the same executable path.

Background self-update requires write access to the installed `msp` binary path.

## Proxy Tool Contract

### `activate_external_mcp`

Input:

```json
{
  "external_mcp_name": "github"
}
```

Output:

```text
example_tool: Example description
another_tool: Another description that is longer but still fits in the preview
```

Each output line is `tool_name: description-preview`. If a tool has no description, the line is just `tool_name`.

### `activate_external_mcp_tool`

Input:

```json
{
  "external_mcp_name": "github",
  "tool_name": "example_tool"
}
```

Output:

```json
{
  "tool": {
    "name": "example_tool",
    "title": "Example Tool",
    "description": "Example description",
    "input_schema": {}
  }
}
```

### `call_tool_in_external_mcp`

Input:

```json
{
  "external_mcp_name": "github",
  "tool_name": "example_tool",
  "args_in_json": "{\"owner\":\"octo-org\",\"repo\":\"demo\"}"
}
```

`args_in_json` must decode to a JSON object or `null`.

## Limitations

- Downstream MCP servers must use either stdio or Streamable HTTP transport.
- Remote OAuth currently assumes an interactive browser-based authorization code flow.
- Tool discovery depends on metadata cached by `reload`.
- The proxy exposes a fixed activation-and-call interface instead of dynamically re-exporting downstream tools as first-class proxy tools.
