const { TestContext } = require("./test-context");
const { getToolByName } = require("./tools");
const { VerifyLogger } = require("./logger");
const { sessionExists } = require("./session-manager");

const logger = new VerifyLogger("ToolExecutor");

async function createTestContext(page, options = {}) {
  const ctx = new TestContext(options);
  ctx.setPage(page);
  await ctx.createSnapshot();
  return ctx;
}

async function executeTool(ctx, toolName, params = {}) {
  const tool = getToolByName(toolName);
  if (!tool) {
    throw new Error(
      `Unknown tool: ${toolName}. Use 'listTools' to see available tools.`,
    );
  }

  logger.info(`Executing tool: ${toolName}`, params);

  try {
    const result = await tool.handler(ctx, params);
    return { success: true, result };
  } catch (error) {
    logger.error(`Tool ${toolName} failed: ${error.message}`);

    const suggestions =
      ctx.snapshotManager?.getSuggestions?.(params.uid || params.selector) ||
      [];
    return {
      success: false,
      error: error.message,
      suggestions: suggestions.length > 0 ? suggestions : undefined,
    };
  }
}

async function runScript(ctx, script) {
  const lines = script
    .split("\n")
    .filter((l) => l.trim() && !l.trim().startsWith("#"));
  const results = [];

  for (const line of lines) {
    const trimmed = line.trim();
    const match = trimmed.match(/^(\w+)(?:\s+(.*))?$/);

    if (!match) {
      logger.warn(`Invalid command: ${trimmed}`);
      continue;
    }

    const [, toolName, paramsStr] = match;
    let params = {};

    if (paramsStr) {
      try {
        params = JSON.parse(paramsStr);
      } catch {
        params = { uid: paramsStr };
      }
    }

    const result = await executeTool(ctx, toolName, params);
    results.push({ tool: toolName, params, ...result });

    if (!result.success) {
      logger.error(`Script failed at: ${toolName}`);
      break;
    }
  }

  return results;
}

async function verifyUI(page, options = {}) {
  const {
    saveBrowserSession,
    loadBrowserSession,
  } = require("./session-manager");

  const {
    url,
    actions = [],
    assertions = [],
    sessionName = null,
    timeout = 30000,
  } = options;

  const ctx = await createTestContext(page, { timeout });

  if (sessionName && (await sessionExists(sessionName))) {
    await loadBrowserSession(page, sessionName);
  }

  if (url) {
    await executeTool(ctx, "navigate", { url });
  }

  const actionResults = [];
  for (const action of actions) {
    const { tool, ...params } = action;
    const result = await executeTool(ctx, tool, params);
    actionResults.push(result);

    if (!result.success) {
      return {
        success: false,
        phase: "action",
        failedAt: tool,
        error: result.error,
        suggestions: result.suggestions,
        results: actionResults,
      };
    }
  }

  const assertionResults = [];
  for (const assertion of assertions) {
    const { tool, ...params } = assertion;
    const result = await executeTool(ctx, tool, params);
    assertionResults.push(result);

    if (!result.success) {
      return {
        success: false,
        phase: "assertion",
        failedAt: tool,
        error: result.error,
        results: assertionResults,
      };
    }
  }

  if (sessionName) {
    await saveBrowserSession(page, sessionName);
  }

  const networkCollector = ctx.getNetworkCollector();
  const consoleCollector = ctx.getConsoleCollector();

  return {
    success: true,
    actionResults,
    assertionResults,
    networkStats: {
      total: networkCollector.getRequests().length,
      failed: networkCollector.getFailedRequests().length,
    },
    consoleStats: {
      errors: consoleCollector.getErrors().length,
      warnings: consoleCollector.getWarnings().length,
    },
  };
}

module.exports = {
  createTestContext,
  executeTool,
  runScript,
  verifyUI,
};
