const readline = require("readline");
const { executeTool } = require("./tool-executor");
const {
  tools,
  getToolsByCategory,
  listTools,
} = require("./tools");
const { saveBrowserSession, loadBrowserSession } = require("./session-manager");
const { VerifyLogger } = require("./logger");

const logger = new VerifyLogger("InteractiveMode");

async function runInteractiveMode(ctx) {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  console.log("\n=== E2E Test Interactive Mode ===");
  console.log("Commands:");
  console.log("  tools              - List all available tools");
  console.log("  tools <category>   - List tools in category");
  console.log("  <tool> [params]    - Execute a tool (params as JSON)");
  console.log("  snapshot           - Take a page snapshot");
  console.log("  screenshot [name]  - Take a screenshot");
  console.log("  save [session]     - Save browser session");
  console.log("  load [session]     - Load browser session");
  console.log("  vars               - Show stored variables");
  console.log("  exit               - Exit interactive mode");
  console.log("");

  const prompt = () =>
    rl.question("e2e> ", async (input) => {
      const trimmed = input.trim();
      if (!trimmed) {
        prompt();
        return;
      }

      if (trimmed === "exit" || trimmed === "quit") {
        rl.close();
        return;
      }

      try {
        if (trimmed === "tools") {
          const allTools = listTools();
          console.log("\nAvailable tools by category:");
          Object.entries(allTools).forEach(([category, toolList]) => {
            console.log(`\n  ${category} (${toolList.length}):`);
            toolList.forEach((t) =>
              console.log(`    - ${t.name}: ${t.description}`),
            );
          });
        } else if (trimmed.startsWith("tools ")) {
          const category = trimmed.substring(6);
          const categoryTools = getToolsByCategory(category);
          if (categoryTools.length === 0) {
            console.log(`No tools found in category: ${category}`);
          } else {
            console.log(`\n${category} tools:`);
            categoryTools.forEach((t) => {
              console.log(`  ${t.name}: ${t.description}`);
              if (t.params) {
                console.log(`    params: ${JSON.stringify(t.params)}`);
              }
            });
          }
        } else if (trimmed === "snapshot") {
          const result = await executeTool(ctx, "takeSnapshot", {});
          console.log(result.success ? result.result.formatted : result.error);
        } else if (trimmed.startsWith("screenshot")) {
          const name =
            trimmed.substring(11).trim() || `screenshot-${Date.now()}`;
          const result = await executeTool(ctx, "screenshot", { name });
          console.log(
            result.success ? `Saved: ${result.result.path}` : result.error,
          );
        } else if (trimmed.startsWith("save")) {
          const session = trimmed.substring(5).trim() || "default";
          await saveBrowserSession(ctx.page, session);
          console.log(`Session saved: ${session}`);
        } else if (trimmed.startsWith("load")) {
          const session = trimmed.substring(5).trim() || "default";
          await loadBrowserSession(ctx.page, session);
          console.log(`Session loaded: ${session}`);
        } else if (trimmed === "vars") {
          const vars = ctx.getAllVariables();
          console.log("Variables:", JSON.stringify(vars, null, 2));
        } else {
          const match = trimmed.match(/^(\w+)(?:\s+(.*))?$/);
          if (match) {
            const [, toolName, paramsStr] = match;
            let params = {};
            if (paramsStr) {
              try {
                params = JSON.parse(paramsStr);
              } catch {
                const simpleMatch = paramsStr.match(/^(\S+)(?:\s+(.*))?$/);
                if (simpleMatch) {
                  params = { uid: simpleMatch[1], value: simpleMatch[2] };
                } else {
                  params = { uid: paramsStr };
                }
              }
            }
            const result = await executeTool(ctx, toolName, params);
            if (result.success) {
              console.log("Result:", JSON.stringify(result.result, null, 2));
            } else {
              console.log("Error:", result.error);
              if (result.suggestions) {
                console.log("Suggestions:", result.suggestions);
              }
            }
          }
        }
      } catch (e) {
        console.log("Error:", e.message);
      }

      prompt();
    });

  prompt();

  return new Promise((resolve) => {
    rl.on("close", resolve);
  });
}

async function runWatchMode(page, ctx, options = {}) {
  const { watchInterval = 1000 } = options;

  logger.info("Watch mode started. Press Ctrl+C to exit.");

  let lastSnapshot = null;

  const watchLoop = setInterval(async () => {
    try {
      const snapshot = await ctx.createSnapshot();
      const snapshotManager = ctx.getSnapshotManager();
      const formatted = snapshotManager.formatSnapshot();

      if (formatted !== lastSnapshot) {
        console.clear();
        console.log("=== Live Snapshot ===\n");
        console.log(formatted);
        console.log("\n=== Network Requests ===");
        const reqs = ctx.getNetworkCollector().getRequests().slice(-5);
        reqs.forEach((r) =>
          console.log(`  [${r.method}] ${r.url.substring(0, 80)}`),
        );
        lastSnapshot = formatted;
      }
    } catch (e) {
      logger.error("Watch error:", e.message);
    }
  }, watchInterval);

  process.on("SIGINT", () => {
    clearInterval(watchLoop);
    logger.info("Watch mode stopped");
    process.exit(0);
  });

  await new Promise(() => {});
}

module.exports = {
  runInteractiveMode,
  runWatchMode,
};
