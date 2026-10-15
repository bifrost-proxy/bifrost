// Legacy traffic suite: extract focused fixture modules as lifecycle ownership
// is isolated, preserving the registered end-to-end cases and assertions.
import { test, expect } from "@playwright/test";
import {
  execFileAsync,
  proxyUrl,
  proxyPort,
  apiBase,
  decodeQueryJson,
  startMockServer,
  sendProxyRequest,
  clearTraffic,
  waitForRecordedTraffic,
  startIsolatedBackend,
  waitForTrafficRecordByApi,
  updateTrafficClientApp,
  startTemporaryProxy,
  waitForClientApp,
  searchTraffic,
  queryTraffic,
  getTrafficStatistics,
} from "./helpers/traffic-fixtures";

test("加载流量列表并显示详情", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/hello-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
  await page.reload();
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
  await expect(row).toBeVisible();
  await expect(row).toContainText(proxyPort);
  const firstRow = row;
  await expect(firstRow).toHaveAttribute(
    "data-response-size",
    expect.stringMatching(/^[1-9]\d*$/),
  );
  await firstRow.click();

  const header = page.getByTestId("traffic-detail-header");
  await expect(header).toContainText("GET");
  await expect(header).toHaveAttribute(
    "data-url",
    expect.stringContaining("/hello"),
  );
  await expect(page.getByTestId("traffic-detail")).toContainText("Proxy Port");
  await expect(page.getByTestId("traffic-detail")).toContainText(proxyPort);

  await server.close();
});

test("独立详情路由加载单条请求", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/detail-route-${token}`;

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
    await page.reload();

    const row = page
      .getByTestId("traffic-row")
      .filter({ hasText: path })
      .first();
    await expect(row).toBeVisible();
    const recordId = await row.getAttribute("data-record-id");
    expect(recordId).toBeTruthy();

    await page.goto(`/_bifrost/traffic/detail?id=${recordId}`);
    await expect(page).toHaveURL(
      new RegExp(`/traffic/detail\\?id=${recordId}$`),
    );
    await expect(page.getByText(`Request ID: ${recordId}`)).toBeVisible();
    await expect(page.getByTestId("traffic-detail")).toBeVisible();
    await expect(page.getByTestId("traffic-detail-header")).toContainText(path);
  } finally {
    await server.close();
  }
});

test("详情页 Copy as cURL 会包含 POST body", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/copy-curl-body-${token}`;
  const body = JSON.stringify({ hello: token });

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page
      .context()
      .grantPermissions(["clipboard-read", "clipboard-write"]);

    await execFileAsync(
      "curl",
      [
        "-sS",
        "--fail",
        "--noproxy",
        "",
        "-x",
        proxyUrl,
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "--data",
        body,
        `http://127.0.0.1:${server.port}${path}`,
      ],
      { timeout: 10000 },
    );

    await page.reload();

    const row = page
      .getByTestId("traffic-row")
      .filter({ hasText: path })
      .first();
    await expect(row).toBeVisible();
    await row.click();

    await page.getByTestId("traffic-detail-copy-trigger").click();
    await page.getByText("Copy as cURL").click();

    const readCopiedText = () =>
      page.evaluate(async () => navigator.clipboard.readText());

    await expect.poll(readCopiedText).toContain("--data");
    await expect.poll(readCopiedText).toContain(body);
  } finally {
    await server.close();
  }
});

