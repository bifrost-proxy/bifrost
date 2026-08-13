#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const COLUMN_BASE = "https://zhuanlan.zhihu.com";
const MAIN_BASE = "https://www.zhihu.com";
const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
const IMAGE_MIME_BY_EXTENSION = new Map([
  [".jpg", "image/jpeg"], [".jpeg", "image/jpeg"], [".png", "image/png"],
  [".gif", "image/gif"], [".webp", "image/webp"], [".bmp", "image/bmp"],
]);

export class PublishError extends Error {
  constructor(message, options = {}) {
    super(message);
    this.name = "PublishError";
    this.authFailure = Boolean(options.authFailure);
    this.status = options.status ?? null;
    this.code = options.code ?? null;
  }
}

function parseValue(raw) {
  const value = raw.trim();
  if (!value) return "";
  if (value === "true") return true;
  if (value === "false") return false;
  if (value === "null") return null;
  if (/^-?\d+(?:\.\d+)?$/.test(value)) return Number(value);
  if ((value.startsWith("[") && value.endsWith("]")) || (value.startsWith("{") && value.endsWith("}"))) {
    try {
      return JSON.parse(value);
    } catch (error) {
      throw new PublishError("frontmatter JSON 值无效: " + error.message);
    }
  }
  if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
    return value.slice(1, -1);
  }
  return value;
}

function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function safeUrl(value) {
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) ? url.toString() : null;
  } catch {
    return null;
  }
}

function imageSource(value) {
  const source = String(value).trim();
  if (!source) return null;
  if (/^https?:\/\//i.test(source)) return safeUrl(source);
  if (/^data:image\/(?:jpeg|png|gif|webp|bmp);base64,/i.test(source)) return source;
  if (/^[a-z][a-z\d+.-]*:/i.test(source)) return null;
  return source;
}

function renderInline(source) {
  const tokens = [];
  const hold = (html) => {
    const key = "\u0000" + tokens.length + "\u0000";
    tokens.push(html);
    return key;
  };
  let value = String(source);
  value = value.replace(/\x60([^\x60]+)\x60/g, (_, code) => hold("<code>" + escapeHtml(code) + "</code>"));
  value = value.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (_, alt, rawUrl) => {
    const source = imageSource(rawUrl);
    if (!source) throw new PublishError("图片地址仅支持 HTTP(S)、本地图片路径或 data image URI");
    return hold('<img src="' + escapeHtml(source) + '" alt="' + escapeHtml(alt) + '">');
  });
  value = value.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, label, rawUrl) => {
    const url = safeUrl(rawUrl.trim());
    return url ? hold('<a href="' + escapeHtml(url) + '">' + escapeHtml(label) + "</a>") : escapeHtml(label);
  });
  value = escapeHtml(value)
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/~~([^~]+)~~/g, "<del>$1</del>")
    .replace(/(^|\s)\*([^*\n]+)\*(?=\s|[，。！？,.!?]|$)/g, "$1<em>$2</em>")
    .replace(/(^|\s)_([^_\n]+)_(?=\s|[，。！？,.!?]|$)/g, "$1<em>$2</em>");
  return value.replace(/\u0000(\d+)\u0000/g, (_, index) => tokens[Number(index)]);
}

