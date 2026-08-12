import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  PublishError,
  buildPublishPayload,
  cookieNames,
  decodeTrafficBody,
  extractContext,
  isAuthFailure,
  loadState,
  normalizeApiBase,
  parseArticle,
  publishArticle,
} from "./zhihu-publish.mjs";

function source(title = "测试文章") {
  return [
    "---",
    `title: "${title}"`,
    "category_id: 123",
    "tag_ids: [\"456\"]",
    "---",
    "## 为什么需要它",
    "",
    "你好 **Bifrost**。",
    "",
    "- 不再手抄请求",
    "- 保留登录态",
  ].join("\n");
}

function context(id = "REQ-test") {
  return {
    sourceId: id,
    cookieNames: ["_xsrf", "z_c0"],
    headers: new Map([
      ["cookie", "_xsrf=secret; z_c0=secret"],
      ["x-xsrftoken", "secret"],
      ["user-agent", "Test Agent"],
      ["content-length", "999"],
    ]),
  };
}

function fakeBifrost(template) {
  return {
    findTemplates() {
      return {
        create: context("REQ-create"),
        update: context("REQ-update"),
        publish: context("REQ-publish"),
        publishTemplate: structuredClone(template),
      };
    },
  };
}

const template = {
  action: "article",
  data: {
    publish: { traceId: "old" },
    extra_info: { publisher: "pc", pc_business_params: JSON.stringify({ scene: "editor" }) },
    draft: { disabled: 1, id: "1", isPublished: false },
    commentsPermission: { comment_permission: "anyone" },
    creationStatement: { disclaimer_type: "none", disclaimer_status: "close" },
    contentsTables: { table_of_contents_enabled: false },
    appreciate: { can_reward: false, tagline: "" },
    hybrid: { html: "old", textLength: 3 },
    title: { title: "old" },
  },
};

test("parseArticle accepts extra platform frontmatter and renders HTML", () => {
  const article = parseArticle(source());
  assert.equal(article.title, "测试文章");
  assert.match(article.html, /<h2>为什么需要它<\/h2>/);
  assert.match(article.html, /<strong>Bifrost<\/strong>/);
  assert.equal(article.text, "为什么需要它 你好 Bifrost。 不再手抄请求 保留登录态");
});

test("parseArticle rejects missing title and unclosed code fence", () => {
  assert.throws(() => parseArticle("---\nfoo: bar\n---\nbody"), /缺少 title/);
  assert.throws(() => parseArticle("---\ntitle: x\n---\n```js\nbody"), /代码块未闭合/);
});

test("captured body and authentication context are decoded safely", () => {
  assert.deepEqual(decodeTrafficBody({ success: true, encoding: "text", data: '{"id":1}' }), { id: 1 });
  assert.deepEqual(
    decodeTrafficBody({ success: true, encoding: "base64", data: Buffer.from('{"id":2}').toString("base64") }),
    { id: 2 },
  );
  const captured = extractContext({
    id: "REQ-1",
    request_headers: [
      ["cookie", "z_c0=secret; _xsrf=another"],
      ["x-xsrftoken", "secret"],
    ],
  });
  assert.deepEqual(captured.cookieNames, ["_xsrf", "z_c0"]);
  assert.deepEqual(cookieNames("b=2; a=1"), ["a", "b"]);
  assert.throws(() => extractContext({ request_headers: [] }), /缺少知乎 Cookie/);
});

test("API bases and authentication failure classification are conservative", () => {
  assert.equal(normalizeApiBase("https://zhuanlan.zhihu.com/", "column"), "https://zhuanlan.zhihu.com");
  assert.equal(normalizeApiBase("https://www.zhihu.com/", "main"), "https://www.zhihu.com");
  assert.equal(normalizeApiBase("http://127.0.0.1:4321/", "main"), "http://127.0.0.1:4321");
  assert.throws(() => normalizeApiBase("https://example.com", "main"), /拒绝/);
  assert.equal(isAuthFailure(403, null, ""), true);
  assert.equal(isAuthFailure(400, 0, "invalid title"), false);
});