test("左侧 Filters 展示基础请求数量", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(
      `http://127.0.0.1:${server.port}/filter-count-a-${token}`,
    );
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}/filter-count-b-${token}`,
    );

    await expect(page.getByLabel("Local (127.0.0.1) count")).toHaveText("2");
    await expect(page.getByLabel("127.0.0.1 count").first()).toHaveText("2");
  } finally {
    await server.close();
  }
});

test("切换页面后保留已加载流量并持续接收 push", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const firstPath = `/persist-first-${token}`;
  const secondPath = `/persist-second-${token}`;
  let holdOffPageSubscription = false;
  let releaseOffPageSubscription: (() => void) | undefined;
  let offPageInsertReceived = false;
  // Relay the real backend socket. Delay only the off-page subscription to
  // deterministically exercise an insert sent before the server sees it, but
  // after the browser has removed its traffic handlers. No frames are mocked.
  await page.routeWebSocket(/\/_bifrost\/api\/push(?:\?|$)/, (socket) => {
    const backend = socket.connectToServer();
    const held: Array<string | Buffer> = [];
    socket.onMessage((message) => {
      const subscription = JSON.parse(message.toString());
      if (holdOffPageSubscription && subscription.need_traffic === false) {
        held.push(message);
        releaseOffPageSubscription = () => {
          holdOffPageSubscription = false;
          for (const pending of held.splice(0)) backend.send(pending);
        };
      } else {
        backend.send(message);
      }
    });
    backend.onMessage((message) => {
      const frame = JSON.parse(message.toString());
      if (
        frame.type === "traffic_delta" &&
        JSON.stringify(frame.data.inserts).includes(secondPath)
      ) {
        offPageInsertReceived = true;
      }
      socket.send(message);
    });
  });

  try {
    await page.goto("/_bifrost/settings");
    await expect(page).toHaveURL(/\/_bifrost\/settings$/);

    await sendProxyRequest(`http://127.0.0.1:${server.port}${firstPath}`);
    await waitForRecordedTraffic(request, firstPath);

    await page.getByText("Network", { exact: true }).click();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.reload();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
    ).toBeVisible();

    holdOffPageSubscription = true;
    await page.getByText("Settings", { exact: true }).click();
    await expect(page).toHaveURL(/\/_bifrost\/settings$/);
    await expect.poll(() => Boolean(releaseOffPageSubscription)).toBeTruthy();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${secondPath}`);
    await waitForRecordedTraffic(request, secondPath);
    await expect.poll(() => offPageInsertReceived).toBeTruthy();
    releaseOffPageSubscription!();

    await page.getByText("Network", { exact: true }).click();
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: firstPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: secondPath }).first(),
    ).toBeVisible();
  } finally {
    await server.close();
  }
});

test("无规则时把 hop-by-hop 响应头标为协议处理", async ({ page, request }) => {
  await clearTraffic(request);
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/header-protocol-${token}`;
  const networkFile = `01 network

[meta]
name = "header-protocol-${token}"
version = "1.0.0"
created_at = "2026-08-05T00:00:00Z"

---
[{"id":"REQ-${token}","method":"GET","url":"https://example.test${path}","status":200,"has_rule_hit":false,"response_headers":[["content-type","application/json"]],"original_response_headers":[["content-type","application/json"],["connection","keep-alive"]],"duration_ms":1,"timestamp":1785900000000}]
`;
  const importResponse = await request.post(`${apiBase}/bifrost-file/import`, {
    data: networkFile,
    headers: { "Content-Type": "text/plain" },
  });
  expect(importResponse.ok()).toBeTruthy();

  await page.goto("/_bifrost/traffic");
  const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
  await expect(row).toBeVisible();
  await row.click();
  await page.getByTestId("response-tab-header").click();

  await expect(
    page
      .getByTestId("response-header-view-tab-current")
      .locator("xpath=ancestor::label"),
  ).toHaveText("Sent to client");
  await expect(
    page
      .getByTestId("response-header-view-tab-original")
      .locator("xpath=ancestor::label"),
  ).toHaveText("Upstream original");
  await expect(
    page.getByTestId("response-header-view-configured-summary"),
  ).toHaveText("Configured changes: 0");
  await expect(
    page.getByTestId("response-header-view-protocol-summary"),
  ).toHaveText("Protocol handling: 1");

  const protocolBadge = page.getByTestId("response-header-view-protocol-badge");
  await expect(protocolBadge).toHaveText("Protocol handling");
  const connectionRow = page.locator("tr", { hasText: "connection" });
  await expect(connectionRow).toBeVisible();
  expect(
    await connectionRow
      .locator("td")
      .first()
      .evaluate((element) => getComputedStyle(element).textDecorationLine),
  ).not.toContain("line-through");
  await protocolBadge.hover();
  await expect(page.getByRole("tooltip")).toContainText(
    "No rule, script, or breakpoint change is implied.",
  );

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(protocolBadge).toBeVisible();
});

