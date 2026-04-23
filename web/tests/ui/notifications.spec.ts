import { expect, test } from "@playwright/test";
import { openPage, seedNotifications } from "./helpers/admin-helpers";

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

test("Notifications tables default to unread filter and keep pagination size fixed", async ({
  page,
}) => {
  await seedNotifications(buildSeedNotifications());

  await openPage(page, "notifications?tab=all");
  const activePane = page.locator(".ant-tabs-tabpane-active");

  await expect(activePane.getByText("TLS unread marker")).toBeVisible();
  await expect(activePane.getByText("Authorization unread marker")).toBeVisible();
  await expect(activePane.getByText("TLS read marker")).toHaveCount(0);
  await expect(activePane.getByText("Authorization read marker")).toHaveCount(0);
  await expect(activePane.locator(".ant-pagination-options")).toHaveCount(0);

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
