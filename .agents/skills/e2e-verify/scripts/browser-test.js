#!/usr/bin/env node

const fs = require("fs");
const path = require("path");

const {
  launchBrowser,
  closeBrowser,
  connectToBrowser,
  launchDetachedBrowser,
  connectToDetachedBrowser,
  setupPageListeners,
  DEFAULT_VIEWPORT,
} = require("./lib/browser-manager");

const {
  saveBrowserSession,
  loadBrowserSession,
  sessionExists,
  getSessionInfo,
  deleteSession,
  listSessions,
} = require("./lib/session-manager");

const {
  createTestContext,
  executeTool,
  runScript,
  verifyUI,
} = require("./lib/tool-executor");

const {
  runInteractiveMode,
  runWatchMode,
} = require("./lib/interactive-mode");

const {
  handleSSOIfNeeded,
  detectSSOPage,
  waitForSSOLogin,
  ensureLoggedIn,
} = require("./lib/sso-handler");

const {
  parseArgs,
  showVersion,
  showHelp,
  showScenarioHelp,
  showScenarioList,
  showScenarioActions,
  showToolsList,
  VERSION,
} = require("./lib/cli-parser");

const { ScenarioExecutor } = require("./lib/scenario-executor");
const { tools, getToolByName, listTools, getToolsByCategory } = require("./lib/tools");
const { VerifyLogger } = require("./lib/logger");
const { DEFAULT_UI_URL } = require("./lib/config");

const logger = new VerifyLogger("BrowserTest");

function handleFatalError(error) {
  logger.error(`Fatal error: ${error.message}`);
  if (process.env.DEBUG) {
    console.error(error.stack);
  }
  process.exit(1);
}

async function cmdLaunch(args) {
  const { url, options } = args;

  const browser = await launchBrowser({
    headless: options.headless,
    fixedViewport: true,
  });

  const page = await browser.newPage();
  setupPageListeners(page);

  if (url) {
    logger.info(`Navigating to: ${url}`);
    await page.goto(url, { waitUntil: "networkidle0", timeout: options.timeout });
    await new Promise((r) => setTimeout(r, 2000));
  }

  const ctx = await createTestContext(page, { timeout: options.timeout });

  await handleSSOIfNeeded(page, ctx, { timeout: 120000 });

  if (options.interactive) {
    await runInteractiveMode(ctx);
    await closeBrowser(browser);
  } else {
    const snapshot = await executeTool(ctx, "takeSnapshot", {});
    console.log("\n=== Page Snapshot ===\n");
    console.log(snapshot.result?.formatted || "No snapshot available");

    console.log("\nBrowser remains open. Press Ctrl+C to close.");
    await new Promise(() => {});
  }
}

async function cmdDetach(args) {
  const info = await launchDetachedBrowser({ headless: false });
  console.log("Detached browser launched.");
  console.log(`WebSocket: ${info.wsEndpoint}`);
  console.log(`PID: ${info.pid}`);
}

async function cmdConnect(args) {
  const result = await connectToDetachedBrowser();
  if (!result) {
    console.log("No detached browser found.");
    return;
  }

  const { browser } = result;
  const pages = await browser.pages();
  const page = pages[0] || (await browser.newPage());

  setupPageListeners(page);
  const ctx = await createTestContext(page, { timeout: args.options.timeout });

  if (args.options.interactive) {
    await runInteractiveMode(ctx);
  } else {
    console.log("Connected to detached browser.");
    console.log(`Current URL: ${page.url()}`);
  }
}

async function cmdScenario(args) {
  const { positionalArgs, options } = args;

  if (options.list) {
    showScenarioList();
    return;
  }

  if (options.showHelp || positionalArgs.length === 0) {
    showScenarioHelp();
    return;
  }

  const scenarioName = positionalArgs[0];
  if (options.actions) {
    showScenarioActions(scenarioName);
    return;
  }

  const { loadScenario } = require("./lib/scenario-executor");
  const { NetworkCollector } = require("./lib/network-collector");
  const { SCENARIOS_DIR, SESSION_DIR } = require("./lib/config");
  const { startIsolatedProxy, rewriteScenarioStrings } = require("./lib/isolated-proxy");
  const path = require("path");

  const scenarioPath = path.join(SCENARIOS_DIR, `${scenarioName}.json`);
  let scenario;
  try {
    scenario = await loadScenario(scenarioPath);
  } catch (e) {
    logger.error(`Failed to load scenario '${scenarioName}': ${e.message}`);
    process.exit(1);
  }
  let isolatedProxy = null;
  let browser = null;
  let networkCollector = null;
  try {
    if (options.isolatedProxy) {
      isolatedProxy = await startIsolatedProxy(scenarioName);
      scenario = rewriteScenarioStrings(scenario, isolatedProxy.port);
      scenario.config = {
        ...scenario.config,
        baseUrl: isolatedProxy.baseUrl,
      };
      logger.info(
        `Started isolated proxy for scenario '${scenarioName}' on port ${isolatedProxy.port}`,
      );
    }

    const userDataDir = path.join(
      SESSION_DIR,
      `profile-${isolatedProxy?.runId || `${scenarioName}-${Date.now()}`}`,
    );
    browser = await launchBrowser({ headless: options.headless, userDataDir });
    const page = await browser.newPage();
    setupPageListeners(page);

    const ctx = await createTestContext(page, { timeout: options.timeout });

    networkCollector = new NetworkCollector();
    networkCollector.attach(page);
    networkCollector.start();

    const baseUrl = options.baseUrl || scenario.config?.baseUrl || DEFAULT_UI_URL;
    logger.info(`Navigating to: ${baseUrl}`);
    await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: options.timeout });
    await new Promise((r) => setTimeout(r, 2000));

    if (scenario.config?.waitForLogin) {
      await handleSSOIfNeeded(page, ctx, { timeout: scenario.config?.loginTimeout || 120000 });
    }

    const executor = new ScenarioExecutor(ctx, scenario, {
      networkCollector,
      showNetwork: options.verbose,
    });

    const result = await executor.run();

    if (result.success) {
      logger.info(`Scenario '${scenarioName}' completed successfully`);
    } else {
      logger.error(`Scenario '${scenarioName}' failed`);
      process.exitCode = 1;
    }
  } finally {
    networkCollector?.stop();
    if (browser) {
      await closeBrowser(browser);
    }
    if (isolatedProxy) {
      await isolatedProxy.stop();
      logger.info(
        `Stopped isolated proxy for scenario '${scenarioName}' on port ${isolatedProxy.port}`,
      );
    }
  }

  if (process.exitCode) {
    process.exit(process.exitCode);
  }
}