export function markdownToHtml(markdown) {
  const output = [];
  let fence = null;
  let language = "";
  let codeLines = [];
  let list = null;
  const closeList = () => {
    if (!list) return;
    output.push("</" + list + ">");
    list = null;
  };
  for (const rawLine of String(markdown).replace(/\r\n/g, "\n").split("\n")) {
    const fenceMatch = rawLine.match(/^\s*(\x60{3,}|~{3,})([^\s]*)\s*$/);
    if (fenceMatch) {
      closeList();
      if (!fence) {
        fence = fenceMatch[1][0];
        language = fenceMatch[2] || "";
        codeLines = [];
      } else if (fence === fenceMatch[1][0]) {
        const className = language ? ' class="language-' + escapeHtml(language) + '"' : "";
        output.push("<pre><code" + className + ">" + escapeHtml(codeLines.join("\n")) + "</code></pre>");
        fence = null;
        language = "";
        codeLines = [];
      }
      continue;
    }
    if (fence) {
      codeLines.push(rawLine);
      continue;
    }
    if (!rawLine.trim()) {
      closeList();
      continue;
    }
    const heading = rawLine.match(/^\s{0,3}(#{1,6})\s+(.+)$/);
    if (heading) {
      closeList();
      const level = heading[1].length;
      output.push("<h" + level + ">" + renderInline(heading[2]) + "</h" + level + ">");
      continue;
    }
    if (/^\s{0,3}(?:-{3,}|\*{3,}|_{3,})\s*$/.test(rawLine)) {
      closeList();
      output.push("<hr>");
      continue;
    }
    const unordered = rawLine.match(/^\s*[-+*]\s+(.+)$/);
    const ordered = rawLine.match(/^\s*\d+[.)]\s+(.+)$/);
    if (unordered || ordered) {
      const kind = unordered ? "ul" : "ol";
      if (list !== kind) {
        closeList();
        list = kind;
        output.push("<" + kind + ">");
      }
      output.push("<li>" + renderInline((unordered || ordered)[1]) + "</li>");
      continue;
    }
    const quote = rawLine.match(/^\s*>\s?(.*)$/);
    if (quote) {
      closeList();
      output.push("<blockquote><p>" + renderInline(quote[1]) + "</p></blockquote>");
      continue;
    }
    closeList();
    output.push("<p>" + renderInline(rawLine.trim()) + "</p>");
  }
  closeList();
  if (fence) throw new PublishError("Markdown 代码块未闭合");
  return output.join("\n");
}

export function markdownToText(markdown) {
  const codeTokens = [];
  const holdCode = (value) => {
    const token = "\u0000CODE" + codeTokens.length + "\u0000";
    codeTokens.push(value);
    return token;
  };
  const lines = String(markdown).replace(/\r\n/g, "\n").split("\n");
  const visibleLines = [];
  let fence = null;
  let codeLines = [];
  for (const line of lines) {
    const fenceMatch = line.match(/^\s*(\x60{3,}|~{3,})[^\n]*$/);
    if (fenceMatch && (!fence || fence === fenceMatch[1][0])) {
      if (!fence) {
        fence = fenceMatch[1][0];
        codeLines = [];
      } else {
        visibleLines.push(holdCode(codeLines.join("\n")));
        fence = null;
        codeLines = [];
      }
      continue;
    }
    if (fence) codeLines.push(line);
    else visibleLines.push(line);
  }
  let value = visibleLines.join("\n")
    .replace(/\x60([^\x60]*)\x60/g, (_, code) => holdCode(code))
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*>\s?/gm, "")
    .replace(/^\s*(?:[-+*]|\d+[.)])\s+/gm, "")
    .replace(/[*_~]/g, "");
  value = value.replace(/\u0000CODE(\d+)\u0000/g, (_, index) => codeTokens[Number(index)]);
  return value
    .replace(/\s+/g, " ")
    .trim();
}

export function parseArticle(source) {
  const normalized = String(source).replace(/\r\n/g, "\n");
  const match = normalized.match(/^---\n([\s\S]*?)\n---\n?/);
  if (!match) throw new PublishError("文章必须以 YAML frontmatter 开头");
  const metadata = {};
  for (const [index, line] of match[1].split("\n").entries()) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = line.indexOf(":");
    if (separator <= 0) throw new PublishError("frontmatter 第 " + (index + 2) + " 行缺少 key: value");
    metadata[line.slice(0, separator).trim()] = parseValue(line.slice(separator + 1));
  }
  const markdown = normalized.slice(match[0].length).trim();
  const title = String(metadata.title ?? "").trim();
  if (!title) throw new PublishError("frontmatter 缺少 title");
  if (title.length > 100) throw new PublishError("title 不能超过 100 个字符");
  if (!markdown) throw new PublishError("文章正文不能为空");
  const disclaimerType = String(metadata.disclaimer_type ?? "none").trim();
  const article = {
    title,
    markdown,
    html: markdownToHtml(markdown),
    text: markdownToText(markdown),
    commentPermission: String(metadata.comment_permission ?? "anyone").trim(),
    disclaimerType,
    disclaimerStatus: disclaimerType === "none" ? "close" : "open",
    tableOfContents: Boolean(metadata.table_of_contents ?? false),
    canReward: Boolean(metadata.can_reward ?? false),
  };
  if (!article.commentPermission) throw new PublishError("comment_permission 不能为空");
  return article;
}

function decodeHtmlAttribute(value) {
  return String(value)
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

export function extractHtmlImages(html) {
  const images = [];
  for (const match of String(html).matchAll(/<img\b[^>]*\bsrc=(?:"([^"]*)"|'([^']*)')[^>]*>/gi)) {
    images.push({ html: match[0], src: decodeHtmlAttribute(match[1] ?? match[2] ?? "") });
  }
  return images;
}

export function isZhihuImageUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && (
      url.hostname === "zhimg.com" || url.hostname.endsWith(".zhimg.com")
    );
  } catch {
    return false;
  }
}