test("publish payload replaces every dynamic article field", () => {
  const article = parseArticle(source());
  const payload = buildPublishPayload(template, article, "9001", 1234);
  assert.equal(payload.data.draft.id, "9001");
  assert.equal(payload.data.title.title, article.title);
  assert.equal(payload.data.hybrid.html, article.html);
  assert.equal(payload.data.hybrid.textLength, Array.from(article.text).length);
  assert.match(payload.data.publish.traceId, /^1234,/);
  const business = JSON.parse(payload.data.extra_info.pc_business_params);
  assert.equal(business.title, article.title);
  assert.equal(business.content, article.html);
  assert.equal(business.scene, "editor");
});

test("mock E2E creates, saves, publishes, persists state, and prevents duplicates", async (contextHandle) => {
  const requests = [];
  const server = http.createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    requests.push({ method: request.method, url: request.url, body: body ? JSON.parse(body) : null });
    response.setHeader("content-type", "application/json");
    if (request.url === "/api/v4/me") response.end(JSON.stringify({ id: "user-1", name: "tester" }));
    else if (request.url === "/api/articles/drafts") response.end(JSON.stringify({ id: "9001" }));
    else if (request.url === "/api/articles/9001/draft") response.end("{}");
    else if (request.url === "/api/v4/content/publish") {
      response.end(JSON.stringify({ code: 0, data: { result: JSON.stringify({ publish: { id: "8001" } }) } }));
    } else {
      response.statusCode = 404;
      response.end(JSON.stringify({ message: "not found" }));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  contextHandle.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "zhihu-publish-test-"));
  contextHandle.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const articlePath = path.join(temporary, "article.md");
  const stateFile = path.join(temporary, "state.json");
  fs.writeFileSync(articlePath, source());
  const address = server.address();
  const base = `http://127.0.0.1:${address.port}`;
  const options = {
    article: articlePath,
    mode: "publish",
    forceNew: false,
    stateFile,
    columnBase: base,
    mainBase: base,
  };
  const first = await publishArticle(options, { bifrost: fakeBifrost(template), log: () => {} });
  assert.deepEqual(first, {
    status: "published",
    title: "测试文章",
    draft_id: "9001",
    article_id: "8001",
    url: "https://zhuanlan.zhihu.com/p/8001",
  });
  assert.deepEqual(requests.map((item) => [item.method, item.url]), [
    ["GET", "/api/v4/me"],
    ["POST", "/api/articles/drafts"],
    ["PATCH", "/api/articles/9001/draft"],
    ["PATCH", "/api/articles/9001/draft"],
    ["POST", "/api/v4/content/publish"],
  ]);
  assert.equal(requests[2].body.title, "测试文章");
  assert.match(requests[3].body.content, /<h2>/);
  assert.equal(requests[4].body.data.draft.id, "9001");
  assert.equal(fs.statSync(stateFile).mode & 0o777, 0o600);
  assert.equal(loadState(stateFile)[Object.keys(loadState(stateFile))[0]].article_id, "8001");

  const before = requests.length;
  const duplicate = await publishArticle(options, { bifrost: fakeBifrost(template), log: () => {} });
  assert.equal(duplicate.status, "already_published");
  assert.equal(requests.length, before);

  const preview = await publishArticle({ ...options, mode: "dry-run" });
  assert.equal(preview.status, "dry_run");
  assert.equal(preview.action, "already_published");
  assert.equal(preview.duplicate, true);

  fs.writeFileSync(articlePath, source("正文已修改"));
  await assert.rejects(
    publishArticle(options, { bifrost: fakeBifrost(template), log: () => {} }),
    /已有已发布记录，但正文已变化/,
  );
  assert.equal(requests.length, before);
});

test("non-authentication API errors remain actionable", () => {
  const error = new PublishError("invalid title", { status: 400, code: 100, authFailure: false });
  assert.equal(error.authFailure, false);
  assert.equal(error.status, 400);
});
