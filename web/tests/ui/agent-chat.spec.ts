import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

const AGENT_SESSION_EVENTS_ROUTE =
  "**/_bifrost/api/im-gateway/agent/sessions/events*";

async function routeAgentSessionEvents(page: Page, body?: string) {
  await page.unroute(AGENT_SESSION_EVENTS_ROUTE).catch(() => {});
  await page.route(AGENT_SESSION_EVENTS_ROUTE, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        body ||
        'retry: 60000\nevent: connected\ndata: {"eventType":"connected"}\n\n',
    });
  });
}

async function routeBuiltinAgentChatConfig(page: Page) {
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
}

async function pasteImageFiles(page: Page, testId: string, files: Array<{ name: string; type: string; content: string }>) {
  await page.getByTestId(testId).evaluate((element, pastedFiles) => {
    const data = new DataTransfer();
    pastedFiles.forEach((file) => {
      data.items.add(new File([file.content], file.name, { type: file.type }));
    });
    const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: data });
    element.dispatchEvent(event);
  }, files);
}

test.beforeEach(async ({ page }) => {
  await routeAgentSessionEvents(page);
});

test("AI Agent Chat deep link renders local chat preview and composer flow", async ({
  page,
}) => {
  const scrollableResponse = [
    "API run complete [docs](https://example.test/docs)",
    ...Array.from({ length: 60 }, (_, index) =>
      `Scrollable transcript detail ${index + 1}: token HUD layout remains stable while the conversation overflows.`,
    ),
  ].join("\n\n");
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
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
          codex: { enabled: true, adapter: "codex" },
          web: { enabled: true, adapter: "chatgpt_web" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        content: "",
        work_dir: "/tmp/default-agent-workspace",
      }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    const request = route.request();
    expect(request.postDataJSON()).toMatchObject({
      message: "Review the latest diff",
      work_dir: "/tmp/custom-agent-workspace",
    });
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: status\ndata: {"eventType":"status","status":{"state":"tool_execution","current_loop_iteration":1,"max_loop_iterations":4,"total_tokens_used":1234,"estimated_context_tokens":450,"context_window_tokens":1000,"context_usage_percent":45,"compaction_count":1,"work_dir":"/tmp/workspace","message_count":3,"local_tool_count":8,"mcp_tool_count":2,"runner_type":"builtin","runner_id":"bifrost"}}\n\n' +
        'event: context_updated\ndata: {"eventType":"context_updated","context":{"estimatedContextTokens":420,"contextWindowTokens":1000,"contextUsagePercent":42,"compactionCount":1,"historyVersion":7,"messageCount":3,"totalTokensUsed":1234}}\n\n' +
        'event: compaction_started\ndata: {"eventType":"compaction_started","compaction":{"trigger":"auto","reason":"context_limit","phase":"mid_turn","preTokens":920,"compactionCount":1,"historyVersion":7,"context":{"estimatedContextTokens":920,"contextWindowTokens":1000,"contextUsagePercent":92,"compactionCount":1,"historyVersion":7,"messageCount":8}}}\n\n' +
        'event: compaction_finished\ndata: {"eventType":"compaction_finished","compaction":{"trigger":"auto","reason":"context_limit","phase":"mid_turn","preTokens":920,"postTokens":420,"tokensSaved":500,"messagesRemoved":4,"durationMs":12,"compactionCount":2,"historyVersion":8,"context":{"estimatedContextTokens":420,"contextWindowTokens":1000,"contextUsagePercent":42,"compactionCount":2,"historyVersion":8,"messageCount":4}}}\n\n' +
        'event: plan_updated\ndata: {"eventType":"plan_updated","title":"UI telemetry","steps":[{"step":"Gather context","status":"completed"},{"step":"Implement UI","status":"in_progress"}]}\n\n' +
        'event: tool_started\ndata: {"eventType":"tool_started","toolName":"shell","arguments":"pnpm test"}\n\n' +
        'event: tool_finished\ndata: {"eventType":"tool_finished","durationMs":42,"log":{"tool_name":"shell","arguments":"pnpm test","result":"ok","success":true}}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"Tool result checked."}\n\n' +
        `event: run_finished\ndata: ${JSON.stringify({
          eventType: "run_finished",
          response: scrollableResponse,
          planSteps: [
            { step: "Gather context", status: "completed" },
            { step: "Implement UI", status: "completed" },
          ],
          toolCalls: [
            {
              tool_name: "shell",
              arguments: "pnpm test",
              result: "ok",
              success: true,
            },
          ],
        })}\n\n`,
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await expect(page.getByTestId("ai-nav-agent-chat")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-chat-section")).toBeVisible();
  await expect(page.getByTestId("agent-chat-settings-open")).toBeVisible();
  await expect(page.getByTestId("agent-chat-info")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-plan")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-prompt-chips")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Review the latest diff" })).toHaveCount(0);
  const initialSessionHint = await page.getByTestId("agent-chat-input-hint").innerText();
  await page.getByTestId("agent-chat-input").fill("/");
  const compactOption = page
    .getByTestId("agent-chat-slash-command-option")
    .filter({ hasText: "/compact" });
  await expect(compactOption).toContainText("/compact");
  await expect(compactOption).toContainText(
    "立即压缩当前对话上下文",
  );
  await page.getByTestId("agent-chat-input").fill("");
  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByTestId("agent-chat-info")).toContainText(
    "Streaming Agent workspace",
  );
  await expect(page.getByTestId("agent-chat-new")).toBeVisible();
  await expect(page.getByPlaceholder("Working directory")).toHaveValue(
    "/tmp/default-agent-workspace",
  );
  await expect(page.getByText("Run Settings")).toBeVisible();
  await expect(page.getByTestId("agent-chat-empty-state")).toContainText(
    "Start a conversation",
  );
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("agent-chat-settings-modal")).toHaveCount(0);
  await page.getByTestId("agent-chat-new").click();
  await expect(page.getByTestId("agent-chat-new-modal")).toBeVisible();
  await expect(page.getByTestId("agent-chat-new-workspace")).toHaveValue(
    "/tmp/default-agent-workspace",
  );
  await expect(page.getByTestId("agent-chat-new-runner")).toContainText("Bifrost Agent");
  await page.getByTestId("agent-chat-new-workspace").fill("/tmp/custom-agent-workspace");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("agent-chat-new-modal")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-input-hint")).toHaveText(initialSessionHint);
  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByTestId("agent-chat-workspace-display")).toHaveValue(
    "/tmp/custom-agent-workspace",
  );
  await expect(page.getByTestId("agent-chat-workspace-display")).toBeDisabled();
  await page.keyboard.press("Escape");

  const sendButton = page.getByTestId("agent-chat-send");
  await expect(sendButton).toBeDisabled();

  await page.getByTestId("agent-chat-input").fill("Review the latest diff");
  await expect(page.getByTestId("agent-chat-input")).toHaveValue(
    "Review the latest diff",
  );
  await expect(sendButton).toBeEnabled();

  await sendButton.click();
  await expect(page.getByTestId("agent-chat-input")).toHaveValue("");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Review the latest diff",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "API run complete",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Tool result checked.",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "我先执行一步检查。",
  );
  const completedTurnToggle = page.getByTestId("agent-chat-turn-collapse-toggle");
  await expect(completedTurnToggle).toContainText("已处理");
  await expect(completedTurnToggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("agent-chat-process-block")).toHaveCount(0);
  await completedTurnToggle.click();
  await expect(completedTurnToggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Tool result checked.",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "我先执行一步检查。",
  );
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const fallback = Array.from(
          document.querySelectorAll('[data-testid="agent-chat-message-assistant"]'),
        ).find((item) => item.textContent?.includes("我先执行一步检查。"));
        const process = document.querySelector('[data-testid="agent-chat-process-block"]');
        if (!fallback || !process) return false;
        return process.getBoundingClientRect().top > fallback.getBoundingClientRect().top;
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const process = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-process-block"]',
        );
        if (!process) return false;
        const style = window.getComputedStyle(process);
        const box = process.getBoundingClientRect();
        return (
          Number.parseFloat(style.marginTop) === 0 &&
          Number.parseFloat(style.marginBottom) <= 2 &&
          Number.parseFloat(style.paddingTop) === 0 &&
          Number.parseFloat(style.paddingBottom) === 0 &&
          box.height <= 20
        );
      }),
    )
    .toBe(true);
  await expect(page.getByRole("link", { name: "docs" })).toHaveAttribute(
    "target",
    "_blank",
  );
  await expect(page.getByRole("link", { name: "docs" })).toHaveAttribute(
    "rel",
    "noreferrer",
  );
  await expect
    .poll(async () =>
      page
        .getByTestId("agent-chat-message-bubble-assistant")
        .first()
        .evaluate((bubble) => {
          const track = document.querySelector('[data-testid="agent-chat-message-track"]');
          if (!track) return false;
          const bubbleBox = bubble.getBoundingClientRect();
          const trackBox = track.getBoundingClientRect();
          return (
            Math.abs(bubbleBox.left - trackBox.left) < 2 &&
            Math.abs(bubbleBox.right - trackBox.right) < 2
          );
        }),
    )
    .toBe(true);
  await expect(page.getByTestId("agent-chat-source-tag")).toContainText("Web");
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("bifrost");
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await expect(page.getByTestId("agent-chat-token-hud")).toContainText("Tokens");
  await expect(page.getByTestId("agent-chat-token-hud")).toContainText("1.2K");
  await expect(page.getByTestId("agent-chat-token-hud")).toContainText("Context");
  await expect(page.getByTestId("agent-chat-token-hud")).toContainText("45%");
  await expect(page.getByTestId("agent-chat-token-hud")).not.toContainText("Compression");
  await expect(page.getByTestId("agent-chat-compaction-divider")).toHaveCount(1);
  await expect(page.getByTestId("agent-chat-compaction-divider")).toContainText(
    "上下文已自动压缩",
  );
  await expect(page.getByTestId("agent-chat-plan-current")).toContainText("Implement UI");
  await expect(page.getByTestId("agent-chat-plan-more")).toContainText("+1");
  await expect(page.getByTestId("agent-chat-plan")).not.toContainText("Gather context");
  await expect(page.getByTestId("agent-chat-plan-list")).toHaveCount(0);
  await page.getByTestId("agent-chat-plan-toggle").hover();
  await expect(page.getByTestId("agent-chat-plan-popover")).toContainText("Gather context");
  await expect(page.getByTestId("agent-chat-plan-popover")).toContainText("Implement UI");
  await expect(page.getByTestId("agent-chat-plan-popover")).toContainText("Completed");
  await page.mouse.move(0, 0);
  await expect(page.getByTestId("agent-chat-plan-list")).toHaveCount(0);
  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByText("Agent Chat Status")).toBeVisible();
  await expect(page.getByTestId("agent-chat-status")).toContainText("Finished");
  await expect(page.getByTestId("agent-chat-status")).toContainText("Tool Execution");
  await expect(page.getByTestId("agent-chat-tools")).toHaveCount(0);
  await expect(page.getByText("Result: ok")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-context")).toContainText("45%");
  await expect(page.getByTestId("agent-chat-context")).toContainText("Mid Turn");
  await expect(page.getByTestId("agent-chat-context")).toContainText("500");
  await expect(page.getByTestId("agent-chat-context")).toContainText("4");
  await expect(page.getByTestId("agent-chat-context")).toContainText("builtin / bifrost");
  await expect(page.getByText("/tmp/workspace")).toBeVisible();
  await expect(page.getByTestId("agent-chat-errors")).toContainText("No errors");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("agent-chat-settings-modal")).toHaveCount(0);

  // Process steps block (collapsible thinking process) should be visible
  const processBlock = page.getByTestId("agent-chat-process-block");
  await expect(processBlock).toBeVisible();
  // Collapsed by default - shows summary line only
  await expect(processBlock).toContainText("已运行 1 条命令");
  await expect(processBlock).toContainText("1s");
  await expect(processBlock).not.toContainText("pnpm test");
  // Click to expand
  await processBlock.getByText("已运行 1 条命令").click();
  // After expanding, shows individual step summaries
  await expect(processBlock).toContainText("shell");
  await expect(processBlock).not.toContainText("active");
  await expect(processBlock).not.toContainText("Run state: Running");

  await page.setViewportSize({ width: 640, height: 460 });
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const messages = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-messages"]',
        );
        const track = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-message-track"]',
        );
        const section = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-section"]',
        );
        if (!messages || !track || !section) return false;
        const messageBox = messages.getBoundingClientRect();
        const trackBox = track.getBoundingClientRect();
        return (
          section.scrollWidth <= section.clientWidth + 1 &&
          messages.scrollWidth <= messages.clientWidth + 1 &&
          trackBox.left - messageBox.left >= 10 &&
          messageBox.right - trackBox.right >= 10 &&
          trackBox.width <= messageBox.width - 20
        );
      }),
    )
    .toBe(true);

  await page.getByTestId("agent-chat-messages").evaluate((element) => {
    if (element.scrollHeight <= element.clientHeight + 120) {
      throw new Error("agent chat messages did not overflow before scroll control assertion");
    }
    element.scrollTop = 0;
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  const scrollBottomLayer = page.getByTestId("agent-chat-scroll-bottom-layer");
  const scrollBottomButton = page.getByTestId("agent-chat-scroll-bottom");
  await expect(scrollBottomLayer).toHaveCSS("opacity", "1");
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const button = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-scroll-bottom"]',
        );
        const composer = document.querySelector<HTMLElement>(
          '[data-testid="agent-chat-composer-track"]',
        );
        if (!button || !composer) return false;
        const buttonBox = button.getBoundingClientRect();
        const composerBox = composer.getBoundingClientRect();
        const centerDelta = Math.abs(
          (buttonBox.left + buttonBox.right) / 2 -
            (composerBox.left + composerBox.right) / 2,
        );
        return (
          centerDelta < 4 &&
          buttonBox.bottom <= composerBox.top + 12 &&
          buttonBox.bottom >= composerBox.top - 80
        );
      }),
    )
    .toBe(true);
  await scrollBottomButton.click();
  await expect(scrollBottomLayer).toHaveCSS("opacity", "0");
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-messages").evaluate((element) => {
        const distanceFromBottom =
          element.scrollHeight - element.scrollTop - element.clientHeight;
        return distanceFromBottom < 4;
      }),
    )
    .toBe(true);
});

