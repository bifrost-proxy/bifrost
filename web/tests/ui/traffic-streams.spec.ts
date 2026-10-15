// Legacy traffic suite: extract focused fixture modules as lifecycle ownership
// is isolated, preserving the registered end-to-end cases and assertions.
import { test, expect } from "@playwright/test";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import WebSocket from "ws";
import { HttpProxyAgent } from "http-proxy-agent";
import {
  proxyUrl,
  apiBase,
  currentFilePath,
  startMockServer,
  startSseServer,
  startOpenAiLikeSseServer,
  startWsServer,
  startTlsWsServer,
  sendProxyRequest,
  clearTraffic,
  setDocumentVisibility,
  streamSseViaProxy,
  sendWsViaProxy,
} from "./helpers/traffic-fixtures";

test("订阅更新提示与滚动状态一致", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  for (let i = 0; i < 30; i += 1) {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}/batch-${token}-${i}`,
    );
  }

  await expect(page.getByTestId("traffic-row").first()).toBeVisible();

  const scrollContainer = page.getByTestId("traffic-table-scroll");
  await scrollContainer.evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  });

  await sendProxyRequest(`http://127.0.0.1:${server.port}/latest-${token}`);
  const newTrafficIndicator = page.getByTestId("traffic-new-indicator");
  await expect(newTrafficIndicator).toBeVisible();
  await newTrafficIndicator.click({ force: true });
  await expect(
    page
      .getByTestId("traffic-row")
      .filter({ hasText: `/latest-${token}` })
      .first(),
  ).toBeVisible();

  await server.close();
});

test("WebSocket 帧与 size 更新展示正确", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-${token}`;

  await sendWsViaProxy(`ws://127.0.0.1:${server.port}${wsPath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const wsRow = page
    .getByTestId("traffic-row")
    .filter({ hasText: wsPath })
    .last();
  const frameCountAttr = await wsRow.getAttribute("data-frame-count");
  const frameCount = Number(frameCountAttr ?? "0");
  expect(frameCount).toBeGreaterThanOrEqual(2);
  await wsRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
  await expect(page.getByTestId("ws-frames-summary")).toContainText("frames");
  const frameRows = page.getByTestId("ws-frame-row");
  await expect(frameRows.first()).toBeVisible();
  expect(await frameRows.count()).toBeGreaterThanOrEqual(2);
  await expect(page.getByTestId("ws-frames-table")).toContainText("6 B");

  await server.close();
});

test("WebSocket loopback TLS 回显与 size 增长", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startTlsWsServer();
  const ruleName = `wss-loopback-${server.port}`;
  const echoPath = `/tls-ws-echo-${server.port}`;
  const repoRoot = path.resolve(path.dirname(currentFilePath), "../../..");
  const rule = (
    await fs.readFile(
      path.join(repoRoot, "e2e-tests/rules/tls/ui_ws_loopback.txt"),
      "utf8",
    )
  ).replaceAll("__MOCK_WSS_PORT__", String(server.port));
  try {
    const ruleResponse = await request.post(`${apiBase}/rules`, {
      data: { name: ruleName, content: rule, enabled: true },
    });
    expect(ruleResponse.ok()).toBe(true);
    await sendWsViaProxy(`wss://127.0.0.1:${server.port}${echoPath}`);

    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const wsRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: echoPath })
      .first();
    await expect(wsRow).toBeVisible();

    await expect
      .poll(async () => {
        const frameCountAttr = await wsRow.getAttribute("data-frame-count");
        return Number(frameCountAttr ?? "0");
      })
      .toBeGreaterThanOrEqual(2);

    await wsRow.click();
    await page.getByTestId("response-tab-messages").click();

    await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
    await expect(page.getByTestId("ws-frames-table")).toContainText("hello");
    await expect(page.getByTestId("ws-frames-table")).toContainText("6 B");
  } finally {
    await request.delete(`${apiBase}/rules/${ruleName}`);
    await server.close();
  }
});

