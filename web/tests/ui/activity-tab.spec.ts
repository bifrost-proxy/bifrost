import { expect, test, type Page, type Route } from "@playwright/test";
import { apiBase, openPage, uniqueName } from "./helpers/admin-helpers";

const emptyTlsConfig = {
  enable_tls_interception: false,
  intercept_exclude: [],
  intercept_include: [],
  app_intercept_exclude: [],
  app_intercept_include: [],
  ip_intercept_exclude: [],
  ip_intercept_include: [],
  unsafe_ssl: false,
  disconnect_on_config_change: true,
};

async function fulfillJson(route: Route, body: unknown) {
  await route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

function trafficRecord(id: string, seq: number, app: string, responseSize: number) {
  return {
    id,
    seq,
    ts: Date.now(),
    m: "GET",
    h: "example.test",
    p: `/activity-${seq}`,
    s: 200,
    ct: "text/plain",
    req_sz: 128,
    res_sz: responseSize,
    dur: 12,
    proto: "http",
    cip: "127.0.0.1",
    capp: app,
    cpid: 1000 + seq,
    flags: 0,
    fc: 0,
    st: new Date().toISOString(),
    et: new Date().toISOString(),
    rc: 0,
    rp: [],
  };
}

function activityMetricsSnapshot() {
  return {
    timestamp: Date.now(),
    memory_used: 1,
    memory_total: 1,
    memory_usage_percent: 100,
    cpu_usage: 1,
    total_requests: 1777,
    active_connections: 18,
    bytes_sent: 952107008,
    bytes_received: 33344717,
    total_traffic_bytes: 985451725,
    bytes_sent_rate: 113,
    bytes_received_rate: 57,
    qps: 0,
    max_qps: 0,
    max_bytes_sent_rate: 0,
    max_bytes_received_rate: 0,
    http: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    https: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    tunnel: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    ws: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    wss: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    h3: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    h3s: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
    socks5: { requests: 0, bytes_sent: 0, bytes_received: 0, active_connections: 0 },
  };
}

async function mockActivityApi(
  page: Page,
  pushedOverview?: { metrics: ReturnType<typeof activityMetricsSnapshot>; recorded: number },
) {
  const longHeaderValue = "ppe_old_" + "x".repeat(160);
  await page.route("**/_bifrost/api/**", async (route) => {
    const url = new URL(route.request().url());
    const apiPath = url.pathname.replace(/^\/_bifrost\/api/, "");

    if (apiPath === "/auth/status") {
      await fulfillJson(route, { auth_required: false });
      return;
    }
    if (apiPath === "/sync/status") {
      await fulfillJson(route, {
        enabled: false,
        auto_sync: false,
        remote_base_url: "",
        has_session: false,
        reachable: false,
        authorized: false,
        syncing: false,
        reason: "disabled",
      });
      return;
    }
    if (apiPath === "/config/tls") {
      await fulfillJson(route, emptyTlsConfig);
      return;
    }
    if (apiPath === "/config") {
      await fulfillJson(route, {
        tls: emptyTlsConfig,
        tray: {
          enabled: false,
          supported: false,
          system_stats_supported: false,
          show_system_stats: false,
          system_stats_items: {
            cpu: false,
            memory: false,
            disk: false,
            upload: false,
            download: false,
          },
        },
        port: 9900,
        host: "127.0.0.1",
      });
      return;
    }
    if (apiPath === "/metrics") {
      await fulfillJson(route, pushedOverview?.metrics ?? activityMetricsSnapshot());
      return;
    }
    if (apiPath === "/metrics/apps" || apiPath === "/metrics/hosts") {
      await fulfillJson(route, {
        items: [],
        summary: {
          total: 0,
          requests: 0,
          bytes_sent: 0,
          bytes_received: 0,
          total_traffic_bytes: 0,
        },
      });
      return;
    }
    if (apiPath === "/system/overview") {
      await fulfillJson(route, {
        system: {
          version: "0.0.0-test",
          device_name: "test",
          os: "macos",
          arch: "aarch64",
          cpu_logical_cores: 8,
          memory_total_bytes: 1,
          memory_available_bytes: 1,
          uptime_secs: 1,
          pid: 123,
        },
        metrics: pushedOverview?.metrics ?? activityMetricsSnapshot(),
        rules: { total: 30, enabled: 1 },
        traffic: { recorded: pushedOverview?.recorded ?? 1777 },
        server: { port: 9900, admin_url: "http://127.0.0.1:9900/_bifrost/" },
        pending_authorizations: 0,
      });
      return;
    }
    if (apiPath === "/proxy/system") {
      await fulfillJson(route, {
        supported: true,
        enabled: false,
        host: "127.0.0.1",
        port: 9900,
        bypass: "",
        managed_by_bifrost: false,
        configured_enabled: false,
      });
      return;
    }
    if (apiPath === "/proxy/cli") {
      await fulfillJson(route, {
        enabled: true,
        shell: "CLI",
        config_files: [],
        proxy_url: "127.0.0.1:9900",
      });
      return;
    }
    if (apiPath === "/traffic/updates") {
      await fulfillJson(route, {
        new_records: [
          trafficRecord("t-1", 1, "codex", 2207),
          trafficRecord("t-2", 2, "codex", 800),
          trafficRecord("t-3", 3, "Microsoft Edge Helper", 990),
          trafficRecord("t-4", 4, "Lark Helper", 235),
        ],
        updated_records: [],
        has_more: false,
        server_total: 1777,
        server_sequence: 4,
      });
      return;
    }
    if (apiPath === "/traffic/statistics") {
      await fulfillJson(route, {
        total_requests: 2500,
        server_sequence: 2501,
        client_ips: { "127.0.0.1": 2500 },
        proxy_ports: { "9900": 2500 },
        applications: { codex: 1750, "Microsoft Edge Helper": 750 },
        account_names: {},
        domains: { "example.test": 2500 },
      });
      return;
    }
    if (apiPath === "/rules/active-summary") {
      await fulfillJson(route, {
        total: 1,
        rules: [
          {
            name: "Default",
            rule_count: 1,
            group_id: null,
            group_name: null,
          },
        ],
        variable_conflicts: [],
        merged_content: [
          "# Global default rules.",
          "# These rules are always enabled and apply to every proxy listener.",
          "",
          `https://app.example.com/api/v1/oncall/ reqHeaders://{"x-tt-env":"${longHeaderValue}","x-use-ppe":"1"}`,
          "https://app.example.com/api/v1/oncall/ passthrough://",
          'https://app.example.com/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_new","x-use-ppe":"1"}',
          "https://app.example.com/api/v1/oncall/ passthrough://",
          'https://partial.example.test/api/internal/ reqHeaders://{"x-env":"narrow"}',
          'https://partial.example.test/api/ reqHeaders://{"x-env":"broad","x-stable":"keep"}',
          "a.com status://200",
        ].join("\n"),
      });
      return;
    }
    if (apiPath === "/ports") {
      await fulfillJson(route, [
        {
          port: 18888,
          host: "127.0.0.1",
          name: "Activity temporary port",
          status: "running",
          rule_refs: [{ type: "local_rule", name: "activity-temp-rule" }],
          missing_refs: [],
          created_at: Date.now(),
          updated_at: Date.now(),
        },
        {
          port: 18889,
          host: "127.0.0.1",
          name: "Second temporary port",
          status: "running",
          rule_refs: [{ type: "local_rule", name: "activity-temp-rule-two" }],
          missing_refs: [],
          created_at: Date.now(),
          updated_at: Date.now(),
        },
      ]);
      return;
    }
    if (apiPath === "/ports/18888/active-summary") {
      await fulfillJson(route, {
        port: 18888,
        total: 2,
        rules: [
          { name: "Default", rule_count: 1, group_id: null, group_name: null },
          { name: "activity-temp-rule", rule_count: 1, group_id: null, group_name: null },
        ],
        merged_content: "# Default\nactivity-temp.test status://219 resBody://(activity-temp-rule)",
      });
      return;
    }
    if (apiPath === "/ports/18889/active-summary") {
      await fulfillJson(route, {
        port: 18889,
        total: 2,
        rules: [
          { name: "Default", rule_count: 1, group_id: null, group_name: null },
          { name: "activity-temp-rule-two", rule_count: 1, group_id: null, group_name: null },
        ],
        merged_content: [
          "# Default",
          "activity-temp-two.test status://220 resBody://(activity-temp-rule-two)",
          'activity-temp-two.test reqHeaders://{"x-extra-long-header":"temporary-port-layout-' + "x".repeat(120) + '"}',
        ].join("\n"),
      });
      return;
    }
    if (apiPath === "/notifications/unread-count") {
      await fulfillJson(route, { unread_count: 0 });
      return;
    }
    if (apiPath === "/notifications/client-trust") {
      await fulfillJson(route, { items: [], untrusted_count: 0 });
      return;
    }
    if (apiPath === "/whitelist/pending" || apiPath === "/config/ip-tls/pending") {
      await fulfillJson(route, []);
      return;
    }
    if (apiPath === "/remote-invoke/pairings/pending") {
      await fulfillJson(route, { pairings: [] });
      return;
    }
    if (apiPath === "/mobile-devices") {
      await fulfillJson(route, {
        android: { adb_available: false, devices: [], message: "mocked" },
        ios: {
          supported: false,
          devices: [],
          configurator: { supported: false, cfgutil_available: false, message: "mocked" },
          message: "mocked",
        },
        ios_profile_url: "",
        ios_profile_qrcode_url: "",
      });
      return;
    }
    if (apiPath === "/system/version-check") {
      await fulfillJson(route, {
        has_update: false,
        current_version: "0.0.0-test",
        latest_version: null,
        release_highlights: [],
        release_url: null,
        checked_at: "2026-07-05T00:00:00Z",
      });
      return;
    }
    if (apiPath === "/rules") {
      await fulfillJson(route, []);
      return;
    }
    if (apiPath === "/scripts") {
      await fulfillJson(route, { request: [], response: [], decode: [], parser: [] });
      return;
    }
    if (apiPath === "/syntax") {
      await fulfillJson(route, {
        protocols: [],
        template_variables: [],
        patterns: [],
        protocol_aliases: {},
        scripts: {
          request_scripts: [],
          response_scripts: [],
          decode_scripts: [],
          parser_scripts: [],
        },
        filter_specs: [],
      });
      return;
    }

    await fulfillJson(route, { success: true });
  });
}

async function mockActivityStatisticsWebSocket(page: Page) {
  await page.addInitScript(() => {
    const statisticsMessage = JSON.stringify({
      type: "traffic_statistics",
      data: {
        total_requests: 2501,
        server_sequence: 2502,
        client_ips: { "127.0.0.1": 2501 },
        proxy_ports: { "9900": 2501 },
        applications: { codex: 1751, "Microsoft Edge Helper": 750 },
        account_names: {},
        domains: { "example.test": 2501 },
      },
    });

    class ActivityMockWebSocket extends EventTarget {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;

      readonly CONNECTING = 0;
      readonly OPEN = 1;
      readonly CLOSING = 2;
      readonly CLOSED = 3;
      readyState = ActivityMockWebSocket.CONNECTING;
      bufferedAmount = 0;
      extensions = "";
      protocol = "";
      binaryType: BinaryType = "blob";
      url: string;
      onopen: ((event: Event) => void) | null = null;
      onmessage: ((event: MessageEvent) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      onclose: ((event: CloseEvent) => void) | null = null;

      constructor(url: string | URL) {
        super();
        this.url = String(url);
        window.setTimeout(() => {
          this.readyState = ActivityMockWebSocket.OPEN;
          this.onopen?.(new Event("open"));
          window.setTimeout(() => {
            this.onmessage?.(new MessageEvent("message", { data: statisticsMessage }));
          }, 1_000);
        }, 0);
      }

      send() {}

      close() {
        this.readyState = ActivityMockWebSocket.CLOSED;
        this.onclose?.(new CloseEvent("close", { code: 1000 }));
      }
    }

    window.WebSocket = ActivityMockWebSocket as unknown as typeof WebSocket;
  });
}

test("Metrics and status bar consume authoritative server fields", async ({
  page,
}) => {
  const metrics = {
    ...activityMetricsSnapshot(),
    timestamp: Date.now() + 1,
    memory_used: 50,
    memory_total: 100,
    memory_usage_percent: 50,
    total_requests: 4321,
    bytes_sent: 1024,
    bytes_received: 1024,
    total_traffic_bytes: 2048,
  };
  await mockActivityApi(page, { metrics, recorded: 5028 });
  await mockActivityStatisticsWebSocket(page);

  await page.goto("/_bifrost/settings?tab=metrics");
  await expect(page.getByText("Performance Metrics", { exact: true })).toBeVisible();
  await expect(page.getByText("5,028", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("4,321", { exact: true }).first()).toBeVisible();

  const statusBar = page.getByTestId("status-bar");
  await expect(statusBar).toContainText("Total:2.0 KB");
  await expect(statusBar).toContainText("Req:4321");
});

test("Activity tab is first, default, data-rich, and animated on hover", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await mockActivityApi(page);
  await mockActivityStatisticsWebSocket(page);

  await page.goto("/_bifrost/");
  await expect(page).toHaveURL(/\/_bifrost\/activity$/);
  await expect(page.getByTestId("activity-page")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Activity" })).toBeVisible();

  const firstNav = page.getByTestId("app-sidebar-nav-item").first();
  await expect(firstNav).toHaveAttribute("data-nav-label", "Activity");
  await expect(firstNav).toHaveAttribute("data-nav-key", "/activity");

  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "Active Connections" })).toContainText("18");
  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "Active Connections" })).toContainText("2 apps");
  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "Upload" })).toContainText("113 B/s");
  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "Requests" })).toContainText("2,501");
  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "System Proxy" })).toContainText("Disabled");
  await expect(page.getByTestId("activity-stat-card").filter({ hasText: "System Proxy" })).toContainText("http://127.0.0.1:9900");
  await expect(page.getByTestId("activity-rule-pill").filter({ hasText: "Default" })).toContainText("1 entries");
  const mergedRules = page.getByTestId("activity-merged-rules");
  await expect(mergedRules).toContainText("a.com status://200");
  await expect(mergedRules.locator('[data-effect-status="active"]')).toHaveCount(4);
  await expect(mergedRules.locator('[data-effect-status="partial"]')).toHaveCount(1);
  await expect(mergedRules.locator('[data-effect-status="shadowed"]')).toHaveCount(2);
  await expect(mergedRules.locator('[data-line-number="1"] > [data-line-gutter="true"]')).toHaveText("1");
  await expect(mergedRules.locator('[data-line-number="3"] > [data-line-gutter="true"]')).toHaveText("3");
  const wrapMetrics = await mergedRules.evaluate((element) => ({
    scrollWidth: element.scrollWidth,
    clientWidth: element.clientWidth,
  }));
  expect(wrapMetrics.scrollWidth).toBeLessThanOrEqual(wrapMetrics.clientWidth + 1);
  await mergedRules
    .locator('[data-effect-status="shadowed"]')
    .filter({ hasText: "ppe_old_" })
    .hover();
  await expect(page.getByText(/reqHeaders fields are replaced by line/)).toBeVisible();
  await mergedRules
    .locator('[data-effect-status="partial"]')
    .filter({ hasText: "x-stable" })
    .hover();
  await expect(page.getByText(/outside that narrower scope/)).toBeVisible();
  await expect(page.getByTestId("activity-temporary-ports-panel")).toContainText("Temporary Ports");
  await expect(page.getByTestId("activity-temporary-port-card-18888")).toContainText("127.0.0.1:18888");
  await expect(page.getByTestId("activity-temporary-port-card-18888")).toContainText("activity-temp-rule");
  await expect(page.getByTestId("activity-temporary-port-merged-18888")).toContainText("resBody://(activity-temp-rule)");
  await expect(page.getByTestId("activity-temporary-port-card-18889")).toContainText("127.0.0.1:18889");
  await expect(page.getByTestId("activity-temporary-port-card-18889")).toContainText("activity-temp-rule-two");
  const temporaryPortLayout = await page
    .locator('[data-testid^="activity-temporary-port-card-"]')
    .evaluateAll((cards) => cards.map((card) => {
      const rect = card.getBoundingClientRect();
      return { top: rect.top, left: rect.left, width: rect.width };
    }));
  expect(temporaryPortLayout).toHaveLength(2);
  expect(temporaryPortLayout[1].top).toBeGreaterThan(temporaryPortLayout[0].top + 8);
  expect(Math.abs(temporaryPortLayout[1].left - temporaryPortLayout[0].left)).toBeLessThan(2);
  const temporaryMergedMetrics = await page
    .getByTestId("activity-temporary-port-merged-18889")
    .evaluate((element) => ({
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
    }));
  expect(temporaryMergedMetrics.scrollHeight).toBeLessThanOrEqual(temporaryMergedMetrics.clientHeight + 1);
  expect(temporaryMergedMetrics.scrollWidth).toBeLessThanOrEqual(temporaryMergedMetrics.clientWidth + 1);
  await expect(page.getByTestId("activity-app-row").filter({ hasText: "codex" }).first()).toContainText("1,751");

  const mergedMetrics = await page.getByTestId("activity-merged-rules").evaluate((element) => {
    const parent = element.parentElement;
    return {
      codeHeight: element.getBoundingClientRect().height,
      parentHeight: parent?.getBoundingClientRect().height ?? 0,
    };
  });
  expect(mergedMetrics.codeHeight).toBeGreaterThan(250);
  expect(mergedMetrics.parentHeight - mergedMetrics.codeHeight).toBeLessThan(110);

  await page.evaluate(() => navigator.clipboard.writeText(""));
  await page.getByTestId("activity-merged-rules").evaluate((element) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let textNode: Node | null = null;
    while (walker.nextNode()) {
      if (walker.currentNode.textContent?.includes("a.com status://200")) {
        textNode = walker.currentNode;
        break;
      }
    }
    if (!textNode) return;
    const text = textNode.textContent || "";
    const start = text.indexOf("a.com");
    const range = document.createRange();
    range.setStart(textNode, start);
    range.setEnd(textNode, start + "a.com status://200".length);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
  });
  await page.getByTestId("activity-copy-merged-rules").click();
  await expect
    .poll(async () => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("a.com status://200");

  await page.evaluate(() => {
    window.getSelection()?.removeAllRanges();
    return navigator.clipboard.writeText("");
  });
  await page.getByTestId("activity-copy-merged-rules").click();
  await expect
    .poll(async () => page.evaluate(() => navigator.clipboard.readText()))
    .toContain("# Global default rules.");

  const firstCard = page.getByTestId("activity-stat-card").first();
  const cardTransformBefore = await firstCard.evaluate(
    (element) => window.getComputedStyle(element).transform,
  );
  await firstCard.hover();
  await page.waitForTimeout(220);
  const cardTransformAfter = await firstCard.evaluate(
    (element) => window.getComputedStyle(element).transform,
  );
  expect(cardTransformAfter).not.toBe(cardTransformBefore);

  const codexRow = page.getByTestId("activity-app-row").filter({ hasText: "codex" }).first();
  const fill = codexRow.locator('[class*="barFill"]');
  const fillFilterBefore = await fill.evaluate((element) => window.getComputedStyle(element).filter);
  await codexRow.hover();
  await page.waitForTimeout(220);
  const fillFilterAfter = await fill.evaluate((element) => window.getComputedStyle(element).filter);
  expect(fillFilterAfter).not.toBe(fillFilterBefore);

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByTestId("activity-page")).toBeVisible();
  await expect(page.getByTestId("activity-rules-panel")).toContainText("Active Rule Analysis");
  await expect(page.getByTestId("activity-distribution-panel")).toContainText("Traffic Distribution");
  const darkPanelBackground = await page
    .getByTestId("activity-rules-panel")
    .evaluate((element) => window.getComputedStyle(element).backgroundColor);
  expect(darkPanelBackground).not.toBe("rgba(255, 255, 255, 0.86)");

  await page.locator('[data-testid="activity-rule-pill"][data-rule-name="Default"]').click();
  await expect(page).toHaveURL(/\/_bifrost\/rules\?rule=Default$/);
});

