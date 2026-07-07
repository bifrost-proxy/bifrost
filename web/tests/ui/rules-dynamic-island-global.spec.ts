import { expect, test, type Page, type Route } from "@playwright/test";

const activeRuleName = "global-active-rule";
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

async function mockAdminApi(page: Page) {
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
        android: {
          adb_available: false,
          devices: [],
          message: "mocked",
        },
        ios: {
          supported: false,
          devices: [],
          configurator: {
            supported: false,
            cfgutil_available: false,
            message: "mocked",
          },
          message: "mocked",
        },
        ios_profile_url: "",
        ios_profile_qrcode_url: "",
      });
      return;
    }
    if (apiPath === "/rules/active-summary") {
      await fulfillJson(route, {
        total: 2,
        rules: [
          {
            name: activeRuleName,
            rule_count: 1,
            group_id: null,
            group_name: null,
          },
          {
            name: "team-active-rule",
            rule_count: 2,
            group_id: "team-1",
            group_name: "Team One",
          },
        ],
        variable_conflicts: [],
        merged_content: [
          `https://nextoncall.bytedance.net/api/v1/oncall/ reqHeaders://{"x-tt-env":"${longHeaderValue}","x-use-ppe":"1"}`,
          "https://nextoncall.bytedance.net/api/v1/oncall/ passthrough://",
          'https://nextoncall.bytedance.net/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_new","x-use-ppe":"1"}',
          "https://nextoncall.bytedance.net/api/v1/oncall/ passthrough://",
          'https://partial.example.test/api/internal/ reqHeaders://{"x-env":"narrow"}',
          'https://partial.example.test/api/ reqHeaders://{"x-env":"broad","x-stable":"keep"}',
        ].join("\n"),
      });
      return;
    }
    if (apiPath === "/rules") {
      await fulfillJson(route, [
        {
          name: activeRuleName,
          enabled: true,
          sort_order: 0,
          rule_count: 1,
          created_at: "2026-07-02T00:00:00Z",
          updated_at: "2026-07-02T00:00:00Z",
        },
      ]);
      return;
    }
    if (apiPath === `/rules/${activeRuleName}`) {
      await fulfillJson(route, {
        name: activeRuleName,
        content: "example.test statusCode://204",
        enabled: true,
        sort_order: 0,
        created_at: "2026-07-02T00:00:00Z",
        updated_at: "2026-07-02T00:00:00Z",
        sync: { status: "local_only" },
      });
      return;
    }
    if (apiPath === "/rules/reference-candidates") {
      await fulfillJson(route, []);
      return;
    }
    if (apiPath === "/scripts") {
      await fulfillJson(route, {
        request: [],
        response: [],
        decode: [],
        parser: [],
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
        checked_at: "2026-07-02T00:00:00Z",
      });
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
    if (apiPath === "/traffic" || apiPath === "/traffic/query") {
      await fulfillJson(route, {
        records: [],
        items: [],
        total: 0,
        has_more: false,
      });
      return;
    }

    await fulfillJson(route, { success: true });
  });
}

