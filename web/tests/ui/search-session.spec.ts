import type {} from "./fixtures/search-session";
import { test, expect } from "@playwright/test";

test("search rows, scroll and selection survive stream batches, pages and continuous traffic", async ({
  page,
}) => {
  await page.route("**/api/security/csrf", (route) =>
    route.fulfill({ json: { csrf_token: "test" } }),
  );
  const result = (id: number) => ({
    record: {
      id: `r${id}`,
      seq: 1000 - id,
      ts: 0,
      m: "GET",
      h: "match.test",
      p: `/match/${id}`,
      s: 200,
      ct: "text/plain",
      req_sz: 0,
      res_sz: 10,
      dur: 1,
      proto: "http",
      cip: "127.0.0.1",
      flags: 0,
      fc: 0,
      st: new Date(0).toISOString(),
    },
    matches: [],
  });
  let requests = 0;
  await page.route("**/api/search/stream", async (route) => {
    requests++;
    const cursor = route.request().postDataJSON().cursor;
    const rows = cursor
      ? [result(59), ...Array.from({ length: 30 }, (_, i) => result(i + 60))]
      : Array.from({ length: 60 }, (_, i) => result(i));
    const events =
      rows
        .map((data) => `event: result\ndata: ${JSON.stringify(data)}\n\n`)
        .join("") +
      `event: done\ndata: ${JSON.stringify({ total_searched: rows.length, total_matched: rows.length, next_cursor: cursor ? null : 940, has_more: !cursor, search_id: "test" })}\n\n`;
    await route.fulfill({ contentType: "text/event-stream", body: events });
  });
  await page.goto("/_bifrost/tests/ui/fixtures/search-session.html");
  await page.getByRole("textbox").fill("match");
  expect(requests).toBe(0);
  await page.getByTestId("search-mode-submit").click();
  await expect(page.getByTestId("search-result-row").first()).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => window.searchStore.getState().isSearching))
    .toBe(false);
  // Capture a real DOM node and its scrolling container across later updates.
  await page.evaluate(() => {
    const row = document.querySelector('[data-record-id="r0"]')!;
    Object.assign(window, {
      firstRow: row,
      scrollContainer: row.parentElement!.parentElement,
    });
  });
  await page.getByTestId("search-result-row").first().click();
  await expect(page.getByTestId("detail")).toHaveText("r0");
  await page.evaluate(() => {
    window.scrollContainer.scrollTop = 700;
  });
  await expect
    .poll(() => page.evaluate(() => window.scrollContainer.scrollTop))
    .toBe(700);
  await expect(page.locator('[data-record-id="r10"]')).toBeVisible();
  await page.evaluate(() => {
    window.firstRow = document.querySelector('[data-record-id="r10"]')!;
  });
  // Feed a page while already scrolled: preserve the same anchor DOM node and offset.
  await page.getByRole("button", { name: "Load More", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => window.searchStore.getState().results.length),
    )
    .toBe(90);
  expect(
    await page.evaluate(
      () =>
        window.firstRow === document.querySelector('[data-record-id="r10"]'),
    ),
  ).toBe(true);
  expect(await page.evaluate(() => window.scrollContainer.scrollTop)).toBe(700);
  const visible = await page.getByTestId("search-result-row").allTextContents();
  for (let i = 0; i < 5; i++) {
    await page.evaluate(() => {
      const s = window.trafficStore;
      const m = s.getState().recordsMutation;
      s.setState({
        recordsMutation: {
          ...m,
          version: m.version + 1,
          reset: true,
          incomplete: true,
        },
      });
    });
    await page.waitForTimeout(250);
  }
  expect(requests).toBe(2);
  expect(
    await page.evaluate(() => window.searchStore.getState().isSearching),
  ).toBe(false);
  expect(await page.getByTestId("search-result-row").allTextContents()).toEqual(
    visible,
  );
  expect(await page.evaluate(() => window.scrollContainer.scrollTop)).toBe(700);
  await expect(page.getByTestId("detail")).toHaveText("r0");
  // Editing filters/query never submits a search.
  await page.getByRole("textbox").fill("draft");
  await page.evaluate(() => {
    window.filterStore.setState({ selectedDomains: ["other.test"] });
  });
  expect(requests).toBe(2);
  await page.getByTestId("search-mode-submit").click();
  await expect.poll(() => requests).toBe(3);
  await page.getByRole("button", { name: "Exit", exact: true }).click();
  expect(await page.evaluate(() => window.searchStore.getState().mode)).toBe(
    "normal",
  );
});
