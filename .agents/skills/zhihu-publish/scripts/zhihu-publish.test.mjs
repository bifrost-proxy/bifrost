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
  isZhihuImageUrl,
  isZhihuUploadImageUrl,
  loadState,
  normalizeApiBase,
  parseArticle,
  parseArgs,
  processArticleImages,
  publishArticle,
  summarizeArticleImages,
  uploadRemoteImage,
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
    "![示意图](https://example.com/demo.gif)",
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
      ["x-zse-96", "endpoint-specific-signature"],
      ["x-zst-81", "endpoint-specific-signature"],
      ["user-agent", "Test Agent"],
      ["content-length", "999"],
    ]),
  };
}

function fakeBifrost(template, includeCreate = true) {
  return {
    findTemplates() {
      return {
        create: includeCreate ? context("REQ-create") : null,
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
  assert.match(article.html, /<img src="https:\/\/example.com\/demo.gif" alt="示意图">/);
  assert.equal(article.text, "为什么需要它 你好 Bifrost。 不再手抄请求 保留登录态");
  assert.deepEqual(summarizeArticleImages(article), {
    image_count: 1,
    remote_images: 1,
    local_images: 0,
    data_images: 0,
    zhihu_images: 0,
  });
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
  assert.equal(isZhihuImageUrl("https://pic4.zhimg.com/demo.gif"), true);
  assert.equal(isZhihuImageUrl("https://pic-private.zhihu.com/demo.gif"), false);
  assert.equal(isZhihuUploadImageUrl("https://pic-private.zhihu.com/demo.gif"), true);
  assert.equal(isZhihuImageUrl("http://pic4.zhimg.com/demo.gif"), false);
  assert.equal(isZhihuUploadImageUrl("https://pic-private.zhihu.com.evil.example/demo.gif"), false);
  assert.throws(() => parseArgs(["--publish", "--force-new", "--update-existing"]), /不能同时使用/);
});

test("image processing uploads remote, local, and data images and wraps standalone figures", async (contextHandle) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "zhihu-images-test-"));
  contextHandle.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64");
  fs.writeFileSync(path.join(temporary, "local.png"), png);
  const dataUri = "data:image/png;base64," + png.toString("base64");
  const article = parseArticle([
    "---", "title: images", "---",
    "![remote](https://example.com/a.gif)",
    "![local](./local.png)",
    `![inline](${dataUri})`,
    "![hosted](https://pic4.zhimg.com/already.png)",
  ].join("\n"));
  const calls = [];
  const fakeFetch = async (url, options) => {
    calls.push({ url: String(url), options });
    const index = calls.length;
    return new Response(JSON.stringify({ src: `https://pic-private.zhihu.com/uploaded-${index}.png` }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const processed = await processArticleImages(
    article,
    path.join(temporary, "article.md"),
    "https://zhuanlan.zhihu.com",
    context(),
    fakeFetch,
  );
  assert.equal(calls.length, 3);
  assert.equal(calls[0].options.body instanceof URLSearchParams, true);
  assert.equal(calls[0].options.body.get("source"), "article");
  assert.equal(calls[0].options.headers["x-zse-96"], undefined);
  assert.equal(calls[0].options.headers["x-zst-81"], undefined);
  assert.equal(calls[0].options.headers["x-xsrftoken"], "secret");
  assert.equal(calls[1].options.body instanceof FormData, true);
  assert.equal(calls[1].options.headers["content-type"], undefined);
  assert.equal(processed.imageSummary.image_count, 4);
  assert.equal(processed.imageSummary.zhihu_images, 4);
  assert.equal((processed.html.match(/<figure data-size="normal">/g) ?? []).length, 4);
  assert.doesNotMatch(processed.html, /<p>\s*<figure/);
});

test("image upload rejects unsafe response hosts and unsupported local content", async (contextHandle) => {
  await assert.rejects(
    uploadRemoteImage("https://zhuanlan.zhihu.com", context(), "https://example.com/a.png", async () =>
      new Response(JSON.stringify({ src: "https://evil.example/a.png" }), { status: 200 })),
    /官方图片地址/,
  );
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "zhihu-invalid-image-"));
  contextHandle.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  fs.writeFileSync(path.join(temporary, "fake.png"), "not an image");
  const article = parseArticle("---\ntitle: invalid\n---\n![bad](./fake.png)");
  await assert.rejects(
    processArticleImages(article, path.join(temporary, "article.md"), "https://zhuan.zhihu.com", context(), async () => {}),
    /格式与内容不匹配/,
  );
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
    const contentType = request.headers["content-type"] ?? "";
    let parsedBody = null;
    if (body && contentType.includes("application/json")) parsedBody = JSON.parse(body);
    else if (body && contentType.includes("application/x-www-form-urlencoded")) parsedBody = Object.fromEntries(new URLSearchParams(body));
    requests.push({ method: request.method, url: request.url, body: parsedBody, rawBody: body, contentType });
    response.setHeader("content-type", "application/json");
    if (request.url === "/api/v4/me") response.end(JSON.stringify({ id: "user-1", name: "tester" }));
    else if (request.url === "/api/uploaded_images") response.end(JSON.stringify({ src: "https://pic4.zhimg.com/demo.gif" }));
    else if (request.url === "/api/articles/drafts") response.end(JSON.stringify({ id: "9001" }));
    else if (request.url === "/api/articles/9001/draft") response.end("{}");
    else if (request.url === "/api/v4/content/publish") {
      response.end(JSON.stringify(parsedBody?.data?.draft?.isPublished
        ? { code: 0, data: {} }
        : { code: 0, data: { result: JSON.stringify({ publish: { id: "8001" } }) } }));
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
    updateExisting: false,
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
    image_count: 1,
  });
  assert.deepEqual(requests.map((item) => [item.method, item.url]), [
    ["GET", "/api/v4/me"],
    ["POST", "/api/uploaded_images"],
    ["POST", "/api/articles/drafts"],
    ["PATCH", "/api/articles/9001/draft"],
    ["PATCH", "/api/articles/9001/draft"],
    ["POST", "/api/v4/content/publish"],
  ]);
  assert.deepEqual(requests[1].body, { url: "https://example.com/demo.gif", source: "article" });
  assert.equal(requests[3].body.title, "测试文章");
  assert.match(requests[4].body.content, /<figure data-size="normal"><img src="https:\/\/pic4\.zhimg\.com\/demo\.gif"/);
  assert.match(requests[5].body.data.hybrid.html, /pic4\.zhimg\.com/);
  assert.equal(requests[5].body.data.draft.id, "9001");
  assert.equal(fs.statSync(stateFile).mode & 0o777, 0o600);
  assert.equal(loadState(stateFile)[Object.keys(loadState(stateFile))[0]].article_id, "8001");

  const before = requests.length;
  const duplicate = await publishArticle(options, { bifrost: fakeBifrost(template), log: () => {} });
  assert.equal(duplicate.status, "already_published");
  assert.equal(requests.length, before);

  const updated = await publishArticle(
    { ...options, updateExisting: true },
    { bifrost: fakeBifrost(template, false), log: () => {} },
  );
  assert.equal(updated.status, "updated");
  assert.equal(updated.article_id, "8001");
  assert.equal(requests.at(-1).body.data.draft.isPublished, true);
  assert.equal(requests.filter((item) => item.url === "/api/articles/drafts").length, 1);
  const afterUpdate = requests.length;

  const preview = await publishArticle({ ...options, mode: "dry-run" });
  assert.equal(preview.status, "dry_run");
  assert.equal(preview.action, "already_published");
  assert.equal(preview.duplicate, true);

  fs.writeFileSync(articlePath, source("正文已修改"));
  await assert.rejects(
    publishArticle(options, { bifrost: fakeBifrost(template), log: () => {} }),
    /已有已发布记录，但正文已变化/,
  );
  assert.equal(requests.length, afterUpdate);
});

test("non-authentication API errors remain actionable", () => {
  const error = new PublishError("invalid title", { status: 400, code: 100, authFailure: false });
  assert.equal(error.authFailure, false);
  assert.equal(error.status, 400);
});