test("Header 仅在存在差异时显示方向与来源", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/headers-plain-${token}`;
  const ruleName = `header-diff-${token}`;
  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: `127.0.0.1 resHeaders://X-UI-Header-Diff=${token}`,
      enabled: true,
    },
  });
  expect(createRuleRes.ok()).toBeTruthy();

  try {
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
    await page.reload();
    const row = page
      .getByTestId("traffic-row")
      .filter({ hasText: path })
      .first();
    await expect(row).toBeVisible();
    await row.click();
    await page.getByTestId("request-tab-header").click();

    await expect(page.getByTestId("request-header-view-mode-tabs")).toHaveCount(
      0,
    );
    await expect(
      page.getByTestId("request-header-view-tab-current"),
    ).toHaveCount(0);
    await expect(
      page.getByTestId("request-header-view-tab-original"),
    ).toHaveCount(0);

    await page.getByTestId("response-tab-header").click();
    await expect(
      page.getByTestId("response-header-view-mode-tabs"),
    ).toBeVisible();
    await expect(
      page
        .getByTestId("response-header-view-tab-current")
        .locator("xpath=ancestor::label"),
    ).toHaveText("Sent to client");
    await expect(
      page
        .getByTestId("response-header-view-tab-original")
        .locator("xpath=ancestor::label"),
    ).toHaveText("Upstream original");
    await expect(
      page.getByTestId("response-header-view-configured-summary"),
    ).toContainText("Configured changes: 1");
  } finally {
    await request.delete(`${apiBase}/rules/${encodeURIComponent(ruleName)}`);
    await server.close();
  }
});

test("清空流量时前端等待服务端完成后清理", async ({ page, request }) => {
  await clearTraffic(request);
  const server = await startMockServer();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const path = `/clear-${token}`;

  await page.goto("/_bifrost/traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
  const row = page.getByTestId("traffic-row").filter({ hasText: path }).first();
  await expect(row).toBeVisible();
  const localClientCount = page.getByLabel("Local (127.0.0.1) count");
  await expect(localClientCount).toHaveText("1");

  let deleteSeen = false;
  await page.route("**/_bifrost/api/traffic", async (route, req) => {
    if (req.method() === "DELETE") {
      deleteSeen = true;
      await new Promise((r) => setTimeout(r, 2000));
    }
    await route.continue();
  });

  await page.getByTestId("toolbar-clear-all").click();

  await expect(page.getByTestId("toolbar-clear-all")).toBeDisabled();
  await expect(row).toBeVisible();
  await expect(localClientCount).toHaveText("1", { timeout: 500 });
  expect(deleteSeen).toBeTruthy();
  await expect(row).toHaveCount(0, { timeout: 5000 });
  await expect(localClientCount).toHaveCount(0, { timeout: 5000 });

  await server.close();
});

test("Client App 空值与模糊匹配会同步作用于列表查询和 Fuzzy Search", async () => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const emptyClientPath = `/client-empty-${token}`;
  const fuzzyClientPath = `/client-fuzzy-${token}`;
  const fuzzyClientApp = `Client Search ${token}`;
  const fuzzyKeyword = token.slice(0, 6);

  try {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${emptyClientPath}`,
      backend.proxyUrl,
    );
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${fuzzyClientPath}`,
      backend.proxyUrl,
    );

    const emptyRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      emptyClientPath,
    );
    const fuzzyRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      fuzzyClientPath,
    );

    await updateTrafficClientApp(backend.dataDir, emptyRecord.id, "");
    await updateTrafficClientApp(
      backend.dataDir,
      fuzzyRecord.id,
      fuzzyClientApp,
    );
    await waitForClientApp(backend.baseApi, emptyRecord.id, "");
    await waitForClientApp(backend.baseApi, fuzzyRecord.id, fuzzyClientApp);

    const emptyTraffic = await queryTraffic(backend.baseApi, {
      client_app_empty: "true",
      limit: "50",
    });
    expect(emptyTraffic.records.map((item) => item.p)).toContain(
      emptyClientPath,
    );
    expect(emptyTraffic.records.map((item) => item.p)).not.toContain(
      fuzzyClientPath,
    );

    const emptySearch = await searchTraffic(backend.baseApi, [
      { field: "client_app", operator: "is_empty", value: "" },
    ]);
    expect(emptySearch.results.map((item) => item.record.p)).toContain(
      emptyClientPath,
    );
    expect(emptySearch.results.map((item) => item.record.p)).not.toContain(
      fuzzyClientPath,
    );

    const fuzzyTraffic = await queryTraffic(backend.baseApi, {
      client_app: fuzzyKeyword,
      client_app_match: "contains",
      limit: "50",
    });
    expect(fuzzyTraffic.records.map((item) => item.p)).toContain(
      fuzzyClientPath,
    );
    expect(fuzzyTraffic.records.map((item) => item.p)).not.toContain(
      emptyClientPath,
    );

    const fuzzySearch = await searchTraffic(backend.baseApi, [
      { field: "client_app", operator: "contains", value: fuzzyKeyword },
    ]);
    expect(fuzzySearch.results.map((item) => item.record.p)).toContain(
      fuzzyClientPath,
    );
    expect(fuzzySearch.results.map((item) => item.record.p)).not.toContain(
      emptyClientPath,
    );
  } finally {
    await backend.close();
    await server.close();
  }
});