test("WebSocket 详情 Messages 面板打开后可实时收到新消息", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-live-${token}`;
  const wsUrl = `ws://127.0.0.1:${server.port}${wsPath}`;
  const agent = new HttpProxyAgent(proxyUrl);
  const ws = new WebSocket(wsUrl, { agent });

  try {
    await new Promise<void>((resolve, reject) => {
      ws.once("open", resolve);
      ws.once("error", reject);
    });
    await server.waitForClient();

    ws.send(`seed-${token}`);

    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const wsRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: wsPath })
      .last();
    await expect(wsRow).toBeVisible();
    await wsRow.click();
    await page.getByTestId("response-tab-messages").click();

    await expect(page.getByTestId("ws-frames-pane")).toBeVisible();
    await expect(page.getByTestId("ws-frames-table")).toContainText(
      `seed-${token}`,
    );
    await page.waitForTimeout(500);

    const livePayload = `after-open-${token}`;
    for (let i = 0; i < 5; i += 1) {
      ws.send(`${livePayload}-${i}`);
      await page.waitForTimeout(200);
    }

    await expect(page.getByTestId("ws-frames-table")).toContainText(
      livePayload,
    );
  } finally {
    if (
      ws.readyState === WebSocket.OPEN ||
      ws.readyState === WebSocket.CONNECTING
    ) {
      await new Promise<void>((resolve) => {
        ws.once("close", resolve);
        ws.close();
      });
    }
    await server.close();
  }
});

test("WebSocket 列表未到底部时不应强制滚动到底部", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const server = await startWsServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const wsPath = `/ws-scroll-${token}`;
  const wsUrl = `ws://127.0.0.1:${server.port}${wsPath}`;

  const agent = new HttpProxyAgent(proxyUrl);
  const ws = new WebSocket(wsUrl, { agent });

  await new Promise<void>((resolve, reject) => {
    ws.once("open", resolve);
    ws.once("error", reject);
  });

  for (let i = 0; i < 120; i += 1) {
    ws.send(`seed-${token}-${i}`);
  }

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const wsRow = page
    .getByTestId("traffic-row")
    .filter({ hasText: wsPath })
    .last();
  await expect(wsRow).toBeVisible();
  await wsRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("ws-frames-pane")).toBeVisible();

  const summary = page.getByTestId("ws-frames-summary");
  const summaryHandle = await summary.elementHandle();
  if (summaryHandle) {
    await page.waitForFunction((el: SVGElement | HTMLElement) => {
      const text = el.textContent || "";
      const match = text.match(/(\d+)\s+of\s+(\d+)\s+frames/);
      if (!match) return false;
      return Number(match[1]) >= 50;
    }, summaryHandle);
  }

  const scrollContainer = page.getByTestId("ws-frames-table");
  await scrollContainer.evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  });
  const scrollTopBefore = await scrollContainer.evaluate((el) => el.scrollTop);

  for (let i = 120; i < 140; i += 1) {
    ws.send(`append-${token}-${i}`);
  }

  if (summaryHandle) {
    await page.waitForFunction((el: SVGElement | HTMLElement) => {
      const text = el.textContent || "";
      const match = text.match(/(\d+)\s+of\s+(\d+)\s+frames/);
      if (!match) return false;
      return Number(match[1]) >= 70;
    }, summaryHandle);
  }

  const scrollTopAfter = await scrollContainer.evaluate((el) => el.scrollTop);
  expect(scrollTopAfter).toBe(scrollTopBefore);

  await new Promise<void>((resolve) => {
    ws.once("close", resolve);
    ws.close();
  });
  await server.close();
});

test("SSE 事件订阅与列表展示正确", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await expect(sseRow).toHaveAttribute(
    "data-response-size",
    expect.stringMatching(/^[1-9]\d*$/),
  );
  await sseRow.click();
  await page.getByTestId("response-tab-messages").click();

  await expect(page.getByTestId("sse-message-container")).toBeVisible();
  await expect(page.getByTestId("sse-message-count")).toContainText("events");
  await expect(page.getByTestId("sse-message-list")).toContainText("alpha");
  await expect(page.getByTestId("sse-message-list")).toContainText("beta");

  await server.close();
});

test("SSE 详情通过弹窗展开且不改变列表项高度", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await sseRow.click();
  await page.getByTestId("response-tab-messages").click();

  const container = page.getByTestId("sse-message-container");
  await expect(container).toBeVisible();

  const search = container.getByPlaceholder("Search events...");
  await search.fill("target-long");

  const detailCard = container
    .getByTestId("sse-event-card")
    .filter({ hasText: "target-long" })
    .first();

  await expect(detailCard).toBeVisible();
  const beforeBox = await detailCard.boundingBox();
  expect(beforeBox).not.toBeNull();

  await detailCard.getByTestId("sse-event-toggle").click();

  await expect
    .poll(async () => {
      const currentHeight = (await detailCard.boundingBox())?.height || 0;
      return Math.abs(currentHeight - (beforeBox?.height || 0));
    })
    .toBeLessThan(2);
  await expect(page.getByTestId("sse-event-detail-content")).toBeVisible();
  await expect(page.getByTestId("sse-event-detail-content")).toContainText(
    "target-long",
  );
  await page
    .locator(".ant-modal")
    .filter({ has: page.getByTestId("sse-event-detail-content") })
    .locator(".ant-modal-footer")
    .getByRole("button", { name: "Close" })
    .click();
  await expect(page.getByTestId("sse-event-detail-content")).not.toBeVisible();

  await server.close();
});