test("AI Agent Chat token HUD stays subtle above the composer", async ({
  page,
}, testInfo) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/agent-token-hud" }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: status\ndata: {"eventType":"status","status":{"state":"model_response","total_tokens_used":2468,"estimated_context_tokens":875,"context_window_tokens":2500,"context_usage_percent":35,"compaction_count":1}}\n\n' +
        'event: context_updated\ndata: {"eventType":"context_updated","context":{"estimatedContextTokens":875,"contextWindowTokens":2500,"contextUsagePercent":35,"compactionCount":1,"totalTokensUsed":2468}}\n\n' +
        'event: compaction_started\ndata: {"eventType":"compaction_started","compaction":{"phase":"pre_turn","compactionCount":1,"context":{"estimatedContextTokens":875,"contextWindowTokens":2500,"contextUsagePercent":35,"compactionCount":1}}}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"HUD telemetry visible"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"HUD telemetry visible"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await expect(page.getByTestId("agent-chat-token-hud")).toHaveCount(0);
  await page.getByTestId("agent-chat-input").fill("Show token HUD");
  await page.getByTestId("agent-chat-send").click();

  const hud = page.getByTestId("agent-chat-token-hud");
  await expect(hud).toContainText("Tokens");
  await expect(hud).toContainText("2.5K");
  await expect(hud).toContainText("Context");
  await expect(hud).toContainText("35%");
  await expect(hud).not.toContainText("Compression");
  await expect(page.getByTestId("agent-chat-input")).toBeVisible();

  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-token-hud").evaluate((element) => {
        const input = document.querySelector('[data-testid="agent-chat-input"]');
        if (!input) return false;
        const hudBox = element.getBoundingClientRect();
        const inputBox = input.getBoundingClientRect();
        return hudBox.bottom <= inputBox.top - 12 && hudBox.height <= 22;
      }),
    )
    .toBe(true);
  await expect(page.getByTestId("agent-chat-token-hud-progress")).toHaveJSProperty(
    "style.width",
    "35%",
  );
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-token-hud-progress").evaluate((element) =>
        getComputedStyle(element).backgroundColor,
      ),
    )
    .not.toBe("rgb(22, 119, 255)");
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const track = document.querySelector('[data-testid="agent-chat-token-hud-track"]');
        const item = document.querySelector('[data-testid="agent-chat-token-hud-item"]');
        const composer = document.querySelector('[data-testid="agent-chat-composer-track"]');
        if (!track || !item || !composer) return false;
        const trackBox = track.getBoundingClientRect();
        const itemBox = item.getBoundingClientRect();
        const composerBox = composer.getBoundingClientRect();
        return (
          trackBox.right <= itemBox.left &&
          trackBox.top < composerBox.top - 12 &&
          itemBox.top < composerBox.top - 12
        );
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-message-track").evaluate((track) => {
        const style = window.getComputedStyle(track);
        return Number.parseFloat(style.paddingBottom) >= 72;
      }),
    )
    .toBe(true);

  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(hud).toContainText("2.5K");
  await expect(hud).toContainText("35%");
  const screenshotPath = testInfo.outputPath("agent-chat-token-hud.png");
  await hud.screenshot({ path: screenshotPath });
  await testInfo.attach("agent-chat-token-hud", {
    path: screenshotPath,
    contentType: "image/png",
  });
});

test("AI Agent Chat runs compact as a control command without chat messages", async ({
  page,
}) => {
  let compactCalls = 0;
  let releaseFirstCompactResponse: (() => void) | undefined;
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
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
          codex: { enabled: true, adapter: "codex" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/agent-compact" }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    const body = route.request().postDataJSON();
    expect(body).toMatchObject({
      message: "/compact",
      work_dir: "/tmp/agent-compact",
    });
    compactCalls += 1;
    if (compactCalls === 1) {
      await new Promise<void>((resolve) => {
        releaseFirstCompactResponse = resolve;
      });
    }
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: compaction_started\ndata: {"eventType":"compaction_started","compaction":{"trigger":"manual","reason":"user_requested","phase":"standalone_turn","preTokens":900,"compactionCount":1,"context":{"estimatedContextTokens":900,"contextWindowTokens":1000,"contextUsagePercent":90,"compactionCount":1}}}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"我先执行一步检查。"}\n\n' +
        'event: tool_started\ndata: {"eventType":"tool_started","toolName":"shell","arguments":"should-not-render"}\n\n' +
        'event: compaction_finished\ndata: {"eventType":"compaction_finished","compaction":{"trigger":"manual","reason":"user_requested","phase":"standalone_turn","preTokens":900,"postTokens":240,"tokensSaved":660,"messagesRemoved":3,"durationMs":12,"compactionCount":2,"context":{"estimatedContextTokens":240,"contextWindowTokens":1000,"contextUsagePercent":24,"compactionCount":2}}}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"上下文已自动压缩"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  const input = page.getByTestId("agent-chat-input");
  await input.fill("/");
  await expect(
    page.getByTestId("agent-chat-slash-command-option").filter({ hasText: "/plan" }),
  ).toHaveAttribute(
    "data-active",
    "true",
  );
  await input.press("ArrowDown");
  await expect(
    page.getByTestId("agent-chat-slash-command-option").filter({ hasText: "/compact" }),
  ).toHaveAttribute("data-active", "true");
  await input.press("Enter");

  await expect.poll(() => compactCalls).toBe(1);
  await expect(page.getByTestId("agent-chat-compaction-divider")).toContainText(
    "上下文正在自动压缩",
  );
  await expect(page.getByTestId("agent-chat-thinking-tail")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    /Running \d+ command/,
  );
  releaseFirstCompactResponse?.();
  await expect(input).toHaveValue("");
  await expect(page.getByTestId("agent-chat-message-bubble-user")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-compaction-divider")).toContainText(
    "上下文已自动压缩",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText("/compact");
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "我先执行一步检查。",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "should-not-render",
  );

  await input.fill("/compact");
  await page.getByTestId("agent-chat-send").click();
  await expect.poll(() => compactCalls).toBe(2);
  await expect(page.getByTestId("agent-chat-message-bubble-user")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText("/compact");
});

test("AI Agent Chat supports slash plan mode from the composer", async ({ page }) => {
  let planCalls = 0;
  let releasePlanResponse: (() => void) | undefined;
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
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
          codex: { enabled: true, adapter: "codex" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/agent-plan" }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    const body = route.request().postDataJSON();
    const requestIndex = planCalls + 1;
    expect(body).toMatchObject({
      message:
        requestIndex === 1
          ? "Create a migration plan"
          : "Create another migration plan",
      collaboration_mode: "plan",
      work_dir: "/tmp/agent-plan",
    });
    expect(body.message).not.toContain("/plan");
    planCalls += 1;
    if (planCalls === 1) {
      await new Promise<void>((resolve) => {
        releasePlanResponse = resolve;
      });
    }
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: proposed_plan\ndata: {"eventType":"proposed_plan","content":"- Inspect current schema\\n- Draft migration steps\\n- Verify rollback"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"","proposedPlan":"- Inspect current schema\\n- Draft migration steps\\n- Verify rollback"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  const input = page.getByTestId("agent-chat-input");
  await input.fill("/");
  await expect(
    page.getByTestId("agent-chat-slash-command-option").filter({ hasText: "/plan" }),
  ).toContainText("进入规划模式");
  await page.getByTestId("agent-chat-slash-command-option").filter({ hasText: "/plan" }).click();
  await expect(input).toHaveValue("");
  await expect(page.getByTestId("agent-chat-plan-mode-pill")).toContainText("Plan Mode");
  await expect(page.getByTestId("agent-chat-input-hint")).toContainText("Planning only");

  await input.fill("Create a migration plan");
  await page.getByTestId("agent-chat-send").click();
  await expect.poll(() => planCalls).toBe(1);
  await expect(page.getByTestId("agent-chat-active-plan-mode")).toContainText("Plan Mode");
  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Stop");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Planning...");
  releasePlanResponse?.();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Create a migration plan",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "/plan Create a migration plan",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Plan Mode result",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Verify rollback",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText("Planning...");
  await expect(page.getByTestId("agent-chat-active-plan-mode")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-plan-mode-pill")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-input-hint")).toHaveText(
    "Shift + Enter for a new line",
  );
  await expect(input).toBeEnabled();
  await expect(input).toHaveValue("");
  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Send");
  await expect(page.getByTestId("agent-chat-send")).toBeDisabled();

  await input.fill("/plan Create another migration plan");
  await page.getByTestId("agent-chat-send").click();
  await expect.poll(() => planCalls).toBe(2);
});

test("AI Agent Chat slash runner call selects a runner and renders the result", async ({
  page,
}) => {
  let runnerCallRequested = false;
  let releaseRunnerCallStream: (() => void) | undefined;
  const runnerCallStreamReady = new Promise<void>((resolve) => {
    releaseRunnerCallStream = resolve;
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: runnerCallRequested
          ? [
              {
                session_key: "runner-call:admin-chat:test:codex",
                status: "active",
                running: true,
                state: "running",
                title: "Runner Call",
                source: "runner_call",
                runner_type: "bifrost_agent",
                last_active_time: 2,
                start_time: 1,
              },
            ]
          : [],
      }),
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
          codex: { enabled: true, adapter: "codex" },
          web: { enabled: true, adapter: "chatgpt_web" },
        },
        channels: {},
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
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/runner-call%3Aadmin-chat%3Atest%3Acodex",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "runner-call:admin-chat:test:codex",
          source: "runner_call",
          state: "idle",
          work_dir: "/tmp/default-agent-workspace",
          runner_type: "codex",
          runner_id: "codex",
          title: "Run with codex",
          messages: [
            {
              role: "user",
              content: "Use the current context from another runner",
              timestamp: 1,
            },
            {
              role: "assistant",
              content: "Codex runner result",
              timestamp: 2,
            },
          ],
        }),
      });
    },
  );
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    const body = route.request().postDataJSON();
    expect(body).toMatchObject({
      sessionKey: "runner-call:admin-chat:test:codex",
      runnerId: "codex",
      adapter: "codex",
      message: "continue inside child thread",
    });
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started"}\n' +
        '{"eventType":"run_finished","response":"Child thread follow-up only"}\n',
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/runner-calls/stream", async (route) => {
    runnerCallRequested = true;
    const body = route.request().postDataJSON();
    expect(body).toMatchObject({
      callerRunnerId: "bifrost_agent",
      callerRunnerAdapter: "bifrost_agent",
      targetRunnerId: "codex",
      message: "Use the current context from another runner",
      workDir: "/tmp/default-agent-workspace",
    });
    expect(body.callerSessionKey).toContain("admin-chat-");
    expect(Array.isArray(body.callerMessages)).toBe(true);
    await runnerCallStreamReady;
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"runner_call_started","callId":"call-1","childSessionKey":"runner-call:admin-chat:test:codex","targetRunnerId":"codex","targetAdapter":"codex"}\n' +
        '{"eventType":"tool_started","title":"codex","content":"reading context"}\n' +
        '{"eventType":"runner_call_finished","callId":"call-1","status":"succeeded","response":"Codex runner result","targetRunnerId":"codex","targetAdapter":"codex"}\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-input").fill("/");
  await expect(page.getByTestId("agent-chat-slash-runner-panel")).toBeVisible();
  await expect(page.getByTestId("agent-chat-slash-runner-panel")).toContainText(
    "Bifrost Agent",
  );
  await page
    .getByTestId("agent-chat-slash-runner-option")
    .filter({ hasText: "codex" })
    .click();
  await expect(page.getByTestId("agent-chat-selected-runner")).toContainText(
    "Run with codex",
  );
  await page
    .getByTestId("agent-chat-input")
    .fill("Use the current context from another runner");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-selected-runner")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Run with codex",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText("running");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Use the current context from another runner",
  );
  releaseRunnerCallStream?.();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Codex runner result",
  );
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText(
    "bifrost_agent",
  );
  await expect(page.getByTestId("agent-chat-thread-list")).not.toContainText(
    "Runner Call",
  );
  await expect(page.getByTestId("agent-chat-thread-item")).toHaveCount(1);
  await page.getByTestId("agent-chat-runner-call-open-child").first().click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Codex runner result",
  );
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("codex");
  await page.getByTestId("agent-chat-input").fill("continue inside child thread");
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Child thread follow-up only",
  );
});

