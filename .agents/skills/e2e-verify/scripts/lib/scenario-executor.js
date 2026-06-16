const fs = require("fs").promises;
const path = require("path");

const { VariableResolver } = require("./variable-resolver");
const { getActionHandler, listActions } = require("./action-handlers");

const LOG_COLORS = {
  info: "\x1b[36m",
  action: "\x1b[32m",
  snapshot: "\x1b[35m",
  assert: "\x1b[33m",
  error: "\x1b[31m",
  warn: "\x1b[33m",
  step: "\x1b[34m",
  network: "\x1b[90m",
  reset: "\x1b[0m",
};

const NETWORK_TRIGGER_ACTIONS = new Set([
  "goto",
  "click",
  "fill",
  "select",
  "reload",
  "goBack",
  "goForward",
  "evaluate",
  "waitForNavigation",
]);

class ScenarioExecutor {
  #ctx = null;
  #scenario = null;
  #config = {};
  #stepIndex = 0;
  #results = [];
  #startTime = 0;
  #networkCollector = null;
  #showNetwork = true;
  #lastNetworkCheck = 0;

  constructor(ctx, scenario, options = {}) {
    this.#ctx = ctx;
    this.#scenario = scenario;
    this.#config = scenario.config || {};
    this.variables = new VariableResolver(scenario.variables || {});
    this.#networkCollector = options.networkCollector || null;
    this.#showNetwork = options.showNetwork ?? true;
  }

  get config() {
    return this.#config;
  }

  get ctx() {
    return this.#ctx;
  }

  log(level, message) {
    const color = LOG_COLORS[level] || LOG_COLORS.info;
    const reset = LOG_COLORS.reset;
    const prefix =
      level === "step"
        ? "📌"
        : level === "error"
          ? "❌"
          : level === "network"
            ? "🌐"
            : "  ";
    const resolvedMessage = this.variables.resolve(message);
    console.log(`${color}${prefix} ${resolvedMessage}${reset}`);
  }

  #logNetworkRequests(action) {
    if (!this.#networkCollector || !this.#showNetwork) return;
    if (!NETWORK_TRIGGER_ACTIONS.has(action)) return;

    const requests = this.#networkCollector.getRequestsSince(
      this.#lastNetworkCheck,
    );
    const apiRequests = requests.filter(
      (r) =>
        r.resourceType === "xhr" ||
        r.resourceType === "fetch" ||
        r.url.includes("/api/") ||
        r.url.includes("/v1/") ||
        r.url.includes("/v2/") ||
        r.fullUrl.includes("/api/") ||
        r.fullUrl.includes("/gateway/"),
    );

    if (apiRequests.length > 0) {
      console.log(
        LOG_COLORS.network +
          "   ┌─ 网络请求 ─────────────────────────────────────" +
          LOG_COLORS.reset,
      );
      for (const req of apiRequests) {
        console.log(
          LOG_COLORS.network +
            "   │ " +
            this.#networkCollector.formatRequest(req) +
            LOG_COLORS.reset,
        );
      }
      console.log(
        LOG_COLORS.network +
          "   └───────────────────────────────────────────────" +
          LOG_COLORS.reset,
      );
    }

