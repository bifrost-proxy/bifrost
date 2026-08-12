#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

import { PublishError, parseArticle } from "./juejin-publish.mjs";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, "../../../..");
const TITLE_SELECTORS = ["h1.article-title", ".article-title", "main h1", "article h1", "h1"];
const BODY_SELECTORS = ["article .article-content", ".article-content", ".markdown-body", "article"];

function decodeEntity(value) {
  return value
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
}

export function markdownToText(markdown) {
  const source = decodeEntity(String(markdown ?? ""))
    .replace(/<!--[^]*?-->/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/<((?:https?:\/\/|mailto:)[^>]+)>/g, "$1")
    .replace(/<[^>]+>/g, " ");
  const renderedLines = [];
  let fence = null;
  for (const originalLine of source.split("\n")) {
    const fenceMatch = originalLine.match(/^\s*(`{3,}|~{3,})/);
    if (fenceMatch) {
      if (!fence) fence = fenceMatch[1][0];
      else if (fence === fenceMatch[1][0]) fence = null;
      continue;
    }
    if (fence) {
      renderedLines.push(originalLine);
      continue;
    }
    let line = originalLine
      .replace(/^\s{0,3}#{1,6}\s+/, "")
      .replace(/^\s*>\s?/, "")
      .replace(/^\s*(?:[-+*]|\d+[.)])\s+/, "");
    if (/^\s*\|?(?:\s*:?-{3,}:?\s*\|)+\s*$/.test(line) || /^\s*-{3,}\s*$/.test(line)) {
      continue;
    }
    line = line
      .replace(/`([^`]*)`/g, "$1")
      .replace(/\*\*([^*]+)\*\*/g, "$1")
      .replace(/__([^_]+)__/g, "$1")
      .replace(/~~([^~]+)~~/g, "$1")
      .replace(/(^|\s)\*([^*\n]+)\*(?=\s|[，。！？,.!?]|$)/g, "$1$2")
      .replace(/(^|\s)_([^_\n]+)_(?=\s|[，。！？,.!?]|$)/g, "$1$2")
      .replace(/\|/g, " ");
    renderedLines.push(line);
  }
  return renderedLines.join("\n").replace(/\s+/g, " ").trim();
}

export function pageTextToText(value) {
  return decodeEntity(String(value ?? ""))
    .replace(/[\u200B-\u200D\uFEFF]/g, "")
    .replace(/^\s*(?:[•·]|[-+*]|\d+[.)])\s+/gm, "")
    .replace(/^\s*(?:复制代码|已复制)\s*$/gm, "")
    .replace(/\s+/g, " ")
    .trim();
}

export function normalizeVerificationUrl(value, allowLoopback = false) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new PublishError("--url 必须是有效 URL");
  }
  const production = url.protocol === "https:"
    && ["juejin.cn", "www.juejin.cn"].includes(url.hostname)
    && /^\/post\/\d+\/?$/.test(url.pathname);
  const loopback = allowLoopback
    && ["http:", "https:"].includes(url.protocol)
    && ["127.0.0.1", "::1", "localhost"].includes(url.hostname)
    && /^\/post\/\d+\/?$/.test(url.pathname);
  if (!production && !loopback) {
    throw new PublishError("只允许验证 https://juejin.cn/post/<数字文章ID>；本机 mock 需显式传 --allow-loopback");
  }
  url.search = "";
  url.hash = "";
  return url.toString();
}

export function urlFromArticleId(value) {
  const articleId = String(value ?? "").trim();
  if (!/^\d+$/.test(articleId) || articleId === "0") {
    throw new PublishError("文章 ID 必须是非零数字");
  }
  return `https://juejin.cn/post/${articleId}`;
}

function normalizeCloseDelayMs(value) {
  const delay = value ?? 5_000;
  if (!Number.isInteger(delay) || delay < 0 || delay > 60_000) {
    throw new PublishError("--close-delay-ms 必须是 0 到 60000 之间的整数");
  }
  return delay;
}

