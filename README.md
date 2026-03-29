# MSP (MCP Smart Proxy)

`msp` is a small Rust CLI that helps an AI work with multiple MCP servers through one proxy server. By proxying multiple downstream MCP servers, it can significantly reduce the number of tools an Agent sees and reduce the cost and token usage wasted in unused tools.

The installed binary name is `msp`.
Running `msp` without any arguments prints the top-level command help.

It does simple things:

1. It connects to a configured MCP server and caches its tool metadata.
2. It generates a one-sentence summary of the toolset using a configured AI provider, which can be the Codex CLI or the OpenCode CLI.
3. It starts a stdio MCP server that exposes the cached toolsets through a small proxy interface.

## How it works

The proxy server currently exposes three tools:

- `activate_external_mcp`: the description of this tool contains all the MCP servers' name and the one-sentence summary of each one's toolset. Calling this tool with MCP server name as argument returns a plain-text list of downstream tool names and description previews.
- `activate_external_mcp_tool`: returns the full cached definition for one downstream tool by MCP server name and tool name.
- `call_tool_in_external_mcp`: calls one downstream tool by external MCP server name and tool name.

Your Agents see only these three tools. When they want to use a tool from a MCP server, they call `activate_external_mcp` to see the cached tool index, optionally call `activate_external_mcp_tool` to inspect one full tool definition, and then call a specific tool with `call_tool_in_external_mcp`.

## Requirements

- Homebrew, or `curl`/`wget` plus `tar`, for installation
- The `codex` CLI for summary using the `codex` provider
- The `opencode` CLI for summary using the `opencode` provider
- `npx` for running `mcp-remote` when adding http URLs as MCP servers
- Any downstream MCP servers must use stdio transport. If it's http transport, msp will add `npx -y mcp-remote` in front of the URL to convert it to stdio

## Install

Install the latest released build with Homebrew:

```bash
brew install cybershape/mcp-smart-proxy/msp
```

Or install the latest release for the current platform with the repository installer:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | bash
```

The installer resolves the latest version through the GitHub Releases redirect path instead of the GitHub REST API, which avoids unauthenticated `api.github.com` rate limits.

By default the installer writes `msp` to `/opt/homebrew/bin` when that directory exists and is writable, then falls back to `/usr/local/bin`, and finally to `~/.local/bin`.

Install to a custom location instead:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | INSTALL_DIR=/tmp/msp/bin bash
```

Install a specific released version instead of the latest one:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | VERSION=v0.0.19 bash
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

- Application success output is a single message line without a log prefix.
- Application warnings are a single line on stderr prefixed with `warning:`.
- Application failure output is printed as a labeled `application error:` block with the stage, summary, and numbered causes.
- Successful external commands stay silent.
- Failed external commands emit a labeled `external command failure:` block.
- External output blocks are printed only for failures and use a labeled `external command output:` block with the stage, target, command line, stream, and fenced stream content markers.

Example success output:

```text
Reloaded MCP server `github`. Cache file: /Users/example/.cache/mcp-smart-proxy/github.json
```

Example warning output:

```text
warning: A newer msp release is available: v0.0.16 (current: v0.0.15). See https://github.com/cybershape/mcp-smart-proxy/releases
```

Example failure output:

```text
external command failure:
stage: reload.fetch_tools
target: github
command: npx -y @modelcontextprotocol/server-github
status: list-tools-failed
external command output:
stage: reload.fetch_tools
target: github
command: npx -y @modelcontextprotocol/server-github
stream: stderr
----- stderr begin -----
GitHub token is missing
----- stderr end -----
application error:
stage: reload.fetch_tools.list_tools
summary: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
causes:
  1. cli.reload: failed to reload MCP server `github`
  2. reload.fetch_tools: failed to fetch tools from MCP server `github`
  3. reload.fetch_tools.list_tools: failed to list tools from external command `npx -y @modelcontextprotocol/server-github`
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
env_vars = ["GITHUB_TOKEN"]

[servers.github.env]
GITHUB_API_URL = "https://api.github.com"

[servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[servers.legacy-demo]
transport = "stdio"
command = "uvx"
args = ["legacy-demo-server"]
enabled = false
```

Notes:

- The config file only stores managed `servers`.
- Each server is enabled by default. Set `enabled = false` or run `msp disable <name>` to keep it configured but exclude it from `msp mcp` activation and bulk reload.
- `env` stores static environment variables for the downstream MCP server, while `env_vars` lists variable names that `msp` forwards from its own process environment when it starts that server.
- `msp config <name>` shows one managed server's current `transport`, `enabled`, `command`, `args`, `env`, and `env_vars` values, and can also update them in place.
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
3. immediately runs the same refresh flow as `reload`, and rolls the new server back out of the config if refresh fails

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

### Enable or disable a server

Disable a server:

```bash
msp disable server1
```

Enable it again:

```bash
msp enable server1
```

These commands resolve the server by exact or normalized name and update its `enabled` flag in the config file.
Disabled servers stay in the config and keep their cache files, but `msp reload --provider ...` without a name and `msp mcp --provider ...` skip them.
An explicit `msp reload --provider ... <name>` still works for a disabled server.

### Show or update a server config

Show one server's current config:

```bash
msp config github
```

Update fields in place:

```bash
msp config github --cmd uvx --clear-args --arg demo-server --env DEMO_REGION=global --env-var DEMO_TOKEN --enabled false
```

This command:

