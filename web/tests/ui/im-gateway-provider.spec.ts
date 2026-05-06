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
    await openPage(page, "settings?tab=im-gateway");
    await page.getByRole("button", { name: /Add Provider/i }).click();
    await page.getByLabel("Provider ID").fill(providerId);
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

    await openPage(page, "settings?tab=im-gateway");
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
    await page.getByLabel("App ID").fill(appId);
    await page.getByLabel("App Secret").fill(appSecret);
    await page.getByRole("button", { name: "Create" }).click();
    await waitForToast(page, `provider with id '${providerId}' already exists`);
  } finally {
    await request.delete(`${apiBase}/im-gateway/providers/${providerId}`);
  }
});
