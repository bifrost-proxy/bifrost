// Legacy traffic suite: extract focused fixture modules as lifecycle ownership
// is isolated, preserving the registered end-to-end cases and assertions.
import { test, expect } from "@playwright/test";
import {
  startMockServer,
  sendProxyRequest,
  clearTraffic,
  startIsolatedBackend,
  queryTraffic,
  getTrafficStatistics,
  updateTrafficMaxRecords,
  setDocumentVisibility,
  installTrafficBurstRecorder,
  readTrafficBurstStats,
  installTrafficStatisticsRecorder,
  readTrafficStatisticsFrames,
  readTrafficStatisticsDiagnostics,
  seedTrafficBatch,
} from "./helpers/traffic-fixtures";

test("实时更新与页面刷新保持数据", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const firstPath = `/first-${token}`;
  const secondPath = `/second-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${firstPath}`);
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
  ).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${secondPath}`);
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
  ).toBeVisible();

  await page.reload();
  await expect(
    page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
  ).toBeVisible();

  await server.close();
});

test("刷新时首屏仍能保留最新窗口中的筛选结果", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPrefix = `/latest-window-${token}`;
  const filterParam = Buffer.from(
    JSON.stringify([
      {
        id: `filter-${token}`,
        field: "path",
        operator: "contains",
        value: targetPrefix,
      },
    ]),
  ).toString("base64url");

  try {
    const paths = Array.from(
      { length: 1000 },
      (_, index) => `/noise-${token}-${index}`,
    );
    paths.push(`${targetPrefix}-a`, `${targetPrefix}-b`);
    await seedTrafficBatch(paths, server.port);

    await page.goto(`/_bifrost/traffic?filter=${filterParam}`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-a` })
        .first(),
    ).toBeVisible();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-b` })
        .first(),
    ).toBeVisible();

    await page.reload();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-a` })
        .first(),
    ).toBeVisible();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-b` })
        .first(),
    ).toBeVisible();
  } finally {
    await server.close();
  }
});

test("有界筛选扫描仍会命中首屏之外的旧记录", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPrefix = `/historic-backfill-${token}`;
  const filterParam = Buffer.from(
    JSON.stringify([
      {
        id: `filter-historic-${token}`,
        field: "path",
        operator: "contains",
        value: targetPrefix,
      },
    ]),
  ).toString("base64url");

  try {
    const paths = [`${targetPrefix}-a`, `${targetPrefix}-b`];
    paths.push(
      ...Array.from(
        { length: 1000 },
        (_, index) => `/historic-noise-${token}-${index}`,
      ),
    );
    await seedTrafficBatch(paths, server.port);

    await page.goto(`/_bifrost/traffic?filter=${filterParam}`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-a` })
        .first(),
    ).toBeVisible({ timeout: 15000 });
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: `${targetPrefix}-b` })
        .first(),
    ).toBeVisible({ timeout: 15000 });
  } finally {
    await server.close();
  }
});

