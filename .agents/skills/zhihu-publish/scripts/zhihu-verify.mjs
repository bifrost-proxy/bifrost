#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  BifrostClient,
  PublishError,
  extractHtmlImages,
  filteredHeaders,
  isZhihuImageUrl,
  parseArticle,
} from "./zhihu-publish.mjs";

function decodeEntities(value) {
  const named = {
    amp: "&", apos: "'", gt: ">", lt: "<", nbsp: " ", quot: '"',
  };
  return String(value).replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (match, entity) => {
    const lower = entity.toLowerCase();
    if (lower.startsWith("#x")) return String.fromCodePoint(Number.parseInt(lower.slice(2), 16));
    if (lower.startsWith("#")) return String.fromCodePoint(Number.parseInt(lower.slice(1), 10));
    return named[lower] ?? match;
  });
}

function normalizeVisibleText(value) {
  return String(value)
    .replace(/[\u200B-\u200D\uFEFF]/g, "")
    .replace(/\s+/g, " ")
    .replace(/\s+([，。！？；：、,.!?;:])/g, "$1")
    .trim();
}

export function htmlToText(html) {
  return normalizeVisibleText(decodeEntities(String(html ?? ""))
    .replace(/<!--[^]*?-->/g, " ")
    .replace(/<img\b[^>]*>/gi, " ")
    .replace(/<(?:br|hr)\s*\/?>/gi, " ")
    .replace(/<\/(?:p|div|h[1-6]|li|blockquote|pre|tr|section|article)>/gi, " ")
    .replace(/<[^>]+>/g, " ")
  );
}

export function normalizeArticleId(value) {
  const id = String(value ?? "").trim();
  if (!/^\d+$/.test(id) || id === "0") throw new PublishError("文章 ID 必须是非零数字");
  return id;
}

export function normalizeVerifyBase(value, allowLoopback = false) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new PublishError("--base 必须是有效 URL");
  }
  const production = url.protocol === "https:" && url.hostname === "zhuanlan.zhihu.com";
  const loopback = allowLoopback
    && ["http:", "https:"].includes(url.protocol)
    && ["127.0.0.1", "::1", "localhost"].includes(url.hostname);
  if (!production && !loopback) {
    throw new PublishError("只允许验证 zhuanlan.zhihu.com；本机 mock 需显式传 --allow-loopback");
  }
  url.pathname = url.pathname.replace(/\/$/, "") || "/";
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/, "");
}

export function compareArticle(article, remote) {
  const expectedTitle = normalizeVisibleText(article.title);
  const actualTitle = normalizeVisibleText(remote.title);
  const expectedBody = normalizeVisibleText(article.text);
  const actualBody = htmlToText(remote.content);
  const titleMatch = expectedTitle === actualTitle;
  const bodyMatch = expectedBody === actualBody;
  const duplicatedTitle = !expectedBody.startsWith(expectedTitle) && actualBody.startsWith(expectedTitle);
  const expectedImages = extractHtmlImages(article.html);
  const actualImages = extractHtmlImages(remote.content);
  const imageCountMatch = expectedImages.length === actualImages.length;
  const imagesHostedByZhihu = actualImages.every(({ src }) => isZhihuImageUrl(src));
  const actualImageHosts = [...new Set(actualImages.map(({ src }) => {
    try {
      return new URL(src).hostname;
    } catch {
      return "invalid";
    }
  }))].sort();
  let firstDifference = null;
  if (!bodyMatch) {
    let index = 0;
    const limit = Math.min(expectedBody.length, actualBody.length);
    while (index < limit && expectedBody[index] === actualBody[index]) index += 1;
    firstDifference = {
      index,
      expected: expectedBody.slice(Math.max(0, index - 40), index + 80),
      actual: actualBody.slice(Math.max(0, index - 40), index + 80),
    };
  }
  return {
    matches: titleMatch && bodyMatch && !duplicatedTitle && imageCountMatch && imagesHostedByZhihu,
    checks: {
      title_match: titleMatch,
      body_match: bodyMatch,
      unexpected_duplicate_title: duplicatedTitle,
      image_count_match: imageCountMatch,
      images_hosted_by_zhihu: imagesHostedByZhihu,
    },
    expected: {
      title: expectedTitle,
      body_characters: Array.from(expectedBody).length,
      image_count: expectedImages.length,
    },
    actual: {
      title: actualTitle,
      body_characters: Array.from(actualBody).length,
      image_count: actualImages.length,
      image_hosts: actualImageHosts,
    },
    first_difference: firstDifference,
  };
}

