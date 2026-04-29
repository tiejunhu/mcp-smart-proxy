import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";

const MSP_HELP_ARGS = ["cli", "-h"] as const;
const MSP_HELP_TIMEOUT_MS = 5_000;

type MspHelpState = {
  helpText?: string;
  error?: string;
  loadPromise?: Promise<void>;
  notifiedFailure: boolean;
};

function formatFailureMessage(error: string): string {
  return `MSP prompt injection disabled: ${error}. Run /reload after installing or fixing msp.`;
}

function notifyFailureOnce(state: MspHelpState, ctx?: ExtensionContext): void {
  if (!state.notifiedFailure && state.error && ctx?.hasUI) {
    ctx.ui.notify(formatFailureMessage(state.error), "warning");
    state.notifiedFailure = true;
  }
}

function trimHelpText(stdout: string): string | undefined {
  const text = stdout.trim();
  return text.length > 0 ? text : undefined;
}

function buildSystemPromptSection(helpText: string, bashAvailable: boolean): string {
  const usageGuidance = bashAvailable
    ? [
      "- Use the `bash` tool to inspect and call MSP-backed tools from this session.",
      "- Inspect one tool's arguments with `msp cli <mcp-name> <tool-name> -h`.",
      "- Invoke one tool with `msp cli <mcp-name> <tool-name> --<parameter> <value>`.",
      "- Always inspect the help text before invoking a tool.",
    ]
    : [
      "- The current active tool set does not include `bash`, so treat this MSP inventory as reference only unless another extension provides shell execution.",
    ];

  return `

## MSP CLI Inventory

The local system has the \`msp\` CLI available. The following block is the cached output of \`msp cli -h\` for this pi session. Use it as the current inventory of cached MCP servers that can be reached through MSP.

\`\`\`text
${helpText}
\`\`\`

## How To Use MSP From This Session

${usageGuidance.join("\n")}
`;
}

async function loadMspHelp(pi: ExtensionAPI, state: MspHelpState, ctx?: ExtensionContext): Promise<void> {
  if (state.helpText || state.error) {
    notifyFailureOnce(state, ctx);
    return;
  }

  if (state.loadPromise) {
    return state.loadPromise;
  }

  state.loadPromise = (async () => {
    try {
      const result = await pi.exec("msp", [...MSP_HELP_ARGS], {
        timeout: MSP_HELP_TIMEOUT_MS,
      });
      const helpText = trimHelpText(result.stdout);

      if (result.code === 0 && helpText) {
        state.helpText = helpText;
        state.error = undefined;
        return;
      }

      const stderr = result.stderr.trim();
      state.error = stderr.length > 0 ? stderr : `msp cli -h exited with code ${result.code}`;
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
    }

    notifyFailureOnce(state, ctx);
  })().finally(() => {
    state.loadPromise = undefined;
  });

  return state.loadPromise;
}

export default function mspCliSystemPromptExtension(pi: ExtensionAPI) {
  const state: MspHelpState = {
    notifiedFailure: false,
  };

  pi.on("session_start", async (_event, ctx) => {
    void loadMspHelp(pi, state, ctx);
  });

  pi.on("before_agent_start", async (event, ctx) => {
    await loadMspHelp(pi, state, ctx);

    if (!state.helpText) {
      return;
    }

    const bashAvailable = event.systemPromptOptions.selectedTools?.includes("bash") ?? false;

    return {
      systemPrompt: `${event.systemPrompt}${buildSystemPromptSection(state.helpText, bashAvailable)}`,
    };
  });
}