test("主筛选器支持按代理端口过滤 Traffic", async ({ page }) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const mainPortPath = `/port-filter-main-${token}`;
  const otherPortPath = `/port-filter-other-${token}`;

  try {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${mainPortPath}`,
      backend.proxyUrl,
    );
    const otherPort = await startTemporaryProxy(backend.baseApi);
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${otherPortPath}`,
      `http://127.0.0.1:${otherPort}`,
    );

    const mainRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      mainPortPath,
    );
    const otherRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      otherPortPath,
    );
    expect(otherRecord.lp).toBe(otherPort);
    await expect
      .poll(
        async () => (await getTrafficStatistics(backend.baseApi)).proxy_ports,
      )
      .toEqual({ [backend.port]: 1, [otherPort]: 1 });

    const portTraffic = await queryTraffic(backend.baseApi, {
      listener_port: String(backend.port),
      limit: "50",
    });
    expect(portTraffic.records.map((item) => item.p)).toContain(mainPortPath);
    expect(portTraffic.records.map((item) => item.p)).not.toContain(
      otherPortPath,
    );
    expect(mainRecord.lp).toBe(backend.port);

    const portSearch = await searchTraffic(backend.baseApi, [
      {
        field: "listener_port",
        operator: "equals",
        value: String(backend.port),
      },
    ]);
    expect(portSearch.results.map((item) => item.record.p)).toContain(
      mainPortPath,
    );
    expect(portSearch.results.map((item) => item.record.p)).not.toContain(
      otherPortPath,
    );

    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }).first(),
    ).toBeVisible();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: otherPortPath })
        .first(),
    ).toBeVisible();

    if ((await page.getByPlaceholder("Enter value...").count()) === 0) {
      await page.getByRole("button", { name: "Add Filter" }).click();
    }
    await page.getByRole("combobox").first().click();
    await page
      .locator(
        ".ant-select-dropdown:not(.ant-select-dropdown-hidden) .ant-select-item-option-content",
      )
      .filter({ hasText: "Port" })
      .click();
    await expect(page.getByText("Equals").first()).toBeVisible();
    await page
      .getByPlaceholder("Enter proxy port...")
      .first()
      .fill(String(backend.port));

    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPortPath }),
    ).toHaveCount(0);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("左侧筛选器仅在多代理端口时展示 Proxy port 并同步到 URL", async ({
  page,
}) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const mainPortPath = `/panel-port-main-${token}`;
  const otherPortPath = `/panel-port-other-${token}`;

  try {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${mainPortPath}`,
      backend.proxyUrl,
    );
    const mainRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      mainPortPath,
    );

    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }).first(),
    ).toBeVisible();
    await expect(page.getByTestId("filter-section-proxy-port")).toHaveCount(0);
    expect(mainRecord.lp).toBe(backend.port);

    const otherPort = await startTemporaryProxy(backend.baseApi);
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${otherPortPath}`,
      `http://127.0.0.1:${otherPort}`,
    );
    const otherRecord = await waitForTrafficRecordByApi(
      backend.baseApi,
      otherPortPath,
    );
    expect(otherRecord.lp).toBe(otherPort);
    await expect
      .poll(
        async () => (await getTrafficStatistics(backend.baseApi)).proxy_ports,
      )
      .toEqual({ [backend.port]: 1, [otherPort]: 1 });

    await page.reload();
    await expect(page.getByTestId("filter-section-proxy-port")).toBeVisible();
    await expect(
      page
        .getByTestId("filter-section-proxy-port")
        .getByTestId("filter-item-proxy_port"),
    ).toHaveCount(2);

    await page
      .getByTestId("filter-section-proxy-port")
      .locator(`[data-filter-value="${otherPort}"]`)
      .click();

    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: otherPortPath })
        .first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }),
    ).toHaveCount(0);
    await expect(page.getByTestId("filter-selection-summary")).toContainText(
      "Port 1",
    );
    await expect(page.getByTestId("filter-clear-active")).toBeVisible();

    const panelParam = new URL(page.url()).searchParams.get("panel");
    expect(panelParam).toBeTruthy();
    expect(
      decodeQueryJson<{ proxyPorts?: string[] }>(panelParam || "").proxyPorts,
    ).toEqual([String(otherPort)]);

    await page.reload();
    await expect(
      page
        .getByTestId("traffic-row")
        .filter({ hasText: otherPortPath })
        .first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: mainPortPath }),
    ).toHaveCount(0);
  } finally {
    await backend.close();
    await server.close();
  }
});