export function isZhihuUploadImageUrl(value) {
  if (isZhihuImageUrl(value)) return true;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.hostname === "pic-private.zhihu.com";
  } catch {
    return false;
  }
}

function validateUploadedImageUrl(value) {
  const url = String(value ?? "").trim();
  if (!isZhihuUploadImageUrl(url)) {
    throw new PublishError("知乎图片上传响应缺少有效的 HTTPS 官方图片地址");
  }
  return url;
}

function uploadHeaders(context, overrides = {}) {
  const headers = filteredHeaders(context);
  for (const name of Object.keys(headers)) {
    if (name === "content-type" || name.startsWith("x-zse-") || name.startsWith("x-zst-")) delete headers[name];
  }
  return { ...headers, ...overrides };
}

export async function uploadRemoteImage(columnBase, context, source, fetchImpl = fetch) {
  const url = new URL(source);
  if (url.protocol !== "https:" && url.protocol !== "http:") throw new PublishError("远程图片必须使用 HTTP(S)");
  const response = await fetchImpl(new URL("/api/uploaded_images", columnBase), {
    method: "POST",
    headers: uploadHeaders(context, {
      accept: "application/json, text/plain, */*",
      "content-type": "application/x-www-form-urlencoded;charset=UTF-8",
      "x-requested-with": "fetch",
    }),
    body: new URLSearchParams({ url: url.toString(), source: "article" }),
    signal: AbortSignal.timeout(30_000),
  });
  const payload = await readJsonResponse(response, "转存知乎远程图片");
  return validateUploadedImageUrl(payload.src ?? payload.data?.src);
}

function sniffImageMime(buffer, fallback = "") {
  if (buffer.length >= 3 && buffer[0] === 0xff && buffer[1] === 0xd8 && buffer[2] === 0xff) return "image/jpeg";
  if (buffer.length >= 8 && buffer.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) return "image/png";
  if (buffer.length >= 6 && ["GIF87a", "GIF89a"].includes(buffer.subarray(0, 6).toString("ascii"))) return "image/gif";
  if (buffer.length >= 12 && buffer.subarray(0, 4).toString("ascii") === "RIFF" && buffer.subarray(8, 12).toString("ascii") === "WEBP") return "image/webp";
  if (buffer.length >= 2 && buffer.subarray(0, 2).toString("ascii") === "BM") return "image/bmp";
  return fallback;
}

function validateImageBuffer(buffer, expectedMime = "") {
  if (!Buffer.isBuffer(buffer) || buffer.length === 0) throw new PublishError("图片内容为空");
  if (buffer.length > MAX_IMAGE_BYTES) throw new PublishError("图片超过 20 MiB 限制");
  const mime = sniffImageMime(buffer);
  if (!mime || (expectedMime && mime !== expectedMime)) throw new PublishError("图片格式与内容不匹配或不受支持");
  return mime;
}

export async function uploadBinaryImage(columnBase, context, buffer, filename, expectedMime = "", fetchImpl = fetch) {
  const mime = validateImageBuffer(buffer, expectedMime);
  const form = new FormData();
  form.append("picture", new Blob([buffer], { type: mime }), filename || "image" + ([...IMAGE_MIME_BY_EXTENSION].find(([, value]) => value === mime)?.[0] ?? ""));
  form.append("source", "article");
  const response = await fetchImpl(new URL("/api/uploaded_images", columnBase), {
    method: "POST",
    headers: uploadHeaders(context, { accept: "application/json, text/plain, */*", "x-requested-with": "fetch" }),
    body: form,
    signal: AbortSignal.timeout(30_000),
  });
  const payload = await readJsonResponse(response, "上传知乎本地图片");
  return validateUploadedImageUrl(payload.src ?? payload.data?.src);
}

function loadImageSource(source, articlePath) {
  if (/^data:image\//i.test(source)) {
    const match = source.match(/^data:(image\/(?:jpeg|png|gif|webp|bmp));base64,([a-z\d+/=\s]+)$/i);
    if (!match) throw new PublishError("data image URI 必须是受支持格式的 base64 数据");
    return { buffer: Buffer.from(match[2].replace(/\s/g, ""), "base64"), filename: "inline", mime: match[1].toLowerCase() };
  }
  const filePath = path.isAbsolute(source) ? source : path.resolve(path.dirname(articlePath), source);
  const extension = path.extname(filePath).toLowerCase();
  const mime = IMAGE_MIME_BY_EXTENSION.get(extension);
  if (!mime) throw new PublishError("本地图片仅支持 JPEG、PNG、GIF、WebP 或 BMP: " + source);
  let stat;
  try {
    stat = fs.statSync(filePath);
  } catch (error) {
    throw new PublishError("无法读取本地图片 " + source + ": " + error.message);
  }
  if (!stat.isFile()) throw new PublishError("本地图片路径不是文件: " + source);
  if (stat.size > MAX_IMAGE_BYTES) throw new PublishError("图片超过 20 MiB 限制: " + source);
  return { buffer: fs.readFileSync(filePath), filename: path.basename(filePath), mime };
}

