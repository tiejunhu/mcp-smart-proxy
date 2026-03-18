# MSP (MCP Smart Proxy)

`msp` is a small Rust CLI that helps an AI work with multiple MCP servers through one proxy server. By proxying multiple downstream MCP servers, it can significantly reduce the number of tools an Agent sees and reduce the cost and token usage wasted in unused tools.

The installed binary name is `msp`.
Running `msp` without any arguments prints the top-level command help.

It does simple things:

1. It connects to a configured MCP server and caches its tool metadata.
2. It generates a one-sentence summary of the toolset using a configured AI provider, which can be the Codex CLI or the OpenCode CLI.
3. It starts a stdio MCP server that exposes the cached toolsets through a small proxy interface.

## How it works

The proxy server currently exposes two tools:

- `activate_external_mcp`: the description of this tool contains all the MCP servers' name and the one-sentence summary of each one's toolset. Calling this tool with MCP server name as argument returns the list of tools from that downstream MCP server.
- `call_tool_in_external_mcp`: calls one downstream tool by external MCP server name and tool name.

Your Agents see only these two tools. When they want to use a tool from a MCP server, they call `activate_external_mcp` to see the list of tools. Then they can call a specific tool with `call_tool_in_external_mcp`.

## Requirements

- Homebrew for installation on macOS and Linux
- The `codex` CLI for summary using the `codex` provider
- The `opencode` CLI for summary using the `opencode` provider
- `npx` for running `mcp-remote` when adding http URLs as MCP servers
- Any downstream MCP servers must use stdio transport. If it's http transport, msp will add `npx -y mcp-remote` in front of the URL to convert it to stdio

## Install

Install the latest released build with Homebrew:

```bash
brew install tiejunhu/mcp-smart-proxy/msp
```

After installation, run:

```bash
msp
```

## Quick Start

For a really quick start, if you have codex installed and configured with some MCP servers, you can just run:

```bash
msp import codex --replace
```

This command imports all of your Codex MCP servers into `msp`, replaces all MCP servers in Codex with `msp mcp --provider codex`, and backs up your original Codex MCP server config to `~/.codex/config.msp-backup.toml`.

If you want to restore your original Codex MCP servers, run:

```bash
msp restore codex
```

After that, when you want to add a new MCP server, run:

```bash
msp add --provider codex <mcp server name> <command>
```

## Console Output

`msp` writes structured console output so another AI or operator can distinguish application events from external command output without making humans read raw log blobs.

- Application success output is a single line in the form `[MSP][INFO][stage] message`.
- Application warnings are a single line on stderr in the form `[MSP][WARN][stage] message`.
- Application failure output is printed as a short error block with the stage, summary, and numbered causes.
- Successful external commands stay silent.
- Failed external commands emit `=== MSP EXTERNAL COMMAND FAILURE BEGIN ===` and `=== MSP EXTERNAL COMMAND FAILURE END ===`.
- External output blocks are printed only for failures and include the stage, target, command line, stream, and fenced stream content markers.

Example success output:

```text
[MSP][INFO][cli.reload] Reloaded MCP server `github`. Cache file: /Users/example/.cache/mcp-smart-proxy/github.json
```

Example warning output:

```text
[MSP][WARN][startup.version_check] A newer msp release is available: v0.0.16 (current: v0.0.15). See https://github.com/tiejunhu/mcp-smart-proxy/releases
```

Example failure output:

```text
=== MSP EXTERNAL COMMAND FAILURE BEGIN ===
stage: reload.fetch_tools
target: github
command: npx -y @modelcontextprotocol/server-github
status: list-tools-failed
=== MSP EXTERNAL COMMAND FAILURE END ===
=== MSP EXTERNAL OUTPUT BEGIN ===
stage: reload.fetch_tools
target: github
command: npx -y @modelcontextprotocol/server-github
stream: stderr
----- stderr begin -----
GitHub token is missing
----- stderr end -----
=== MSP EXTERNAL OUTPUT END ===
=== MSP ERROR BEGIN ===
stage: reload.fetch_tools.list_tools
summary: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
causes:
  1. cli.reload: failed to reload MCP server `github`
  2. reload.fetch_tools: failed to fetch tools from MCP server `github`
  3. reload.fetch_tools.list_tools: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
=== MSP ERROR END ===
```

