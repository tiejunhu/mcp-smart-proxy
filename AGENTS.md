# First in first

- Use English for all code, comments, and documentation, to ensure that it is accessible to the widest possible audience.
- Keep all usage of this project in README.md, update it as needed, and make sure it is clear and concise.
- Before making any changes, present the plan (in the same language as user's input) to the user and ask the user for approving (along with the plan) using any available request user input tool (using the same language as user's input).
- Reduce complexity, during reading and editing the code, find complexity and try to reduce it.

# Code edit

- prefer small files, with a single responsibility, and a clear API
- always make code clean, readable, and maintainable, avoid unnecessary complexity, avoid over-engineering, avoid duplicate code, and follow Rust best practices and idioms
- when some function is not needed, don't hesitate to remove it, we can always add it back when we need it
- the original code architecture and design may not be perfect, improve it when you see an opportunity, but don't try to redesign the whole codebase in one go, make incremental improvements and refactorings as you go
- after editing, always run `cargo fmt` to ensure consistent code formatting, and `cargo clippy` to catch common mistakes and improve code quality

# Documentation

- Keep README.md update to date and user facing, user frendly, and concise, and make sure it covers all the features and usage of the project.
- Doc about the code, design and decision should be updated in AGENTS.md

## Design notes

- Keep shared import/export workflow helpers split by responsibility: provider-specific parsing stays in `src/config/import_export/<provider>.rs`, while format-specific file operations live in shared helpers under `src/config/import_export/`.
- Keep CLI orchestration thin: `src/commands.rs` should focus on top-level dispatch, while grouped command workflows such as import/install or remote auth should live in `src/commands/*.rs`.
- Keep `add` explicit about provider-dependent caching: `msp add` must require `--provider`, resolve the provider before persisting anything, then immediately refresh that server's cache with the chosen provider, and roll back the new config entry if cache generation fails, while keeping provider resolution and reload orchestration out of `src/config/`.
- Keep `msp mcp` on a daemon/client split: the foreground `msp mcp` process should stay a thin stdio MCP facade, while the shared daemon owns downstream MCP communication, socket lifecycle, idle shutdown, and background self-update work.
- Keep `msp mcp` startup cache-first: daemon `load_toolsets` requests should return the current cached snapshot immediately, trigger provider-specific refresh in the background, and avoid blocking MCP readiness on downstream refresh success.
- Keep `msp cli` on the same daemon protocol path as the proxy: terminal listing/help should read cached toolsets through daemon `load_toolsets`, and final downstream tool execution must go through daemon `call_tool` instead of opening separate direct connections from the CLI command.
- Keep daemon management semantics centralized in `src/daemon/`: user-facing commands such as `msp daemon status|stop|restart` should stay thin wrappers over shared lifecycle helpers instead of duplicating socket/process control in CLI dispatch.
- Keep daemon control requests fail-fast: status/stop/restart probes should use short client-side timeouts and report an unresponsive daemon clearly instead of hanging forever when a socket accepts but never replies.
- Keep daemon observability centralized in `src/daemon/`: runtime lifecycle and request logs should be written to a stable file next to the socket so unresponsive or stuck daemons can be diagnosed after detached startup.
- Keep daemon recovery and startup diagnostics together in `src/daemon/`: `stop`/`restart` should be able to force-stop an unresponsive daemon by pid state, and detached startup logs should remain available until the replacement daemon passes a status probe.
- Keep daemon refresh orchestration bounded and non-blocking: concurrent `load_toolsets` refreshes for the same provider should collapse into one shared run, blocking file-lock acquisition must stay off the Tokio worker threads, and downstream tool discovery / summary subprocesses should use explicit timeouts.
- Keep detached daemon startup truly detached on Unix: background daemon children should start in their own session/process group so parent CLI exit does not leave stale socket/pid state behind.
- Keep daemon socket naming short and stable: store the default socket directly under `~/.cache/mcp-smart-proxy/`, derive its file name from a compact config-path hash, and validate Unix socket path length before bind/connect so both default and overridden paths fail early with clear errors.
- Keep self-update logic split by concern: version comparison, state-file persistence, binary installation, and runtime orchestration should not live in a single Rust module.
- Keep local config record construction centralized: adding or importing a server should go through shared draft builders instead of duplicating transport-to-table conversion logic.
- Keep MCP proxy logic split between cache loading, tool-schema helpers, downstream client lifecycle, and request dispatch so `src/mcp_server/` remains easy to extend without re-reading one large file.
- Keep the proxy MCP tool naming user-facing and stable around `additional_mcp`: use `activate_additional_mcp` for server inventory, `activate_tool_in_additional_mcp` for tool detail lookup, and `call_tool_in_additional_mcp` for downstream invocation.
- Keep downstream tool metadata normalization centralized in `src/types.rs`: proxy-specific annotation overrides such as forcing `destructiveHint = false` should be applied in shared snapshot/cache helpers instead of being duplicated in reload, cache loading, or MCP response code.
- Keep GitHub release publication in CI on the `gh` CLI path instead of Node-based third-party release actions, so release jobs stay aligned with GitHub-hosted tooling and avoid deprecated Node runtime churn.
- Keep popup input logic split by concern: shared request/response types stay under `src/input_popup/`, the macOS UI stays in the embedded Swift/AppKit helper built from `swift/input_popup/main.swift`, Rust owns helper extraction and subprocess orchestration, non-macOS targets return a clear unsupported error without linking GUI libraries, and CLI/MCP entrypoints should call the shared popup runner instead of duplicating dialog behavior.
- Keep popup modal completion one-shot in the Swift helper: explicit completion actions, including sending an empty answer, must not be reclassified as a window-close cancellation, so successful selections always survive until JSON response encoding.
- Keep popup progression sequential in the Swift helper only while the timer is active: opening the dialog must not preselect any answer, future unanswered questions must stay locked, and the timer target should always stay on the current unanswered question. Countdown-driven auto-select must answer that current question and then move the timer target forward. Once user input disables the timer, the dialog should switch to a fully manual mode where all questions are editable with one neutral presentation instead of active/answered/pending state distinctions, and both clicks and dialog-wide shortcuts may retarget pending or already answered questions for edits.
- Keep popup timer semantics explicit and input-driven in the Swift helper: auto-select exists only for the no-input case, the dialog should expose a visible stop control before the user answers, the first keyboard or mouse input anywhere in the dialog must also disable the countdown for the rest of that dialog, and using the stop control must clear focus instead of moving it to another dialog element.
- Keep popup completion gated by validity in the Swift helper: every question must still end with a valid answer, including non-empty custom `Other` input, and the dialog should finish automatically once the final answer is confirmed.
- Keep popup keyboard selection deterministic in the Swift helper: assign dialog-wide `1-9a-z` shortcuts in display order until that shortcut set is exhausted, let only those shortcut keys be intercepted at the window level, let plain shortcut keys act only on the current question whenever no custom input is focused, and treat Return inside `Other` as confirm-and-advance.
- Keep popup presentation aligned with native macOS dialog conventions: use a compact informative header, grouped question cards, larger full-row hit targets, restrained accenting for the selected option, and content-driven window sizing that avoids large empty regions. Keep header copy minimal and task-oriented: prefer a short result-oriented title plus only status text that directly supports the current choice, avoid long instructional help text in the header, and label the primary action with the workflow outcome rather than a generic mechanism like Submit.
- Keep popup height strategy content-driven and bounded: size the dialog to the measured content by default, cap the full window height at 800 points, keep the action row fixed, and introduce scrolling only for the content region once that cap is exceeded.
- Keep remote OAuth split by concern: generic OAuth discovery and token storage should stay reusable under `src/remote/oauth.rs`, while unsupported hosted endpoints should be rejected earlier by shared config-level remote URL validation.

# Packages

- rmcp, for mcp server and client
- clap, for command line parsing
- serde, for serialization and deserialization of messages
- tokio, for async runtime

# Console output

- Console output must clearly separate application output from external command output.
- External command output must include the stage, the command line, the stream (`stdout` or `stderr`), and clear start/end or block markers.
- Error output must explain which stage failed and preserve enough original external output that another AI model can diagnose the failing step from the console transcript alone.
