import type { Page } from "@playwright/test";

// storageState only seeds the configured baseURL. Isolated backends use a
// different origin, so seed their English test copy before the app hydrates.
export async function gotoIsolatedTraffic(page: Page, baseUrl: string) {
  const origin = new URL(baseUrl).origin;
  await page.addInitScript((targetOrigin) => {
    if (
      window.location.origin === targetOrigin &&
      localStorage.getItem("bifrost-locale") === null
    ) {
      localStorage.setItem(
        "bifrost-locale",
        JSON.stringify({ state: { locale: "en" }, version: 0 }),
      );
    }
  }, origin);
  await page.goto(`${origin}/_bifrost/traffic`);
}