test("主筛选器支持临时停用单条条件", async ({ page }) => {
  const server = await startMockServer();
  const backend = await startIsolatedBackend();
  const token = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const targetPath = `/filter-enabled-target-${token}`;
  const otherPath = `/filter-enabled-other-${token}`;

  try {
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${targetPath}`,
      backend.proxyUrl,
    );
    await sendProxyRequest(
      `http://127.0.0.1:${server.port}${otherPath}`,
      backend.proxyUrl,
    );
    await waitForTrafficRecordByApi(backend.baseApi, targetPath);
    await waitForTrafficRecordByApi(backend.baseApi, otherPath);

    await page.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }).first(),
    ).toBeVisible();

    await page.getByRole("button", { name: "Add Filter" }).click();
    const enabledCheckbox = page
      .getByRole("checkbox", { name: "Enable filter" })
      .first();
    await expect(enabledCheckbox).toBeChecked();

    await page.getByRole("combobox").first().click();
    await page
      .locator(
        ".ant-select-dropdown:not(.ant-select-dropdown-hidden) .ant-select-item-option-content",
      )
      .filter({ hasText: "Path" })
      .click();
    await page.getByPlaceholder("Enter value...").first().fill(targetPath);

    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }),
    ).toHaveCount(0);

    await enabledCheckbox.uncheck();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }).first(),
    ).toBeVisible();

    await enabledCheckbox.check();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: targetPath }).first(),
    ).toBeVisible();
    await expect(
      page.getByTestId("traffic-row").filter({ hasText: otherPath }),
    ).toHaveCount(0);
  } finally {
    await backend.close();
    await server.close();
  }
});
