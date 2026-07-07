import { expect, test, type Page } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

async function routeAiLayoutApis(page: Page) {
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
  for (const path of ["providers", "targets", "routes", "schedules", "history/events", "history/runs"]) {
    await page.route(`**/_bifrost/api/im-gateway/${path}`, async (route) => {
      await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
    });
  }
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
  expect(sidebarBox?.width).toBeGreaterThanOrEqual(160);
  expect(sidebarBox?.width).toBeLessThanOrEqual(190);
  const landingBox = await landing.boundingBox();
  expect(landingBox?.width).toBeGreaterThan(500);
  const inputPillBox = await landing.getByTestId("agent-chat-new-input-pill").boundingBox();
  expect(inputPillBox?.height).toBeGreaterThanOrEqual(46);
  expect(inputPillBox?.height).toBeLessThanOrEqual(58);
  const composerRadius = await landing.getByTestId("agent-chat-new-input-pill").evaluate((element) => {
    return window.getComputedStyle(element).borderRadius;
  });
  expect(Number.parseFloat(composerRadius)).toBeGreaterThan(20);

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

  await page.getByTestId("agent-chat-thread-item").filter({ hasText: "Existing thread" }).click();
  await expect(page.getByTestId("ai-nav-new-chat")).not.toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");

  await page.getByTestId("ai-nav-new-chat").click();
  await expect(page.getByTestId("agent-chat-new-inline-header")).toContainText("How can Bifrost help?");

  await page.getByTestId("ai-nav-tools-asr").click();
  await expect(page).toHaveURL(/view=asr/);
  await expect(page.getByTestId("ai-nav-tools-asr")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-im").click();
  await expect(page).toHaveURL(/view=im/);
  await expect(page.getByTestId("ai-nav-im")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-tools-videos").click();
  await expect(page).toHaveURL(/view=videos/);
  await expect(page.getByTestId("ai-nav-tools-videos")).toHaveAttribute("aria-current", "true");

  await page.getByTestId("ai-nav-settings").click();
  await expect(page).toHaveURL(/view=settings/);
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByTestId("ai-nav-settings")).toHaveAttribute("aria-current", "true");
  await expect(page.getByRole("tab", { name: /Agent/ })).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-general")).toBeVisible();
  await page.getByRole("tab", { name: /IM Gateway/ }).click();
  await expect(page.getByTestId("im-gateway-section-connections")).toBeVisible();
});

test("AI layout maps legacy links into the new shell", async ({ page }) => {
  await openPage(page, "ai?aiSection=agent-model&agentSection=model");
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByRole("tab", { name: /Agent/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("agent-settings-section-model")).toBeVisible();

  await openPage(page, "ai?settings=agent&agentSection=runners");
  await expect(page.getByTestId("ai-settings-content")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-runners")).toBeVisible();

  await openPage(page, "ai?aiSection=tools-asr");
  await expect(page.getByTestId("ai-nav-tools-asr")).toHaveAttribute("aria-current", "true");

  await openPage(page, "ai?aiSection=im-gateway-routes&imGatewaySection=routes");
  await expect(page.getByTestId("ai-nav-im")).toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
});

test("AI chat session links open the existing conversation layout", async ({ page }) => {
  await openPage(page, "ai?view=chat&session=history-thread-1");

  await expect(page.getByTestId("ai-new-chat-landing")).toHaveCount(0);
  await expect(page.getByTestId("ai-nav-new-chat")).not.toHaveAttribute("aria-current", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Existing answer");
  await expect(page.getByTestId("agent-chat-composer-track")).toBeVisible();
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
