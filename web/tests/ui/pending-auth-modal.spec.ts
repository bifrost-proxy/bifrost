import { test, expect } from "@playwright/test";
import {
  openPage,
  resetAccessControl,
  waitForToast,
} from "./helpers/admin-helpers";

test.describe("Global Pending Auth Modal", () => {
  test.describe.configure({ mode: "serial" });

  test.beforeEach(async ({ request }) => {
    await resetAccessControl(request);
  });

  test("全局弹窗在有 pending 授权请求时显示，且可在任意页面出现", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "192.168.1.100", first_seen: Math.floor(Date.now() / 1000) - 60, attempt_count: 3 },
      { ip: "10.0.0.50", first_seen: Math.floor(Date.now() / 1000) - 120, attempt_count: 1 },
    ];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body =
        `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 2 })}\n\n` +
        `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[1], total_pending: 2 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(fakePendingList),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });
    await expect(page.locator(".ant-modal")).toContainText("Pending Authorization Requests");
    await expect(page.locator(".ant-modal")).toContainText("192.168.1.100");
    await expect(page.locator(".ant-modal")).toContainText("10.0.0.50");
  });

  test("全局弹窗 Approve 按钮可以批准 pending 请求", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "192.168.1.100", first_seen: Math.floor(Date.now() / 1000) - 60, attempt_count: 3 },
    ];
    let currentPendingList = [...fakePendingList];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body = `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 1 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending/approve", (route) => {
      const data = route.request().postDataJSON() as { ip: string };
      currentPendingList = currentPendingList.filter((p) => p.ip !== data.ip);
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ success: true, message: "Approved" }),
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(currentPendingList),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });
    await expect(page.locator(".ant-modal")).toContainText("192.168.1.100");

    await page.getByTestId("pending-auth-approve-192.168.1.100").click();
    await waitForToast(page, "Approved 192.168.1.100");
  });

  test("全局弹窗 Reject 按钮可以拒绝 pending 请求", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "10.0.0.50", first_seen: Math.floor(Date.now() / 1000) - 120, attempt_count: 1 },
    ];
    let currentPendingList = [...fakePendingList];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body = `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 1 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending/reject", (route) => {
      const body = route.request().postDataJSON();
      currentPendingList = currentPendingList.filter((p) => p.ip !== body.ip);
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ success: true, message: "Rejected" }),
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(currentPendingList),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });
    await expect(page.locator(".ant-modal")).toContainText("10.0.0.50");

    await page.getByTestId("pending-auth-reject-10.0.0.50").click();
    await waitForToast(page, "Rejected 10.0.0.50");
  });

  test("全局弹窗 Clear All 按钮可以清除所有 pending 请求", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "192.168.1.100", first_seen: Math.floor(Date.now() / 1000) - 60, attempt_count: 3 },
      { ip: "10.0.0.50", first_seen: Math.floor(Date.now() / 1000) - 120, attempt_count: 1 },
    ];
    let currentPendingList = [...fakePendingList];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body =
        `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 2 })}\n\n` +
        `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[1], total_pending: 2 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(currentPendingList),
        });
      } else if (route.request().method() === "DELETE") {
        currentPendingList = [];
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ success: true, message: "Cleared" }),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });
    await expect(page.locator(".ant-modal")).toContainText("192.168.1.100");
    await expect(page.locator(".ant-modal")).toContainText("10.0.0.50");

    await page.getByTestId("pending-auth-modal-clear-all").click();
    await page.getByRole("button", { name: "Yes" }).click();
    await waitForToast(page, "Cleared all pending authorizations");
    await expect(
      page.locator(".ant-modal").filter({ hasText: "Pending Authorization" }),
    ).not.toBeVisible();
  });

  test("全局弹窗 Clear All 失败时也会关闭，不阻塞页面", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "192.168.1.100", first_seen: Math.floor(Date.now() / 1000) - 60, attempt_count: 3 },
    ];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body = `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 1 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(fakePendingList),
        });
      } else if (route.request().method() === "DELETE") {
        route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({ error: "Cross-site admin write request rejected" }),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });

    await page.getByTestId("pending-auth-modal-clear-all").click();
    await page.getByRole("button", { name: "Yes" }).click();
    await waitForToast(page, "Failed to clear pending authorizations");
    await expect(
      page.locator(".ant-modal").filter({ hasText: "Pending Authorization" }),
    ).not.toBeVisible();
  });

  test("全局弹窗操作遇到过期 CSRF token 时会刷新 token 并重试", async ({
    page,
  }) => {
    const fakePendingList = [
      {
        ip: "192.168.1.100",
        first_seen: Math.floor(Date.now() / 1000) - 60,
        attempt_count: 3,
      },
    ];
    let currentPendingList = [...fakePendingList];
    let csrfRequestCount = 0;
    let approveRequestCount = 0;

    await page.route("**/security/csrf", (route) => {
      csrfRequestCount += 1;
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          csrf_token: csrfRequestCount === 1 ? "stale-token" : "fresh-token",
          header_name: "X-Bifrost-CSRF",
        }),
      });
    });

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body = `data: ${JSON.stringify({
        event_type: "new",
        pending_auth: fakePendingList[0],
        total_pending: 1,
      })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending/approve", (route) => {
      approveRequestCount += 1;
      if (route.request().headers()["x-bifrost-csrf"] !== "fresh-token") {
        route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({ error: "Missing or invalid admin CSRF token" }),
        });
        return;
      }
      const data = route.request().postDataJSON() as { ip: string };
      currentPendingList = currentPendingList.filter((p) => p.ip !== data.ip);
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ success: true, message: "Approved" }),
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(currentPendingList),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });

    await page.getByTestId("pending-auth-approve-192.168.1.100").click();
    await waitForToast(page, "Approved 192.168.1.100");
    await expect(
      page.locator(".ant-modal").filter({ hasText: "Pending Authorization" }),
    ).not.toBeVisible();
    expect(csrfRequestCount).toBe(2);
    expect(approveRequestCount).toBe(2);
  });

  test("全局弹窗 Settings 按钮可以导航到 Access Control 设置页", async ({
    page,
  }) => {
    const fakePendingList = [
      { ip: "192.168.1.100", first_seen: Math.floor(Date.now() / 1000) - 60, attempt_count: 3 },
    ];

    await page.route("**/whitelist/pending/stream**", (route) => {
      const body = `data: ${JSON.stringify({ event_type: "new", pending_auth: fakePendingList[0], total_pending: 1 })}\n\n`;
      route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        },
        body,
      });
    });

    await page.route("**/whitelist/pending", (route) => {
      if (route.request().method() === "GET") {
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(fakePendingList),
        });
      } else {
        route.continue();
      }
    });

    await openPage(page, "traffic");
    await expect(page.locator(".ant-modal")).toBeVisible({ timeout: 10000 });

    await page.getByTestId("pending-auth-modal-settings").click();
    await expect(page).toHaveURL(/settings.*tab=access/);
    await expect(
      page.locator(".ant-modal").filter({ hasText: "Pending Authorization" }),
    ).not.toBeVisible();
  });

  test("无 pending 请求时不显示全局弹窗", async ({
    page,
  }) => {
    await openPage(page, "traffic");
    await page.waitForTimeout(2000);
    await expect(page.locator(".ant-modal").filter({ hasText: "Pending Authorization" })).not.toBeVisible();
  });
});