test("SSE 外部站点流式更新与 size 增长", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/external-sse-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write("id: 1\ndata: alpha\n\n");
    const timer = setInterval(() => {
      res.write(`id: ${Date.now()}\ndata: update-${Date.now()}\n\n`);
    }, 400);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "6",
      "--noproxy",
      "",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const row = page
      .getByTestId("traffic-row")
      .filter({ hasText: ssePath })
      .first();
    await expect(row).toBeVisible();

    const sizeBefore = Number(
      (await row.getAttribute("data-response-size")) || "0",
    );
    await expect
      .poll(async () => {
        const size = await row.getAttribute("data-response-size");
        return Number(size || "0");
      })
      .toBeGreaterThan(sizeBefore);

    await row.click();
    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect
      .poll(async () => {
        const text = await page.getByTestId("sse-message-count").textContent();
        const match = text?.match(/(\d+)\s+events/);
        return match ? Number(match[1]) : 0;
      })
      .toBeGreaterThan(0);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情 Messages 面板打开后可实时收到新事件", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-live-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(`id: ${counter + 1}\ndata: after-open-${token}-${counter}\n\n`);
      if (counter >= 6) {
        clearInterval(timer);
        res.end();
      }
    }, 400);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "8",
      "--noproxy",
      "",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: ssePath })
      .first();
    await expect(sseRow).toBeVisible();
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(page.getByTestId("sse-message-list")).toContainText(
      `seed-${token}`,
    );
    await expect(page.getByTestId("sse-message-list")).toContainText(
      `after-open-${token}`,
    );
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情切换 Response tab 后消息列表不应丢失", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-tab-switch-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(`id: ${counter + 1}\ndata: after-tab-${token}-${counter}\n\n`);
      if (counter >= 8) {
        clearInterval(timer);
        res.end();
      }
    }, 300);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "8",
      "--noproxy",
      "",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: ssePath })
      .first();
    await expect(sseRow).toBeVisible();
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    const messages = page.getByTestId("sse-message-list");
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-tab-${token}-1`);

    await page.getByTestId("response-tab-header").click();
    await expect(
      page.getByTestId("response-header-view-mode-tabs"),
    ).toBeVisible();
    await page.waitForTimeout(700);

    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-tab-${token}-1`);
    await expect(messages).toContainText(`after-tab-${token}-2`);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("SSE 详情切到弹窗再附回右侧面板后消息列表不应丢失", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/sse-detach-${token}`;
  const server = createServer((req, res) => {
    if (req.url !== ssePath) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(`id: 1\ndata: seed-${token}\n\n`);
    let counter = 0;
    const timer = setInterval(() => {
      counter += 1;
      res.write(
        `id: ${counter + 1}\ndata: after-detach-${token}-${counter}\n\n`,
      );
      if (counter >= 10) {
        clearInterval(timer);
        res.end();
      }
    }, 300);
    req.on("close", () => {
      clearInterval(timer);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const stream = spawn(
    "curl",
    [
      "-sS",
      "-N",
      "--max-time",
      "10",
      "--noproxy",
      "",
      "-x",
      proxyUrl,
      `http://127.0.0.1:${port}${ssePath}`,
    ],
    { stdio: "ignore" },
  );

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const sseRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: ssePath })
      .first();
    await expect(sseRow).toBeVisible();
    const mobileTrustDismissButton = page.getByRole("button", {
      name: "Not now",
    });
    const mobileTrustPromptVisible = await mobileTrustDismissButton
      .waitFor({ state: "visible", timeout: 15_000 })
      .then(() => true)
      .catch(() => false);
    if (mobileTrustPromptVisible) {
      await mobileTrustDismissButton.click();
      await expect(mobileTrustDismissButton).toBeHidden();
    }
    await sseRow.click();
    await page.getByTestId("response-tab-messages").click();

    const messages = page.getByTestId("sse-message-list");
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-detach-${token}-1`);

    const popupPromise = page.waitForEvent("popup");
    await page.getByTestId("traffic-detail-open-window").click();
    const popup = await popupPromise;
    await popup.waitForLoadState("domcontentloaded");
    await expect(popup.getByTestId("traffic-detail-attach-back")).toBeVisible();

    await popup.getByTestId("traffic-detail-attach-back").click();
    await expect.poll(() => popup.isClosed()).toBe(true);

    await expect(page.getByTestId("traffic-detail")).toBeVisible();
    await page.getByTestId("response-tab-messages").click();
    await expect(page.getByTestId("sse-message-container")).toBeVisible();
    await expect(messages).toContainText(`seed-${token}`);
    await expect(messages).toContainText(`after-detach-${token}-1`);
    await expect(messages).toContainText(`after-detach-${token}-2`);
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((err?: Error) => (err ? reject(err) : resolve())),
    );
    const waitForClose = new Promise<void>((resolve) => {
      if (stream.exitCode !== null) {
        resolve();
        return;
      }
      stream.once("close", () => resolve());
    });
    if (stream.exitCode === null) {
      stream.kill("SIGTERM");
    }
    await waitForClose;
  }
});

test("OpenAI 风格 SSE 会自动打开聚合后的 Response tab", async ({
  page,
  request,
}) => {
  await clearTraffic(request);
  const server = await startOpenAiLikeSseServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const ssePath = `/openai-sse-test?token=${token}`;

  await streamSseViaProxy(`http://127.0.0.1:${server.port}${ssePath}`);

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  let recordId: string | null = null;
  await expect
    .poll(async () => {
      const row = page
        .getByTestId("traffic-row")
        .filter({ hasText: "/openai-sse-test" })
        .last();
      recordId = await row.getAttribute("data-record-id");
      return recordId;
    })
    .toMatch(/^REQ-/);

  const sseRow = page.locator(
    `[data-testid="traffic-row"][data-record-id="${recordId}"]`,
  );
  await sseRow.click();

  await expect(page.getByTestId("response-tab-openai")).toBeVisible();
  await expect(page.getByTestId("traffic-detail")).toContainText("Model Info");
  await expect(page.getByTestId("traffic-detail")).toContainText("gpt-4o-mini");
  await expect(page.getByTestId("traffic-detail")).toContainText(
    "token-1token-2token-3",
  );

  await server.close();
});

