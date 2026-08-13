# 知乎文章发布 Skill 设计

## 目标

在 `.agents/skills/zhihu-publish/` 提供一套可重复执行的 Markdown 文章预检、草稿保存、正式发布和线上验收流程。登录态只从 Bifrost 已捕获的知乎请求中临时读取，不保存 Cookie、XSRF token 或知乎签名头。

## 已验证的真实请求链路

2026-08-13 通过 Bifrost 还原文章 `2071038899124807536` 的发布过程：

1. `POST https://zhuanlan.zhihu.com/api/articles/drafts` 创建空草稿。
2. `PATCH https://zhuanlan.zhihu.com/api/articles/{draft_id}/draft` 保存标题。
3. `PATCH https://zhuanlan.zhihu.com/api/articles/{draft_id}/draft` 保存 HTML 正文和目录设置。
4. `POST https://www.zhihu.com/api/v4/content/publish` 发布文章。
5. 成功响应中的 `data.result` 是 JSON 字符串，`publish.id` 为公开文章 ID。
6. 浏览器随后访问 `https://zhuanlan.zhihu.com/p/{article_id}?just_published=2`。

创建、保存和发布请求均使用 Cookie、`x-xsrftoken`、`x-zse-*` / `x-zst-*` 等头。不同 host 或接口的知乎签名头不可假定通用，因此脚本分别读取最近的专栏草稿请求和正式发布请求作为模板。

## 文件结构

- `SKILL.md`：操作顺序、安全规则和失败恢复。
- `scripts/zhihu-publish.mjs`：解析文章、Markdown 转 HTML、从 Bifrost 读取请求模板、创建/更新/发布及幂等状态管理。
- `scripts/zhihu-verify.mjs`：使用 Bifrost 捕获的知乎请求头读取公开页或文章详情，并严格比较标题与正文文本。
- `references/article.template.md`：最小文章 frontmatter。
- `references/traffic-contract.md`：真实接口与字段契约，仅在排查接口变更时读取。

## 安全与幂等

- 不向终端、测试日志、状态文件或最终回复输出 Cookie、token、原始 headers。
- 状态文件只保存源文件路径、内容哈希、draft/article ID 和 URL，权限为 `0600`。
- 同一路径或同一内容哈希已发布且正文未变化时返回 `already_published`；已发布源文件发生变化时拒绝静默复用旧文章。
- `--force-new` 仅在用户明确要求重复发布时使用。
- API base 仅允许知乎正式域名或 loopback mock，防止把凭证发往第三方。
- 非鉴权错误不按登录过期重试；鉴权失败最多重新从 Bifrost 读取一次最新捕获。

## 验证范围

- Node 单元测试：frontmatter、Markdown/HTML、鉴权提取、URL 限制、幂等状态和错误分类。
- Mock E2E：创建草稿、标题/正文 PATCH、正式发布、重复发布保护和严格正文验收。
- 真实 human test：对真实知乎账号执行 dry-run、鉴权检查、发布 Pesty 文章、线上读取与正文对比。
- 线上验收通过 `GET /api/articles/{id}` 完成，不启动或操控浏览器。
- 本任务不修改 Rust 生产代码，不运行 Cargo、Rust coverage 或 workspace local-ci。
