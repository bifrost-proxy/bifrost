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

  await openPage(page, "settings?tab=agent");

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

test("Settings Agent 左侧导航按 URL 切换独立编辑卡片", async ({ page }) => {
  const agentConfig = {
    enabled: true,
    work_dir: "/tmp/agent-ui",
    base_instructions: "Base prompt",
    developer_instructions: "Developer instructions",
    user_instructions: "User instructions",
    default_base_instructions: "Default base prompt",
    model: "gpt-test",
    model_provider: "mock",
    max_completion_tokens: 4096,
    shell_timeout_secs: 60,
    max_history_messages: 20,
    model_providers: {
      mock: {
        api_key: "$MODEL_API_KEY",
        request_max_retries: 1,
        stream_idle_timeout_ms: 30000,
        stream_max_retries: 2,
      },
    },
    memories: {
      generate_memories: true,
      use_memories: true,
      disable_on_external_context: false,
      max_raw_memories_for_consolidation: 100,
      max_unused_days: 90,
      max_rollout_age_days: 30,
    },
    mcp_servers: {
      filesystem: {
        enabled: true,
        transport: "stdio",
        command: "mock-mcp",
      },
    },
  };

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
      body: JSON.stringify([
        {
          id: "mock",
          name: "Mock Provider",
          base_url: "https://model.example.test",
          env_key: "MODEL_API_KEY",
        },
      ]),
    });
  });

  await openPage(page, "settings?tab=agent");

  const nav = page.getByTestId("agent-settings-section-nav");
  await expect(nav).toBeVisible();
  await expect(page.getByTestId("agent-settings-nav-general")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-settings-section-general")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toHaveCount(0);

  await page.getByTestId("agent-settings-nav-mcp-servers").click();
  await expect(page.getByTestId("agent-settings-nav-mcp-servers")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page).toHaveURL(/agentSection=mcp-servers/);
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-general")).toHaveCount(0);

  await page.reload();
  await expect(page.getByTestId("agent-settings-nav-mcp-servers")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-general")).toHaveCount(0);

  await page.getByTestId("agent-settings-nav-runtime").click();
  await expect(page.getByTestId("agent-settings-nav-runtime")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page).toHaveURL(/agentSection=runtime/);
  await expect(page.getByTestId("agent-settings-section-runtime")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toHaveCount(0);

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.getByTestId("agent-settings-nav-mcp-servers").click();
  await expect(page).toHaveURL(/agentSection=mcp-servers/);
  await expect(page.getByTestId("agent-settings-nav-mcp-servers")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("agent-settings-section-mcp-servers")).toBeVisible();
  await expect(page.getByTestId("agent-settings-section-runtime")).toHaveCount(0);
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

  await openPage(page, "settings?tab=im-gateway");
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

test("Settings IM Gateway 左侧导航按 URL 切换独立面板", async ({ page }) => {
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

  await openPage(page, "settings?tab=im-gateway");

  await expect(page.getByTestId("im-gateway-section-nav")).toBeVisible();
  await expect(page.getByTestId("im-gateway-nav-connections")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-connections")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /Connections/ })).toHaveCount(0);

  await page.getByTestId("im-gateway-nav-routes").click();
  await expect(page).toHaveURL(/imGatewaySection=routes/);
  await expect(page.getByTestId("im-gateway-nav-routes")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-connections")).toHaveCount(0);

  await page.reload();
  await expect(page.getByTestId("im-gateway-nav-routes")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("im-gateway-section-routes")).toBeVisible();

  await page.getByTestId("im-gateway-nav-history").click();
  await expect(page).toHaveURL(/imGatewaySection=history/);
  await expect(page.getByTestId("im-gateway-section-history")).toBeVisible();
  await expect(page.getByTestId("im-gateway-section-routes")).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /Events/ })).toBeVisible();

  await page.getByTestId("theme-toggle").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.getByTestId("im-gateway-nav-targets").click();
  await expect(page).toHaveURL(/imGatewaySection=targets/);
  await expect(page.getByTestId("im-gateway-nav-targets")).toHaveAttribute(
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
  await page.route("**/_bifrost/api/remote-invoke/calls", async (route) => {
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
  await expect(page.getByText("Recent Calls")).toBeVisible();
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

  try {
    await request.put(`${apiBase}/remote-invoke/shell-config`, {
      data: {
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
      },
    });

    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Remote Invoke/ }).click({ force: true });
    await page.getByRole("button", { name: "Manage Access" }).click();

    const dialog = page.getByRole("dialog", { name: "Manage Shell Access" });
    await expect(dialog.locator('input[value="readonly-profile"]')).toHaveAttribute(
      "readonly",
      "",
    );
    await expect(dialog.locator('input[value="readonly-policy"]')).toHaveAttribute(
      "readonly",
      "",
    );

    await dialog.locator('input[value="Readonly Profile"]').fill(updatedProfileName);
    await dialog.locator('input[value="Readonly Policy"]').fill(updatedPolicyName);
    await dialog.getByRole("button", { name: "Save" }).click();
    await waitForToast(page, "Shell access config saved");

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/remote-invoke/shell-config`);
        return response.json();
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