test("Network 大历史使用 1000 条双向滑动窗口且服务端统计保持完整", async ({
  page,
}) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const paths = Array.from(
    { length: 2300 },
    (_, index) => `/bounded-window-${token}-${index}`,
  );

  try {
    await seedTrafficBatch(paths, server.port, backend.proxyUrl);
    await expect
      .poll(
        async () => (await queryTraffic(backend.baseApi, { limit: "1" })).total,
      )
      .toBeGreaterThanOrEqual(paths.length);
    const oldestPath = (
      await queryTraffic(backend.baseApi, { direction: "forward", limit: "1" })
    ).records[0]?.p;
    const newestPath = (
      await queryTraffic(backend.baseApi, { direction: "backward", limit: "1" })
    ).records[0]?.p;
    expect(oldestPath).toBeTruthy();
    expect(newestPath).toBeTruthy();
    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);

    const table = page.getByTestId("traffic-table");
    const scroll = page.getByTestId("traffic-table-scroll");
    await expect(table).toBeVisible();
    await expect(table).toHaveAttribute("data-loaded-count", "500");
    await page.waitForTimeout(750);
    await expect(table).toHaveAttribute("data-loaded-count", "500");

    const statistics = await getTrafficStatistics(backend.baseApi);
    expect(statistics.total_requests).toBeGreaterThanOrEqual(paths.length);
    const localCount = statistics.client_ips["127.0.0.1"];
    expect(localCount).toBeGreaterThanOrEqual(paths.length);
    await expect(
      page.locator(
        '[data-testid="filter-item-client_ip"][data-filter-value="127.0.0.1"]',
      ),
    ).toContainText(localCount.toLocaleString());
    const domainCount = statistics.domains["127.0.0.1"];
    expect(domainCount).toBeGreaterThanOrEqual(paths.length);
    await expect(
      page.locator(
        '[data-testid="filter-item-domain"][data-filter-value="127.0.0.1"]',
      ),
    ).toContainText(domainCount.toLocaleString());

    for (let pageIndex = 0; pageIndex < 4; pageIndex += 1) {
      await scroll.evaluate((element) => {
        element.scrollTop = 0;
        element.dispatchEvent(new Event("scroll"));
      });
      await scroll.hover();
      await page.mouse.wheel(0, -600);
      await expect(table).toHaveAttribute("data-loaded-count", "1000");
    }

    await scroll.evaluate((element) => {
      element.scrollTop = 0;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: oldestPath! }).first(),
    ).toBeVisible();

    for (let pageIndex = 0; pageIndex < 3; pageIndex += 1) {
      await scroll.evaluate((element) => {
        element.scrollTop = element.scrollHeight;
        element.dispatchEvent(new Event("scroll"));
      });
      await scroll.hover();
      await page.mouse.wheel(0, 600);
      await expect(table).toHaveAttribute("data-loaded-count", "1000");
    }
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: newestPath! }).first(),
    ).toBeVisible();
    await expect(table).toHaveAttribute("data-loaded-count", "1000");

    await page
      .locator(
        '[data-testid="app-sidebar-nav-item"][data-nav-label="Settings"]',
      )
      .click();
    await expect(page).toHaveURL(/\/settings/);
    await page
      .locator('[data-testid="app-sidebar-nav-item"][data-nav-label="Network"]')
      .click();
    await expect(page).toHaveURL(/\/traffic/);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect
      .poll(async () =>
        Number(
          (await page
            .getByTestId("traffic-table")
            .getAttribute("data-loaded-count")) ?? "0",
        ),
      )
      .toBeLessThanOrEqual(1000);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("Traffic 统计通过 WebSocket 按变化推送且突发流量每秒最多一帧", async ({
  page,
}, testInfo) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const paths = Array.from(
    { length: 50 },
    (_, index) => `/statistics-push-${token}-${index}`,
  );

  let seedWindow: { startedAt: number; finishedAt?: number } | undefined;
  try {
    await installTrafficStatisticsRecorder(page);
    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect
      .poll(async () => (await readTrafficStatisticsFrames(page)).length)
      .toBeGreaterThanOrEqual(1);

    const initialFrames = await readTrafficStatisticsFrames(page);
    expect(initialFrames.at(-1)?.totalRequests).toBe(0);
    await page.waitForTimeout(1_100);
    expect(await readTrafficStatisticsFrames(page)).toHaveLength(
      initialFrames.length,
    );

    seedWindow = { startedAt: Date.now() };
    await seedTrafficBatch(paths, server.port, backend.proxyUrl, paths.length);
    seedWindow.finishedAt = Date.now();
    await expect
      .poll(
        async () =>
          (await getTrafficStatistics(backend.baseApi)).total_requests,
      )
      .toBe(paths.length);
    await expect
      .poll(async () => {
        const frames = await readTrafficStatisticsFrames(page);
        return frames.at(-1)?.totalRequests ?? 0;
      })
      .toBe(paths.length);

    const burstFrames = await readTrafficStatisticsFrames(page);
    expect(burstFrames.length).toBeGreaterThan(initialFrames.length);
    // Preparing 50 real requests can span multiple server ticks. Enforce
    // the one-second throttle for every pair using the existing 900ms
    // browser-delivery tolerance, rather than assume the whole seed is one tick.
    for (
      let index = initialFrames.length;
      index < burstFrames.length;
      index++
    ) {
      expect(burstFrames[index].connectionId).toBe(
        initialFrames.at(-1)!.connectionId,
      );
      expect(
        burstFrames[index].receivedAt - burstFrames[index - 1].receivedAt,
      ).toBeGreaterThanOrEqual(900);
    }
    expect(burstFrames.at(-1)?.totalRequests).toBe(paths.length);

    const statistics = await getTrafficStatistics(backend.baseApi);
    expect(statistics.total_requests).toBe(paths.length);
    await expect(
      page.locator(
        '[data-testid="filter-item-client_ip"][data-filter-value="127.0.0.1"]',
      ),
    ).toContainText(statistics.client_ips["127.0.0.1"].toLocaleString());

    await page.waitForTimeout(1_100);
    expect(await readTrafficStatisticsFrames(page)).toHaveLength(
      burstFrames.length,
    );
  } finally {
    try {
      await testInfo.attach("statistics-timing-and-subscriptions", {
        body: JSON.stringify(
          { seedWindow, ...(await readTrafficStatisticsDiagnostics(page)) },
          null,
          2,
        ),
        contentType: "application/json",
      });
    } finally {
      await backend.close();
      await server.close();
    }
  }
});

