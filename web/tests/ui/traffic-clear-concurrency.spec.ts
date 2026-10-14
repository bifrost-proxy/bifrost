import { test, expect, type WebSocketRoute } from "@playwright/test";
import type { TrafficUpdatesResponseCompact } from "../../src/types";
import {
  apiBase,
  sendProxyRequest,
  startMockHttpServer,
  waitForTrafficRow,
  uniqueName,
} from "./helpers/admin-helpers";

test("并发清空期间转发持续可用，清空后落盘和推送恢复", async ({
  page,
  request,
}) => {
  const server = await startMockHttpServer();
  const token = uniqueName("clear-concurrent");
  const frames: string[] = [];
  page.on("websocket", (socket) => {
    if (socket.url().includes("/api/push")) {
      socket.on("framereceived", (frame) => frames.push(String(frame.payload)));
    }
  });
  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await sendProxyRequest(`http://127.0.0.1:${server.port}/${token}-before`);
    await waitForTrafficRow(page, `${token}-before`);
    const writers = Array.from({ length: 8 }, async (_, writer) => {
      for (let i = 0; i < 12; i++) {
        await sendProxyRequest(
          `http://127.0.0.1:${server.port}/${token}-during-${writer}-${i}`,
        );
      }
    });
    const clears = Array.from({ length: 32 }, async () => {
      const result = await request.delete(`${apiBase}/traffic`, {
        timeout: 15000,
      });
      expect(result.ok()).toBe(true);
    });
    await Promise.all([...writers, ...clears]);
    const after = Array.from({ length: 5 }, (_, i) => `${token}-after-${i}`);
    for (const path of after) {
      await sendProxyRequest(`http://127.0.0.1:${server.port}/${path}`);
    }
    for (const path of after) {
      await waitForTrafficRow(page, path);
    }
    await expect
      .poll(async () => {
        const result = await request.get(`${apiBase}/traffic?limit=1000`);
        expect(result.ok()).toBe(true);
        const text = await result.text();
        return after.every((path) => text.includes(path));
      })
      .toBe(true);
    expect(
      after.every((path) => frames.some((frame) => frame.includes(path))),
    ).toBe(true);
    expect(
      server.requests.filter((r) => r.url?.includes(`${token}-during`)),
    ).toHaveLength(96);
  } finally {
    await server.close();
  }
});

test("清空按钮等待结果，连点仅发一次，失败保留流量并允许重试", async ({
  page,
}) => {
  const server = await startMockHttpServer();
  const path = uniqueName("clear-failure");
  let requests = 0;
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => {
    release = resolve;
  });
  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await sendProxyRequest(`http://127.0.0.1:${server.port}/${path}`);
    const row = await waitForTrafficRow(page, path);
    await page.route("**/_bifrost/api/traffic", async (route) => {
      if (route.request().method() !== "DELETE") return route.continue();
      requests++;
      if (requests === 1) {
        await blocked;
        return route.fulfill({
          status: 500,
          contentType: "application/json",
          body: JSON.stringify({ error: "injected clear failure" }),
        });
      }
      return route.continue();
    });
    const button = page.getByTestId("toolbar-clear-all");
    await button.evaluate((element) => {
      (element as HTMLButtonElement).click();
      (element as HTMLButtonElement).click();
    });
    await expect(button).toBeDisabled();
    await expect(row).toBeVisible();
    await expect.poll(() => requests).toBe(1);
    await expect(
      page.getByText("Traffic cleared", { exact: true }),
    ).toHaveCount(0);
    release();
    await expect(button).toBeEnabled();
    await expect(row).toBeVisible();
    await expect(
      page.getByText("injected clear failure", { exact: true }),
    ).toBeVisible();
    await button.click();
    await expect(
      page.getByText("Traffic cleared", { exact: true }),
    ).toBeVisible();
    await expect(row).toHaveCount(0);
    await expect(button).toBeEnabled();
    expect(requests).toBe(2);
  } finally {
    release?.();
    await server.close();
  }
});

