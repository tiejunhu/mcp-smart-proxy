# MSP (MCP Smart Proxy)

`msp` is a small Rust CLI that helps an AI work with multiple MCP servers through one proxy server. By proxying multiple downstream MCP servers, it can significantly reduce the number of tools an Agent sees and reduce the cost and token usage wasted in unused tools.

The installed binary name is `msp`.
Running `msp` without any arguments prints the top-level command help.

It does simple things:

1. It connects to a configured MCP server and caches its tool metadata.
2. It generates a one-sentence summary of the toolset using a configured AI provider, which can be the Codex CLI, the OpenCode CLI, or Claude Code.
3. It starts a stdio MCP server that exposes the cached toolsets through a small proxy interface.

## How it works

The proxy server currently exposes three tools:

- `activate_external_mcp`: the description of this tool contains all the MCP servers' name and the one-sentence summary of each one's toolset. Calling this tool with MCP server name as argument returns a plain-text list of downstream tool names and description previews.
- `activate_external_mcp_tool`: returns the full cached definition for one downstream tool by MCP server name and tool name.
- `call_tool_in_external_mcp`: calls one downstream tool by external MCP server name and tool name.

Your Agents see only these three tools. When they want to use a tool from a MCP server, they call `activate_external_mcp` to see the cached tool index, optionally call `activate_external_mcp_tool` to inspect one full tool definition, and then call a specific tool with `call_tool_in_external_mcp`.

## Requirements

- `curl`/`wget` plus `tar`, for installation
- The `codex` CLI for summary using the `codex` provider
- The `opencode` CLI for summary using the `opencode` provider
- The `claude` CLI for summary using the `claude` provider
- A browser session for remote MCP servers that require OAuth login

## Install

Install the latest release for the current platform with the repository installer:

```bash
curl -fsSL https://raw.githubusercontent.com/cybershape/mcp-smart-proxy/master/install.sh | bash
```

The installer resolves the latest version through the GitHub Releases redirect path instead of the GitHub REST API, which avoids unauthenticated `api.github.com` rate limits.

For non-root installs the installer writes `msp` to `~/.local/bin` by default so the running user can later replace that binary during background self-update. Root installs still default to `/usr/local/bin`.

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

## Background Self-Update

When you run `msp mcp`, the long-running proxy process checks GitHub Releases in the background every 30 minutes.

- If it finds a newer released build for the current platform, it downloads the matching archive and atomically replaces the current `msp` binary.
- After a successful replacement, it writes a latest-version record next to the binary, for example `~/.local/bin/msp.latest-version.json`.
- If a running `msp` process finds that its own version is older than that latest-version record, it automatically restarts itself into the updated binary.
- Concurrent self-updates for the same installed binary are serialized with sibling `.lock` files so multiple `msp` processes do not race while replacing the executable or writing the version record.

Background self-update requires that the running user can write to the installed `msp` path.

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

[servers.remote-demo]
transport = "remote"
url = "https://example.com/mcp"

[servers.remote-demo.headers]
Authorization = "Bearer ${DEMO_TOKEN}"

[servers.legacy-demo]
transport = "stdio"
command = "uvx"
args = ["legacy-demo-server"]
enabled = false
```

Notes:

- The config file only stores managed `servers`.
- Each server is enabled by default. Set `enabled = false` or run `msp disable <name>` to keep it configured but exclude it from `msp mcp` activation and bulk reload.
- `transport = "stdio"` servers use `command` plus `args`. `transport = "remote"` servers use a Streamable HTTP `url` plus optional `headers`.
- `env` stores static environment variables for the downstream MCP server, while `env_vars` lists variable names that `msp` forwards from its own process environment when it starts that server.
- `msp config <name>` shows one managed server's current `transport`, `enabled`, `command` / `args` or `url` / `headers`, plus `env` and `env_vars`, and can also update them in place.
- Remote OAuth configuration is discovered automatically at runtime. Access tokens are cached outside the main config file.
- `add`, `reload`, and `mcp` require `--provider <codex|opencode|claude>`.
- `import` accepts `--provider <codex|opencode|claude>` and falls back to the current import source provider when omitted.
- `codex` uses the built-in default model `gpt-5.2`.
- `opencode` uses the built-in default model `openai/gpt-5.2`.
- `claude` uses the built-in default model `sonnet`.

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

If the command passed to `add` is a single `http://` or `https://` URL, `msp` stores it as a native remote server:

```toml
[servers.remote-demo]
transport = "remote"
url = "https://example.com/mcp"
```

At execution time, `msp` connects to that remote entry directly through `rmcp` Streamable HTTP. There is no `mcp-remote` fallback path.

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

Update a remote server in place:

```bash
msp config remote-demo --url https://example.com/mcp --clear-headers --header Authorization='Bearer ${DEMO_TOKEN}'
```

This command:

1. resolves the server by exact or normalized name
2. prints the current `transport`, `enabled`, `command` / `args` or `url` / `headers`, `env`, and `env_vars` values when no update flags are passed
3. updates stdio servers with `--cmd`, `--arg`, and `--clear-args`
4. updates remote servers with `--url`, `--header KEY=VALUE`, `--unset-header KEY`, and `--clear-headers`
5. adds or replaces static environment variables with `--env KEY=VALUE`, removes specific keys with `--unset-env KEY`, and clears the whole table with `--clear-env`
6. adds forwarded environment variable names with `--env-var NAME`, removes specific names with `--unset-env-var NAME`, and clears the whole list with `--clear-env-vars`
7. updates `enabled` with `--enabled true|false`

`msp config --transport stdio` and `msp config --transport remote` are both accepted.

### Log in or out of a remote server

Start OAuth login for one remote server:

```bash
msp login remote-demo
```

Clear cached OAuth credentials for one remote server:

```bash
msp logout remote-demo
```

This command pair:

1. resolves the server by exact or normalized name
2. requires the server to use `transport = "remote"`
3. uses OAuth metadata discovered from the remote MCP server instead of extra local OAuth config
4. starts a local callback listener on an automatically selected random port
5. opens the browser for authorization when needed
6. stores OAuth credentials under `~/.cache/mcp-smart-proxy/oauth/<server-name>.json`

Credential reads and writes use a sibling `.lock` file so concurrent login, logout, and automatic token refresh stay serialized per server.

### Import servers from Codex

```bash
msp import codex
```

This command:

1. reads Codex MCP servers from `$CODEX_HOME/config.toml` or `~/.codex/config.toml`
2. imports each server into the `msp` config
3. preserves each imported server's optional `enabled` flag and defaults to enabled when the source omits it
4. preserves each imported server's optional `env` table and `env_vars` list, and stores remote entries as native `msp` `transport = "remote"` config with `url` plus optional `headers`
5. reloads only imported servers that are enabled
6. resolves the summary provider with priority `--provider`, then the current import source provider (`codex`)

Without `--provider`, `import codex` uses the `codex` provider with the built-in default model `gpt-5.2`.
For example, `msp import --provider opencode codex` imports Codex servers but summarizes them with OpenCode.

If a Codex server name already exists in the `msp` config after normalization, that server is skipped.
If a Codex server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only Codex MCP servers defined with `command`, optional string `args`, optional boolean `enabled`, optional string-to-string `env`, optional string-array `env_vars`, or remote `url` with optional string-to-string `http_headers`, string `bearer_token_env_var`, and string-to-string `env_http_headers` are importable. `bearer_token_env_var` becomes an `Authorization` header placeholder inside `msp`'s native `headers` table, and `env_http_headers` maps each header name to an environment variable placeholder. Entries that rely on other settings such as `cwd` are rejected instead of being imported partially.

Running `msp import` without a source prints the command help instead of a missing-argument error.

### Import servers from OpenCode

```bash
msp import opencode
```

This command:

1. reads OpenCode MCP servers from `~/.config/opencode/opencode.json`
2. imports each server into the `msp` config
3. preserves each imported server's optional `enabled` flag and defaults to enabled when the source omits it
4. preserves each imported local server's optional `environment` object as `msp` server `env`, and stores remote `url` plus optional `headers` as native `msp` remote config
5. reloads only imported servers that are enabled
6. resolves the summary provider with priority `--provider`, then the current import source provider (`opencode`)

Without `--provider`, `import opencode` uses the `opencode` provider with the built-in default model `openai/gpt-5.2`.
For example, `msp import --provider codex opencode` imports OpenCode servers but summarizes them with Codex.

If an OpenCode server name already exists in the `msp` config after normalization, that server is skipped.
If an OpenCode server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only OpenCode MCP servers defined as local servers with a string-array `command`, optional `type = "local"`, optional boolean `enabled`, and optional string-to-string `environment`, or as remote servers with `type = "remote"`, a string `url`, optional boolean `enabled`, and optional string-to-string `headers`, are importable. OpenCode already supports environment-variable substitution inside `headers`, and `msp import opencode` preserves those remote header placeholders inside `msp`'s native `headers` table. Other settings are rejected instead of being imported partially.

### Import servers from Claude Code

```bash
msp import claude
```

This command:

1. reads Claude Code MCP servers from `~/.claude.json`
2. imports servers from the user-scope `mcpServers` object into the `msp` config
3. imports local `stdio` servers with `command`, optional `args`, and optional `env`
4. stores remote `http` or `sse` servers with `url` plus optional `headers` as native `msp` remote config
5. preserves Claude-style header placeholders like `${API_KEY}` in `msp`'s native `headers` table and records the referenced env var names in `env_vars`
6. reloads imported servers with the summary provider resolved by priority `--provider`, then the current import source provider (`claude`)

Without `--provider`, `import claude` uses the `claude` provider with the built-in default model `sonnet`.
For example, `msp import --provider codex claude` imports Claude Code servers but summarizes them with Codex.

If a Claude Code server name already exists in the `msp` config after normalization, that server is skipped.
If a Claude Code server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only the Claude Code user-scope `mcpServers` object in `~/.claude.json` is imported in this release. Project-scoped `.mcp.json`, local per-project entries inside `~/.claude.json`, and Claude-specific advanced settings such as OAuth metadata or `headersHelper` are not imported.

