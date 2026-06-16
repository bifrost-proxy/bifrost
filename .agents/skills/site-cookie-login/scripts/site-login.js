#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const readline = require("readline");
const http = require("http");
const { spawn, spawnSync } = require("child_process");

function resolveNpmCliPath() {
  const npmCliPath = path.resolve(
    path.dirname(process.execPath),
    "../lib/node_modules/npm/bin/npm-cli.js",
  );
  return fs.existsSync(npmCliPath) ? npmCliPath : null;
}

function ensureSiteLoginDependencies() {
  const marker = path.resolve(__dirname, "node_modules/puppeteer");
  if (fs.existsSync(marker)) {
    return;
  }

  const npmCliPath = resolveNpmCliPath();
  if (!npmCliPath) {
    throw new Error("未找到 npm-cli.js，无法自动安装 puppeteer 依赖");
  }

  console.log("📦 未检测到 puppeteer，正在为 site-cookie-login 自动安装依赖...");
  const install = spawnSync(process.execPath, [npmCliPath, "ci"], {
    cwd: __dirname,
    env: {
      ...process.env,
      PATH: `${path.dirname(process.execPath)}${path.delimiter}${process.env.PATH || ""}`,
    },
    stdio: "inherit",
  });
  if (install.status !== 0) {
    throw new Error(`依赖安装失败，退出码: ${install.status ?? "unknown"}`);
  }
}

function loadPuppeteer() {
  try {
    return require("puppeteer");
  } catch {}

  ensureSiteLoginDependencies();

  try {
    return require(path.resolve(__dirname, "node_modules/puppeteer"));
  } catch {}

  throw new Error("未找到 puppeteer，请先执行 npm install 安装依赖");
}

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) {
      continue;
    }
    const normalizedKey = key.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      args[normalizedKey] = true;
      continue;
    }
    args[normalizedKey] = next;
    i += 1;
  }
  return args;
}

function ensureDir(filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
}

function readTextIfExists(filePath) {
  try {
    if (!fs.existsSync(filePath)) {
      return "";
    }
    return fs.readFileSync(filePath, "utf-8").trim();
  } catch {
    return "";
  }
}

function normalizeCookieString(cookieString) {
  return String(cookieString || "")
    .split(";")
    .map((item) => item.trim())
    .filter(Boolean)
    .filter((item) => {
      const separator = item.indexOf("=");
      return separator > 0 && item.slice(separator + 1).trim();
    })
    .join("; ");
}

function parseCookieEntries(cookieString) {
  return normalizeCookieString(cookieString)
    .split(";")
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item) => {
      const separator = item.indexOf("=");
      return [item.slice(0, separator).trim(), item.slice(separator + 1).trim()];
    })
    .filter(([name, value]) => name && value);
}

function mergeCookieSources(cookieStrings) {
  const merged = new Map();
  for (const cookieString of cookieStrings) {
    for (const [name, value] of parseCookieEntries(cookieString)) {
      merged.set(name, value);
    }
  }
  return Array.from(merged.entries())
    .map(([name, value]) => `${name}=${value}`)
    .join("; ");
}

function getCookieValue(cookieString, cookieName) {
  for (const [name, value] of parseCookieEntries(cookieString)) {
    if (name === cookieName) {
      return value;
    }
  }
  return null;
}

function validateRequiredCookies(cookieString, requiredCookies) {
  const missing = (requiredCookies || []).filter((cookieName) => !getCookieValue(cookieString, cookieName));
  return {
    valid: missing.length === 0,
    missing,
  };
}

function resolveFromCwd(filePath) {
  if (path.isAbsolute(filePath)) {
    return filePath;
  }
  return path.resolve(process.cwd(), filePath);
}

function loadConfig(configPath) {
  const raw = fs.readFileSync(configPath, "utf-8");
  const config = JSON.parse(raw);
  if (!config.url || !config.domain || !config.outputFile) {
    throw new Error("配置缺少必要字段: url / domain / outputFile");
  }
  config.outputFile = resolveFromCwd(config.outputFile);
  if (config.mergeCookieFiles) {
    config.mergeCookieFiles = config.mergeCookieFiles.map(resolveFromCwd);
  }
  return config;
}

function printCookieSummary(cookieString, config) {
  const cookies = parseCookieEntries(cookieString);
  console.log(`   Cookie 数量: ${cookies.length}`);
  if ((config.requiredCookies || []).length > 0) {
    console.log("   关键 Cookie:");
    for (const cookieName of config.requiredCookies) {
      const value = getCookieValue(cookieString, cookieName);
      const preview = value ? (value.length > 30 ? `${value.slice(0, 30)}...` : value) : "<missing>";
      console.log(`      STAR ${cookieName}=${preview}`);
    }
  }
}

