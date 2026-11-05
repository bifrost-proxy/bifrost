import { test, expect } from "@playwright/test";
import { gotoIsolatedTraffic } from "./helpers/isolated-locale";
import {
  startIsolatedBackend,
  startMockServer,
} from "./helpers/traffic-fixtures";

test("隔离后端语言 seed 只作用于目标 origin 并保留已有选择", async ({
  page,
  context,
}) => {
  const backend = await startIsolatedBackend();
  const otherOrigin = await startMockServer();
  const defaultPage = await context.newPage();
  try {
    // A fresh isolated origin still uses the product's Chinese default.
    await defaultPage.goto(`${backend.baseUrl}/_bifrost/traffic`);
    await expect(
      defaultPage.getByRole("button", { name: "search 模糊搜索", exact: true }),
    ).toBeVisible();
    await defaultPage.close();

    await gotoIsolatedTraffic(page, backend.baseUrl);
    await expect(
      page.getByRole("button", { name: "search Fuzzy Search", exact: true }),
    ).toBeVisible();
    await page.reload();
    await expect(
      page.getByRole("button", { name: "search Fuzzy Search", exact: true }),
    ).toBeVisible();

    // Seeding must not reset a locale subsequently selected by a test.
    await page.evaluate(() => {
      localStorage.setItem(
        "bifrost-locale",
        JSON.stringify({ state: { locale: "zh-CN" }, version: 0 }),
      );
    });
    await page.reload();
    await expect(
      page.getByRole("button", { name: "search 模糊搜索", exact: true }),
    ).toBeVisible();

    await page.goto(`http://127.0.0.1:${otherOrigin.port}`);
    expect(
      await page.evaluate(() => localStorage.getItem("bifrost-locale")),
    ).toBeNull();
  } finally {
    await defaultPage.close();
    await backend.close();
    await otherOrigin.close();
  }
});