test("push 重连后已删除记录应从列表移除", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const keepPath = `/reconnect-keep-${token}`;
  const deletePath1 = `/reconnect-del1-${token}`;
  const deletePath2 = `/reconnect-del2-${token}`;
  let pushConnections = 0;
  let closedPushConnections = 0;
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/api/push")) return;
    pushConnections += 1;
    socket.on("close", () => {
      closedPushConnections += 1;
    });
  });

  try {
    await sendProxyRequest(`http://127.0.0.1:${server.port}${keepPath}`);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${deletePath1}`);
    await sendProxyRequest(`http://127.0.0.1:${server.port}${deletePath2}`);

    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const keepRow = page
      .getByTestId("traffic-row")
      .filter({ hasText: keepPath })
      .first();
    const delRow1 = page
      .getByTestId("traffic-row")
      .filter({ hasText: deletePath1 })
      .first();
    const delRow2 = page
      .getByTestId("traffic-row")
      .filter({ hasText: deletePath2 })
      .first();
    await expect(keepRow).toBeVisible();
    await expect(delRow1).toBeVisible();
    await expect(delRow2).toBeVisible();
    await expect.poll(() => pushConnections).toBeGreaterThan(0);

    const keepId = await keepRow.getAttribute("data-record-id");
    const delId1 = await delRow1.getAttribute("data-record-id");
    const delId2 = await delRow2.getAttribute("data-record-id");
    expect(keepId).toBeTruthy();
    expect(delId1).toBeTruthy();
    expect(delId2).toBeTruthy();

    await setDocumentVisibility(page, "hidden");
    await expect.poll(() => pushConnections - closedPushConnections).toBe(0);
    const connectionsBeforeResume = pushConnections;

    for (const id of [delId1!, delId2!]) {
      const response = await request.delete(`${apiBase}/traffic`, {
        data: { ids: [id] },
      });
      expect(response.ok()).toBeTruthy();
      const deleted = await request.get(
        `${apiBase}/traffic/${encodeURIComponent(id)}`,
      );
      expect(deleted.status()).toBe(404);
    }
    const retained = await request.get(
      `${apiBase}/traffic/${encodeURIComponent(keepId!)}`,
    );
    expect(retained.ok()).toBeTruthy();
    // Both deletes happen offline, so all three cached rows must still be present.
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: token }),
    ).toHaveCount(3);

    await setDocumentVisibility(page, "visible");
    await expect
      .poll(() => pushConnections)
      .toBeGreaterThan(connectionsBeforeResume);
    await expect(delRow1).toHaveCount(0);
    await expect(delRow2).toHaveCount(0);
    await expect(keepRow).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: token }),
    ).toHaveCount(1);
  } finally {
    await setDocumentVisibility(page, "visible");
    await server.close();
  }
});
