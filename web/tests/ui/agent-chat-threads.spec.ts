import { expect, test } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

test("AI Agent Chat thread list loads in batches and virtualizes rows", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  const sessions = Array.from({ length: 55 }, (_, index) => {
    const ordinal = index + 1;
    const label = ordinal.toString().padStart(2, "0");
    return {
      session_key: `batch-thread-${label}`,
      status: ordinal === 1 ? "active" : "ended",
      running: ordinal === 1,
      title: `Thread ${label}`,
      source: "admin-api",
      runner_type: "codex",
      work_dir: "/tmp/thread-batch-workspace",
      start_time: 1_779_700_000 - ordinal * 120,
      last_active_time: 1_779_700_000 - ordinal * 60,
      duration_secs: 60,
    };
  });

  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions }),
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
        defaultRunnerId: "codex",
        runners: {},
        channels: {},
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/im-gateway/agent/sessions/batch-thread-*",
    async (route) => {
      const sessionKey = decodeURIComponent(route.request().url().split("/").pop() || "");
      const detail = sessions.find((item) => item.session_key === sessionKey) || sessions[0];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: detail.session_key,
          title: detail.title,
          source: detail.source,
          runner_type: detail.runner_type,
          work_dir: detail.work_dir,
          messages: [
            { role: "user", content: `${detail.title} prompt` },
            { role: "assistant", content: `${detail.title} answer` },
          ],
        }),
      });
    },
  );

  await openPage(page, "ai?aiSection=agent-chat&agentSection=chat");

  const threadList = page.getByTestId("agent-chat-thread-list");
  await expect(page.getByTestId("agent-chat-thread-load-count")).toHaveText(
    "Showing 20 of 55",
  );
  await expect(page.getByTestId("agent-chat-thread-load-more")).toBeVisible();
  await expect
    .poll(() => page.getByTestId("agent-chat-thread-virtual-row").count())
    .toBeLessThan(55);

  await threadList.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await page.getByTestId("agent-chat-thread-load-more").click();
  await expect(page.getByTestId("agent-chat-thread-load-count")).toHaveText(
    "Showing 40 of 55",
  );

  await threadList.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await page.getByTestId("agent-chat-thread-load-more").click();
  await expect(page.getByTestId("agent-chat-thread-load-count")).toHaveText(
    "Showing 55 of 55",
  );
  await expect(page.getByTestId("agent-chat-thread-load-more")).toHaveCount(0);
  await expect
    .poll(() => page.getByTestId("agent-chat-thread-virtual-row").count())
    .toBeLessThan(55);
});
