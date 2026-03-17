# mcp-smart-proxy

`mcp-smart-proxy` is a small Rust CLI that helps an AI work with multiple MCP servers through one proxy server. By proxying multiple downstream MCP servers, it can significantly reduce the number of tools an Agent sees and reduce the cost and token usage wasted in unused tools.

The installed binary name is `msp`.
Running `msp` without any arguments prints the top-level command help.

It does simple things:

1. It connects to a configured MCP server and caches its tool metadata.
2. It generates a one-sentence summary of the toolset using a configured AI provider, which can be the OpenAI API, the Codex CLI, or the OpenCode CLI.
3. It starts a stdio MCP server that exposes the cached toolsets through a small proxy interface.

## What It Does

The proxy server currently exposes two tools:

- `activate_external_mcp`: the description of this tool contains the MCP server name and the one-sentence summary of its toolset. Calling this tool returns the list of tools from that downstream MCP server.
- `call_tool_in_external_mcp`: calls one downstream tool by external MCP server name and tool name.

This lets Agents see only the MCP server's name/one-sentence summary first. When they want to use a tool from that server, they call `activate_external_mcp` to see the list of tools. Then they can call a specific tool with `call_tool_in_external_mcp`.

## Requirements

- Homebrew for installation on macOS and Linux
- An OpenAI-compatible API key for summary using the `openai` provider
- The `codex` CLI for summary using the `codex` provider
- The `opencode` CLI for summary using the `opencode` provider
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

## Build

```bash
cargo build --bin msp
```

Run the CLI during development with:

```bash
cargo run -- --help
```

## Console Output

`msp` writes structured console output so another AI or operator can distinguish application events from external command output without making humans read raw log blobs.

- Application success output is a single line in the form `[MSP][INFO][stage] message`.
- Application failure output is printed as a short error block with the stage, summary, and numbered causes.
- Successful external commands stay silent.
- Failed external commands emit `=== MSP EXTERNAL COMMAND FAILURE BEGIN ===` and `=== MSP EXTERNAL COMMAND FAILURE END ===`.
- External output blocks are printed only for failures and include the stage, target, command line, stream, and fenced stream content markers.

Example success output:

