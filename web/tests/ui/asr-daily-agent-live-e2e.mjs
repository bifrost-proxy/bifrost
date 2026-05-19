import { chromium } from "@playwright/test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const baseUrl = (process.env.BIFROST_LIVE_BASE_URL || "http://127.0.0.1:18896").replace(/\/$/, "");
const apiBase = `${baseUrl}/_bifrost/api`;
const dataDir = path.join(os.homedir(), ".bifrost");
const today = process.env.ASR_DAILY_AGENT_LIVE_DATE || new Date().toISOString().slice(0, 10);
const timeoutMs = Number(process.env.ASR_DAILY_AGENT_LIVE_TIMEOUT_MS || 900000);

const createdTaskIds = [];
const createdAudioDirs = [];

async function api(pathname, options = {}) {
  const res = await fetch(`${apiBase}${pathname}`, {
    ...options,
    headers: {
      "content-type": "application/json",
      ...(options.headers || {}),
    },
  });
  const text = await res.text();
  let body = null;
  if (text.trim()) {
    try {
      body = JSON.parse(text);
    } catch {
      body = text;
    }
  }
  if (!res.ok) {
    throw new Error(`API ${options.method || "GET"} ${pathname} failed ${res.status}: ${text}`);
  }
  return body;
}

async function createTask(nameSuffix) {
  const audioDir = await fs.mkdtemp(path.join(os.tmpdir(), `bifrost-asr-live-audio-${nameSuffix}-`));
  createdAudioDirs.push(audioDir);
  const task = await api("/asr/tasks", {
    method: "POST",
    body: JSON.stringify({
      name: `ASR Daily Agent live ${nameSuffix}`,
      audio_dir: audioDir,
      enabled: false,
      recursive: false,
    }),
  });
  createdTaskIds.push(task.id);
  return task;
}

async function writeDailyInput(taskId, runnerId) {
  const dailyDir = path.join(dataDir, "asr", "data", "text", taskId, "daily");
  await fs.mkdir(path.join(dailyDir, "report"), { recursive: true });
  const marker = `ASR_LIVE_RUNNER_${runnerId}_${Date.now()}`;
  await fs.writeFile(
    path.join(dailyDir, `${today}.md`),
    [
      `# ${today} ASR Daily Agent Live Verification`,
      "",
      `Runner: ${runnerId}`,
      `Marker: ${marker}`,
      "",
      "今天的会议记录：需要确认 Daily Agent 可以读取 ASR 日文档，执行 Runner，并生成报告文件。",
    ].join("\n"),
    "utf-8",
  );
  return { dailyDir, marker, reportPath: path.join(dailyDir, "report", `${today}-report.md`) };
}

async function setInstructions(taskId, marker) {
  const content = [
    "# ASR Daily Agent Live Verification",
    "",
    "你正在执行 Bifrost ASR Daily Agent 的真实 Runner 验证。",
    `只处理 ${today}.md，并创建或覆盖 report/${today}-report.md。`,
    "",
    "报告必须包含以下固定文本：",
    "- ASR Daily Agent Live Runner Result",
    "- runner validation passed",
    `- ${marker}`,
    "",
    "不要修改其他日期或其他目录。",
  ].join("\n");
  await api(`/asr/tasks/${taskId}/daily-agent/agents`, {
    method: "PUT",
    body: JSON.stringify({ content }),
  });
}

async function configureDailyAgent(taskId, runnerId) {
  await api(`/asr/tasks/${taskId}/daily-agent`, {
    method: "PUT",
    body: JSON.stringify({
      enabled: true,
      runner: runnerId,
      timeout_ms: timeoutMs,
      trigger_policy: "manual_only",
      im_delivery: {
        enabled: false,
        channel: "",
        mode: "full_report",
        send_policy: "on_success_with_report",
      },
    }),
  });
}

async function runDailyAgent(taskId) {
  await api(`/asr/tasks/${taskId}/daily-agent/run?force=true&date=${today}`, { method: "POST" });
  const started = Date.now();
  while (Date.now() - started < timeoutMs + 60000) {
    await new Promise((resolve) => setTimeout(resolve, 5000));
    const state = await api(`/asr/tasks/${taskId}/daily-agent`);
    const status = state?.last_run?.status;
    if (status === "success") return state;
    if (status === "failed") {
      throw new Error(`Daily Agent failed for ${taskId}: ${state?.last_run?.error || "unknown error"}`);
    }
  }
  throw new Error(`Daily Agent timed out for ${taskId}`);
}

