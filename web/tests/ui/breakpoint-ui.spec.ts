import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  clearTraffic,
  openPage,
  sendProxyRequestWithResponse,
  setMonacoEditor,
  startMockHttpServer,
  waitForTrafficRow,
  waitForToast,
  uniqueName,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ request }) => {
  await clearRules(request);
  await clearTraffic(request);
});

test("Sidebar exposes an OpenAPI entry above the theme toggle", async ({
  page,
}) => {
  await openPage(page, "traffic");

  const openApi = page.getByTestId("app-sidebar-openapi");
  await expect(openApi).toBeVisible();
  await expect(openApi).toHaveText("OpenAPI");

  const positions = await page.evaluate(() => {
    const openApiRect = document
      .querySelector('[data-testid="app-sidebar-openapi"]')
      ?.getBoundingClientRect();
    const themeRect = document
      .querySelector('[data-testid="theme-toggle"]')
      ?.getBoundingClientRect();
    return {
      openApiBottom: openApiRect?.bottom ?? 0,
      themeTop: themeRect?.top ?? 0,
    };
  });
  expect(positions.openApiBottom).toBeLessThanOrEqual(positions.themeTop);

  const popupPromise = page.waitForEvent("popup");
  await openApi.click();
  const popup = await popupPromise;
  await expect(popup).toHaveURL(/\/_bifrost\/swagger/);
  await popup.close();
});

test("Traffic toolbar exposes one global Breakpoint gate with rule-phase guidance", async ({
  page,
  request,
}) => {
  const originalRes = await request.get(`${apiBase}/breakpoint/settings`);
  const original = (await originalRes.json()) as {
    enabled: boolean;
    max_body_bytes: number;
  };

  try {
    await request.post(`${apiBase}/breakpoint/settings`, {
      data: { ...original, enabled: false },
    });

    await openPage(page, "traffic");
    const toggle = page.getByTestId("toolbar-breakpoint-toggle");
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    await expect(page.locator("body")).not.toContainText("Hook Request");
    await expect(page.locator("body")).not.toContainText("Hook Response");

    await page.getByTestId("toolbar-breakpoint-help").hover();
    await expect(page.locator(".ant-popover")).toContainText(
      "Breakpoint global gate",
    );
    await expect(page.locator(".ant-popover")).toContainText(
      "breakpoint://request",
    );
    await expect(page.locator(".ant-popover")).toContainText(
      "breakpoint://response",
    );
    await expect(page.locator(".ant-popover")).toContainText(
      "This switch alone does not pause traffic",
    );

    await toggle.click();
    await expect
      .poll(async () => {
        const res = await request.get(`${apiBase}/breakpoint/settings`);
        const body = (await res.json()) as { enabled: boolean };
        return body.enabled;
      })
      .toBe(true);
  } finally {
    await request.post(`${apiBase}/breakpoint/settings`, { data: original });
  }
});

test("Rules editor opens Breakpoint value suggestions", async ({
  page,
  request,
}) => {
  const ruleName = uniqueName("breakpoint-hints");
  const createRuleRes = await request.post(`${apiBase}/rules`, {
    data: {
      name: ruleName,
      content: "ui-breakpoint.test breakpoint://request",
      enabled: true,
    },
  });
  if (!createRuleRes.ok()) {
    throw new Error(await createRuleRes.text());
  }

  await openPage(page, "rules");
  await page.getByTestId("rule-item").filter({ hasText: ruleName }).first().click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();

  const editorContainer = page.getByTestId("rule-editor-container");
  await setMonacoEditor(page, editorContainer, "ui-breakpoint.test breakpoint://");
  const editorInput = editorContainer.getByRole("textbox", { name: "Editor content" });
  await editorInput.click({ force: true });
  await page.keyboard.press(process.platform === "darwin" ? "Meta+ArrowRight" : "End");
  await page.keyboard.press("Control+Space");

  const suggestions = page.locator(".suggest-widget .monaco-list-row");
  await expect(suggestions.filter({ hasText: "request" }).first()).toBeVisible();
});

