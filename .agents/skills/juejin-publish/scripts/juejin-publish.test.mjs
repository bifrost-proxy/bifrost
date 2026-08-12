import assert from "node:assert/strict";
import test from "node:test";

import {
  PublishError,
  cookieNames,
  extractAuth,
  isAuthFailure,
  maskedWordCount,
  normalizeApiBase,
  parseArticle,
  realWordCount,
  trimForWordCount,
} from "./juejin-publish.mjs";

test("parseArticle reads required metadata and preserves markdown", () => {
  const article = parseArticle([
    "---",
    'title: "Bifrost 发布能力"',
    'category_id: "1000000000000000001"',
    'tag_ids: ["2000000000000000002"]',
    "sync_to_org: false",
    "---",
    "# 正文",
    "",
    "Hello Bifrost.",
  ].join("\n"));
  assert.equal(article.title, "Bifrost 发布能力");
  assert.deepEqual(article.tagIds, ["2000000000000000002"]);
  assert.equal(article.syncToOrg, false);
  assert.match(article.markdown, /Hello Bifrost/);
});

test("parseArticle rejects incomplete publication metadata", () => {
  assert.throws(
    () => parseArticle("---\ntitle: no category\n---\nbody"),
    (error) => error instanceof PublishError && /category_id/.test(error.message),
  );
  assert.throws(
    () => parseArticle("---\ntitle: no tags\ncategory_id: 123\n---\nbody"),
    (error) => error instanceof PublishError && /tag_ids/.test(error.message),
  );
});

test("word count matches the captured Juejin worker algorithm", () => {
  assert.equal(realWordCount("你好 Bifrost 2026"), 7);
  assert.equal(maskedWordCount(976), 1078316);
  assert.equal(trimForWordCount("![cover](https://example.com/a.png)\n你好").trim(), "你好");
});

test("extractAuth reads headers without exposing values in cookieNames", () => {
  const auth = extractAuth({
    id: "REQ-test",
    url: "https://api.juejin.cn/content_api/v1/article_draft/update?aid=2608&uuid=",
    request_headers: [
      ["cookie", "sessionid=secret; csrf_session_id=another"],
      ["x-secsdk-csrf-token", "csrf-secret"],
      ["user-agent", "Test Agent"],
    ],
  });
  assert.equal(auth.aid, "2608");
  assert.equal(auth.cookie, "sessionid=secret; csrf_session_id=another");
  assert.deepEqual(cookieNames(auth.cookie), ["csrf_session_id", "sessionid"]);
});

test("extractAuth rejects captures without a login session", () => {
  assert.throws(
    () => extractAuth({
      id: "REQ-test",
      url: "https://api.juejin.cn/",
      request_headers: [
        ["cookie", "csrf_session_id=value"],
        ["x-secsdk-csrf-token", "csrf"],
      ],
    }),
    /登录 Cookie 不完整/,
  );
});

test("authentication failure classification is conservative", () => {
  assert.equal(isAuthFailure(401, 0, ""), true);
  assert.equal(isAuthFailure(200, 403, ""), true);
  assert.equal(isAuthFailure(200, 0, "not logged in"), true);
  assert.equal(isAuthFailure(400, 2001, "invalid category"), false);
});

test("API base only accepts Juejin production or loopback endpoints", () => {
  assert.equal(normalizeApiBase("https://api.juejin.cn/"), "https://api.juejin.cn");
  assert.equal(normalizeApiBase("http://127.0.0.1:4321/"), "http://127.0.0.1:4321");
  assert.throws(() => normalizeApiBase("http://api.juejin.cn"), /拒绝/);
  assert.throws(() => normalizeApiBase("https://example.com"), /拒绝/);
});