test("AI Agent Chat marks runner-call finished failures as failed", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
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
          codex: { enabled: true, adapter: "codex" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        content: "",
        work_dir: "/tmp/default-agent-workspace",
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/runner-calls/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"runner_call_started","callId":"call-failed","childSessionKey":"runner-call:admin-chat:test:codex","targetRunnerId":"codex","targetAdapter":"codex"}\n' +
        '{"eventType":"runner_call_finished","callId":"call-failed","status":"failed","response":"","targetRunnerId":"codex","targetAdapter":"codex"}\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-input").fill("/");
  await page
    .getByTestId("agent-chat-slash-runner-option")
    .filter({ hasText: "codex" })
    .click();
  await page.getByTestId("agent-chat-input").fill("Run failing codex");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Runner call failed with status: failed",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(/failed|失败/);
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Runner is running...",
  );
});

test("AI Agent Chat marks external runner finished failures as failed", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        content: "",
        work_dir: "/tmp/default-agent-workspace",
      }),
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
          codex: { enabled: true, adapter: "codex" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started","content":"started"}\n' +
        '{"eventType":"assistant_delta","content":"partial output"}\n' +
        '{"eventType":"run_finished","status":"failed","error":"Codex exploded"}\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-new").click();
  await page.getByTestId("agent-chat-new-runner").click();
  await page
    .locator(".ant-select-dropdown .ant-select-item-option-content")
    .filter({ hasText: "codex" })
    .last()
    .click();
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("codex");

  await page.getByTestId("agent-chat-input").fill("Run failing codex");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Codex exploded",
  );
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Error");
  await expect(page.getByTestId("agent-chat-thread-list")).not.toContainText("running");
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Codex should not finish",
  );
});

test("AI Agent Chat keeps plan content compact and scrolls after five steps", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1200, height: 780 });
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "light" }, version: 0 }),
    );
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
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
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: status\ndata: {"eventType":"status","status":{"state":"tool_execution","total_tokens_used":941800,"estimated_context_tokens":32400,"context_window_tokens":100000,"context_usage_percent":32.4}}\n\n' +
        'event: context_updated\ndata: {"eventType":"context_updated","context":{"estimatedContextTokens":32400,"contextWindowTokens":100000,"contextUsagePercent":32.4,"totalTokensUsed":941800}}\n\n' +
        'event: plan_updated\ndata: {"eventType":"plan_updated","title":"Density check","steps":[{"step":"Gather context","status":"completed"},{"step":"Compress every row","status":"in_progress"},{"step":"Keep the long plan item on one compact line without growing the composer vertically","status":"pending"},{"step":"Check five row limit","status":"pending"},{"step":"Preserve hover details","status":"pending"},{"step":"Overflow item six","status":"pending"},{"step":"Overflow item seven","status":"pending"}]}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Done","planSteps":[{"step":"Gather context","status":"completed"},{"step":"Compress every row","status":"in_progress"},{"step":"Keep the long plan item on one compact line without growing the composer vertically","status":"pending"},{"step":"Check five row limit","status":"pending"},{"step":"Preserve hover details","status":"pending"},{"step":"Overflow item six","status":"pending"},{"step":"Overflow item seven","status":"pending"}]}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-input").evaluate((input) => {
        const style = getComputedStyle(input);
        const lineHeight = Number.parseFloat(style.lineHeight);
        const paddingTop = Number.parseFloat(style.paddingTop);
        const paddingBottom = Number.parseFloat(style.paddingBottom);
        return Math.round((input.clientHeight - paddingTop - paddingBottom) / lineHeight);
      }),
    )
    .toBe(2);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-input").evaluate((input) => {
        const hint = document.querySelector(
          '[data-testid="agent-chat-input-hint"]',
        ) as HTMLElement | null;
        const sendButton = document.querySelector(
          '[data-testid="agent-chat-send"]',
        ) as HTMLElement | null;
        const inputBox = input.getBoundingClientRect();
        const hintBox = hint?.getBoundingClientRect();
        const sendBox = sendButton?.getBoundingClientRect();
        const style = getComputedStyle(input);
        return {
          bottomGap: hintBox ? Math.round(inputBox.bottom - hintBox.bottom) : 999,
          paddingTop: Number.parseFloat(style.paddingTop),
          sendBottomGap: sendBox ? Math.round(inputBox.bottom - sendBox.bottom) : 999,
        };
      }),
    )
    .toMatchObject({
      bottomGap: 8,
      paddingTop: 8,
      sendBottomGap: 8,
    });
  await expect(page.getByTestId("agent-chat-input-hint")).toHaveText(
    "Shift + Enter for a new line",
  );
  await page
    .getByTestId("agent-chat-input")
    .fill(Array.from({ length: 10 }, (_, index) => `line ${index + 1}`).join("\n"));
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-input").evaluate((input) => {
        const style = getComputedStyle(input);
        const lineHeight = Number.parseFloat(style.lineHeight);
        const paddingTop = Number.parseFloat(style.paddingTop);
        const paddingBottom = Number.parseFloat(style.paddingBottom);
        return {
          rows: Math.round((input.clientHeight - paddingTop - paddingBottom) / lineHeight),
          scrolls: input.scrollHeight > input.clientHeight,
        };
      }),
    )
    .toMatchObject({
      rows: 7,
      scrolls: true,
    });
  await page.getByTestId("agent-chat-input").fill("");
  await page.getByTestId("agent-chat-input").fill("Show a dense plan");
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-plan-current")).toContainText(
    "Compress every row",
  );
  await expect(page.getByTestId("agent-chat-plan-more")).toContainText("+6");
  await expect(page.getByTestId("agent-chat-plan-list")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-plan")).not.toContainText(
    "Overflow item seven",
  );
  await page.getByTestId("agent-chat-plan-toggle").hover();
  await expect(page.getByTestId("agent-chat-plan-popover")).toContainText("Overflow item seven");
  await expect(page.getByTestId("agent-chat-plan-item")).toHaveCount(7);

  const assertCompactPlan = async () => {
    await page.getByTestId("agent-chat-plan-toggle").hover();
    await expect
      .poll(async () =>
        page.getByTestId("agent-chat-plan-list").evaluate((list) => {
          const listBox = list.getBoundingClientRect();
          const items = Array.from(
            list.querySelectorAll('[data-testid="agent-chat-plan-item"]'),
          );
          const firstItem = items[0] as HTMLElement | undefined;
          const firstText = firstItem?.querySelector(".ant-typography") as HTMLElement | null;
          const itemBox = firstItem?.getBoundingClientRect();
          const visibleRows = items.filter((item) => {
            const box = item.getBoundingClientRect();
            return box.top >= listBox.top - 1 && box.bottom <= listBox.bottom + 1;
          }).length;
          return {
            clientHeight: list.clientHeight,
            firstItemHeight: itemBox?.height ?? 999,
            itemFontSize: firstText ? getComputedStyle(firstText).fontSize : "",
            itemLineHeight: firstText ? getComputedStyle(firstText).lineHeight : "",
            scrolls: list.scrollHeight > list.clientHeight,
            statusTagCount: list.querySelectorAll(".ant-tag").length,
            visibleRows,
          };
        }),
      )
      .toMatchObject({
        clientHeight: 132,
        firstItemHeight: 24,
        itemFontSize: "12px",
        itemLineHeight: "18px",
        scrolls: true,
        statusTagCount: 0,
        visibleRows: 5,
      });
    await expect(page.getByTestId("agent-chat-plan")).not.toContainText("Density check");
    await expect(page.getByTestId("agent-chat-plan-popover")).toContainText("Task progress");
    await expect(page.getByTestId("agent-chat-plan-status-completed")).toHaveCount(1);
    await expect(page.getByTestId("agent-chat-plan-status-in-progress")).toHaveCount(1);
    await expect(page.getByTestId("agent-chat-plan-status-pending")).toHaveCount(5);
    await expect
      .poll(async () =>
        page.getByTestId("agent-chat-plan").evaluate((panel) => {
          const composer = document.querySelector('[data-testid="agent-chat-composer-track"]');
          const hud = document.querySelector('[data-testid="agent-chat-token-hud"]');
          const input = document.querySelector('[data-testid="agent-chat-input"]');
          const popover = document.querySelector('[data-testid="agent-chat-plan-popover"]');
          const panelBox = panel.getBoundingClientRect();
          const composerBox = composer?.getBoundingClientRect();
          const hudBox = hud?.getBoundingClientRect();
          const inputBox = input?.getBoundingClientRect();
          const popoverBox = popover?.getBoundingClientRect();
          return {
            panelIsCompact: panelBox.height <= 34,
            capsuleFloatsAboveComposer:
              Boolean(composerBox) &&
              panelBox.left >= composerBox!.left &&
              panelBox.right <= composerBox!.right &&
              panelBox.bottom <= composerBox!.top - 24,
            capsuleAboveContextHud:
              Boolean(hudBox) && panelBox.bottom <= hudBox!.top - 6,
            inputKeepsNormalTopPadding:
              Boolean(inputBox) && Boolean(composerBox) && inputBox!.top <= composerBox!.top + 24,
            popoverFloatsAboveCapsule:
              Boolean(popoverBox) && popoverBox!.bottom <= panelBox.top - 6,
            popoverWidth: Math.round(popoverBox?.width ?? 999),
          };
        }),
      )
      .toMatchObject({
        capsuleAboveContextHud: true,
        capsuleFloatsAboveComposer: true,
        inputKeepsNormalTopPadding: true,
        panelIsCompact: true,
        popoverFloatsAboveCapsule: true,
      });
  };

  await assertCompactPlan();
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await assertCompactPlan();
  await page.mouse.move(0, 0);
  await expect(page.getByTestId("agent-chat-plan-list")).toHaveCount(0);
});

test("AI Agent Chat keeps conversation content centered at 750px max", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
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

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  for (const testId of ["agent-chat-message-track", "agent-chat-composer-track"]) {
    await expect
      .poll(async () =>
        page.getByTestId(testId).evaluate((element) => {
          const track = element.getBoundingClientRect();
          const container =
            element.closest('[data-testid="agent-chat-messages"]') ??
            element.parentElement;
          const containerBox = container?.getBoundingClientRect();
          const centeredDelta = containerBox
            ? Math.abs(
                track.left - containerBox.left - (containerBox.width - track.width) / 2,
              )
            : 999;
          return track.width <= 780 && track.width > 650 && centeredDelta < 24;
        }),
      )
      .toBe(true);
  }
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-composer-track").evaluate((composer) => {
        const container = composer.closest('[data-testid="agent-chat-messages"]');
        if (!container) return false;
        const composerBox = composer.getBoundingClientRect();
        const containerBox = container.getBoundingClientRect();
        const bottomGap = containerBox.bottom - composerBox.bottom;
        return bottomGap >= 0 && bottomGap <= 18;
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-send").evaluate((sendButton) => {
        const input = document.querySelector('[data-testid="agent-chat-input"]');
        if (!input) return false;
        const inputBox = input.getBoundingClientRect();
        const sendBox = sendButton.getBoundingClientRect();
        return (
          sendBox.left > inputBox.left &&
          sendBox.top > inputBox.top &&
          sendBox.right <= inputBox.right &&
          sendBox.bottom <= inputBox.bottom
        );
      }),
    )
    .toBe(true);
  await expect(page.getByTestId("agent-chat-prompt-chips")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-session-label")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-input-hint")).toContainText(
    "Shift + Enter",
  );
  await expect(page.getByTestId("agent-chat-input-hint")).not.toContainText("Session:");
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-input").evaluate((input) => {
        const inputStyle = getComputedStyle(input);
        const composer = document.querySelector('[data-testid="agent-chat-composer-track"]');
        if (!composer) return false;
        const composerStyle = getComputedStyle(composer);
        return (
          inputStyle.borderTopWidth === "0px" &&
          inputStyle.boxShadow === "none" &&
          composerStyle.borderTopWidth === "1px"
        );
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-new").evaluate((newButton) => {
        const settingsButton = document.querySelector(
          '[data-testid="agent-chat-settings-open"]',
        );
        const title = document.querySelector('[data-testid="agent-chat-title"]');
        const sourceTag = document.querySelector('[data-testid="agent-chat-source-tag"]');
        if (!settingsButton || !title || !sourceTag) return false;
        const titleBox = title.getBoundingClientRect();
        const tagBox = sourceTag.getBoundingClientRect();
        const settingsBox = settingsButton.getBoundingClientRect();
        const newBox = newButton.getBoundingClientRect();
        return Boolean(
          settingsButton.closest(".ant-card-head") &&
            newButton.closest(".ant-card-head") &&
            tagBox.top > titleBox.bottom &&
            Math.abs(
              (settingsBox.top + settingsBox.bottom) / 2 -
                (newBox.top + newBox.bottom) / 2,
            ) < 2,
        );
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-messages").evaluate((container) => {
        return container.querySelectorAll(".ant-avatar").length === 0;
      }),
    )
    .toBe(true);
});

