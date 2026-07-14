import { test, expect, type Route } from "@playwright/test";
import {
  apiBase,
  backendPort,
  openPage,
  resetAccessControl,
  startMockSyncServer,
  setSelectValue,
  waitForToast,
  uniqueName,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ request }) => {
  await resetAccessControl(request);
});

test("左侧一级导航在小窗口下可滚动且 Settings 仍可达", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 360 });
  await openPage(page, "traffic");

  const navScroll = page.getByTestId("app-sidebar-nav-scroll");
  await expect(navScroll).toBeVisible();
  await expect(page.getByTestId("theme-toggle")).toBeVisible();

  const metrics = await navScroll.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    overflowY: window.getComputedStyle(element).overflowY,
  }));
  expect(metrics.scrollHeight).toBeGreaterThan(metrics.clientHeight);
  expect(metrics.overflowY).toBe("auto");

  const firstItemMinHeight = await page
    .getByTestId("app-sidebar-nav-item")
    .first()
    .evaluate((element) => window.getComputedStyle(element).minHeight);
  expect(firstItemMinHeight).toBe("64px");

  await navScroll.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await page.locator('[data-testid="app-sidebar-nav-item"][data-nav-label="Settings"]').click();
  await expect(page).toHaveURL(/\/_bifrost\/settings/);
});

test("版本更新弹窗 Upgrade Command Copy 写入剪贴板", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.route("**/_bifrost/api/system/version-check**", async (route) => {
    await route.fulfill({
      json: {
        has_update: true,
        current_version: "0.0.62",
        latest_version: "0.0.73",
        release_highlights: ["fix: copy upgrade command regression"],
        release_url: "https://github.com/bifrost-proxy/bifrost/releases/tag/v0.0.73",
      },
    });
  });

  await openPage(page, "traffic");
  await page.evaluate(async () => navigator.clipboard.writeText(""));
  await page.getByTestId("statusbar-version-button").click();
  await expect(page.getByText("New Version Available")).toBeVisible();

  await page.getByTestId("version-upgrade-copy-button").click();
  await waitForToast(page, "Command copied to clipboard");

  const clipboardText = await page.evaluate(async () => navigator.clipboard.readText());
  expect(clipboardText).toBe("bifrost upgrade");
});

