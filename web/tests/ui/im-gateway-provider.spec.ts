import { test, expect } from "@playwright/test";
import { apiBase, openPage, uniqueName, waitForToast } from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test("IM Gateway Provider 创建后会立即启动长连接", async ({ page, request }) => {
  const providerId = uniqueName("im-provider-auto-connect");
  let connectCalls = 0;

  await page.route("**/im-gateway/providers/feishu-setup/start", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        success: true,
        session_id: "setup-session-1",
        verification_url: "https://open.feishu.cn/page/launcher?user_code=123",
        expires_at: Date.now() + 300_000,
        expires_in_seconds: 300,
        interval_seconds: 2,
        brand: "feishu",
      }),
    });
  });
  await page.route("**/im-gateway/providers/feishu-setup/setup-session-1/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        success: true,
        status: "confirmed",
        app_id: "cli_test_provider_id",
        secret_configured: true,
        owner_open_id: "ou_setup_owner",
        brand: "feishu",
        base_url: "https://open.feishu.cn/open-apis",
      }),
    });
  });
  await page.route("**/im-gateway/providers/feishu-setup/setup-session-1/provider", async (route) => {
    const requestBody = route.request().postDataJSON() as { id: string; display_name?: string };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        success: true,
        provider: {
          id: requestBody.id,
          provider_type: "feishu",
          display_name: requestBody.display_name || requestBody.id,
          enabled: true,
          base_url: "https://open.feishu.cn/open-apis",
          app_id: "cli_test_provider_id",
          secret_configured: true,
          owner_open_id: "ou_setup_owner",
          event_connection_enabled: true,
          event_types: ["message.receive"],
          created_at: Date.now(),
          updated_at: Date.now(),
        },
      }),
    });
  });
  await page.route(`**/im-gateway/providers/${providerId}`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: providerId,
        provider_type: "feishu",
        display_name: providerId,
        enabled: true,
        base_url: "https://open.feishu.cn/open-apis",
        app_id: "cli_test_provider_id",
        secret_configured: true,
        owner_open_id: "ou_setup_owner",
        event_connection_enabled: true,
        event_types: ["message.receive"],
        created_at: Date.now(),
        updated_at: Date.now(),
      }),
    });
  });
  await page.route(`**/im-gateway/providers/${providerId}/connect`, async (route) => {
    connectCalls += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ success: true, message: "Connection started" }),
    });
  });

  try {
    await page.addInitScript(() => {
      window.localStorage.setItem(
        "bifrost-theme",
        JSON.stringify({ state: { mode: "dark" }, version: 0 }),
      );
    });
    await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await page.getByRole("button", { name: /Add Provider/i }).click();
    const dialog = page.getByRole("dialog").last();
    await expect(dialog.getByText(/Choose the IM platform first/)).toBeVisible();
    await expect(dialog.getByLabel("App ID")).toHaveCount(0);
    await expect(dialog.getByLabel("App Secret")).toHaveCount(0);
    await expect(dialog.getByText("Agent Runner")).toHaveCount(0);
    await expect(dialog.getByText("Agent Working Directory")).toHaveCount(0);
    await page.getByText("Feishu", { exact: true }).click();
    await page.getByRole("button", { name: "Next" }).click();
    await page.getByLabel("Provider ID").fill(providerId);
    await expect(page.getByText("https://open.feishu.cn/page/launcher?user_code=123")).toBeVisible();
    await expect(dialog.getByLabel("App ID")).toHaveCount(0);
    await expect(dialog.getByLabel("App Secret")).toHaveCount(0);
    await expect(page.getByText("App created. Bifrost has the App ID and Secret on the server.")).toBeVisible();
    await expect(dialog.getByText("Open Setup Page")).toBeVisible();
    await page.getByRole("button", { name: "Connect", exact: true }).click();
    await waitForToast(page, "Feishu provider connected");
    await expect(dialog.getByText("Connection")).toBeVisible();
    await expect(dialog.getByText("Agent Runner", { exact: true })).toBeVisible();

    expect(connectCalls).toBe(1);
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
});

