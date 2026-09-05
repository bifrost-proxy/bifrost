import { test, expect, type Page } from "@playwright/test";
import { gzipSync } from "node:zlib";
import {
  apiBase,
  clearReplay,
  clearTraffic,
  openPage,
  sendProxyRequest,
  sendProxyRequestWithResponse,
  startMockHttpServer,
  uniqueName,
  waitForTrafficRow,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ request }) => {
  await clearTraffic(request);
  await clearReplay(request);
});

async function dismissConnectedDeviceModal(page: Page) {
  await page.addLocatorHandler(
    page.getByRole("dialog").filter({
      hasText: "Install Bifrost CA profile on connected iPhone?",
    }),
    async (dialog) => {
      await dialog.getByRole("button", { name: "Not now" }).click();
    },
  );
}

test("从其他 tab 切到 Replay 时立即加载已保存请求列表", async ({
  page,
  request,
}) => {
  const requestName = uniqueName("replay-route-enter");

  const createRes = await request.post(`${apiBase}/replay/requests`, {
    data: {
      name: requestName,
      request_type: "http",
      method: "GET",
      url: "https://example.com/replay-route-enter",
      headers: [],
      body: { type: "none" },
      is_saved: true,
    },
  });
  expect(createRes.ok()).toBeTruthy();

  await openPage(page, "traffic");
  await expect(page.getByTestId("traffic-table")).toBeVisible();

  await page.getByText("Replay", { exact: true }).click();

  const requestNode = page
    .getByTestId("replay-request-node")
    .filter({ hasText: requestName })
    .first();
  await expect(requestNode).toBeVisible();
});

test("Replay 页面保存请求、创建分组、移动并执行，然后查看历史记录", async ({
  page,
  request,
}) => {
  const requestName = uniqueName("replay-request");
  const groupName = uniqueName("replay-group");
  const server = await startMockHttpServer();

  try {
    await openPage(page, "replay");
    await expect(page.getByTestId("replay-request-panel")).toBeVisible();

    await page.getByTestId("replay-url-input").fill(`http://127.0.0.1:${server.port}/replay-check`);
    await page.getByTestId("replay-save-button").click();
    const saveDialog = page.getByRole("dialog", { name: "Save Request" });
    await saveDialog.getByTestId("replay-save-name-input").fill(requestName);
    await saveDialog.getByRole("button", { name: "Save" }).click();

    await page.getByTestId("replay-new-group-button").click();
    await page.getByTestId("replay-group-name-input").fill(groupName);
    await page.getByRole("button", { name: "Create" }).click();
    await expect(page.getByText(groupName, { exact: true }).first()).toBeVisible();

    const requestsRes = await request.get(`${apiBase}/replay/requests?saved=true&limit=100`);
    expect(requestsRes.ok()).toBeTruthy();
    const requestsPayload = (await requestsRes.json()) as {
      requests: Array<{ id: string; name?: string; url: string }>;
    };
    const replayRequest = requestsPayload.requests.find(
      (item) => item.name === requestName || item.url.endsWith("/replay-check"),
    );
    expect(replayRequest?.id).toBeTruthy();

    const groupsRes = await request.get(`${apiBase}/replay/groups`);
    expect(groupsRes.ok()).toBeTruthy();
    const groupsPayload = (await groupsRes.json()) as {
      groups: Array<{ id: string; name: string }>;
    };
    const replayGroup = groupsPayload.groups.find((item) => item.name === groupName);
    expect(replayGroup?.id).toBeTruthy();
    await expect(page.locator(`[data-group-id="${replayGroup!.id}"]`)).toHaveCount(1);

    const moveRes = await request.put(
      `${apiBase}/replay/requests/${encodeURIComponent(replayRequest!.id)}/move`,
      { data: { group_id: replayGroup!.id } },
    );
    expect(moveRes.ok()).toBeTruthy();

    const requestNode = page
      .getByTestId("replay-request-node")
      .filter({ hasText: requestName })
      .first();
    await expect(requestNode).toBeVisible();

    await requestNode.click();
    await page.getByTestId("replay-send-button").click();
    await expect.poll(() => server.requests.length).toBeGreaterThan(0);

    await page.getByTestId("replay-mode-history").click();
    await expect(page.getByText("/replay-check")).toBeVisible();
    await page.getByTestId("replay-history-item").first().click();
    await expect(page.getByTestId("replay-history-reuse-button")).toBeVisible();
    await page.getByTestId("replay-history-reuse-button").click();
    await expect(page.getByTestId("replay-url-input")).toHaveValue(
      `http://127.0.0.1:${server.port}/replay-check`,
    );

    await openPage(page, "traffic");
    await waitForTrafficRow(page, "/replay-check");
  } finally {
    await server.close();
  }
});