test("Activity shows temporary port details from a running temporary listener", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("activity-temp-port-rule");
  let temporaryPort: number | null = null;

  try {
    await request.post(`${apiBase}/rules`, {
      data: {
        name: ruleName,
        content: `activity-temp-real.test status://219 resBody://(${ruleName})`,
        enabled: false,
      },
    });
    const bindRes = await request.post(`${apiBase}/ports`, {
      data: {
        port: 0,
        name: "Activity UI temporary port",
        rule_refs: [{ type: "local_rule", name: ruleName }],
      },
    });
    const binding = (await bindRes.json()) as { port: number };
    temporaryPort = binding.port;

    await openPage(page, "activity");
    const card = page.getByTestId(`activity-temporary-port-card-${temporaryPort}`);
    await expect(card).toBeVisible();
    await expect(card).toContainText(`127.0.0.1:${temporaryPort}`);
    await expect(card).toContainText("Activity UI temporary port");
    await expect(card).toContainText(ruleName);
    await expect(page.getByTestId(`activity-temporary-port-merged-${temporaryPort}`)).toContainText(
      `resBody://(${ruleName})`,
    );

    const transformBefore = await card.evaluate(
      (element) => window.getComputedStyle(element).transform,
    );
    await card.hover();
    await page.waitForTimeout(220);
    const transformAfter = await card.evaluate(
      (element) => window.getComputedStyle(element).transform,
    );
    expect(transformAfter).not.toBe(transformBefore);
  } finally {
    if (temporaryPort !== null) {
      await request.delete(`${apiBase}/ports/${temporaryPort}`);
    }
    await request.delete(`${apiBase}/rules/${encodeURIComponent(ruleName)}`);
  }
});