test("IM Gateway Feishu setup 启动失败时显示可重试错误状态", async ({ page }) => {
  let startCalls = 0;

  await page.route("**/im-gateway/providers/feishu-setup/start", async (route) => {
    startCalls += 1;
    if (startCalls === 1) {
      await route.fulfill({
        status: 502,
        contentType: "application/json",
        body: JSON.stringify({
          status: 502,
          error: "Failed to start Feishu setup: proxy loop blocked",
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        success: true,
        session_id: "setup-session-retry",
        verification_url: "https://open.feishu.cn/page/launcher?user_code=456",
        expires_at: Date.now() + 300_000,
        expires_in_seconds: 300,
        interval_seconds: 5,
        brand: "feishu",
      }),
    });
  });

  await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
  await page.getByRole("button", { name: /Add Provider/i }).click();
  await page.getByText("Feishu", { exact: true }).click();
  await page.getByRole("button", { name: "Next" }).click();
  const dialog = page.getByRole("dialog", { name: "Add IM Provider" });
  await expect(dialog.getByText("Failed to start Feishu setup.")).toBeVisible();
  await expect(dialog.getByText("Failed to start Feishu setup: proxy loop blocked")).toBeVisible();
  await dialog.getByRole("button", { name: "Retry Setup" }).click();
  await expect(page.getByText("https://open.feishu.cn/page/launcher?user_code=456")).toBeVisible();
});

test("IM Gateway Weixin Provider 创建后立即弹出扫码二维码", async ({ page, request }) => {
  const providerId = uniqueName("im-provider-weixin-qr");

  await page.route("**/im-gateway/providers", async (route) => {
    if (route.request().method() !== "POST") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ success: true }),
    });
  });
  await page.route(`**/im-gateway/providers/${providerId}`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: providerId,
        provider_type: "weixin",
        display_name: providerId,
        enabled: true,
        base_url: "https://wx2.qq.com/cgi-bin/mmwebwx-bin",
        app_id: "weixin_qr_user",
        secret_configured: true,
        owner_open_id: "wx_owner",
        event_connection_enabled: true,
        event_types: ["message.receive"],
        created_at: Date.now(),
        updated_at: Date.now(),
      }),
    });
  });
  await page.route(`**/im-gateway/providers/${providerId}/weixin-login/start`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        success: true,
        scan_url: "https://login.weixin.qq.com/qrcode/test-weixin-qr",
        expires_in_seconds: 60,
      }),
    });
  });

  try {
    await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
    await page.getByRole("button", { name: /Add Provider/i }).click();
    const dialog = page.getByRole("dialog", { name: "Add IM Provider" });
    await expect(dialog.getByText(/Choose the IM platform first/)).toBeVisible();
    await page.getByRole("button", { name: "Next" }).click();
    await expect(dialog.getByText(/immediately show the Weixin QR code/)).toBeVisible();
    await expect(dialog.getByText("Agent Runner")).toHaveCount(0);
    await page.getByLabel("Provider ID").fill(providerId);
    await page.getByRole("button", { name: "Create and Show QR" }).click();
    await expect(page.getByRole("dialog", { name: "Scan Weixin QR" })).toBeVisible();
    await expect(page.getByText("https://login.weixin.qq.com/qrcode/test-weixin-qr")).toBeVisible();
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
});

