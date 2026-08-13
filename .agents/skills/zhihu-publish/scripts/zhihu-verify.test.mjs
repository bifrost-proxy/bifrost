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
  "![示意图](https://example.com/demo.png)",
  "",
  "1. 第一项",
  "2. [第二项](https://example.com)",
].join("\n");

test("HTML and Markdown normalize to the same visible text", () => {
  const article = parseArticle(markdown);
  assert.equal(htmlToText(article.html), article.text);
  assert.equal(htmlToText('<p>A&nbsp;B &#x4F60;&#22909;</p><img alt="图">'), "A B 你好");
  assert.equal(htmlToText("<p><code>client_app</code>、 <code>client_process</code></p>"), "client_app、client_process");
  assert.equal(htmlToText('<p><a href="https://link.zhihu.com/?target=https%3A//github.com/bifrost-proxy/bifrost"><span class="invisible">https://</span><span class="visible">github.com/bifrost-prox</span><span class="invisible">y/bifrost</span><span class="ellipsis"></span></a></p>'), "https://github.com/bifrost-proxy/bifrost");
});

test("Markdown normalization preserves underscores inside inline and fenced code", () => {
  const article = parseArticle([
    "---", 'title: "代码标识符"', "---",
    "普通 *强调* 与 `client_app`。",
    "",
    "```text",
    "client_process client_pid",
    "```",
  ].join("\n"));
  assert.equal(article.text, "普通 强调 与 client_app。 client_process client_pid");
  assert.equal(htmlToText(article.html), article.text);
});

test("Markdown normalization ignores language labels on permissive closing fences", () => {
  const article = parseArticle([
    "---", 'title: "流程图"', "---",
    "```text",
    "飞书",
    "```text",
    "↓",
    "```",
    "Bifrost",
    "```text",
  ].join("\n"));
  assert.equal(article.text, "飞书 ↓ Bifrost");
  assert.equal(htmlToText(article.html), article.text);
});

test("comparison is exact and rejects duplicated body title", () => {
  const article = parseArticle(markdown);
  const hostedHtml = article.html.replace("https://example.com/demo.png", "https://pic4.zhimg.com/demo.png");
  const success = compareArticle(article, { title: article.title, content: hostedHtml });
  assert.equal(success.matches, true);
  assert.equal(success.checks.image_count_match, true);
  assert.equal(success.checks.images_hosted_by_zhihu, true);
  const missingImage = compareArticle(article, { title: article.title, content: hostedHtml.replace(/<p><img[^>]+><\/p>/, "") });
  assert.equal(missingImage.matches, false);
  assert.equal(missingImage.checks.image_count_match, false);
  const externalImage = compareArticle(article, { title: article.title, content: article.html });
  assert.equal(externalImage.matches, false);
  assert.equal(externalImage.checks.images_hosted_by_zhihu, false);
  const mismatch = compareArticle(article, { title: article.title, content: "<p>测试文章</p>" + hostedHtml });
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
  const hostedHtml = article.html.replace("https://example.com/demo.png", "https://pic4.zhimg.com/demo.png");
  const server = http.createServer((request, response) => {
    response.setHeader("content-type", "application/json");
    if (request.url === "/api/articles/123") {
      response.end(JSON.stringify({ id: 123, title: article.title, content: hostedHtml }));
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
  assert.equal(result.comparison.actual.image_count, 1);
  assert.deepEqual(result.comparison.actual.image_hosts, ["pic4.zhimg.com"]);
  assert.equal(result.source_id, "loopback");
});
