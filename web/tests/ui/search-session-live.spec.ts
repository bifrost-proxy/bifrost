import { test, expect } from "@playwright/test";
import { createServer, request as httpRequest } from "node:http";
import type { AddressInfo } from "node:net";

test("real proxy traffic does not restart completed search and full details survive paging", async ({
  page,
  request,
}) => {
  let upstreamRequests = 0;
  const upstream = createServer((req, res) => {
    upstreamRequests++;
    res.setHeader("Content-Type", "text/plain");
    res.end(`search-live-body ${req.url}`);
  });
  await new Promise<void>((resolve) =>
    upstream.listen(0, "127.0.0.1", resolve),
  );
  const port = (upstream.address() as AddressInfo).port;
  const marker =
    process.env.BIFROST_SEARCH_TEST_MARKER || `search-live-${Date.now()}`;
  const proxy = (path: string) =>
    new Promise<void>((resolve, reject) => {
      const req = httpRequest(
        {
          host: "127.0.0.1",
          port: 8848,
          path: `http://127.0.0.1:${port}/${marker}/${path}`,
          method: "GET",
          headers: { Host: `127.0.0.1:${port}` },
        },
        (res) => {
          res.resume();
          res.on("end", () =>
            res.statusCode === 200
              ? resolve()
              : reject(new Error(`proxy status ${res.statusCode}`)),
          );
        },
      );
      req.on("error", reject);
      req.end();
    });
  const searchRequests: string[] = [],
    pushFrames: string[] = [];
  page.on("request", (req) => {
    if (/\/api\/search(?:\/stream)?$/.test(new URL(req.url()).pathname))
      searchRequests.push(req.postData() || "");
  });
  page.on("websocket", (ws) => {
    if (ws.url().includes("/api/push"))
      ws.on("framereceived", (frame) => pushFrames.push(String(frame.payload)));
  });
  try {
    // More than the production streaming max_results=1000, to exercise real pagination.
    console.log("live-stage: seed");
    let next = process.env.BIFROST_SEARCH_TEST_REUSE_SEED === "1" ? 1100 : 0;
    await Promise.all(
      Array.from({ length: 32 }, async () => {
        while (next < 1100) {
          const i = next++;
          await proxy(`seed-${i}`);
        }
      }),
    );
    await expect
      .poll(
        async () =>
          (
            await request.get(
              "http://127.0.0.1:8848/_bifrost/api/traffic?limit=1",
            )
          )
            .json()
            .then((x) => x.total ?? x.server_total ?? 0),
        { timeout: 15000 },
      )
      .toBeGreaterThanOrEqual(1100);
    console.log("live-stage: navigate");
    await page.goto("/_bifrost/traffic");
    await expect(page.getByTestId("traffic-table")).toBeVisible();
    await page.getByRole("button", { name: /Fuzzy Search/ }).click();
    await page
      .getByRole("textbox", { name: "Enter keyword to search all content..." })
      .fill(marker);
    await page.getByTestId("search-mode-submit").click();
    await expect(page.getByTestId("search-result-row").first()).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Load More", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /Stop/ })).toHaveCount(0);
    console.log("live-stage: select");
    const selected = page.getByTestId("search-result-row").first();
    const selectedId = await selected.getAttribute("data-record-id");
    await selected.click();
    const detail = page.getByTestId("traffic-detail");
    await expect(detail).toBeVisible();
    const header = page.getByTestId("traffic-detail-header");
    const selectedUrl = await header.getAttribute("data-url");
    expect(selectedUrl).toContain(marker);
    await page.getByTestId("response-tab-body").click();
    await expect(detail).toContainText("search-live-body");
    // Record the actual scroll container and selected detail DOM node.
    const scroller = await selected.evaluateHandle(
      (row) => row.parentElement!.parentElement!,
    );
    await scroller.evaluate((el) => {
      el.scrollTop = 700;
    });
    await expect.poll(() => scroller.evaluate((el) => el.scrollTop)).toBe(700);
    const anchorId = await page
      .getByTestId("search-result-row")
      .first()
      .getAttribute("data-record-id");
    const anchor = await page
      .locator(`[data-record-id="${anchorId}"]`)
      .elementHandle();
    const detailNode = await detail.elementHandle();
    console.log("live-stage: page");
    const beforePage = searchRequests.length;
    await page.getByRole("button", { name: "Load More", exact: true }).click();
    await expect.poll(() => searchRequests.length).toBe(beforePage + 1);
    await expect(
      page.getByRole("button", { name: "Load More", exact: true }),
    ).toHaveCount(0);
    expect(await scroller.evaluate((el) => el.scrollTop)).toBe(700);
    expect(
      await anchor!.evaluate(
        (el, id) => el === document.querySelector(`[data-record-id="${id}"]`),
        anchorId,
      ),
    ).toBe(true);
    await expect(header).toHaveAttribute("data-url", selectedUrl!);
    expect(
      await detailNode!.evaluate(
        (el) => el === document.querySelector('[data-testid="traffic-detail"]'),
      ),
    ).toBe(true);
    const visible = await page
      .getByTestId("search-result-row")
      .allTextContents();
    await expect(detail).toContainText("search-live-body");
    const detailText = await detail.innerText();
    const loadingWatcher = await page.evaluateHandle(() => {
      const state = { restarts: 0 };
      const observer = new MutationObserver(() => {
        if (
          Array.from(document.querySelectorAll("button")).some(
            (button) =>
              button.textContent?.trim() === "Stop" &&
              button.offsetParent !== null,
          )
        )
          state.restarts++;
      });
      observer.observe(document.body, { childList: true, subtree: true });
      return { state, disconnect: () => observer.disconnect() };
    });
    const stableRequests = searchRequests.length;
    console.log("live-stage: continuous traffic");
    const liveSuffix = Date.now();
    const latePath = `late-after-completion-${liveSuffix}`;
    const lastPath = `new-${liveSuffix}-23`;
    for (let i = 0; i < 24; i++) {
      await proxy(i === 0 ? latePath : `new-${liveSuffix}-${i}`);
      await new Promise((r) => setTimeout(r, 125));
    }
    await expect
      .poll(() => pushFrames.some((frame) => frame.includes(latePath)))
      .toBe(true);
    expect(searchRequests).toHaveLength(stableRequests);
    await expect(page.getByRole("button", { name: /Stop/ })).toHaveCount(0);
    expect(
      await page.getByTestId("search-result-row").allTextContents(),
    ).toEqual(visible);
    expect(await scroller.evaluate((el) => el.scrollTop)).toBe(700);
    await expect(header).toHaveAttribute("data-url", selectedUrl!);
    expect(
      await detailNode!.evaluate(
        (el) => el === document.querySelector('[data-testid="traffic-detail"]'),
      ),
    ).toBe(true);
    expect(await detail.innerText()).toBe(detailText);
    expect(
      await loadingWatcher.evaluate((watcher) => watcher.state.restarts),
    ).toBe(0);
    await loadingWatcher.evaluate((watcher) => watcher.disconnect());
    await page.getByTestId("search-mode-submit").click();
    await expect.poll(() => searchRequests.length).toBe(stableRequests + 1);
    await expect(
      page.getByTestId("search-result-row").filter({ hasText: lastPath }),
    ).toBeVisible();
    expect(selectedId).toBeTruthy();
    console.log(
      JSON.stringify({
        upstreamRequests,
        seedRequests: 1100,
        liveRequests: 24,
        searchRequests: searchRequests.length,
        pushFrames: pushFrames.length,
        selectedId,
        scrollTop: 700,
        detailUrl: selectedUrl,
      }),
    );
  } finally {
    upstream.closeAllConnections();
    await new Promise<void>((resolve) => upstream.close(() => resolve()));
  }
});