export async function verifyArticle(options, dependencies = {}) {
  const articleId = normalizeArticleId(options.articleId);
  const base = normalizeVerifyBase(options.base, options.allowLoopback);
  const article = options.article
    ? parseArticle(fs.readFileSync(path.resolve(options.article), "utf8"))
    : null;
  let context;
  if (options.allowLoopback && new URL(base).hostname !== "zhuanlan.zhihu.com") {
    context = { headers: new Map(), sourceId: "loopback", cookieNames: [] };
  } else {
    const bifrost = dependencies.bifrost ?? new BifrostClient();
    context = bifrost.findPublicContext();
    if (!context) throw new PublishError("Bifrost 中没有可用的知乎页面请求上下文");
  }
  const response = await fetch(new URL("/api/articles/" + articleId, base), {
    headers: filteredHeaders(context, { accept: "application/json" }),
    signal: AbortSignal.timeout(options.timeoutMs ?? 30_000),
  });
  if (!response.ok) throw new PublishError("知乎文章读取失败（HTTP " + response.status + "）");
  let remote;
  try {
    remote = await response.json();
  } catch {
    throw new PublishError("知乎文章接口返回非 JSON");
  }
  if (String(remote.id ?? articleId) !== articleId || !remote.title || typeof remote.content !== "string") {
    throw new PublishError("知乎文章接口响应结构已变化");
  }
  const result = {
    status: article ? "verified" : "loaded",
    article_id: articleId,
    url: "https://zhuanlan.zhihu.com/p/" + articleId,
    title: remote.title,
    source_id: context.sourceId,
  };
  if (!article) return result;
  const comparison = compareArticle(article, remote);
  if (!comparison.matches) {
    throw new PublishError("线上文章与本地 Markdown 不一致: " + JSON.stringify(comparison));
  }
  return { ...result, comparison };
}

export function parseArgs(argv) {
  const options = {
    articleId: null,
    article: null,
    base: process.env.ZHIHU_VERIFY_BASE || "https://zhuanlan.zhihu.com",
    allowLoopback: false,
    timeoutMs: 30_000,
  };
  const valueAfter = (index, flag) => {
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) throw new PublishError(flag + " 缺少参数值");
    return value;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (/^\d+$/.test(arg) && !options.articleId) options.articleId = arg;
    else if (arg === "--article-id") options.articleId = valueAfter(index++, arg);
    else if (arg === "--article") options.article = valueAfter(index++, arg);
    else if (arg === "--base") options.base = valueAfter(index++, arg);
    else if (arg === "--allow-loopback") options.allowLoopback = true;
    else if (arg === "--timeout-ms") options.timeoutMs = Number(valueAfter(index++, arg));
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new PublishError("未知参数: " + arg);
  }
  if (!options.help && !options.articleId) throw new PublishError("缺少文章 ID");
  if (!Number.isInteger(options.timeoutMs) || options.timeoutMs < 1_000 || options.timeoutMs > 120_000) {
    throw new PublishError("--timeout-ms 必须是 1000 到 120000 之间的整数");
  }
  return options;
}

function printHelp() {
  process.stdout.write(`Usage:
  zhihu-verify <article-id> --article /absolute/article.md
  zhihu-verify --article-id <id>

Options:
  --article PATH       严格比较线上标题/正文与本地 Markdown
  --timeout-ms N       请求超时，默认 30000
  --base URL           测试专用 API 根地址
  --allow-loopback     允许本机 mock 地址
`);
}

export async function main(argv = process.argv.slice(2), dependencies = {}) {
  const options = parseArgs(argv);
  if (options.help) {
    printHelp();
    return 0;
  }
  const result = await verifyArticle(options, dependencies);
  process.stdout.write(JSON.stringify(result, null, 2) + "\n");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write("[zhihu-verify] ERROR: " + (error instanceof Error ? error.message : String(error)) + "\n");
    process.exitCode = 1;
  });
}
