import { test, expect } from "@playwright/test";
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

test("Settings 代理与证书卡片会反映 system proxy、cli proxy、下载与二维码真实状态", async ({
  page,
  request,
}) => {
  const systemProxyRes = await request.get(`${apiBase}/proxy/system`);
  const systemProxy = (await systemProxyRes.json()) as {
    supported: boolean;
    enabled: boolean;
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
      String(systemProxy.enabled),
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

  const proxyQrSrc = await page.getByTestId("settings-proxy-qrcode").getAttribute("src");
  expect(proxyQrSrc).toContain("/_bifrost/public/proxy/qrcode");
  if (proxyAddressInfo.addresses && proxyAddressInfo.addresses.length > 0) {
    expect(proxyQrSrc).toContain(encodeURIComponent(proxyAddressInfo.addresses[0].ip));
  }

  await page.getByRole("tab", { name: /Certificate/ }).click();
  await expect(page.getByTestId("settings-certificate-tab")).toBeVisible();

  const downloadButton = page.getByTestId("settings-certificate-download");
  await expect(downloadButton).toBeVisible();

  if (certInfo.available) {
    const href = await downloadButton.getAttribute("href");
    if (!href) {
      throw new Error("Expected certificate download href to be present");
    }
    const downloadResponse = await request.get(new URL(href, page.url()).toString());
    expect(downloadResponse.ok()).toBeTruthy();
    const certQrSrc = await page.getByTestId("settings-certificate-qrcode").getAttribute("src");
    expect(certQrSrc).toContain("/_bifrost/public/cert/qrcode");
  } else {
    await expect(downloadButton).toHaveClass(/ant-btn-disabled/);
    await expect(page.locator("body")).toContainText("QR code not available");
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
  ], undefined, { responseDelayMs: 2500 });

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
      .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"))
      .toBe("unauthorized");
    await expect(page.getByTestId("settings-sync-last-action")).toHaveText("No sync result yet");

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
      .poll(async () => page.getByTestId("statusbar-sync").getAttribute("data-sync-state"))
      .toBe("unauthorized");
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
        const body = (await response.json()) as { last_sync_action?: string | null };
        return body.last_sync_action ?? null;
      })
      .toBe("bidirectional");
    await expect(page.getByTestId("settings-sync-last-action")).toHaveText(
      "Local and remote changes exchanged",
    );

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
    await expect(page.getByTestId("settings-sync-last-action")).toHaveText(
      "Newer remote changes pulled into local",
    );

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

    const remoteOverwriteTime = new Date(Date.now() + 1000).toISOString();
    remoteServer.upsertEnv({
      ...existingRemote!,
      rule: "local.example.com host://127.0.0.1:3150",
      update_time: remoteOverwriteTime,
    });

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/rules/${encodeURIComponent(localRuleName)}`);
        const body = (await response.json()) as { content: string; enabled: boolean };
        return body;
      }, { timeout: 10000 })
      .toMatchObject({
        content: expect.stringContaining("127.0.0.1:3150"),
        enabled: false,
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
    await expect(page.getByTestId("settings-sync-last-action")).toHaveText(
      "Local changes pushed to remote",
    );

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
