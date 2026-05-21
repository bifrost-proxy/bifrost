import { expect, type Page, test } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

const taskId = "asr-runner-select-test";
const reportDate = "2026-05-14";
const reportPath = `/tmp/bifrost-asr-runner-select/daily/report/${reportDate}-report.md`;
const longInstructions = Array.from(
  { length: 36 },
  (_, index) =>
    `# Daily Agent line ${index + 1}\n\nKeep the ASR daily report concise, actionable, and tied to the transcript evidence.`
).join("\n");
const reportContent = [
  "# Daily Agent Markdown Report",
  "",
  "## Highlights",
  "",
  "- Runner validation passed",
  "- Markdown content is rendered",
  "",
  "| Date | Result |",
  "| --- | --- |",
  `| ${reportDate} | ok |`,
].join("\n");

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
      audio_source_bytes: 0,
      audio_source_file_count: 0,
      cleanable_source_bytes: 0,
      cleanable_source_file_count: 0,
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
        report_index_status: {
          report_files: 2,
          processed_documents: 1,
          indexed_reports: 1,
          unindexed_reports: 1,
          processed_missing_report: 0,
          unindexed_dates: ["2026-05-15"],
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
      body: JSON.stringify({
        task_id: taskId,
        processed_documents: [
          {
            date: "2026-05-13",
            source_sha256: "13ac5f76d34dbb35",
            source_len_bytes: 128_000,
            processed_at_ms: 1_779_126_000_000,
            runner: "web",
            report_path: `/tmp/bifrost-asr-runner-select/daily/report/2026-05-13-report.md`,
            last_run_id: "daily-agent-older-report-run",
          },
          {
            date: reportDate,
            source_sha256: "43ac5f76d34dbb35",
            source_len_bytes: 190_512,
            processed_at_ms: 1_779_212_800_000,
            runner: "web",
            report_path: reportPath,
            last_run_id: "daily-agent-report-run",
          },
          {
            date: "2026-05-16",
            source_sha256: "16ac5f76d34dbb35",
            source_len_bytes: 220_000,
            processed_at_ms: 1_779_385_600_000,
            runner: "web",
            report_path: `/tmp/bifrost-asr-runner-select/daily/report/2026-05-16-report.md`,
            last_run_id: "daily-agent-newest-report-run",
          },
        ],
      }),
    });
  });

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/reports/**`,
    async (route) => {
      if (route.request().url().endsWith(`/reports/${reportDate}`)) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            task_id: taskId,
            task_name: "ASR Runner Select Test",
            date: reportDate,
            path: reportPath,
            size: 24_832,
            modified_ms: 1_779_212_900_000,
            processed_at_ms: 1_779_212_800_000,
            runner: "web",
            last_run_id: "daily-agent-report-run",
            content: reportContent,
          }),
        });
        return;
      }
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error: "ASR Daily Agent report date must use YYYY-MM-DD",
        }),
      });
    },
  );

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
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();

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
  await expect(page.getByText("Indexed Reports")).toBeVisible();
  await expect(page.getByText("1/2")).toBeVisible();
  await expect(page.getByText("Unindexed Reports")).toBeVisible();
  await expect(page.getByText("Unindexed report dates: 2026-05-15")).toBeVisible();
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

test("ASR Daily Agent opens processed reports as full-page Markdown details", async ({
  page,
}) => {
  await installDailyAgentMocks(page);

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent Records", exact: true }).click();
  await expect(page).toHaveURL(/asrTaskTab=daily-agent-records/);

  await page.reload();
  await expect(
    page.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("Run Results")).toBeVisible();
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();
  await expect(page.getByText("Processed Documents")).toHaveCount(0);
  await page.getByRole("tab", { name: "Daily Agent Records", exact: true }).click();
  const runRows = page
    .getByTestId("asr-daily-agent-run-results-table")
    .locator(".ant-table-tbody tr.ant-table-row");
  await expect(runRows.first().locator("td").first()).toHaveText("2026-05-16");

  const reportLink = page.getByTestId(`asr-daily-agent-report-link-${reportDate}`);
  await expect(reportLink).toHaveText(`${reportDate}-report.md`);
  await reportLink.click();

  await expect(page).toHaveURL(new RegExp(`asrDailyReport=${reportDate}`));
  const reportPage = page.getByTestId("asr-daily-agent-report-page");
  await expect(reportPage).toBeVisible();
  await expect(reportPage.getByText(`${reportDate}-report.md`, { exact: true })).toBeVisible();
  await expect(reportPage.getByText("ASR Runner Select Test")).toBeVisible();
  await expect(reportPage.getByText(reportPath, { exact: true })).toBeVisible();
  await expect(reportPage.getByText("web", { exact: true })).toBeVisible();

  const markdown = page.getByTestId("asr-daily-agent-report-content");
  await expect(markdown.locator("h1")).toHaveText("Daily Agent Markdown Report");
  await expect(markdown.locator("li").first()).toHaveText("Runner validation passed");
  await expect(markdown.locator("table")).toContainText(reportDate);

  await page.reload();
  await expect(page).toHaveURL(new RegExp(`asrDailyReport=${reportDate}`));
  await expect(page.getByTestId("asr-daily-agent-report-page")).toBeVisible();
  await expect(page.getByTestId("asr-daily-agent-report-content").locator("h1")).toHaveText(
    "Daily Agent Markdown Report",
  );

  await page.getByLabel("Back to Daily Agent reports").click();
  await expect(page).not.toHaveURL(/asrDailyReport=/);
  await expect(page).toHaveURL(/asrTaskTab=daily-agent-records/);
  await expect(page.getByTestId("asr-task-detail-page")).toBeVisible();
  await expect(
    page.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");

  await openPage(
    page,
    `ai?aiSection=tools-asr&asrTask=${taskId}&asrDailyReport=${reportDate}`,
  );
  await expect(page.getByTestId("asr-daily-agent-report-page")).toBeVisible();
  await page.getByLabel("Back to Daily Agent reports").click();
  await expect(page).toHaveURL(/asrTaskTab=daily-agent-records/);
  await expect(
    page.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});