    this.#lastNetworkCheck = Date.now();
  }

  async executeStep(step) {
    const { action, ...params } = step;

    if (!action) {
      throw new Error("Step 缺少 action 字段");
    }

    const handler = getActionHandler(action);
    if (!handler) {
      throw new Error(
        `未知的 action: ${action}。支持的 actions: ${listActions().join(", ")}`,
      );
    }

    const resolvedParams = this.variables.resolveObject(params);

    this.#lastNetworkCheck = Date.now();

    try {
      const result = await handler(this.#ctx, resolvedParams, this);

      await new Promise((r) => setTimeout(r, 100));
      this.#logNetworkRequests(action);

      return { success: true, action, result };
    } catch (error) {
      this.#logNetworkRequests(action);
      return { success: false, action, error: error.message };
    }
  }

  async run() {
    const { name, description, steps } = this.#scenario;

    console.log("\n" + "=".repeat(60));
    console.log(`🧪 场景: ${name}`);
    if (description) {
      console.log(`   ${description}`);
    }
    console.log("=".repeat(60) + "\n");

    this.#startTime = Date.now();
    this.#results = [];

    const totalSteps = steps.length;

    for (let i = 0; i < steps.length; i++) {
      this.#stepIndex = i;
      const step = steps[i];

      if (step.action === "log" && step.message?.startsWith("====")) {
        this.log("step", step.message);
        this.#results.push({ success: true, action: "log", skipped: false });
        continue;
      }

      const stepNum = `[${i + 1}/${totalSteps}]`;
      const actionDesc = this.#getActionDescription(step);

      if (step.action !== "log" && step.action !== "wait") {
        process.stdout.write(`${stepNum} ${actionDesc}... `);
      }

      const result = await this.executeStep(step);
      this.#results.push(result);

      if (!result.success) {
        console.log("❌");
        this.log("error", `步骤失败: ${result.error}`);
        break;
      }

      if (step.action !== "log" && step.action !== "wait") {
        console.log("✅");
      }
    }

    return this.#generateReport();
  }

  #getActionDescription(step) {
    const { action, ...params } = step;

    switch (action) {
      case "goto":
        return `导航到 ${params.url}`;
      case "waitForLogin":
        return "等待登录";
      case "takeSnapshot":
        return params.name ? `快照 [${params.name}]` : "创建快照";
      case "click":
        if (params.role && params.name)
          return `点击 ${params.role} "${params.name}"`;
        if (params.uid) return `点击 [${params.uid}]`;
        if (params.selector) return `点击 ${params.selector}`;
        return "点击元素";
      case "fill":
        if (params.role && params.name)
          return `填写 ${params.role} "${params.name}"`;
        if (params.uid) return `填写 [${params.uid}]`;
        if (params.selector) return `填写 ${params.selector}`;
        return "填写表单";
      case "wait":
        return `等待 ${params.ms || params.timeout}ms`;
      case "waitForSelector":
        return `等待选择器 ${params.selector}`;
      case "screenshot":
        return `截图 ${params.name || ""}`;
      case "assert":
        return `断言: ${params.message || params.condition?.substring(0, 30)}`;
      case "evaluate":
        return `执行脚本`;
      case "findElement":
        return `查找元素 ${params.role || ""} "${params.name || ""}"`;
      default:
        return action;
    }
  }

  #generateReport() {
    const duration = Date.now() - this.#startTime;
    const passed = this.#results.filter((r) => r.success).length;
    const failed = this.#results.filter((r) => !r.success).length;
    const total = this.#results.length;

    console.log("\n" + "=".repeat(60));
    console.log("📊 执行报告");
    console.log("=".repeat(60));
    console.log(`   场景: ${this.#scenario.name}`);
    console.log(`   耗时: ${(duration / 1000).toFixed(2)}s`);
    console.log(`   步骤: ${passed}/${total} 通过`);

    if (this.#networkCollector && this.#showNetwork) {
      const summary = this.#networkCollector.getSummary();
      console.log(`   网络: ${summary.api} API 请求, ${summary.failed} 失败`);

      const failedReqs = this.#networkCollector.getFailedRequests();
      if (failedReqs.length > 0) {
        console.log(`\n   ⚠️  失败的请求:`);
        failedReqs.slice(0, 5).forEach((req) => {
          console.log(`      ${req.method} ${req.status} ${req.url}`);
          if (req.error) {
            console.log(`         错误: ${req.error}`);
          }
        });
        if (failedReqs.length > 5) {
          console.log(`      ... 还有 ${failedReqs.length - 5} 个失败请求`);
        }
      }
    }

    if (failed > 0) {
      console.log(`\n   ❌ 失败步骤:`);
      this.#results
        .filter((r) => !r.success)
        .forEach((r, i) => {
          console.log(`      ${i + 1}. [${r.action}] ${r.error}`);
        });
    }

    const success = failed === 0;
    console.log(`\n   结果: ${success ? "✅ 通过" : "❌ 失败"}`);
    console.log("=".repeat(60) + "\n");

    return {
      success,
      scenario: this.#scenario.name,
      duration,
      steps: {
        total,
        passed,
        failed,
      },
      results: this.#results,
      variables: this.variables.getAll(),
      network: this.#networkCollector
        ? this.#networkCollector.getSummary()
        : null,
    };
  }
}

async function loadScenario(scenarioPath) {
  const content = await fs.readFile(scenarioPath, "utf8");
  return JSON.parse(content);
}

async function listScenarios(scenariosDir) {
  try {
    const files = await fs.readdir(scenariosDir);
    const scenarios = [];

    for (const file of files) {
      if (!file.endsWith(".json")) continue;

      const filePath = path.join(scenariosDir, file);
      try {
        const content = await fs.readFile(filePath, "utf8");
        const scenario = JSON.parse(content);
        scenarios.push({
          name: path.basename(file, ".json"),
          title: scenario.name,
          description: scenario.description,
          steps: scenario.steps?.length || 0,
        });
      } catch {
        scenarios.push({
          name: path.basename(file, ".json"),
          title: "解析失败",
          description: "",
          steps: 0,
        });
      }
    }

    return scenarios;
  } catch {
    return [];
  }
}

module.exports = {
  ScenarioExecutor,
  loadScenario,
  listScenarios,
};
