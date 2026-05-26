import { expect, test } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

test("AI Agent Chat deep link renders local chat preview and composer flow", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"Review received"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"API run complete [docs](https://example.test/docs)","planSteps":[{"step":"Gather context","status":"completed"},{"step":"Implement UI","status":"completed"}],"toolCalls":[{"tool_name":"shell","arguments":"pnpm test","result":"ok","success":true}]}\n\n',
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
      page.getByTestId("agent-chat-message-bubble-assistant").evaluate((bubble) => {
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
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Gather context");
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Implement UI");
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Completed");
  await page.getByTestId("agent-chat-plan-toggle").click();
  await expect(page.getByTestId("agent-chat-plan")).not.toContainText("Gather context");
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Plan");
  await page.getByTestId("agent-chat-plan-toggle").click();
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Gather context");
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
  await expect(processBlock).toContainText("steps completed");
  // Click to expand
  await processBlock.click();
  // After expanding, shows individual step summaries
  await expect(processBlock).toContainText("shell");
  await expect(processBlock).toContainText("Compacted");
});

test("AI Agent Chat slash runner call selects a runner and renders the result", async ({
  page,
}) => {
  let runnerCallRequested = false;
  let releaseRunnerCallStream: (() => void) | undefined;
  const runnerCallStreamReady = new Promise<void>((resolve) => {
    releaseRunnerCallStream = resolve;
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
        'event: plan_updated\ndata: {"eventType":"plan_updated","title":"Density check","steps":[{"step":"Gather context","status":"completed"},{"step":"Compress every row","status":"in_progress"},{"step":"Keep the long plan item on one compact line without growing the composer vertically","status":"pending"},{"step":"Check five row limit","status":"pending"},{"step":"Preserve collapse toggle","status":"pending"},{"step":"Overflow item six","status":"pending"},{"step":"Overflow item seven","status":"pending"}]}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"Done","planSteps":[{"step":"Gather context","status":"completed"},{"step":"Compress every row","status":"in_progress"},{"step":"Keep the long plan item on one compact line without growing the composer vertically","status":"pending"},{"step":"Check five row limit","status":"pending"},{"step":"Preserve collapse toggle","status":"pending"},{"step":"Overflow item six","status":"pending"},{"step":"Overflow item seven","status":"pending"}]}\n\n',
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
  await expect(page.getByTestId("agent-chat-plan")).toContainText("Overflow item seven");
  await expect(page.getByTestId("agent-chat-plan-item")).toHaveCount(7);

  const assertCompactPlan = async () => {
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
    await expect(page.getByTestId("agent-chat-plan-status-completed")).toHaveCount(1);
    await expect(page.getByTestId("agent-chat-plan-status-in-progress")).toHaveCount(1);
    await expect(page.getByTestId("agent-chat-plan-status-pending")).toHaveCount(5);
    await expect
      .poll(async () =>
        page.getByTestId("agent-chat-plan").evaluate((panel) => {
          const composer = document.querySelector('[data-testid="agent-chat-composer-track"]');
          const panelBox = panel.getBoundingClientRect();
          const composerBox = composer?.getBoundingClientRect();
          return {
            panelIsCompact: panelBox.height <= 176,
            contained:
              Boolean(composerBox) &&
              panelBox.left >= composerBox!.left &&
              panelBox.right <= composerBox!.right &&
              panelBox.top >= composerBox!.top &&
              panelBox.bottom <= composerBox!.bottom,
          };
        }),
      )
      .toMatchObject({
        contained: true,
        panelIsCompact: true,
      });
  };

  await assertCompactPlan();
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await assertCompactPlan();
  await page.getByTestId("agent-chat-plan-toggle").click();
  await expect(page.getByTestId("agent-chat-plan-list")).toHaveCount(0);
});

test("AI Agent Chat keeps conversation content centered at 750px max", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}`,
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await expect
    .poll(async () =>
      page.evaluate(
        () => {
          const behaviors =
            (
              window as Window & {
                __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined>;
              }
            ).__agentChatScrollBehaviors || [];
          return behaviors[behaviors.length - 1];
        },
      ),
    )
    .toBe("auto");

  await page.reload();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Question before refresh",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Answer survived refresh",
  );
  await expect(page.getByTestId("agent-chat-title")).toHaveText("Refresh recovery");
  await expect
    .poll(async () =>
      page.evaluate(
        () => {
          const behaviors =
            (
              window as Window & {
                __agentChatScrollBehaviors?: Array<ScrollBehavior | undefined>;
              }
            ).__agentChatScrollBehaviors || [];
          return behaviors[behaviors.length - 1];
        },
      ),
    )
    .toBe("auto");
});

test("AI Agent Chat falls back to persisted history when active detail is gone", async ({
  page,
}) => {
  const historyPath = "/tmp/fallback-ended-session.jsonl";
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
    `**/_bifrost/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}`,
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await expect(page.getByRole("button", { name: /Bf Ended thread 1\b.*Web.*duration 1m/ })).toBeVisible();
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
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
});

test("AI Agent Chat renders local generated image attachments", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
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
  await expect(page.getByTestId("agent-chat-messages")).not.toContainText(
    "Line one\nLine two",
  );

  await input.press("Enter");

  await expect(input).toHaveValue("");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Line one");
  await expect(page.getByTestId("agent-chat-messages")).toContainText("Line two");
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Multiline sent through API",
  );
});

test("AI Agent Chat supports running stop, guide, queue, and queue removal", async ({
  page,
}) => {
  const messages: string[] = [];
  let releaseFirst: (() => void) | undefined;
  const firstRequestCanFinish = new Promise<void>((resolve) => {
    releaseFirst = resolve;
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

test("AI Agent Chat surfaces busy sessions as recoverable errors", async ({ page }) => {
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
