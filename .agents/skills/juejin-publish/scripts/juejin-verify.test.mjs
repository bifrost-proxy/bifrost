import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";

import { parseArticle } from "./juejin-publish.mjs";
import {
  compareArticlePage,
  markdownToText,
  normalizeVerificationUrl,
  parseArgs,
  pageTextToText,
  urlFromArticleId,
  verifyWithEdge,
} from "./juejin-verify.mjs";

function article(markdown = "## 正文\n\n你好 **Bifrost**。") {
  return parseArticle([
    "---",
    'title: "测试标题"',
    'category_id: "1000000000000000001"',
    'tag_ids: ["2000000000000000002"]',
    "---",
    markdown,
  ].join("\n"));
}

test("markdown and rendered page text normalize to the same content", () => {
  assert.equal(
    markdownToText("## 标题\n\n1. **第一项**\n2. [第二项](https://example.com)\n\n```text\nhello\n```"),
    "标题 第一项 第二项 hello",
  );
  assert.equal(pageTextToText("标题\n1. 第一项\n2. 第二项\nhello"), "标题 第一项 第二项 hello");
  assert.equal(
    markdownToText("```bash\nexport FEISHU_APP_SECRET=x\necho '*.local'\n```"),
    "export FEISHU_APP_SECRET=x echo '*.local'",
  );
});

test("comparison requires exact title and normalized body", () => {
  const result = compareArticlePage(article(), {
    title: "测试标题",
    body: "正文\n你好 Bifrost。",
    titleSelector: ".article-title",
    bodySelector: ".article-content",
  });
  assert.equal(result.matches, true);
  assert.deepEqual(result.checks, {
    title_match: true,
    body_match: true,
    unexpected_duplicate_title: false,
  });
});

test("comparison reports title mismatch and first body difference", () => {
  const result = compareArticlePage(article(), {
    title: "另一个标题",
    body: "正文\n你好 Codex。",
    titleSelector: "h1",
    bodySelector: "article",
  });
  assert.equal(result.matches, false);
  assert.equal(result.checks.title_match, false);
  assert.equal(result.checks.body_match, false);
  assert.equal(typeof result.first_difference.index, "number");
});

test("comparison detects an unexpected duplicated title in article body", () => {
  const result = compareArticlePage(article(), {
    title: "测试标题",
    body: "测试标题\n正文\n你好 Bifrost。",
    titleSelector: ".article-title",
    bodySelector: ".article-content",
  });
  assert.equal(result.matches, false);
  assert.equal(result.checks.unexpected_duplicate_title, true);
});

test("verification URL accepts Juejin posts and rejects unrelated pages", () => {
  assert.equal(urlFromArticleId("7672957997422346303"), "https://juejin.cn/post/7672957997422346303");
  assert.throws(() => urlFromArticleId("not-an-id"), /非零数字/);
  assert.equal(
    normalizeVerificationUrl("https://juejin.cn/post/7672957997422346303?from=test#section"),
    "https://juejin.cn/post/7672957997422346303",
  );
  assert.throws(() => normalizeVerificationUrl("https://juejin.cn/editor/drafts/new"), /只允许验证/);
  assert.throws(() => normalizeVerificationUrl("https://example.com/post/123"), /只允许验证/);
  assert.equal(
    normalizeVerificationUrl("http://127.0.0.1:4321/post/123", true),
    "http://127.0.0.1:4321/post/123",
  );
});

test("direct verifier calls reject an invalid close delay before opening Edge", async () => {
  await assert.rejects(
    verifyWithEdge({
      article: null,
      url: "http://127.0.0.1:4321/post/123",
      headless: true,
      allowLoopback: true,
      timeoutMs: 15_000,
      closeDelayMs: -1,
    }),
    /--close-delay-ms 必须是 0 到 60000 之间的整数/,
  );
});

test("refresh CLI count defaults to one and accepts two or three", () => {
  assert.equal(parseArgs(["123", "--refresh"]).refreshCount, 1);
  assert.equal(parseArgs(["123", "--refresh", "2"]).refreshCount, 2);
  assert.equal(parseArgs(["123", "--refresh", "3"]).refreshCount, 3);
  assert.throws(() => parseArgs(["123", "--refresh", "11"]), /1 到 10/);
});

test("refresh count is bounded consistently for direct verifier calls", async () => {
  await assert.rejects(
    verifyWithEdge({
      article: null,
      url: "http://127.0.0.1:4321/post/123",
      headless: true,
      allowLoopback: true,
      timeoutMs: 15_000,
      closeDelayMs: 0,
      refreshCount: 11,
      refreshDelayMs: 0,
    }),
    /1 到 10/,
  );
});

test("Microsoft Edge loads a mock article and refreshes it three times", async (context) => {
  let articleRequests = 0;
  const server = http.createServer((request, response) => {
    if (request.url === "/post/123") articleRequests += 1;
    const body = `<!doctype html><html><body><main>
      <h1 class="article-title">测试标题</h1>
      <div class="article-content"><h2>正文</h2><p>你好 <strong>Bifrost</strong>。</p></div>
    </main></body></html>`;
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  context.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const address = server.address();
  const startedAt = Date.now();
  const result = await verifyWithEdge({
    article: null,
    url: `http://127.0.0.1:${address.port}/post/123`,
    headless: true,
    allowLoopback: true,
    timeoutMs: 15_000,
    closeDelayMs: 25,
    refreshCount: 3,
    refreshDelayMs: 25,
  });
  assert.equal(result.status, "loaded");
  assert.equal(result.loaded, true);
  assert.equal(result.article_id, "123");
  assert.equal(result.http_status, 200);
  assert.equal(result.browser.name, "Microsoft Edge");
  assert.equal(result.navigation_count, 4);
  assert.equal(result.close_delay_ms, 25);
  assert.deepEqual(result.refresh, {
    requested: 3,
    performed: 3,
    delay_ms: 25,
    complete: true,
    attempts: [1, 2, 3].map((index) => ({
      index,
      http_status: 200,
    })),
  });
  assert.ok(Date.now() - startedAt >= 75);
  assert.equal(articleRequests, 4);
});

test("Microsoft Edge refreshes every requested time and checks content only at the end", async (context) => {
  let articleRequests = 0;
  const progress = [];
  const server = http.createServer((request, response) => {
    if (request.url === "/post/123") articleRequests += 1;
    const body = `<!doctype html><html><body><main>
      <h1 class="article-title">测试标题</h1>
      <div class="article-content"><p>第 ${articleRequests} 次加载</p></div>
    </main></body></html>`;
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  context.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const address = server.address();
  const result = await verifyWithEdge({
    article: null,
    url: `http://127.0.0.1:${address.port}/post/123`,
    headless: true,
    allowLoopback: true,
    timeoutMs: 15_000,
    closeDelayMs: 0,
    refreshCount: 5,
    refreshDelayMs: 0,
    onProgress: (message) => progress.push(message),
  });
  assert.equal(result.status, "loaded");
  assert.equal(result.navigation_count, 6);
  assert.equal(result.refresh.requested, 5);
  assert.equal(result.refresh.performed, 5);
  assert.equal(result.refresh.complete, true);
  assert.equal(result.refresh.attempts.length, 5);
  assert.equal(articleRequests, 6);
  assert.deepEqual(
    progress.filter((message) => message.includes(" complete")),
    [
      ...[1, 2, 3, 4, 5].map((index) => `refresh ${index}/5 complete`),
      "final content check complete",
    ],
  );
});