1. resolves the server by exact or normalized name
2. prints the current `transport`, `enabled`, `command`, `args`, `env`, and `env_vars` values when no update flags are passed
3. updates `command` with `--cmd`
4. appends `--arg` values to the current args list, or replaces the list when combined with `--clear-args`
5. adds or replaces static environment variables with `--env KEY=VALUE`, removes specific keys with `--unset-env KEY`, and clears the whole table with `--clear-env`
6. adds forwarded environment variable names with `--env-var NAME`, removes specific names with `--unset-env-var NAME`, and clears the whole list with `--clear-env-vars`
7. updates `enabled` with `--enabled true|false`

`msp` currently supports only `stdio` managed servers, so `msp config --transport stdio` is accepted but no other transport is valid.

### Import servers from Codex

```bash
msp import codex
```

This command:

1. reads Codex MCP servers from `$CODEX_HOME/config.toml` or `~/.codex/config.toml`
2. imports each server into the `msp` config
3. preserves each imported server's optional `enabled` flag and defaults to enabled when the source omits it
4. preserves each imported server's optional `env` table and `env_vars` list, and converts remote `url` plus optional `http_headers`, `bearer_token_env_var`, and `env_http_headers` into `npx -y mcp-remote ... --header ...`
5. reloads only imported servers that are enabled
6. resolves the summary provider with priority `--provider`, then the current import source provider (`codex`)

Without `--provider`, `import codex` uses the `codex` provider with the built-in default model `gpt-5.2`.
For example, `msp import --provider opencode codex` imports Codex servers but summarizes them with OpenCode.

If a Codex server name already exists in the `msp` config after normalization, that server is skipped.
If a Codex server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only Codex MCP servers defined with `command`, optional string `args`, optional boolean `enabled`, optional string-to-string `env`, optional string-array `env_vars`, or remote `url` with optional string-to-string `http_headers`, string `bearer_token_env_var`, and string-to-string `env_http_headers` are importable. `bearer_token_env_var` becomes an `Authorization: Bearer ${ENV_VAR}` header, and `env_http_headers` maps each header name to an environment variable placeholder. Entries that rely on other settings such as `cwd` are rejected instead of being imported partially.

Running `msp import` without a source prints the command help instead of a missing-argument error.

### Import servers from OpenCode

```bash
msp import opencode
```

This command:

1. reads OpenCode MCP servers from `~/.config/opencode/opencode.json`
2. imports each server into the `msp` config
3. preserves each imported server's optional `enabled` flag and defaults to enabled when the source omits it
4. preserves each imported local server's optional `environment` object as `msp` server `env`, and converts remote `url` plus optional `headers` into `npx -y mcp-remote ... --header ...`
5. reloads only imported servers that are enabled
6. resolves the summary provider with priority `--provider`, then the current import source provider (`opencode`)

Without `--provider`, `import opencode` uses the `opencode` provider with the built-in default model `openai/gpt-5.2`.
For example, `msp import --provider codex opencode` imports OpenCode servers but summarizes them with Codex.

If an OpenCode server name already exists in the `msp` config after normalization, that server is skipped.
If an OpenCode server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only OpenCode MCP servers defined as local servers with a string-array `command`, optional `type = "local"`, optional boolean `enabled`, and optional string-to-string `environment`, or as remote servers with `type = "remote"`, a string `url`, optional boolean `enabled`, and optional string-to-string `headers`, are importable. OpenCode already supports environment-variable substitution inside `headers`, and `msp import opencode` preserves those remote header placeholders by converting them for `mcp-remote`. Other settings are rejected instead of being imported partially.

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

1. imports the target tool's MCP servers into `msp`, preserving optional `enabled` flags, and uses that import source's built-in provider
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
Each configured server is emitted on its own application output line with its enabled or disabled state and the last successful cache refresh time in `YYYY-MM-DD HH:MM:SS` format.

Example:

```text
Configured 2 MCP server(s) in /Users/example/.config/mcp-smart-proxy/config.toml (1 enabled, 1 disabled)
`github` [enabled]: npx -y @modelcontextprotocol/server-github (last updated: 2026-03-16 10:30:45)
`slack` [disabled]: uvx slack-mcp (last updated: never)
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

1. reloads the named MCP server, or every enabled configured server if no name is given
2. connects to each selected MCP server
3. fetches its tool list
4. compares the fetched tool list with the cached tool list using JSON string equality
5. if the tools changed, resolves the summary provider from the required `--provider` and writes the cache file

If the fetched tools match the cached tools exactly, `reload` skips the summary call and leaves the cache file unchanged.
Before refreshing one server's cache, `reload` acquires a sibling `.lock` file for that cache path so concurrent refreshes of the same server serialize instead of duplicating work or racing on cache writes.

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

Before exposing the proxy stdio MCP server upstream, this command automatically reloads every enabled configured MCP server.

That startup reload resolves the summary provider from the required `--provider`.

Only after that reload phase succeeds does the proxy start over stdio and load the refreshed cached toolsets. If any server fails to reload, the proxy does not report ready upstream.

While `msp mcp` is running, it checks GitHub for a newer release every 30 minutes and stores the result in `~/.cache/mcp-smart-proxy/version-update.json`.

If a newer release exists, `msp mcp` writes or updates that file. If the current binary is already up to date, `msp mcp` deletes the file.

All other `msp` commands only read that cached record on startup. If the file says a newer version is available, they print a single warning line to stderr so normal stdout output and stdio MCP traffic stay untouched.

## Typical Workflow

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
msp disable github
msp enable github
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

```text
example_tool: Example description
another_tool: Another description that is longer but still fits in the preview
```

Each line is `tool_name: description-preview`.
If a tool has no description, the line is just `tool_name`.
The description preview is at most 80 characters total. If it is truncated, the preview ends with `...`.

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

- Only stdio downstream MCP servers are supported.
- Tool discovery depends on cached metadata produced by `reload`.
- The proxy does not dynamically list downstream tools as first-class proxy tools; it exposes a fixed activation-and-call interface instead.