function normalizeRefreshDelayMs(value) {
  const delay = value ?? 3_000;
  if (!Number.isInteger(delay) || delay < 0 || delay > 60_000) {
    throw new PublishError("--refresh-delay-ms 必须是 0 到 60000 之间的整数");
  }
  return delay;
}

function normalizeRefreshCount(value) {
  const count = value ?? 0;
  if (!Number.isInteger(count) || count < 0 || count > 1000) {
    throw new PublishError("--refresh 次数必须是 1 到 1000 之间的整数");
  }
  return count;
}

export function compareArticlePage(article, snapshot) {
  const expectedTitle = pageTextToText(article.title);
  const actualTitle = pageTextToText(snapshot.title);
  const expectedBody = markdownToText(article.markdown);
  const actualBody = pageTextToText(snapshot.body);
  const titleMatch = actualTitle === expectedTitle;
  const bodyMatch = actualBody === expectedBody;
  const unexpectedDuplicateTitle = !expectedBody.startsWith(expectedTitle)
    && actualBody.startsWith(expectedTitle);
  let firstDifference = null;
  if (!bodyMatch) {
    const limit = Math.min(expectedBody.length, actualBody.length);
    let index = 0;
    while (index < limit && expectedBody[index] === actualBody[index]) index += 1;
    firstDifference = {
      index,
      expected: expectedBody.slice(Math.max(0, index - 40), index + 80),
      actual: actualBody.slice(Math.max(0, index - 40), index + 80),
    };
  }
  return {
    matches: titleMatch && bodyMatch && !unexpectedDuplicateTitle,
    checks: {
      title_match: titleMatch,
      body_match: bodyMatch,
      unexpected_duplicate_title: unexpectedDuplicateTitle,
    },
    expected: {
      title: article.title,
      body_characters: expectedBody.length,
    },
    actual: {
      title: snapshot.title,
      body_characters: actualBody.length,
      title_selector: snapshot.titleSelector,
      body_selector: snapshot.bodySelector,
    },
    first_difference: firstDifference,
  };
}

export function parseArgs(argv) {
  const options = {
    article: null,
    articleId: null,
    url: null,
    headless: false,
    allowLoopback: false,
    timeoutMs: 30_000,
    closeDelayMs: 5_000,
    refreshCount: 0,
    refreshDelayMs: 3_000,
  };
  const valueAfter = (index, name) => {
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) throw new PublishError(name + " 缺少参数值");
    return value;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--article") options.article = valueAfter(index++, arg);
    else if (arg === "--article-id") options.articleId = valueAfter(index++, arg);
    else if (arg === "--url") options.url = valueAfter(index++, arg);
    else if (arg === "--headless") options.headless = true;
    else if (arg === "--allow-loopback") options.allowLoopback = true;
    else if (arg === "--refresh") {
      const next = argv[index + 1];
      options.refreshCount = 1;
      if (next && !next.startsWith("--")) {
        if (!/^\d+$/.test(next)) throw new PublishError("--refresh 次数必须是 1 到 10 之间的整数");
        options.refreshCount = Number(next);
        index += 1;
      }
      if (options.refreshCount < 1 || options.refreshCount > 1000) {
        throw new PublishError("--refresh 次数必须是 1 到 1000 之间的整数");
      }
    }
    else if (arg === "--timeout-ms") options.timeoutMs = Number(valueAfter(index++, arg));
    else if (arg === "--close-delay-ms") options.closeDelayMs = Number(valueAfter(index++, arg));
    else if (arg === "--refresh-delay-ms") options.refreshDelayMs = Number(valueAfter(index++, arg));
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (/^\d+$/.test(arg) && !options.articleId) options.articleId = arg;
    else throw new PublishError("未知参数: " + arg);
  }
  if (options.help) return options;
  if (options.articleId && options.url) throw new PublishError("文章 ID 与 --url 不能同时使用");
  if (!options.articleId && !options.url) throw new PublishError("缺少文章 ID");
  if (options.articleId) options.url = urlFromArticleId(options.articleId);
  if (!Number.isInteger(options.timeoutMs) || options.timeoutMs < 1_000 || options.timeoutMs > 120_000) {
    throw new PublishError("--timeout-ms 必须是 1000 到 120000 之间的整数");
  }
  options.closeDelayMs = normalizeCloseDelayMs(options.closeDelayMs);
  options.refreshDelayMs = normalizeRefreshDelayMs(options.refreshDelayMs);
  options.refreshCount = normalizeRefreshCount(options.refreshCount);
  return options;
}

