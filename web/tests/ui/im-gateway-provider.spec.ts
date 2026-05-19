import { test, expect } from "@playwright/test";
import { apiBase, openPage, uniqueName, waitForToast } from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test("IM Gateway Provider 创建后会立即启动长连接", async ({ page, request }) => {
  const providerId = uniqueName("im-provider-auto-connect");
  let connectCalls = 0;

  await page.route(`**/im-gateway/providers/${providerId}/connect`, async (route) => {
    connectCalls += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ success: true, message: "Connection started" }),
    });
  });

  try {
    await openPage(page, "ai?aiSection=im-gateway-connections&imGatewaySection=connections");
    await page.getByRole("button", { name: /Add Provider/i }).click();
    await page.getByLabel("Provider ID").fill(providerId);
    await page.getByRole("combobox", { name: /Type/ }).click();
    await page.getByTitle("Feishu").click();
    await page.getByLabel("App ID").fill("cli_test_provider_id");
    await page.getByLabel("App Secret").fill("sk_test_provider_secret");
    await page.getByRole("button", { name: "Create" }).click();
    await waitForToast(page, "Provider created and connected");

    expect(connectCalls).toBe(1);
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
    await page.getByLabel("Provider ID").fill(providerId);
    await page.getByRole("combobox", { name: /Type/ }).click();
    await page.getByTitle("Feishu").click();
    await page.getByLabel("App ID").fill(appId);
    await page.getByLabel("App Secret").fill(appSecret);
    await page.getByRole("button", { name: "Create" }).click();
    await waitForToast(page, `provider with id '${providerId}' already exists`);
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
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
