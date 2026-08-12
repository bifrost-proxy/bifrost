import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { parseArticle } from "./zhihu-publish.mjs";
import {
  compareArticle,
  htmlToText,
  normalizeArticleId,
  normalizeVerifyBase,
  verifyArticle,
} from "./zhihu-verify.mjs";

const markdown = [
  "---",
  'title: "测试文章"',
  "---",
  "## 痛点",
  "",
  "你好 **Bifrost**。",
  "",
  "1. 第一项",
  "2. [第二项](https://example.com)",
].join("\n");

test("HTML and Markdown normalize to the same visible text", () => {
  const article = parseArticle(markdown);
  assert.equal(htmlToText(article.html), article.text);
  assert.equal(htmlToText('<p>A&nbsp;B &#x4F60;&#22909;</p><img alt="图">'), "A B 你好");
});

test("comparison is exact and rejects duplicated body title", () => {
  const article = parseArticle(markdown);
  assert.equal(compareArticle(article, { title: article.title, content: article.html }).matches, true);
  const mismatch = compareArticle(article, { title: article.title, content: "<p>测试文章</p>" + article.html });
  assert.equal(mismatch.matches, false);
  assert.equal(mismatch.checks.unexpected_duplicate_title, true);
  assert.equal(typeof mismatch.first_difference.index, "number");
});

test("verification target only accepts Zhihu or explicit loopback", () => {
  assert.equal(normalizeArticleId("123"), "123");
  assert.throws(() => normalizeArticleId("x"), /非零数字/);
  assert.equal(normalizeVerifyBase("https://zhuanlan.zhihu.com/"), "https://zhuanlan.zhihu.com");
  assert.equal(normalizeVerifyBase("http://127.0.0.1:4321/", true), "http://127.0.0.1:4321");
  assert.throws(() => normalizeVerifyBase("https://example.com"), /只允许验证/);
});

test("mock E2E verifies the API title and body without a browser", async (context) => {
  const article = parseArticle(markdown);
  const server = http.createServer((request, response) => {
    response.setHeader("content-type", "application/json");
    if (request.url === "/api/articles/123") {
      response.end(JSON.stringify({ id: 123, title: article.title, content: article.html }));
    } else {
      response.statusCode = 404;
      response.end("{}");
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  context.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "zhihu-verify-test-"));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const articlePath = path.join(temporary, "article.md");
  fs.writeFileSync(articlePath, markdown);
  const address = server.address();
  const result = await verifyArticle({
    articleId: "123",
    article: articlePath,
    base: `http://127.0.0.1:${address.port}`,
    allowLoopback: true,
    timeoutMs: 5_000,
  });
  assert.equal(result.status, "verified");
  assert.equal(result.comparison.matches, true);
  assert.equal(result.source_id, "loopback");
});