## Configuration

The default config path is:

```text
~/.config/mcp-smart-proxy/config.toml
```

You can override it with `--config <PATH>`.

Example config:

```toml
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

- The config file only stores managed `servers`.
- `add`, `reload`, and `mcp` require `--provider <codex|opencode>`.
- `import` accepts `--provider <codex|opencode>` and falls back to the current import source provider when omitted.
- `codex` uses the built-in default model `gpt-5.2`.
- `opencode` uses the built-in default model `openai/gpt-5.2`.

## Commands

### Add a server

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

This command:

1. resolves the summary provider from the required `--provider`
2. writes the server definition into the config file
3. immediately runs the same refresh flow as `reload`

`add` also rejects `msp mcp` so the proxy does not register itself as a downstream server.

Server names are normalized to lowercase kebab-case. For example, `GitHub Tools` becomes `github-tools`.

If the command passed to `add` is a single `http://` or `https://` URL, it is automatically converted to:

```bash
npx -y mcp-remote <URL>
```

Example:

```bash
msp add remote-demo https://example.com/mcp
```

### Import servers from Codex

```bash
msp import codex
```

This command:

1. reads Codex MCP servers from `$CODEX_HOME/config.toml` or `~/.codex/config.toml`
2. imports each server into the `msp` config
3. reloads every imported server immediately
4. resolves the summary provider with priority `--provider`, then the current import source provider (`codex`)

Without `--provider`, `import codex` uses the `codex` provider with the built-in default model `gpt-5.2`.
For example, `msp import --provider opencode codex` imports Codex servers but summarizes them with OpenCode.

If a Codex server name already exists in the `msp` config after normalization, that server is skipped.
If a Codex server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only Codex MCP servers defined with `command` and optional string `args` are importable. Entries that rely on other settings such as `env`, `cwd`, or non-stdio transports are rejected instead of being imported partially.

Running `msp import` without a source prints the command help instead of a missing-argument error.

### Import servers from OpenCode

```bash
msp import opencode
```

This command:

1. reads OpenCode MCP servers from `~/.config/opencode/opencode.json`
2. imports each server into the `msp` config
3. reloads every imported server immediately
4. resolves the summary provider with priority `--provider`, then the current import source provider (`opencode`)

Without `--provider`, `import opencode` uses the `opencode` provider with the built-in default model `openai/gpt-5.2`.
For example, `msp import --provider codex opencode` imports OpenCode servers but summarizes them with Codex.

If an OpenCode server name already exists in the `msp` config after normalization, that server is skipped.
If an OpenCode server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only OpenCode MCP servers defined with a string-array `command` and optional `type = "local"` are importable. Entries that rely on other settings or use non-local server types are rejected instead of being imported partially.

### Install this proxy into Codex or OpenCode

Install into Codex:

```bash
msp install codex
```

Install into OpenCode:

```bash
msp install opencode
```

Replace existing target MCP servers after importing them into `msp`:

```bash
msp install codex --replace
msp install opencode --replace
```

This command:

1. reads the target config file for Codex or OpenCode
2. checks whether that config already contains an MCP server that runs `msp mcp`
3. if that server already uses `msp mcp --provider codex` or `msp mcp --provider opencode`, reports it as already installed
4. otherwise updates the existing `msp mcp` entry to the requested provider, or creates a new entry if none exists
5. prefers the server name `msp`; if that name is already used by another server, creates `msp1`, `msp2`, and so on

`install codex` writes into `$CODEX_HOME/config.toml` or `~/.codex/config.toml`.
`install opencode` writes into `~/.config/opencode/opencode.json`.

With `--replace`, `install` performs four extra steps before the final install:

1. imports the target tool's MCP servers into `msp` and uses that import source's built-in provider
2. merges every MCP server currently present in the target config into a backup file
3. removes all MCP servers from the target config
4. installs `msp mcp --provider codex` or `msp mcp --provider opencode`

The backup files are:

