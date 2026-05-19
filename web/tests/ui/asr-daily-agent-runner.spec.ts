import { expect, type Page, test } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

const taskId = "asr-runner-select-test";
const longInstructions = Array.from(
  { length: 36 },
  (_, index) =>
    `# Daily Agent line ${index + 1}\n\nKeep the ASR daily report concise, actionable, and tied to the transcript evidence.`
).join("\n");

function taskDetail() {
  return {
    id: taskId,
    name: "ASR Runner Select Test",
    audio_dir: "/tmp/bifrost-asr-runner-select",
    recursive: true,
    enabled: false,
    schedule: { kind: "daily", hour: 3, minute: 17 },
    language: "chinese",
    model: "Qwen3-ASR-1.7B",
    runtime_strategy: "reuse_per_file",
    created_at_ms: 1_700_000_000_000,
    updated_at_ms: 1_700_000_000_000,
    summary: {
      discovered: 0,
      processed: 0,
      pending: 0,
      failed: 0,
      partial_success: 0,
      failed_chunk_count: 0,
      deleted_after_processing: 0,
      running: false,
    },
    files: [],
    daily_documents: [],
  };
}

async function installDailyAgentMocks(page: Page) {
  let dailyAgentConfig = {
    enabled: false,
    runner: "bifrost_agent",
    timeout_ms: 7_200_000,
    trigger_policy: "after_asr_run",
    session_key: undefined as string | undefined,
    instructions_source: "default",
    im_delivery: {
      enabled: false,
      channel: undefined as string | undefined,
      mode: "summary",
      send_policy: "on_success_with_report",
    },
  };
  const updates: Array<Record<string, unknown>> = [];

  await page.route("**/_bifrost/api/asr/status**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        status: "ready",
        ready: true,
        installed: true,
        platform_supported: true,
        ffmpeg_available: true,
        managed: true,
        server_url: "http://127.0.0.1:54321",
        install_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs",
        model_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs/Qwen3-ASR-1.7B",
        model: "Qwen3-ASR-1.7B",
        language: "chinese",
        message: "ready",
      }),
    });
  });

  await page.route(`**/_bifrost/api/asr/tasks/${taskId}`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(taskDetail()),
    });
  });

  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ tasks: [taskDetail()] }),
    });
  });

  await page.route(`**/_bifrost/api/asr/tasks/${taskId}/daily-agent`, async (route) => {
    if (route.request().method() === "PUT") {
      const payload = route.request().postDataJSON() as Record<string, unknown>;
      updates.push(payload);
      dailyAgentConfig = { ...dailyAgentConfig, ...payload };
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ok: true,
        task_id: taskId,
        config: dailyAgentConfig,
        workspace: {
          daily_dir: "/tmp/bifrost-asr-runner-select/daily",
          report_dir: "/tmp/bifrost-asr-runner-select/daily/report",
          agents_path: "/tmp/bifrost-asr-runner-select/daily/AGENTS.md",
          agents_exists: true,
          git_available: true,
          git_initialized: true,
          report_count: 0,
        },
        last_run: {},
      }),
    });
  });

  await page.route(`**/_bifrost/api/asr/tasks/${taskId}/daily-agent/agents`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: taskId,
        content: longInstructions,
        source: "default",
      }),
    });
  });

  await page.route(`**/_bifrost/api/asr/tasks/${taskId}/daily-agent/runs`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ task_id: taskId, processed_documents: [] }),
    });
  });

  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "codex",
        runners: {
          codex: {
            enabled: true,
            adapter: "codex",
            adapterConfig: {},
            injectBifrostTools: true,
            skillPaths: [],
            deliveryMode: "final_reply",
          },
          web: {
            enabled: true,
            adapter: "chatgpt_web",
            adapterConfig: {},
            injectBifrostTools: false,
            skillPaths: [],
            deliveryMode: "final_reply",
          },
        },
        channels: {},
      }),
    });
  });

  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          id: "feishu-main",
          provider_type: "feishu",
          display_name: "Feishu Main",
          enabled: true,
          owner_open_id: "ou_owner",
          event_connection_enabled: true,
          event_types: [],
          created_at: 1,
          updated_at: 1,
        },
      ]),
    });
  });

  await page.route("**/_bifrost/api/im-gateway/targets", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          id: "daily-report-room",
          provider_id: "feishu-main",
          display_name: "Daily Report Room",
          receive_id_type: "chat_id",
          receive_id: "oc_daily_report",
          default_msg_type: "text",
          enabled: true,
          created_at: 1,
          updated_at: 1,
        },
      ]),
    });
  });

  return { updates };
}

test("ASR Daily Agent uses simple Runner and IM Channel dropdowns", async ({ page }) => {
  const { updates } = await installDailyAgentMocks(page);

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent" }).click();

  const instructions = page.getByTestId("asr-daily-agent-instructions");
  await expect(instructions).toHaveValue(longInstructions);
  await expect
    .poll(async () =>
      instructions.evaluate((node) => {
        const textarea = node as HTMLTextAreaElement;
        return textarea.clientHeight + 2 >= textarea.scrollHeight;
      })
    )
    .toBe(true);
  await expect
    .poll(async () =>
      instructions.evaluate((node) => window.getComputedStyle(node).overflowY)
    )
    .toBe("hidden");

  await expect(page.getByText("Runner Type")).toHaveCount(0);
  await expect(page.getByText("Runner ID")).toHaveCount(0);
  const runnerSelect = page.getByTestId("asr-daily-agent-runner-select");
  await expect(runnerSelect).toContainText("Bifrost Agent");

  await runnerSelect.click();
  const dropdown = page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)").last();
  await expect(dropdown).toContainText("Bifrost Agent");
  await expect(dropdown).toContainText("codex");
  await expect(dropdown).toContainText("web");
  await dropdown.getByTitle("web").click();
  await waitForToast(page, "Configuration saved");

  expect(updates.at(-1)).toMatchObject({
    runner: "web",
  });

  const imChannelSelect = page.getByTestId("asr-daily-agent-im-channel-select");
  await expect(page.getByText("Provider ID")).toHaveCount(0);
  await expect(page.getByText("Target ID")).toHaveCount(0);
  await imChannelSelect.click();
  const channelDropdown = page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)").last();
  await expect(channelDropdown).toContainText("Feishu Main / Owner");
  await expect(channelDropdown).toContainText("Daily Report Room");
  await channelDropdown.getByTitle("Daily Report Room (Feishu Main)").click();
  await waitForToast(page, "Configuration saved");

  expect(updates.at(-1)).toMatchObject({
    im_delivery: {
      channel: "target:daily-report-room",
    },
  });
});
