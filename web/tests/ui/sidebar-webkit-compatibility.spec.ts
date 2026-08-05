import {
  expect,
  test,
  type Locator,
  type Page,
  type Route,
} from "@playwright/test";

const emptyMetrics = {
  timestamp: 0,
  memory_used: 0,
  memory_total: 0,
  memory_usage_percent: 0,
  cpu_usage: 0,
  total_requests: 0,
  active_connections: 0,
  bytes_sent: 0,
  bytes_received: 0,
  total_traffic_bytes: 0,
  bytes_sent_rate: 0,
  bytes_received_rate: 0,
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

async function mockSidebarApi(page: Page) {
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
          system_stats_items: {},
        },
        port: 9900,
        host: "127.0.0.1",
      });
      return;
    }
    if (apiPath === "/metrics") {
      await fulfillJson(route, emptyMetrics);
      return;
    }
    if (apiPath === "/system/overview") {
      await fulfillJson(route, {
        system: {
          version: "0.0.0-test",
          device_name: "sidebar-test",
          os: "macos",
          arch: "aarch64",
          cpu_logical_cores: 8,
          memory_total_bytes: 0,
          memory_available_bytes: 0,
          uptime_secs: 0,
          pid: 1,
        },
        metrics: emptyMetrics,
        rules: { total: 0, enabled: 0 },
        traffic: { recorded: 0 },
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
        enabled: false,
        shell: "",
        config_files: [],
        proxy_url: "127.0.0.1:9900",
      });
      return;
    }
    if (apiPath === "/traffic/updates") {
      await fulfillJson(route, {
        new_records: [],
        updated_records: [],
        has_more: false,
        server_total: 0,
        server_sequence: 0,
      });
      return;
    }
    if (apiPath === "/rules/active-summary") {
      await fulfillJson(route, {
        total: 0,
        rules: [],
        variable_conflicts: [],
        merged_content: "",
      });
      return;
    }
    if (apiPath === "/ports") {
      await fulfillJson(route, []);
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
        ios: { supported: false, devices: [], message: "mocked" },
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
        checked_at: "2026-07-14T00:00:00Z",
      });
      return;
    }

    await fulfillJson(route, { success: true });
  });
}

async function expectHorizontallyContained(locator: Locator, sidebar: Locator) {
  const [elementBox, sidebarBox] = await Promise.all([
    locator.boundingBox(),
    sidebar.boundingBox(),
  ]);
  expect(elementBox).not.toBeNull();
  expect(sidebarBox).not.toBeNull();
  if (!elementBox || !sidebarBox) return;

  const tolerance = 0.75;
  expect(elementBox.x).toBeGreaterThanOrEqual(sidebarBox.x - tolerance);
  expect(elementBox.x + elementBox.width).toBeLessThanOrEqual(
    sidebarBox.x + sidebarBox.width + tolerance,
  );
}

async function expectSidebarGeometry(page: Page) {
  const sidebar = page.getByTestId("desktop-sidebar-window-drag-region");
  const scroll = page.getByTestId("app-sidebar-nav-scroll");
  await expect(sidebar).toBeVisible();
  await expect(scroll).toBeVisible();

  const metrics = await scroll.evaluate((element) => {
    const style = window.getComputedStyle(element);
    const sidebar = element.parentElement;
    return {
      sidebarWidth: sidebar?.getBoundingClientRect().width ?? 0,
      clientWidth: element.clientWidth,
      rectWidth: element.getBoundingClientRect().width,
      overflowX: style.overflowX,
      overflowY: style.overflowY,
      scrollbarGutter: style.scrollbarGutter,
    };
  });
  expect(metrics.sidebarWidth).toBe(50);
  expect(metrics.rectWidth).toBe(49);
  expect(metrics.clientWidth).toBe(metrics.rectWidth);
  expect(metrics.overflowX).toBe("hidden");
  expect(metrics.overflowY).toBe("auto");
  expect(metrics.scrollbarGutter).toBe("auto");

  const items = page.getByTestId("app-sidebar-nav-item");
  const count = await items.count();
  expect(count).toBeGreaterThan(0);
  for (let index = 0; index < count; index += 1) {
    const item = items.nth(index);
    await expectHorizontallyContained(item, sidebar);
    await expectHorizontallyContained(item.getByTestId("app-sidebar-nav-icon"), sidebar);
    await expectHorizontallyContained(item.getByTestId("app-sidebar-nav-label"), sidebar);
  }
  await expectHorizontallyContained(page.getByTestId("app-sidebar-openapi"), sidebar);
  await expectHorizontallyContained(page.getByTestId("theme-toggle"), sidebar);
}

test("WebKit 与 Chromium 中侧栏内容完整且小窗口可滚动", async ({
  page,
}, testInfo) => {
  await mockSidebarApi(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/_bifrost/activity");
  await expect(page.getByTestId("activity-page")).toBeVisible();

  await expectSidebarGeometry(page);
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expectSidebarGeometry(page);

  await page.setViewportSize({ width: 900, height: 360 });
  const scroll = page.getByTestId("app-sidebar-nav-scroll");
  const overflowMetrics = await scroll.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }));
  expect(overflowMetrics.scrollHeight).toBeGreaterThan(overflowMetrics.clientHeight);
  await scroll.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });

  const settings = page.locator(
    '[data-testid="app-sidebar-nav-item"][data-nav-label="Settings"]',
  );
  await expect(settings).toBeVisible();
  await expectSidebarGeometry(page);
  await settings.click();
  await expect(page).toHaveURL(/\/_bifrost\/settings$/);

  const screenshot = await page.screenshot({ fullPage: false });
  await testInfo.attach(`sidebar-${testInfo.project.name || "default"}`, {
    body: screenshot,
    contentType: "image/png",
  });
});
