import { expect, type Page, test } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

const taskId = "asr-runner-select-test";
const reportDate = "2026-05-14";
const reportPath = `/tmp/bifrost-asr-runner-select/.daily/report/${reportDate}-report.md`;
const longInstructions = Array.from(
  { length: 36 },
  (_, index) =>
    `# Daily Agent line ${index + 1}\n\nKeep the ASR daily report concise, actionable, and tied to the transcript evidence.`,
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
    daily_documents: [
      {
        date: reportDate,
        path: `/tmp/bifrost-asr-runner-select/.daily/${reportDate}.md`,
        size: 148_000,
        text_chars: 72_000,
        modified_ms: 1_779_212_700_000,
      },
      {
        date: "2026-05-13",
        path: "/tmp/bifrost-asr-runner-select/.daily/2026-05-13.md",
        size: 98_000,
        text_chars: 44_000,
        modified_ms: 1_779_126_000_000,
      },
    ],
  };
}

async function installDailyAgentMocks(page: Page) {
  let dailyAgentConfig = {
    enabled: false,
    runner: "codex",
    timeout_ms: 7_200_000,
    trigger_policy: "after_asr_run",
    session_key: undefined as string | undefined,
    instructions_source: "default",
    report_sync_dir: undefined as string | undefined,
    auto_process_from_date: undefined as string | undefined,
    last_report_sync: undefined as
      | {
          target_dir: string;
          total_files: number;
          copied_files: number;
          skipped_files: number;
          failed_files: number;
          synced_at_ms: number;
          errors?: string[];
        }
      | undefined,
    im_delivery: {
      enabled: false,
      channel: undefined as string | undefined,
      mode: "summary",
      send_policy: "on_success_with_report",
    },
    agent_id: "daily_report",
    name: "daily_report",
    output_dir: "daily_report",
    agents: [
      {
        id: "daily_report",
        name: "daily_report",
        enabled: true,
        runner: "codex",
        timeout_ms: 7_200_000,
        trigger_policy: "after_asr_run",
        session_key: undefined as string | undefined,
        chatgpt_project_url: undefined as string | undefined,
        instructions_source: "default",
        im_delivery: {
          enabled: false,
          channel: undefined as string | undefined,
          mode: "summary",
          send_policy: "on_success_with_report",
        },
        output_dir: "daily_report",
      },
    ],
  };
  const updates: Array<Record<string, unknown>> = [];
  const runRequests: string[] = [];
  const templateApplyRequests: Array<Record<string, unknown>> = [];

  const officialTemplate = {
    id: "daily-research",
    version: 1,
    name: "Daily Research",
    description:
      "Generate a daily summary, extract research questions, dispatch independent research, and produce a linked digest.",
    official: true,
    prompt_customizable: true,
    execution_mode: "serial_stages_parallel_research_fanout",
    agents: [],
  };

  await page.route(
    "**/_bifrost/api/asr/daily-agent-templates",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ templates: [officialTemplate] }),
      });
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/apply-template`,
    async (route) => {
      const payload = route.request().postDataJSON() as Record<string, unknown>;
      templateApplyRequests.push(payload);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ok: true,
          template_id: "daily-research",
          config: dailyAgentConfig,
        }),
      });
    },
  );

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

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/sync`,
    async (route) => {
      dailyAgentConfig = {
        ...dailyAgentConfig,
        last_report_sync: {
          target_dir:
            dailyAgentConfig.report_sync_dir ||
            "/tmp/bifrost-asr-runner-select-sync",
          total_files: 2,
          copied_files: 2,
          skipped_files: 0,
          failed_files: 0,
          synced_at_ms: 1_779_212_999_000,
        },
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ok: true,
          sync: dailyAgentConfig.last_report_sync,
        }),
      });
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/run**`,
    async (route) => {
      runRequests.push(route.request().url());
      await route.fulfill({
        status: 202,
        contentType: "application/json",
        body: JSON.stringify({
          status: "queued",
          message: "Daily Agent run queued",
        }),
      });
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily/${reportDate}`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          task_id: taskId,
          task_name: "ASR Runner Select Test",
          date: reportDate,
          path: `/tmp/bifrost-asr-runner-select/.daily/${reportDate}.md`,
          size: 148_000,
          modified_ms: 1_779_212_700_000,
          content: `# ${reportDate} Daily Document\n\nSingle document run source.`,
        }),
      });
    },
  );

  await page.route(
    new RegExp(`.*/_bifrost/api/asr/tasks/${taskId}$`),
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(taskDetail()),
      });
    },
  );

  await page.route(new RegExp(".*/_bifrost/api/asr/tasks$"), async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ tasks: [taskDetail()] }),
    });
  });

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent`,
    async (route) => {
      if (route.request().method() === "PUT") {
        const payload = route.request().postDataJSON() as Record<
          string,
          unknown
        >;
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
            daily_dir: "/tmp/bifrost-asr-runner-select/.daily",
            report_dir: "/tmp/bifrost-asr-runner-select/.daily/report",
            agents_path: "/tmp/bifrost-asr-runner-select/.daily/AGENTS.md",
            agents_exists: true,
            git_available: true,
            git_initialized: true,
            report_count: 0,
            agents: [
              {
                agent_id: "daily_report",
                name: "daily_report",
                output_dir: "daily_report",
                report_dir:
                  "/tmp/bifrost-asr-runner-select/.daily/report/daily_report",
                instructions_path:
                  "/tmp/bifrost-asr-runner-select/.daily/daily_report/AGENTS.md",
                instructions_exists: true,
                report_count: 2,
              },
            ],
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
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/agents**`,
    async (route) => {
      if (route.request().method() === "PUT") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            task_id: taskId,
            agent_id: "daily_report",
            agent_name: "daily_report",
            content:
              route.request().postDataJSON()?.content ?? longInstructions,
            source: "file",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          task_id: taskId,
          agent_id: "daily_report",
          agent_name: "daily_report",
          content: longInstructions,
          source: "default",
        }),
      });
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/runs`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          task_id: taskId,
          processed_documents: [
            {
              date: "2026-05-13",
              agent_id: "daily_report",
              agent_name: "daily_report",
              output_dir: "daily_report",
              source_sha256: "13ac5f76d34dbb35",
              source_len_bytes: 128_000,
              processed_at_ms: 1_779_126_000_000,
              runner: "web",
              report_path: `/tmp/bifrost-asr-runner-select/.daily/report/2026-05-13-report.md`,
              last_run_id: "daily-agent-older-report-run",
            },
            {
              date: reportDate,
              agent_id: "daily_report",
              agent_name: "daily_report",
              output_dir: "daily_report",
              source_sha256: "43ac5f76d34dbb35",
              source_len_bytes: 190_512,
              processed_at_ms: 1_779_212_800_000,
              runner: "web",
              report_path: reportPath,
              last_run_id: "daily-agent-report-run",
            },
            {
              date: "2026-05-16",
              agent_id: "daily_report",
              agent_name: "daily_report",
              output_dir: "daily_report",
              source_sha256: "16ac5f76d34dbb35",
              source_len_bytes: 220_000,
              processed_at_ms: 1_779_385_600_000,
              runner: "web",
              report_path: `/tmp/bifrost-asr-runner-select/.daily/report/2026-05-16-report.md`,
              last_run_id: "daily-agent-newest-report-run",
            },
          ],
        }),
      });
    },
  );

  await page.route(
    `**/_bifrost/api/asr/tasks/${taskId}/daily-agent/reports/**`,
    async (route) => {
      if (
        new URL(route.request().url()).pathname.endsWith(
          `/reports/${reportDate}`,
        )
      ) {
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

  return { updates, runRequests, templateApplyRequests };
}

function waitForDailyAgentConfigReload(page: Page) {
  return page.waitForResponse((response) => {
    const request = response.request();
    const path = new URL(response.url()).pathname;
    return (
      request.method() === "GET" &&
      path.endsWith(`/_bifrost/api/asr/tasks/${taskId}/daily-agent`)
    );
  });
}

test("ASR Daily Agent uses simple Runner and IM Channel dropdowns", async ({
  page,
}) => {
  const { updates } = await installDailyAgentMocks(page);

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();

  await expect(page.getByTestId("asr-daily-agents-table")).toBeVisible();
  await page.getByRole("button", { name: "Add Agent" }).click();
  await waitForToast(page, "Configuration saved");
  await page.getByTestId("asr-daily-agent-edit-custom_agent_2").click();
  await expect(page.getByTestId("asr-daily-agent-detail")).toBeVisible();

  const instructions = page.getByTestId("asr-daily-agent-instructions");
  await expect(instructions).toHaveValue(longInstructions);
  await expect
    .poll(async () =>
      instructions.evaluate((node) => {
        const textarea = node as HTMLTextAreaElement;
        return textarea.clientHeight + 2 >= textarea.scrollHeight;
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      instructions.evaluate((node) => window.getComputedStyle(node).overflowY),
    )
    .toBe("hidden");

  await expect(page.getByText("Runner Type")).toHaveCount(0);
  await expect(page.getByText("Runner ID")).toHaveCount(0);
  await expect(
    page.getByText("Indexed Reports", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("1/2")).toBeVisible();
  await expect(page.getByText("Unindexed Reports")).toBeVisible();
  const runnerSelect = page.getByTestId("asr-daily-agent-runner-select");
  await expect(runnerSelect).toContainText("codex");

  await runnerSelect.click();
  const dropdown = page
    .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)")
    .last();
  await expect(dropdown).toContainText("codex");
  await expect(dropdown).toContainText("codex");
  await expect(dropdown).toContainText("web");
  const runnerConfigReload = waitForDailyAgentConfigReload(page);
  await dropdown.getByTitle("web").click();
  await waitForToast(page, "Configuration saved");
  await runnerConfigReload;

  expect(updates.at(-1)).toMatchObject({
    agents: expect.arrayContaining([
      expect.objectContaining({ id: "custom_agent_2", runner: "web" }),
    ]),
  });

  const imChannelSelect = page.getByTestId("asr-daily-agent-im-channel-select");
  await expect(page.getByText("Provider ID")).toHaveCount(0);
  await expect(page.getByText("Target ID")).toHaveCount(0);
  await imChannelSelect.click();
  const channelDropdown = page
    .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)")
    .last();
  await expect(channelDropdown).toContainText("Feishu Main / Owner");
  await expect(channelDropdown).toContainText("Daily Report Room");
  const channelConfigReload = waitForDailyAgentConfigReload(page);
  await channelDropdown.getByTitle("Daily Report Room (Feishu Main)").click();
  await waitForToast(page, "Configuration saved");
  await channelConfigReload;

  expect(updates.at(-1)).toMatchObject({
    agents: expect.arrayContaining([
      expect.objectContaining({
        id: "custom_agent_2",
        im_delivery: expect.objectContaining({
          channel: "target:daily-report-room",
        }),
      }),
    ]),
  });

  await page.getByRole("button", { name: "Daily Agents" }).click();
  await expect(page.getByTestId("asr-daily-agents-table")).toBeVisible();
  const syncDirInput = page.getByTestId("asr-daily-agent-report-sync-dir");
  const syncDirSaveButton = page.getByTestId(
    "asr-daily-agent-report-sync-dir-save",
  );
  await expect(syncDirInput).toBeEnabled();
  await expect(syncDirSaveButton).toBeDisabled();
  const nextSyncDir = "/tmp/bifrost-asr-runner-select-sync-updated";
  await syncDirInput.fill(nextSyncDir);
  await expect(syncDirInput).toHaveValue(nextSyncDir);
  await expect(syncDirSaveButton).toBeEnabled();
  await syncDirSaveButton.click();
  await waitForToast(page, "Report sync directory saved");

  expect(updates.at(-1)).toMatchObject({
    report_sync_dir: nextSyncDir,
  });

  await page.getByTestId("asr-daily-agent-sync-reports-button").click();
  await waitForToast(page, "Synced 2 copied, 0 skipped");
});

test("ASR Daily Agent saves a ChatGPT Project for independent research", async ({
  page,
}) => {
  const { updates } = await installDailyAgentMocks(page);
  const projectUrl = "https://chatgpt.com/g/g-p-daily-research/project";

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();
  await page.getByTestId("asr-daily-agent-edit-daily_report").click();

  await page.getByTestId("asr-daily-agent-research-fanout-switch").click();
  await waitForToast(page, "Configuration saved");
  await expect(
    page.getByTestId("asr-daily-agent-research-max-concurrency"),
  ).toHaveValue("3");
  const projectInput = page.getByTestId("asr-daily-agent-research-project-url");
  await expect(projectInput).toBeVisible();
  await expect(projectInput).toHaveAttribute(
    "placeholder",
    "https://chatgpt.com/g/g-p-.../project",
  );
  await projectInput.fill(projectUrl);
  expect(updates.at(-1)).not.toMatchObject({
    agents: expect.arrayContaining([
      expect.objectContaining({
        research_fanout: expect.objectContaining({
          chatgpt_project_url: projectUrl,
        }),
      }),
    ]),
  });
  await projectInput.press("Enter");
  await waitForToast(page, "Configuration saved");

  await expect(
    page.getByText(
      "When a Project URL is configured, every new research conversation is created inside that ChatGPT Project.",
      { exact: false },
    ),
  ).toBeVisible();
  expect(updates.at(-1)).toMatchObject({
    agents: expect.arrayContaining([
      expect.objectContaining({
        id: "daily_report",
        research_fanout: expect.objectContaining({
          chatgpt_interface_mode: "chat",
          chatgpt_model: "pro",
          chatgpt_project_url: projectUrl,
        }),
      }),
    ]),
  });
});

test("ASR Daily Agent saves the future-only cutoff and report Project", async ({
  page,
}) => {
  const { updates } = await installDailyAgentMocks(page);
  const projectUrl = "https://chatgpt.com/g/g-p-daily-report/project";

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();

  const cutoff = page.getByTestId("asr-daily-agent-auto-process-from-date");
  await cutoff.fill("2026-07-17");
  await page
    .getByTestId("asr-daily-agent-auto-process-from-date-save")
    .click();
  await waitForToast(page, "Automatic processing start date saved");
  expect(updates.at(-1)).toMatchObject({
    auto_process_from_date: "2026-07-17",
  });

  await page.getByTestId("asr-daily-agent-edit-daily_report").click();
  const projectInput = page.getByTestId("asr-daily-agent-project-url");
  await expect(projectInput).toBeVisible();
  await expect(
    page.getByText("Existing reports are not migrated.", { exact: false }),
  ).toBeVisible();
  await projectInput.fill(projectUrl);
  await projectInput.press("Enter");
  await waitForToast(page, "Configuration saved");

  expect(updates.at(-1)).toMatchObject({
    agents: expect.arrayContaining([
      expect.objectContaining({
        id: "daily_report",
        chatgpt_project_url: projectUrl,
      }),
    ]),
  });
});

test("ASR Daily Agent applies the official customizable research template", async ({
  page,
}) => {
  const { templateApplyRequests } = await installDailyAgentMocks(page);

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();

  const template = page.getByTestId("asr-daily-agent-template-daily-research");
  await expect(template).toBeVisible();
  await expect(template).toContainText("Daily Research (Official Template)");
  await expect(template).toContainText("Every Agent Prompt remains editable");
  await expect(template).toContainText(
    "no personal keywords, repositories, channels, or ChatGPT Project are included",
  );
  const templateBox = await template.boundingBox();
  expect(templateBox?.width).toBeGreaterThan(600);
  expect(templateBox?.height).toBeGreaterThan(100);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(template).toBeVisible();
  await page
    .getByTestId("asr-daily-agent-apply-template-daily-research")
    .click();
  await page
    .locator(".ant-popconfirm-buttons")
    .getByRole("button", { name: "Use template", exact: true })
    .click();
  await waitForToast(page, "Official Daily Research template applied");

  expect(templateApplyRequests).toEqual([
    expect.objectContaining({ template_id: "daily-research" }),
  ]);
  if (process.env.BIFROST_UI_SCREENSHOT_PATH) {
    await page.screenshot({
      path: process.env.BIFROST_UI_SCREENSHOT_PATH,
      fullPage: true,
    });
  }
});

test("ASR Daily Agent opens processed reports as full-page Markdown details", async ({
  page,
}) => {
  await installDailyAgentMocks(page);

  await openPage(page, `ai?aiSection=tools-asr&asrTask=${taskId}`);
  await page
    .getByRole("tab", { name: "Daily Agent Records", exact: true })
    .click();
  await expect(page).toHaveURL(/asrTaskTab=daily-agent-records/);

  await page.reload();
  await expect(
    page.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("Run Results")).toBeVisible();
  await page.getByRole("tab", { name: "Daily Agent", exact: true }).click();
  await expect(page.getByText("Processed Documents")).toHaveCount(0);
  await openPage(
    page,
    `ai?aiSection=tools-asr&asrTask=${taskId}&asrTaskTab=daily-agent-records`,
  );
  await expect(
    page.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("Run Results")).toBeVisible();
  const runRows = page
    .getByTestId("asr-daily-agent-run-results-table")
    .locator(".ant-table-tbody tr.ant-table-row");
  await expect(runRows.first().locator("td").first()).toHaveText("2026-05-16");

  const reportLink = page.getByTestId(
    `asr-daily-agent-report-link-daily_report-${reportDate}`,
  );
  await expect(reportLink).toHaveText(`${reportDate}-report.md`);
  await expect(reportLink).toBeVisible();
  await reportLink.scrollIntoViewIfNeeded();
  await reportLink.click();

  await expect(page).toHaveURL(new RegExp(`asrDailyReport=${reportDate}`));
  const reportPage = page.getByTestId("asr-daily-agent-report-page");
  await expect(reportPage).toBeVisible();
  await expect(
    reportPage.getByText(`${reportDate}-report.md`, { exact: true }),
  ).toBeVisible();
  await expect(reportPage.getByText("ASR Runner Select Test")).toBeVisible();
  await expect(reportPage.getByText(reportPath, { exact: true })).toBeVisible();
  await expect(reportPage.getByText("web", { exact: true })).toBeVisible();

  const markdown = page.getByTestId("asr-daily-agent-report-content");
  await expect(markdown.locator("h1")).toHaveText(
    "Daily Agent Markdown Report",
  );
  await expect(markdown.locator("li").first()).toHaveText(
    "Runner validation passed",
  );
  await expect(markdown.locator("table")).toContainText(reportDate);

  await page.reload();
  await expect(page).toHaveURL(new RegExp(`asrDailyReport=${reportDate}`));
  await expect(page.getByTestId("asr-daily-agent-report-page")).toBeVisible();
  await expect(
    page.getByTestId("asr-daily-agent-report-content").locator("h1"),
  ).toHaveText("Daily Agent Markdown Report");

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

test("ASR Daily Docs row can run Daily Agent for one document", async ({
  page,
}) => {
  const { runRequests } = await installDailyAgentMocks(page);

  await openPage(
    page,
    `ai?aiSection=tools-asr&asrTask=${taskId}&asrTaskTab=daily`,
  );
  await expect(page.getByRole("tab", { name: /Daily Docs/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  await page.getByTestId(`asr-daily-doc-run-agent-${reportDate}`).click();
  await waitForToast(page, "Daily Agent run queued");

  expect(runRequests).toHaveLength(1);
  const runUrl = new URL(runRequests[0]);
  expect(runUrl.pathname).toBe(
    `/_bifrost/api/asr/tasks/${taskId}/daily-agent/run`,
  );
  expect(runUrl.searchParams.get("date")).toBe(reportDate);
  expect(runUrl.searchParams.has("force")).toBe(false);

  await page.getByRole("button", { name: "Open document" }).first().click();
  await expect(page).toHaveURL(new RegExp(`asrDay=${reportDate}`));
  await expect(page.getByTestId("asr-daily-document-content")).toContainText(
    "Single document run source.",
  );
});