test("Rules 状态胶囊在全局页面可见、可拖拽，并能跳转到 Rules 详情", async ({
  page,
}) => {
  const runtimeErrors: string[] = [];
  page.on("pageerror", (error) => {
    runtimeErrors.push(`pageerror: ${error.stack || error.message}`);
  });
  page.on("console", (message) => {
    if (message.type() === "error") {
      runtimeErrors.push(`console: ${message.text()}`);
    }
  });
  await mockAdminApi(page);
  await page.addInitScript(() => {
    window.localStorage.removeItem("bifrost.rulesDynamicIsland.position");
  });

  await page.goto("/_bifrost/traffic");

  const trigger = page.getByTestId("rules-dynamic-island-trigger");
  await trigger.waitFor({ state: "visible", timeout: 10000 }).catch(async () => {
    throw new Error(
      [
        "Rules status capsule did not render.",
        `URL: ${page.url()}`,
        `Body: ${(await page.locator("body").innerText().catch(() => "")).slice(0, 500)}`,
        `Runtime errors: ${runtimeErrors.join("\n") || "none"}`,
      ].join("\n"),
    );
  });
  await expect(trigger).toContainText("2 active");

  const beforeDrag = await trigger.boundingBox();
  if (!beforeDrag) {
    throw new Error("Rules status capsule is not visible before drag");
  }

  await page.mouse.move(
    beforeDrag.x + beforeDrag.width / 2,
    beforeDrag.y + beforeDrag.height / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    beforeDrag.x + beforeDrag.width / 2 + 120,
    beforeDrag.y + beforeDrag.height / 2 + 80,
    { steps: 5 },
  );
  await page.mouse.up();

  const afterDrag = await trigger.boundingBox();
  if (!afterDrag) {
    throw new Error("Rules status capsule is not visible after drag");
  }
  expect(Math.abs(afterDrag.x - beforeDrag.x)).toBeGreaterThan(40);
  expect(Math.abs(afterDrag.y - beforeDrag.y)).toBeGreaterThan(30);

  await page.reload();
  await trigger.waitFor({ state: "visible", timeout: 10000 });
  const afterReload = await trigger.boundingBox();
  if (!afterReload) {
    throw new Error("Rules status capsule is not visible after reload");
  }
  expect(Math.abs(afterReload.x - beforeDrag.x)).toBeLessThan(8);
  expect(Math.abs(afterReload.y - beforeDrag.y)).toBeLessThan(8);

  await trigger.click();
  await expect(page.getByTestId("rules-dynamic-island-panel")).toBeVisible();
  await page.getByTestId("rules-dynamic-island-merged-toggle").click();
  const mergedPanel = page.getByTestId("rules-dynamic-island-merged-content");
  await expect(mergedPanel).toBeVisible();
  await expect(mergedPanel.locator('[data-effect-status="active"]')).toHaveCount(3);
  await expect(mergedPanel.locator('[data-effect-status="partial"]')).toHaveCount(1);
  await expect(mergedPanel.locator('[data-effect-status="shadowed"]')).toHaveCount(2);
  await expect(mergedPanel.locator('[data-line-number="1"] > [data-line-gutter="true"]')).toHaveText("1");
  await expect(mergedPanel.locator('[data-line-number="4"] > [data-line-gutter="true"]')).toHaveText("4");
  const wrapMetrics = await mergedPanel.evaluate((element) => ({
    scrollWidth: element.scrollWidth,
    clientWidth: element.clientWidth,
  }));
  expect(wrapMetrics.scrollWidth).toBeLessThanOrEqual(wrapMetrics.clientWidth + 1);
  const coveredReqHeaders = mergedPanel
    .locator('[data-effect-status="shadowed"]')
    .filter({ hasText: "ppe_old_" });
  await coveredReqHeaders.hover();
  await expect(page.getByText(/reqHeaders fields are replaced by line/)).toBeVisible();
  await mergedPanel
    .locator('[data-effect-status="partial"]')
    .filter({ hasText: "x-stable" })
    .hover();
  await expect(page.getByText(/outside that narrower scope/)).toBeVisible();
  const tooltipBox = await page.locator(".ant-tooltip").last().boundingBox();
  expect(tooltipBox?.width ?? 0).toBeGreaterThan(420);

  const activeRuleRow = page
    .getByTestId("rules-dynamic-island-rule-row")
    .filter({ hasText: activeRuleName });
  await expect(activeRuleRow).toContainText(activeRuleName);

  await activeRuleRow.click();

  await expect(page).toHaveURL(new RegExp(`/_bifrost/rules\\?rule=${activeRuleName}`));
  await expect(page.getByTestId("rule-editor")).toBeVisible();
  await expect(page.getByTestId("rule-editor")).toContainText(activeRuleName);
});