async function cmdRun(args) {
  const { positionalArgs, url, options } = args;

  if (positionalArgs.length === 0) {
    console.log("Usage: node browser-test.js run <script.txt> [url]");
    return;
  }

  const scriptFile = positionalArgs[0];
  const targetUrl = url || positionalArgs[1] || DEFAULT_UI_URL;

  if (!fs.existsSync(scriptFile)) {
    logger.error(`Script file not found: ${scriptFile}`);
    process.exit(1);
  }

  const script = fs.readFileSync(scriptFile, "utf8");

  const browser = await launchBrowser({ headless: options.headless });
  const page = await browser.newPage();
  setupPageListeners(page);

  await page.goto(targetUrl, { waitUntil: "networkidle0", timeout: options.timeout });
  await new Promise((r) => setTimeout(r, 2000));

  const ctx = await createTestContext(page, { timeout: options.timeout });

  await handleSSOIfNeeded(page, ctx, { timeout: 120000 });

  const results = await runScript(ctx, script);

  console.log("\n=== Script Results ===\n");
  console.log(JSON.stringify(results, null, 2));

  const failed = results.some((r) => !r.success);
  if (failed) {
    logger.error("Script execution failed");
    process.exit(1);
  }

  await closeBrowser(browser);
}

async function cmdWatch(args) {
  const { url, options } = args;
  const targetUrl = url || DEFAULT_UI_URL;

  const browser = await launchBrowser({ headless: false });
  const page = await browser.newPage();
  setupPageListeners(page);

  await page.goto(targetUrl, { waitUntil: "networkidle0", timeout: options.timeout });
  await new Promise((r) => setTimeout(r, 2000));

  const ctx = await createTestContext(page, { timeout: options.timeout });

  await handleSSOIfNeeded(page, ctx, { timeout: 120000 });

  await runWatchMode(page, ctx);
}

async function cmdSessions(args) {
  const sessions = await listSessions();

  if (sessions.length === 0) {
    console.log("No saved sessions found.");
    return;
  }

  console.log("\nSaved Sessions:");
  console.log("-".repeat(40));
  sessions.forEach((s) => console.log(`  ${s}`));
}

async function cmdTools(args) {
  const category = args.positionalArgs[0] || null;
  showToolsList(category);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.options.showVersion) {
    showVersion();
    return;
  }

  if (args.options.showHelp && !args.command) {
    showHelp();
    return;
  }

  const command = args.command || "help";

  switch (command) {
    case "launch":
      await cmdLaunch(args);
      break;

    case "detach":
      await cmdDetach(args);
      break;

    case "connect":
      await cmdConnect(args);
      break;

    case "scenario":
      await cmdScenario(args);
      break;

    case "run":
      await cmdRun(args);
      break;

    case "watch":
      await cmdWatch(args);
      break;

    case "sessions":
      await cmdSessions(args);
      break;

    case "tools":
      await cmdTools(args);
      break;

    case "help":
    default:
      showHelp();
      break;
  }
}

module.exports = {
  launchBrowser,
  closeBrowser,
  connectToBrowser,
  launchDetachedBrowser,
  connectToDetachedBrowser,
  setupPageListeners,
  createTestContext,
  executeTool,
  runScript,
  verifyUI,
  runInteractiveMode,
  runWatchMode,
  saveBrowserSession,
  loadBrowserSession,
  sessionExists,
  getSessionInfo,
  deleteSession,
  listSessions,
  handleSSOIfNeeded,
  detectSSOPage,
  waitForSSOLogin,
  ensureLoggedIn,
  tools,
  getToolByName,
  listTools,
  getToolsByCategory,
  DEFAULT_VIEWPORT,
  VERSION,
};

if (require.main === module) {
  main().catch(handleFatalError);
}