test("AI Agent Chat new chat can select an external runner", async ({ page }) => {
  let externalPayload: Record<string, unknown> | undefined;
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
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
          codex: { enabled: true, adapter: "codex" },
          web: { enabled: true, adapter: "chatgpt_web" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    externalPayload = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started","content":"started"}\n' +
        '{"eventType":"assistant_delta","content":"Inspecting through Codex"}\n' +
        '{"eventType":"run_finished","status":"succeeded","response":"Codex run complete"}\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-new").click();
  await expect(page.getByTestId("agent-chat-new-runner")).toContainText("Bifrost Agent");
  await page.getByTestId("agent-chat-new-runner").click();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("agent-chat-new-runner")).toContainText("codex");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("codex");

  await page.getByTestId("agent-chat-input").fill("/");
  await expect(page.getByTestId("agent-chat-slash-command-option")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-slash-runner-option").first()).toContainText(
    "Bifrost Agent",
  );

  await page.getByTestId("agent-chat-input").fill("Run via Codex");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Codex run complete",
  );
  expect(externalPayload).toMatchObject({
    message: "Run via Codex",
    runnerId: "codex",
    adapter: "codex",
    workDir: "/tmp/default-agent-workspace",
  });
});

test("AI Agent Chat selects the first thread on initial entry", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "first-thread",
            status: "ended",
            title: "First thread title",
            source: "admin-api",
            runner_type: "bifrost_agent",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/first-thread",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "first-thread",
          title: "First thread title",
          source: "admin-api",
          runner_type: "bifrost_agent",
          messages: [
            { role: "user", content: "first thread prompt" },
            { role: "assistant", content: "first thread answer" },
          ],
        }),
      });
    },
  );

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await expect(page).toHaveURL(/session=first-thread/);
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "first thread answer",
  );
  await expect(
    page.getByTestId("agent-chat-thread-item").filter({ hasText: "First thread title" }),
  ).toHaveAttribute("aria-current", "true");
});

test("AI Agent Chat updates the thread list from session events", async ({ page }) => {
  let sessionsAllCalls = 0;
  await page.unroute(AGENT_SESSION_EVENTS_ROUTE);
  await routeAgentSessionEvents(
    page,
    'retry: 60000\nevent: sessions_changed\ndata: {"eventType":"sessions_changed","sessionKey":"incoming-im-loop","reason":"started"}\n\n',
  );
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    sessionsAllCalls += 1;
    const sessions = [
      {
        session_key: "current-web-thread",
        status: "ended",
        title: "Current Web thread",
        source: "admin-api",
        runner_type: "bifrost_agent",
        start_time: 100,
        last_active_time: 120,
      },
    ];
    if (sessionsAllCalls >= 2) {
      sessions.unshift({
        session_key: "incoming-im-loop",
        status: "active",
        running: true,
        title: "Incoming IM loop",
        source: "feishu",
        runner_type: "bifrost_agent",
        run_state: "running",
        start_time: 200,
        last_active_time: 240,
      });
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/current-web-thread",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "current-web-thread",
          title: "Current Web thread",
          source: "admin-api",
          messages: [
            { role: "user", content: "Keep me selected" },
            { role: "assistant", content: "Current reply stays visible" },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=current-web-thread&view=active",
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Current reply stays visible",
  );
  await expect(page.getByRole("button", { name: /Incoming IM loop/ })).toBeVisible({
    timeout: 5000,
  });
  await expect(page.getByRole("button", { name: /Incoming IM loop.*Feishu/ })).toBeVisible();
  await expect(page).toHaveURL(/session=current-web-thread/);
  await expect(
    page.locator('[data-testid="agent-chat-thread-item"][data-selected="true"]'),
  ).toContainText("Current Web thread");
});

test("AI Agent Chat keeps five rounds for each runner in one thread", async ({
  page,
}) => {
  type RunnerCase = {
    id: "bifrost_agent" | "codex" | "web";
    label: string;
    adapter: "bifrost_agent" | "codex" | "chatgpt_web";
  };
  const runnerCases: RunnerCase[] = [
    { id: "bifrost_agent", label: "Bifrost Agent", adapter: "bifrost_agent" },
    { id: "codex", label: "codex", adapter: "codex" },
    { id: "web", label: "web (ChatGPT Web)", adapter: "chatgpt_web" },
  ];
  const sessions = new Map<
    string,
    {
      session_key: string;
      status: string;
      title: string;
      source: string;
      runner_type: string;
      runner_id?: string;
      turns: number;
      start_time: number;
      last_active_time: number;
      duration_secs: number;
    }
  >();
  const requestsByRunner = new Map<string, Array<Record<string, unknown>>>();

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: Array.from(sessions.values()) }),
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
          codex: { enabled: true, adapter: "codex" },
          web: { enabled: true, adapter: "chatgpt_web" },
        },
        channels: {},
      }),
    });
  });

  const rememberRequest = (
    runnerId: string,
    adapter: string,
    sessionKey: string,
    message: string,
  ) => {
    const requests = requestsByRunner.get(runnerId) || [];
    requests.push({ sessionKey, message });
    requestsByRunner.set(runnerId, requests);
    const now = 1_779_800_000 + requests.length;
    const existing = sessions.get(sessionKey);
    sessions.set(sessionKey, {
      session_key: sessionKey,
      status: "ended",
      title: existing?.title || message,
      source: "admin-api",
      runner_type: adapter,
      runner_id: runnerId === "bifrost_agent" ? undefined : runnerId,
      turns: requests.length * 2,
      start_time: existing?.start_time || now,
      last_active_time: now,
      duration_secs: now - (existing?.start_time || now),
    });
    return requests.length;
  };

  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    const payload = route.request().postDataJSON();
    const round = rememberRequest(
      "bifrost_agent",
      "bifrost_agent",
      String(payload.session_key),
      String(payload.message),
    );
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        `event: run_finished\ndata: {"eventType":"run_finished","response":"Bifrost answer ${round}"}\n\n`,
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    const payload = route.request().postDataJSON();
    const runnerId = String(payload.runnerId);
    const adapter = String(payload.adapter);
    const round = rememberRequest(
      runnerId,
      adapter,
      String(payload.sessionKey),
      String(payload.message),
    );
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started","content":"started"}\n' +
        `{"eventType":"run_finished","status":"succeeded","response":"${runnerId} answer ${round}"}\n`,
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  for (const runner of runnerCases) {
    await page.getByTestId("agent-chat-new").click();
    if (runner.id !== "bifrost_agent") {
      await page.getByTestId("agent-chat-new-runner").click();
      await page
        .locator(".ant-select-dropdown .ant-select-item-option-content")
        .filter({ hasText: runner.label })
        .last()
        .click();
    }
    await page.getByRole("button", { name: "Create" }).click();

    let sessionKey = "";
    for (let round = 1; round <= 5; round += 1) {
      const message = `${runner.id} five-turn round ${round}`;
      await page.getByTestId("agent-chat-input").fill(message);
      await page.getByTestId("agent-chat-send").click();
      await expect(page.getByTestId("agent-chat-messages")).toContainText(
        `${runner.id === "bifrost_agent" ? "Bifrost" : runner.id} answer ${round}`,
      );
      const currentKey =
        (await page.getByTestId("agent-chat-input").getAttribute("data-session-key")) ||
        "";
      expect(currentKey).toBeTruthy();
      if (!sessionKey) {
        sessionKey = currentKey;
      }
      expect(currentKey).toBe(sessionKey);
      await expect(page.locator('[data-testid="agent-chat-thread-item"][data-selected="true"]'))
        .toHaveCount(1);
    }

    const requests = requestsByRunner.get(runner.id) || [];
    expect(requests).toHaveLength(5);
    expect(new Set(requests.map((request) => request.sessionKey))).toEqual(
      new Set([sessionKey]),
    );
    expect(Array.from(sessions.values()).filter((item) => item.session_key === sessionKey))
      .toHaveLength(1);
    expect(sessions.get(sessionKey)?.turns).toBe(10);
  }
});

test("AI Agent Chat restores JSONL history and continues with history path", async ({
  page,
}) => {
  const historyPath = "/tmp/bifrost-agent-history.jsonl";
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "user_message",
              session_key: "history-session",
              content: { message: "Earlier question" },
            },
            {
              timestamp: 2,
              event_type: "assistant_message",
              session_key: "history-session",
              content: { message: "Earlier answer" },
            },
          ],
        }),
      });
    },
  );
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    expect(route.request().postDataJSON()).toMatchObject({
      session_key: "history-session",
      history_path: historyPath,
      message: "Continue this thread",
    });
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Continued from history"}\n\n',
    });
  });

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=history-session&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Earlier question",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Earlier answer",
  );
  await page.getByTestId("agent-chat-input").fill("Continue this thread");
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Continued from history",
  );
});

test("AI Agent Chat loads history detail progressively", async ({ page }) => {
  const historyPath = "/tmp/progressive-history.jsonl";
  const historyUrls: string[] = [];
  let unexpectedFullHistoryRequest = false;
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      historyUrls.push(route.request().url());
      const url = new URL(route.request().url());
      if (url.searchParams.get("tail") === "true") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            events: [
              {
                timestamp: 4,
                event_type: "user_message",
                session_key: "progressive-history",
                content: { message: "Newest question" },
              },
              {
                timestamp: 5,
                event_type: "assistant_message",
                session_key: "progressive-history",
                content: { message: "Newest answer" },
              },
            ],
            count: 2,
            total_count: 5,
            start_index: 3,
            end_index: 5,
            next_cursor: 3,
            has_more: true,
          }),
        });
        return;
      }
      if (url.searchParams.get("cursor") === "3") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            events: [
              {
                timestamp: 3,
                event_type: "assistant_message",
                session_key: "progressive-history",
                content: { message: "Middle answer" },
              },
            ],
            count: 1,
            total_count: 5,
            start_index: 2,
            end_index: 3,
            next_cursor: 2,
            has_more: true,
          }),
        });
        return;
      }
      if (url.searchParams.get("cursor") !== "2") {
        unexpectedFullHistoryRequest = true;
      }
      expect(url.searchParams.get("cursor")).toBe("2");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "user_message",
              session_key: "progressive-history",
              content: { message: "Oldest question" },
            },
            {
              timestamp: 2,
              event_type: "assistant_message",
              session_key: "progressive-history",
              content: { message: "Oldest answer" },
            },
          ],
          count: 2,
          total_count: 5,
          start_index: 0,
          end_index: 2,
          next_cursor: 0,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=progressive-history&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Newest answer",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Oldest question",
  );
  expect(new URL(historyUrls[0]).searchParams.get("tail")).toBe("true");
  expect(new URL(historyUrls[0]).searchParams.has("cursor")).toBe(false);
  await page.getByTestId("agent-chat-load-older").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Middle answer",
  );
  await page.getByTestId("agent-chat-load-older").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Oldest question",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Newest answer",
  );
  expect(
    historyUrls.some((url) => new URL(url).searchParams.get("cursor") === "3"),
  ).toBe(true);
  expect(
    historyUrls.some((url) => new URL(url).searchParams.get("cursor") === "2"),
  ).toBe(true);
  expect(unexpectedFullHistoryRequest).toBe(false);
});

test("AI Agent Chat keeps running history token HUD synced with live status", async ({
  page,
}) => {
  const historyPath = "/tmp/running-token-sync.jsonl";
  const liveStatus = {
    state: "waiting_on_session",
    session_key: "running-token-sync",
    total_tokens_used: 29_668_709,
    estimated_context_tokens: 186_727,
    context_window_tokens: 250_000,
    context_usage_percent: 74.7,
    last_response_tokens: 181_900,
    compaction_count: 2,
    history_version: 11,
    message_count: 204,
    user_turn_count: 50,
    work_dir: "/tmp/live-workspace",
    runner_type: "bifrost_agent",
    runner_id: "bifrost",
    agent_type: "Bifrost Agent",
  };

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "running-token-sync",
            status: "active",
            running: true,
            title: "Running token sync",
            history_path: historyPath,
            has_timeline: true,
            run_state: "running",
            turns: liveStatus.message_count,
            tokens: liveStatus.total_tokens_used,
            estimated_tokens: liveStatus.estimated_context_tokens,
            context_window_tokens: liveStatus.context_window_tokens,
            context_usage_percent: liveStatus.context_usage_percent,
            last_response_tokens: liveStatus.last_response_tokens,
            compaction_count: liveStatus.compaction_count,
            work_dir: liveStatus.work_dir,
            runner_type: liveStatus.runner_type,
            runner_id: liveStatus.runner_id,
            agent_type: liveStatus.agent_type,
            source: "feishu",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/running-token-sync",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "running-token-sync",
          title: "Running token sync",
          history_path: historyPath,
          has_timeline: true,
          run_state: "running",
          message_count: 10,
          total_tokens_used: 19_503_264,
          estimated_tokens: 23_448,
          context_window_tokens: 250_000,
          context_usage_percent: 9.4,
          last_response_tokens: 23_448,
          compaction_count: 1,
          active_status: liveStatus,
          messages: [],
        }),
      });
    },
  );
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "run_state_changed",
              session_key: "running-token-sync",
              content: { state: "running", source_channel: "feishu" },
            },
            {
              timestamp: 2,
              event_type: "user_message",
              session_key: "running-token-sync",
              content: { message: "Question before stale token event" },
            },
            {
              timestamp: 3,
              event_type: "assistant_message",
              session_key: "running-token-sync",
              content: {
                message: "Stale answer snapshot",
                total_tokens: 19_503_264,
                context_tokens: 23_448,
              },
            },
            {
              timestamp: 4,
              event_type: "compaction",
              session_key: "running-token-sync",
              content: {
                total_tokens: 19_503_264,
                post_tokens: 23_448,
                compaction_count: 1,
              },
            },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=running-token-sync&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before stale token event",
  );
  const hud = page.getByTestId("agent-chat-token-hud");
  await expect(hud).toContainText("29.7M");
  await expect(hud).toContainText("74.7%");
  await expect(hud).not.toContainText("19.5M");
  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByTestId("agent-chat-context")).toContainText("29.7M");
  await expect(page.getByTestId("agent-chat-context")).toContainText("74.7%");
  await expect(page.getByTestId("agent-chat-context")).toContainText("186.7K / 250K");
});