async function assertReport(reportPath, marker) {
  const content = await fs.readFile(reportPath, "utf-8");
  for (const expected of ["ASR Daily Agent Live Runner Result", "runner validation passed", marker]) {
    if (!content.includes(expected)) {
      throw new Error(`Report ${reportPath} missing expected text: ${expected}`);
    }
  }
  return { reportPath, bytes: Buffer.byteLength(content), preview: content.slice(0, 240) };
}

async function runWebUiConfigE2e(chatConfig, providers) {
  const task = await createTask("webui");
  await api(`/asr/tasks/${task.id}/daily-agent`, {
    method: "PUT",
    body: JSON.stringify({
      enabled: true,
      runner: "bifrost_agent",
      timeout_ms: timeoutMs,
      trigger_policy: "manual_only",
      im_delivery: { enabled: true, channel: "owner:feishu-main", mode: "full_report", send_policy: "on_success_with_report" },
    }),
  });

  const channelProvider = providers.find((provider) => provider.enabled && provider.owner_open_id);
  if (!channelProvider) {
    throw new Error("No enabled IM provider with owner_open_id found in default data");
  }

  const externalRunnerIds = Object.entries(chatConfig.runners || {})
    .filter(([, runner]) => runner.enabled)
    .map(([id]) => id);
  for (const expected of ["codex", "web"]) {
    if (!externalRunnerIds.includes(expected)) {
      throw new Error(`Required runner '${expected}' is not enabled in default data`);
    }
  }

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const clickVisibleOption = async (nameOrRegex) => {
    await page.waitForTimeout(300);
    const dropdown = page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)").last();
    await dropdown.waitFor({ state: "visible", timeout: 30000 });
    const pattern = typeof nameOrRegex === "string" ? null : nameOrRegex.source;
    const flags = typeof nameOrRegex === "string" ? null : nameOrRegex.flags;
    await dropdown.evaluate(
      (root, query) => {
        const matcher = query.text
          ? (value) => value.trim() === query.text
          : (value) => new RegExp(query.pattern, query.flags).test(value.trim());
        const candidates = Array.from(root.querySelectorAll(".ant-select-item-option-content, [role='option']"));
        const target = candidates.find((element) => {
          const rect = element.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && matcher(element.textContent || "");
        });
        if (!target) {
          throw new Error(`visible option not found: ${query.text || query.pattern}`);
        }
        target.click();
      },
      {
        text: typeof nameOrRegex === "string" ? nameOrRegex : null,
        pattern,
        flags,
      },
    );
  };
  try {
    await page.goto(`${baseUrl}/_bifrost/ai?aiSection=tools-asr&asrTask=${task.id}`, {
      waitUntil: "domcontentloaded",
      timeout: 120000,
    });
    await page.getByRole("tab", { name: "Daily Agent" }).click({ timeout: 120000 });
    await page.getByTestId("asr-daily-agent-runner-select").click();
    await clickVisibleOption("codex");
    await page.waitForFunction(
      async ({ apiBase: innerApiBase, taskId }) => {
        const res = await fetch(`${innerApiBase}/asr/tasks/${taskId}/daily-agent`);
        const body = await res.json();
        return body.config.runner === "codex";
      },
      { apiBase, taskId: task.id },
    );
    await page.getByTestId("asr-daily-agent-im-channel-select").click();
    await clickVisibleOption(new RegExp(`${channelProvider.display_name || channelProvider.id} / Owner`));
    await page.waitForFunction(
      async ({ apiBase: innerApiBase, taskId, channel }) => {
        const res = await fetch(`${innerApiBase}/asr/tasks/${taskId}/daily-agent`);
        const body = await res.json();
        return body.config.im_delivery.channel === channel;
      },
      { apiBase, taskId: task.id, channel: `owner:${channelProvider.id}` },
    );
    const pageText = await page.locator("body").innerText();
    for (const removedLabel of ["Runner Type", "Runner ID", "Provider ID", "Target ID"]) {
      if (pageText.includes(removedLabel)) {
        throw new Error(`Removed label still visible in Web UI: ${removedLabel}`);
      }
    }
    return {
      taskId: task.id,
      selectedRunner: "codex",
      selectedChannel: `owner:${channelProvider.id}`,
    };
  } finally {
    await browser.close();
  }
}

