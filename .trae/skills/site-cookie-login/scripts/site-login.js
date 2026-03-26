#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const readline = require("readline");

function loadPuppeteer() {
  try {
    return require("puppeteer");
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

async function getCookiesFromBrowser(page, domain) {
  const client = await page.target().createCDPSession();
  const { cookies } = await client.send("Network.getAllCookies");
  return cookies
    .filter((cookie) => cookie.domain.includes(domain) || domain.includes(cookie.domain.replace(/^\./, "")))
    .map((cookie) => `${cookie.name}=${cookie.value}`)
    .join("; ");
}

function loadMergedCookieString(config) {
  const files = [config.outputFile, ...(config.mergeCookieFiles || [])];
  return mergeCookieSources(files.map((filePath) => readTextIfExists(filePath)));
}

async function verifyLoginWithCookie(cookieString, config) {
  const normalized = normalizeCookieString(cookieString);
  if (!normalized) {
    return { valid: false, reason: "no_cookie" };
  }

  const requiredCheck = validateRequiredCookies(normalized, config.requiredCookies || []);
  if (!requiredCheck.valid) {
    return {
      valid: false,
      reason: `missing_cookie:${requiredCheck.missing.join(",")}`,
    };
  }

  if (!config.verify || !config.verify.url) {
    return { valid: true, reason: "no_verify_config" };
  }

  const headers = {
    ...(config.verify.headers || {}),
    Cookie: normalized,
  };

  const requestConfig = {
    method: config.verify.method || "GET",
    headers,
  };

  if (config.verify.body !== undefined) {
    requestConfig.body = JSON.stringify(config.verify.body);
  }

  try {
    const response = await fetch(config.verify.url, requestConfig);
    const text = await response.text();
    const bodyLower = text.toLowerCase();
    const successStatuses = config.verify.successStatuses || [200];
    const rejectBodyIncludes = config.verify.rejectBodyIncludes || [];
    const successBodyIncludes = config.verify.successBodyIncludes || [];

    const statusOk = successStatuses.includes(response.status);
    const rejectMatched = rejectBodyIncludes.some((item) => bodyLower.includes(String(item).toLowerCase()));
    const successMatched =
      successBodyIncludes.length === 0 ||
      successBodyIncludes.some((item) => bodyLower.includes(String(item).toLowerCase()));

    return {
      valid: statusOk && !rejectMatched && successMatched,
      reason: statusOk && !rejectMatched && successMatched ? "ok" : `http_${response.status}`,
      bodyPreview: text.slice(0, 200),
    };
  } catch (error) {
    return {
      valid: false,
      reason: error.message,
    };
  }
}

async function waitForLogin(page, config) {
  let success = false;
  const startedAt = Date.now();
  const timeout = Number(config.timeout || 300000);

  const checkOnce = async () => {
    const browserCookies = await getCookiesFromBrowser(page, config.domain);
    const mergedCookies = mergeCookieSources([browserCookies, loadMergedCookieString(config)]);
    const result = await verifyLoginWithCookie(mergedCookies, config);
    console.log(`   检测结果: ${result.reason}`);
    if (result.valid) {
      success = true;
    }
  };

  const manualVerifier = startManualVerifier(`[${config.domain}] 按 Enter 立即检测登录态 > `, async () => {
    console.log("\n🔍 手动触发检测...");
    await checkOnce();
  });

  try {
    while (!success && Date.now() - startedAt < timeout) {
      if (page.isClosed()) {
        throw new Error("浏览器页面已关闭，无法继续等待登录");
      }
      await checkOnce();
      if (success) {
        return true;
      }
      await new Promise((resolve) => setTimeout(resolve, 3000));
    }
  } finally {
    manualVerifier.stop();
  }

  return success;
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
  try {
    browser = await puppeteer.launch({
      headless: false,
      defaultViewport: null,
      args: ["--start-maximized", "--no-sandbox", "--disable-setuid-sandbox"],
    });

    const page = await browser.newPage();
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) {
        console.log(`📍 ${frame.url()}`);
      }
    });

    await page.goto(config.url, { waitUntil: "networkidle2", timeout: 60000 });

    const immediateCookies = mergeCookieSources([
      await getCookiesFromBrowser(page, config.domain),
      loadMergedCookieString(config),
    ]);
    const immediateCheck = await verifyLoginWithCookie(immediateCookies, config);

    if (!immediateCheck.valid) {
      console.log("\n🔐 请在浏览器中完成登录...");
      const loggedIn = await waitForLogin(page, config);
      if (!loggedIn) {
        throw new Error("等待登录超时");
      }
    } else {
      console.log("✅ 当前浏览器已有可用登录态");
    }

    const finalCookies = normalizeCookieString(
      mergeCookieSources([
        await getCookiesFromBrowser(page, config.domain),
        loadMergedCookieString(config),
      ]),
    );
    const finalCheck = await verifyLoginWithCookie(finalCookies, config);
    if (!finalCheck.valid) {
      throw new Error(`登录完成后校验失败: ${finalCheck.reason}`);
    }

    printCookieSummary(finalCookies, config);
    ensureDir(config.outputFile);
    fs.writeFileSync(config.outputFile, finalCookies);
    console.log(`\n✅ Cookie 已保存到 ${config.outputFile}`);
  } finally {
    if (browser) {
      try {
        const browserProcess = browser.process();
        if (browserProcess) {
          browserProcess.kill("SIGKILL");
        } else {
          await browser.close();
        }
      } catch {}
    }
  }
}

main().catch((error) => {
  console.error(`❌ ${error.message}`);
  process.exit(1);
});