test("Settings Performance configures Breakpoint auto-resume timeout with fixed safety bounds", async ({
  page,
  request,
}) => {
  const originalRes = await request.get(`${apiBase}/config/performance`);
  const original = (await originalRes.json()) as {
    breakpoint: {
      timeout_ms: number;
      timeout_min_ms: number;
      timeout_max_ms: number;
    };
  };
  const nextTimeout =
    original.breakpoint.timeout_ms + 1000 <= original.breakpoint.timeout_max_ms
      ? original.breakpoint.timeout_ms + 1000
      : original.breakpoint.timeout_ms - 1000;

  try {
    await openPage(page, "settings");
    await page.getByRole("tab", { name: /Performance/ }).click();

    await expect(page.getByTestId("settings-performance-tab")).toContainText(
      "Breakpoint Auto-Resume Timeout",
    );
    await expect(page.getByTestId("settings-performance-tab")).toContainText(
      "Fixed safety range",
    );
    await expect(
      page.getByTestId("settings-performance-breakpoint-timeout-bounds"),
    ).toContainText(`Min: ${Math.round(original.breakpoint.timeout_min_ms / 1000)}s`);
    await expect(
      page.getByTestId("settings-performance-breakpoint-timeout-bounds"),
    ).toContainText(`Max: ${Math.round(original.breakpoint.timeout_max_ms / 1000)}s`);

    const slider = page.getByTestId("settings-performance-breakpoint-timeout");
    await expect(slider).toBeVisible();
    const handle = slider.locator(".ant-slider-handle").first();
    await expect(handle).toHaveAttribute(
      "aria-valuemin",
      String(original.breakpoint.timeout_min_ms),
    );
    await expect(handle).toHaveAttribute(
      "aria-valuemax",
      String(original.breakpoint.timeout_max_ms),
    );
    const input = page
      .getByTestId("settings-performance-breakpoint-timeout-input")
      .locator("input");
    await expect(input).toHaveAttribute(
      "aria-valuemin",
      String(original.breakpoint.timeout_min_ms),
    );
    await expect(input).toHaveAttribute(
      "aria-valuemax",
      String(original.breakpoint.timeout_max_ms),
    );

    await handle.focus();
    await page.keyboard.press(nextTimeout > original.breakpoint.timeout_ms ? "ArrowRight" : "ArrowLeft");
    await waitForToast(page, `Breakpoint timeout updated to ${nextTimeout}ms`);

    await expect
      .poll(async () => {
        const res = await request.get(`${apiBase}/config/performance`);
        const body = (await res.json()) as {
          breakpoint: { timeout_ms: number };
        };
        return body.breakpoint.timeout_ms;
      })
      .toBe(nextTimeout);
  } finally {
    await request.put(`${apiBase}/config/performance`, {
      data: { breakpoint_timeout_ms: original.breakpoint.timeout_ms },
    });
  }
});

