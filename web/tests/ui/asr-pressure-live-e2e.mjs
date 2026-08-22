import { chromium } from "@playwright/test";

const baseUrl = process.env.BIFROST_LIVE_BASE_URL;
const taskId = process.env.BIFROST_ASR_PRESSURE_TASK_ID;

if (!baseUrl || !taskId) {
  throw new Error(
    "BIFROST_LIVE_BASE_URL and BIFROST_ASR_PRESSURE_TASK_ID are required",
  );
}

const pages = [
  ["AI hub", "/_bifrost/ai", "ai-module-hub"],
  ["AI channels", "/_bifrost/ai/channels", "ai-detail-page"],
  ["AI agents", "/_bifrost/ai/agents", "ai-detail-page"],
  ["AI runs", "/_bifrost/ai/runs", "agent-run-summaries"],
  ["ASR scheduled", "/_bifrost/ai/asr", "asr-home-tab-scheduled"],
  [
    "ASR management",
    "/_bifrost/ai/asr?asrTab=management",
    "asr-home-tab-management",
  ],
  ["ASR voice", "/_bifrost/ai/asr?asrTab=voice", "asr-home-tab-voice"],
  [
    "ASR task overview",
    `/_bifrost/ai/asr?asrTask=${encodeURIComponent(taskId)}`,
    "asr-task-detail-page",
  ],
  [
    "ASR task daily",
    `/_bifrost/ai/asr?asrTask=${encodeURIComponent(taskId)}&asrTaskTab=daily`,
    "asr-task-daily-docs-tab",
  ],
  [
    "ASR task Daily Agent",
    `/_bifrost/ai/asr?asrTask=${encodeURIComponent(taskId)}&asrTaskTab=daily-agent`,
    "asr-daily-agents-table",
  ],
  [
    "ASR Daily Agent detail",
    `/_bifrost/ai/asr?asrTask=${encodeURIComponent(taskId)}&asrTaskTab=daily-agent&asrDailyAgentEdit=daily_report`,
    "asr-daily-agent-detail",
  ],
  [
    "ASR Daily Agent records",
    `/_bifrost/ai/asr?asrTask=${encodeURIComponent(taskId)}&asrTaskTab=daily-agent-records`,
    "asr-task-detail-page",
  ],
  [
    "Remote Invoke settings",
    "/_bifrost/settings?tab=remote-invoke",
    "settings-remote-invoke-tab",
  ],
  ["Scripts", "/_bifrost/scripts", "scripts-list-panel"],
];

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
const page = await context.newPage();
const failures = [];

page.on("response", async (response) => {
  if (response.status() >= 500 && response.url().includes("/_bifrost/api/")) {
    let body = "";
    try {
      body = (await response.text()).slice(0, 500);
    } catch {
      body = "<unavailable>";
    }
    failures.push(`${response.status()} ${response.url()} ${body}`);
  }
});

page.on("requestfailed", (request) => {
  if (request.url().includes("/_bifrost/api/")) {
    if (request.failure()?.errorText === "net::ERR_ABORTED") {
      return;
    }
    failures.push(
      `REQUEST_FAILED ${request.method()} ${request.url()} ${request.failure()?.errorText || "unknown"}`,
    );
  }
});

try {
  for (const [name, path, testId] of pages) {
    const failureStart = failures.length;
    await page.goto(`${baseUrl}${path}`, {
      waitUntil: "domcontentloaded",
      timeout: 120_000,
    });
    await page.getByTestId(testId).first().waitFor({
      state: "visible",
      timeout: 120_000,
    });
    await page.waitForTimeout(1_500);
    const pressureError = page.getByText(
      "This operation is paused while Bifrost is under resource pressure",
      { exact: false },
    );
    if ((await pressureError.count()) > 0) {
      throw new Error(`${name} rendered the resource-pressure error`);
    }
    if (failures.length > failureStart) {
      throw new Error(
        `${name} emitted server errors:\n${failures.slice(failureStart).join("\n")}`,
      );
    }
    console.log(`[PASS] ${name}: ${page.url()}`);
  }
} finally {
  await browser.close();
}

if (failures.length > 0) {
  throw new Error(`Admin pages emitted server errors:\n${failures.join("\n")}`);
}
