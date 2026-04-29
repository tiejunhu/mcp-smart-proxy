# MSP pi Extension

This directory contains a project-local pi extension that helps pi discover MSP-managed MCP tools.

What it does:

- Detects whether `msp` is available on the local system.
- Runs `msp cli -h` once per pi session.
- Caches that output for the rest of the session.
- Appends the cached help text and short usage guidance to pi's `systemPrompt` in `before_agent_start`.
- If `msp` is missing or the command fails, shows one warning and then stays out of the way.

## Try it without installing

From this repository root:

```bash
pi -e ./pi-extension/msp.ts
```

## Refresh behavior

The extension caches `msp cli -h` per pi session. If your MSP inventory changes, run `/reload` or start a new pi session so the extension refreshes its cached snapshot.
