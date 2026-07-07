import { expect, test, type Page } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

async function routeAiLayoutApis(page: Page) {
  const imProviders = [
    {
      id: "feishu-main",
      provider_type: "feishu",
      display_name: "Feishu Main",
      app_id: "cli_a1234567890",
      secret_configured: true,
      owner_open_id: "ou_mock_owner",
      enabled: true,
      event_connection_enabled: true,
      agent_config: { runner: "bifrost_agent", work_dir: "/tmp/default-agent" },
    },
    {
      id: "weixin-main",
      provider_type: "weixin",
      display_name: "Weixin Main",
      app_id: "wx_a1234567890",
      secret_configured: false,
      owner_open_id: "",
      enabled: false,
      event_connection_enabled: false,
      agent_config: { runner: "claude_runner" },
    },
  ];
  await page.route("**/_bifrost/api/asr/capabilities", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        platform: "macos",
        arch: "aarch64",
        supported_target: "macos-aarch64",
        qwen3_asr: { enabled: true, hidden: false, platform_supported: true },
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/events**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body: 'retry: 60000\nevent: connected\ndata: {"eventType":"connected"}\n\n',
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "history-thread-1",
            status: "ended",
            running: false,
            title: "Existing thread",
            source: "admin-api",
            runner_type: "codex",
            runner_id: "codex_runner",
            work_dir: "/tmp/workspace",
            start_time: 1_779_700_000,
            last_active_time: 1_779_700_100,
            duration_secs: 100,
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/history-thread-1", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        session_key: "history-thread-1",
        title: "Existing thread",
        source: "admin-api",
        runner_type: "codex",
        runner_id: "codex_runner",
        work_dir: "/tmp/workspace",
        messages: [
          { role: "user", content: "Existing prompt" },
          { role: "assistant", content: "Existing answer" },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ work_dir: "/tmp/default-agent-workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {
          codex_runner: { enabled: true, adapter: "codex" },
          claude_runner: { enabled: true, adapter: "claude_code" },
          traex_runner: { enabled: true, adapter: "traex" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    expect(body.runnerId).toBe("claude_runner");
    expect(body.message).toBe("Summarize current workspace status");
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started"}\n' +
        '{"eventType":"title_updated","title":"Workspace summary"}\n' +
        '{"eventType":"run_finished","response":"Summary complete"}\n',
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/default-agent",
        model_providers: {},
        mcp_servers: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers/*/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: "disconnected", reconnect_count: 0 }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(imProviders),
    });
  });
  for (const path of ["targets", "routes", "schedules", "history/events", "history/runs"]) {
    await page.route(`**/_bifrost/api/im-gateway/${path}`, async (route) => {
      await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
    });
  }
}

async function expectCenteredTrack(
  page: Page,
  contentTestId: string,
  trackTestId: string,
  expectedTopGap = 24,
) {
  const contentBox = await page.getByTestId(contentTestId).boundingBox();
  const trackBox = await page.getByTestId(trackTestId).boundingBox();
  expect(contentBox).not.toBeNull();
  expect(trackBox).not.toBeNull();
  expect(trackBox!.width).toBeLessThanOrEqual(1121);
  expect(trackBox!.width).toBeLessThanOrEqual(contentBox!.width);
  expect(Math.round(trackBox!.y - contentBox!.y)).toBe(expectedTopGap);
  const leftGap = trackBox!.x - contentBox!.x;
  const rightGap = contentBox!.x + contentBox!.width - (trackBox!.x + trackBox!.width);
  expect(Math.abs(leftGap - rightGap)).toBeLessThanOrEqual(2);
}

test.beforeEach(async ({ page }) => {
  await routeAiLayoutApis(page);
});

test("AI layout defaults to new chat with centered composer and runner picker", async ({ page }) => {
  await openPage(page, "ai");

  await expect(page.getByTestId("ai-nav-new-chat")).toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("agent-chat-new-inline-header")).toContainText("How can Bifrost help?");
  await expect(page.getByTestId("agent-chat-thread-list")).toBeVisible();
  await expect(page.locator('[data-testid="agent-chat-thread-item"][data-selected="true"]')).toHaveCount(0);
  const landing = page.getByTestId("ai-new-chat-landing");
  await expect(landing.getByTestId("agent-chat-inline-runner")).toContainText("Codex Runner");

  const sidebarBox = await page.getByTestId("ai-section-nav").boundingBox();
  expect(sidebarBox?.width).toBeGreaterThanOrEqual(210);
  expect(sidebarBox?.width).toBeLessThanOrEqual(230);
  const landingBox = await landing.boundingBox();
  expect(landingBox?.width).toBeGreaterThan(500);
  const inputPill = landing.getByTestId("agent-chat-new-input-pill");
  const inputPillBox = await inputPill.boundingBox();
  expect(inputPillBox?.height).toBeGreaterThanOrEqual(110);
  expect(inputPillBox?.height).toBeLessThanOrEqual(140);
  const composerRadius = await landing.getByTestId("agent-chat-new-input-pill").evaluate((element) => {
    return window.getComputedStyle(element).borderRadius;
  });
  expect(Number.parseFloat(composerRadius)).toBeGreaterThanOrEqual(16);

  const inputBox = await landing.getByTestId("agent-chat-input").boundingBox();
  const toolbarBox = await landing.getByTestId("agent-chat-new-toolbar").boundingBox();
  const runnerBox = await landing.getByTestId("agent-chat-new-runner-row").boundingBox();
  const sendBox = await landing.getByTestId("agent-chat-send").boundingBox();
  expect(inputBox).not.toBeNull();
  expect(toolbarBox).not.toBeNull();
  expect(runnerBox).not.toBeNull();
  expect(sendBox).not.toBeNull();
  expect(inputBox!.y + inputBox!.height).toBeLessThanOrEqual(toolbarBox!.y + 4);
  expect(runnerBox!.y).toBeGreaterThanOrEqual(toolbarBox!.y - 1);
  expect(sendBox!.y).toBeGreaterThanOrEqual(toolbarBox!.y - 1);
  expect(sendBox!.y + sendBox!.height).toBeLessThanOrEqual(toolbarBox!.y + toolbarBox!.height + 1);

  await landing.getByTestId("agent-chat-inline-runner").click();
  const dropdown = page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)").last();
  await expect(dropdown.getByText("Bifrost Agent", { exact: true })).toBeVisible();
  await expect(dropdown.getByText("Claude Code", { exact: true })).toBeVisible();
  await dropdown.getByText("Claude Code", { exact: true }).click();

  await landing.getByTestId("agent-chat-input").fill("Summarize current workspace status");
  await landing.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Summary complete");
  await expect(page).toHaveURL(/session=admin-chat-/);
  await expect(page).not.toHaveURL(/mode=new/);
});

test("AI left rail switches ASR, IM, Settings, and history threads", async ({ page }) => {
  await openPage(page, "ai");

  const existingThread = page.getByTestId("agent-chat-thread-item").filter({ hasText: "Existing thread" });
  const beforeSelectBox = await existingThread.boundingBox();
  expect(beforeSelectBox).not.toBeNull();
  expect(beforeSelectBox!.height).toBeGreaterThanOrEqual(35);
  expect(beforeSelectBox!.height).toBeLessThanOrEqual(37);

  await existingThread.click();
  await expect(page.getByTestId("ai-nav-new-chat")).not.toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");
  const afterSelectBox = await existingThread.boundingBox();
  expect(afterSelectBox).not.toBeNull();
  expect(afterSelectBox!.height).toBe(beforeSelectBox!.height);

  const contentBox = await page.getByTestId("ai-section-content").boundingBox();
  const chatTitleBox = await page.getByTestId("agent-chat-title").boundingBox();
  const composerBox = await page.getByTestId("agent-chat-composer-track").boundingBox();
  const messageTrackBox = await page.getByTestId("agent-chat-message-track").boundingBox();
  expect(contentBox).not.toBeNull();
  expect(chatTitleBox).not.toBeNull();
  expect(composerBox).not.toBeNull();
  expect(messageTrackBox).not.toBeNull();
  expect(Math.round(chatTitleBox!.y - contentBox!.y)).toBeGreaterThanOrEqual(24);
  expect(Math.round(messageTrackBox!.y - contentBox!.y)).toBeGreaterThan(24);
  expect(composerBox!.width).toBeGreaterThan(Math.min(940, contentBox!.width - 80));

  await page.getByTestId("ai-nav-new-chat").click();
  await expect(page.getByTestId("agent-chat-new-inline-header")).toContainText("How can Bifrost help?");

  await page.getByTestId("ai-nav-tools-asr").click();
  await expect(page).toHaveURL(/view=asr/);
  await expect(page.getByTestId("ai-nav-tools-asr")).toHaveAttribute("aria-current", "true");
  await expectCenteredTrack(page, "ai-asr-content", "ai-asr-track");

  await page.getByTestId("ai-nav-im").click();
  await expect(page).toHaveURL(/view=im/);
  await expect(page.getByTestId("ai-nav-im")).toHaveAttribute("aria-current", "true");
  await expectCenteredTrack(page, "ai-im-content", "ai-im-track");
  await expect(page.getByTestId("settings-im-card-grid")).toBeVisible();
  await expect(page.getByTestId("settings-im-provider-card-feishu-main")).toBeVisible();
  await expect(page.getByTestId("settings-im-provider-card-weixin-main")).toBeVisible();
  const imGridDisplay = await page.getByTestId("settings-im-card-grid").evaluate((element) => {
    return window.getComputedStyle(element).display;
  });
  expect(imGridDisplay).toBe("grid");

  await page.getByTestId("ai-nav-tools-videos").click();
  await expect(page).toHaveURL(/view=videos/);
  await expect(page.getByTestId("ai-nav-tools-videos")).toHaveAttribute("aria-current", "true");
  await expectCenteredTrack(page, "ai-videos-content", "ai-videos-track");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page).toHaveURL(/view=settings/);
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByTestId("ai-nav-settings")).toHaveAttribute("aria-current", "true");
  await expect(page.getByRole("tab")).toHaveCount(3);
  await expect(page.getByRole("tab", { name: "Agent" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "Runner" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "IM" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Chat" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Back" })).toHaveCount(0);
  await expect(page.getByText("Session Detail")).toHaveCount(0);
  await expect(page.getByTestId("agent-settings-section-general")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-model")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-runtime")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-runners")).toHaveCount(0);
  await expectCenteredTrack(page, "ai-settings-content", "ai-settings-track");

  await page.getByRole("tab", { name: "Runner" }).click();
  await expect(page).toHaveURL(/settings=agent/);
  await expect(page).toHaveURL(/agentSection=runners/);
  await expect(page.getByTestId("agent-settings-section-runners")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-general")).toHaveCount(0);

  await page.getByRole("tab", { name: "IM" }).click();
  await expect(page.getByTestId("im-gateway-section-connections")).toBeVisible();
  await expect(page.getByTestId("settings-im-card-grid")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-targets")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
});

test("AI layout maps legacy links into the new shell", async ({ page }) => {
  await openPage(page, "ai?aiSection=agent-model&agentSection=model");
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Agent" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("agent-settings-section-model")).toBeVisible();

  await openPage(page, "ai?settings=agent&agentSection=runners");
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Runner" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("agent-settings-section-runners")).toBeVisible();

  await openPage(page, "ai?aiSection=tools-asr");
  await expect(page.getByTestId("ai-nav-tools-asr")).toHaveAttribute("aria-current", "true");

  await openPage(page, "ai?aiSection=im-gateway-routes&imGatewaySection=routes");
  await expect(page.getByTestId("ai-nav-im")).toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
});

test("AI Settings clears conversation route state and only shows configuration tabs", async ({ page }) => {
  await openPage(page, "ai?view=settings&settings=agent&agentSection=chat&session=history-thread-1");

  await expect(page).toHaveURL(/view=settings/);
  await expect(page).not.toHaveURL(/session=history-thread-1/);
  await expect(page).not.toHaveURL(/agentSection=chat/);
  await expect(page).toHaveURL(/agentSection=general/);
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByRole("tab")).toHaveCount(3);
  await expect(page.getByRole("tab", { name: "Agent" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "Chat" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Back" })).toHaveCount(0);
  await expect(page.getByText("Session Detail")).toHaveCount(0);
  await expect(page.getByText("Messages", { exact: true })).toHaveCount(0);

  await openPage(page, "ai?view=chat&session=history-thread-1");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");

  await page.getByTestId("ai-nav-settings").click();

  await expect(page).toHaveURL(/view=settings/);
  await expect(page).not.toHaveURL(/session=history-thread-1/);
  await expect(page).not.toHaveURL(/historyPath=/);
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Agent" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "Chat" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Back" })).toHaveCount(0);
  await expect(page.getByText("Session Detail")).toHaveCount(0);
  await expect(page.getByText("Messages", { exact: true })).toHaveCount(0);
  await expect(page.getByTestId("agent-settings-section-general")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-model")).toBeVisible();

  await page.getByRole("tab", { name: "Runner" }).click();
  await expect(page).toHaveURL(/settings=agent/);
  await expect(page).toHaveURL(/agentSection=runners/);
  await expect(page.getByTestId("agent-settings-section-runners")).toBeVisible();

  await page.getByRole("tab", { name: "IM" }).click();
  await expect(page).toHaveURL(/settings=im/);
  await expect(page).toHaveURL(/imGatewaySection=connections/);
  await expect(page.getByTestId("im-gateway-section-connections")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
});

test("AI Settings does not trap left rail navigation", async ({ page }) => {
  await openPage(page, "ai?view=settings&settings=agent&agentSection=model");
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-model")).toBeVisible();

  await page.getByTestId("ai-nav-tools-asr").click();
  await expect(page).toHaveURL(/view=asr/);
  await expect(page).not.toHaveURL(/settings=/);
  await expect(page.getByTestId("ai-settings-content")).toHaveCount(0);
  await expect(page.getByTestId("ai-nav-tools-asr")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();

  await page.getByTestId("ai-nav-im").click();
  await expect(page).toHaveURL(/view=im/);
  await expect(page).not.toHaveURL(/settings=/);
  await expect(page.getByTestId("ai-settings-content")).toHaveCount(0);
  await expect(page.getByTestId("ai-nav-im")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();

  await page.getByTestId("ai-nav-tools-videos").click();
  await expect(page).toHaveURL(/view=videos/);
  await expect(page).not.toHaveURL(/settings=/);
  await expect(page.getByTestId("ai-settings-content")).toHaveCount(0);
  await expect(page.getByTestId("ai-nav-tools-videos")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();

  await page.getByTestId("ai-nav-new-chat").click();
  await expect(page).toHaveURL(/view=chat/);
  await expect(page).toHaveURL(/mode=new/);
  await expect(page).not.toHaveURL(/settings=/);
  await expect(page.getByTestId("ai-settings-content")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-new-inline-header")).toContainText("How can Bifrost help?");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();

  await page.getByTestId("agent-chat-thread-item").filter({ hasText: "Existing thread" }).click();
  await expect(page).toHaveURL(/view=chat/);
  await expect(page).toHaveURL(/session=history-thread-1/);
  await expect(page).not.toHaveURL(/settings=/);
  await expect(page.getByTestId("ai-settings-content")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");
});

test("AI chat session links open the existing conversation layout", async ({ page }) => {
  await openPage(page, "ai?view=chat&session=history-thread-1");

  await expect(page.getByTestId("ai-new-chat-landing")).toHaveCount(0);
  await expect(page.getByTestId("ai-nav-new-chat")).not.toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");
  await expect(page.getByTestId("agent-chat-composer-track")).toBeVisible();
  await expect(page.getByTestId("agent-chat-settings-open")).toBeVisible();
  await expect(page.getByTestId("agent-chat-new")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-thread-item").filter({ hasText: "Existing thread" })).toHaveAttribute("data-selected", "true");
});

test("AI layout stays usable on narrow viewports", async ({ page }) => {
  for (const viewport of [
    { width: 768, height: 900 },
    { width: 390, height: 844 },
  ]) {
    await page.setViewportSize(viewport);
    await openPage(page, "ai");

    await expect(page.getByTestId("ai-nav-new-chat")).toBeVisible();
    const landing = page.getByTestId("ai-new-chat-landing");
    await expect(landing.getByTestId("agent-chat-input")).toBeVisible();
    await expect(landing.getByTestId("agent-chat-inline-runner")).toBeVisible();
    await landing.getByTestId("agent-chat-inline-runner").click();
    await expect(page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)").last()).toBeVisible();
    await page.keyboard.press("Escape");

    const hasHorizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1);
    expect(hasHorizontalOverflow).toBe(false);

    await page.getByTestId("ai-nav-settings").click();
    await expect(page.getByTestId("ai-settings-content")).toBeVisible();
    await expect(page.getByTestId("agent-settings-section-general")).toBeVisible();
  }
});