```text
[MSP][INFO][cli.reload] Reloaded MCP server `github`. Cache file: /Users/example/.cache/mcp-smart-proxy/github.json
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
default_provider = "openai"

[openai]
key = "sk-..."
model = "gpt-5.2"
# baseurl = "https://api.openai.com/v1"

[codex]
model = "gpt-5.2"

[opencode]
model = "openai/gpt-5.2"

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

- `default_provider` is required for `add`, `reload`, and `mcp` only when `--provider` is not supplied.
- `import codex` and `import opencode` do not require `default_provider`; each import flow uses its source provider to generate the one-sentence tool summary.
- `add`, `reload`, `import`, and `mcp` accept `--provider <openai|codex|opencode>`. When present, `--provider` takes priority over the normal provider-selection rule.
- `openai.key` can also come from `OPENAI_API_KEY`.
- `openai.baseurl` can also come from `OPENAI_API_BASE`.
- If `openai.model` or `codex.model` is missing, the default is `gpt-5.2`.
- If `opencode.model` is missing, the default is `openai/gpt-5.2`.

## Commands

### Configure OpenAI settings

```bash
msp config openai --key "$OPENAI_API_KEY" --model gpt-5.2
```

Running `msp config openai` without any flags prints the command help instead of writing an empty update.

Optional fields:

```bash
msp config openai --baseurl https://api.openai.com/v1
msp config openai --default
```

`--default` writes `default_provider = "openai"` into the config file. Commands that use the configured default provider fail fast if `default_provider` is missing.

### Configure Codex settings

```bash
msp config codex --model gpt-5.2
```

Running `msp config codex` without any flags prints the command help instead of writing an empty update.

Optional fields:

```bash
msp config codex --default
```

`codex.model` is optional and defaults to `gpt-5.2`. When Codex is the active summary provider, `msp` calls `codex exec` to generate the same one-sentence toolset summary used by the OpenAI provider.

### Configure OpenCode settings

```bash
msp config opencode --model openai/gpt-5.2
```

Running `msp config opencode` without any flags prints the command help instead of writing an empty update.

Optional fields:

```bash
msp config opencode --default
```

`opencode.model` is optional and defaults to `openai/gpt-5.2`. When OpenCode is the active summary provider, `msp` calls `opencode run` to generate the same one-sentence toolset summary used by the other providers.

### Add a server

```bash
msp add github npx -y @modelcontextprotocol/server-github
```

Override the provider for this run:

```bash
msp add --provider codex github npx -y @modelcontextprotocol/server-github
```

This command:

1. resolves the summary provider with priority `--provider`, then `default_provider`
2. writes the server definition into the config file
3. immediately runs the same refresh flow as `reload`

If neither `--provider` nor a supported `default_provider` is available, `add` fails before changing the config file.
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

Override the provider used during import:

```bash
msp import --provider openai codex
```

This command:

1. reads Codex MCP servers from `$CODEX_HOME/config.toml` or `~/.codex/config.toml`
2. imports each server into the `msp` config
3. reloads every imported server immediately
4. resolves the summary provider with priority `--provider`, then the current import source provider (`codex`)

`import codex` does not require `default_provider`. Without `--provider`, it uses the `codex` provider. If `[codex]` is missing, the default model is `gpt-5.2`.

If a Codex server name already exists in the `msp` config after normalization, that server is skipped.
If a Codex server launches this proxy with `msp mcp`, that entry is also skipped during import.

Only Codex MCP servers defined with `command` and optional string `args` are importable. Entries that rely on other settings such as `env`, `cwd`, or non-stdio transports are rejected instead of being imported partially.

Running `msp import` without a source prints the command help instead of a missing-argument error.

### Import servers from OpenCode

```bash
msp import opencode
```

Override the provider used during import:

```bash
msp import --provider codex opencode
```

This command:

1. reads OpenCode MCP servers from `~/.config/opencode/opencode.json`
2. imports each server into the `msp` config
3. reloads every imported server immediately
4. resolves the summary provider with priority `--provider`, then the current import source provider (`opencode`)

`import opencode` does not require `default_provider`. Without `--provider`, it uses the `opencode` provider. If `[opencode]` is missing, the default model is `openai/gpt-5.2`.

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

1. runs the same import flow as `msp import codex` or `msp import opencode`
2. merges every MCP server currently present in the target config into a backup file
3. removes all MCP servers from the target config
4. installs `msp mcp --provider codex` or `msp mcp --provider opencode`

The backup files are:

- Codex: `$CODEX_HOME/config.msp-backup.toml` or `~/.codex/config.msp-backup.toml`
- OpenCode: `~/.config/opencode/opencode.msp-backup.json`

If a backup file already exists, `--replace` updates it in place by server name so the backup stays deduplicated.

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
msp reload github
```

Or reload every configured server:

```bash
msp reload
```

Override the provider for this run:

```bash
msp reload --provider codex github
```

This command:

1. reloads the named MCP server, or every configured server if no name is given
2. connects to each selected MCP server
3. fetches its tool list
4. compares the fetched tool list with the cached tool list using JSON string equality
5. if the tools changed, resolves the summary provider with priority `--provider`, then `default_provider`, and writes the cache file

If the fetched tools match the cached tools exactly, `reload` skips the summary call and leaves the cache file unchanged.

The cache is stored at:

```text
~/.cache/mcp-smart-proxy/<server-name>.json
```

`reload` requires either `--provider` or a supported `default_provider`.

- For `openai`, configure `openai.key` or `OPENAI_API_KEY`.
- For `codex`, install the `codex` CLI; `reload` runs `codex exec`.

### Start the proxy MCP server

```bash
msp mcp
```

Override the provider used during the startup reload:

```bash
msp mcp --provider opencode
```

Before exposing the proxy stdio MCP server upstream, this command automatically reloads every configured MCP server.

That startup reload resolves the summary provider with priority `--provider`, then `default_provider`.

Only after that reload phase succeeds does the proxy start over stdio and load the refreshed cached toolsets. If any server fails to reload, the proxy does not report ready upstream.

## Typical Workflow

```bash
msp config openai --key "$OPENAI_API_KEY" --default
msp add github npx -y @modelcontextprotocol/server-github
msp list
```

Using Codex:

```bash
msp config codex --default
msp add github npx -y @modelcontextprotocol/server-github
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