test("AI Agent Chat continues external runner history with canonical history path", async ({
  page,
}) => {
  const historyPath = "/tmp/external-runner-history.jsonl";
  let externalPayload: Record<string, unknown> | undefined;
  const historyUrls: string[] = [];
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {
          bifrost_agent: { enabled: true, adapter: "builtin" },
          web: { enabled: true, adapter: "chatgpt_web" },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/external-workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "external-history-session",
            status: "ended",
            title: "External runner history",
            history_path: historyPath,
            has_timeline: true,
            runner_id: "web",
            runner_type: "chatgpt_web",
            work_dir: "/tmp/external-workspace",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      historyUrls.push(route.request().url());
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "user_message",
              session_key: "external-history-session",
              content: { message: "Earlier GPT Web question" },
            },
            {
              timestamp: 2,
              event_type: "assistant_message",
              session_key: "external-history-session",
              content: { message: "Earlier GPT Web answer" },
            },
          ],
        }),
      });
    },
  );
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "builtin agent stream should not be used" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    externalPayload = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body:
        '{"eventType":"run_started","content":"started"}\n' +
        '{"eventType":"run_finished","status":"succeeded","response":"External history continued"}\n',
    });
  });

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=external-history-session&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Earlier GPT Web answer",
  );
  expect(new URL(historyUrls[0]).searchParams.get("tail")).toBe("true");
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("web");
  await page.getByTestId("agent-chat-input").fill("Continue via GPT Web");
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "External history continued",
  );
  expect(externalPayload).toMatchObject({
    message: "Continue via GPT Web",
    sessionKey: "external-history-session",
    runnerId: "web",
    adapter: "chatgpt_web",
    workDir: "/tmp/external-workspace",
    params: { historyPath },
  });
});

test("AI Agent Chat restores active session messages after refresh", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function patchedScrollIntoView(arg?: boolean | ScrollIntoViewOptions) {
      const behavior =
        typeof arg === "object" && arg !== null && "behavior" in arg
          ? arg.behavior
          : undefined;
      (window as Window & { __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined> })
        .__agentChatScrollBehaviors = [
        ...((window as Window & { __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined> })
          .__agentChatScrollBehaviors || []),
        behavior,
      ];
      return original.call(this, arg);
    };
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
          session_key: "active-refresh-session",
          status: "active",
          running: false,
          title: "Refresh recovery",
          source: "chatgpt_web",
          work_dir: "/tmp/restored-workspace",
          turns: 2,
          tokens: 321,
          estimated_tokens: 123,
          compaction_count: 1,
          runner_type: "builtin",
          runner_id: "bifrost",
        },
      ],
    }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/active-refresh-session",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "active-refresh-session",
          title: "Refresh recovery",
          work_dir: "/tmp/restored-workspace",
          message_count: 2,
          total_tokens_used: 321,
          estimated_tokens: 123,
          compaction_count: 1,
          source: "chatgpt_web",
          runner_type: "builtin",
          runner_id: "bifrost",
          messages: [
            { role: "user", content: "Question before refresh" },
            { role: "assistant", content: "Answer survived refresh" },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=active-refresh-session&view=active",
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before refresh",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Answer survived refresh",
  );
  await expect(page.getByTestId("agent-chat-title")).toHaveText("Refresh recovery");
  await expect(page.getByTestId("agent-chat-source-tag")).toContainText("Web");
  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("bifrost");
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByPlaceholder("Working directory")).toHaveValue(
    "/tmp/restored-workspace",
  );
  await expect(page.getByTestId("agent-chat-status")).toContainText("2");
  await expect(page.getByTestId("agent-chat-context")).toContainText("321");
  await expect(page.getByTestId("agent-chat-context")).toContainText("builtin / bifrost");
  await page.keyboard.press("Escape");
  const lastScrollBehavior = await page.evaluate(() => {
    const behaviors =
      (
        window as Window & {
          __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined>;
        }
      ).__agentChatScrollBehaviors || [];
    return behaviors[behaviors.length - 1] ?? "auto";
  });
  expect(lastScrollBehavior).toBe("auto");

  await page.reload();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before refresh",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Answer survived refresh",
  );
  await expect(page.getByTestId("agent-chat-title")).toHaveText("Refresh recovery");
  const lastReloadScrollBehavior = await page.evaluate(() => {
    const behaviors =
      (
        window as Window & {
          __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined>;
        }
      ).__agentChatScrollBehaviors || [];
    return behaviors[behaviors.length - 1] ?? "auto";
  });
  expect(lastReloadScrollBehavior).toBe("auto");
});

test("AI Agent Chat falls back to persisted history when active detail is gone", async ({
  page,
}) => {
  const historyPath = "/tmp/fallback-ended-session.jsonl";
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "ended-after-refresh",
            status: "ended",
            title: "First user title",
            history_path: historyPath,
            source: "admin-api",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/ended-after-refresh",
    async (route) => {
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: JSON.stringify({ error: "session not found" }),
      });
    },
  );
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "user_message",
              session_key: "ended-after-refresh",
              content: { message: "Question before process restart" },
            },
            {
              timestamp: 2,
              event_type: "title_updated",
              session_key: "ended-after-refresh",
              content: { title: "Bifrost Edge" },
            },
            {
              timestamp: 3,
              event_type: "plan_updated",
              session_key: "ended-after-refresh",
              content: {
                explanation: "Plan title should not replace session title",
                plan: [{ step: "Keep title stable", status: "completed" }],
              },
            },
            {
              timestamp: 4,
              event_type: "assistant_message",
              session_key: "ended-after-refresh",
              content: { message: "Answer from persisted JSONL" },
            },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=ended-after-refresh&view=active",
  );

  await expect(page).toHaveURL(/view=history/);
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before process restart",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Answer from persisted JSONL",
  );
  await expect(page.getByTestId("agent-chat-title")).toHaveText("Bifrost Edge");
});

test("AI Agent Chat restores active timeline process steps from history path", async ({
  page,
}) => {
  const historyPath = "/tmp/active-timeline-session.jsonl";
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "active-timeline",
            status: "active",
            title: "IM timeline",
            history_path: historyPath,
            has_timeline: true,
            timeline_event_count: 5,
            run_state: "completed",
            source: "im",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/active-timeline",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "active-timeline",
          title: "IM timeline",
          history_path: historyPath,
          has_timeline: true,
          timeline_event_count: 5,
          run_state: "completed",
          messages: [
            { role: "user", content: "IM timeline question", timestamp: 1 },
            { role: "assistant", content: "IM timeline answer", timestamp: 5 },
          ],
        }),
      });
    },
  );
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "run_state_changed",
              session_key: "active-timeline",
              content: { state: "running", source_channel: "im", agent_kind: "builtin" },
            },
            {
              timestamp: 2,
              event_type: "user_message",
              session_key: "active-timeline",
              content: { message: "IM timeline question" },
            },
            {
              timestamp: 2.5,
              event_type: "assistant_delta",
              session_key: "active-timeline",
              content: { message: "I will run the timeline check first." },
            },
            {
              timestamp: 3,
              event_type: "tool_call",
              session_key: "active-timeline",
              content: {
                tool_name: "exec_command",
                arguments: "{\"cmd\":\"pnpm test\"}",
              },
            },
            {
              timestamp: 4,
              event_type: "tool_result",
              session_key: "active-timeline",
              content: {
                tool_name: "exec_command",
                result: "ok",
                success: true,
              },
            },
            {
              timestamp: 4.5,
              event_type: "assistant_delta",
              session_key: "active-timeline",
              content: { message: "The command completed successfully." },
            },
            {
              timestamp: 5,
              event_type: "assistant_message",
              session_key: "active-timeline",
              content: { message: "IM timeline answer" },
            },
            {
              timestamp: 5,
              event_type: "run_state_changed",
              session_key: "active-timeline",
              content: { state: "completed", source_channel: "im", agent_kind: "builtin" },
            },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=active-timeline&view=active",
  );

  await expect(page).not.toHaveURL(/view=history/);
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "IM timeline question",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "IM timeline answer",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "I will run the timeline check first.",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "The command completed successfully.",
  );
  const completedTurnToggle = page.getByTestId("agent-chat-turn-collapse-toggle");
  await expect(completedTurnToggle).toContainText("已处理 3s");
  await expect(completedTurnToggle).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("agent-chat-process-block")).toHaveCount(0);
  await completedTurnToggle.click();
  await expect(completedTurnToggle).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "I will run the timeline check first.",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "The command completed successfully.",
  );
  const firstDeltaMessage = page
    .getByTestId("agent-chat-message-assistant")
    .filter({ hasText: "I will run the timeline check first." });
  const secondDeltaMessage = page
    .getByTestId("agent-chat-message-assistant")
    .filter({ hasText: "The command completed successfully." });
  const finalAssistantMessage = page
    .getByTestId("agent-chat-message-assistant")
    .filter({ hasText: "IM timeline answer" });
  await expect(firstDeltaMessage.getByTestId("agent-chat-message-time")).toHaveCount(0);
  await expect(secondDeltaMessage.getByTestId("agent-chat-message-time")).toHaveCount(0);
  await expect(finalAssistantMessage.getByTestId("agent-chat-message-time")).toHaveCount(1);
  const processBlock = page.getByTestId("agent-chat-process-block");
  await expect(processBlock).toBeVisible();
  await expect(processBlock).toContainText("已运行 1 条命令");
  await expect(processBlock).toContainText("1s");
  await expect(processBlock).not.toContainText("pnpm test");
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const assistantMessages = Array.from(
          document.querySelectorAll('[data-testid="agent-chat-message-assistant"]'),
        );
        const firstDelta = assistantMessages.find((item) =>
          item.textContent?.includes("I will run the timeline check first."),
        );
        const process = document.querySelector('[data-testid="agent-chat-process-block"]');
        const secondDelta = assistantMessages.find((item) =>
          item.textContent?.includes("The command completed successfully."),
        );
        const finalAssistant = assistantMessages.find((item) =>
          item.textContent?.includes("IM timeline answer"),
        );
        if (!firstDelta || !process || !secondDelta || !finalAssistant) return false;
        return (
          firstDelta.getBoundingClientRect().top <
            process.getBoundingClientRect().top &&
          process.getBoundingClientRect().top <
            secondDelta.getBoundingClientRect().top &&
          secondDelta.getBoundingClientRect().top <
            finalAssistant.getBoundingClientRect().top
        );
      }),
    )
    .toBe(true);
  await processBlock.getByRole("button").click();
  await expect(processBlock).not.toContainText("Run state: Running");
  await expect(processBlock).toContainText("exec_command");
  await processBlock.getByText("exec_command").click();
  await expect(processBlock).toContainText("pnpm test");
  await expect(processBlock).toContainText("ok");
});