test("IM Gateway Provider 编辑时可以补填 App Secret 并显示后端错误", async ({
  page,
  request,
}) => {
  const providerId = uniqueName("im-provider-edit-secret");
  const appId = "cli_test_provider_id";
  const appSecret = "sk_test_provider_secret";

  try {
    const seedResponse = await request.post(`${apiBase}/im-gateway/providers`, {
      data: {
        id: providerId,
        provider_type: "feishu",
        enabled: true,
        event_connection_enabled: true,
        app_id: appId,
      },
    });
    expect(seedResponse.ok()).toBeTruthy();

    await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
    await expect(page.locator("body")).toContainText(providerId);
    await expect(page.locator("body")).toContainText("Not Set");

    await page.getByTestId(`settings-im-provider-edit-${providerId}`).click();
    const editDialog = page.getByRole("dialog", { name: "Edit IM Provider" });
    const compactForm = editDialog.locator(".im-provider-edit-form-compact");
    await expect(compactForm).toBeVisible();
    await expect(compactForm.locator(".ant-form-item").first()).toHaveCSS("margin-bottom", "10px");
    await expect(compactForm.locator(".ant-form-item-label").first()).toHaveCSS("padding-bottom", "2px");
    await page.getByLabel("App Secret").fill(appSecret);
    await page.getByRole("button", { name: "Save" }).click();
    await waitForToast(page, "Provider updated");

    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/im-gateway/providers`);
        const providers = (await response.json()) as Array<{
          id: string;
          secret_configured?: boolean;
        }>;
        return providers.find((provider) => provider.id === providerId)?.secret_configured;
      })
      .toBe(true);

    await page.getByRole("button", { name: /Add Provider/i }).click();
    await page.getByRole("button", { name: "Next" }).click();
    await page.getByLabel("Provider ID").fill(providerId);
    await page.getByRole("button", { name: "Create and Show QR" }).click();
    await waitForToast(page, `provider with id '${providerId}' already exists`);
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
});

test("IM Gateway Provider 卡片展示继承后的默认工作目录", async ({ page }) => {
  const providerId = "im-provider-inherited-workdir";
  const resolvedWorkDir = "/tmp/bifrost-resolved-default-workdir";

  await page.route("**/im-gateway/agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        enabled: true,
        work_dir: null,
        resolved_work_dir: resolvedWorkDir,
        model_providers: {},
        mcp_servers: {},
      }),
    });
  });
  await page.route("**/im-gateway/providers", async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          id: providerId,
          provider_type: "feishu",
          display_name: "Inherited Workdir Provider",
          enabled: true,
          base_url: "https://open.feishu.cn/open-apis",
          app_id: "cli_inherited_workdir",
          secret_configured: true,
          owner_open_id: "ou_inherited_workdir",
          event_connection_enabled: true,
          event_types: ["message.receive"],
          created_at: Date.now(),
          updated_at: Date.now(),
        },
      ]),
    });
  });
  await page.route(`**/im-gateway/providers/${providerId}/status`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        state: "connected",
        reconnect_count: 0,
      }),
    });
  });

  await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");

  const workDirField = page.getByTestId(`settings-im-provider-${providerId}-work-dir`);
  await expect(workDirField).toBeVisible();
  await expect(workDirField).toContainText(resolvedWorkDir);
  await expect(workDirField).not.toContainText("Global default");
});

test("IM Gateway Provider 卡片单列展示并支持复制关键字段", async ({
  page,
  request,
  context,
}) => {
  const providerId = uniqueName("im-provider-copy-fields");
  const appId = "cli_copyable_provider_id";
  const ownerOpenId = "ou_copyable_provider_owner";
  const workDir = "/tmp/test-bifrost-workdir";

  await context.grantPermissions(["clipboard-read", "clipboard-write"]);

  try {
    const seedResponse = await request.post(`${apiBase}/im-gateway/providers`, {
      data: {
        id: providerId,
        provider_type: "feishu",
        display_name: "Copyable Provider",
        enabled: true,
        event_connection_enabled: true,
        app_id: appId,
        owner_open_id: ownerOpenId,
        agent_config: {
          work_dir: workDir,
        },
      },
    });
    expect(seedResponse.ok()).toBeTruthy();

    await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
    const list = page.getByTestId("settings-im-provider-list");
    await expect(list).toBeVisible();
    const listBox = await list.boundingBox();
    expect(listBox).toBeTruthy();
    expect(listBox!.width).toBeLessThanOrEqual(760);
    const centeredDelta = await list.evaluate((node) => {
      const box = node.getBoundingClientRect();
      const parentBox = node.parentElement?.getBoundingClientRect();
      if (!parentBox) return 0;
      return Math.abs(box.x - (parentBox.x + (parentBox.width - box.width) / 2));
    });
    expect(centeredDelta).toBeLessThan(8);

    const card = page.getByTestId(`settings-im-provider-card-${providerId}`);
    const header = card.locator(".ant-card-head");
    await expect(card).toBeVisible();
    await expect(header).toContainText("Copyable Provider");
    await expect(header).toContainText(/Connected|Unknown|Disconnected|Failed|connecting|reconnecting/);
    await expect(header).toContainText("Enabled");
    await expect(header).toContainText("Long Connection");

    const appIdField = page.getByTestId(`settings-im-provider-${providerId}-app-id`);
    const ownerField = page.getByTestId(`settings-im-provider-${providerId}-owner-id`);
    const workDirField = page.getByTestId(`settings-im-provider-${providerId}-work-dir`);
    await expect(appIdField).toBeVisible();
    await expect(ownerField).toBeVisible();
    await expect(workDirField).toBeVisible();

    const appIdBox = await appIdField.boundingBox();
    const ownerBox = await ownerField.boundingBox();
    const workDirBox = await workDirField.boundingBox();
    expect(appIdBox).toBeTruthy();
    expect(ownerBox).toBeTruthy();
    expect(workDirBox).toBeTruthy();
    expect(ownerBox!.y).toBeGreaterThan(appIdBox!.y);
    expect(workDirBox!.y).toBeGreaterThan(ownerBox!.y);

    const appIdCopy = page.getByTestId(`settings-im-provider-${providerId}-app-id-copy`);
    await expect(appIdCopy).toHaveCSS("opacity", "0");
    await appIdField.hover();
    await expect(appIdCopy).toHaveCSS("opacity", "1");
    await appIdCopy.click();
    await expect
      .poll(async () => page.evaluate(async () => navigator.clipboard.readText()))
      .toBe(appId);

    await ownerField.hover();
    await page.getByTestId(`settings-im-provider-${providerId}-owner-id-copy`).click();
    await expect
      .poll(async () => page.evaluate(async () => navigator.clipboard.readText()))
      .toBe(ownerOpenId);

    await workDirField.hover();
    await page.getByTestId(`settings-im-provider-${providerId}-work-dir-copy`).click();
    await expect
      .poll(async () => page.evaluate(async () => navigator.clipboard.readText()))
      .toBe(workDir);
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
});
