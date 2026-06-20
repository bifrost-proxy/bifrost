import { expect, type Page, test } from "@playwright/test";
import { openPage, waitForToast } from "./helpers/admin-helpers";

async function dismissStartupModals(page: Page) {
  await page.waitForSelector(".ant-modal-wrap", { timeout: 1000 }).catch(() => undefined);
  await page.evaluate(() => {
    document
      .querySelectorAll(".ant-modal-root, .ant-modal-wrap, .ant-modal-mask")
      .forEach((element) => element.remove());
  });
  await expect(page.locator(".ant-modal-wrap")).toHaveCount(0);
}

test("Videos tool submits YouTube downloads and polls progress", async ({ page }) => {
  const defaultDir = "/Users/test/Downloads/YouTube";
  const customDir = "/tmp/bifrost-videos-ui";
  const url = "https://www.youtube.com/watch?v=3i5_v_sUZ04";
  let listCalls = 0;
  let openCalled = false;
  let revealCalled = false;

  await page.route("**/_bifrost/api/videos/defaults", async (route) => {
    await route.fulfill({ json: { download_dir: defaultDir } });
  });
  await page.route("**/_bifrost/api/videos/downloads/video-task-1/open", async (route) => {
    openCalled = true;
    await route.fulfill({ json: { ok: true } });
  });
  await page.route("**/_bifrost/api/videos/downloads/video-task-1/reveal", async (route) => {
    revealCalled = true;
    await route.fulfill({ json: { ok: true } });
  });
  await page.route("**/_bifrost/api/videos/downloads", async (route) => {
    if (route.request().method() === "POST") {
      await route.fulfill({
        status: 202,
        json: {
          id: "video-task-1",
          url,
          download_dir: customDir,
          status: "running",
          progress_percent: 12.5,
          downloaded: null,
          total: "2.31GiB",
          speed: "8.00MiB/s",
          eta: "04:10",
          file_path: null,
          message: "[download] 12.5% of 2.31GiB",
          error: null,
          created_at_ms: 1781929777297,
          updated_at_ms: 1781929777297,
        },
      });
      return;
    }

    listCalls += 1;
    await route.fulfill({
      json: {
        downloads:
          listCalls < 3
            ? [
                {
                  id: "video-task-1",
                  url,
                  download_dir: customDir,
                  status: "running",
                  progress_percent: 46.7,
                  downloaded: null,
                  total: "2.31GiB",
                  speed: "7.25MiB/s",
                  eta: "02:52",
                  file_path: null,
                  message: "[download] 46.7% of 2.31GiB",
                  error: null,
                  created_at_ms: 1781929777297,
                  updated_at_ms: 1781929812000,
                },
              ]
            : [
                {
                  id: "video-task-1",
                  url,
                  download_dir: customDir,
                  status: "completed",
                  progress_percent: 100,
                  downloaded: "2.32GiB",
                  total: "2.32GiB",
                  speed: null,
                  eta: null,
                  file_path: `${customDir}/Underwater World 8K [3i5_v_sUZ04].mp4`,
                  message: "completed",
                  error: null,
                  created_at_ms: 1781929777297,
                  updated_at_ms: 1781929862000,
                },
              ],
      },
    });
  });

  await openPage(page, "ai?aiSection=tools-videos");
  await dismissStartupModals(page);

  await expect(page.getByTestId("videos-tool-page")).toBeVisible();
  await expect(page.getByLabel("Download directory", { exact: true })).toHaveValue(defaultDir);
  await page.getByPlaceholder("https://www.youtube.com/watch?v=...", { exact: true }).fill(url);
  await page.getByLabel("Download directory", { exact: true }).fill(customDir);
  await page.getByRole("button", { name: "download Download" }).click();

  await waitForToast(page, "Download started");
  await expect(page.getByText(url)).toBeVisible();
  await expect(page.getByText("running")).toBeVisible();
  await expect(page.getByText("2.31GiB")).toBeVisible();
  await expect(page.getByText("completed")).toBeVisible({ timeout: 5000 });
  await expect(page.getByText("2.32GiB")).toBeVisible();
  await expect(page.getByText(`${customDir}/Underwater World 8K [3i5_v_sUZ04].mp4`)).toBeVisible();

  const popupPromise = page.waitForEvent("popup");
  await page.getByRole("button", { name: "play-circle Play" }).click();
  const popup = await popupPromise;
  expect(popup.url()).toContain("/_bifrost/api/videos/downloads/video-task-1/file");
  await popup.close();

  await page.getByRole("button", { name: "export Open" }).click();
  await waitForToast(page, "Opening video file");
  expect(openCalled).toBe(true);

  await page.getByRole("button", { name: "folder-open Reveal" }).click();
  await waitForToast(page, "Opening folder");
  expect(revealCalled).toBe(true);
});