test("Network detail edits and resumes real request and response breakpoints", async ({
  page,
  request,
}) => {
  const mock = await startMockHttpServer((_req, res) => {
    const responseBody = "original-response";
    res.writeHead(200, {
      "Content-Type": "text/plain",
      "Content-Length": String(Buffer.byteLength(responseBody)),
      "X-Original-Response": "yes",
    });
    res.end(responseBody);
  });
  const originalSettings = (await (
    await request.get(`${apiBase}/breakpoint/settings`)
  ).json()) as { enabled: boolean; max_body_bytes: number };
  const originalPerformance = (await (
    await request.get(`${apiBase}/config/performance`)
  ).json()) as { breakpoint: { timeout_ms: number } };
  const ruleName = uniqueName("breakpoint-real-ui");

  try {
    const createRule = await request.post(`${apiBase}/rules`, {
      data: {
        name: ruleName,
        content: [
          `127.0.0.1:${mock.port}/ui-request breakpoint://request`,
          `127.0.0.1:${mock.port}/ui-response breakpoint://response`,
          `127.0.0.1:${mock.port}/ui-timeout breakpoint://response`,
        ].join("\n"),
        enabled: true,
      },
    });
    expect(createRule.ok()).toBe(true);
    await request.post(`${apiBase}/breakpoint/settings`, {
      data: { enabled: true, max_body_bytes: 1024 * 1024 },
    });
    await openPage(page, "traffic");

    const requestPromise = sendProxyRequestWithResponse(
      `http://127.0.0.1:${mock.port}/ui-request`,
      {
        method: "POST",
        headers: { "Content-Type": "text/plain", "X-Original-Request": "yes" },
        body: "original-request",
      },
    );
    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/breakpoint/pending`);
        const pending = (await response.json()) as Array<{ phase: string }>;
        return pending[0]?.phase;
      })
      .toBe("request");

    await page.addInitScript(() => {
      const realNow = Date.now.bind(Date);
      Date.now = () => realNow() + 60 * 60 * 1000;
    });
    await page.reload();
    const requestRow = await waitForTrafficRow(page, "/ui-request");
    await expect(requestRow).toHaveAttribute("data-breakpoint-phase", "request");
    await expect(requestRow.getByTestId("breakpoint-request-indicator")).toBeVisible();
    const lightBreakpointBg = await requestRow.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    );
    expect(lightBreakpointBg).not.toBe("rgba(0, 0, 0, 0)");
    await page.getByTestId("theme-toggle").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    const darkBreakpointBg = await requestRow.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    );
    expect(darkBreakpointBg).not.toBe(lightBreakpointBg);
    expect(darkBreakpointBg).not.toBe("rgba(0, 0, 0, 0)");
    await page.getByTestId("theme-toggle").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

    await page.getByRole("button", { name: /Fuzzy Search/ }).click();
    await page
      .getByPlaceholder("Enter keyword to search all content...")
      .fill("ui-request");
    await page.getByTestId("search-mode-submit").click();
    const searchRow = page
      .getByTestId("search-result-row")
      .filter({ hasText: "ui-request" })
      .first();
    await expect(searchRow).toHaveAttribute("data-breakpoint-phase", "request");
    await expect(
      searchRow.getByTestId("search-breakpoint-request-indicator"),
    ).toBeVisible();
    const searchBreakpointBg = await searchRow.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    );
    expect(searchBreakpointBg).not.toBe("rgba(0, 0, 0, 0)");
    await page.getByRole("button", { name: "Exit" }).click();

    await requestRow.click();
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "request",
    );
    await expect(page.getByTestId("breakpoint-countdown")).not.toContainText(
      "0.0s",
    );
    await page.getByTestId("breakpoint-method-input").fill("PUT");
    await page
      .getByTestId("breakpoint-url-input")
      .fill(`http://127.0.0.1:${mock.port}/ui-request-edited?mode=ui`);
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      "edited-request-body",
    );
    await page.getByTestId("request-tab-header").click();
    await page.getByTestId("request-header-view-add").click();
    const requestHeaderNames = page.locator('[data-testid^="request-header-view-name-"]');
    const requestHeaderValues = page.locator('[data-testid^="request-header-view-value-"]');
    await requestHeaderNames.last().fill("X-UI-Breakpoint");
    await requestHeaderValues.last().fill("request-edited");
    await page.getByTestId("breakpoint-apply-resume").click();
    await waitForToast(page, "Breakpoint edits applied");

    const requestResult = await requestPromise;
    expect(requestResult.status).toBe(200);
    const receivedRequest = mock.requests.at(-1);
    expect(receivedRequest?.method).toBe("PUT");
    expect(receivedRequest?.url).toBe("/ui-request-edited?mode=ui");
    expect(receivedRequest?.body).toBe("edited-request-body");
    expect(receivedRequest?.headers["x-ui-breakpoint"]).toBe("request-edited");
    await expect(requestRow).not.toHaveAttribute("data-breakpoint-phase", /.+/);
    await expect
      .poll(() =>
        requestRow.evaluate((element) => getComputedStyle(element).backgroundColor),
      )
      .not.toBe(lightBreakpointBg);

    const responsePromise = sendProxyRequestWithResponse(
      `http://127.0.0.1:${mock.port}/ui-response`,
    );
    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/breakpoint/pending`);
        const pending = (await response.json()) as Array<{ phase: string }>;
        return pending[0]?.phase;
      })
      .toBe("response");

    await page.reload();
    const responseRow = await waitForTrafficRow(page, "/ui-response");
    await expect(responseRow.getByTestId("breakpoint-response-indicator")).toBeVisible();
    await responseRow.click();
    await expect(page.getByTestId("breakpoint-editor-banner")).toHaveAttribute(
      "data-phase",
      "response",
    );
    const statusInput = page.getByTestId("breakpoint-status-input");
    await statusInput.fill("418");
    await statusInput.press("Enter");
    await expect(statusInput).toHaveValue("418");
    await setMonacoEditor(
      page,
      page.getByTestId("traffic-detail"),
      "edited-response-body",
    );
    await page.getByTestId("response-tab-header").click();
    await page.getByTestId("response-header-view-add").click();
    const responseHeaderNames = page.locator(
      '[data-testid^="response-header-view-name-"]',
    );
    const responseHeaderValues = page.locator(
      '[data-testid^="response-header-view-value-"]',
    );
    await responseHeaderNames.last().fill("X-UI-Breakpoint-Response");
    await responseHeaderValues.last().fill("response-edited");
    await page.getByTestId("breakpoint-apply-resume").click();
    await waitForToast(page, "Breakpoint edits applied");

    const responseResult = await responsePromise;
    expect(responseResult.status).toBe(418);
    expect(responseResult.body).toBe("edited-response-body");
    expect(responseResult.headers["x-ui-breakpoint-response"]).toBe("response-edited");
    await expect(responseRow).not.toHaveAttribute("data-breakpoint-phase", /.+/);
    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/breakpoint/pending`);
        return ((await response.json()) as unknown[]).length;
      })
      .toBe(0);

    await request.put(`${apiBase}/config/performance`, {
      data: { breakpoint_timeout_ms: 5000 },
    });
    const timeoutPromise = sendProxyRequestWithResponse(
      `http://127.0.0.1:${mock.port}/ui-timeout`,
    );
    await expect
      .poll(async () => {
        const response = await request.get(`${apiBase}/breakpoint/pending`);
        const pending = (await response.json()) as Array<{ phase: string }>;
        return pending[0]?.phase;
      })
      .toBe("response");
    const timeoutRow = await waitForTrafficRow(page, "/ui-timeout");
    await expect(timeoutRow).toHaveAttribute("data-breakpoint-phase", "response");
    const timeoutBreakpointBg = await timeoutRow.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    );
    const timeoutResult = await timeoutPromise;
    expect(timeoutResult.status).toBe(200);
    expect(timeoutResult.body).toBe("original-response");
    await expect(timeoutRow).not.toHaveAttribute("data-breakpoint-phase", /.+/);
    await expect
      .poll(() =>
        timeoutRow.evaluate((element) => getComputedStyle(element).backgroundColor),
      )
      .not.toBe(timeoutBreakpointBg);
  } finally {
    await request.put(`${apiBase}/config/performance`, {
      data: { breakpoint_timeout_ms: originalPerformance.breakpoint.timeout_ms },
    });
    await request.post(`${apiBase}/breakpoint/settings`, { data: originalSettings });
    await mock.close();
  }
});