export function summarizeArticleImages(article) {
  const summary = { image_count: 0, remote_images: 0, local_images: 0, data_images: 0, zhihu_images: 0 };
  for (const { src } of extractHtmlImages(article.html)) {
    summary.image_count += 1;
    if (isZhihuUploadImageUrl(src)) summary.zhihu_images += 1;
    else if (/^https?:\/\//i.test(src)) summary.remote_images += 1;
    else if (/^data:image\//i.test(src)) summary.data_images += 1;
    else summary.local_images += 1;
  }
  return summary;
}

export async function processArticleImages(article, articlePath, columnBase, context, fetchImpl = fetch) {
  const images = extractHtmlImages(article.html);
  const replacements = new Map();
  for (const { src } of images) {
    if (replacements.has(src)) continue;
    if (isZhihuImageUrl(src)) replacements.set(src, src);
    else if (/^https?:\/\//i.test(src)) replacements.set(src, await uploadRemoteImage(columnBase, context, src, fetchImpl));
    else {
      const local = loadImageSource(src, articlePath);
      replacements.set(src, await uploadBinaryImage(columnBase, context, local.buffer, local.filename, local.mime, fetchImpl));
    }
  }
  let html = article.html.replace(/<img\b[^>]*\bsrc=(?:"([^"]*)"|'([^']*)')[^>]*>/gi, (tag, doubleSrc, singleSrc) => {
    const original = decodeHtmlAttribute(doubleSrc ?? singleSrc ?? "");
    const replacement = replacements.get(original);
    if (!replacement) throw new PublishError("图片处理结果缺失");
    return tag.replace(/\bsrc=(?:"[^"]*"|'[^']*')/i, 'src="' + escapeHtml(replacement) + '"');
  });
  html = html.replace(/<p>\s*(<img\b[^>]*>)\s*<\/p>/gi, '<figure data-size="normal">$1</figure>');
  return { ...article, html, imageSummary: summarizeArticleImages({ html }) };
}

function headerMap(headers) {
  const map = new Map();
  for (const item of headers ?? []) {
    if (Array.isArray(item) && item.length >= 2) map.set(String(item[0]).toLowerCase(), String(item[1]));
  }
  return map;
}

export function cookieNames(cookie) {
  return String(cookie ?? "").split(";").map((part) => part.trim().split("=", 1)[0]).filter(Boolean).sort();
}

export function extractContext(traffic) {
  const headers = headerMap(traffic.request_headers);
  const cookie = headers.get("cookie") ?? "";
  const xsrf = headers.get("x-xsrftoken") ?? "";
  if (!cookie || !xsrf) throw new PublishError("Bifrost 流量缺少知乎 Cookie 或 x-xsrftoken");
  const names = cookieNames(cookie);
  if (!names.includes("z_c0") || !names.includes("_xsrf")) throw new PublishError("知乎登录 Cookie 不完整");
  return { sourceId: traffic.id, headers, cookieNames: names };
}

const DROPPED_HEADERS = new Set([
  "accept-encoding", "connection", "content-length", "host", "http2-settings",
  "proxy-authorization", "proxy-connection", "te", "transfer-encoding", "upgrade",
]);

export function filteredHeaders(context, overrides = {}) {
  const result = {};
  for (const [name, value] of context.headers.entries()) {
    const lower = String(name).toLowerCase();
    if (!DROPPED_HEADERS.has(lower) && !lower.startsWith(":")) result[lower] = String(value);
  }
  return { ...result, ...overrides };
}

export function normalizeApiBase(value, kind = "column") {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new PublishError("API base 必须是有效 URL");
  }
  const expectedHost = kind === "main" ? "www.zhihu.com" : "zhuanlan.zhihu.com";
  const production = url.protocol === "https:" && url.hostname === expectedHost;
  const loopback = ["127.0.0.1", "::1", "localhost"].includes(url.hostname)
    && ["http:", "https:"].includes(url.protocol);
  if (!production && !loopback) {
    throw new PublishError("拒绝把知乎登录态发送到非 " + expectedHost + " 或非本机 API 地址");
  }
  url.pathname = url.pathname.replace(/\/$/, "") || "/";
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/, "");
}