test("底部 Sync 状态栏点击后跳转到 Settings Sync", async ({ page }) => {
  await openPage(page, "traffic");

  await page.getByTestId("statusbar-sync").click();

  await expect(page).toHaveURL(/\/_bifrost\/settings\?tab=sync/);
  await expect(page.getByRole("tab", { name: /Sync/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  await openPage(page, "traffic");
  await page.getByTestId("statusbar-sync").focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/_bifrost\/settings\?tab=sync/);

  await openPage(page, "traffic");
  await page.getByTestId("statusbar-sync").focus();
  await page.keyboard.press("Space");
  await expect(page).toHaveURL(/\/_bifrost\/settings\?tab=sync/);
});

test("底部状态栏展示全局 HTTPS Interception 动画警示", async ({
  page,
  request,
}) => {
  const tlsRes = await request.get(`${apiBase}/config/tls`);
  const originalTls = await tlsRes.json();
  await request.put(`${apiBase}/config/tls`, {
    data: {
      ...originalTls,
      enable_tls_interception: true,
      intercept_exclude: [],
      intercept_include: [],
      app_intercept_exclude: [],
      app_intercept_include: [],
      ip_intercept_exclude: [],
      ip_intercept_include: [],
    },
  });
  await page.route("**/_bifrost/api/proxy/system", async (route) => {
    await route.fulfill({
      json: {
        supported: true,
        enabled: true,
        host: "127.0.0.1",
        port: backendPort,
        bypass: "localhost,127.0.0.1",
        managed_by_bifrost: true,
        configured_enabled: true,
      },
    });
  });

  try {
    await openPage(page, "traffic");

    const tlsStatus = page.getByTestId("statusbar-tls-interception");
    const expectTlsStatusReadable = async () => {
      const metrics = await tlsStatus.evaluate((element) => {
        const value = element.querySelector(".statusbar-tls-value--active");
        const valueStyle = value ? window.getComputedStyle(value) : null;
        const rect = element.getBoundingClientRect();
        return {
          color: valueStyle?.color ?? "",
          background: window.getComputedStyle(document.body).backgroundColor,
          width: rect.width,
          height: rect.height,
        };
      });
      expect(metrics.color).not.toBe(metrics.background);
      expect(metrics.width).toBeGreaterThan(70);
      expect(metrics.height).toBeGreaterThan(12);
    };

    await expect(tlsStatus).toBeVisible();
    await expect(tlsStatus).toHaveAttribute("data-tls-state", "full");
    await expect(tlsStatus).toContainText("TLS:");
    await expect(tlsStatus).toContainText("Full On");
    await expect(tlsStatus.locator(".statusbar-tls-dot--active")).toBeVisible();
    await expect(tlsStatus.locator(".statusbar-tls-value--active")).toHaveText("Full On");
    await expectTlsStatusReadable();

    await page.getByTestId("theme-toggle").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect(tlsStatus).toHaveAttribute("data-tls-state", "full");
    await expect(tlsStatus.locator(".statusbar-tls-dot--active")).toBeVisible();
    await expect(tlsStatus.locator(".statusbar-tls-value--active")).toHaveText("Full On");
    await expectTlsStatusReadable();

    await tlsStatus.click();
    await expect(page).toHaveURL(/\/_bifrost\/settings\?tab=tls/);
  } finally {
    await request.put(`${apiBase}/config/tls`, { data: originalTls });
  }
});

test("Settings 访问控制支持模式切换、白名单、临时白名单和 LAN 开关", async ({
  page,
}) => {
  await openPage(page, "settings");
  await page.getByRole("tab", { name: /Access Control/ }).click();
  await expect(page.getByRole("tab", { name: /Access Control/, exact: false })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.locator("body")).toContainText("Access Settings");

  await setSelectValue(page, page.getByTestId("settings-access-mode-select"), "Whitelist");
  await expect(page.locator("body")).toContainText("Only allow whitelisted IPs/CIDRs");

  await page.getByTestId("settings-access-allow-lan").click();

  await page.getByTestId("settings-whitelist-input").fill("10.0.0.1");
  await page.getByTestId("settings-whitelist-add-button").click();
  await waitForToast(page, "Added 10.0.0.1 to whitelist");
  await expect(page.getByTestId("settings-whitelist-table")).toContainText("10.0.0.1");

  await page.getByTestId("settings-temp-whitelist-input").fill("10.0.0.2");
  await page.getByTestId("settings-temp-whitelist-add-button").click();
  await waitForToast(page, "Added 10.0.0.2 to temporary whitelist");
  await expect(page.getByTestId("settings-temp-whitelist-table")).toContainText("10.0.0.2");
});

test("Settings 访问控制重新挂载和保存后保留 CLI 账号与认证开关", async ({
  page,
  request,
}) => {
  const username = uniqueName("ui-persist-account");
  const saved = await request.put(`${apiBase}/whitelist/userpass`, {
    data: {
      enabled: true,
      accounts: [
        {
          username,
          password: "ui-persist-secret",
          enabled: true,
        },
      ],
      loopback_requires_auth: true,
    },
  });
  expect(saved.ok()).toBe(true);

  await openPage(page, "settings?tab=access");
  await expect(page.getByRole("tab", { name: /Access Control/, exact: false })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(
    page.getByTestId(`settings-access-userpass-username-${username}`),
  ).toHaveValue(username);
  await expect(page.getByTestId("settings-access-userpass-enabled")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(page.getByTestId("settings-access-loopback-requires-auth")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByTestId(`settings-access-userpass-username-${username}`),
  ).toHaveValue(username);

  await page.locator('[data-testid="app-sidebar-nav-item"][data-nav-label="Activity"]').click();
  await page.locator('[data-testid="app-sidebar-nav-item"][data-nav-label="Settings"]').click();
  await page.getByRole("tab", { name: /Access Control/ }).click();

  await expect(
    page.getByTestId(`settings-access-userpass-username-${username}`),
  ).toHaveValue(username);
  await expect(page.getByTestId("settings-access-userpass-enabled")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(page.getByTestId("settings-access-loopback-requires-auth")).toHaveAttribute(
    "aria-checked",
    "true",
  );

  await page.getByTestId("settings-access-userpass-enabled").click();
  await expect(page.getByTestId("settings-access-userpass-enabled")).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await page.getByTestId("settings-access-userpass-save-button").click();
  await waitForToast(page, "Updated user/password proxy authentication");
  await expect
    .poll(async () => {
      const response = await request.get(`${apiBase}/whitelist`);
      const status = (await response.json()) as {
        userpass: {
          enabled: boolean;
          loopback_requires_auth: boolean;
          accounts: Array<{ username: string; has_password: boolean }>;
        };
      };
      return {
        enabled: status.userpass.enabled,
        loopbackRequiresAuth: status.userpass.loopback_requires_auth,
        accounts: status.userpass.accounts.map((account) => ({
          username: account.username,
          hasPassword: account.has_password,
        })),
      };
    })
    .toEqual({
      enabled: false,
      loopbackRequiresAuth: true,
      accounts: [{ username, hasPassword: true }],
    });

  await page.reload();
  await expect(page).toHaveURL(/settings\?tab=access/);
  await expect(
    page.getByTestId(`settings-access-userpass-username-${username}`),
  ).toHaveValue(username);
  await expect(page.getByTestId("settings-access-userpass-enabled")).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await expect(page.getByTestId("settings-access-loopback-requires-auth")).toHaveAttribute(
    "aria-checked",
    "true",
  );

  await page.getByTestId("settings-access-userpass-enabled").click();
  await page.getByTestId("settings-access-userpass-save-button").click();
  await waitForToast(page, "Updated user/password proxy authentication");
  await expect
    .poll(async () => {
      const response = await request.get(`${apiBase}/whitelist`);
      const status = (await response.json()) as {
        userpass: {
          enabled: boolean;
          accounts: Array<{ username: string; has_password: boolean }>;
        };
      };
      return {
        enabled: status.userpass.enabled,
        accounts: status.userpass.accounts.map((account) => ({
          username: account.username,
          hasPassword: account.has_password,
        })),
      };
    })
    .toEqual({
      enabled: true,
      accounts: [{ username, hasPassword: true }],
    });
});

test("Settings 性能配置在第二个页面主动刷新后可见", async ({
  page,
  context,
  request,
}) => {
  const perfRes = await request.get(`${apiBase}/config/performance`);
  const perf = (await perfRes.json()) as { traffic: { max_records: number } };
  const original = perf.traffic.max_records;

  try {
    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Performance/ }).click();
    await expect(page.locator("body")).toContainText("Max Records");

    const page2 = await context.newPage();
    await openPage(page2, "settings");
    await page2.getByRole("tab", { name: /Performance/ }).click();
    await expect(page2.locator("body")).toContainText("Max Records");

    const handle = page.locator(".ant-slider-handle").first();
    await expect(handle).toBeVisible();
    await handle.focus();
    await page.keyboard.press("ArrowRight");
    await waitForToast(page, "Max records updated");
    await expect
      .poll(async () => {
        const res = await request.get(`${apiBase}/config/performance`);
        const body = (await res.json()) as { traffic: { max_records: number } };
        return body.traffic.max_records;
      })
      .not.toBe(original);

    const refreshedRes = await request.get(`${apiBase}/config/performance`);
    const refreshed = (await refreshedRes.json()) as { traffic: { max_records: number } };
    await page2.reload();
    await page2.getByRole("tab", { name: /Performance/ }).click();
    await expect(page2.locator("body")).toContainText(
      refreshed.traffic.max_records.toLocaleString(),
    );
    await page2.close();
  } finally {
    await request.put(`${apiBase}/config/performance`, {
      data: { max_records: original },
    });
  }
});

test("Network 超级性能模式覆盖整个工作区并可跳转高亮 Performance 开关", async ({
  page,
  request,
}) => {
  const perfRes = await request.get(`${apiBase}/config/performance`);
  const perf = (await perfRes.json()) as {
    traffic: { super_performance_mode: boolean };
  };
  const original = perf.traffic.super_performance_mode;

  try {
    await request.put(`${apiBase}/config/performance`, {
      data: { super_performance_mode: true },
    });

    let releasePerformanceRequest!: () => void;
    const performanceRequestGate = new Promise<void>((resolve) => {
      releasePerformanceRequest = resolve;
    });
    await page.route("**/_bifrost/api/config/performance", async (route) => {
      await performanceRequestGate;
      await route.continue();
    });

    const navigation = openPage(page, "traffic");
    const loading = page.getByTestId("traffic-performance-loading");
    const overlay = page.getByTestId("traffic-super-performance-overlay");
    await expect(loading).toBeVisible();
    await expect(loading).toContainText("Loading Network...");
    await expect(overlay).toHaveCount(0);
    releasePerformanceRequest();
    await navigation;
    await expect(loading).toHaveCount(0);
    const trafficPage = page.getByTestId("traffic-page");
    const detailPane = page.getByTestId("traffic-detail-pane");
    const trafficTable = page.getByTestId("traffic-table");
    const filterSearch = page.getByPlaceholder("Search filters...");
    const toolbarControl = page.getByTestId("toolbar-clear-all");
    const globalMenuControl = page.getByTestId("theme-toggle");
    await expect(overlay).toBeVisible();
    await expect(overlay).toContainText("Super performance mode is enabled");
    await expect(overlay.locator(".ant-alert")).toHaveCount(0);

    const [overlayBox, pageBox, detailBox, tableBox, filterBox, toolbarBox, menuBox] =
      await Promise.all([
      overlay.boundingBox(),
      trafficPage.boundingBox(),
      detailPane.boundingBox(),
      trafficTable.boundingBox(),
      filterSearch.boundingBox(),
      toolbarControl.boundingBox(),
      globalMenuControl.boundingBox(),
    ]);
    expect(overlayBox).not.toBeNull();
    expect(pageBox).not.toBeNull();
    expect(detailBox).not.toBeNull();
    expect(tableBox).not.toBeNull();
    expect(filterBox).not.toBeNull();
    expect(toolbarBox).not.toBeNull();
    expect(menuBox).not.toBeNull();
    expect(Math.abs(overlayBox!.x - pageBox!.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(overlayBox!.y - pageBox!.y)).toBeLessThanOrEqual(1);
    expect(Math.abs(overlayBox!.width - pageBox!.width)).toBeLessThanOrEqual(1);
    expect(Math.abs(overlayBox!.height - pageBox!.height)).toBeLessThanOrEqual(1);

    for (const coveredBox of [toolbarBox!, filterBox!, tableBox!, detailBox!]) {
      expect(coveredBox.x).toBeGreaterThanOrEqual(overlayBox!.x);
      expect(coveredBox.y).toBeGreaterThanOrEqual(overlayBox!.y);
      expect(coveredBox.x + coveredBox.width).toBeLessThanOrEqual(
        overlayBox!.x + overlayBox!.width,
      );
      expect(coveredBox.y + coveredBox.height).toBeLessThanOrEqual(
        overlayBox!.y + overlayBox!.height,
      );
    }
    expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(overlayBox!.x);

    const lightColors = await overlay.evaluate((element) => {
      const style = window.getComputedStyle(element);
      return { background: style.backgroundColor, color: style.color };
    });
    expect(lightColors.background).not.toBe("rgb(255, 251, 230)");

    await page.getByTestId("theme-toggle").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    const darkColors = await overlay.evaluate((element) => {
      const style = window.getComputedStyle(element);
      return { background: style.backgroundColor, color: style.color };
    });
    expect(darkColors.background).not.toBe(lightColors.background);

    await page.getByTestId("traffic-super-performance-disable-button").click();
    await expect(page).toHaveURL(
      /\/_bifrost\/settings\?tab=performance&highlight=super-performance-mode/,
    );
    await expect(page.getByTestId("settings-performance-tab")).toContainText(
      "Super Performance Mode",
    );
    await expect(page.getByTestId("settings-performance-super-mode")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect
      .poll(async () =>
        page
          .getByTestId("settings-performance-super-mode-highlight")
          .evaluate((element) => window.getComputedStyle(element).boxShadow),
      )
      .not.toBe("none");
  } finally {
    await request.put(`${apiBase}/config/performance`, {
      data: { super_performance_mode: original },
    });
  }
});

test("Settings TLS 与证书页支持开关、模式和只读展示", async ({
  page,
  request,
}) => {
  const tlsRes = await request.get(`${apiBase}/config/tls`);
  const originalTls = await tlsRes.json();

  try {
    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Proxy/ }).click();
    await expect(page.locator("body")).toContainText("HTTPS Interception");

    await page.getByTestId("settings-tls-enable-switch").click();
    await expect
      .poll(async () => {
        const res = await request.get(`${apiBase}/config/tls`);
        const body = await res.json();
        return body.enable_tls_interception;
      })
      .toBe(true);

    await page.getByTestId("settings-tls-include-input").fill("*.ui-e2e.local");
    await page.getByTestId("settings-tls-include-add-button").click();
    await waitForToast(
      page,
      "Restart the target app and reopen the target domain to establish a new connection.",
    );
    await expect(page.locator("body")).toContainText("*.ui-e2e.local");

    await page.getByTestId("settings-tls-exclude-input").fill("*.ui-e2e-skip.local");
    await page.getByTestId("settings-tls-exclude-add-button").click();
    await expect(page.locator("body")).toContainText("*.ui-e2e-skip.local");

    await page.getByRole("tab", { name: /Certificate/ }).click();
    await expect(page.locator("body")).toContainText("Certificate Status");
    await expect(page.getByTestId("settings-certificate-download")).toBeVisible();
    await expect(page.getByTestId("settings-certificate-qrcode")).toBeVisible();
  } finally {
    await request.put(`${apiBase}/config/tls`, { data: originalTls });
  }
});

test("Certificate iOS Configurator 缺失时在禁用按钮旁显示安装入口", async ({
  page,
}) => {
  const mobileDevicesPayload = {
    android: {
      adb_available: false,
      adb_path: null,
      devices: [],
      message: "Android ADB is not available.",
    },
    ios: {
      supported: true,
      devices: [
        {
          id: "00008112000839C63621401E",
          name: "iPad",
          managed_install_target: null,
          platform: "ios",
          status: "connected",
          capability: "guide_only",
          certificate_status: null,
          status_message:
            "Detected over USB. Download the iOS profile, install it on the phone, then enable full trust in Certificate Trust Settings.",
        },
      ],
      configurator: {
        supported: true,
        cfgutil_available: false,
        cfgutil_path: null,
        message:
          "Apple Configurator cfgutil was not found. Install Apple Configurator from the Mac App Store to enable computer-side iPhone profile installation.",
      },
      message: "Detected 1 iOS USB device(s).",
    },
    ios_profile_url: "/_bifrost/public/mobileconfig/ios",
    ios_profile_qrcode_url: "/_bifrost/public/mobileconfig/ios/qrcode",
    ordinary_device_notice: "Ordinary devices require manual confirmation.",
    managed_device_notice:
      "Managed devices can support automatic trust through Configurator or MDM.",
  };

  await page.routeWebSocket(/\/api\/push/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage((message) => {
      server.send(message);
    });
    server.onMessage((message) => {
      try {
        const parsed = JSON.parse(String(message));
        if (parsed?.type === "settings_update" && parsed?.data?.scope === "mobile_devices") {
          ws.send(
            JSON.stringify({
              ...parsed,
              data: {
                ...parsed.data,
                data: mobileDevicesPayload,
              },
            }),
          );
          return;
        }
      } catch {
        // Non-JSON push messages should pass through unchanged.
      }
      ws.send(message);
    });
  });
  await page.route("**/_bifrost/api/cert/info", async (route) => {
    await route.fulfill({
      json: {
        available: true,
        status: "installed_and_trusted",
        status_label: "Installed and trusted",
        installed: true,
        trusted: true,
        status_message: "Bifrost CA is installed and trusted.",
        sha256_fingerprint: "00",
        local_ips: ["127.0.0.1"],
        download_urls: ["http://127.0.0.1:9910/_bifrost/public/cert/ca.pem"],
        qrcode_urls: ["http://127.0.0.1:9910/_bifrost/public/cert/qrcode"],
      },
    });
  });
  const fulfillMobileDevices = async (route: Route) => {
    await route.fulfill({
      json: mobileDevicesPayload,
    });
  };
  await page.route(
    /\/_bifrost\/api\/mobile-devices(?:\/refresh)?(?:\?.*)?$/,
    fulfillMobileDevices,
  );

  await openPage(page, "settings?tab=certificate");

  await expect(page.getByTestId("settings-certificate-tab")).toBeVisible();
  await expect(page.locator("body")).toContainText("Detected 1 iOS USB device(s).");
  await expect(page.getByTestId("settings-mobile-install-ios-configurator")).toBeDisabled();
  await expect(page.getByTestId("settings-mobile-install-ios-proxy-config")).toHaveCount(0);
  await expect(page.getByTestId("settings-mobile-ios-configurator-missing")).toContainText(
    "The Configurator button stays disabled",
  );
  await expect(
    page.getByTestId("settings-mobile-ios-configurator-disabled-reason"),
  ).toContainText("Configurator install is disabled because cfgutil is not installed");

  const appStoreHref = await page
    .getByTestId("settings-mobile-ios-configurator-app-store")
    .getAttribute("href");
  expect(appStoreHref).toContain("macappstore://");
  expect(appStoreHref).toContain("id1037126344");
});

test("Settings 代理与证书卡片会反映 system proxy、cli proxy、下载与二维码真实状态", async ({
  page,
  request,
}) => {
  const systemProxyRes = await request.get(`${apiBase}/proxy/system`);
  const systemProxy = (await systemProxyRes.json()) as {
    supported: boolean;
    enabled: boolean;
    managed_by_bifrost?: boolean;
  };
  const cliProxyRes = await request.get(`${apiBase}/proxy/cli`);
  const cliProxy = (await cliProxyRes.json()) as {
    enabled: boolean;
    shell: string;
    config_files: string[];
  };
  const certRes = await request.get(`${apiBase}/cert`);
  const certInfo = (await certRes.json()) as {
    available?: boolean;
  };
  const proxyAddressRes = await request.get(`${apiBase}/proxy/address`);
  const proxyAddressInfo = (await proxyAddressRes.json()) as {
    addresses?: Array<{ ip: string }>;
  };

  await openPage(page, "settings");
  await page.getByRole("tab", { name: /Proxy/ }).click();
  await expect(page.locator("body")).toContainText("System Proxy");

  if (systemProxy.supported) {
    await expect(page.getByTestId("settings-system-proxy-switch")).toBeVisible();
    await expect(page.getByTestId("settings-system-proxy-switch")).toHaveAttribute(
      "aria-checked",
      String(systemProxy.enabled && systemProxy.managed_by_bifrost !== false),
    );
  } else {
    await expect(page.locator("body")).toContainText("Not Supported");
  }

  await expect(page.getByTestId("settings-cli-proxy-tag")).toHaveText(
    cliProxy.enabled ? "Enabled" : "Disabled",
  );
  await expect(page.getByTestId("settings-cli-proxy-detail")).toContainText(
    `Shell: ${cliProxy.shell || "-"}`,
  );

  const proxyQrSrc = await page
    .getByTestId("settings-proxy-qrcode")
    .locator("img")
    .first()
    .getAttribute("src");
  expect(proxyQrSrc).toContain("/_bifrost/public/proxy/qrcode");
  if (proxyAddressInfo.addresses && proxyAddressInfo.addresses.length > 0) {
    expect(proxyQrSrc).toContain(encodeURIComponent(proxyAddressInfo.addresses[0].ip));
  }

  await page.getByRole("tab", { name: /Certificate/ }).click();
  await expect(page.getByTestId("settings-certificate-tab")).toBeVisible();

  const downloadButton = page.getByTestId("settings-certificate-download");
  await expect(downloadButton).toBeVisible();

  if (certInfo.available) {
    const href = await page
      .getByRole("link", { name: /Download CA Certificate/ })
      .getAttribute("href");
    if (!href) {
      throw new Error("Expected certificate download href to be present");
    }
    const downloadResponse = await request.get(new URL(href, page.url()).toString());
    expect(downloadResponse.ok()).toBeTruthy();
    const certQrSrc = await page
      .getByTestId("settings-certificate-qrcode")
      .locator("img")
      .first()
      .getAttribute("src");
    expect(certQrSrc).toContain("/_bifrost/public/cert/qrcode");
  } else {
    await expect(downloadButton).toHaveClass(/ant-btn-disabled/);
    await expect(page.locator("body")).toContainText("QR code not available");
  }
});

test("Settings Tray tab 支持独立开关系统状态两排展示", async ({ page }) => {
  let trayConfig = {
    enabled: true,
    supported: true,
    system_stats_supported: true,
    show_system_stats: true,
    system_stats_items: {
      cpu: true,
      memory: true,
      disk: true,
      upload: true,
      download: true,
    },
  };
  const updatePayloads: unknown[] = [];

  await page.route("**/_bifrost/api/config", async (route) => {
    await route.fulfill({
      json: {
        host: "127.0.0.1",
        port: backendPort,
        tls: {
          enable_tls_interception: false,
          intercept_exclude: [],
          intercept_include: [],
          app_intercept_exclude: [],
          app_intercept_include: [],
          ip_intercept_exclude: [],
          ip_intercept_include: [],
          unsafe_ssl: true,
          disconnect_on_config_change: false,
        },
        tray: trayConfig,
      },
    });
  });
  await page.route("**/_bifrost/api/config/tray", async (route) => {
    if (route.request().method() === "PUT") {
      const payload = route.request().postDataJSON() as {
        enabled?: boolean;
        show_system_stats?: boolean;
        system_stats_items?: Partial<typeof trayConfig.system_stats_items>;
      };
      updatePayloads.push(payload);
      trayConfig = {
        ...trayConfig,
        ...payload,
        system_stats_items: {
          ...trayConfig.system_stats_items,
          ...payload.system_stats_items,
        },
      };
    }
    await route.fulfill({ json: trayConfig });
  });

  await openPage(page, "settings?tab=tray");
  await expect(page.getByRole("tab", { name: /Tray/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByTestId("settings-tray-tab")).toContainText("System Status");
  await expect(page.getByTestId("settings-tray-tab")).toContainText(
    "CPU, memory, disk, upload, and download speed",
  );

  await expect(page.getByTestId("settings-tray-switch")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(page.getByTestId("settings-tray-system-stats-switch")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(page.getByTestId("settings-tray-system-stats-cpu-switch")).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expect(
    page.getByTestId("settings-tray-system-stats-download-switch"),
  ).toHaveAttribute("aria-checked", "true");

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/_bifrost/settings?tab=tray", { waitUntil: "domcontentloaded" });
  await expect(page.getByTestId("settings-tray-tab")).toBeVisible();
  const mobileLayout = await page.evaluate(() => ({
    viewportWidth: window.innerWidth,
    documentScrollWidth: document.documentElement.scrollWidth,
    traySwitchLeft: document
      .querySelector('[data-testid="settings-tray-switch"]')
      ?.getBoundingClientRect().left,
    statsSwitchLeft: document
      .querySelector('[data-testid="settings-tray-system-stats-switch"]')
      ?.getBoundingClientRect().left,
  }));
  expect(mobileLayout.documentScrollWidth).toBeLessThanOrEqual(
    mobileLayout.viewportWidth,
  );
  expect(mobileLayout.traySwitchLeft).toBeGreaterThan(250);
  expect(mobileLayout.statsSwitchLeft).toBeGreaterThan(250);

  await page.getByTestId("settings-tray-system-stats-switch").click();
  await waitForToast(page, "Tray system stats disabled");
  expect(updatePayloads).toContainEqual({ show_system_stats: false });
  await expect(page.getByTestId("settings-tray-system-stats-switch")).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await expect(
    page.getByTestId("settings-tray-system-stats-download-switch"),
  ).toBeDisabled();

  await page.getByTestId("settings-tray-system-stats-switch").click();
  await waitForToast(page, "Tray system stats enabled");
  await page.getByTestId("settings-tray-system-stats-download-switch").click();
  await waitForToast(page, "Tray system status item updated");
  expect(updatePayloads).toContainEqual({
    system_stats_items: { download: false },
  });
  await expect(
    page.getByTestId("settings-tray-system-stats-download-switch"),
  ).toHaveAttribute("aria-checked", "false");

  await page.getByTestId("settings-tray-switch").click();
  await waitForToast(page, "Tray icon disabled");
  expect(updatePayloads).toContainEqual({ enabled: false });
  await expect(page.getByTestId("settings-tray-switch")).toHaveAttribute(
    "aria-checked",
    "false",
  );
});

test("Settings Tray tab 在系统状态不支持的平台只展示托盘开关", async ({ page }) => {
  let trayConfig = {
    enabled: true,
    supported: true,
    system_stats_supported: false,
    show_system_stats: false,
    system_stats_items: {
      cpu: false,
      memory: false,
      disk: false,
      upload: false,
      download: false,
    },
  };
  const updatePayloads: unknown[] = [];

  await page.route("**/_bifrost/api/config", async (route) => {
    await route.fulfill({
      json: {
        host: "127.0.0.1",
        port: backendPort,
        tls: {
          enable_tls_interception: false,
          intercept_exclude: [],
          intercept_include: [],
          app_intercept_exclude: [],
          app_intercept_include: [],
          ip_intercept_exclude: [],
          ip_intercept_include: [],
          unsafe_ssl: true,
          disconnect_on_config_change: false,
        },
        tray: trayConfig,
      },
    });
  });
  await page.route("**/_bifrost/api/config/tray", async (route) => {
    if (route.request().method() === "PUT") {
      const payload = route.request().postDataJSON() as { enabled?: boolean };
      updatePayloads.push(payload);
      trayConfig = {
        ...trayConfig,
        ...payload,
      };
    }
    await route.fulfill({ json: trayConfig });
  });

  await openPage(page, "settings?tab=tray");
  await expect(page.getByRole("tab", { name: /Tray/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByTestId("settings-tray-tab")).toBeVisible();
  await expect(page.getByTestId("settings-tray-tab")).not.toContainText(
    "System Status",
  );
  await expect(
    page.getByTestId("settings-tray-system-stats-switch"),
  ).toHaveCount(0);
  await expect(
    page.getByTestId("settings-tray-system-stats-download-switch"),
  ).toHaveCount(0);

  await page.getByTestId("settings-tray-switch").click();
  await waitForToast(page, "Tray icon disabled");
  expect(updatePayloads).toEqual([{ enabled: false }]);
  await expect(page.getByTestId("settings-tray-switch")).toHaveAttribute(
    "aria-checked",
    "false",
  );
});

test("Settings Proxy 展示临时端口绑定规则详情卡片", async ({ page, request }) => {
  const ruleName = uniqueName("ui-temp-port-rule");
  let temporaryPort: number | null = null;

  try {
    await request.post(`${apiBase}/rules`, {
      data: {
        name: ruleName,
        content: `temp-card-ui.test status://218 resBody://(${ruleName})`,
        enabled: false,
      },
    });
    const bindRes = await request.post(`${apiBase}/ports`, {
      data: {
        port: 0,
        name: "UI temporary port",
        rule_refs: [{ type: "local_rule", name: ruleName }],
      },
    });
    const binding = (await bindRes.json()) as { port: number };
    temporaryPort = binding.port;

    await openPage(page, "settings?tab=proxy");
    const card = page.getByTestId(`settings-temporary-port-card-${temporaryPort}`);
    await expect(card).toBeVisible();
    await expect(card).toContainText(`127.0.0.1:${temporaryPort}`);
    await expect(card).toContainText("UI temporary port");
    await expect(card).toContainText(ruleName);
    await expect(page.getByTestId(`settings-temporary-port-merged-${temporaryPort}`)).toContainText(
      `resBody://(${ruleName})`,
    );
  } finally {
    if (temporaryPort !== null) {
      await request.delete(`${apiBase}/ports/${temporaryPort}`);
    }
    await request.delete(`${apiBase}/rules/${encodeURIComponent(ruleName)}`);
  }
});

test("Settings Sync 状态信息支持 connected、syncing 与 unreachable", async ({
  page,
  request,
}) => {
  const remoteServer = await startMockSyncServer([
    {
      id: uniqueName("remote-id"),
      user_id: "ui-sync-user",
      name: uniqueName("status-rule"),
      rule: "status.example.com host://127.0.0.1:3010",
      create_time: "2026-03-20T09:00:00Z",
      update_time: "2026-03-20T09:00:00Z",
    },
  ], undefined, { responseDelayMs: 250 });

  try {
    await request.post(`${apiBase}/sync/logout`).catch(() => undefined);
    await request.put(`${apiBase}/sync/config`, {
      data: {
        enabled: true,
        auto_sync: true,
        remote_base_url: remoteServer.baseUrl,
        probe_interval_secs: 2,
        connect_timeout_ms: 1000,
      },
    });

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Sync/ }).click({ force: true });
    await expect
      .poll(async () => {
        const value = await page.getByTestId("statusbar-sync").getAttribute("data-sync-state");
        return value === "unauthorized" || value === "unreachable";
      })
      .toBe(true);
    await expect(page.getByTestId("settings-sync-provider-grid")).toBeVisible();
    await expect(page.getByRole("heading", { name: "Remote Sync" })).toHaveCount(0);

    const loginUrlResponse = await request.get(
      `${apiBase}/sync/login-url?callback_url=${encodeURIComponent(
        `http://127.0.0.1:${backendPort}/login.html`,
      )}`,
    );
    const { login_url: loginUrl } = (await loginUrlResponse.json()) as {
      login_url: string;
    };
    await page.goto(loginUrl);

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { authorized: boolean; reachable: boolean };
        return body.authorized && body.reachable;
      })
      .toBe(true);

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Sync/ }).click({ force: true });

    await expect
      .poll(async () => {
        const value = await page.getByTestId("statusbar-sync").getAttribute("data-sync-state");
        return value === "connected" || value === "ready" || value === "syncing";
      })
      .toBe(true);

    await request.post(`${apiBase}/sync/run`);

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { syncing: boolean; reason: string };
        return body.syncing && body.reason;
      })
      .toBe("syncing");

    await request.put(`${apiBase}/sync/config`, {
      data: {
        enabled: true,
        auto_sync: true,
        remote_base_url: "http://127.0.0.1:9",
        probe_interval_secs: 2,
        connect_timeout_ms: 1000,
      },
    });

    await expect
      .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"))
      .toBe("unreachable");
  } finally {
    try {
      await request.put(`${apiBase}/sync/config`, {
        data: {
          enabled: false,
          remote_base_url: "https://bifrost.bytedance.net",
        },
      });
    } catch {
      // Ignore cleanup errors.
    }
    await remoteServer.close();
  }
});

test("Settings Sync 打开时会轮询刷新页面与底部状态栏", async ({ page }) => {
  let signedIn = false;
  const providerStatus = () => [
    {
      id: "bytedance_internal",
      name: "ByteDance Internal",
      description: "Internal trusted sync and Remote Invoke provider.",
      remote_base_url: "https://sync-poll.example.test",
      connected: signedIn,
      enabled: true,
      reachable: true,
      authorized: signedIn,
      user: signedIn
        ? {
            user_id: "poll-user",
            nickname: "Poll User",
            avatar: "",
            email: "poll-user@example.test",
          }
        : null,
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: signedIn,
    },
    {
      id: "bifrost_cloud",
      name: "Bifrost Cloud",
      description: "Custom Bifrost sync service for teams and self-hosting.",
      remote_base_url: null,
      connected: false,
      enabled: false,
      reachable: false,
      authorized: false,
      user: null,
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: false,
    },
    {
      id: "github_gist",
      name: "GitHub Gist",
      description: "Public GitHub Gist-backed portable sync provider.",
      remote_base_url: null,
      connected: false,
      enabled: false,
      reachable: false,
      authorized: false,
      user: null,
      capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
      remote_invoke_registered: false,
    },
  ];
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: "https://sync-poll.example.test",
        has_session: signedIn,
        reachable: true,
        authorized: signedIn,
        syncing: false,
        reason: signedIn ? "ready" : "unauthorized",
        last_sync_at: signedIn ? "2026-06-19T08:00:00Z" : null,
        last_sync_action: signedIn ? "no_change" : null,
        last_error: null,
        user: signedIn
          ? {
              user_id: "poll-user",
              nickname: "Poll User",
              avatar: "",
              email: "poll-user@example.test",
            }
          : null,
        providers: providerStatus(),
      }),
    });
  });

  await openPage(page, "settings?tab=sync");
  await expect(page.getByRole("tab", { name: /Sync/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByText("Sign in required")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Remote Sync" })).toHaveCount(0);
  await expect(page.getByTestId("settings-sync-provider-card-bytedance_internal")).toContainText(
    "Not signed in",
  );
  await expect(page.getByTestId("statusbar-sync")).toHaveAttribute(
    "data-sync-state",
    "unauthorized",
  );

  signedIn = true;
  await expect(
    page.getByTestId("settings-sync-provider-card-bytedance_internal"),
  ).toContainText("poll-user", {
    timeout: 5_000,
  });
  await expect(page.getByTestId("settings-sync-provider-card-bytedance_internal")).toContainText(
    "Connected",
  );
  await expect
    .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"))
    .toBe("ready");
});

test("Settings Sync 轮询刷新不会覆盖正在编辑的 Bifrost Cloud URL", async ({ page }) => {
  let signedIn = false;
  let savedRemoteBaseUrl = "";
  const providers = (cloudUrl: string) => [
    {
      id: "bytedance_internal",
      name: "ByteDance Internal",
      description: "Internal trusted sync and Remote Invoke provider.",
      remote_base_url: "https://bifrost.bytedance.net",
      connected: signedIn,
      enabled: true,
      reachable: true,
      authorized: signedIn,
      user: signedIn
        ? {
            user_id: "poll-user",
            nickname: "Poll User",
            avatar: "",
            email: "poll-user@example.test",
          }
        : null,
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: signedIn,
    },
    {
      id: "bifrost_cloud",
      name: "Bifrost Cloud",
      description: "Custom Bifrost sync service for teams and self-hosting.",
      remote_base_url: cloudUrl,
      connected: false,
      enabled: false,
      reachable: false,
      authorized: false,
      user: null,
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: false,
    },
    {
      id: "github_gist",
      name: "GitHub Gist",
      description: "Public GitHub Gist-backed portable sync provider.",
      remote_base_url: null,
      connected: false,
      enabled: false,
      reachable: false,
      authorized: false,
      user: null,
      capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
      remote_invoke_registered: false,
    },
  ];
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: "https://sync-poll.example.test",
        has_session: signedIn,
        reachable: true,
        authorized: signedIn,
        syncing: false,
        reason: signedIn ? "ready" : "unauthorized",
        last_sync_at: signedIn ? "2026-06-19T08:00:00Z" : null,
        last_sync_action: signedIn ? "no_change" : null,
        last_error: null,
        user: signedIn
          ? {
              user_id: "poll-user",
              nickname: "Poll User",
              avatar: "",
              email: "poll-user@example.test",
            }
          : null,
        providers: providers("https://sync-poll.example.test"),
      }),
    });
  });
  await page.route("**/_bifrost/api/sync/config", async (route) => {
    const payload = route.request().postDataJSON() as { remote_base_url?: string };
    savedRemoteBaseUrl = payload.remote_base_url ?? "";
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: savedRemoteBaseUrl,
        has_session: true,
        reachable: true,
        authorized: true,
        syncing: false,
        reason: "ready",
        last_sync_at: "2026-06-19T08:00:00Z",
        last_sync_action: "no_change",
        last_error: null,
        user: {
          user_id: "poll-user",
          nickname: "Poll User",
          avatar: "",
          email: "poll-user@example.test",
        },
        providers: providers(savedRemoteBaseUrl),
      }),
    });
  });

  await openPage(page, "settings?tab=sync");
  await expect(page.getByRole("heading", { name: "Remote Sync" })).toHaveCount(0);
  const remoteUrlInput = page.getByTestId("settings-sync-provider-bifrost-cloud-url-input");
  await expect(remoteUrlInput).toHaveValue("https://sync-poll.example.test");

  await remoteUrlInput.fill("https://custom-sync.example.test/custom/");
  signedIn = true;
  await expect
    .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"), {
      timeout: 5_000,
    })
    .toBe("ready");
  await expect(remoteUrlInput).toHaveValue("https://custom-sync.example.test/custom/");

  await page.getByTestId("settings-sync-provider-bifrost-cloud-url-save").click();
  await waitForToast(page, "Bifrost Cloud URL updated");
  expect(savedRemoteBaseUrl).toBe("https://custom-sync.example.test/custom");
  await expect(remoteUrlInput).toHaveValue("https://custom-sync.example.test/custom/");
});

test("Settings Sync Bifrost Cloud URL 必须先通过基础校验再连接", async ({ page }) => {
  let configCalls = 0;
  let loginCalls = 0;
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: "https://bifrost.bytedance.net",
        has_session: false,
        reachable: true,
        authorized: false,
        syncing: false,
        reason: "unauthorized",
        last_sync_at: null,
        last_sync_action: null,
        last_error: null,
        user: null,
        first_run_prompt_required: false,
        providers: [
          {
            id: "bytedance_internal",
            name: "ByteDance Internal",
            description: "Internal trusted sync and Remote Invoke provider.",
            remote_base_url: "https://bifrost.bytedance.net",
            connected: false,
            enabled: true,
            reachable: true,
            authorized: false,
            reason: "unauthorized",
            last_error: null,
            last_sync_at: null,
            last_sync_action: null,
            user: null,
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
          {
            id: "bifrost_cloud",
            name: "Bifrost Cloud",
            description: "Custom Bifrost sync service for teams and self-hosting.",
            remote_base_url: null,
            connected: false,
            enabled: false,
            reachable: false,
            authorized: false,
            reason: "unauthorized",
            last_error: null,
            last_sync_at: null,
            last_sync_action: null,
            user: null,
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
          {
            id: "github_gist",
            name: "GitHub Gist",
            description: "Public GitHub Gist-backed portable sync provider.",
            remote_base_url: "https://api.github.com/gists",
            connected: false,
            enabled: false,
            reachable: false,
            authorized: false,
            reason: "unauthorized",
            last_error: null,
            last_sync_at: null,
            last_sync_action: null,
            user: null,
            capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/sync/config", async (route) => {
    configCalls += 1;
    await route.abort();
  });
  await page.route("**/_bifrost/api/sync/login", async (route) => {
    loginCalls += 1;
    await route.abort();
  });

  await openPage(page, "settings?tab=sync");
  const remoteUrlInput = page.getByTestId("settings-sync-provider-bifrost-cloud-url-input");
  await expect(remoteUrlInput).toHaveValue("");

  await page.getByTestId("settings-sync-provider-login-bifrost_cloud").click();
  await waitForToast(page, "Please enter your self-hosted Bifrost Sync Server URL");
  expect(configCalls).toBe(0);
  expect(loginCalls).toBe(0);

  await remoteUrlInput.fill("http://custom-sync.example.test");
  await page.getByTestId("settings-sync-provider-login-bifrost_cloud").click();
  await waitForToast(page, "Bifrost Cloud URL must start with https://");
  expect(configCalls).toBe(0);
  expect(loginCalls).toBe(0);
});

test("Settings Sync 展示三类 Provider 卡片并支持首登弹窗关闭与启动", async ({ page }) => {
  let configUpdated = false;
  let loginOpened = false;
  const statusBody = {
    enabled: true,
    auto_sync: true,
    remote_base_url: "https://bifrost.bytedance.net",
    has_session: false,
    reachable: true,
    authorized: false,
    syncing: false,
    reason: "unauthorized",
    last_sync_at: null,
    last_sync_action: null,
    last_error: null,
    user: null,
    first_run_prompt_required: true,
    providers: [
      {
        id: "bytedance_internal",
        name: "ByteDance Internal",
        description: "Internal trusted sync and Remote Invoke provider.",
        remote_base_url: "https://bifrost.bytedance.net",
        connected: false,
        enabled: true,
        reachable: true,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
      {
        id: "bifrost_cloud",
        name: "Bifrost Cloud",
        description: "Custom Bifrost sync service for teams and self-hosting.",
        remote_base_url: "https://sync.example.test",
        connected: false,
        enabled: false,
        reachable: false,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
      {
        id: "github_gist",
        name: "GitHub Gist",
        description: "Public GitHub Gist-backed portable sync provider.",
        remote_base_url: null,
        connected: false,
        enabled: false,
        reachable: false,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
    ],
  };

  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(statusBody),
    });
  });
  await page.route("**/_bifrost/api/sync/config", async (route) => {
    configUpdated = true;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(statusBody),
    });
  });
  await page.route("**/_bifrost/api/sync/login", async (route) => {
    loginOpened = true;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(statusBody),
    });
  });

  await openPage(page, "settings?tab=sync");

  await expect(page.getByTestId("settings-sync-provider-grid")).toBeVisible();
  await expect(page.getByTestId("settings-sync-provider-card-bytedance_internal")).toBeVisible();
  await expect(page.getByTestId("settings-sync-provider-card-bifrost_cloud")).toBeVisible();
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toBeVisible();
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toContainText(
    "Not supported",
  );
  await expect(page.getByTestId("settings-sync-provider-login-github_gist")).toBeEnabled();

  const modal = page.getByRole("dialog", { name: "Choose a sync service" });
  await expect(modal).toBeVisible();
  await page.getByRole("button", { name: "Not now" }).click();
  await expect(modal).toBeHidden();

  await page.reload();
  await expect(page.getByRole("dialog", { name: "Choose a sync service" })).toBeVisible();
  await page.getByTestId("settings-sync-first-run-start").click();
  await expect.poll(() => Promise.resolve(configUpdated)).toBe(true);
  await expect.poll(() => Promise.resolve(loginOpened)).toBe(true);
});

test("Settings Sync GitHub Gist 支持 token 登录", async ({ page }) => {
  let loginPayload: { provider_id?: string; token?: string } | null = null;
  const baseStatus = {
    enabled: true,
    auto_sync: true,
    remote_base_url: "https://bifrost.bytedance.net",
    has_session: false,
    reachable: true,
    authorized: false,
    syncing: false,
    reason: "unauthorized",
    last_sync_at: null,
    last_sync_action: null,
    last_error: null,
    user: null,
    first_run_prompt_required: false,
    providers: [
      {
        id: "bytedance_internal",
        name: "ByteDance Internal",
        description: "Internal trusted sync and Remote Invoke provider.",
        remote_base_url: "https://bifrost.bytedance.net",
        connected: false,
        enabled: true,
        reachable: true,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
      {
        id: "bifrost_cloud",
        name: "Bifrost Cloud",
        description: "Custom Bifrost sync service for teams and self-hosting.",
        remote_base_url: "https://sync.example.test",
        connected: false,
        enabled: false,
        reachable: false,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
      {
        id: "github_gist",
        name: "GitHub Gist",
        description: "Public GitHub Gist-backed portable sync provider.",
        remote_base_url: "https://api.github.com/gists",
        connected: false,
        enabled: false,
        reachable: false,
        authorized: false,
        user: null,
        capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
        remote_invoke_registered: false,
      },
    ],
  };
  const connectedStatus = {
    ...baseStatus,
    providers: baseStatus.providers.map((provider) =>
      provider.id === "github_gist"
        ? {
            ...provider,
            connected: true,
            enabled: true,
            reachable: true,
            authorized: true,
            user: {
              user_id: "github:12345",
              nickname: "octocat",
              avatar: "",
              email: "",
            },
          }
        : provider,
    ),
  };

  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(loginPayload ? connectedStatus : baseStatus),
    });
  });
  await page.route("**/_bifrost/api/sync/login", async (route) => {
    loginPayload = route.request().postDataJSON() as typeof loginPayload;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(connectedStatus),
    });
  });

  await openPage(page, "settings?tab=sync");
  await expect(page.getByTestId("settings-sync-provider-github-gist-token-link")).toHaveAttribute(
    "href",
    /github\.com\/settings\/tokens\/new.*scopes=gist/,
  );
  await page.getByTestId("settings-sync-provider-login-github_gist").click();
  const modal = page.getByRole("dialog", { name: "Sign in to GitHub Gist" });
  await expect(modal).toBeVisible();
  await expect(
    page.getByTestId("settings-sync-provider-github-gist-modal-token-link"),
  ).toHaveAttribute("href", /github\.com\/settings\/tokens\/new.*scopes=gist/);
  await page.getByTestId("settings-sync-provider-github-gist-token-input").fill("ghp_test_token");
  await modal.getByRole("button", { name: "Sign In" }).click();

  await expect.poll(() => Promise.resolve(loginPayload)).toEqual({
    provider_id: "github_gist",
    token: "ghp_test_token",
  });
  await waitForToast(page, "GitHub Gist connected");
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toContainText(
    "Connected",
  );
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toContainText(
    "github:12345",
  );
});

test("Settings Sync GitHub Gist token 失效时显示卡片级重连提示", async ({ page }) => {
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: "https://bifrost.bytedance.net",
        has_session: true,
        reachable: true,
        authorized: false,
        syncing: false,
        reason: "error",
        last_sync_at: null,
        last_sync_action: null,
        last_error: "GitHub token is invalid or missing the gist scope",
        user: null,
        first_run_prompt_required: false,
        providers: [
          {
            id: "bytedance_internal",
            name: "ByteDance Internal",
            description: "Internal trusted sync and Remote Invoke provider.",
            remote_base_url: "https://bifrost.bytedance.net",
            connected: false,
            enabled: true,
            reachable: true,
            authorized: false,
            reason: "unauthorized",
            last_error: null,
            user: null,
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
          {
            id: "bifrost_cloud",
            name: "Bifrost Cloud",
            description: "Custom Bifrost sync service for teams and self-hosting.",
            remote_base_url: "https://sync.example.test",
            connected: false,
            enabled: false,
            reachable: false,
            authorized: false,
            reason: "unauthorized",
            last_error: null,
            user: null,
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
          {
            id: "github_gist",
            name: "GitHub Gist",
            description: "Public GitHub Gist-backed portable sync provider.",
            remote_base_url: "https://api.github.com/gists",
            connected: true,
            enabled: true,
            reachable: true,
            authorized: false,
            reason: "error",
            last_error: "GitHub token is invalid or missing the gist scope",
            last_sync_at: "2026-07-07T07:30:00Z",
            last_sync_action: "remote_pulled",
            last_changed_sync_at: "2026-07-07T07:20:00Z",
            last_changed_sync_action: "local_pushed",
            check_interval_secs: 300,
            user: {
              user_id: "github:12345",
              nickname: "octocat",
              avatar: "",
              email: "",
            },
            capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
        ],
      }),
    });
  });

  await openPage(page, "settings?tab=sync");

  const gistCard = page.getByTestId("settings-sync-provider-card-github_gist");
  await expect(page.getByTestId("settings-sync-provider-overview-alert")).toHaveCount(0);
  await expect(gistCard).toContainText("Reconnect required");
  await expect(gistCard).toContainText("GitHub token is invalid or missing the gist scope");
  await expect(gistCard).toContainText("Last change");
  await expect(gistCard).toContainText("Last check");
  await expect(gistCard).toContainText("Check interval");
  await expect(gistCard).toContainText("local pushed");
  await expect(gistCard).toContainText("remote pulled");
  await expect(gistCard).toContainText("Every 5 min");
  await expect(page.getByTestId("settings-sync-provider-error-github_gist")).toBeVisible();
  await expect(page.getByTestId("settings-sync-provider-github-gist-token-link")).toContainText(
    "New Token",
  );
  await expect(page.getByTestId("settings-sync-provider-login-github_gist")).toContainText(
    "Reconnect",
  );
  await expect(page.getByTestId("settings-sync-provider-logout-github_gist")).toBeVisible();
});

test("Settings Sync provider 退出登录只影响当前卡片", async ({ page }) => {
  let scopedLogoutCount = 0;
  let globalLogoutCount = 0;
  const providers = [
    {
      id: "bytedance_internal",
      name: "ByteDance Internal",
      description: "Internal trusted sync and Remote Invoke provider.",
      remote_base_url: "https://bifrost.bytedance.net",
      connected: true,
      enabled: true,
      reachable: true,
      authorized: true,
      reason: "ready",
      last_error: null,
      last_sync_at: "2026-07-07T07:10:00Z",
      last_sync_action: "no_change",
      user: {
        user_id: "byte-user",
        nickname: "Byte User",
        avatar: "",
        email: "byte-user@example.test",
      },
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: true,
    },
    {
      id: "bifrost_cloud",
      name: "Bifrost Cloud",
      description: "Custom Bifrost sync service for teams and self-hosting.",
      remote_base_url: "https://sync.example.test",
      connected: true,
      enabled: true,
      reachable: true,
      authorized: true,
      reason: "ready",
      last_error: null,
      last_sync_at: "2026-07-07T07:20:00Z",
      last_sync_action: "no_change",
      user: {
        user_id: "cloud-user",
        nickname: "Cloud User",
        avatar: "",
        email: "cloud-user@example.test",
      },
      capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
      remote_invoke_registered: true,
    },
    {
      id: "github_gist",
      name: "GitHub Gist",
      description: "Public GitHub Gist-backed portable sync provider.",
      remote_base_url: "https://api.github.com/gists",
      connected: true,
      enabled: true,
      reachable: true,
      authorized: true,
      reason: "ready",
      last_error: null,
      last_sync_at: "2026-07-07T07:30:00Z",
      last_sync_action: "no_change",
      user: {
        user_id: "github:12345",
        nickname: "octocat",
        avatar: "",
        email: "",
      },
      capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
      remote_invoke_registered: false,
    },
  ];
  const statusBody = () => ({
    enabled: true,
    auto_sync: true,
    remote_base_url: "https://bifrost.bytedance.net",
    has_session: providers.some((provider) => provider.connected),
    reachable: true,
    authorized: providers.some((provider) => provider.connected),
    syncing: false,
    reason: "ready",
    last_sync_at: "2026-07-07T07:30:00Z",
    last_sync_action: "no_change",
    last_error: null,
    user: null,
    first_run_prompt_required: false,
    providers,
  });

  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(statusBody()),
    });
  });
  await page.route("**/_bifrost/api/sync/providers/bifrost_cloud/logout", async (route) => {
    scopedLogoutCount += 1;
    const cloud = providers.find((provider) => provider.id === "bifrost_cloud");
    if (cloud) {
      cloud.connected = false;
      cloud.authorized = false;
      cloud.reachable = false;
      cloud.reason = "unauthorized";
      cloud.user = null;
      cloud.remote_invoke_registered = false;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(statusBody()),
    });
  });
  await page.route("**/_bifrost/api/sync/logout", async (route) => {
    globalLogoutCount += 1;
    await route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "global logout should not be called" }),
    });
  });

  await openPage(page, "settings?tab=sync");
  await page.getByTestId("settings-sync-provider-logout-bifrost_cloud").click();

  await expect.poll(() => Promise.resolve(scopedLogoutCount)).toBe(1);
  expect(globalLogoutCount).toBe(0);
  await waitForToast(page, "Bifrost Cloud signed out");
  await expect(page.getByTestId("settings-sync-provider-card-bifrost_cloud")).toContainText(
    "Offline",
  );
  await expect(page.getByTestId("settings-sync-provider-card-bifrost_cloud")).toContainText(
    "Sign In",
  );
  await expect(page.getByTestId("settings-sync-provider-card-bytedance_internal")).toContainText(
    "Connected",
  );
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toContainText(
    "Connected",
  );
  await expect(page.getByTestId("settings-sync-provider-overview-alert")).toHaveCount(0);
});

test("Settings Sync Remote Invoke 支持 ByteDance、Bifrost Cloud 与双通道状态", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: true,
        remote_base_url: "https://bifrost.bytedance.net",
        has_session: true,
        reachable: true,
        authorized: true,
        syncing: false,
        reason: "ready",
        last_sync_at: "2026-07-07T00:00:00Z",
        last_sync_action: "no_change",
        last_error: null,
        user: {
          user_id: "byte-user",
          nickname: "Byte User",
          avatar: "",
          email: "byte-user@example.test",
        },
        first_run_prompt_required: false,
        providers: [
          {
            id: "bytedance_internal",
            name: "ByteDance Internal",
            description: "Internal trusted sync and Remote Invoke provider.",
            remote_base_url: "https://bifrost.bytedance.net",
            connected: true,
            enabled: true,
            reachable: true,
            authorized: true,
            user: {
              user_id: "byte-user",
              nickname: "Byte User",
              avatar: "",
              email: "byte-user@example.test",
            },
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: true,
          },
          {
            id: "bifrost_cloud",
            name: "Bifrost Cloud",
            description: "Custom Bifrost sync service for teams and self-hosting.",
            remote_base_url: "https://sync.example.test",
            connected: true,
            enabled: true,
            reachable: true,
            authorized: true,
            user: {
              user_id: "cloud-user",
              nickname: "Cloud User",
              avatar: "",
              email: "cloud-user@example.test",
            },
            capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
            remote_invoke_registered: true,
          },
          {
            id: "github_gist",
            name: "GitHub Gist",
            description: "Public GitHub Gist-backed portable sync provider.",
            remote_base_url: null,
            connected: false,
            enabled: false,
            reachable: false,
            authorized: false,
            user: null,
            capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
            remote_invoke_registered: false,
          },
        ],
      }),
    });
  });

  await openPage(page, "settings?tab=sync");
  await expect(page.getByTestId("settings-sync-provider-card-bytedance_internal")).toContainText(
    "Registered",
  );
  await expect(page.getByTestId("settings-sync-provider-card-bifrost_cloud")).toContainText(
    "Registered",
  );
  await expect(page.getByTestId("settings-sync-provider-card-github_gist")).toContainText(
    "Not supported",
  );
});

test("Settings Agent 三层 instructions 使用大窗口编辑", async ({ page }) => {
  let patchPayload: Record<string, unknown> | null = null;
  const agentConfig = {
    enabled: true,
    work_dir: "/tmp/agent-ui",
    base_instructions: "",
    developer_instructions: "Initial developer instructions",
    user_instructions: "Initial user instructions",
    default_base_instructions: "Default base prompt\nwith multiple lines",
    model_providers: {},
    mcp_servers: {},
  };

  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    if (route.request().method() === "PATCH") {
      patchPayload = route.request().postDataJSON() as Record<string, unknown>;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ...agentConfig, ...patchPayload }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(agentConfig),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/providers", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });

  await openPage(page, "ai?aiSection=agent-general&agentSection=general");

  await expect(page.getByTestId("settings-agent-base-instructions-button")).toBeVisible();
  await expect(page.getByTestId("settings-agent-developer-instructions-button")).toBeVisible();
  await expect(page.getByTestId("settings-agent-user-instructions-button")).toBeVisible();
  await expect(page.locator("body")).not.toContainText("Default Base Instructions (read-only)");
  await expect(page.getByTestId("settings-agent-base-instructions-preview")).toContainText(
    "Default base prompt",
  );
  await expect(
    page.getByTestId("settings-agent-base-instructions").locator("textarea"),
  ).toHaveCount(0);

  await page.getByTestId("settings-agent-base-instructions-button").click();
  const editor = page.getByRole("dialog", { name: "Base Instructions / System Prompt" });
  await expect(editor).toBeVisible();
  await editor.getByTestId("settings-agent-base-instructions-copy-placeholder").click();
  await expect(editor.getByTestId("settings-agent-base-instructions-modal-textarea")).toHaveValue(
    "Default base prompt\nwith multiple lines",
  );
  await editor.getByTestId("settings-agent-base-instructions-modal-textarea").fill(
    "Default base prompt\nwith multiple lines\nEdited base prompt from large modal",
  );
  await editor.getByRole("button", { name: "OK" }).click();

  await expect(page.getByTestId("settings-agent-base-instructions-preview")).toContainText(
    "Default base prompt",
  );
  await expect
    .poll(() => patchPayload?.base_instructions)
    .toBe("Default base prompt\nwith multiple lines\nEdited base prompt from large modal");
});

test("Agent Runners 新增弹窗只展示当前支持的 Adapter", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        work_dir: "/tmp/agent-ui",
        model_providers: {},
        mcp_servers: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/providers", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        defaultRunnerId: "codex",
        runners: {
          codex: { enabled: true, adapter: "codex", adapterConfig: {} },
          traex: { enabled: true, adapter: "traex", adapterConfig: {} },
          "Claude-Code": { enabled: true, adapter: "claude_code", adapterConfig: {} },
          web: { enabled: true, adapter: "chatgpt_web", adapterConfig: {} },
        },
        channels: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });

  await openPage(page, "ai?aiSection=agent-runners&agentSection=runners");
  await expect(page.getByTestId("agent-settings-section-runners")).toBeVisible();

  await page.getByRole("button", { name: "Add Runner" }).click();
  const dialog = page.getByRole("dialog", { name: "Add Runner" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Adapter").click();

  const adapterDropdown = page.locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)");
  await expect(adapterDropdown).toContainText("Codex CLI");
  await expect(adapterDropdown).toContainText("Traex CLI");
  await expect(adapterDropdown).toContainText("Claude Code");
  await expect(adapterDropdown).toContainText("ChatGPT Web");
  await expect(adapterDropdown).not.toContainText("Custom");
  await expect(adapterDropdown).not.toContainText("Mock");
});

test("AI Agent Chat section 展示聊天工作台并支持真实流式发送", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ sessions: [] }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"Streaming response"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"API run complete"}\n\n',
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ defaultRunnerId: "codex", runners: {}, channels: {} }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/stream", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body:
        'event: run_started\ndata: {"eventType":"run_started"}\n\n' +
        'event: assistant_delta\ndata: {"eventType":"assistant_delta","content":"Streaming response"}\n\n' +
        'event: run_finished\ndata: {"eventType":"run_finished","response":"API run complete"}\n\n',
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

  const input = page.getByTestId("agent-chat-input");
  await input.fill("Create a safe UI implementation plan");
  await page.getByTestId("agent-chat-send").click();

  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "Create a safe UI implementation plan",
  );
  await expect(page.getByTestId("agent-chat-messages")).toContainText(
    "API run complete",
  );
  await expect(input).toHaveValue("");
});

test("AI Agent Session 详情默认展示 Messages Tab 且内容区可真实滚动", async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 520 });

  const agentConfig = {
    enabled: true,
    work_dir: "/tmp/agent-ui",
    model: "gpt-test",
    model_provider: "mock",
    model_providers: {
      mock: {
        api_key: "$MODEL_API_KEY",
      },
    },
    mcp_servers: {},
  };
  const historyPath = "/tmp/bifrost-agent-session-history.jsonl";
  const now = Math.floor(Date.now() / 1000);
  const events = Array.from({ length: 36 }, (_, index) => ({
    event_type: index % 3 === 0 ? "user_message" : index % 3 === 1 ? "assistant_message" : "tool_call",
    timestamp: now + index,
    content:
      index % 3 === 2
        ? {
            tool_name: `mock_tool_${index}`,
            arguments: `{"index":${index},"payload":"${"tool argument ".repeat(10)}"}`,
          }
        : {
            message: `Session event ${index + 1}: ${"message body ".repeat(12)}`,
          },
  }));

  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(agentConfig),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([{ id: "mock", name: "Mock Provider" }]),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/history/*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ events, count: events.length }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/agent-ui",
        content: "AGENTS.md instructions for session detail settings.",
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/skills", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/agent-ui",
        home_dir: "/tmp",
        skills: [
          {
            name: "repo-skill",
            scope: "repo",
            path: "/tmp/agent-ui/.agents/skills/repo-skill/SKILL.md",
            description: "Repo skill",
          },
        ],
      }),
    });
  });

  await openPage(
    page,
    `ai?aiSection=agent-sessions&agentSection=sessions&session=weixin%3Atab-scroll%40im.wechat&view=history&historyPath=${encodeURIComponent(historyPath)}`,
  );

  await expect(page.getByRole("tab", { name: /Messages/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: /Settings/ })).toBeVisible();
  await expect(page.getByText("Event Timeline")).toBeVisible();

  const scrollRegion = page.getByTestId("agent-session-messages-scroll");
  await expect(scrollRegion).toBeVisible();
  const beforeScroll = await scrollRegion.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    overflowY: window.getComputedStyle(element).overflowY,
    scrollTop: element.scrollTop,
  }));
  expect(beforeScroll.scrollHeight).toBeGreaterThan(beforeScroll.clientHeight);
  expect(beforeScroll.overflowY).toBe("auto");

  await scrollRegion.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  const afterScrollTop = await scrollRegion.evaluate((element) => element.scrollTop);
  expect(afterScrollTop).toBeGreaterThan(beforeScroll.scrollTop);
  await expect(page.getByText("mock_tool_35")).toBeVisible();

  await page.getByRole("tab", { name: /Settings/ }).click();
  await expect(page.getByRole("tab", { name: /Settings/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("agent-session-settings-panel")).toBeVisible();
  await expect(page.getByText("Session Info")).toBeVisible();
  await expect(page.getByText("AGENTS.md Instructions", { exact: true })).toBeVisible();
  await expect(page.getByText("repo-skill")).toBeVisible();
});

test("AI Agent Sessions 列表支持点击 title 或整行进入详情", async ({ page }) => {
  const agentConfig = {
    enabled: true,
    work_dir: "/tmp/agent-ui",
    model: "gpt-test",
    model_provider: "mock",
    model_providers: {
      mock: {
        api_key: "$MODEL_API_KEY",
      },
    },
    mcp_servers: {},
  };
  const now = Math.floor(Date.now() / 1000);
  const historyPath = "/tmp/clickable-history.jsonl";
  const historySessionKey = "weixin:row-click@im.wechat";
  const activeSessionKey = "api-active-row";

  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(agentConfig),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([{ id: "mock", name: "Mock Provider" }]),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/all", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        sessions: [
          {
            session_key: historySessionKey,
            status: "ended",
            source: "api",
            work_dir: "/tmp/agent-ui",
            turns: 4,
            tokens: 128,
            start_time: now - 120,
            last_active_time: now - 60,
            duration_secs: 60,
            title: "Clickable history title",
            history_path: historyPath,
          },
          {
            session_key: activeSessionKey,
            status: "active",
            running: false,
            state: "idle",
            source: "api",
            work_dir: "/tmp/agent-ui",
            turns: 2,
            tokens: 64,
            start_time: now - 30,
            last_active_time: now,
            duration_secs: 30,
            title: "Clickable active title",
          },
        ],
        total: 2,
        active_count: 1,
        history_count: 1,
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/sessions/history/*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        events: [
          {
            event_type: "session_start",
            timestamp: now - 120,
            content: { model: "gpt-test", provider: "mock" },
          },
          {
            event_type: "user_message",
            timestamp: now - 90,
            content: { message: "Open history from title" },
          },
          {
            event_type: "assistant_message",
            timestamp: now - 60,
            content: { message: "History detail opened" },
          },
        ],
        count: 3,
      }),
    });
  });
  await page.route(
    `**/_bifrost/api/im-gateway/agent/sessions/${encodeURIComponent(activeSessionKey)}`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_key: activeSessionKey,
          source: "api",
          work_dir: "/tmp/agent-ui",
          message_count: 2,
          total_tokens_used: 64,
          created_at: now - 30,
          last_active_at: now,
          compaction_count: 0,
          estimated_tokens: 32,
          messages: [
            { role: "user", content: "Open active from row" },
            { role: "assistant", content: "Active detail opened" },
          ],
        }),
      });
    },
  );
  await page.route("**/_bifrost/api/im-gateway/agent/instructions", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/agent-ui",
        content: "AGENTS.md instructions for sessions list click test.",
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/agent/skills", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/agent-ui",
        home_dir: "/tmp",
        skills: [],
      }),
    });
  });

  await openPage(page, "ai?aiSection=agent-sessions&agentSection=sessions");

  await expect(page.locator(".anticon-eye")).toHaveCount(0);
  await expect(
    page.getByTestId("agent-session-row").filter({ hasText: "Clickable history title" }),
  ).toBeVisible();
  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByTestId("agent-session-row").filter({ hasText: "Clickable history title" }),
  ).toBeVisible();

  await page
    .getByTestId("agent-session-title")
    .filter({ hasText: "Clickable history title" })
    .click();
  await expect(page).toHaveURL(/session=weixin%3Arow-click%40im\.wechat/);
  await expect(page).toHaveURL(/view=history/);
  await expect(page).toHaveURL(new RegExp(`historyPath=${encodeURIComponent(historyPath)}`));
  await expect(page.getByRole("tab", { name: /Messages/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("History detail opened")).toBeVisible();

  await openPage(page, "ai?aiSection=agent-sessions&agentSection=sessions");
  await expect(page.locator(".anticon-eye")).toHaveCount(0);
  await expect(
    page.getByTestId("agent-session-row").filter({ hasText: "Clickable active title" }),
  ).not.toContainText("Running");
  await expect(
    page.getByTestId("agent-session-row").filter({ hasText: "Clickable active title" }),
  ).toContainText("Active");
  await page
    .getByTestId("agent-session-row")
    .filter({ hasText: "Clickable active title" })
    .click();
  await expect(page).toHaveURL(/session=api-active-row/);
  await expect(page).toHaveURL(/view=active/);
  await expect(page.getByRole("tab", { name: /Messages/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("Active detail opened")).toBeVisible();
});

test("Settings IM Provider instructions 使用大窗口编辑后保存覆盖值", async ({
  page,
}) => {
  let providerPatch: Record<string, unknown> | null = null;
  const provider = {
    id: "modal-provider",
    provider_type: "feishu",
    display_name: "Modal Provider",
    enabled: true,
    app_id: "cli_a123456789",
    secret_configured: true,
    owner_open_id: "ou_modal_owner",
    event_connection_enabled: true,
    event_types: [],
    agent_config: {
      work_dir: "/tmp/provider",
      base_instructions: "Provider base before edit",
      developer_instructions: "Provider developer before edit",
      user_instructions: "Provider user before edit",
    },
    created_at: 1,
    updated_at: 1,
  };

  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/default-agent",
        default_base_instructions: "Default base inherited",
        developer_instructions: "Default developer inherited",
        user_instructions: "Default user inherited",
        model_providers: {},
        mcp_servers: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers/modal-provider/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: "disconnected", reconnect_count: 0 }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers/modal-provider", async (route) => {
    if (route.request().method() === "PATCH") {
      providerPatch = route.request().postDataJSON() as Record<string, unknown>;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ...provider, ...providerPatch }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(provider),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([provider]),
    });
  });

  await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
  await page.getByTestId("settings-im-provider-edit-modal-provider").click();
  await expect(page.getByRole("dialog", { name: "Edit IM Provider" })).toBeVisible();
  await page.getByTestId("settings-im-provider-base-instructions-button").click();

  const editor = page.getByRole("dialog", { name: "Base Instructions / System Prompt" });
  await expect(editor).toBeVisible();
  await editor.getByTestId("settings-im-provider-base-instructions-modal-textarea").fill(
    "Provider base edited from large modal",
  );
  await editor.getByRole("button", { name: "OK" }).click();

  await expect(page.getByTestId("settings-im-provider-base-instructions-preview")).toContainText(
    "Provider base edited from large modal",
  );
  await page
    .getByRole("dialog", { name: "Edit IM Provider" })
    .getByRole("button", {
      name: "Save",
    })
    .click();

  await expect
    .poll(() => {
      const agentConfig = providerPatch?.agent_config as
        | { base_instructions?: string }
        | undefined;
      return agentConfig?.base_instructions;
    })
    .toBe("Provider base edited from large modal");
});

test("AI 一级页整合 IM Gateway 子导航并按 URL 切换独立面板", async ({ page }) => {
  await page.route("**/_bifrost/api/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        work_dir: "/tmp/default-agent",
        model_providers: {},
        mcp_servers: {},
      }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers/*/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ state: "disconnected", reconnect_count: 0 }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          id: "provider-nav-1",
          provider_type: "feishu",
          display_name: "Nav Provider",
          enabled: true,
          app_id: "cli_nav",
          secret_configured: true,
          event_connection_enabled: true,
          event_types: [],
          created_at: 1,
          updated_at: 1,
        },
      ]),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/targets", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/routes", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/schedules", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/history/events", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });
  await page.route("**/_bifrost/api/im-gateway/history/runs", async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  });

  await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");

  await expect(page.getByTestId("ai-section-nav")).toBeVisible();
  await expect(page.getByTestId("ai-nav-im-gateway-connections")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-connections")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /Connections/ })).toHaveCount(0);

  await page.getByTestId("ai-nav-im-gateway-routes").click();
  await expect(page).toHaveURL(/aiSection=im-gateway-routes/);
  await expect(page).toHaveURL(/imGatewaySection=routes/);
  await expect(page.getByTestId("ai-nav-im-gateway-routes")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-connections")).toHaveCount(0);

  await page.reload();
  await expect(page.getByTestId("ai-nav-im-gateway-routes")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();

  await page.getByTestId("ai-nav-im-gateway-history").click();
  await expect(page).toHaveURL(/aiSection=im-gateway-history/);
  await expect(page).toHaveURL(/imGatewaySection=history/);
  await expect(page.getByTestId("im-gateway-section-history")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /Events/ })).toBeVisible();

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.getByTestId("ai-nav-im-gateway-targets").click();
  await expect(page).toHaveURL(/aiSection=im-gateway-targets/);
  await expect(page).toHaveURL(/imGatewaySection=targets/);
  await expect(page.getByTestId("ai-nav-im-gateway-targets")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-targets")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-history")).toHaveCount(0);
});

test("Settings Remote Invoke 将 Connection Status 与 Discovery Mode 合并到同一张状态卡片", async ({
  page,
}) => {
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: false,
        remote_base_url: "http://127.0.0.1:8787",
        has_session: true,
        reachable: true,
        authorized: true,
        syncing: false,
        reason: "ready",
        user: {
          user_id: "sync-user-1",
          nickname: "Remote Tester",
          avatar: "",
          email: "remote-tester@example.test",
        },
      }),
    });
  });
  await page.route("**/_bifrost/api/remote-invoke/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "connected",
        discovery_session: null,
        pending_pairings_count: 0,
        active_call_ids: ["call-ui-1"],
      }),
    });
  });
  await page.route("**/_bifrost/api/remote-invoke/identity", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        instance_id: "client-ui-1",
        device_name: "UI Test Device",
        platform: "darwin",
      }),
    });
  });
  await page.route("**/_bifrost/api/remote-invoke/calls**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        calls: [
          {
            call_id: "call-ui-1",
            grant_id: "grant-ui-1",
            client_instance_id: "client-ui-1",
            caller_fingerprint: "caller-ui-fingerprint",
            caller_display_name: "Remote Tester",
            command: { command: "status", kind: "query.readonly" },
            command_summary: { command_preview: "status" },
            command_kind: "query.readonly",
            status: "completed",
            created_at: Date.now(),
            started_at: Date.now(),
            finished_at: Date.now(),
            ended_at: Date.now(),
            exit_code: 0,
            duration_ms: 12,
            bytes_in: 0,
            bytes_out: 128,
          },
        ],
      }),
    });
  });

  await openPage(page, "settings");
  await page.getByRole("tab", { name: /Remote Invoke/ }).click({ force: true });

  const statusCard = page.getByTestId("settings-remote-invoke-status-card");
  const connectionSection = page.getByTestId(
    "settings-remote-invoke-connection-section",
  );
  const discoverySection = page.getByTestId(
    "settings-remote-invoke-discovery-section",
  );
  const sshCard = page.getByTestId("settings-remote-invoke-ssh-card");
  const shellCard = page.getByTestId("settings-remote-invoke-shell-card");

  await expect(statusCard).toBeVisible();
  await expect(connectionSection).toContainText("Connection Status");
  await expect(connectionSection).toContainText("Relay Connection");
  await expect(connectionSection).not.toContainText("Active Calls");
  await expect(discoverySection).toContainText("Discovery Mode");
  await expect(discoverySection).toContainText("Enter Discovery Mode");
  await expect(sshCard).toBeVisible();
  await expect(sshCard).toContainText("SSH Key");
  await expect(shellCard).toBeVisible();
  await expect(shellCard).toContainText("Shell Access");
  await expect(shellCard).toContainText("Configuration Mode");
  await expect(shellCard).toContainText("Policy Set Version");
  await expect(page.getByText("Recent Calls", { exact: true })).toBeVisible();
  await expect(page.getByText("by Remote Tester")).toBeVisible();
});

test("Settings Remote Invoke Grants 展示 SSH key 与 Pair code 连接方式", async ({
  page,
}) => {
  const now = Date.now();
  await page.route("**/_bifrost/api/remote-invoke/grants", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        grants: [
          {
            grant_id: "grant-ssh-1",
            client_instance_id: "client-1",
            caller_fingerprint: "sshcallerabcdef123456",
            caller_display_name: "ssh caller",
            auth_method: "ssh_publickey",
            ssh_key_fingerprint: "SHA256:ssh-key-fingerprint",
            grant_mode: "permanent",
            grant_scope: "remote_shell_exec",
            status: "active",
            created_at: now,
            first_authorized_at: now,
            expires_at: null,
            last_used_at: now,
            max_calls: 999999,
            remaining_calls: 999999,
            use_count: 1,
            file_access: "none",
          },
          {
            grant_id: "grant-code-1",
            client_instance_id: "client-1",
            caller_fingerprint: "codecallerabcdef123456",
            caller_display_name: "code caller",
            auth_method: "pair_code",
            grant_mode: "1h",
            grant_scope: "remote_query",
            status: "active",
            created_at: now,
            first_authorized_at: now,
            expires_at: now + 60 * 60 * 1000,
            last_used_at: now,
            max_calls: 999999,
            remaining_calls: 999998,
            use_count: 1,
            file_access: "none",
          },
        ],
      }),
    });
  });

  await openPage(page, "settings");
  await page.getByRole("tab", { name: /Remote Invoke/ }).click({ force: true });

  const grantsCard = page.getByTestId("settings-remote-invoke-grants-card");
  await expect(grantsCard).toContainText("ssh caller");
  await expect(grantsCard).toContainText("SSH key");
  await expect(grantsCard).toContainText("code caller");
  await expect(grantsCard).toContainText("Pair code");
});

test("Settings Remote Invoke 未登录 Sync 的 Remote Status 提示兼容黑色主题", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "dark" }, version: 0 }),
    );
  });
  await page.route("**/_bifrost/api/sync/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        auto_sync: false,
        remote_base_url: "https://sync.example.test",
        has_session: false,
        reachable: true,
        authorized: false,
        syncing: false,
        reason: "not_signed_in",
        user: null,
      }),
    });
  });

  await openPage(page, "settings");
  await page.getByRole("tab", { name: /Remote Invoke/ }).click({ force: true });

  const prompt = page.getByTestId("settings-remote-invoke-sync-signin-prompt");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("Sign in to Sync to enable Remote Invoke");
  await expect(prompt).toContainText("https://sync.example.test");

  const colors = await prompt.evaluate((element) => {
    const style = window.getComputedStyle(element);
    return {
      background: style.backgroundColor,
      border: style.borderTopColor,
    };
  });

  expect(colors.background).not.toBe("rgb(255, 251, 230)");
  expect(colors.border).not.toBe("rgb(255, 229, 143)");
});

test("Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读", async ({
  page,
  request,
}) => {
  const shellConfigRes = await request.get(`${apiBase}/remote-invoke/shell-config`);
  const originalShellConfig = await shellConfigRes.json();
  const updatedProfileName = uniqueName("profile-name");
  const updatedPolicyName = uniqueName("policy-name");
  let shellConfig = {
    schema_version: 1,
    version: 1,
    profiles: [
      {
        id: "readonly-profile",
        name: "Readonly Profile",
        description: "profile for readonly id regression",
        enabled: true,
        metadata: {},
      },
    ],
    policies: [
      {
        id: "readonly-policy",
        name: "Readonly Policy",
        description: "policy for readonly id regression",
        enabled: true,
        profile_id: "readonly-profile",
        metadata: {
          exec_mode: "argv_exec",
        },
      },
    ],
  };

  try {
    await page.route("**/_bifrost/api/sync/status", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          enabled: true,
          auto_sync: false,
          remote_base_url: "http://127.0.0.1:8787",
          has_session: true,
          reachable: true,
          authorized: true,
          syncing: false,
          reason: "ready",
          user: {
            user_id: "sync-user-shell",
            nickname: "Shell Tester",
            avatar: "",
            email: "shell-tester@example.test",
          },
        }),
      });
    });
    await page.route("**/_bifrost/api/remote-invoke/status", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          state: "connected",
          discovery_session: null,
          pending_pairings_count: 0,
          active_call_ids: [],
        }),
      });
    });
    await page.route(/\/_bifrost\/api\/remote-invoke\/shell-config(?:\?.*)?$/, async (route) => {
      if (route.request().method() === "PUT") {
        shellConfig = route.request().postDataJSON() as typeof shellConfig;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(shellConfig),
      });
    });

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Remote Invoke/ }).click({ force: true });
    await page.getByText("Shell Access", { exact: true }).scrollIntoViewIfNeeded();
    const manageAccessButton = page.getByTestId("settings-remote-invoke-manage-shell-access");
    await expect(manageAccessButton).toBeVisible();
    await manageAccessButton.click({ force: true });

    const dialog = page.getByRole("dialog", { name: /Shell access/ });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("switch").first().click();
    await expect(dialog.getByRole("heading", { name: /Execution environments/ })).toBeVisible();
    const inputIndexByValue = async (value: string) => {
      const index = await dialog.locator("input").evaluateAll((nodes, expected) =>
        nodes.findIndex((node) => (node as HTMLInputElement).value === expected),
      value);
      expect(index).toBeGreaterThanOrEqual(0);
      return index;
    };
    const profileIdInput = dialog.locator("input").nth(await inputIndexByValue("readonly-profile"));
    const policyIdInput = dialog.locator("input").nth(await inputIndexByValue("readonly-policy"));
    await expect
      .poll(() => profileIdInput.evaluate((node) => (node as HTMLInputElement).readOnly))
      .toBeTruthy();
    await expect
      .poll(() => policyIdInput.evaluate((node) => (node as HTMLInputElement).readOnly))
      .toBeTruthy();

    await dialog.locator("input").nth(await inputIndexByValue("Readonly Profile")).fill(updatedProfileName);
    await dialog.locator("input").nth(await inputIndexByValue("Readonly Policy")).fill(updatedPolicyName);
    await dialog.getByRole("button", { name: "Save" }).click();
    await waitForToast(page, "Shell access config saved");

    await expect
      .poll(async () => {
        return shellConfig;
      })
      .toMatchObject({
        profiles: [
          {
            id: "readonly-profile",
            name: updatedProfileName,
          },
        ],
        policies: [
          {
            id: "readonly-policy",
            name: updatedPolicyName,
            profile_id: "readonly-profile",
          },
        ],
      });
  } finally {
    await request.put(`${apiBase}/remote-invoke/shell-config`, {
      data: originalShellConfig,
    });
  }
});

test("Settings Remote Invoke File Access 从 grant 行配置并绑定已连接 grant", async ({
  page,
}) => {
  let savedConfig: unknown = null;
  await page.route("**/_bifrost/api/remote-invoke/grants", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        grants: [
          {
            grant_id: "grant-connected-1",
            client_instance_id: "client-1",
            caller_fingerprint: "abcdef1234567890",
            caller_display_name: "mira",
            grant_mode: "permanent",
            grant_scope: "remote_query",
            status: "active",
            created_at: Date.now(),
            first_authorized_at: Date.now(),
            expires_at: null,
            last_used_at: null,
            max_calls: 0,
            remaining_calls: 0,
            use_count: 0,
            file_access: "read",
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/remote-invoke/file-access-config", async (route) => {
    if (route.request().method() === "PUT") {
      savedConfig = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(savedConfig),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ grant: [] }),
    });
  });

  await openPage(page, "settings");
  await page.getByText("Remote Invoke", { exact: true }).click();
  await expect(page.getByRole("button", { name: "Manage Policies" })).toHaveCount(0);
  await page.locator('button:has-text("File Access")').first().dispatchEvent("click");

  const dialog = page.getByRole("dialog", { name: /File Access: mira/ });
  await expect(dialog.locator('input[value="grant-connected-1"]')).toBeDisabled();
  await dialog.getByText("Read Write", { exact: true }).click();
  await dialog.getByText("All", { exact: true }).click();
  await dialog.getByRole("button", { name: "Save" }).click();

  await expect.poll(() => savedConfig).toMatchObject({
    grant: [
      {
        grant_id: "grant-connected-1",
        roots: ["/"],
      },
    ],
  });
});

test("Settings Remote Invoke File Access 继承 SSH key 默认 All Directories 策略", async ({
  page,
}) => {
  const sshFingerprint = "SHA256:ssh-key-fingerprint";
  let savedConfig: unknown = null;

  await page.route("**/_bifrost/api/remote-invoke/grants", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        grants: [
          {
            grant_id: "grant-ssh-1",
            client_instance_id: "client-1",
            caller_fingerprint: "sshcallerabcdef123456",
            caller_display_name: "ssh caller",
            auth_method: "ssh_publickey",
            ssh_key_fingerprint: sshFingerprint,
            grant_mode: "permanent",
            grant_scope: "remote_shell_exec",
            status: "active",
            created_at: Date.now(),
            first_authorized_at: Date.now(),
            expires_at: null,
            last_used_at: null,
            max_calls: 0,
            remaining_calls: 0,
            use_count: 0,
            file_access: "read_write",
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/remote-invoke/file-access-config", async (route) => {
    if (route.request().method() === "PUT") {
      savedConfig = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(savedConfig),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        grant: [
          {
            match: { ssh_fingerprint: sshFingerprint },
            name: "ssh-key:agent-default",
            roots: ["/"],
            ops: [
              "read",
              "list",
              "stat",
              "glob",
              "search",
              "hash",
              "write",
              "edit",
              "mkdir",
              "move",
              "delete",
              "apply_patch",
            ],
            allow_overwrite: true,
            allow_recursive_delete: false,
          },
        ],
      }),
    });
  });

  await openPage(page, "settings");
  await page.getByText("Remote Invoke", { exact: true }).click();
  await page.locator('button:has-text("File Access")').first().dispatchEvent("click");

  const dialog = page.getByRole("dialog", { name: /File Access: ssh caller/ });
  await expect(dialog.getByText("Read Write", { exact: true }).locator("..")).toHaveClass(
    /checked/,
  );
  await expect(dialog.getByText("All", { exact: true }).locator("..")).toHaveClass(/checked/);
  await expect(dialog.getByTestId("file-access-roots-input")).toHaveCount(0);

  await dialog.getByRole("button", { name: "Save" }).click();

  await expect.poll(() => savedConfig).toMatchObject({
    grant: [
      {
        match: { ssh_fingerprint: sshFingerprint },
        roots: ["/"],
      },
      {
        grant_id: "grant-ssh-1",
        roots: ["/"],
      },
    ],
  });
});

test("Settings Sync 支持登录、同步、更新覆盖与断网重连", async ({
  page,
  request,
}) => {
  await resetAccessControl(request);
  const remoteName = uniqueName("remote-rule");
  const remoteServer = await startMockSyncServer([
    {
      id: uniqueName("remote-id"),
      user_id: "ui-sync-user",
      name: remoteName,
      rule: "remote.example.com host://127.0.0.1:3010",
      create_time: "2026-03-20T09:00:00Z",
      update_time: "2026-03-20T09:00:00Z",
    },
  ], undefined, { responseDelayMs: 1200 });

  try {
    await request.post(`${apiBase}/sync/logout`).catch(() => undefined);
    await request.put(`${apiBase}/sync/config`, {
      data: {
        enabled: true,
        auto_sync: true,
        remote_base_url: remoteServer.baseUrl,
        probe_interval_secs: 2,
        connect_timeout_ms: 1000,
      },
    });

    const localRuleName = uniqueName("local-rule");
    await request.post(`${apiBase}/rules`, {
      data: {
        name: localRuleName,
        content: "local.example.com host://127.0.0.1:3000",
      },
    });

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Sync/ }).click({ force: true });
    await expect
      .poll(async () => {
        const value = await page.getByTestId("statusbar-sync").getAttribute("data-sync-state");
        return value === "unauthorized" || value === "unreachable";
      })
      .toBe(true);
    const loginUrlResponse = await request.get(
      `${apiBase}/sync/login-url?callback_url=${encodeURIComponent(
        `http://127.0.0.1:${backendPort}/login.html`,
      )}`,
    );
    const { login_url: loginUrl } = (await loginUrlResponse.json()) as {
      login_url: string;
    };
    await page.goto(loginUrl);

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as {
          authorized: boolean;
          reachable: boolean;
          user?: { user_id: string };
        };
        return body.authorized && body.reachable && body.user?.user_id;
      })
      .toBe("ui-sync-user");

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Sync/ }).click({ force: true });

    await expect
      .poll(async () => {
        const value = await page.getByTestId("statusbar-sync").getAttribute("data-sync-state");
        return value === "connected" || value === "ready" || value === "syncing";
      })
      .toBe(true);

    const syncingRuleName = uniqueName("syncing-rule");
    await request.post(`${apiBase}/rules`, {
      data: {
        name: syncingRuleName,
        content: "syncing.example.com host://127.0.0.1:3333",
      },
    });

    await expect
      .poll(
        async () =>
          remoteServer.listEnvs().find((env) => env.name === syncingRuleName)?.rule || "",
        { timeout: 10000 },
      )
      .toContain("127.0.0.1:3333");

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as {
          last_sync_action?: string | null;
          authorized?: boolean;
          reachable?: boolean;
        };
        return Boolean(body.authorized && body.reachable && body.last_sync_action);
      })
      .toBe(true);

    await expect
      .poll(
        async () =>
          remoteServer.listEnvs().find((env) => env.name === localRuleName)?.rule || "",
        { timeout: 10000 },
      )
      .toContain("127.0.0.1:3000");

    await expect
      .poll(async () => remoteServer.listEnvs().some((env) => env.name === localRuleName))
      .toBe(true);

    const localRuleRes = await request.get(`${apiBase}/rules/${encodeURIComponent(remoteName)}`);
    expect(localRuleRes.ok()).toBeTruthy();
    const importedRemoteRule = (await localRuleRes.json()) as { enabled: boolean; content: string };
    expect(importedRemoteRule.enabled).toBe(false);

    const existingRemote = remoteServer.listEnvs().find((env) => env.name === localRuleName);
    expect(existingRemote).toBeTruthy();
    remoteServer.upsertEnv({
      ...existingRemote!,
      rule: "local.example.com host://127.0.0.1:3100",
      update_time: "2026-03-20T12:00:00Z",
    });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`);
        const body = (await response.json()) as { content: string; enabled: boolean };
        return body;
      }, { timeout: 10000 })
      .toMatchObject({
        content: expect.stringContaining("127.0.0.1:3100"),
        enabled: true,
      });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { last_sync_action?: string | null };
        return body.last_sync_action ?? null;
      })
      .toBe("remote_pulled");
    await request.put(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`, {
      data: {
        enabled: false,
      },
    });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`);
        const body = (await response.json()) as { enabled: boolean };
        return body.enabled;
      })
      .toBe(false);

    const remoteOverwriteTime = new Date(Date.now() + 60_000).toISOString();
    remoteServer.upsertEnv({
      ...existingRemote!,
      rule: "local.example.com host://127.0.0.1:3150",
      update_time: remoteOverwriteTime,
    });
    await request.post(`${apiBase}/sync/run`);

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`);
        const body = (await response.json()) as { content: string; enabled: boolean };
        return body;
      }, { timeout: 10000 })
      .toMatchObject({
        content: expect.stringContaining("127.0.0.1:3150"),
        enabled: true,
      });

    await page.waitForTimeout(1500);

    await request.put(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`, {
      data: {
        content: "local.example.com host://127.0.0.1:3200",
      },
    });

    await expect
      .poll(
        async () =>
          remoteServer
            .listEnvs()
            .find((env) => env.name === localRuleName)
            ?.rule || "",
        { timeout: 10000 },
      )
      .toContain("127.0.0.1:3200");

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { last_sync_action?: string | null };
        return body.last_sync_action ?? null;
      })
      .toBe("local_pushed");
    await request.put(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`, {
      data: {
        enabled: true,
      },
    });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`);
        const body = (await response.json()) as { enabled: boolean };
        return body.enabled;
      })
      .toBe(true);

    await request.put(`${apiBase}/sync/config`, {
      data: {
        enabled: true,
        auto_sync: true,
        remote_base_url: "http://127.0.0.1:9",
        probe_interval_secs: 2,
        connect_timeout_ms: 1000,
      },
    });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { reason: string };
        return body.reason;
      }, { timeout: 10000 })
      .toBe("unreachable");

    await expect
      .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"))
      .toBe("unreachable");

    await request.put(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`, {
      data: {
        content: "local.example.com host://127.0.0.1:3250",
      },
    });

    const remoteBeforeReconnect = remoteServer
      .listEnvs()
      .find((env) => env.name === localRuleName);
    expect(remoteBeforeReconnect).toBeTruthy();
    remoteServer.upsertEnv({
      ...remoteBeforeReconnect!,
      update_time: "2026-03-20T00:00:00Z",
    });

    await request.put(`${apiBase}/sync/config`, {
      data: {
        enabled: true,
        auto_sync: true,
        remote_base_url: remoteServer.baseUrl,
        probe_interval_secs: 2,
        connect_timeout_ms: 1000,
      },
    });

    await expect
      .poll(
        async () =>
          remoteServer
            .listEnvs()
            .find((env) => env.name === localRuleName)
            ?.rule || "",
        { timeout: 10000 },
      )
      .toContain("127.0.0.1:3250");

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/sync/status`);
        const body = (await response.json()) as { last_sync_action?: string | null };
        return body.last_sync_action ?? null;
      })
      .toBe("local_pushed");

  } finally {
    try {
      await request.put(`${apiBase}/sync/config`, {
        data: {
          enabled: false,
          remote_base_url: "https://bifrost.bytedance.net",
        },
      });
    } catch {
      // Ignore cleanup errors when the test intentionally stops the mock remote.
    }
    await remoteServer.close();
  }
});