Only Claude Code MCP servers defined as `stdio` servers with `command`, optional `args`, optional `env`, and optional `type`, or as `http` / `sse` servers with `url`, optional `headers`, and optional `type`, are importable. Other settings are rejected instead of being imported partially.

### Install this proxy into Codex, OpenCode, or Claude Code

Install into Codex:

```bash
msp install codex
```

Install into OpenCode:

```bash
msp install opencode
```

Install into Claude Code:

```bash
msp install claude
```

Replace existing target MCP servers after importing them into `msp`:

```bash
msp install codex --replace
msp install opencode --replace
msp install claude --replace
```

This command:

1. reads the target config file for Codex, OpenCode, or Claude Code
2. checks whether that config already contains an MCP server that runs `msp mcp`
3. if that server already uses `msp mcp --provider codex`, `msp mcp --provider opencode`, or `msp mcp --provider claude`, reports it as already installed
4. otherwise updates the existing `msp mcp` entry to the requested provider, or creates a new entry if none exists
5. prefers the server name `msp`; if that name is already used by another server, creates `msp1`, `msp2`, and so on

`install codex` writes into `$CODEX_HOME/config.toml` or `~/.codex/config.toml`.
`install opencode` writes into `~/.config/opencode/opencode.json`.
`install claude` writes into the user-scope `mcpServers` object in `~/.claude.json`.

With `--replace`, `install` performs four extra steps before the final install:

1. imports the target tool's MCP servers into `msp`, preserving optional `enabled` flags, and uses that import source's built-in provider
2. merges every MCP server currently present in the target config into a backup file
3. removes all MCP servers from the target config
4. installs `msp mcp --provider codex`, `msp mcp --provider opencode`, or `msp mcp --provider claude`

The backup files are:

- Codex: `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`
- OpenCode: `~/.config/opencode/opencode.msp-backup.json`
- Claude Code: `~/.claude.msp-backup.json`

If a backup file already exists, `--replace` updates it in place by server name so the backup stays deduplicated.

### Restore backed up MCP servers into Codex, OpenCode, or Claude Code

Restore into Codex:

```bash
msp restore codex
```

Restore into OpenCode:

```bash
msp restore opencode
```

Restore into Claude Code:

```bash
msp restore claude
```

This command:

1. reads the target backup file created by `msp install --replace`
2. removes every MCP server in the target config whose command is `msp mcp ...`
3. merges every backed up MCP server back into the target config by server name

`restore codex` reads from `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`.
`restore opencode` reads from `~/.config/opencode/opencode.msp-backup.json`.
`restore claude` reads from `~/.claude.msp-backup.json`.

If the backup file is missing, `restore` fails instead of restoring partially.

### List configured servers

```bash
msp list
```

This command reads the configured MCP servers from the config file and prints each normalized server name with its configured command line or remote URL.
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
If a remote Streamable HTTP server requires OAuth and cached credentials are missing or expired, `reload` can open a browser and complete login automatically before it retries the request.

The cache is stored at:

```text
~/.cache/mcp-smart-proxy/<server-name>.json
```

- `reload` fails if `--provider` is omitted.
- For `codex`, install the `codex` CLI; `reload` runs `codex exec`.
- For `opencode`, install the `opencode` CLI; `reload` runs `opencode run`.
- For `claude`, install the `claude` CLI; `reload` runs `claude --bare --tools \"\" -p`.

### Start the proxy MCP server

```bash
msp mcp --provider codex
```

Before exposing the proxy stdio MCP server upstream, this command automatically reloads every enabled configured MCP server.

That startup reload resolves the summary provider from the required `--provider`.

Only after that reload phase succeeds does the proxy start over stdio and load the refreshed cached toolsets. If any server fails to reload, the proxy does not report ready upstream.

While `msp mcp` is running, it checks GitHub for a newer release every 30 minutes.

If a newer release exists for the current platform, `msp mcp` downloads it, atomically replaces the current `msp` binary, and writes the installed-version record next to that binary.

The same background check also refreshes `~/.cache/mcp-smart-proxy/version-update.json`, which one-shot `msp` commands read on startup to print a warning when a newer release is available.

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

Importing existing Claude Code MCP servers:

```bash
msp import claude
```

Install into Codex:

```bash
msp install codex
```

Install into OpenCode:

```bash
msp install opencode
```

Install into Claude Code:

```bash
msp install claude
```

Replace existing target MCP servers during install:

```bash
msp install codex --replace
msp install opencode --replace
msp install claude --replace
```

Restore backed up MCP servers into Codex, OpenCode, or Claude Code:

```bash
msp restore codex
msp restore opencode
msp restore claude
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

- Downstream MCP servers must use either stdio or Streamable HTTP transport.
- Remote downstream MCP servers do not fall back to `mcp-remote`.
- Remote OAuth currently assumes an interactive browser-based authorization code flow.
- Tool discovery depends on cached metadata produced by `reload`.
- The proxy does not dynamically list downstream tools as first-class proxy tools; it exposes a fixed activation-and-call interface instead.