function help() {
  return `Usage:
  juejin-verify 7672957997422346303
  juejin-verify --article-id 7672957997422346303 --article /absolute/article.md

Options:
  --article-id ID      要打开的掘金文章 ID；也可直接作为第一个位置参数
  --article PATH       可选；同时严格比较页面标题/正文与本地 Markdown
  --headless           使用无界面 Edge（仅用于自动化测试）
  --timeout-ms N       页面等待超时，默认 30000
  --close-delay-ms N   加载成功后延迟关闭 Edge，默认 5000
  --refresh [N]        刷新 N 次并仅检查最终页面；省略 N 时默认 1，最多 10
  --refresh-delay-ms N 每次刷新前等待时间，默认 3000
  --url URL            测试专用的完整文章 URL
  --allow-loopback     允许 http(s)://127.0.0.1/post/<id> 本机 mock

The verifier opens Microsoft Edge and navigates once by default. --refresh [N]
performs 1-10 bounded reloads and checks the final page once; it never reads
Bifrost traffic, captures cookies, or replays requests.
`;
}

function loadPlaywright() {
  const candidates = [
    path.join(REPO_ROOT, "web", "package.json"),
    path.join(process.cwd(), "web", "package.json"),
  ];
  for (const candidate of candidates) {
    if (!fs.existsSync(candidate)) continue;
    try {
      return createRequire(candidate)("playwright");
    } catch {
      // Try the next repository-relative install.
    }
  }
  try {
    return createRequire(import.meta.url)("playwright");
  } catch {
    throw new PublishError("未找到 Playwright；请先执行 pnpm --dir web install");
  }
}

async function readPage(page, timeoutMs) {
  await page.waitForFunction(
    ({ titleSelectors, bodySelectors }) => {
      const title = titleSelectors.map((selector) => document.querySelector(selector)).find(Boolean);
      const body = bodySelectors.map((selector) => document.querySelector(selector)).find(Boolean);
      return Boolean(title?.textContent?.trim() && body?.textContent?.trim());
    },
    { titleSelectors: TITLE_SELECTORS, bodySelectors: BODY_SELECTORS },
    { timeout: timeoutMs },
  );
  return page.evaluate(({ titleSelectors, bodySelectors }) => {
    const titleSelector = titleSelectors.find((selector) => document.querySelector(selector));
    const bodySelector = bodySelectors.find((selector) => document.querySelector(selector));
    const titleNode = document.querySelector(titleSelector);
    const bodyNode = document.querySelector(bodySelector);
    return {
      title: titleNode?.innerText ?? "",
      body: bodyNode?.innerText ?? "",
      titleSelector,
      bodySelector,
    };
  }, { titleSelectors: TITLE_SELECTORS, bodySelectors: BODY_SELECTORS });
}

