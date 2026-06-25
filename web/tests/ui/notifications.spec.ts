import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { apiBase, openPage, seedNotifications } from "./helpers/admin-helpers";

function buildSeedNotifications() {
  const now = Math.floor(Date.now() / 1000);
  return [
    ...Array.from({ length: 21 }, (_, index) => ({
      notificationType: index % 2 === 0 ? "tls_trust_change" : "pending_authorization",
      title: `Unread filler ${index + 1}`,
      message: `Unread filler message ${index + 1}`,
      metadata:
        index % 2 === 0 ? JSON.stringify({ domain: `filler-${index + 1}.example.com` }) : null,
      status: "unread",
      createdAt: now - 100 - index,
      updatedAt: now - 100 - index,
    })),
    {
      notificationType: "tls_trust_change",
      title: "TLS unread marker",
      message: "Unread TLS notification",
      metadata: JSON.stringify({ domain: "tls-unread.example.com" }),
      status: "unread",
      createdAt: now,
      updatedAt: now,
    },
    {
      notificationType: "tls_trust_change",
      title: "TLS read marker",
      message: "Read TLS notification",
      metadata: JSON.stringify({ domain: "tls-read.example.com" }),
      status: "read",
      actionTaken: "passthrough",
      createdAt: now - 1,
      updatedAt: now - 1,
    },
    {
      notificationType: "pending_authorization",
      title: "Authorization unread marker",
      message: "Unread authorization notification",
      status: "unread",
      createdAt: now - 2,
      updatedAt: now - 2,
    },
    {
      notificationType: "pending_authorization",
      title: "Authorization read marker",
      message: "Read authorization notification",
      status: "read",
      actionTaken: "dismissed",
      createdAt: now - 3,
      updatedAt: now - 3,
    },
  ];
}

async function createAvailabilityCheckNotification(request: APIRequestContext) {
  const sessionResponse = await request.post(`${apiBase}/trust-probe/sessions`, {
    data: {
      host: "127.0.0.1",
      ttlSeconds: 600,
    },
  });
  expect(sessionResponse.ok()).toBeTruthy();
  const session = (await sessionResponse.json()) as {
    sessionId: string;
  };
  const reportResponse = await request.post(
    `${apiBase.replace(/\/api$/, "")}/public/trust-probe/${encodeURIComponent(session.sessionId)}/report`,
    {
      data: {
        type: "page_opened",
        deviceId: `ui-test-device-${Date.now()}`,
        platformHint: "iPhone",
        message: "UI test device opened the availability check page.",
      },
    },
  );
  expect(reportResponse.ok()).toBeTruthy();
}

async function dismissConnectedDeviceModal(page: Page, timeoutMs = 5000) {
  const notNowButton = page.getByRole("button", { name: "Not now" });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await notNowButton.isVisible({ timeout: 500 }).catch(() => false)) {
      await notNowButton.click();
      await expect(notNowButton).toBeHidden();
      return;
    }
    await page.waitForTimeout(250);
  }
}

test("Notifications tables default to unread filter and keep pagination size fixed", async ({
  page,
}) => {
  await seedNotifications(buildSeedNotifications());

  await openPage(page, "notifications?tab=all");
  await dismissConnectedDeviceModal(page);
  const activePane = page.locator(".ant-tabs-tabpane-active");

  await expect(activePane.getByText("TLS unread marker")).toBeVisible();
  await expect(activePane.getByText("Authorization unread marker")).toBeVisible();
  await expect(activePane.getByText("TLS read marker")).toHaveCount(0);
  await expect(activePane.getByText("Authorization read marker")).toHaveCount(0);
  await expect(activePane.locator(".ant-pagination-options")).toHaveCount(0);
  await dismissConnectedDeviceModal(page, 15000);

  const allFilter = page.getByTestId("notifications-status-filter-all");
  await allFilter.getByText("All", { exact: true }).click();
  await expect(activePane.getByText("TLS read marker")).toBeVisible();
  await expect(activePane.getByText("Authorization read marker")).toBeVisible();

  await allFilter.getByText("Read", { exact: true }).click();
  await expect(activePane.getByText("TLS read marker")).toBeVisible();
  await expect(activePane.getByText("Authorization read marker")).toBeVisible();
  await expect(activePane.getByText("TLS unread marker")).toHaveCount(0);
  await expect(activePane.getByText("Authorization unread marker")).toHaveCount(0);

  await page.getByRole("tab", { name: /TLS Trust/ }).click();
  await expect(activePane.getByText("TLS unread marker")).toBeVisible();
  await expect(activePane.getByText("TLS read marker")).toHaveCount(0);

  const tlsFilter = page.getByTestId("notifications-status-filter-tls_trust_change");
  await tlsFilter.getByText("All", { exact: true }).click();
  await expect(activePane.getByText("TLS read marker")).toBeVisible();

  await page.getByRole("tab", { name: /Authorization/ }).click();
  await expect(activePane.getByText("Authorization unread marker")).toBeVisible();
  await expect(activePane.getByText("Authorization read marker")).toHaveCount(0);
  await expect(activePane.locator(".ant-pagination-options")).toHaveCount(0);
});

test("Availability check notification bubble sits lower, animates, drags, and opens", async ({
  page,
  request,
}) => {
  await openPage(page, "traffic");
  await dismissConnectedDeviceModal(page);
  await page.evaluate(() => {
    window.localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "light" }, version: 0 }),
    );
  });
  await page.reload();
  await dismissConnectedDeviceModal(page, 15000);
  await createAvailabilityCheckNotification(request);
  await dismissConnectedDeviceModal(page, 15000);

  const center = page.getByTestId("availability-check-notification-center");
  const bubble = page.getByTestId("availability-check-notification-bubble");
  await expect(center).toBeVisible();
  await expect(center).toHaveClass(/availability-check-notification-center--alerting/);
  await expect(bubble).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  const initialBox = await center.boundingBox();
  expect(initialBox).not.toBeNull();
  expect(initialBox!.y).toBeGreaterThanOrEqual(64);
  expect(initialBox!.x + initialBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width);

  await page.evaluate(() => {
    window.localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "dark" }, version: 0 }),
    );
  });
  await page.reload();
  await dismissConnectedDeviceModal(page, 15000);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(center).toBeVisible();
  await expect(center).toHaveClass(/availability-check-notification-center--alerting/);
  await dismissConnectedDeviceModal(page, 15000);

  const darkBox = await center.boundingBox();
  expect(darkBox).not.toBeNull();

  await page.mouse.move(darkBox!.x + darkBox!.width / 2, darkBox!.y + darkBox!.height / 2);
  await page.mouse.down();
  await page.mouse.move(darkBox!.x - 120, darkBox!.y + 120, { steps: 8 });
  await page.mouse.up();

  const draggedBox = await center.boundingBox();
  expect(draggedBox).not.toBeNull();
  expect(draggedBox!.x).toBeLessThan(darkBox!.x - 40);
  expect(draggedBox!.y).toBeGreaterThan(darkBox!.y + 60);
  expect(draggedBox!.x).toBeGreaterThanOrEqual(18);
  expect(draggedBox!.y).toBeGreaterThanOrEqual(18);

  await page.waitForTimeout(50);
  await bubble.click();
  await expect(page.getByText("Availability Check", { exact: true })).toBeVisible();
  await expect(page.getByTestId("availability-check-notification-item")).toContainText(/iphone/i);
});
