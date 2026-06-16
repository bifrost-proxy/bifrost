const puppeteer = require("puppeteer");
const fs = require("fs").promises;
const path = require("path");
const { VerifyLogger } = require("./logger");
const { SESSION_DIR, SHARED_PROFILE_DIR } = require("./config");

const logger = new VerifyLogger("BrowserManager");

const DEFAULT_VIEWPORT = { width: 1280, height: 900 };

async function ensureDir(dir) {
  try {
    await fs.mkdir(dir, { recursive: true });
  } catch (e) {
    if (e.code !== "EEXIST") throw e;
  }
}

function setupPageListeners(page, options = {}) {
  const { collectNetwork = true, collectConsole = true } = options;

  if (collectConsole) {
    page.on("console", (msg) => {
      const type = msg.type();
      const text = msg.text();
      if (type === "error") {
        logger.error(`[Console] ${text}`);
      } else if (type === "warning") {
        logger.warn(`[Console] ${text}`);
      }
    });

    page.on("pageerror", (error) => {
      logger.error(`[PageError] ${error.message}`);
    });
  }

  if (collectNetwork) {
    page.on("requestfailed", (request) => {
      logger.warn(
        `[RequestFailed] ${request.url()} - ${request.failure()?.errorText}`,
      );
    });
  }
}

async function launchBrowser(options = {}) {
  const {
    headless = false,
    viewport = DEFAULT_VIEWPORT,
    fixedViewport = true,
    userDataDir = SHARED_PROFILE_DIR,
    devtools = false,
    slowMo = 0,
    args = [],
    cleanTabs = true,
  } = options;

  await ensureDir(userDataDir);

  const launchArgs = [
    "--no-sandbox",
    "--disable-setuid-sandbox",
    "--disable-web-security",
    "--disable-features=IsolateOrigins,site-per-process",
    "--disable-session-crashed-bubble",
    "--disable-infobars",
    `--window-size=${viewport.width},${viewport.height}`,
    ...args,
  ];

  const launchOptions = {
    headless,
    devtools,
    slowMo,
    args: launchArgs,
    defaultViewport: fixedViewport ? viewport : null,
  };

  if (userDataDir) {
    launchOptions.userDataDir = userDataDir;
  }

  const browser = await puppeteer.launch(launchOptions);
  logger.info("Browser launched");

  if (cleanTabs) {
    const pages = await browser.pages();
    if (pages.length > 1) {
      for (let i = 1; i < pages.length; i++) {
        await pages[i].close();
      }
    }
    if (pages.length > 0 && pages[0].url() !== "about:blank") {
      await pages[0].goto("about:blank");
    }
  }

  if (fixedViewport) {
    logger.info(
      `Browser launched (fixed viewport: ${viewport.width}x${viewport.height})`,
    );
  } else {
    logger.info("Browser launched (viewport follows window size)");
  }
  return browser;
}

async function connectToBrowser(wsEndpoint) {
  const browser = await puppeteer.connect({ browserWSEndpoint: wsEndpoint });
  logger.info("Connected to existing browser");
  return browser;
}

async function launchDetachedBrowser(options = {}) {
  const browser = await launchBrowser({ ...options, headless: false });
  const wsEndpoint = browser.wsEndpoint();
  const pid = browser.process()?.pid;

  const detachedInfo = {
    wsEndpoint,
    pid,
    launchedAt: new Date().toISOString(),
  };

  await ensureDir(SESSION_DIR);
  await fs.writeFile(
    path.join(SESSION_DIR, "detached.json"),
    JSON.stringify(detachedInfo, null, 2),
  );

  logger.info(`Detached browser launched, PID: ${pid}`);
  logger.info(`WebSocket: ${wsEndpoint}`);

  browser.disconnect();
  return detachedInfo;
}

async function connectToDetachedBrowser() {
  try {
    const infoPath = path.join(SESSION_DIR, "detached.json");
    const data = await fs.readFile(infoPath, "utf8");
    const info = JSON.parse(data);

    const browser = await connectToBrowser(info.wsEndpoint);
    return { browser, info };
  } catch (e) {
    logger.error("No detached browser found or connection failed");
    return null;
  }
}

async function closeBrowser(browser, options = {}) {
  const { graceful = true, timeout = 5000 } = options;

  if (!browser) return;

  try {
    if (graceful) {
      const pages = await browser.pages();
      logger.info(`Closing ${pages.length} page(s)...`);

      for (const page of pages) {
        try {
          if (!page.isClosed()) {
            await Promise.race([
              page.close(),
              new Promise((_, reject) =>
                setTimeout(
                  () => reject(new Error("Page close timeout")),
                  timeout,
                ),
              ),
            ]);
          }
        } catch (e) {
          logger.warn(`Failed to close page: ${e.message}`);
        }
      }
    }

    await browser.close();
    logger.info("Browser closed");
  } catch (e) {
    logger.error(`Error closing browser: ${e.message}`);
    try {
      const proc = browser.process();
      if (proc) {
        proc.kill("SIGKILL");
        logger.info("Browser process killed");
      }
    } catch (killErr) {
      logger.error(`Failed to kill browser process: ${killErr.message}`);
    }
  }
}

module.exports = {
  launchBrowser,
  closeBrowser,
  connectToBrowser,
  launchDetachedBrowser,
  connectToDetachedBrowser,
  setupPageListeners,
  ensureDir,
  DEFAULT_VIEWPORT,
};