export async function verifyWithEdge(options) {
  const articlePath = options.article ? path.resolve(options.article) : null;
  const article = articlePath ? parseArticle(fs.readFileSync(articlePath, "utf8")) : null;
  const url = normalizeVerificationUrl(options.url, options.allowLoopback);
  const articleId = new URL(url).pathname.split("/").filter(Boolean).at(-1);
  const closeDelayMs = normalizeCloseDelayMs(options.closeDelayMs);
  const refreshCount = normalizeRefreshCount(options.refreshCount);
  const refreshDelayMs = normalizeRefreshDelayMs(options.refreshDelayMs);
  const reportProgress = typeof options.onProgress === "function" ? options.onProgress : () => {};
  const { chromium } = loadPlaywright();
  const browser = await chromium.launch({ channel: "msedge", headless: options.headless });
  try {
    const page = await browser.newPage();
    const response = await page.goto(url, { waitUntil: "domcontentloaded", timeout: options.timeoutMs });
    const httpStatus = response?.status() ?? 0;
    if (!response || httpStatus >= 400) {
      throw new PublishError("文章页面加载失败（HTTP " + httpStatus + "）");
    }
    const attempts = [];
    for (let index = 1; index <= refreshCount; index += 1) {
      reportProgress(`waiting ${refreshDelayMs}ms before refresh ${index}/${refreshCount}`);
      if (refreshDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, refreshDelayMs));
      }
      let refreshResponse;
      try {
        refreshResponse = await page.reload({ waitUntil: "domcontentloaded", timeout: options.timeoutMs });
      } catch {
        attempts.push({
          index,
          http_status: 0,
          error: "refresh_navigation_failed",
        });
        reportProgress(`refresh ${index}/${refreshCount} failed: refresh_navigation_failed`);
        break;
      }
      const refreshStatus = refreshResponse?.status() ?? 0;
      if (!refreshResponse || refreshStatus >= 400) {
        attempts.push({
          index,
          http_status: refreshStatus,
          error: "refresh_http_failed",
        });
        reportProgress(`refresh ${index}/${refreshCount} failed: refresh_http_failed (HTTP ${refreshStatus})`);
        break;
      }
      attempts.push({ index, http_status: refreshStatus });
      reportProgress(`refresh ${index}/${refreshCount} complete`);
    }
    const refreshComplete = attempts.length === refreshCount
      && attempts.every((attempt) => !attempt.error);
    const snapshot = await readPage(page, options.timeoutMs);
    reportProgress("final content check complete");
    const refresh = {
      requested: refreshCount,
      performed: attempts.length,
      delay_ms: refreshDelayMs,
      complete: refreshComplete,
      attempts,
    };
    const base = {
      browser: {
        name: "Microsoft Edge",
        version: browser.version(),
        mode: options.headless ? "headless" : "headed",
      },
      navigation_count: 1 + attempts.length,
      article_id: articleId,
      url,
      http_status: httpStatus,
      close_delay_ms: closeDelayMs,
      refresh,
    };
    let result;
    if (!article) {
      result = {
        status: refreshComplete ? "loaded" : "unstable",
        loaded: true,
        ...base,
        actual: {
          title: snapshot.title,
          body_characters: pageTextToText(snapshot.body).length,
          title_selector: snapshot.titleSelector,
          body_selector: snapshot.bodySelector,
        },
      };
    } else {
      const comparison = compareArticlePage(article, snapshot);
      result = {
        status: !refreshComplete ? "unstable" : comparison.matches ? "verified" : "mismatch",
        loaded: true,
        ...base,
        article: articlePath,
        ...comparison,
        matches: comparison.matches && refreshComplete,
      };
    }
    if (closeDelayMs > 0) {
      reportProgress(`waiting ${closeDelayMs}ms before closing Edge`);
      await new Promise((resolve) => setTimeout(resolve, closeDelayMs));
    }
    return result;
  } finally {
    await browser.close();
  }
}

async function main() {
  try {
    const options = parseArgs(process.argv.slice(2));
    if (options.help) {
      process.stdout.write(help());
      return;
    }
    const result = await verifyWithEdge({
      ...options,
      onProgress: (message) => process.stderr.write(`[juejin-verify] ${message}\n`),
    });
    process.stdout.write(JSON.stringify(result, null, 2) + "\n");
    if (!["loaded", "verified"].includes(result.status)) process.exitCode = 1;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write("juejin-verify: " + message + "\n");
    process.exitCode = 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  await main();
}
