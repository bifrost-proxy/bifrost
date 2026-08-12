#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const COLUMN_BASE = "https://zhuanlan.zhihu.com";
const MAIN_BASE = "https://www.zhihu.com";

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
    const url = safeUrl(rawUrl.trim());
    return url ? hold('<img src="' + escapeHtml(url) + '" alt="' + escapeHtml(alt) + '">') : escapeHtml(alt);
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
  return String(markdown)
    .replace(/\x60{3}[^\n]*\n([\s\S]*?)\x60{3}/g, "$1")
    .replace(/~~~[^\n]*\n([\s\S]*?)~~~/g, "$1")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*>\s?/gm, "")
    .replace(/^\s*(?:[-+*]|\d+[.)])\s+/gm, "")
    .replace(/\x60([^\x60]*)\x60/g, "$1")
    .replace(/[*_~]/g, "")
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

export function decodeTrafficBody(body, label = "Bifrost body") {
  if (body === undefined || body === null) throw new PublishError(label + " 不存在");
  let value = body;
  if (typeof body === "object" && Object.hasOwn(body, "success")) {
    if (!body.success) throw new PublishError(label + " 读取失败");
    value = body.data;
    if (body.encoding === "base64" && typeof value === "string") {
      value = Buffer.from(value, "base64").toString("utf8");
    }
  }
  if (Buffer.isBuffer(value)) value = value.toString("utf8");
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return null;
    try {
      return JSON.parse(trimmed);
    } catch (error) {
      throw new PublishError(label + " 不是有效 JSON: " + error.message);
    }
  }
  if (value && typeof value === "object") return value;
  throw new PublishError(label + " 格式不受支持");
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

  getTraffic(id, bodies = false) {
    const args = ["traffic", "get", String(id)];
    if (bodies) args.push("--request-body", "--response-body");
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

  findTemplates() {
    const create = this.latestContext(
      "zhuanlan.zhihu.com", "/api/articles/drafts", "POST",
      (item) => item.path === "/api/articles/drafts",
    );
    const update = this.latestContext(
      "zhuanlan.zhihu.com", "/draft", "PATCH",
      (item) => /^\/api\/articles\/\d+\/draft$/.test(item.path),
    );
    let publish = null;
    let publishTemplate = null;
    for (const candidate of this.search("www.zhihu.com", "/api/v4/content/publish", "POST")) {
      if (candidate.path !== "/api/v4/content/publish") continue;
      try {
        const traffic = this.getTraffic(candidate.id, true);
        publish = extractContext(traffic);
        publishTemplate = decodeTrafficBody(traffic.request_body, "知乎发布请求模板");
        if (publishTemplate?.action === "article" && publishTemplate?.data) break;
        publish = null;
        publishTemplate = null;
      } catch {
        publish = null;
        publishTemplate = null;
      }
    }
    if (!create || !update || !publish || !publishTemplate) {
      throw new PublishError("Bifrost 中缺少完整的知乎创建、保存或发布请求；请在知乎编辑器正常保存/发布一次后重试");
    }
    return { create, update, publish, publishTemplate };
  }

  findPublicContext() {
    const context = this.latestContext(
      "zhuanlan.zhihu.com", "/p/", "GET",
      (item) => /^\/p\/\d+/.test(item.path),
    );
    if (context) return context;
    const templates = this.findTemplates();
    return templates.create;
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

export function buildPublishPayload(template, article, draftId, now = Date.now()) {
  const payload = structuredClone(template);
  if (payload?.action !== "article" || !payload.data) throw new PublishError("知乎发布请求模板结构已变化");
  payload.action = "article";
  payload.data.draft = { ...(payload.data.draft ?? {}), disabled: 1, id: String(draftId), isPublished: false };
  payload.data.title = { ...(payload.data.title ?? {}), title: article.title };
  payload.data.hybrid = {
    ...(payload.data.hybrid ?? {}),
    html: article.html,
    textLength: Array.from(article.text).length,
  };
  payload.data.commentsPermission = { comment_permission: article.commentPermission };
  payload.data.creationStatement = {
    disclaimer_type: article.disclaimerType,
    disclaimer_status: article.disclaimerStatus,
  };
  payload.data.contentsTables = { table_of_contents_enabled: article.tableOfContents };
  payload.data.appreciate = { ...(payload.data.appreciate ?? {}), can_reward: article.canReward };
  payload.data.publish = {
    ...(payload.data.publish ?? {}),
    traceId: String(now) + "," + crypto.randomUUID(),
  };
  const extra = payload.data.extra_info ?? {};
  let business = {};
  if (typeof extra.pc_business_params === "string" && extra.pc_business_params.trim()) {
    try {
      business = JSON.parse(extra.pc_business_params);
    } catch {
      throw new PublishError("知乎发布请求模板中的 pc_business_params 无效");
    }
  }
  business.title = article.title;
  business.content = article.html;
  payload.data.extra_info = {
    ...extra,
    publisher: extra.publisher ?? "pc",
    pc_business_params: JSON.stringify(business),
  };
  return payload;
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
  if (options.mode === "dry-run") {
    let action = "create_draft";
    if (entry?.article_id && entry.content_hash === sourceHash) action = "already_published";
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
    };
  }
  if (entry?.article_id && entry.content_hash !== sourceHash) {
    throw new PublishError("该源文件已有已发布记录，但正文已变化；为避免误发重复文章，请使用新文件或在明确需要第二篇时传 --force-new");
  }
  if (entry?.article_id) {
    return {
      status: "already_published",
      title: article.title,
      article_id: entry.article_id,
      url: entry.url || publicUrl(entry.article_id),
    };
  }
  const bifrost = dependencies.bifrost ?? new BifrostClient();
  let templates = bifrost.findTemplates();
  const log = dependencies.log ?? ((message) => process.stderr.write("[zhihu-publish] " + message + "\n"));
  log("已从 Bifrost 读取知乎请求模板（Cookie " + templates.create.cookieNames.length + " 项，敏感值未输出）");
  if (!(await validateAuth(options.mainBase, templates.publish))) {
    throw new PublishError("Bifrost 最近流量中的知乎登录态已失效；请正常访问知乎后重试", { authFailure: true });
  }

  async function withAuthRetry(operation) {
    try {
      return await operation(templates);
    } catch (error) {
      if (!(error instanceof PublishError) || !error.authFailure) throw error;
      log("知乎登录态请求失败，重新读取一次最新 Bifrost 流量后重试");
      templates = bifrost.findTemplates();
      if (!(await validateAuth(options.mainBase, templates.publish))) throw error;
      return operation(templates);
    }
  }

  let draftId = entry?.draft_id ? String(entry.draft_id) : null;
  if (!draftId) {
    const created = await withAuthRetry((current) => apiRequest(
      options.columnBase, "POST", "/api/articles/drafts", current.create,
      { title: article.title, delta_time: 0, can_reward: article.canReward }, "创建知乎草稿",
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
      content: article.html,
      table_of_contents: article.tableOfContents,
      delta_time: 0,
      can_reward: article.canReward,
    },
    "保存知乎正文",
  ));
  state[key].content_hash = sourceHash;
  state[key].updated_at = new Date().toISOString();
  saveState(options.stateFile, state);
  if (options.mode === "draft-only") {
    return { status: "draft_saved", title: article.title, draft_id: draftId };
  }

  const published = await withAuthRetry((current) => apiRequest(
    options.mainBase, "POST", "/api/v4/content/publish", current.publish,
    buildPublishPayload(current.publishTemplate, article, draftId), "发布知乎文章",
  ));
  const articleId = parsePublishedId(published);
  const url = publicUrl(articleId);
  state[key] = {
    ...state[key],
    content_hash: sourceHash,
    article_id: articleId,
    url,
    published_at: new Date().toISOString(),
  };
  saveState(options.stateFile, state);
  return { status: "published", title: article.title, draft_id: draftId, article_id: articleId, url };
}

export function parseArgs(argv) {
  const options = {
    article: null,
    mode: null,
    forceNew: false,
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
    else if (arg === "--state-file") options.stateFile = path.resolve(valueAfter(index++, arg));
    else if (arg === "--column-base") options.columnBase = valueAfter(index++, arg);
    else if (arg === "--main-base") options.mainBase = valueAfter(index++, arg);
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new PublishError("未知参数: " + arg);
  }
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
  --state-file PATH  覆盖默认幂等状态文件
  --column-base URL  测试专用专栏 API 根地址
  --main-base URL    测试专用知乎主站 API 根地址
`);
}

async function checkAuth(options, dependencies = {}) {
  const bifrost = dependencies.bifrost ?? new BifrostClient();
  const templates = bifrost.findTemplates();
  const authenticated = await validateAuth(options.mainBase, templates.publish);
  if (!authenticated) throw new PublishError("Bifrost 最近流量中的知乎登录态已失效", { authFailure: true });
  return {
    status: "authenticated",
    sources: {
      create: templates.create.sourceId,
      update: templates.update.sourceId,
      publish: templates.publish.sourceId,
    },
    cookie_names: templates.publish.cookieNames,
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
