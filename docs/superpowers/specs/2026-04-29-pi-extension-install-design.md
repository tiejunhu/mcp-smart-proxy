# Design: bundled pi extension installation for `msp`

## Summary

Add a new command, `msp install pi`, that installs the bundled pi extension to the global auto-discovery location `~/.pi/agent/extensions/msp.ts`.

To simplify distribution, the extension source must be embedded in the `msp` binary instead of being copied from the repository checkout at runtime.

If the target file already exists, installation overwrites it.

When `msp` later updates itself, any already-installed global pi extension file at `~/.pi/agent/extensions/msp.ts` must also be updated automatically if the embedded extension content changed.

## Goals

- Support `msp install pi` as a first-class CLI command.
- Install the bundled pi extension into pi's global extension directory.
- Remove runtime dependence on the repository checkout for extension installation.
- Keep update behavior automatic for already-installed global pi extensions.
- Keep the implementation small and separate from provider-specific MCP host install logic.

## Non-goals

- Do not support project-local pi extension installation.
- Do not preserve user edits to `~/.pi/agent/extensions/msp.ts`.
- Do not add a new backup format for pi extension installation.
- Do not redesign existing host install/restore flows for Codex, OpenCode, Claude, or Copilot.

## Approaches considered

### A. Embed `msp.ts` in the binary and manage one global file

Store the extension source with `include_str!`, add a dedicated pi install path, and synchronize the installed file whenever a newer binary is running.

- Pros: simplest distribution model, no external assets, predictable update behavior, small code surface.
- Cons: local manual edits to the installed extension will be overwritten.

### B. Keep the extension external and copy it from the checkout

Install by reading `pi-extension/msp.ts` from the current working tree.

- Pros: simple for local development.
- Cons: breaks single-binary distribution, fails when running outside a checkout, does not meet the requirement.

### C. Download the extension during install/update

Fetch the extension from a remote source on demand.

- Pros: decouples extension updates from binary releases.
- Cons: adds network dependence, more failure modes, and more complexity than needed.

## Decision

Use approach A.

## CLI design

### Install

Add `Pi` to `InstallTarget` so the command becomes:

```bash
msp install pi
```

Behavior:

- Resolve the target path as `~/.pi/agent/extensions/msp.ts`.
- Create parent directories if needed.
- Write the embedded extension source atomically.
- Overwrite any existing file at that path.
- Print a normal install success message that names the written file.

### Restore

Add `pi` support to:

```bash
msp restore pi
```

Behavior:

- Remove `~/.pi/agent/extensions/msp.ts` if present.
- Report whether a file was removed.
- Do not touch other pi extensions.

This keeps the CLI consistent with the existing `install` / `restore` command family.

## Architecture

Add a focused module for pi extension installation and synchronization.

### New responsibilities

A small `pi_extension` module should own:

- the embedded `msp.ts` source
- global install path resolution
- install/overwrite logic
- restore/remove logic
- conditional synchronization for already-installed global extension files

### Existing command wiring

- `src/cli.rs`: add `Pi` to `InstallTarget` and tests.
- `src/commands/import_cmd.rs`: dispatch `install pi` and `restore pi` without mixing pi logic into provider import/replace flow.
- `src/commands/provider.rs`: remain provider-focused for MCP host integrations only.

This keeps provider-specific import/install helpers separate from the pi extension file workflow.

## Data flow

### `msp install pi`

1. CLI parses `InstallTarget::Pi`.
2. Command dispatch calls the pi extension installer.
3. Installer resolves `~/.pi/agent/extensions/msp.ts`.
4. Installer writes the embedded content atomically, creating directories as needed.
5. CLI prints a success message.

### `msp restore pi`

1. CLI parses `InstallTarget::Pi`.
2. Command dispatch calls the pi extension restore helper.
3. Helper removes the installed file if it exists.
4. CLI prints the result.

### Automatic sync after self-update

1. A newer `msp` binary replaces the existing executable.
2. The process restarts into the new binary.
3. Early startup checks whether `~/.pi/agent/extensions/msp.ts` exists.
4. If the file does not exist, do nothing.
5. If it exists and differs from the embedded source, overwrite it atomically.
6. If it already matches, do nothing.

The sync path must be silent on the common no-op case.

## Error handling

### Install errors

Report clear failures for:

- missing `HOME`
- failing to create `~/.pi/agent/extensions/`
- failing to write the target file

### Restore errors

Report clear failures for:

- missing `HOME`
- failing to remove the target file

### Automatic sync errors

Automatic sync should be best-effort:

- if the global pi extension is not installed, skip silently
- if sync fails for an installed file, print one warning with enough context to diagnose the path and reason
- sync failure must not block normal CLI or daemon startup

## Testing

Add tests for:

- CLI parsing for `msp install pi`
- CLI parsing for `msp restore pi`
- install path resolution under a test home directory
- install creating parent directories and writing embedded content
- install overwriting an existing file
- restore removing an installed file
- sync updating an existing outdated installed file
- sync skipping when the file is absent
- sync skipping when the file already matches

## Documentation changes

Update `README.md` to:

- document `msp install pi`
- document `msp restore pi`
- document the global install path `~/.pi/agent/extensions/msp.ts`
- state that the pi extension is bundled into the binary
- state that self-update also refreshes the already-installed global pi extension when needed
- fix the local test command to use the actual file name `./pi-extension/msp.ts`

Update `AGENTS.md` design notes to record that bundled pi extension installation and update synchronization live in the dedicated pi extension workflow rather than provider-specific host install helpers.

## Self-review

- No placeholders remain.
- CLI and update behavior are consistent.
- Scope is limited to pi extension install/restore/sync.
- Overwrite semantics are explicit.
