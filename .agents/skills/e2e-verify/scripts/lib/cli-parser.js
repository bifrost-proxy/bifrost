const fs = require("fs");
const path = require("path");
const { listTools, getToolsByCategory } = require("./tools");
const { SCENARIOS_DIR } = require("./config");

const VERSION = "2.0.0";

function parseArgs(argv) {
  const args = {
    command: null,
    url: null,
    options: {
      headless: false,
      interactive: false,
      sessionName: null,
      branch: null,
      verbose: false,
      timeout: 30000,
      isolatedProxy: true,
      baseUrl: null,
    },
    positionalArgs: [],
    rawArgs: argv,
  };

  let i = 0;
  while (i < argv.length) {
    const arg = argv[i];

    if (!args.command && !arg.startsWith("-")) {
      args.command = arg;
      i++;
      continue;
    }

    if (!arg.startsWith("-") && args.command) {
      if (arg.startsWith("http://") || arg.startsWith("https://")) {
        args.url = arg;
      } else {
        args.positionalArgs.push(arg);
      }
      i++;
      continue;
    }

    switch (arg) {
      case "-h":
      case "--help":
        args.options.showHelp = true;
        break;
      case "-v":
      case "--version":
        args.options.showVersion = true;
        break;
      case "--headless":
        args.options.headless = true;
        break;
      case "-i":
      case "--interactive":
        args.options.interactive = true;
        break;
      case "-l":
      case "--list":
        args.options.list = true;
        break;
      case "-a":
      case "--actions":
        args.options.actions = true;
        break;
      case "--verbose":
        args.options.verbose = true;
        break;
      case "-s":
      case "--session":
        args.options.sessionName = argv[++i];
        break;
      case "-b":
      case "--branch":
        args.options.branch = argv[++i];
        break;
      case "-t":
      case "--timeout":
        args.options.timeout = parseInt(argv[++i], 10) || 30000;
        break;
      case "--shared-proxy":
        args.options.isolatedProxy = false;
        break;
      case "--base-url":
        args.options.baseUrl = argv[++i];
        break;
      default:
        if (arg.startsWith("-")) {
          console.warn(`Unknown option: ${arg}`);
        }
    }
    i++;
  }

  if (!args.command && argv.length === 0) {
    args.options.showHelp = true;
  }

  return args;
}

function showVersion() {
  console.log(`e2e-verify v${VERSION}`);
}

function showHelp() {
  console.log(`
e2e-verify - E2E Testing Tool

Usage:
  node browser-test.js <command> [options] [url]

Commands:
  launch [url]           Launch browser and navigate to URL
  detach                 Launch browser in detached mode
  connect                Connect to detached browser
  scenario <name>        Run a test scenario
  run <script> [url]     Run test script file
  watch [url]            Watch mode with live snapshots
  sessions               List saved sessions
  tools [category]       List available tools
  help                   Show this help

Options:
  -i, --interactive      Enter interactive mode after launch
  -l, --list             List scenarios (with scenario command)
  -a, --actions          Show steps for the selected scenario
  -s, --session <name>   Session name for save/load
  -b, --branch <name>    Branch name for session
  -t, --timeout <ms>     Default timeout (default: 30000)
  --base-url <url>       Override UI base URL
  --shared-proxy         Reuse an existing proxy instead of starting an isolated one
  --headless             Run in headless mode
  --verbose              Verbose output
  -h, --help             Show help
  -v, --version          Show version

Examples:
  # Launch browser and navigate to the Bifrost UI
  node browser-test.js launch http://localhost:3000/_bifrost/

  # Launch in interactive mode
  node browser-test.js launch http://localhost:3000/_bifrost/ -i

  # Run a scenario
  node browser-test.js scenario stream-sse

  # List all scenarios
  node browser-test.js scenario --list

  # Show scenario steps
  node browser-test.js scenario traffic-delete --actions

  # Run test script
  node browser-test.js run test-script.txt http://localhost:3000/_bifrost/

  # List tools
  node browser-test.js tools input
`);
}

function showScenarioHelp() {
  console.log(`
Scenario Commands:

Usage:
  node browser-test.js scenario <name> [options]
  node browser-test.js scenario --list

Options:
  -l, --list             List all available scenarios
  -a, --actions          Show steps for the selected scenario
  -b, --branch <name>    Branch name for session
  --base-url <url>       Override UI base URL
  --shared-proxy         Reuse an existing proxy instead of starting an isolated one
  --verbose              Verbose output

Examples:
  node browser-test.js scenario --list
  node browser-test.js scenario stream-sse
  node browser-test.js scenario replay-history-filters --headless
  node browser-test.js scenario traffic-delete --actions
`);
}

function showScenarioList() {
  console.log("\nAvailable Scenarios:");
  console.log("-".repeat(60));

  try {
    const files = fs.readdirSync(SCENARIOS_DIR);
    const scenarios = files.filter((f) => f.endsWith(".json"));

    if (scenarios.length === 0) {
      console.log("  No scenarios found.");
      console.log(`  Scenarios directory: ${SCENARIOS_DIR}`);
      return [];
    }

    scenarios.forEach((file) => {
      const name = path.basename(file, ".json");
      try {
        const content = JSON.parse(
          fs.readFileSync(path.join(SCENARIOS_DIR, file), "utf8"),
        );
        const description = content.description || "No description";
        console.log(`  ${name.padEnd(20)} ${description}`);
      } catch {
        console.log(`  ${name.padEnd(20)} (invalid JSON)`);
      }
    });

    return scenarios.map((f) => path.basename(f, ".json"));
  } catch (e) {
    console.log(`  Error reading scenarios: ${e.message}`);
    return [];
  }
}

function showScenarioActions(scenarioName) {
  const scenarioPath = path.join(SCENARIOS_DIR, `${scenarioName}.json`);

  try {
    const content = JSON.parse(fs.readFileSync(scenarioPath, "utf8"));
    console.log(`\nScenario: ${scenarioName}`);
    console.log("-".repeat(60));
    console.log(`Description: ${content.description || "N/A"}`);
    console.log(`Base URL: ${content.config?.baseUrl || "N/A"}`);

    if (content.steps?.length) {
      console.log(`\nSteps (${content.steps.length}):`);
      content.steps.forEach((step, i) => {
        const params = { ...step };
        delete params.action;
        console.log(
          `  ${i + 1}. ${step.action} ${JSON.stringify(params)}`.substring(
            0,
            120,
          ),
        );
      });
    }

    return content;
  } catch (e) {
    console.log(`Error loading scenario: ${e.message}`);
    return null;
  }
}

function showToolsList(category = null) {
  if (category) {
    const categoryTools = getToolsByCategory(category);
    if (categoryTools.length === 0) {
      console.log(`No tools found in category: ${category}`);
      return;
    }
    console.log(`\n${category} tools (${categoryTools.length}):`);
    console.log("-".repeat(60));
    categoryTools.forEach((t) => {
      console.log(`  ${t.name.padEnd(20)} ${t.description}`);
    });
  } else {
    const allTools = listTools();
    console.log("\nAvailable Tools by Category:");
    console.log("=".repeat(60));
    Object.entries(allTools).forEach(([cat, toolList]) => {
      console.log(`\n${cat} (${toolList.length}):`);
      console.log("-".repeat(40));
      toolList.forEach((t) => {
        console.log(`  ${t.name.padEnd(20)} ${t.description}`);
      });
    });
  }
}

module.exports = {
  parseArgs,
  showVersion,
  showHelp,
  showScenarioHelp,
  showScenarioList,
  showScenarioActions,
  showToolsList,
  VERSION,
};