test("清空同步读取期间另一客户端删除，迟到更新不恢复旧行且保留新流量", async ({
  page,
  request,
}) => {
  const server = await startMockHttpServer();
  const snapshotPath = uniqueName("read-deleted");
  const livePath = uniqueName("read-live");
  let afterClear = false;
  let snapshotHeld = false;
  let release!: () => void;
  let snapshotReady!: (id: string) => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const ready = new Promise<string>((resolve) => {
    snapshotReady = resolve;
  });
  const deletedFrames: string[] = [];
  let pushSocket: WebSocketRoute | undefined;
  let delayedUpdate: string | undefined;
  let socketGeneration = 0;
  let registeredGeneration = 0;
  let subscribedGeneration = 0;
  let replacementGeneration = 0;
  let releaseReconnect!: () => void;
  const reconnectGate = new Promise<void>((resolve) => {
    releaseReconnect = resolve;
  });
  await page.routeWebSocket("**/api/push*", async (socket) => {
    const generation = ++socketGeneration;
    pushSocket = socket;
    // Make the replacement-connection gap deterministic. A GET response alone
    // does not prove that a new server push client has registered after clear.
    if (afterClear) {
      replacementGeneration = generation;
      await reconnectGate;
    }
    const serverSocket = socket.connectToServer();
    socket.onMessage((message) => {
      const subscription = JSON.parse(String(message)) as {
        need_traffic?: boolean;
      };
      if (subscription.need_traffic) subscribedGeneration = generation;
      serverSocket.send(message);
    });
    serverSocket.onMessage((message) => {
      const frame = JSON.parse(String(message)) as { type: string };
      if (frame.type === "connected") registeredGeneration = generation;
      deletedFrames.push(String(message));
      socket.send(message);
    });
  });
  try {
    const initialClear = await request.delete(`${apiBase}/traffic`);
    expect(initialClear.ok()).toBe(true);
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.route("**/_bifrost/api/traffic", async (route) => {
      if (route.request().method() !== "DELETE" || afterClear)
        return route.continue();
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      await sendProxyRequest(`http://127.0.0.1:${server.port}/${snapshotPath}`);
      await expect
        .poll(async () => {
          const response = await request.get(
            `${apiBase}/traffic/updates?limit=1000`,
          );
          return (await response.text()).includes(snapshotPath);
        })
        .toBe(true);
      afterClear = true;
      await route.fulfill({ response });
    });
    await page.route("**/_bifrost/api/traffic/updates*", async (route) => {
      if (!afterClear || snapshotHeld) return route.continue();
      const response = await route.fetch();
      const body = (await response.json()) as TrafficUpdatesResponseCompact;
      const record = body.new_records.find((record) =>
        record.p.includes(snapshotPath),
      );
      if (!record) return route.fulfill({ response });
      snapshotHeld = true;
      // Reproduce the server's separate event queues with an update built from
      // the real pre-delete row/metadata, delivered after the deletion frame.
      delayedUpdate = JSON.stringify({
        type: "traffic_delta",
        data: {
          inserts: [],
          updates: [record],
          has_more: false,
          server_total: body.server_total,
          server_sequence: body.server_sequence,
          database_epoch: body.database_epoch,
        },
      });
      snapshotReady(record.id);
      await held;
      await route.fulfill({ response });
    });
    await expect
      .poll(
        () =>
          registeredGeneration > 0 &&
          subscribedGeneration === registeredGeneration,
      )
      .toBe(true);
    const initialGeneration = socketGeneration;
    await page.getByTestId("toolbar-clear-all").click();
    const deletedId = await ready;
    releaseReconnect();
    await expect
      .poll(
        () =>
          replacementGeneration > initialGeneration &&
          registeredGeneration === replacementGeneration &&
          subscribedGeneration === replacementGeneration,
      )
      .toBe(true);
    const beforeDelete = await request.get(
      `${apiBase}/traffic/${encodeURIComponent(deletedId)}`,
    );
    expect(beforeDelete.ok()).toBe(true);
    const deletion = await request.delete(`${apiBase}/traffic`, {
      data: { ids: [deletedId] },
    });
    expect(deletion.ok()).toBe(true);
    const result = (await deletion.json()) as { message?: string };
    expect(result.message).toBe("1 traffic records cleared successfully");
    const afterDelete = await request.get(
      `${apiBase}/traffic/${encodeURIComponent(deletedId)}`,
    );
    expect(afterDelete.status()).toBe(404);
    await expect
      .poll(() =>
        deletedFrames.some(
          (frame) =>
            frame.includes("traffic_deleted") && frame.includes(deletedId),
        ),
      )
      .toBe(true);
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
    );
    expect(pushSocket).toBeDefined();
    expect(delayedUpdate).toBeDefined();
    pushSocket!.send(delayedUpdate!);
    // Give the throttled UI batch a chance to flush while the GET is held.
    // This extra controlled read delay exposes temporary resurrection too.
    await page.waitForTimeout(250);
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: snapshotPath }),
    ).toHaveCount(0);
    await sendProxyRequest(`http://127.0.0.1:${server.port}/${livePath}`);
    release();
    await expect(page.getByTestId("toolbar-clear-all")).toBeEnabled();
    await waitForTrafficRow(page, livePath);
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: snapshotPath }),
    ).toHaveCount(0);
    await expect(page.getByLabel("Local (127.0.0.1) count")).toHaveText("1");
  } finally {
    releaseReconnect();
    release?.();
    await server.close();
  }
});