test("从 Traffic 导入后首次保存的模板在执行后可见历史，刷新后仍能恢复", async ({
  page,
}) => {
  const requestName = uniqueName("replay-import-save");
  const server = await startMockHttpServer();
  const path = `/a-com-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;

  try {
    await openPage(page, "traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    await sendProxyRequest(`http://127.0.0.1:${server.port}${path}`);
    const trafficCountBeforeReplay = server.requests.length;

    const row = await waitForTrafficRow(page, path);
    await row.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Replay" }).click();

    await expect(page).toHaveURL(/\/_bifrost\/replay$/);
    await expect(page.getByTestId("replay-url-input")).toHaveValue(
      `http://127.0.0.1:${server.port}${path}`,
    );

    await page.getByTestId("replay-save-button").click();
    const saveDialog = page.getByRole("dialog", { name: "Save Request" });
    await saveDialog.getByTestId("replay-save-name-input").fill(requestName);
    await saveDialog.getByRole("button", { name: "Save" }).click();

    const requestNode = page
      .getByTestId("replay-request-node")
      .filter({ hasText: requestName })
      .first();
    await expect(requestNode).toBeVisible();

    await page.getByTestId("replay-send-button").click();
    await expect
      .poll(() => server.requests.length, { message: "wait for replay request to reach mock server" })
      .toBeGreaterThan(trafficCountBeforeReplay);

    await page.getByTestId("replay-mode-history").click();
    await expect(page.getByTestId("replay-history-scope")).toContainText(requestName);
    await expect(page.getByTestId("replay-history-item").filter({ hasText: path }).first()).toBeVisible();

    await page.reload();
    await expect(page.getByTestId("replay-url-input")).toHaveValue(
      `http://127.0.0.1:${server.port}${path}`,
    );

    await page.getByTestId("replay-mode-history").click();
    await expect(page.getByTestId("replay-history-scope")).toContainText(requestName);
    await expect(page.getByTestId("replay-history-item").filter({ hasText: path }).first()).toBeVisible();
  } finally {
    await server.close();
  }
});

test("从 Traffic 导入 gzip 请求后以明文正文重放且不保留压缩头", async ({ page }) => {
  const server = await startMockHttpServer();
  const path = `/compressed-replay-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
  const body = JSON.stringify({ limit: 1, source: "traffic" });

  try {
    await dismissConnectedDeviceModal(page);
    await openPage(page, "traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();

    const captureResponse = await sendProxyRequestWithResponse(
      `http://127.0.0.1:${server.port}${path}`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Content-Encoding": "gzip",
        },
        body: gzipSync(body),
      },
    );
    expect(captureResponse.status).toBe(200);

    const row = await waitForTrafficRow(page, path);
    await row.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Replay" }).click();
    await expect(page).toHaveURL(/\/_bifrost\/replay$/);

    const requestCountBeforeReplay = server.requests.length;
    await page.getByTestId("replay-send-button").click();
    await expect.poll(() => server.requests.length).toBeGreaterThan(requestCountBeforeReplay);

    const replayed = server.requests.at(-1);
    expect(replayed?.body).toBe(body);
    expect(replayed?.headers["content-encoding"]).toBeUndefined();
    expect(Number(replayed?.headers["content-length"])).toBe(Buffer.byteLength(body));
  } finally {
    await server.close();
  }
});

test("Replay 执行中可以点击 Cancel 中止请求", async ({ page }) => {
  const server = await startMockHttpServer((_req, res) => {
    setTimeout(() => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: true }));
    }, 15_000);
  });

  try {
    await openPage(page, "replay");
    await expect(page.getByTestId("replay-request-panel")).toBeVisible();

    await page.getByTestId("replay-url-input").fill(`http://127.0.0.1:${server.port}/cancel-check`);
    await page.getByTestId("replay-send-button").click();

    const cancelButton = page.getByRole("button", { name: "Cancel" });
    await expect(cancelButton).toBeVisible();
    await expect(page.getByTestId("replay-response-executing")).toBeVisible();

    await cancelButton.click();

    await expect(page.getByTestId("replay-send-button")).toBeVisible();
    await expect(page.getByTestId("replay-response-executing")).toHaveCount(0);
  } finally {
    await server.close();
  }
});