test("AI Agent Chat updates running history timeline from SSE without cross-thread mixing", async ({
  page,
}) => {
  const historyPath = "/tmp/running-refresh-session.jsonl";
  const otherHistoryPath = "/tmp/other-running-session.jsonl";
  const runningStartedAt = Math.floor(Date.now() / 1000) - 3;
  let historyCalls = 0;
  await routeAgentSessionEvents(
    page,
    [
      'event: timeline_changed',
      `data: ${JSON.stringify({
        eventType: "timeline_changed",
        sessionKey: "other-running-refresh",
        historyPath: otherHistoryPath,
        endIndex: 99,
      })}`,
      '',
      'event: timeline_changed',
      `data: ${JSON.stringify({
        eventType: "timeline_changed",
        sessionKey: "running-refresh",
        historyPath,
        endIndex: 7,
      })}`,
      '',
    ].join("\n"),
  );
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "running-refresh",
            status: "active",
            running: true,
            title: "Running refresh",
            history_path: historyPath,
            has_timeline: true,
            timeline_event_count: 2,
            run_state: "running",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      historyCalls += 1;
      const requestUrl = new URL(route.request().url());
      const initialEvents = [
        {
          timestamp: runningStartedAt - 10,
          event_type: "user_message",
          session_key: "running-refresh",
          content: { message: "Previous question" },
        },
        {
          timestamp: runningStartedAt - 9,
          event_type: "assistant_message",
          session_key: "running-refresh",
          content: { message: "Previous answer" },
        },
        {
          timestamp: runningStartedAt,
          event_type: "user_message",
          session_key: "running-refresh",
          content: { message: "Question before refresh" },
        },
      ];
      const incrementalEvents = [
        {
          timestamp: runningStartedAt + 0.5,
          event_type: "assistant_delta",
          session_key: "running-refresh",
          content: { message: "I am checking the refreshed run state." },
        },
        {
          timestamp: runningStartedAt + 1,
          event_type: "tool_call",
          session_key: "running-refresh",
          content: {
            tool_name: "exec_command",
            arguments: "{\"cmd\":\"cargo test\"}",
          },
        },
        {
          timestamp: runningStartedAt + 2,
          event_type: "tool_result",
          session_key: "running-refresh",
          content: {
            tool_name: "exec_command",
            result: "still running output",
            success: true,
          },
        },
        {
          timestamp: runningStartedAt + 2.5,
          event_type: "assistant_delta",
          session_key: "running-refresh",
          content: { message: "The command is still being observed." },
        },
      ];
      const isIncremental = requestUrl.searchParams.has("since");
      const events = isIncremental ? incrementalEvents : [...initialEvents, ...incrementalEvents];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events,
          start_index: isIncremental ? 3 : 0,
          end_index: 7,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=running-refresh&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before refresh",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "I am checking the refreshed run state.",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "The command is still being observed.",
  );
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "other-running-refresh",
  );
  const runningSummary = page.getByTestId("agent-chat-turn-running-summary");
  await expect(runningSummary).toContainText(/已处理 \d+s/);
  const firstRunningSummary = await runningSummary.textContent();
  await page.waitForTimeout(1100);
  await expect
    .poll(async () => {
      const next = await runningSummary.textContent();
      return Boolean(next && firstRunningSummary && next !== firstRunningSummary);
    })
    .toBe(true);
  await expect(page.getByTestId("agent-chat-thinking-tail")).toBeVisible();
  const processBlock = page.getByTestId("agent-chat-process-block");
  await expect(processBlock).toBeVisible();
  await expect(processBlock).not.toContainText("cargo test");
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const users = Array.from(
          document.querySelectorAll('[data-testid="agent-chat-message-user"]'),
        );
        const assistants = Array.from(
          document.querySelectorAll('[data-testid="agent-chat-message-assistant"]'),
        );
        const lastUser = users.at(-1);
        const lastAssistant = assistants.at(-1);
        if (!lastUser || !lastAssistant) return false;
        return (
          lastAssistant.getBoundingClientRect().top >
          lastUser.getBoundingClientRect().top
        );
      }),
    )
    .toBe(true);
  await processBlock.getByRole("button").click();
  await expect(processBlock).not.toContainText("Run state: Running");
  await expect(processBlock).toContainText("exec_command");
  await processBlock.getByText("exec_command").click();
  await expect(processBlock).toContainText("cargo test");
  await expect(processBlock).toContainText("still running output");
  expect(historyCalls).toBeGreaterThan(0);
  expect(historyCalls).toBeLessThanOrEqual(3);
});