function startManualVerifier(prompt, onTrigger) {
  if (!process.stdin.isTTY) {
    return { stop() {} };
  }

  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: true,
  });

  let running = false;
  let stopped = false;
  rl.setPrompt(prompt);
  rl.prompt();

  rl.on("line", async () => {
    if (stopped) {
      return;
    }
    if (running) {
      console.log("⏳ 上一次检测还在执行，请稍候...");
      rl.prompt();
      return;
    }
    running = true;
    try {
      await onTrigger();
    } finally {
      running = false;
      if (!stopped) {
        rl.prompt();
      }
    }
  });

  return {
    stop() {
      if (stopped) {
        return;
      }
      stopped = true;
      rl.close();
    },
  };
}

function findSystemChrome() {
  const candidates =
    process.platform === "darwin"
      ? [
          "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
          "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
          "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
      : process.platform === "win32"
        ? [
            `${process.env.PROGRAMFILES}\\Google\\Chrome\\Application\\chrome.exe`,
            `${process.env["PROGRAMFILES(X86)"]}\\Google\\Chrome\\Application\\chrome.exe`,
            `${process.env.LOCALAPPDATA}\\Google\\Chrome\\Application\\chrome.exe`,
          ]
        : [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
          ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function fetchJson(url) {
  return new Promise((resolve, reject) => {
    http
      .get(url, (res) => {
        let data = "";
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          try {
            resolve(JSON.parse(data));
          } catch (e) {
            reject(e);
          }
        });
      })
      .on("error", reject);
  });
}

async function waitForDebugEndpoint(port, timeout = 30000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    try {
      const data = await fetchJson(`http://127.0.0.1:${port}/json/version`);
      if (data.webSocketDebuggerUrl) {
        return data.webSocketDebuggerUrl;
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`Chrome 远程调试端口 ${port} 未在 ${timeout}ms 内就绪`);
}

async function launchBrowser(puppeteer, config) {
  const chromePath = findSystemChrome();
  if (!chromePath) {
    throw new Error("未找到系统 Chrome 浏览器，请安装 Google Chrome");
  }

  const repoRoot = process.cwd();
  const userDataDir = path.resolve(repoRoot, ".env/.chrome-profile");
  fs.mkdirSync(userDataDir, { recursive: true });
  const debugPort = 19222 + Math.floor(Math.random() * 1000);

  console.log(`🌐 启动 Chrome (端口 ${debugPort})...`);
  const chromeProcess = spawn(
    chromePath,
    [
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${userDataDir}`,
      "--no-first-run",
      "--no-default-browser-check",
      config.url,
    ],
    { stdio: "ignore", detached: false },
  );

  chromeProcess.on("error", (err) => {
    throw new Error(`Chrome 启动失败: ${err.message}`);
  });

  const wsUrl = await waitForDebugEndpoint(debugPort);
  const browser = await puppeteer.connect({
    browserWSEndpoint: wsUrl,
    defaultViewport: null,
  });

  browser._chromeProcess = chromeProcess;
  return browser;
}

function loadMergedCookieString(config) {
  const files = [config.outputFile, ...(config.mergeCookieFiles || [])];
  return mergeCookieSources(files.map((filePath) => readTextIfExists(filePath)));
}

async function verifyWithFetch(cookieString, config) {
  if (!config.verify || !config.verify.url) {
    return { valid: true, reason: "no_verify_config" };
  }

  try {
    const response = await fetch(config.verify.url, {
      method: "GET",
      redirect: "follow",
      headers: {
        Accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        Cookie: cookieString,
        "User-Agent":
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
      },
    });
    const text = await response.text();
    const finalUrl = response.url || "";

    const rejectUrlIncludes = config.verify.rejectUrlIncludes || [];
    if (rejectUrlIncludes.some((item) => finalUrl.includes(String(item)))) {
      return { valid: false, reason: `redirect_to:${finalUrl}` };
    }

    const rejectBodyIncludes = config.verify.rejectBodyIncludes || [];
    if (rejectBodyIncludes.some((item) => text.toLowerCase().includes(String(item).toLowerCase()))) {
      return { valid: false, reason: "reject_body_matched" };
    }

    return { valid: true, reason: "ok" };
  } catch (error) {
    return { valid: false, reason: `fetch_error:${error.message}` };
  }
}

function startCookieListener(browser, config) {
  let latestCookie = "";
  const sessions = [];

  async function attachToPage(page) {
    try {
      const client = await page.createCDPSession();
      await client.send("Network.enable");
      sessions.push(client);

      client.on("Network.requestWillBeSentExtraInfo", (params) => {
        const cookie = (params.headers || {})["cookie"] || (params.headers || {})["Cookie"] || "";
        if (cookie && cookie.includes("user_session")) {
          latestCookie = cookie;
        }
      });

      client.on("Network.requestWillBeSent", (params) => {
        const url = params.request.url || "";
        if (url.includes(config.domain)) {
          const cookie =
            (params.request.headers || {})["cookie"] ||
            (params.request.headers || {})["Cookie"] ||
            "";
          if (cookie && cookie.includes("user_session")) {
            latestCookie = cookie;
          }
        }
      });
    } catch {}
  }

  (async () => {
    try {
      const pages = await browser.pages();
      for (const p of pages) {
        await attachToPage(p);
      }
    } catch {}
  })();

  browser.on("targetcreated", async (target) => {
    if (target.type() === "page") {
      try {
        const p = await target.page();
        if (p) await attachToPage(p);
      } catch {}
    }
  });

  return {
    getCookie() {
      return latestCookie;
    },
    async stop() {
      for (const s of sessions) {
        await s.detach().catch(() => {});
      }
    },
  };
}

async function waitForValidCookie(browser, config, listener) {
  const startedAt = Date.now();
  const timeout = Number(config.timeout || 300000);

  const checkOnce = async () => {
    const cookie = listener.getCookie();
    if (!cookie) {
      return { valid: false, reason: "no_cookie_captured" };
    }
    return await verifyWithFetch(cookie, config);
  };

  const manualVerifier = startManualVerifier(`[${config.domain}] 按 Enter 立即检测登录态 > `, async () => {
    console.log("\n🔍 手动触发检测...");
    const result = await checkOnce();
    console.log(`   检测结果: ${result.reason}`);
  });

  try {
    while (Date.now() - startedAt < timeout) {
      let browserAlive = true;
      try {
        const pages = await browser.pages();
        if (pages.length === 0) {
          browserAlive = false;
        }
      } catch {
        browserAlive = false;
      }

      const result = await checkOnce();
      console.log(`   检测结果: ${result.reason}`);
      if (result.valid) {
        return listener.getCookie();
      }

      if (!browserAlive) {
        console.log("⚠️  浏览器已关闭，最终 Cookie 验证未通过");
        return null;
      }

      await new Promise((resolve) => setTimeout(resolve, 3000));
    }
  } finally {
    manualVerifier.stop();
  }

  return null;
}

async function main() {
  const args = parseArgs(process.argv);
  if (!args.config) {
    throw new Error("缺少 --config");
  }

  const configPath = path.resolve(process.cwd(), args.config);
  const config = loadConfig(configPath);
  const puppeteer = loadPuppeteer();

  console.log("🚀 Site Cookie Login");
  console.log("====================");
  console.log(`站点: ${config.name || config.domain}`);
  console.log(`目标页面: ${config.url}`);
  console.log(`输出文件: ${config.outputFile}`);
  console.log("");

  let browser;
  let listener;
  try {
    browser = await launchBrowser(puppeteer, config);

    listener = startCookieListener(browser, config);

    await new Promise((r) => setTimeout(r, 3000));

    const pages = await browser.pages();
    const page =
      pages.find((p) => {
        try {
          return p.url().includes(config.domain);
        } catch {
          return false;
        }
      }) || pages[0];
    if (page) {
      page.on("framenavigated", (frame) => {
        if (frame === page.mainFrame()) {
          console.log(`📍 ${frame.url()}`);
        }
      });
    }

    const initialCookie = listener.getCookie();
    let finalCookies = null;

    if (initialCookie) {
      const check = await verifyWithFetch(initialCookie, config);
      console.log(`   初始检测: ${check.reason}`);
      if (check.valid) {
        console.log("✅ 当前浏览器已有可用登录态");
        finalCookies = initialCookie;
      }
    }

    if (!finalCookies) {
      console.log("\n🔐 请在浏览器中完成登录...");
      finalCookies = await waitForValidCookie(browser, config, listener);
      if (!finalCookies) {
        throw new Error("等待登录超时");
      }
    }

    finalCookies = normalizeCookieString(finalCookies);

    printCookieSummary(finalCookies, config);
    ensureDir(config.outputFile);
    fs.writeFileSync(config.outputFile, finalCookies);
    console.log(`\n✅ Cookie 已保存到 ${config.outputFile}`);
  } finally {
    if (listener) {
      await listener.stop();
    }
    if (browser) {
      const chromeProcess = browser._chromeProcess;
      try {
        await browser.disconnect();
      } catch {}
      if (chromeProcess) {
        try {
          chromeProcess.kill("SIGTERM");
        } catch {}
        await new Promise((r) => setTimeout(r, 500));
        try {
          chromeProcess.kill("SIGKILL");
        } catch {}
      }
    }
  }
}

main().catch((error) => {
  console.error(`❌ ${error.message}`);
  process.exit(1);
});