export function isAuthFailure(status, code, message = "") {
  if (status === 401 || status === 403) return true;
  if ([401, 403, 10001, 10002, 10003].includes(Number(code))) return true;
  return /(login|logged|auth|token|session|cookie|登录|鉴权|未登录|登录态)/i.test(String(message));
}

export class BifrostClient {
  constructor(binary = process.env.BIFROST_BIN || "bifrost") {
    this.binary = binary;
  }

  run(args) {
    const result = spawnSync(this.binary, args, {
      encoding: "utf8",
      env: process.env,
      maxBuffer: 32 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
    if (result.error) throw new PublishError("无法执行 Bifrost: " + result.error.message);
    if (result.status !== 0) {
      throw new PublishError("Bifrost 命令失败（exit " + result.status + "）；为避免泄露登录信息，未回显命令输出");
    }
    return String(result.stdout || "").trim();
  }

  search(host, pathPart, method, limit = 50) {
    const parsed = JSON.parse(this.run([
      "search", pathPart, "--url", "--host", host, "--path", pathPart,
      "--method", method, "--latest", "30d", "--limit", String(limit), "--format", "json",
    ]));
    return (parsed.results ?? []).sort((left, right) =>
      Number(right.seq ?? right.sequence ?? 0) - Number(left.seq ?? left.sequence ?? 0));
  }

  getTraffic(id) {
    const args = ["traffic", "get", String(id)];
    args.push("--format", "json");
    return JSON.parse(this.run(args));
  }

  latestContext(host, pathPart, method, predicate = () => true) {
    for (const candidate of this.search(host, pathPart, method)) {
      if (!predicate(candidate)) continue;
      try {
        return extractContext(this.getTraffic(candidate.id));
      } catch {
        // Skip incomplete or expired captures.
      }
    }
    return null;
  }

  findContexts() {
    const create = this.latestContext(
      "zhuanlan.zhihu.com", "/api/articles/drafts", "POST",
      (item) => item.path === "/api/articles/drafts",
    );
    const update = this.latestContext(
      "zhuanlan.zhihu.com", "/draft", "PATCH",
      (item) => /^\/api\/articles\/\d+\/draft$/.test(item.path),
    );
    const publish = this.latestContext(
      "www.zhihu.com", "/api/v4/content/publish", "POST",
      (item) => item.path === "/api/v4/content/publish",
    );
    if (!update || !publish) {
      throw new PublishError("Bifrost 中缺少完整的知乎保存或发布鉴权上下文；请在知乎编辑器正常保存并发布一次后重试");
    }
    return { create, update, publish };
  }

  findPublicContext() {
    const context = this.latestContext(
      "zhuanlan.zhihu.com", "/p/", "GET",
      (item) => /^\/p\/\d+/.test(item.path),
    );
    if (context) return context;
    const contexts = this.findContexts();
    return contexts.create ?? contexts.update;
  }
}

async function readJsonResponse(response, label) {
  let payload;
  const text = await response.text();
  try {
    payload = text ? JSON.parse(text) : {};
  } catch {
    throw new PublishError(label + " 返回非 JSON（HTTP " + response.status + "）", {
      authFailure: response.status === 401 || response.status === 403,
      status: response.status,
    });
  }
  const code = payload.code ?? payload.error?.code ?? null;
  const message = payload.message ?? payload.toast_message ?? payload.error?.message ?? "";
  if (!response.ok || payload.error || (code !== null && Number(code) !== 0)) {
    throw new PublishError(label + " 失败（HTTP " + response.status + (code === null ? "" : ", code " + code) +"): " + String(message).slice(0, 200), {
      authFailure: isAuthFailure(response.status, code, message),
      status: response.status,
      code,
    });
  }
  return payload;
}

export async function apiRequest(base, method, endpoint, context, body, label = "知乎 API") {
  const response = await fetch(new URL(endpoint, base), {
    method,
    headers: filteredHeaders(context, {
      accept: "application/json, text/plain, */*",
      "content-type": "application/json",
    }),
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });
  return readJsonResponse(response, label);
}

export function buildPublishPayload(article, draftId, now = Date.now(), options = {}) {
  const business = {
    disclaimer_type: article.disclaimerType,
    disclaimer_status: article.disclaimerStatus,
    table_of_contents_enabled: article.tableOfContents,
    content: article.html,
    title: article.title,
    commercial_report_info: { commercial_types: [] },
    commercial_zhitask_bind_info: null,
    canReward: article.canReward,
  };
  return {
    action: "article",
    data: {
      publish: { traceId: String(now) + "," + crypto.randomUUID() },
      extra_info: {
        publisher: "pc",
        pc_business_params: JSON.stringify(business),
      },
      draft: {
        disabled: 1,
        id: String(draftId),
        isPublished: Boolean(options.isPublished),
      },
      commentsPermission: { comment_permission: article.commentPermission },
      creationStatement: {
        disclaimer_type: article.disclaimerType,
        disclaimer_status: article.disclaimerStatus,
      },
      contentsTables: { table_of_contents_enabled: article.tableOfContents },
      commercialReportInfo: { isReport: 0 },
      appreciate: { can_reward: article.canReward, tagline: "" },
      hybridInfo: {},
      hybrid: {
        html: article.html,
        textLength: Array.from(article.text).length,
      },
      title: { title: article.title },
    },
  };
}

function defaultStateFile() {
  if (process.env.ZHIHU_PUBLISH_STATE_FILE) return path.resolve(process.env.ZHIHU_PUBLISH_STATE_FILE);
  const root = process.env.XDG_STATE_HOME || path.join(os.homedir(), ".local", "state");
  return path.join(root, "bifrost", "zhihu-publish.json");
}

export function loadState(filePath) {
  try {
    const parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch (error) {
    if (error.code === "ENOENT") return {};
    throw new PublishError("无法读取发布状态文件: " + error.message);
  }
}

export function saveState(filePath, state) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  const temporary = filePath + ".tmp-" + process.pid;
  fs.writeFileSync(temporary, JSON.stringify(state, null, 2) + "\n", { mode: 0o600 });
  fs.renameSync(temporary, filePath);
  fs.chmodSync(filePath, 0o600);
}

function hash(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function articleKey(articlePath) {
  return hash(path.resolve(articlePath));
}

function publicUrl(articleId) {
  return COLUMN_BASE + "/p/" + articleId;
}

function parsePublishedId(payload) {
  let result = payload?.data?.result ?? payload?.result ?? null;
  if (typeof result === "string") {
    try {
      result = JSON.parse(result);
    } catch {
      throw new PublishError("知乎发布成功响应中的 result 不是有效 JSON");
    }
  }
  const value = result?.publish?.id ?? result?.id ?? payload?.data?.publish?.id;
  const id = String(value ?? "");
  if (!/^\d+$/.test(id)) throw new PublishError("知乎发布成功响应缺少文章 ID");
  return id;
}

async function validateAuth(mainBase, context) {
  const response = await fetch(new URL("/api/v4/me", mainBase), {
    headers: filteredHeaders(context, { accept: "application/json" }),
    signal: AbortSignal.timeout(30_000),
  });
  if (response.status === 401 || response.status === 403) return false;
  if (!response.ok) throw new PublishError("知乎登录态探测失败（HTTP " + response.status + "）");
  const payload = await response.json();
  return Boolean(payload?.id || payload?.url_token || payload?.name);
}

export async function publishArticle(options, dependencies = {}) {
  if (!options.article) throw new PublishError("缺少 --article");
  if (!["dry-run", "draft-only", "publish"].includes(options.mode)) {
    throw new PublishError("必须选择 --dry-run、--draft-only 或 --publish");
  }
  const articlePath = path.resolve(options.article);
  const source = fs.readFileSync(articlePath, "utf8");
  const article = parseArticle(source);
  const sourceHash = hash(source);
  const state = loadState(options.stateFile);
  const key = articleKey(articlePath);
  let entry = state[key] ?? null;
  if (!entry && !options.forceNew) {
    entry = Object.values(state).find((candidate) =>
      candidate && candidate.content_hash === sourceHash && (candidate.draft_id || candidate.article_id)) ?? null;
  }
  if (options.forceNew) entry = null;
  if (options.articleId) {
    if (!options.updateExisting) throw new PublishError("--article-id 必须与 --update-existing 一起使用");
    if (entry?.article_id && String(entry.article_id) !== options.articleId) {
      throw new PublishError("--article-id 与状态文件中已记录的文章 ID 不一致");
    }
    entry = {
      ...entry,
      article_path: articlePath,
      content_hash: sourceHash,
      replaced_draft_id: entry?.draft_id && String(entry.draft_id) !== options.articleId
        ? String(entry.draft_id)
        : (entry?.replaced_draft_id ?? null),
      draft_id: options.articleId,
      article_id: options.articleId,
      url: publicUrl(options.articleId),
    };
  }
  if (options.mode === "dry-run") {
    const imageSummary = summarizeArticleImages(article);
    let action = "create_draft";
    if (entry?.article_id && options.updateExisting) action = "update_existing_article";
    else if (entry?.article_id && entry.content_hash === sourceHash) action = "already_published";
    else if (entry?.article_id) action = "published_source_changed";
    else if (entry?.draft_id) action = "update_existing_draft";
    return {
      status: "dry_run",
      title: article.title,
      markdown_characters: article.markdown.length,
      html_characters: article.html.length,
      text_characters: Array.from(article.text).length,
      action,
      duplicate: Boolean(entry?.article_id && entry.content_hash === sourceHash),
      ...imageSummary,
    };
  }
  if (entry?.article_id && entry.content_hash !== sourceHash && !options.updateExisting) {
    throw new PublishError("该源文件已有已发布记录，但正文已变化；为避免误发重复文章，请使用新文件或在明确需要第二篇时传 --force-new");
  }
  if (entry?.article_id && !options.updateExisting) {
    return {
      status: "already_published",
      title: article.title,
      article_id: entry.article_id,
      url: entry.url || publicUrl(entry.article_id),
    };
  }
  const bifrost = dependencies.bifrost ?? new BifrostClient();
  let contexts = bifrost.findContexts();
  const log = dependencies.log ?? ((message) => process.stderr.write("[zhihu-publish] " + message + "\n"));
  log("已从 Bifrost 读取知乎鉴权上下文（Cookie " + contexts.update.cookieNames.length + " 项，敏感值未输出；发布结构由脚本内置）");
  if (!(await validateAuth(options.mainBase, contexts.publish))) {
    throw new PublishError("Bifrost 最近流量中的知乎登录态已失效；请正常访问知乎后重试", { authFailure: true });
  }

  let draftId = entry?.draft_id ? String(entry.draft_id) : (options.updateExisting && entry?.article_id ? String(entry.article_id) : null);
  if (!draftId && !contexts.create) {
    throw new PublishError("Bifrost 中没有创建草稿的鉴权上下文；请在知乎新建文章并保存一次后重试");
  }

  const processedArticle = await withAuthRetry((current) => processArticleImages(
    article, articlePath, options.columnBase, current.update, dependencies.fetch ?? fetch,
  ));
  log("图片处理完成（共 " + processedArticle.imageSummary.image_count + " 张，均已转换为知乎图床地址）");

  async function withAuthRetry(operation) {
    try {
      return await operation(contexts);
    } catch (error) {
      if (!(error instanceof PublishError) || !error.authFailure) throw error;
      log("知乎登录态请求失败，重新读取一次最新 Bifrost 流量后重试");
      contexts = bifrost.findContexts();
      if (!(await validateAuth(options.mainBase, contexts.publish))) throw error;
      return operation(contexts);
    }
  }

  if (!draftId) {
    const created = await withAuthRetry((current) => apiRequest(
      options.columnBase, "POST", "/api/articles/drafts", current.create,
      { title: processedArticle.title, delta_time: 0, can_reward: processedArticle.canReward }, "创建知乎草稿",
    ));
    draftId = String(created.id ?? created.data?.id ?? "");
    if (!/^\d+$/.test(draftId)) throw new PublishError("创建知乎草稿成功但响应缺少草稿 ID");
    state[key] = {
      article_path: articlePath,
      content_hash: sourceHash,
      draft_id: draftId,
      article_id: null,
      url: null,
      updated_at: new Date().toISOString(),
    };
    saveState(options.stateFile, state);
  }

  if (!state[key]) {
    state[key] = {
      article_path: articlePath,
      content_hash: sourceHash,
      draft_id: draftId,
      article_id: null,
      url: null,
      updated_at: new Date().toISOString(),
    };
    saveState(options.stateFile, state);
  }

  await withAuthRetry((current) => apiRequest(
    options.columnBase, "PATCH", "/api/articles/" + draftId + "/draft", current.update,
    { title: article.title, delta_time: 0, can_reward: article.canReward }, "保存知乎标题",
  ));
  await withAuthRetry((current) => apiRequest(
    options.columnBase, "PATCH", "/api/articles/" + draftId + "/draft", current.update,
    {
      content: processedArticle.html,
      table_of_contents: processedArticle.tableOfContents,
      delta_time: 0,
      can_reward: processedArticle.canReward,
    },
    "保存知乎正文",
  ));
  if (options.articleId) state[key] = { ...entry, updated_at: new Date().toISOString() };
  state[key].content_hash = sourceHash;
  state[key].updated_at = new Date().toISOString();
  saveState(options.stateFile, state);
  if (options.mode === "draft-only") {
    return { status: options.updateExisting ? "updated_draft_saved" : "draft_saved", title: processedArticle.title, draft_id: draftId };
  }

  const published = await withAuthRetry((current) => apiRequest(
    options.mainBase, "POST", "/api/v4/content/publish", current.publish,
    buildPublishPayload(processedArticle, draftId, Date.now(), {
      isPublished: Boolean(options.updateExisting && entry?.article_id),
    }), "发布知乎文章",
  ));
  let articleId;
  try {
    articleId = parsePublishedId(published);
  } catch (error) {
    if (!options.updateExisting || !entry?.article_id) throw error;
    articleId = String(entry.article_id);
  }
  const url = publicUrl(articleId);
  state[key] = {
    ...state[key],
    content_hash: sourceHash,
    article_id: articleId,
    url,
    published_at: new Date().toISOString(),
  };
  saveState(options.stateFile, state);
  return {
    status: options.updateExisting ? "updated" : "published",
    title: processedArticle.title,
    draft_id: draftId,
    article_id: articleId,
    url,
    image_count: processedArticle.imageSummary.image_count,
  };
}

export function parseArgs(argv) {
  const options = {
    article: null,
    mode: null,
    forceNew: false,
    updateExisting: false,
    articleId: null,
    stateFile: defaultStateFile(),
    columnBase: process.env.ZHIHU_COLUMN_BASE || COLUMN_BASE,
    mainBase: process.env.ZHIHU_MAIN_BASE || MAIN_BASE,
  };
  const valueAfter = (index, flag) => {
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) throw new PublishError(flag + " 缺少参数值");
    return value;
  };
  const setMode = (mode) => {
    if (options.mode && options.mode !== mode) throw new PublishError("发布模式互相冲突");
    options.mode = mode;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--article") options.article = valueAfter(index++, arg);
    else if (arg === "--dry-run") setMode("dry-run");
    else if (arg === "--draft-only") setMode("draft-only");
    else if (arg === "--publish") setMode("publish");
    else if (arg === "--check-auth") setMode("check-auth");
    else if (arg === "--force-new") options.forceNew = true;
    else if (arg === "--update-existing") options.updateExisting = true;
    else if (arg === "--article-id") options.articleId = valueAfter(index++, arg);
    else if (arg === "--state-file") options.stateFile = path.resolve(valueAfter(index++, arg));
    else if (arg === "--column-base") options.columnBase = valueAfter(index++, arg);
    else if (arg === "--main-base") options.mainBase = valueAfter(index++, arg);
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new PublishError("未知参数: " + arg);
  }
  if (options.forceNew && options.updateExisting) throw new PublishError("--force-new 与 --update-existing 不能同时使用");
  if (options.articleId && (!/^\d+$/.test(options.articleId) || options.articleId === "0")) {
    throw new PublishError("--article-id 必须是非零数字");
  }
  if (options.articleId && !options.updateExisting) throw new PublishError("--article-id 必须与 --update-existing 一起使用");
  return options;
}

function printHelp() {
  process.stdout.write(`Usage:
  zhihu-publish --article <file.md> --dry-run
  zhihu-publish --article <file.md> --draft-only
  zhihu-publish --article <file.md> --publish
  zhihu-publish --check-auth

Options:
  --force-new         忽略已发布记录，创建另一篇文章
  --update-existing   明确更新状态文件中已发布的同一篇文章
  --article-id ID     与 --update-existing 配合，将已知知乎文章纳入幂等状态
  --state-file PATH  覆盖默认幂等状态文件
  --column-base URL  测试专用专栏 API 根地址
  --main-base URL    测试专用知乎主站 API 根地址
`);
}

async function checkAuth(options, dependencies = {}) {
  const bifrost = dependencies.bifrost ?? new BifrostClient();
  const contexts = bifrost.findContexts();
  const authenticated = await validateAuth(options.mainBase, contexts.publish);
  if (!authenticated) throw new PublishError("Bifrost 最近流量中的知乎登录态已失效", { authFailure: true });
  return {
    status: "authenticated",
    sources: {
      create: contexts.create?.sourceId ?? null,
      update: contexts.update.sourceId,
      publish: contexts.publish.sourceId,
    },
    cookie_names: contexts.publish.cookieNames,
    publish_schema: "built_in",
  };
}

export async function main(argv = process.argv.slice(2), dependencies = {}) {
  const options = parseArgs(argv);
  if (options.help) {
    printHelp();
    return 0;
  }
  if (!options.mode) throw new PublishError("必须选择一种操作模式");
  if (options.mode !== "dry-run") {
    options.columnBase = normalizeApiBase(options.columnBase, "column");
    options.mainBase = normalizeApiBase(options.mainBase, "main");
  }
  const result = options.mode === "check-auth"
    ? await checkAuth(options, dependencies)
    : await publishArticle(options, dependencies);
  process.stdout.write(JSON.stringify(result, null, 2) + "\n");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write("[zhihu-publish] ERROR: " + (error instanceof Error ? error.message : String(error)) + "\n");
    process.exitCode = 1;
  });
}
