import { expect, test, type Page } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

const runSnapshotTime = Math.floor(Date.now() / 1000);
const runItems = [
  {
    session_key: "run-newer",
    status: "running",
    title: "Summarize weekly project progress",
    runner_id: "codex",
    duration_secs: 0,
    user_message_count: 4,
    source: "feishu",
    start_time: runSnapshotTime - 10 * 60,
  },
  {
    session_key: "run-older",
    status: "completed",
    title: "Generate release summary",
    runner_id: "claude",
    duration_secs: 300,
    user_message_count: 2,
    source: "weixin",
    start_time: runSnapshotTime - 20 * 60,
  },
];

async function routeAiHubApis(page: Page, requestedPaths: string[]) {
  page.on("request", (request) => {
    if (request.url().includes("/api/im-gateway/agent/")) {
      requestedPaths.push(new URL(request.url()).pathname);
    }
  });
  await page.route("**/_bifrost/api/asr/capabilities", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        platform: "macos",
        arch: "aarch64",
        supported_target: "macos-aarch64",
        qwen3_asr: { enabled: true, hidden: false, platform_supported: true },
        local_transcription: {
          enabled: true,
          hidden: false,
          platform_supported: true,
        },
        speech_workbench: {
          enabled: true,
          hidden: false,
          platform_supported: true,
        },
        directory_tasks: {
          enabled: true,
          hidden: false,
          platform_supported: true,
        },
        speaker_diarization: {
          enabled: true,
          hidden: false,
          platform_supported: true,
        },
        voiceprint: { enabled: true, hidden: false, platform_supported: true },
        voice_wake_asr: {
          enabled: true,
          hidden: false,
          platform_supported: true,
        },
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        tasks: [
          { id: "task-1", name: "会议录音", summary: { running: true } },
          { id: "task-2", name: "访谈", summary: { running: false } },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/providers/*/status",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ state: "connected", reconnect_count: 0 }),
      });
    },
  );
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          id: "feishu-main",
          provider_type: "feishu",
          display_name: "飞书主通道",
          enabled: true,
          event_connection_enabled: true,
          event_types: [],
        },
        {
          id: "weixin-main",
          provider_type: "weixin",
          display_name: "微信通道",
          enabled: false,
          event_connection_enabled: false,
          event_types: [],
        },
        {
          id: "feishu-backup",
          provider_type: "feishu",
          display_name: "Feishu Backup",
          enabled: true,
          event_connection_enabled: true,
          event_types: [],
        },
      ]),
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
          codex: { enabled: true, adapter: "codex", adapterConfig: {} },
          claude: { enabled: true, adapter: "claude_code", adapterConfig: {} },
        },
        channels: {},
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/session-summaries**",
    async (route) => {
      const url = new URL(route.request().url());
      const status = url.searchParams.get("status");
      const filtered = status
        ? runItems.filter((item) => item.status === status)
        : runItems;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items:
            url.searchParams.get("limit") === "1"
              ? filtered.slice(0, 1)
              : filtered,
          summary: {
            running_count: filtered.filter((item) => item.status === "running")
              .length,
            total_count: filtered.length,
            active_runners: filtered.some((item) => item.status === "running")
              ? [{ runner_id: "codex", count: 1 }]
              : [],
          },
          next_cursor: null,
          updated_at: runSnapshotTime,
        }),
      });
    },
  );
  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ work_dir: "/tmp/agent", history: {} }),
    });
  });
}

test.beforeEach(async ({ page }) => {
  await routeAiHubApis(page, []);
});

test("AI home presents four stable module cards with operational summaries", async ({
  page,
}) => {
  await openPage(page, "ai");

  const hub = page.getByTestId("ai-module-hub");
  await expect(hub).toBeVisible();
  await expect(page.getByTestId("ai-module-card-asr")).toContainText("2");
  await expect(page.getByTestId("ai-module-card-asr")).toContainText("1");
  await expect(page.getByTestId("ai-module-card-channels")).toContainText(
    "Connected",
  );
  await expect(page.getByTestId("ai-module-card-agents")).toContainText(
    "codex",
  );
  await expect(page.getByTestId("ai-module-card-runs")).toContainText("2");
  await expect(page.getByTestId("ai-module-card-runs")).toContainText(
    "codex × 1",
  );
  await expect(page.getByText("New Chat", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Threads", { exact: true })).toHaveCount(0);

  const first = await page.getByTestId("ai-module-card-asr").boundingBox();
  const second = await page
    .getByTestId("ai-module-card-channels")
    .boundingBox();
  const third = await page.getByTestId("ai-module-card-agents").boundingBox();
  expect(first).not.toBeNull();
  expect(second).not.toBeNull();
  expect(third).not.toBeNull();
  expect(Math.abs(first!.y - second!.y)).toBeLessThanOrEqual(2);
  expect(third!.y).toBeGreaterThan(first!.y + first!.height);
});

test("module details use dedicated routes and always return to AI home", async ({
  page,
}) => {
  for (const [card, path, title] of [
    ["ai-module-card-asr", "asr", "ASR"],
    ["ai-module-card-channels", "channels", "IM Channels"],
    ["ai-module-card-agents", "agents", "Agent Configuration"],
    ["ai-module-card-runs", "runs", "Agent Runs"],
  ]) {
    await openPage(page, "ai");
    const hubBounds = await page.getByTestId("ai-hub-content").boundingBox();
    await page.getByTestId(card).click();
    await expect(page).toHaveURL(new RegExp(`/_bifrost/ai/${path}`));
    await expect(
      page.getByRole("heading", { name: title, exact: true }),
    ).toBeVisible();
    const headerBounds = await page
      .getByTestId("ai-detail-content")
      .boundingBox();
    const bodyBounds = await page.getByTestId("ai-detail-body").boundingBox();
    expect(hubBounds).not.toBeNull();
    expect(headerBounds).not.toBeNull();
    expect(bodyBounds).not.toBeNull();
    expect(
      Math.abs(headerBounds!.width - hubBounds!.width),
    ).toBeLessThanOrEqual(2);
    expect(Math.abs(bodyBounds!.width - hubBounds!.width)).toBeLessThanOrEqual(
      2,
    );
    expect(Math.abs(headerBounds!.x - hubBounds!.x)).toBeLessThanOrEqual(2);
    expect(Math.abs(bodyBounds!.x - hubBounds!.x)).toBeLessThanOrEqual(2);
    const bodyPaddingTop = await page
      .getByTestId("ai-detail-body")
      .evaluate((element) => parseFloat(getComputedStyle(element).paddingTop));
    expect(bodyPaddingTop).toBeGreaterThanOrEqual(24);
    if (path === "channels") {
      const providerCards = page.locator(
        '[data-testid^="settings-im-provider-card-"]',
      );
      await expect(providerCards).toHaveCount(3);
      const providerBoxes = await Promise.all(
        [0, 1, 2].map((index) => providerCards.nth(index).boundingBox()),
      );
      expect(providerBoxes.every(Boolean)).toBe(true);
      expect(
        Math.abs(providerBoxes[0]!.y - providerBoxes[1]!.y),
      ).toBeLessThanOrEqual(2);
      expect(providerBoxes[2]!.y).toBeGreaterThan(
        providerBoxes[0]!.y + providerBoxes[0]!.height,
      );
    }
    if (path === "agents") {
      await expect(
        page.getByTestId("agent-settings-section-general"),
      ).toBeVisible();
      await expect(
        page.getByTestId("agent-settings-section-skills"),
      ).toHaveCount(0);
      await expect(
        page.getByTestId("agent-settings-section-runners"),
      ).toBeVisible();
      await expect(page.getByTestId("agent-chat-section")).toHaveCount(0);
      await expect(
        page.getByTestId("agent-settings-section-history"),
      ).toHaveCount(0);
      await expect(
        page.getByTestId("agent-settings-section-sessions"),
      ).toHaveCount(0);
      await expect(page.getByText("Enable Agent", { exact: true })).toHaveCount(
        0,
      );
      const sectionOrder = await page
        .locator("[data-agent-section]")
        .evaluateAll((sections) =>
          sections.map((section) => section.getAttribute("data-agent-section")),
        );
      expect(sectionOrder).toEqual(["runners", "general"]);
    }
    await page.getByTestId("ai-home-link").click();
    await expect(page).toHaveURL(/\/_bifrost\/ai$/);
    await expect(page.getByTestId("ai-module-hub")).toBeVisible();
  }
});

test("run records are newest-first summaries without drilldown or detail requests", async ({
  page,
}) => {
  const requestedPaths: string[] = [];
  await page.unrouteAll({ behavior: "wait" });
  await routeAiHubApis(page, requestedPaths);
  await openPage(page, "ai/runs");

  const table = page.getByTestId("agent-run-summary-table");
  await expect(table).toContainText("Summarize weekly project progress");
  await expect(table).toContainText("Generate release summary");
  await expect(table).toContainText("codex");
  await expect(table).toContainText("4");
  await expect(table).toContainText("Feishu");
  await expect(table).toContainText("Weixin");
  const rows = table.locator("tbody tr");
  await expect(rows.nth(0)).toContainText("Summarize weekly project progress");
  await expect(rows.nth(0)).toContainText(/10m \d+s/);
  await expect(rows.nth(1)).toContainText("Generate release summary");
  await expect(rows.locator("a")).toHaveCount(0);
  await expect(page.getByText("消息正文", { exact: true })).toHaveCount(0);
  await expect(page.getByText("思考过程", { exact: true })).toHaveCount(0);

  await page.reload();
  await expect(table).toBeVisible();
  await expect(rows.nth(0)).toContainText(/10m \d+s/);

  await page.locator('[aria-label="Run filters"] .ant-select').first().click();
  await page
    .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)")
    .last()
    .getByText("Running", { exact: true })
    .click();
  await expect(page).toHaveURL(/status=running/);
  await expect(rows).toHaveCount(1);
  expect(requestedPaths.some((path) => path.includes("/sessions/all"))).toBe(
    false,
  );
  expect(
    requestedPaths.some((path) => path.includes("/sessions/history")),
  ).toBe(false);
  expect(requestedPaths.some((path) => /\/sessions\/[^/]+$/.test(path))).toBe(
    false,
  );
});

test("mobile layout is single-column and uses non-interactive summary cards", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openPage(page, "ai");
  const cards = [
    page.getByTestId("ai-module-card-asr"),
    page.getByTestId("ai-module-card-channels"),
    page.getByTestId("ai-module-card-agents"),
    page.getByTestId("ai-module-card-runs"),
  ];
  const boxes = await Promise.all(cards.map((card) => card.boundingBox()));
  expect(boxes.every(Boolean)).toBe(true);
  expect(boxes[1]!.y).toBeGreaterThan(boxes[0]!.y + boxes[0]!.height);

  await openPage(page, "ai/channels");
  const providerCards = page.locator(
    '[data-testid^="settings-im-provider-card-"]',
  );
  await expect(providerCards).toHaveCount(3);
  const providerBoxes = await Promise.all(
    [0, 1, 2].map((index) => providerCards.nth(index).boundingBox()),
  );
  expect(providerBoxes.every(Boolean)).toBe(true);
  expect(providerBoxes[1]!.y).toBeGreaterThan(
    providerBoxes[0]!.y + providerBoxes[0]!.height,
  );
  expect(providerBoxes[2]!.y).toBeGreaterThan(
    providerBoxes[1]!.y + providerBoxes[1]!.height,
  );

  await openPage(page, "ai/runs");
  await expect(page.getByTestId("agent-run-summary-list")).toBeVisible();
  await expect(page.getByTestId("agent-run-summary-table")).toHaveCount(0);
  await expect(page.getByTestId("agent-run-summary-list")).toContainText(
    "Started",
  );
  await expect(
    page.getByTestId("agent-run-summary-list").locator("a, button"),
  ).toHaveCount(0);
});

test("AI hub follows both light and dark system themes", async ({ page }) => {
  const backgrounds: string[] = [];
  const providerBackgrounds: string[] = [];
  for (const mode of ["light", "dark"] as const) {
    await page.addInitScript((themeMode) => {
      window.localStorage.setItem(
        "bifrost-theme",
        JSON.stringify({ state: { mode: themeMode }, version: 0 }),
      );
    }, mode);
    await openPage(page, "ai");
    await expect(page.locator("html")).toHaveAttribute("data-theme", mode);
    const card = page.getByTestId("ai-module-card-asr");
    backgrounds.push(
      await card.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    );
    await expect(card).toBeVisible();

    await openPage(page, "ai/channels");
    const providerCard = page.getByTestId(
      "settings-im-provider-card-feishu-main",
    );
    await expect(providerCard).toBeVisible();
    providerBackgrounds.push(
      await providerCard.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    );
  }
  expect(backgrounds[0]).not.toBe(backgrounds[1]);
  expect(providerBackgrounds[0]).not.toBe(providerBackgrounds[1]);
});

test("legacy chat and detail links resolve to the summary-only run list", async ({
  page,
}) => {
  await openPage(
    page,
    "ai?session=legacy-secret-session&historyPath=%2Ftmp%2Fsecret.jsonl",
  );
  await expect(page).toHaveURL(/\/_bifrost\/ai\/runs\?q=legacy-secret-session/);
  await expect(page.getByTestId("agent-run-summaries")).toBeVisible();
  await expect(page.getByTestId("agent-chat-messages")).toHaveCount(0);
});
