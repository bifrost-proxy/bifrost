#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const DEFAULT_API_BASE = "https://api.juejin.cn";
const DEFAULT_LOGIN_URL = "https://juejin.cn/editor/drafts/new?v=2";
const DEFAULT_AID = "2608";
const COUNT_MASK = 538942;

export class PublishError extends Error {
  constructor(message, options = {}) {
    super(message);
    this.name = "PublishError";
    this.authFailure = Boolean(options.authFailure);
    this.status = options.status ?? null;
    this.errNo = options.errNo ?? null;
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

export function parseArticle(source) {
  const normalized = source.replace(/\r\n/g, "\n");
  const match = normalized.match(/^---\n([\s\S]*?)\n---\n?/);
  if (!match) throw new PublishError("文章必须以 YAML frontmatter 开头");
  const metadata = {};
  for (const [index, line] of match[1].split("\n").entries()) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = line.indexOf(":");
    if (separator <= 0) throw new PublishError("frontmatter 第 " + (index + 2) + " 行缺少 key: value");
    const key = line.slice(0, separator).trim();
    metadata[key] = parseValue(line.slice(separator + 1));
  }
  const markdown = normalized.slice(match[0].length).trim();
  if (!markdown) throw new PublishError("文章正文不能为空");
  return normalizeArticle(metadata, markdown);
}

function normalizeStringArray(value, field) {
  if (value === undefined || value === null || value === "") return [];
  const values = Array.isArray(value) ? value : String(value).split(",");
  const normalized = values.map((item) => String(item).trim()).filter(Boolean);
  if (normalized.some((item) => !/^\d+$/.test(item))) {
    throw new PublishError(field + " 必须是数字 ID 数组");
  }
  return normalized;
}

function stripMarkdown(markdown) {
  return markdown
    .replace(/\x60{3}[\s\S]*?\x60{3}/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/<[^>]+>/g, " ")
    .replace(/[#>*_~|\-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

export function normalizeArticle(metadata, markdown) {
  const title = String(metadata.title ?? "").trim();
  const categoryId = String(metadata.category_id ?? "").trim();
  const tagIds = normalizeStringArray(metadata.tag_ids, "tag_ids");
  if (!title) throw new PublishError("frontmatter 缺少 title");
  if (title.length > 100) throw new PublishError("title 不能超过 100 个字符");
  if (!/^\d+$/.test(categoryId) || categoryId === "0") throw new PublishError("frontmatter category_id 必须是非零数字 ID");
  if (tagIds.length === 0) throw new PublishError("frontmatter 至少需要一个 tag_ids");
  const brief = String(metadata.brief_content ?? stripMarkdown(markdown).slice(0, 100)).trim();
  const pics = metadata.pics ?? [];
  if (!Array.isArray(pics)) throw new PublishError("pics 必须是 JSON 数组");
  return {
    title,
    categoryId,
    tagIds,
    briefContent: brief.slice(0, 100),
    coverImage: String(metadata.cover_image ?? "").trim(),
    linkUrl: String(metadata.link_url ?? "").trim(),
    editType: Number(metadata.edit_type ?? 10),
    themeIds: normalizeStringArray(metadata.theme_ids, "theme_ids"),
    columnIds: normalizeStringArray(metadata.column_ids, "column_ids"),
    syncToOrg: Boolean(metadata.sync_to_org ?? false),
    pics,
    markdown,
  };
}

export function trimForWordCount(originalSource) {
  let source = String(originalSource ?? "");
  source = source.replace(/\x60{3}mermaid[^]*?\x60{2,3}/gs, "");
  source = source.replace(/\$[\s\S]+?\$\$|\$[\s\S]+?\$/g, "");
  source = source.replace(/.*---\ntheme:.*?\n---/gs, "");
  source = source.replace(/.*---\nhighlight:.*?\n---/gs, "");
  source = source.replace(/\<\S+?\>/gs, (match) => match.toLowerCase());
  source = source.replace(/\<link\S+?\<\/link\>/gs, "");
  source = source.replace(/\<style\S+?\<\/style\>/gs, "");
  source = source.replace(/\<script\S+?\<\/script\>/gs, "");
  source = source.replace(/\<\S\s+?\>/gs, "\n");
  source = source.replace(/\s{2,}/gs, "\n");
  source = source.replace(/!\[.*\]\(http.*?\)/g, "");
  source = source.replace(/((http|https):\/\/)?[\w\-_]+(\.[\w\-_]+)+([\w.,@?^=%&:\/~+#-]*[\w@?^=%&\/~+#-])?/gs, "");
  source = source.replace(/<[^>\n\u4e00-\u9fff]*>/g, "");
  return source.trim();
}

export function realWordCount(content) {
  const matches = trimForWordCount(content).match(/[\u4e00-\u9fbf\u3400-\u4dbf\u3040-\u309f\u30a0-\u30ff]|[a-zA-Z]+|\d/g);
  return matches?.length ?? 0;
}

export function maskedWordCount(realCount) {
  return (Number(realCount) ^ COUNT_MASK) + COUNT_MASK;
}

function headerMap(headers) {
  const map = new Map();
  for (const item of headers ?? []) {
    if (!Array.isArray(item) || item.length < 2) continue;
    map.set(String(item[0]).toLowerCase(), String(item[1]));
  }
  return map;
}

export function cookieNames(cookie) {
  return String(cookie ?? "").split(";").map((part) => part.trim().split("=", 1)[0]).filter(Boolean).sort();
}

export function extractAuth(traffic) {
  const headers = headerMap(traffic.request_headers);
  const cookie = headers.get("cookie") ?? "";
  const csrfToken = headers.get("x-secsdk-csrf-token") ?? "";
  if (!cookie) throw new PublishError("Bifrost 流量中没有 Cookie");
  if (!csrfToken) throw new PublishError("Bifrost 流量中没有 x-secsdk-csrf-token");
  const names = cookieNames(cookie);
  if (!names.some((name) => name === "sessionid" || name === "sessionid_ss")) {
    throw new PublishError("Bifrost 流量中的登录 Cookie 不完整");
  }
  let url;
  try {
    url = new URL(traffic.url);
  } catch {
    url = new URL("https://" + traffic.host + (traffic.path ?? ""));
  }
  return {
    cookie,
    csrfToken,
    userAgent: headers.get("user-agent") ?? "Mozilla/5.0",
    aid: url.searchParams.get("aid") || DEFAULT_AID,
    uuid: url.searchParams.get("uuid") || "",
    sourceId: traffic.id,
    cookieNames: names,
  };
}

export function isAuthFailure(status, errNo, message = "") {
  if (status === 401 || status === 403) return true;
  if ([1, 2, 401, 403, 1001, 1002, 1003].includes(Number(errNo))) return true;
  return /(login|logged|auth|token|session|登录|鉴权|未登录)/i.test(String(message));
}

export function normalizeApiBase(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new PublishError("--api-base 必须是有效 URL");
  }
  const isProduction = url.protocol === "https:" && url.hostname === "api.juejin.cn";
  const isLoopback = ["127.0.0.1", "::1", "localhost"].includes(url.hostname)
    && ["http:", "https:"].includes(url.protocol);
  if (!isProduction && !isLoopback) {
    throw new PublishError("拒绝把掘金登录态发送到非 api.juejin.cn 或非本机 API 地址");
  }
  url.pathname = url.pathname.replace(/\/$/, "") || "/";
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/, "");
}

class BifrostClient {
  constructor(binary = process.env.BIFROST_BIN || "bifrost") {
    this.binary = binary;
  }

  run(args) {
    const result = spawnSync(this.binary, args, {
      encoding: "utf8",
      env: process.env,
      maxBuffer: 16 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
    if (result.error) throw new PublishError("无法执行 Bifrost: " + result.error.message);
    if (result.status !== 0) {
      throw new PublishError("Bifrost 命令失败（exit " + result.status + "）；为避免泄露登录信息，未回显命令输出");
    }
    return String(result.stdout || "").trim();
  }

  searchLatestAuth() {
    const queries = [
      ["search", "article_draft", "--url", "--host", "api.juejin.cn", "--latest", "30d", "--limit", "50", "--format", "json"],
      ["search", "user/get", "--url", "--host", "api.juejin.cn", "--latest", "30d", "--limit", "50", "--format", "json"],
    ];
    const candidates = [];
    for (const args of queries) {
      try {
        const parsed = JSON.parse(this.run(args));
        candidates.push(...(parsed.results ?? []));
      } catch {
        // Try the next safe query. Credentials are never printed.
      }
    }
    candidates.sort((left, right) => Number(right.seq ?? 0) - Number(left.seq ?? 0));
    for (const candidate of candidates) {
      try {
        return extractAuth(this.getTraffic(candidate.id));
      } catch {
        // Skip OPTIONS rows, incomplete captures, and details removed by retention.
      }
    }
    return null;
  }

  getTraffic(id) {
    return JSON.parse(this.run(["traffic", "get", String(id), "--format", "json"]));
  }

  captureFreshTraffic(loginUrl = DEFAULT_LOGIN_URL) {
    const output = this.run([
      "capture", "wait", "--host", "api.juejin.cn",
      "--path", "/content_api/v1/author_center/count",
      "--open", loginUrl, "--timeout", "5m", "--format", "json",
    ]);
    const captured = JSON.parse(output);
    const id = captured.id ?? captured.record?.id;
    if (!id) throw new PublishError("Bifrost capture wait 没有返回请求 ID");
    return this.getTraffic(id);
  }
}

function makeHeaders(auth) {
  return {
    accept: "application/json, text/plain, */*",
    "content-type": "application/json",
    cookie: auth.cookie,
    origin: "https://juejin.cn",
    referer: "https://juejin.cn/",
    "user-agent": auth.userAgent,
    "x-secsdk-csrf-token": auth.csrfToken,
  };
}

async function readApiResponse(response) {
  let payload;
  try {
    payload = await response.json();
  } catch {
    throw new PublishError("掘金 API 返回非 JSON（HTTP " + response.status + "）", {
      authFailure: response.status === 401 || response.status === 403,
      status: response.status,
    });
  }
  const errNo = Number(payload.err_no ?? 0);
  const message = String(payload.err_msg ?? "");
  if (!response.ok || errNo !== 0) {
    throw new PublishError(
      "掘金 API 失败（HTTP " + response.status + ", err_no " + errNo + "）: " + message.slice(0, 200),
      {
        authFailure: isAuthFailure(response.status, errNo, message),
        status: response.status,
        errNo,
      },
    );
  }
  return payload.data ?? payload;
}

async function apiRequest(apiBase, method, endpoint, auth, body) {
  const url = new URL(endpoint, apiBase);
  url.searchParams.set("aid", auth.aid || DEFAULT_AID);
  url.searchParams.set("uuid", auth.uuid || "");
  const response = await fetch(url, {
    method,
    headers: makeHeaders(auth),
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });
  return readApiResponse(response);
}

export async function validateAuth(apiBase, auth) {
  try {
    await apiRequest(apiBase, "GET", "/user_api/v1/user/get", auth);
    return true;
  } catch (error) {
    if (error instanceof PublishError && error.authFailure) return false;
    throw error;
  }
}

async function resolveAuth(options) {
  const bifrost = options.bifrost;
  const apiBase = options.apiBase;
  const allowBrowser = options.allowBrowser;
  const loginUrl = options.loginUrl;
  const log = options.log;
  const latestAuth = bifrost.searchLatestAuth();
  if (latestAuth) {
    if (await validateAuth(apiBase, latestAuth)) {
      log("已从 Bifrost 请求 " + latestAuth.sourceId + " 读取并验证登录态（" + latestAuth.cookieNames.length + " 个 Cookie）");
      return latestAuth;
    }
    log("Bifrost 最近流量中的登录态已失效");
  }
  if (!allowBrowser) {
    throw new PublishError("没有可用登录态；移除 --no-browser 后可自动打开浏览器重新采集");
  }
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    log("正在打开浏览器并等待 Bifrost 采集登录流量（第 " + attempt + "/2 次）…");
    const traffic = bifrost.captureFreshTraffic(loginUrl);
    try {
      const auth = extractAuth(traffic);
      if (await validateAuth(apiBase, auth)) {
        log("新的登录态已验证（" + auth.cookieNames.length + " 个 Cookie）");
        return auth;
      }
      log("本次采集的登录态仍无效");
    } catch (error) {
      if (!(error instanceof PublishError)) throw error;
      log("本次采集未得到完整登录态: " + error.message);
    }
  }
  throw new PublishError("浏览器登录完成后仍未采集到有效掘金 Cookie");
}

function draftPayload(article, id) {
  return {
    ...(id ? { id } : {}),
    category_id: article.categoryId,
    tag_ids: article.tagIds,
    link_url: article.linkUrl,
    cover_image: article.coverImage,
    title: article.title,
    brief_content: article.briefContent,
    edit_type: article.editType,
    html_content: "<p></p>",
    mark_content: article.markdown,
    theme_ids: article.themeIds,
    pics: article.pics,
  };
}

function defaultStateFile() {
  if (process.env.JUEJIN_PUBLISH_STATE_FILE) return path.resolve(process.env.JUEJIN_PUBLISH_STATE_FILE);
  const stateRoot = process.env.XDG_STATE_HOME || path.join(os.homedir(), ".local", "state");
  return path.join(stateRoot, "bifrost", "juejin-publish.json");
}

function loadState(filePath) {
  try {
    const parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch (error) {
    if (error.code === "ENOENT") return {};
    throw new PublishError("无法读取发布状态文件: " + error.message);
  }
}

function saveState(filePath, state) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  const temporary = filePath + ".tmp-" + process.pid;
  fs.writeFileSync(temporary, JSON.stringify(state, null, 2) + "\n", { mode: 0o600 });
  fs.renameSync(temporary, filePath);
  fs.chmodSync(filePath, 0o600);
}

function articleKey(articlePath) {
  const absolute = path.resolve(articlePath);
  return crypto.createHash("sha256").update(absolute).digest("hex");
}

function contentHash(source) {
  return crypto.createHash("sha256").update(source).digest("hex");
}

function publicUrl(articleId) {
  return "https://juejin.cn/post/" + articleId;
}

function parseArgs(argv) {
  const options = {
    article: null,
    mode: null,
    noBrowser: false,
    forceNew: false,
    apiBase: process.env.JUEJIN_API_BASE || DEFAULT_API_BASE,
    stateFile: defaultStateFile(),
    loginUrl: process.env.JUEJIN_LOGIN_URL || DEFAULT_LOGIN_URL,
  };
  const nextValue = (index, flag) => {
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) throw new PublishError(flag + " 缺少参数值");
    return value;
  };
  const setMode = (mode) => {
    if (options.mode && options.mode !== mode) {
      throw new PublishError("发布模式互相冲突: --" + options.mode + " 与 --" + mode);
    }
    options.mode = mode;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--article") options.article = nextValue(index++, arg);
    else if (arg === "--dry-run") setMode("dry-run");
    else if (arg === "--draft-only") setMode("draft-only");
    else if (arg === "--publish") setMode("publish");
    else if (arg === "--check-auth") setMode("check-auth");
    else if (arg === "--no-browser") options.noBrowser = true;
    else if (arg === "--force-new") options.forceNew = true;
    else if (arg === "--api-base") options.apiBase = nextValue(index++, arg);
    else if (arg === "--state-file") options.stateFile = path.resolve(nextValue(index++, arg));
    else if (arg === "--login-url") options.loginUrl = nextValue(index++, arg);
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new PublishError("未知参数: " + arg);
  }
  return options;
}

function printHelp() {
  process.stdout.write(
    [
      "Usage:",
      "  juejin-publish --article <file.md> --dry-run",
      "  juejin-publish --article <file.md> --draft-only",
      "  juejin-publish --article <file.md> --publish",
      "  juejin-publish --check-auth [--no-browser]",
      "",
      "Options:",
      "  --no-browser       Cookie 缺失或失效时禁止自动打开浏览器",
      "  --force-new        已记录为发布的文件仍创建一篇新文章",
      "  --state-file PATH  覆盖默认幂等状态文件",
      "  --api-base URL     覆盖 API 根地址（仅测试/受控环境）",
      "  --login-url URL    覆盖重新登录时打开的页面",
      "",
    ].join("\n"),
  );
}

async function publishArticle(options) {
  if (!options.article) throw new PublishError("缺少 --article");
  if (!options.mode) throw new PublishError("必须选择 --dry-run、--draft-only 或 --publish");
  const source = fs.readFileSync(path.resolve(options.article), "utf8");
  const article = parseArticle(source);
  const sourceHash = contentHash(source);
  const state = loadState(options.stateFile);
  const key = articleKey(options.article);
  let entry = state[key] ?? null;

  if (!entry && !options.forceNew) {
    entry = Object.values(state).find((candidate) =>
      candidate && candidate.content_hash === sourceHash && (candidate.article_id || candidate.draft_id),
    ) ?? null;
  }

  if (entry?.article_id && !options.forceNew) {
    return {
      status: "already_published",
      title: article.title,
      article_id: entry.article_id,
      url: entry.url || publicUrl(entry.article_id),
    };
  }
  if (options.forceNew) entry = null;

  const count = realWordCount(article.markdown);
  if (options.mode === "dry-run") {
    return {
      status: "dry_run",
      title: article.title,
      category_id: article.categoryId,
      tag_ids: article.tagIds,
      origin_word_count: count,
      encrypted_word_count: maskedWordCount(count),
      action: entry?.draft_id ? "update_existing_draft" : "create_draft",
    };
  }

  const log = (message) => process.stderr.write("[juejin-publish] " + message + "\n");
  const bifrost = new BifrostClient();
  let auth = await resolveAuth({
    bifrost,
    apiBase: options.apiBase,
    allowBrowser: !options.noBrowser,
    loginUrl: options.loginUrl,
    log,
  });

  async function withAuthRetry(operation) {
    try {
      return await operation(auth);
    } catch (error) {
      if (!(error instanceof PublishError) || !error.authFailure || options.noBrowser) throw error;
      log("请求期间登录态失效，重新通过 Bifrost 采集后重试一次");
      auth = await resolveAuth({
        bifrost,
        apiBase: options.apiBase,
        allowBrowser: true,
        loginUrl: options.loginUrl,
        log,
      });
      return operation(auth);
    }
  }

  let draftId = entry?.draft_id ?? null;
  if (!draftId) {
    const created = await withAuthRetry((current) =>
      apiRequest(options.apiBase, "POST", "/content_api/v1/article_draft/create", current, draftPayload(article)),
    );
    draftId = String(created.id ?? created.draft_id ?? "");
    if (!draftId) throw new PublishError("创建草稿成功但响应缺少 draft id");
  }

  state[key] = {
    article_path: path.resolve(options.article),
    content_hash: sourceHash,
    draft_id: draftId,
    article_id: null,
    url: null,
    updated_at: new Date().toISOString(),
  };
  saveState(options.stateFile, state);

  await withAuthRetry((current) =>
    apiRequest(options.apiBase, "POST", "/content_api/v1/article_draft/update", current, draftPayload(article, draftId)),
  );

  state[key].updated_at = new Date().toISOString();
  saveState(options.stateFile, state);

  if (options.mode === "draft-only") {
    return { status: "draft_saved", title: article.title, draft_id: draftId };
  }

  const published = await withAuthRetry((current) =>
    apiRequest(options.apiBase, "POST", "/content_api/v1/article/publish", current, {
      draft_id: draftId,
      sync_to_org: article.syncToOrg,
      column_ids: article.columnIds,
      theme_ids: article.themeIds,
      encrypted_word_count: maskedWordCount(count),
      origin_word_count: count,
    }),
  );
  const articleId = String(published.article_id ?? "");
  if (!articleId) throw new PublishError("发布成功响应缺少 article_id");
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

async function checkAuth(options) {
  const auth = await resolveAuth({
    bifrost: new BifrostClient(),
    apiBase: options.apiBase,
    allowBrowser: !options.noBrowser,
    loginUrl: options.loginUrl,
    log: (message) => process.stderr.write("[juejin-publish] " + message + "\n"),
  });
  return {
    status: "authenticated",
    source_id: auth.sourceId,
    cookie_names: auth.cookieNames,
  };
}

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    printHelp();
    return 0;
  }
  if (options.mode !== "dry-run") options.apiBase = normalizeApiBase(options.apiBase);
  const result = options.mode === "check-auth"
    ? await checkAuth(options)
    : await publishArticle(options);
  process.stdout.write(JSON.stringify(result, null, 2) + "\n");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write("[juejin-publish] ERROR: " + message + "\n");
    process.exitCode = 1;
  });
}