- Codex: `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`
- OpenCode: `~/.config/opencode/opencode.msp-backup.json`

If a backup file already exists, `--replace` updates it in place by server name so the backup stays deduplicated.

### Restore backed up MCP servers into Codex or OpenCode

Restore into Codex:

```bash
msp restore codex
```

Restore into OpenCode:

```bash
msp restore opencode
```

This command:

1. reads the target backup file created by `msp install --replace`
2. removes every MCP server in the target config whose command is `msp mcp ...`
3. merges every backed up MCP server back into the target config by server name

`restore codex` reads from `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`.
`restore opencode` reads from `~/.config/opencode/opencode.msp-backup.json`.

If the backup file is missing, `restore` fails instead of restoring partially.

### List configured servers

```bash
msp list
```

This command reads the configured stdio MCP servers from the config file and prints each normalized server name with its configured command line.
Each configured server is emitted on its own application output line and includes the last successful cache refresh time in `YYYY-MM-DD HH:MM:SS` format.

Example:

```text
[MSP][INFO][cli.list] Configured 2 MCP server(s) in /Users/example/.config/mcp-smart-proxy/config.toml
[MSP][INFO][cli.list.server] `github`: npx -y @modelcontextprotocol/server-github (last updated: 2026-03-16 10:30:45)
[MSP][INFO][cli.list.server] `slack`: uvx slack-mcp (last updated: never)
```

### Remove a server

```bash
msp remove github
```

This command:

1. resolves the server by exact name or normalized name
2. removes the server definition from the config file
3. deletes the cached tool file at `~/.cache/mcp-smart-proxy/<server-name>.json` if it exists

### Reload cached tools

```bash
msp reload --provider codex github
```

Or reload every configured server:

```bash
msp reload --provider codex
```

This command:

1. reloads the named MCP server, or every configured server if no name is given
2. connects to each selected MCP server
3. fetches its tool list
4. compares the fetched tool list with the cached tool list using JSON string equality
5. if the tools changed, resolves the summary provider from the required `--provider` and writes the cache file

If the fetched tools match the cached tools exactly, `reload` skips the summary call and leaves the cache file unchanged.

The cache is stored at:

```text
~/.cache/mcp-smart-proxy/<server-name>.json
```

- `reload` fails if `--provider` is omitted.
- For `codex`, install the `codex` CLI; `reload` runs `codex exec`.
- For `opencode`, install the `opencode` CLI; `reload` runs `opencode run`.

### Start the proxy MCP server

```bash
msp mcp --provider codex
```

Before exposing the proxy stdio MCP server upstream, this command automatically reloads every configured MCP server.

That startup reload resolves the summary provider from the required `--provider`.

Only after that reload phase succeeds does the proxy start over stdio and load the refreshed cached toolsets. If any server fails to reload, the proxy does not report ready upstream.

While `msp mcp` is running, it checks GitHub for a newer release every 30 minutes and stores the result in `~/.cache/mcp-smart-proxy/version-update.json`.

If a newer release exists, `msp mcp` writes or updates that file. If the current binary is already up to date, `msp mcp` deletes the file.

All other `msp` commands only read that cached record on startup. If the file says a newer version is available, they print a single warning line to stderr so normal stdout output and stdio MCP traffic stay untouched.

## Typical Workflow

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
msp list
```

Using Codex:

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

Importing existing Codex MCP servers:

```bash
msp import codex
```

Importing existing OpenCode MCP servers:

```bash
msp import opencode
```

Install into Codex:

```bash
msp install codex
```

Install into OpenCode:

```bash
msp install opencode
```

Replace existing target MCP servers during install:

```bash
msp install codex --replace
msp install opencode --replace
```

Restore backed up MCP servers into Codex or OpenCode:

```bash
msp restore codex
msp restore opencode
```

## Proxy Tool Contract

### `activate_external_mcp`

Input:

```json
{
  "external_mcp_name": "github"
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

- Only stdio downstream MCP servers are supported.
- Tool discovery depends on cached metadata produced by `reload`.
- The proxy does not dynamically list downstream tools as first-class proxy tools; it exposes a fixed activation-and-call interface instead.