async function runRunnerCase(runnerId) {
  const task = await createTask(runnerId);
  const { marker, reportPath } = await writeDailyInput(task.id, runnerId);
  await setInstructions(task.id, marker);
  await configureDailyAgent(task.id, runnerId);
  await runDailyAgent(task.id);
  const report = await assertReport(reportPath, marker);
  const runs = await api(`/asr/tasks/${task.id}/daily-agent/runs`);
  return {
    runner: runnerId,
    taskId: task.id,
    status: "success",
    report,
    processedRunner: runs?.processed_documents?.[0]?.runner || null,
  };
}

async function runImSendCase() {
  const task = await createTask("im-send");
  const { marker, reportPath } = await writeDailyInput(task.id, "im_send");
  await setInstructions(task.id, marker);
  const sendStartedAtMs = Date.now();
  await api(`/asr/tasks/${task.id}/daily-agent`, {
    method: "PUT",
    body: JSON.stringify({
      enabled: true,
      runner: "bifrost_agent",
      timeout_ms: timeoutMs,
      trigger_policy: "manual_only",
      im_delivery: {
        enabled: true,
        channel: "owner:feishu-main",
        mode: "full_report",
        send_policy: "on_success_with_report",
      },
    }),
  });
  await runDailyAgent(task.id);
  const state = await api(`/asr/tasks/${task.id}/daily-agent`);
  if (!state.config?.im_delivery?.last_sent_at_ms) {
    throw new Error(`IM delivery did not record last_sent_at_ms: ${JSON.stringify(state.config?.im_delivery)}`);
  }
  if (state.config?.im_delivery?.last_send_error) {
    throw new Error(`IM delivery recorded last_send_error: ${state.config.im_delivery.last_send_error}`);
  }
  const report = await assertReport(reportPath, marker);
  const messages = await api("/im-gateway/providers/feishu-main/messages?direction=outbound");
  const reportMessage = (messages || []).find((message) =>
    message.timestamp >= sendStartedAtMs &&
    message.target_id === "__owner__" &&
    message.msg_type === "text" &&
    message.content_preview?.includes("ASR Daily Agent Live Runner Result")
  );
  if (!reportMessage) {
    throw new Error(
      `IM delivery did not send full report text; recent outbound previews: ${JSON.stringify(
        (messages || []).slice(0, 5).map((message) => ({
          timestamp: message.timestamp,
          target_id: message.target_id,
          msg_type: message.msg_type,
          content_preview: message.content_preview,
        })),
      )}`,
    );
  }
  return {
    taskId: task.id,
    channel: state.config.im_delivery.channel,
    mode: state.config.im_delivery.mode,
    lastSentAtMs: state.config.im_delivery.last_sent_at_ms,
    sentPreview: reportMessage.content_preview,
    report,
  };
}

async function cleanup() {
  if (process.env.ASR_DAILY_AGENT_LIVE_KEEP_ARTIFACTS === "1") return;
  for (const taskId of createdTaskIds) {
    try {
      await api(`/asr/tasks/${taskId}`, { method: "DELETE" });
    } catch {
      // Best effort cleanup; the run evidence was already printed.
    }
  }
  for (const dir of createdAudioDirs) {
    await fs.rm(dir, { recursive: true, force: true });
  }
}

async function main() {
  const [chatConfig, providers] = await Promise.all([
    api("/im-gateway/chat/config"),
    api("/im-gateway/providers"),
  ]);
  const ui = await runWebUiConfigE2e(chatConfig, providers);
  const runnerResults = [];
  for (const runnerId of ["bifrost_agent", "codex", "web"]) {
    runnerResults.push(await runRunnerCase(runnerId));
  }
  const imSend = await runImSendCase();
  const result = {
    baseUrl,
    dataDir,
    date: today,
    webUi: ui,
    runners: runnerResults,
    imSend,
  };
  console.log(JSON.stringify(result, null, 2));
}

main()
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  })
  .finally(cleanup);