test("AI Agent Chat lets completed history override stale running thread summary", async ({
  page,
}) => {
  const historyPath = "/tmp/stale-running-completed.jsonl";
  const startedAt = Math.floor(Date.now() / 1000) - 20;
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "traex",
        runners: { traex: { enabled: true, adapter: "traex" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "stale-running-completed",
            status: "active",
            running: true,
            title: "Stale running completed",
            history_path: historyPath,
            has_timeline: true,
            run_state: "running",
            runner_id: "traex",
            runner_type: "traex",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: startedAt,
              event_type: "session_start",
              session_key: "stale-running-completed",
              content: { runner_id: "traex", runtime: "external_cli", source: "admin-api" },
            },
            {
              timestamp: startedAt + 1,
              event_type: "run_state_changed",
              session_key: "stale-running-completed",
              content: { state: "running", agent_kind: "traex", source_channel: "web" },
            },
            {
              timestamp: startedAt + 2,
              event_type: "user_message",
              session_key: "stale-running-completed",
              content: { message: "Queued message" },
            },
            {
              timestamp: startedAt + 3,
              event_type: "assistant_delta",
              session_key: "stale-running-completed",
              content: { message: "Completed from queued run." },
            },
            {
              timestamp: startedAt + 4,
              event_type: "run_state_changed",
              session_key: "stale-running-completed",
              content: { state: "completed", agent_kind: "traex", source_channel: "web" },
            },
            {
              timestamp: startedAt + 5,
              event_type: "assistant_message",
              session_key: "stale-running-completed",
              content: { message: "Completed from queued run." },
            },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=stale-running-completed&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("traex");
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Send");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Completed from queued run.",
  );
  await expect(page.getByTestId("agent-chat-thinking-tail")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-process-block")).toHaveCount(0);
  await expect(page.getByText("Completed from queued run.")).toHaveCount(1);
  await page.getByTestId("agent-chat-turn-collapse-toggle").click();
  await expect(page.getByText("Completed from queued run.")).toHaveCount(1);
});

test("AI Agent Chat stops running history from terminal SSE without hot thread refresh", async ({
  page,
}) => {
  const historyPath = "/tmp/running-summary-stops.jsonl";
  const startedAt = Math.floor(Date.now() / 1000) - 10;
  let sessionListRequests = 0;
  let incrementalHistoryRequests = 0;
  await routeAgentSessionEvents(
    page,
    [
      'event: timeline_changed',
      `data: ${JSON.stringify({
        eventType: "timeline_changed",
        sessionKey: "running-summary-stops",
        historyPath,
        endIndex: 6,
      })}`,
      '',
    ].join("\n"),
  );
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "traex",
        runners: { traex: { enabled: true, adapter: "traex" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    sessionListRequests += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "running-summary-stops",
            status: "active",
            running: true,
            title: "Running summary stops",
            history_path: historyPath,
            has_timeline: true,
            run_state: "running",
            state: "running",
            runner_id: "traex",
            runner_type: "traex",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      const requestUrl = new URL(route.request().url());
      const terminalEvents = [
        {
          timestamp: startedAt + 4,
          event_type: "run_state_changed",
          session_key: "running-summary-stops",
          content: { state: "completed", agent_kind: "traex", source_channel: "web" },
        },
        {
          timestamp: startedAt + 5,
          event_type: "assistant_message",
          session_key: "running-summary-stops",
          content: { message: "The run is done." },
        },
      ];
      if (requestUrl.searchParams.has("since")) {
        incrementalHistoryRequests += 1;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            events: terminalEvents,
            start_index: 4,
            end_index: 6,
            has_more: false,
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: startedAt,
              event_type: "session_start",
              session_key: "running-summary-stops",
              content: {
                runner_id: "traex",
                runner_type: "traex",
                runtime: "external_cli",
                source: "admin-api",
              },
            },
            {
              timestamp: startedAt + 1,
              event_type: "run_state_changed",
              session_key: "running-summary-stops",
              content: { state: "running", agent_kind: "traex", source_channel: "web" },
            },
            {
              timestamp: startedAt + 2,
              event_type: "user_message",
              session_key: "running-summary-stops",
              content: { message: "Will the spinner stop?" },
            },
            {
              timestamp: startedAt + 3,
              event_type: "assistant_delta",
              session_key: "running-summary-stops",
              content: { message: "The model already produced a final-looking update." },
            },
            ...terminalEvents,
          ],
          start_index: 0,
          end_index: 6,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=running-summary-stops&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Send");
  await expect(page.getByTestId("agent-chat-thinking-tail")).toHaveCount(0);
  expect(incrementalHistoryRequests).toBeLessThanOrEqual(1);
  const listRequestsAfterStop = sessionListRequests;
  await page.waitForTimeout(1800);
  expect(sessionListRequests).toBe(listRequestsAfterStop);
});

test("AI Agent Chat ignores stale running history responses after switching threads", async ({
  page,
}) => {
  const historyA = "/tmp/switch-running-a.jsonl";
  const historyB = "/tmp/switch-running-b.jsonl";
  let releaseA: (() => void) | undefined;
  const holdA = new Promise<void>((resolve) => {
    releaseA = resolve;
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "traex",
        runners: { traex: { enabled: true, adapter: "traex" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "switch-a",
            status: "active",
            running: true,
            title: "Switch A",
            history_path: historyA,
            has_timeline: true,
            run_state: "running",
            runner_id: "traex",
            runner_type: "traex",
            source: "web",
          },
          {
            session_key: "switch-b",
            status: "active",
            running: true,
            title: "Switch B",
            history_path: historyB,
            has_timeline: true,
            run_state: "running",
            runner_id: "traex",
            runner_type: "traex",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/history/**", async (route) => {
    const requestUrl = route.request().url();
    const isA = requestUrl.includes(encodeURIComponent(historyA));
    if (isA) {
      await holdA;
    }
    const sessionKey = isA ? "switch-a" : "switch-b";
    const marker = isA ? "STALE_A_RESPONSE" : "CURRENT_B_RESPONSE";
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        events: [
          {
            timestamp: Math.floor(Date.now() / 1000),
            event_type: "user_message",
            session_key: sessionKey,
            content: { message: `${marker} user` },
          },
          {
            timestamp: Math.floor(Date.now() / 1000) + 1,
            event_type: "assistant_delta",
            session_key: sessionKey,
            content: { message: `${marker} assistant` },
          },
        ],
        start_index: 0,
        end_index: 2,
        has_more: false,
      }),
    });
  });

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=switch-a&view=history&historyPath=${encodeURIComponent(historyA)}`,
  );
  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=switch-b&view=history&historyPath=${encodeURIComponent(historyB)}`,
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText("CURRENT_B_RESPONSE");
  releaseA?.();
  await page.waitForTimeout(500);
  await expect(page.getByTestId("agent-chat-messages")).toContainText("CURRENT_B_RESPONSE");
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText("STALE_A_RESPONSE");
});

test("AI Agent Chat active view restores same-session running history path", async ({
  page,
}) => {
  const historyPath = "/tmp/active-sse-history.jsonl";
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "web",
        runners: { web: { enabled: true, adapter: "chatgpt_web" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "active-sse-session",
            status: "active",
            running: true,
            title: "Active SSE session",
            history_path: historyPath,
            has_timeline: true,
            run_state: "running",
            runner_id: "web",
            runner_type: "chatgpt_web",
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/active-sse-session", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        session_key: "active-sse-session",
        source: "web",
        runner_id: "web",
        runner_type: "chatgpt_web",
        history_path: historyPath,
        has_timeline: true,
        run_state: "running",
        messages: [],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: Math.floor(Date.now() / 1000),
              event_type: "user_message",
              session_key: "active-sse-session",
              content: { message: "Active SSE question" },
            },
            {
              timestamp: Math.floor(Date.now() / 1000) + 1,
              event_type: "assistant_delta",
              session_key: "active-sse-session",
              content: { message: "Active SSE running update" },
            },
          ],
          start_index: 0,
          end_index: 2,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=active-sse-session&view=active",
  );

  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText("web");
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Running");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Active SSE running update",
  );
});

test("AI Agent Chat lets idle thread summary override stale running history on initial load", async ({
  page,
}) => {
  const historyPath = "/tmp/initial-idle-stale-running.jsonl";
  const startedAt = Math.floor(Date.now() / 1000) - 30;
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "initial-idle-stale-running",
            status: "active",
            running: false,
            state: "idle",
            run_state: "idle",
            title: "Initial idle stale running",
            history_path: historyPath,
            has_timeline: true,
            source: "web",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: startedAt,
              event_type: "run_state_changed",
              session_key: "initial-idle-stale-running",
              content: { state: "running", agent_kind: "builtin", source_channel: "web" },
            },
            {
              timestamp: startedAt + 1,
              event_type: "user_message",
              session_key: "initial-idle-stale-running",
              content: { message: "stale question" },
            },
          ],
          start_index: 0,
          end_index: 2,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=initial-idle-stale-running&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await expect(page.getByTestId("agent-chat-turn-running-group")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-thinking-tail")).toHaveCount(0);
});

test("AI Agent Chat lets active detail idle run_state override stale running history", async ({
  page,
}) => {
  const historyPath = "/tmp/active-detail-idle-stale-running.jsonl";
  const startedAt = Math.floor(Date.now() / 1000) - 30;
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/active-detail-idle-stale-running",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "active-detail-idle-stale-running",
          status: "active",
          run_state: "idle",
          title: "Active detail idle",
          history_path: historyPath,
          has_timeline: true,
          source: "web",
          messages: [],
        }),
      });
    },
  );
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: startedAt,
              event_type: "run_state_changed",
              session_key: "active-detail-idle-stale-running",
              content: { state: "running", agent_kind: "builtin", source_channel: "web" },
            },
            {
              timestamp: startedAt + 1,
              event_type: "user_message",
              session_key: "active-detail-idle-stale-running",
              content: { message: "stale active detail question" },
            },
          ],
          start_index: 0,
          end_index: 2,
          has_more: false,
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=active-detail-idle-stale-running&view=active",
  );

  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Ready");
  await expect(page.getByTestId("agent-chat-turn-running-group")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Agent is running...",
  );
});

test("AI Agent Chat can start a new chat while the selected thread is running", async ({
  page,
}) => {
  const historyPath = "/tmp/running-thread.jsonl";
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "running-thread",
            status: "active",
            running: true,
            title: "Running thread",
            history_path: historyPath,
            has_timeline: true,
            timeline_event_count: 2,
            run_state: "running",
            source: "web",
            runner_id: "bifrost_agent",
            runner_type: "bifrost_agent",
          },
        ],
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}**`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          events: [
            {
              timestamp: 1,
              event_type: "user_message",
              session_key: "running-thread",
              content: { message: "Keep running" },
            },
            {
              timestamp: 2,
              event_type: "tool_call",
              session_key: "running-thread",
              content: { tool_name: "exec_command", arguments: "sleep 100" },
            },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    `ai?aiSection=agent-chat&agentSection=chat&session=running-thread&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("Running");
  await expect(page.getByTestId("agent-chat-new")).toBeEnabled();

  await page.getByTestId("agent-chat-new").click();
  await expect(page.getByTestId("agent-chat-new-modal")).toBeVisible();
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByTestId("agent-chat-new-modal")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-state-tag")).toContainText("New");
  await expect(page.getByTestId("agent-chat-input")).toBeEnabled();
});

test("AI Agent Chat thread list scrolls and selects only the active duplicate", async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 900 });
  const overflowThreads = Array.from({ length: 32 }, (_, index) => ({
    session_key: `ended-${index}`,
    status: "ended",
            title: `Ended thread ${index}`,
            history_path: `/tmp/ended-${index}.jsonl`,
            source: index % 2 === 0 ? "feishu" : "chatgpt_web",
            start_time: 20,
            last_active_time: 100 - index,
            duration_secs: 80 - index,
          }));
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "duplicate-selection",
            status: "active",
            running: false,
            title: "Active duplicate",
            source: "admin-api",
            start_time: 120,
            last_active_time: 200,
            duration_secs: 80,
          },
          {
            session_key: "duplicate-selection",
            status: "ended",
            title: "Ended duplicate",
            history_path: "/tmp/duplicate-selection.jsonl",
            source: "feishu",
            start_time: 120,
            last_active_time: 300,
            duration_secs: 180,
          },
          {
            session_key: "running-thread",
            status: "active",
            running: true,
            title: "Running thread",
            source: "feishu",
            start_time: 100,
            last_active_time: 150,
          },
          ...overflowThreads,
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/duplicate-selection",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "duplicate-selection",
          title: "Active duplicate",
          messages: [{ role: "user", content: "Keep the active row selected" }],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=duplicate-selection&view=active",
  );

  await expect(page.getByRole("button", { name: /Active duplicate/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /Ended duplicate/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Bf Active duplicate.*Web.*duration/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /Bf Ended thread 0.*Feishu.*duration 1m/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /GPT Ended thread 1\b.*Web.*duration 1m/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /Ended thread 0.*Ended/ })).toHaveCount(0);
  await expect(page.locator('[aria-label="running"]')).toHaveCount(1);
  await expect(page.locator('[data-testid="agent-chat-thread-item"][data-selected="true"]')).toHaveCount(1);
  await expect(page.getByRole("button", { name: /Active duplicate/ })).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-chat-thread-list")).not.toContainText(">");
  const selectedThread = page.locator(
    '[data-testid="agent-chat-thread-item"][data-selected="true"]',
  );
  const selectedThreadBox = await selectedThread.boundingBox();
  expect(selectedThreadBox).toBeTruthy();
  await page.mouse.move(
    selectedThreadBox!.x + selectedThreadBox!.width - 36,
    selectedThreadBox!.y + selectedThreadBox!.height / 2,
  );
  await page.waitForTimeout(650);
  await expect(page.locator(".ant-popover").filter({ hasText: "Workspace:" }))
    .toHaveCount(0);
  const runnerMark = selectedThread.getByTestId("agent-chat-thread-runner-mark");
  await runnerMark.hover();
  await page.waitForTimeout(450);
  await expect(page.locator(".ant-popover").filter({ hasText: "Workspace:" }))
    .toHaveCount(0);
  await page.waitForTimeout(150);
  await expect(page.locator(".ant-popover").filter({ hasText: "Workspace:" }))
    .toBeVisible();
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-thread-list").evaluate(
        (element) => element.scrollHeight > element.clientHeight,
      ),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const conversation = document.querySelector('[data-testid="agent-chat-messages"]')?.closest(".ant-card");
        const threads = document.querySelector('[data-testid="agent-chat-thread-list"]')?.closest(".ant-card");
        if (!conversation || !threads) {
          return false;
        }
        const conversationBox = conversation.getBoundingClientRect();
        const threadsBox = threads.getBoundingClientRect();
        return threadsBox.left > conversationBox.left && threadsBox.top < conversationBox.bottom;
      }),
    )
    .toBe(true);
});

test("AI Agent Chat selects completed external runner rows without history path", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "external-ended-active-url",
            status: "ended",
            title: "Completed Codex row",
            source: "admin-api",
            runner_type: "codex",
            runner_id: "codex",
            start_time: 120,
            last_active_time: 180,
            duration_secs: 60,
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/external-ended-active-url",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "external-ended-active-url",
          title: "Completed Codex row",
          source: "admin-api",
          runner_type: "codex",
          runner_id: "codex",
          messages: [{ role: "user", content: "External runner completed" }],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=external-ended-active-url&view=active",
  );

  const selectedRows = page.locator(
    '[data-testid="agent-chat-thread-item"][data-selected="true"]',
  );
  await expect(selectedRows).toHaveCount(1);
  await expect(page.getByRole("button", { name: /Completed Codex row/ })).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "External runner completed",
  );
});

test("AI Agent Chat clicking the selected thread keeps loaded messages", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "selected-thread-repeat",
            status: "ended",
            title: "Repeat selected row",
            source: "feishu",
            start_time: 120,
            last_active_time: 180,
            duration_secs: 60,
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/selected-thread-repeat",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "selected-thread-repeat",
          title: "Repeat selected row",
          source: "feishu",
          messages: [
            { role: "user", content: "Loaded conversation should remain" },
            { role: "assistant", content: "Still here after clicking selected row" },
          ],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=selected-thread-repeat&view=active",
  );

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Loaded conversation should remain",
  );
  const selectedRow = page.locator(
    '[data-testid="agent-chat-thread-item"][data-selected="true"]',
  );
  await expect(selectedRow).toHaveCount(1);
  await selectedRow.click();
  await selectedRow.click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Still here after clicking selected row",
  );
  await expect(page.getByTestId("agent-chat-message-user")).not.toContainText("You");
  await expect(page.getByTestId("agent-chat-message-assistant")).not.toContainText(
    "Bifrost Agent",
  );
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-message-bubble-user").evaluate((element) => {
        const style = window.getComputedStyle(element);
        return style.borderTopStyle === "none" && style.backgroundColor !== "rgba(0, 0, 0, 0)";
      }),
    )
    .toBe(true);
  await expect
    .poll(async () =>
      page.getByTestId("agent-chat-message-bubble-assistant").evaluate((element) => {
        const style = window.getComputedStyle(element);
        return style.borderTopStyle === "none" && style.backgroundColor === "rgba(0, 0, 0, 0)";
      }),
    )
    .toBe(true);
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Preview workspace",
  );
});

test("AI Agent Chat uses detail runner metadata instead of source for selected row", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "trae-title-fallback",
            status: "active",
            running: false,
            title: "Runner: 你好",
            source: "Traex",
          },
          {
            session_key: "source-runner-mismatch",
            status: "active",
            running: false,
            title: "ChatGPT Web: legacy title",
            source: "chatgpt_web",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/source-runner-mismatch",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "source-runner-mismatch",
          title: "ChatGPT Web: legacy title",
          source: "chatgpt_web",
          agent_type: "Bifrost Agent",
          runner_type: "bifrost_agent",
          messages: [{ role: "user", content: "Runner metadata wins" }],
        }),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=source-runner-mismatch&view=active",
  );

  await expect(page.getByTestId("agent-chat-runner-tag")).toContainText(
    "bifrost_agent",
  );
  await expect(
    page.getByRole("button", { name: /Bf ChatGPT Web: legacy title/ }),
  ).toHaveAttribute("aria-current", "true");
  await expect(
    page.getByRole("button", { name: /Tr Runner: 你好/ }),
  ).toBeVisible();
});

test("AI Agent Chat renders local generated image attachments", async ({ page }) => {
  await page.addInitScript(() => {
    const originalScrollIntoView = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function patchedScrollIntoView(this: Element) {
      (window as Window & { __agentChatScrolledImageIds?: string[] }).__agentChatScrolledImageIds =
        [
          ...((window as Window & { __agentChatScrolledImageIds?: string[] })
            .__agentChatScrolledImageIds || []),
          this.getAttribute("data-agent-chat-image-id") || "",
        ];
      return originalScrollIntoView.call(this);
    };
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: "image-attachment-session",
            status: "ended",
            title: "Generated image",
            source: "admin-api",
            runner_type: "chatgpt_web",
            runner_id: "abc",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/image-attachment-session",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "image-attachment-session",
          title: "Generated image",
          source: "admin-api",
          runner_type: "chatgpt_web",
          runner_id: "abc",
          messages: [
            { role: "user", content: "draw a sun" },
            {
              role: "assistant",
              content:
                "![Generated sun](/Users/eden/.bifrost/agent/im_gateway/attachments/chatgpt_web/run/image.png)",
            },
            {
              role: "user",
              content: "and inspect this attachment",
              content_parts: [
                { type: "text", text: "and inspect this attachment" },
                {
                  type: "image_url",
                  image_url: {
                    url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lZ0fWQAAAABJRU5ErkJggg==",
                  },
                },
              ],
            },
            {
              role: "assistant",
              content:
                "![Generated moon](/Users/eden/.bifrost/agent/im_gateway/attachments/chatgpt_web/run/moon.png)",
            },
          ],
        }),
      });
    },
  );
  await page.route(
    "**/_bifrost/api/im-gateway/attachments/chatgpt_web/run/image.png",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "image/png",
        body: Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lZ0fWQAAAABJRU5ErkJggg==",
          "base64",
        ),
      });
    },
  );
  await page.route(
    "**/_bifrost/api/im-gateway/attachments/chatgpt_web/run/moon.png",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "image/png",
        body: Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFElEQVR42mP8z8BQz0AEYBxVSFIBAC03A/0fR0gCAAAAAElFTkSuQmCC",
          "base64",
        ),
      });
    },
  );

  await openPage(
    page,
    "ai?aiSection=agent-chat&agentSection=chat&session=image-attachment-session&view=active",
  );

  const image = page.getByRole("img", { name: "Generated sun" });
  await expect(image).toHaveAttribute(
    "src",
    /\/_bifrost\/api\/im-gateway\/attachments\/chatgpt_web\/run\/image\.png$/,
  );
  await expect
    .poll(async () => image.evaluate((element) => (element as HTMLImageElement).naturalWidth))
    .toBe(1);

  await expect(page.getByTestId("agent-chat-previewable-image")).toHaveCount(3);
  await image.click();
  await expect(page.getByTestId("agent-chat-image-lightbox")).toBeVisible();
  await expect(page.getByTestId("agent-chat-image-lightbox-count")).toHaveText("1 / 3");
  await expect(page.getByTestId("agent-chat-image-lightbox-img")).toHaveAttribute(
    "src",
    /\/_bifrost\/api\/im-gateway\/attachments\/chatgpt_web\/run\/image\.png$/,
  );

  await page.getByTestId("agent-chat-image-lightbox-next").click();
  await expect(page.getByTestId("agent-chat-image-lightbox-count")).toHaveText("2 / 3");
  await expect(page.getByTestId("agent-chat-image-lightbox-img")).toHaveAttribute(
    "src",
    /^data:image\/png;base64,/,
  );

  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("agent-chat-image-lightbox-count")).toHaveText("3 / 3");
  await expect(page.getByTestId("agent-chat-image-lightbox-img")).toHaveAttribute(
    "src",
    /\/_bifrost\/api\/im-gateway\/attachments\/chatgpt_web\/run\/moon\.png$/,
  );

  await page.keyboard.press("ArrowLeft");
  await expect(page.getByTestId("agent-chat-image-lightbox-count")).toHaveText("2 / 3");
  await page.getByTestId("agent-chat-image-lightbox").click({ position: { x: 12, y: 12 } });
  await expect(page.getByTestId("agent-chat-image-lightbox")).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as Window & { __agentChatScrolledImageIds?: string[] })
            .__agentChatScrolledImageIds || [],
      ),
    )
    .toContain("session-image-attachment-session-2-part-0");

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await image.scrollIntoViewIfNeeded();
  await image.click();
  await expect(page.getByTestId("agent-chat-image-lightbox")).toBeVisible();
  await page.getByTestId("agent-chat-image-lightbox-close").click();
  await expect(page.getByTestId("agent-chat-image-lightbox")).toHaveCount(0);
});

