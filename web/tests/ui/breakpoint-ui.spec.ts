import { test, expect } from "@playwright/test";
import {
  apiBase,
  clearRules,
  openPage,
  setMonacoEditor,
  waitForToast,
  uniqueName,
} from "./helpers/admin-helpers";

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ request }) => {
  await clearRules(request);
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
