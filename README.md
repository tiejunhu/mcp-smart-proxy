# MSP (MCP Smart Proxy)

`msp` is a small Rust CLI that lets an AI work with many MCP servers through one proxy server.

Instead of exposing every downstream MCP tool directly, `msp` exposes only three proxy tools. This keeps the upstream tool list small, reduces prompt noise, and avoids wasting tokens on tools the agent will never use.

The installed binary name is `msp`. Running `msp` without arguments shows the top-level help.

## Why use it

- Reduce the number of tools your agent sees (reduce the token cost without losing any tool).
- Cache downstream MCP tool metadata and summaries.
- Proxy both local stdio MCP servers and remote Streamable HTTP MCP servers.
- Reuse your existing Codex, OpenCode, or Claude Code MCP setup instead of rebuilding everything from scratch.

## How it works

`msp` does three things:

1. Connects to each configured MCP server and caches its tool metadata.
2. Generates a short summary for each server by using a configured provider: `codex`, `opencode`, or `claude`.
3. Starts a stdio MCP proxy that exposes these proxy tools:
   - `activate_additional_mcp`
   - `activate_tool_in_additional_mcp`
   - `call_tool_in_additional_mcp`
   - `request_user_input_in_popup` when started with `msp mcp --enable-input`

Agents first inspect the cached server index, optionally inspect one tool definition, and then call the downstream tool through the proxy.

When a host starts `msp mcp --provider <provider>`, `msp` auto-starts one background daemon for that config file. That daemon owns downstream MCP communication and periodic self-update checks. Later `msp mcp` processes that use the same config reuse the same Unix socket daemon, even when they pass different `--provider` values. The daemon exits after 1 hour with no requests.

The default daemon socket lives under `~/.cache/mcp-smart-proxy/` and uses a short hash of the config path so it stays within Unix socket path limits on macOS and Linux.

## Requirements

- `curl` or `wget`, plus `tar`, for installation
- The `codex` CLI when using `--provider codex`
- The `opencode` CLI when using `--provider opencode`
- The `claude` CLI when using `--provider claude`
- A browser session for remote MCP servers that require OAuth login
- A macOS desktop session when using popup input dialogs

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

The first launch starts the shared daemon automatically. Later launches for the same config reuse it.

You can inspect or control that shared process directly:

```bash
msp daemon status
msp daemon stop
msp daemon restart
```

## Common Tasks

### Add a server

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

If the command is a single `http://` or `https://` URL, `msp` stores it as a native remote server:

```bash
msp add --provider codex remote-demo https://example.com/mcp
```

`add` requires `--provider` so `msp` can summarize the fetched tools immediately. The command succeeds only when both config persistence and the initial cache refresh succeed; if cache generation fails, `msp` rolls back the new server entry instead of leaving partial config behind. You can still refresh later with `msp reload --provider ...`, or let the shared `msp mcp --provider ...` daemon refresh enabled servers in the background after startup.

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

Disabled servers stay in the config and keep their cache files, but bulk `reload` and daemon-managed `mcp` startup skip them.

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

Forward one shell variable into a remote server config:

```bash
msp config remote-demo --env-var DEMO_TOKEN
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

### Test popup input

Open a sample popup dialog locally:

```bash
msp input test
```

The MCP tool `request_user_input_in_popup` is exposed only when the host starts `msp mcp --enable-input`. On macOS it uses an embedded Swift/AppKit helper, presents questions in grouped cards with larger click targets, keeps a short header that explains the interaction model, sizes the window to its content up to a maximum height of 800 points, scrolls only the content area when that limit is exceeded, unlocks one question at a time, starts each active question with a 10-second countdown, automatically selects the first option when that countdown expires without user interaction, always appends a final `Other` option, assigns dialog-wide `1-9a-z` shortcuts in display order until the shortcut set is exhausted, lets plain shortcut keys select options for the current question while no custom field is focused, confirms `Other` with Return, returns one answer per question, and returns an empty `answers` object when the user cancels or closes the dialog.

The released `msp` binary still ships as a single executable. On macOS, `msp` extracts the embedded popup helper into `~/.cache/mcp-smart-proxy/popup-input/` on first use and reuses it for later dialogs.

Popup input dialogs are currently supported only on macOS. Linux builds do not include the popup helper, so they build cleanly in headless environments and ignore `msp mcp --enable-input`.

When building from source on macOS, popup input requires `xcrun swiftc` so Cargo can compile the helper during the build.

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

#### Unsupported Figma remote MCP server

`msp` does not support Figma's hosted MCP endpoint at `https://mcp.figma.com/mcp`.

The proxy rejects that URL during `msp add --provider ...`, `msp config --url`, and local config load with a clear error instead of letting setup continue into a broken OAuth flow.

During `msp import ...` and `msp install ... --replace`, Figma hosted MCP entries are skipped for import and left in the original host config instead of being deleted.

```bash
msp add --provider codex figma https://mcp.figma.com/mcp
```

Expected result:

```text
server `figma` uses unsupported remote MCP URL `https://mcp.figma.com/mcp`; msp does not support Figma's hosted MCP endpoint
```

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

When the daemon is running, it is also responsible for periodic background self-update checks for `msp mcp` traffic.

### Manage the shared daemon

Check whether the daemon for the current config is running:

```bash
msp daemon status
```

Stop the daemon and remove its socket and pid state:

```bash
msp daemon stop
```

Restart the daemon for the current config:

```bash
msp daemon restart
```

All three commands also accept `--socket <path>` when you need to target a custom daemon socket.

Keep custom socket paths short enough for Unix domain socket limits.

The daemon writes a runtime log next to its socket, for example `~/.cache/mcp-smart-proxy/msp-<scope>.sock.log`.

Detached daemon startup also writes `~/.cache/mcp-smart-proxy/msp-<scope>.sock.startup.log` until the new daemon answers a status probe. If startup fails or the new daemon becomes unresponsive before it can serve status, that startup log is kept for diagnosis.

If the daemon socket accepts a connection but never replies, `msp daemon status`, `stop`, and `restart` fail quickly instead of hanging indefinitely. `stop` and `restart` also fall back to force-stopping the unresponsive daemon by pid-file state so the socket can recover without manual cleanup.

Concurrent daemon refresh requests for the same provider are coalesced into one shared reload. Slow cache-lock waits, MCP tool discovery, and summary subprocesses also fail with timeouts instead of blocking the daemon forever.

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

The default daemon socket path is derived from this config path with a short stable hash, so different config files still get distinct daemons without exceeding Unix socket length limits.

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

When the proxy starts, `msp mcp --provider ...` serves the currently cached toolsets immediately and asks the shared daemon to refresh every enabled configured server in the background with the selected summary provider. The current stdio session keeps using the startup cache snapshot; refreshed cache is used by later sessions or explicit reloads. Background refresh failures are logged by the daemon and do not block MCP readiness.

## Background Self-Update

When `msp mcp` is running, it checks GitHub Releases every 30 minutes.

- If a newer build exists for the current platform, it downloads and atomically replaces the current `msp` binary.
- It writes a sibling latest-version record next to the binary.
- If the running process sees that it is older than that record, it restarts itself into the updated binary.
- Lock files prevent concurrent updates from racing on the same executable path.

Background self-update requires write access to the installed `msp` binary path.

## Proxy Tool Contract

### `activate_additional_mcp`

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

### `activate_tool_in_additional_mcp`

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

### `call_tool_in_additional_mcp`

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