test("AI Agent Chat thread context menu deletes after inline confirmation", async ({
  page,
}) => {
  let deleteCalls = 0;
  let sessions = [
    {
      session_key: "delete-target",
      status: "ended",
      title: "Delete target conversation",
      source: "admin-api",
      runner_type: "bifrost_agent",
      runner_id: "bifrost_agent",
      work_dir: "/tmp/delete-target-workspace",
      start_time: 1779700000,
      last_active_time: 1779700060,
      duration_secs: 60,
    },
  ];

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/delete-target",
    async (route) => {
      if (route.request().method() === "DELETE") {
        deleteCalls += 1;
        sessions = [];
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ ok: true }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: "delete-target",
          title: "Delete target conversation",
          messages: [],
        }),
      });
    },
  );

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  const item = page
    .getByTestId("agent-chat-thread-item")
    .filter({ hasText: "Delete target conversation" });
  await expect(item).toBeVisible();
  await item.click({ button: "right" });

  await expect(page.getByTestId("agent-chat-thread-delete")).toBeVisible();
  await page.getByTestId("agent-chat-thread-delete").click();
  await expect(page.getByTestId("agent-chat-thread-delete-confirm")).toBeVisible();
  await expect(page.getByTestId("agent-chat-thread-delete-cancel")).toBeVisible();

  await page.getByTestId("agent-chat-thread-delete-confirm").click();

  await expect
    .poll(() => deleteCalls)
    .toBe(1);
  await expect(item).toHaveCount(0);
});

test("AI Agent Chat composer keeps Shift Enter multiline and sends on Enter", async ({
  page,
}) => {
  await routeBuiltinAgentChatConfig(page);
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    expect(route.request().postDataJSON()).toMatchObject({
      message: "Line one\nLine two",
    });
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Multiline sent through API"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  const input = page.getByTestId("agent-chat-input");
  await input.fill("Line one");
  await input.press("Shift+Enter");
  await input.pressSequentially("Line two");

  await expect(input).toHaveValue("Line one\nLine two");
  await expect(page.getByTestId("agent-chat-message-user")).toHaveCount(0);

  await input.press("Enter");

  await expect(input).toHaveValue("");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Line one");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Line two");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Multiline sent through API",
  );
});

test("AI Agent Chat supports pasted image previews and pure image send", async ({
  page,
}) => {
  const requests: Array<Record<string, unknown>> = [];
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    requests.push(route.request().postDataJSON());
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Image received"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  const input = page.getByTestId("agent-chat-input");
  await input.focus();
  await pasteImageFiles(
    page,
    "agent-chat-input",
    Array.from({ length: 7 }, (_, index) => ({
      name: `image-${index}.png`,
      type: "image/png",
      content: `image-${index}`,
    })),
  );

  await expect(page.getByTestId("agent-chat-image-preview")).toHaveCount(6);
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByTestId("agent-chat-image-preview")).toHaveCount(6);
  await page.getByLabel("Remove pasted image 1").click();
  await expect(page.getByTestId("agent-chat-image-preview")).toHaveCount(5);
  await expect(page.getByTestId("agent-chat-send")).toBeEnabled();
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-input")).toHaveValue("");
  await expect(page.getByTestId("agent-chat-image-preview")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Attached 5 images");
  await expect(page.getByTestId("agent-chat-message-images")).toBeVisible();
  await expect(page.getByTestId("agent-chat-message-images").locator("img")).toHaveCount(5);
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Image received");
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({ message: "" });
  expect((requests[0].images as unknown[]).length).toBe(5);
  expect(requests[0].images).toEqual(
    expect.arrayContaining([expect.objectContaining({ mime_type: "image/png" })]),
  );
});

test("AI Agent Chat sends pasted images to external runner stream", async ({
  page,
}) => {
  let requestPayload: Record<string, unknown> | undefined;
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ sessions: [] }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "codex",
        runners: { codex: { enabled: true, adapter: "codex" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    requestPayload = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: "application/x-ndjson",
      body: '{"eventType":"run_finished","status":"succeeded","response":"Codex saw image"}\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-input").fill("Describe this image");
  await pasteImageFiles(page, "agent-chat-input", [
    { name: "external.png", type: "image/png", content: "external" },
  ]);
  await expect(page.getByTestId("agent-chat-image-preview")).toHaveCount(1);
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText("Codex saw image");
  expect(requestPayload).toMatchObject({
    message: "Describe this image",
    runnerId: "codex",
  });
  expect(requestPayload?.images).toEqual(
    expect.arrayContaining([expect.objectContaining({ mimeType: "image/png" })]),
  );
});

test("AI Agent Chat persists collapsed thread rail", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ sessions: [] }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }) });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await expect(page.getByTestId("agent-chat-thread-list")).toBeVisible();
  await page.getByTestId("agent-chat-threads-collapse").click();
  await expect(page.getByTestId("agent-chat-thread-list")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-threads-expand")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.localStorage.getItem("bifrost.agentChat.threadRailCollapsed"),
      ),
    )
    .toBe("true");

  await page.reload();
  await expect(page.getByTestId("agent-chat-thread-list")).toHaveCount(0);
  await expect(page.getByTestId("agent-chat-threads-expand")).toBeVisible();
  await page.getByTestId("agent-chat-threads-expand").click();
  await expect(page.getByTestId("agent-chat-thread-list")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.localStorage.getItem("bifrost.agentChat.threadRailCollapsed"),
      ),
    )
    .toBe("false");
});

test("AI Agent Chat queues running input for external runners", async ({
  page,
}) => {
  const messages: string[] = [];
  let releaseFirst: (() => void) | undefined;
  const firstRequestCanFinish = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ sessions: [] }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "traex",
        runners: { traex: { enabled: true, adapter: "traex" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    const payload = route.request().postDataJSON();
    messages.push(payload.message);
    expect(payload).toMatchObject({
      runnerId: "traex",
      adapter: "traex",
    });
    if (payload.message === "Start external long task") {
      await firstRequestCanFinish;
      await route.fulfill({
        status: 200,
        contentType: "application/x-ndjson",
        body:
          '{"eventType":"run_started","status":{"runner_id":"traex","runner_type":"traex"}}\n' +
          '{"eventType":"run_finished","status":"succeeded","response":"External long task done"}\n',
      });
      return;
    }
    if (payload.message === "/q follow up while running") {
      await route.fulfill({
        status: 200,
        contentType: "application/x-ndjson",
        body:
          '{"eventType":"run_finished","response":"queued","queued":true,"queueItems":[{"seq":1,"message":"follow up while running"}]}\n',
      });
      return;
    }
    if (payload.message === "/stop") {
      await route.fulfill({
        status: 200,
        contentType: "application/x-ndjson",
        body:
          '{"eventType":"run_finished","response":"stopped","stopped":true}\n',
      });
      return;
    }
    await route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: `unexpected message: ${String(payload.message)}`,
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-input").fill("Start external long task");
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Stop");

  await page.getByTestId("agent-chat-input").fill("follow up while running");
  await expect(page.getByText("This runner queues follow-up messages")).toBeVisible();
  await expect(page.getByText("Queue", { exact: true })).toBeVisible();
  await expect(page.getByText("Guide")).toHaveCount(0);
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-queue-panel")).toContainText(
    "follow up while running",
  );

  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText("stopped");
  releaseFirst?.();
  await expect
    .poll(() => messages.join("|"))
    .toBe("Start external long task|/q follow up while running|/stop");
});

test("AI Agent Chat supports running stop, guide, queue, and queue removal", async ({
  page,
}) => {
  const messages: string[] = [];
  let releaseFirst: (() => void) | undefined;
  const firstRequestCanFinish = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ sessions: [] }) });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: {},
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }) });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    const payload = route.request().postDataJSON();
    messages.push(payload.message);
    if (payload.message === "Start long task") {
      await firstRequestCanFinish;
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body:
          'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
          'event: run_finished\ndata: {"eventType":"run_finished","response":"Long task done"}\n\n',
      });
      return;
    }
    if (payload.message === "/q queued follow up") {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body:
          'event: run_finished\ndata: {"eventType":"run_finished","response":"✅ 消息已收到，将在当前任务完成后处理（排队 1 条）","queued":true,"queueItems":[{"seq":1,"message":"queued follow up"}]}\n\n',
      });
      return;
    }
    if (payload.message === "/rq 1") {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body:
          'event: run_finished\ndata: {"eventType":"run_finished","response":"removed","queued":true,"queueItems":[]}\n\n',
      });
      return;
    }
    if (payload.message === "/stop") {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body:
          'event: run_finished\ndata: {"eventType":"run_finished","response":"stopped","stopped":true}\n\n',
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_finished\ndata: {"eventType":"run_finished","response":"guide accepted","guide":true}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");
  await page.getByTestId("agent-chat-input").fill("Start long task");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-send")).toHaveAttribute("aria-label", "Stop");

  await page.getByTestId("agent-chat-input").fill("guide now");
  await expect(page.getByText("Running input")).toBeVisible();
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText("guide accepted");

  await page.getByTestId("agent-chat-input").fill("queued follow up");
  await page.locator(".ant-segmented-item").filter({ hasText: /^Queue$/ }).click();
  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-queue-panel")).toContainText(
    "queued follow up",
  );
  const messageBubbles = page.locator('[data-testid^="agent-chat-message-bubble-"]');
  await expect
    .poll(async () => (await messageBubbles.allTextContents()).join("\n"))
    .not.toContain("queued follow up");
  await expect
    .poll(async () => (await messageBubbles.allTextContents()).join("\n"))
    .not.toContain("消息已收到");

  await page.getByLabel("Remove queued message 1").click();
  await expect(page.getByTestId("agent-chat-queue-panel")).toBeHidden();
  await expect
    .poll(async () => (await messageBubbles.allTextContents()).join("\n"))
    .not.toContain("removed");

  await page.getByTestId("agent-chat-send").click();
  await expect(page.getByTestId("agent-chat-messages")).toContainText("stopped");

  releaseFirst?.();
  await expect
    .poll(() => messages.join("|"))
    .toBe("Start long task|guide now|/q queued follow up|/rq 1|/stop");
});

test("AI Agent Chat surfaces run errors in the telemetry panel", async ({ page }) => {
  await routeBuiltinAgentChatConfig(page);
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: run_failed\ndata: {"eventType":"run_failed","error":"tool permission denied"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await page.getByTestId("agent-chat-input").fill("Trigger a guarded tool");
  await page.getByTestId("agent-chat-send").click();

  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByTestId("agent-chat-status")).toContainText("Failed");
  await expect(page.getByTestId("agent-chat-errors")).toContainText(
    "tool permission denied",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "tool permission denied",
  );
});

test("AI Agent Chat consumes assistant_final without a trailing SSE separator", async ({
  page,
}) => {
  await routeBuiltinAgentChatConfig(page);
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: assistant_final\ndata: {"eventType":"assistant_final","content":"Final-only answer"}',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await page.getByTestId("agent-chat-input").fill("Return final only");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Final-only answer",
  );
});

test("AI Agent Chat streams long task output previews into tool output", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        version: 1,
        defaultRunnerId: "bifrost_agent",
        runners: { bifrost_agent: { enabled: true, adapter: "builtin" } },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ content: "", work_dir: "/tmp/workspace" }),
    });
  });
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: tool_started\ndata: {"eventType":"tool_started","toolName":"exec_command","arguments":"{\\"cmd\\":\\"cargo test\\"}"}\n\n' +
        'event: long_task_status\ndata: {"eventType":"long_task_status","sessionKey":"web-long-task","sessionId":"sess-1","profile":"long","state":"waiting_on_session","elapsedMs":463000,"lastOutputPreview":"running test shard 42\\nstreamed stdout chunk","nextCheckAtMs":123456,"unchangedHeartbeats":0}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"Still monitoring."}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Done watching."}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await page.getByTestId("agent-chat-input").fill("Watch the long command");
  await page.getByTestId("agent-chat-send").click();

  await page.getByTestId("agent-chat-turn-collapse-toggle").click();
  const processBlock = page.getByTestId("agent-chat-process-block");
  await expect(processBlock).toBeVisible();
  await expect(processBlock).toContainText("已运行 1 条命令");
  await expect(processBlock).not.toContainText("streamed stdout chunk");
  await processBlock.getByRole("button").click();
  await expect(processBlock).toContainText("exec_command");
  await processBlock.getByText("exec_command").click();
  await expect(processBlock).toContainText("Input");
  await expect(processBlock).toContainText("Output");
  await expect(processBlock).toContainText("running test shard 42");
  await expect(processBlock).toContainText("streamed stdout chunk");
});

test("AI Agent Chat surfaces busy sessions as recoverable errors", async ({ page }) => {
  await routeBuiltinAgentChatConfig(page);
  await page.route("**/_bifrost/api/agent/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: run_busy\ndata: {"eventType":"run_busy","message":"Session is already running"}\n\n',
    });
  });

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  await page.getByTestId("agent-chat-input").fill("Start while busy");
  await page.getByTestId("agent-chat-send").click();

  await page.getByTestId("agent-chat-settings-open").click();
  await expect(page.getByTestId("agent-chat-status")).toContainText("Failed");
  await expect(page.getByTestId("agent-chat-errors")).toContainText(
    "Session is already running",
  );
  await expect(page.getByTestId("agent-chat-input")).toBeEnabled();
});

test("AI page preserves legacy agentSection session deep link", async ({ page }) => {
  await openPage(page, "ai?agentSection=sessions&session=session-key-1");

  await expect(page.getByTestId("ai-nav-agent-sessions")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page).toHaveURL(/aiSection=agent-sessions/);
});