test("Search 在组合条件下按 WebSocket 变更实时增量重算且定向查询有界", async ({
  page,
}) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const initialPath = `/live-search-${token}-initial`;
  const livePath = `/live-search-${token}-pushed`;
  const nonMatchingPath = `/unrelated-${Date.now()}`;

  try {
    await seedTrafficBatch(
      [initialPath, nonMatchingPath],
      server.port,
      backend.proxyUrl,
    );
    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.getByRole("button", { name: "Fuzzy Search" }).click();
    const input = page.getByPlaceholder(
      "Enter keyword to search all content...",
    );
    await input.fill(`live-search-${token}`);
    await page.getByTestId("search-mode-submit").click();
    await expect(
      page.getByText(initialPath, { exact: false }).first(),
    ).toBeVisible();

    await seedTrafficBatch(
      [livePath, `/unrelated-live-${token}`],
      server.port,
      backend.proxyUrl,
    );
    await expect(
      page.getByText(livePath, { exact: false }).first(),
    ).toBeVisible({
      timeout: 5_000,
    });
    await expect(
      page.getByText(`/unrelated-live-${token}`, { exact: false }),
    ).toHaveCount(0);

    const records = await queryTraffic(backend.baseApi, {
      limit: "100",
      direction: "backward",
    });
    const initialId = records.records.find(
      (record) => record.p === initialPath,
    )?.id;
    const liveId = records.records.find((record) => record.p === livePath)?.id;
    const unrelatedId = records.records.find(
      (record) => record.p === `/unrelated-live-${token}`,
    )?.id;
    expect(initialId).toBeTruthy();
    expect(liveId).toBeTruthy();
    expect(unrelatedId).toBeTruthy();

    const csrfResponse = await fetch(`${backend.baseApi}/security/csrf`);
    const csrf = (await csrfResponse.json()) as {
      csrf_token: string;
      header_name?: string;
    };
    const targetedResponse = await fetch(`${backend.baseApi}/search`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        [csrf.header_name || "X-Bifrost-CSRF"]: csrf.csrf_token,
      },
      body: JSON.stringify({
        keyword: `live-search-${token}`,
        scope: { all: false, url: true },
        filters: {
          protocols: ["HTTP"],
          status_ranges: ["2xx"],
          content_types: ["application/json"],
          conditions: [
            { field: "method", operator: "equals", value: "GET" },
            {
              field: "path",
              operator: "contains",
              value: `/live-search-${token}`,
            },
          ],
          client_ips: ["127.0.0.1"],
          client_apps: [],
          account_names: [],
          domains: ["127.0.0.1"],
        },
        record_ids: [initialId, liveId, unrelatedId],
        limit: 500,
        max_scan: 500,
        max_results: 500,
      }),
    });
    const targetedBody = await targetedResponse.text();
    expect(
      targetedResponse.ok,
      `targeted search failed (${targetedResponse.status}): ${targetedBody}`,
    ).toBeTruthy();
    const targeted = JSON.parse(targetedBody) as {
      results: Array<{ record: { id: string; p: string } }>;
    };
    expect(targeted.results.map((item) => item.record.id).sort()).toEqual(
      [initialId!, liveId!].sort(),
    );

    const oversizedResponse = await fetch(`${backend.baseApi}/search`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        [csrf.header_name || "X-Bifrost-CSRF"]: csrf.csrf_token,
      },
      body: JSON.stringify({
        keyword: token,
        scope: { all: true },
        filters: {},
        record_ids: Array.from({ length: 501 }, (_, index) => `id-${index}`),
      }),
    });
    expect(oversizedResponse.status).toBe(400);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("服务端 3000 条真实流量压力下按配置滚动淘汰", async () => {
  test.setTimeout(600_000);
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const paths = Array.from(
    { length: 3_000 },
    (_, index) => `/rolling-storage-stress-${token}-${index}`,
  );

  try {
    await updateTrafficMaxRecords(backend.baseApi, 1_000);
    const startedAt = Date.now();
    await seedTrafficBatch(paths, server.port, backend.proxyUrl);
    await expect
      .poll(
        async () => {
          const snapshot = await queryTraffic(backend.baseApi, {
            limit: "1",
            direction: "forward",
          });
          return (
            snapshot.server_sequence >= paths.length + 1 &&
            snapshot.total <= 1_150
          );
        },
        { timeout: 30_000 },
      )
      .toBe(true);

    const snapshot = await queryTraffic(backend.baseApi, {
      limit: "1",
      direction: "forward",
    });
    expect(snapshot.total).toBeGreaterThanOrEqual(800);
    expect(snapshot.total).toBeLessThanOrEqual(1_150);
    expect(snapshot.records[0]?.seq).toBeGreaterThan(1);
    expect(Date.now() - startedAt).toBeLessThan(480_000);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("服务端滚动淘汰后休眠恢复洪峰仍保持有界且可交互", async ({ page }) => {
  test.setTimeout(300_000);
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const initialPaths = Array.from(
    { length: 600 },
    (_, index) => `/rolling-initial-${token}-${index}`,
  );
  const burstPaths = Array.from(
    { length: 600 },
    (_, index) => `/rolling-burst-${token}-${index}`,
  );
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  try {
    await updateTrafficMaxRecords(backend.baseApi, 1_000);
    await seedTrafficBatch(initialPaths, server.port, backend.proxyUrl);
    await installTrafficBurstRecorder(page);

    const initialPush = page.waitForEvent("websocket", (socket) =>
      socket.url().includes("/api/push"),
    );
    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await initialPush;
    const table = page.getByTestId("traffic-table");
    await expect(table).toBeVisible();
    await expect(table).toHaveAttribute("data-loaded-count", "500");

    await setDocumentVisibility(page, "hidden");
    await page.waitForTimeout(500);
    await seedTrafficBatch(burstPaths, server.port, backend.proxyUrl);

    await expect
      .poll(
        async () => {
          const snapshot = await queryTraffic(backend.baseApi, { limit: "1" });
          return (
            snapshot.server_sequence >=
              initialPaths.length + burstPaths.length + 1 &&
            snapshot.total <= 1_150
          );
        },
        { timeout: 30_000 },
      )
      .toBe(true);
    const serverSnapshot = await queryTraffic(backend.baseApi, { limit: "1" });
    expect(serverSnapshot.total).toBeGreaterThanOrEqual(800);
    expect(serverSnapshot.total).toBeLessThanOrEqual(1_150);
    const newestBurstPath = serverSnapshot.records[0]?.p;
    expect(newestBurstPath).toMatch(new RegExp(`^/rolling-burst-${token}-`));

    await page.evaluate(() => {
      const state = {
        last: performance.now(),
        maxGap: 0,
        ticks: 0,
        timer: 0,
      };
      state.timer = window.setInterval(() => {
        const now = performance.now();
        state.maxGap = Math.max(state.maxGap, now - state.last);
        state.last = now;
        state.ticks += 1;
      }, 16);
      Object.defineProperty(window, "__trafficEventLoopProbe", {
        configurable: true,
        value: state,
      });
    });

    const resumedPush = page.waitForEvent("websocket", (socket) =>
      socket.url().includes("/api/push"),
    );
    await setDocumentVisibility(page, "visible");
    await resumedPush;

    await expect
      .poll(
        async () =>
          Number((await table.getAttribute("data-loaded-count")) ?? "0"),
        { timeout: 30_000 },
      )
      .toBe(serverSnapshot.total);
    const scroll = page.getByTestId("traffic-table-scroll");
    await scroll.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: newestBurstPath! })
        .first(),
    ).toBeVisible();

    await scroll.evaluate((element) => {
      element.scrollTop = 0;
      element.dispatchEvent(new Event("scroll"));
    });
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: initialPaths[200] })
        .first(),
    ).toHaveCount(0);

    await expect
      .poll(async () => (await readTrafficBurstStats(page)).deltaCount)
      .toBeGreaterThanOrEqual(2);
    const burstStats = await readTrafficBurstStats(page);
    expect(burstStats.maxBatch).toBeLessThanOrEqual(500);
    expect(burstStats.totalInserts).toBeLessThanOrEqual(2_500);
    expect(burstStats.latestOldestSequence).toBeGreaterThan(1);
    expect(burstStats.latestServerTotal).toBe(serverSnapshot.total);

    await page.waitForTimeout(500);
    const browserMetrics = await page.evaluate(() => {
      const probe = (
        window as typeof window & {
          __trafficEventLoopProbe: {
            maxGap: number;
            ticks: number;
            timer: number;
          };
        }
      ).__trafficEventLoopProbe;
      window.clearInterval(probe.timer);
      const memory = (
        performance as Performance & {
          memory?: { usedJSHeapSize?: number };
        }
      ).memory;
      return {
        maxEventLoopGapMs: probe.maxGap,
        eventLoopTicks: probe.ticks,
        usedJsHeapBytes: memory?.usedJSHeapSize ?? 0,
      };
    });
    expect(browserMetrics.eventLoopTicks).toBeGreaterThan(5);
    expect(browserMetrics.maxEventLoopGapMs).toBeLessThan(1_500);
    if (browserMetrics.usedJsHeapBytes > 0) {
      expect(browserMetrics.usedJsHeapBytes).toBeLessThan(512 * 1024 * 1024);
    }

    const settingsStartedAt = Date.now();
    await page
      .locator(
        '[data-testid="app-sidebar-nav-item"][data-nav-label="Settings"]',
      )
      .click();
    await expect(page).toHaveURL(/\/settings/);
    expect(Date.now() - settingsStartedAt).toBeLessThan(3_000);

    const networkStartedAt = Date.now();
    await page
      .locator('[data-testid="app-sidebar-nav-item"][data-nav-label="Network"]')
      .click();
    await expect(page).toHaveURL(/\/traffic/);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    expect(Date.now() - networkStartedAt).toBeLessThan(3_000);
    expect(pageErrors).toEqual([]);
  } finally {
    await backend.close();
    await server.close();
  }
});