test("清空成功后同步读取失败，按钮恢复且已接收的新流量保留", async ({
  page,
  request,
}) => {
  const server = await startMockHttpServer();
  const obsoletePath = uniqueName("sync-obsolete");
  const livePath = uniqueName("sync-survivor");
  let afterClear = false;
  let snapshotHeld = false;
  let replacementFailed = false;
  let release!: () => void;
  let ready!: (id: string) => void;
  const held = new Promise<void>((done) => {
    release = done;
  });
  const snapshotReady = new Promise<string>((done) => {
    ready = done;
  });
  const frames: string[] = [];
  page.on("websocket", (socket) => {
    if (socket.url().includes("/api/push"))
      socket.on("framereceived", (frame) => frames.push(String(frame.payload)));
  });
  try {
    expect((await request.delete(`${apiBase}/traffic`)).ok()).toBe(true);
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.route("**/_bifrost/api/traffic", async (route) => {
      if (route.request().method() !== "DELETE" || afterClear)
        return route.continue();
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      await sendProxyRequest(`http://127.0.0.1:${server.port}/${obsoletePath}`);
      afterClear = true;
      await route.fulfill({ response });
    });
    await page.route("**/_bifrost/api/traffic/updates*", async (route) => {
      if (!afterClear) return route.continue();
      if (snapshotHeld) {
        replacementFailed = true;
        return route.fulfill({
          status: 500,
          contentType: "application/json",
          body: JSON.stringify({ error: "injected replacement read failure" }),
        });
      }
      const response = await route.fetch();
      const body = (await response.json()) as TrafficUpdatesResponseCompact;
      const record = body.new_records.find((r) => r.p.includes(obsoletePath));
      expect(record).toBeDefined();
      snapshotHeld = true;
      ready(record!.id);
      await held;
      await route.fulfill({ response });
    });
    await page.getByTestId("toolbar-clear-all").click();
    const deletedId = await snapshotReady;
    await expect(page.getByTestId("toolbar-clear-all")).toBeEnabled();
    await expect(
      page.getByText("Traffic cleared", { exact: true }),
    ).toBeVisible();
    // Freeze only after the UI clear result is visible. This deterministically
    // leaves live writes in the pending batch when invalidation cancels it.
    await page.evaluate(() => {
      const raf = window.requestAnimationFrame.bind(window);
      const cancel = window.cancelAnimationFrame.bind(window);
      const queued = new Map<number, FrameRequestCallback>();
      let id = 0;
      window.requestAnimationFrame = (callback) => {
        queued.set(--id, callback);
        return id;
      };
      window.cancelAnimationFrame = (value) => {
        if (value < 0) queued.delete(value);
        else cancel(value);
      };
      (window as Window & { resumeTestFrames?: () => void }).resumeTestFrames =
        () => {
          window.requestAnimationFrame = raf;
          window.cancelAnimationFrame = cancel;
          for (const callback of queued.values()) raf(callback);
          queued.clear();
        };
    });
    await sendProxyRequest(`http://127.0.0.1:${server.port}/${livePath}`);
    await expect
      .poll(() => frames.some((frame) => frame.includes(livePath)))
      .toBe(true);
    const deletion = await request.delete(`${apiBase}/traffic`, {
      data: { ids: [deletedId] },
    });
    expect(deletion.ok()).toBe(true);
    expect((await deletion.json()).message).toBe(
      "1 traffic records cleared successfully",
    );
    await expect
      .poll(() =>
        frames.some(
          (frame) =>
            frame.includes("traffic_deleted") && frame.includes(deletedId),
        ),
      )
      .toBe(true);
    release();
    await expect.poll(() => replacementFailed).toBe(true);
    await page.evaluate(() =>
      (
        window as Window & { resumeTestFrames?: () => void }
      ).resumeTestFrames?.(),
    );
    await waitForTrafficRow(page, livePath);
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: obsoletePath }),
    ).toHaveCount(0);
    await expect(page.getByTestId("toolbar-clear-all")).toBeEnabled();
    await expect(page.getByLabel("Local (127.0.0.1) count")).toHaveText("1");
  } finally {
    release?.();
    await page
      .evaluate(() =>
        (
          window as Window & { resumeTestFrames?: () => void }
        ).resumeTestFrames?.(),
      )
      .catch(() => {});
    await server.close();
  }
});