test("Videos tool rejects non-YouTube URLs without adding a task", async ({ page }) => {
  await page.route("**/_bifrost/api/videos/defaults", async (route) => {
    await route.fulfill({ json: { download_dir: "/Users/test/Downloads/YouTube" } });
  });
  await page.route("**/_bifrost/api/videos/downloads", async (route) => {
    if (route.request().method() === "POST") {
      await route.fulfill({
        status: 400,
        json: { error: "Only YouTube URLs are supported", status: 400 },
      });
      return;
    }
    await route.fulfill({ json: { downloads: [] } });
  });

  await openPage(page, "ai?aiSection=tools-videos");
  await dismissStartupModals(page);

  await page.getByPlaceholder("https://www.youtube.com/watch?v=...", { exact: true }).fill("https://example.com/video.mp4");
  await page.getByLabel("Download directory", { exact: true }).fill("/tmp/videos");
  await page.getByRole("button", { name: "download Download" }).click();

  await waitForToast(page, "Only YouTube URLs are supported");
  await expect(page.getByText("No downloads")).toBeVisible();
});

test("Videos tool retries failed downloads", async ({ page }) => {
  const url = "https://www.youtube.com/watch?v=XnxLbR4GFVY";
  let retryCalled = false;

  await page.route("**/_bifrost/api/videos/defaults", async (route) => {
    await route.fulfill({ json: { download_dir: "/Users/test/Downloads/YouTube" } });
  });
  await page.route("**/_bifrost/api/videos/downloads/failed-task-1/retry", async (route) => {
    retryCalled = true;
    await route.fulfill({
      status: 202,
      json: {
        id: "failed-task-1",
        url,
        download_dir: "/Users/test/Downloads/YouTube",
        status: "running",
        progress_percent: null,
        downloaded: null,
        total: null,
        speed: null,
        eta: null,
        file_path: null,
        message: "queued for retry",
        error: null,
        created_at_ms: 1781932211000,
        updated_at_ms: 1781932300000,
      },
    });
  });
  await page.route("**/_bifrost/api/videos/downloads", async (route) => {
    await route.fulfill({
      json: {
        downloads: [
          {
            id: "failed-task-1",
            url,
            download_dir: "/Users/test/Downloads/YouTube",
            status: "failed",
            progress_percent: 31.2,
            downloaded: "739.02MiB",
            total: null,
            speed: "2.82MiB/s",
            eta: "02:31",
            file_path: null,
            message: "network error",
            error: "network error",
            created_at_ms: 1781932211000,
            updated_at_ms: 1781932211000,
          },
        ],
      },
    });
  });

  await openPage(page, "ai?aiSection=tools-videos");
  await dismissStartupModals(page);

  await expect(page.getByText(url)).toBeVisible();
  await expect(page.getByText("failed")).toBeVisible();
  await page.getByRole("button", { name: "reload Retry" }).click();

  await waitForToast(page, "Download retry started");
  await expect(page.getByText("running")).toBeVisible();
  expect(retryCalled).toBe(true);
});
