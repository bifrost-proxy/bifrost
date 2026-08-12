# 知乎文章发布 Skill 真实场景测试

## 功能模块说明

验证 `.agents/skills/zhihu-publish` 能只使用 Bifrost 已捕获的网络请求完成 Markdown 预检、知乎登录态探测、草稿创建、正式发布、线上正文比对和重复发布保护，全程不操作 Chrome 或其他浏览器 UI。

## 前置条件

1. Bifrost 代理运行在 `127.0.0.1:9900`，且已有一次知乎编辑器保存与发布流量。
2. 知乎登录态有效，Bifrost 流量中包含创建草稿、更新草稿和正式发布请求。
3. 测试文章为 `/Users/eden_studio/.local/share/bifrost/juejin-articles/pesty-introduction.md`。
4. 在仓库 worktree `/Users/eden_studio/work/github/bifrost-zhihu-publish-skill` 中执行命令。

## 测试用例

### TC-ZP-01：Node 单元测试与 loopback E2E

操作步骤：

1. 执行 `node --test .agents/skills/zhihu-publish/scripts/*.test.mjs`。
2. 确认 mock 服务收到 `GET /api/v4/me`、创建草稿、两次保存和正式发布请求。
3. 确认无浏览器进程或 Playwright 被调用。

预期结果：全部测试通过；覆盖解析、渲染、流量 body 解码、请求模板重建、状态文件权限、完整发布链路、重复保护和无浏览器正文验收。

### TC-ZP-02：真实文章 dry-run

操作步骤：

1. 执行 `.agents/skills/zhihu-publish/scripts/zhihu-publish --article /Users/eden_studio/.local/share/bifrost/juejin-articles/pesty-introduction.md --dry-run`。
2. 检查输出标题、Markdown/HTML/正文字符数与下一步动作。

预期结果：返回 `status=dry_run`；标题为 Pesty 文章标题；不创建草稿、不发布文章。

### TC-ZP-03：Bifrost 登录态和请求模板检查

操作步骤：

1. 执行 `.agents/skills/zhihu-publish/scripts/zhihu-publish --check-auth`。
2. 检查输出只包含请求 ID、Cookie 名称和鉴权状态。

预期结果：返回 `status=authenticated`；创建、更新、发布三个模板均有来源请求 ID；输出不含 Cookie 值、XSRF token 或 `x-zse-*` / `x-zst-*` 值。

### TC-ZP-04：真实发布 Pesty 文章

操作步骤：

1. 执行 `.agents/skills/zhihu-publish/scripts/zhihu-publish --article /Users/eden_studio/.local/share/bifrost/juejin-articles/pesty-introduction.md --publish`。
2. 记录返回的草稿 ID、文章 ID 和公开 URL。
3. 确认状态文件只保存路径、内容哈希、ID、URL 和时间戳，文件权限为 `0600`。

预期结果：返回 `status=published`；公开 URL 为 `https://zhuanlan.zhihu.com/p/<数字ID>`；没有输出或落盘任何认证密钥。

### TC-ZP-05：无浏览器线上标题与正文严格验收

操作步骤：

1. 使用 TC-ZP-04 返回的文章 ID 执行 `.agents/skills/zhihu-publish/scripts/zhihu-verify <文章ID> --article /Users/eden_studio/.local/share/bifrost/juejin-articles/pesty-introduction.md`。
2. 检查标题、正文字符数和三项比对结果。

预期结果：返回 `status=verified`；`title_match=true`、`body_match=true`、`unexpected_duplicate_title=false`；验收通过知乎文章 JSON 接口完成，不启动浏览器。

### TC-ZP-06：重复发布保护

操作步骤：

1. 对同一个源文件再次执行 TC-ZP-04 的发布命令，不传 `--force-new`。
2. 对比文章 ID，并检查 Bifrost 中没有因本次命令新增创建草稿或发布请求。

预期结果：返回 `status=already_published`，文章 ID 与首次发布一致，不创建第二篇文章。

### TC-ZP-07：Skill 结构和敏感信息检查

操作步骤：

1. 执行 `python3 /Users/eden_studio/.codex/skills/.system/skill-creator/scripts/quick_validate.py .agents/skills/zhihu-publish`（使用具备 PyYAML 的环境；若系统 Python 缺依赖，记录环境问题并用静态等价检查补充）。
2. 执行 `rg -n 'z_c0=|_xsrf=|x-zse-96:|x-zst-81:' .agents/skills/zhihu-publish design/zhihu-publish-skill.md human_tests/zhihu-publish.md | rg -v 'secret|another|rg -n'`，排除单元测试中的显式假值和本条检查命令自身。
3. 检查所有 shell 包装器为可执行文件。

预期结果：Skill 结构合法；仓库文件中没有真实 Cookie/token/签名值；包装器可直接运行。

## 清理步骤

1. 保留已发布的 Pesty 知乎文章，这是用户要求的交付物。
2. 删除测试使用的临时目录和 loopback 状态文件；保留正式幂等状态文件以避免重复发布。
3. 不清理 Bifrost 历史流量，避免破坏用户既有调试数据。

## 2026-08-13 执行记录

- TC-ZP-01：通过，11/11 Node 单元与 loopback E2E 用例通过。
- TC-ZP-02：通过，Pesty 文章 dry-run 返回字符统计和幂等动作，未写入知乎。
- TC-ZP-03：通过，从 Bifrost 找到创建、更新、发布请求模板并通过 `/api/v4/me` 验证登录态，未输出认证值。
- TC-ZP-04：通过，发布文章 ID `2071054898267943107`，公开地址为 `https://zhuanlan.zhihu.com/p/2071054898267943107`；状态文件权限 `0600` 且字段白名单符合预期。
- TC-ZP-05：首次执行发现图片 alt 不属于知乎可见正文，修正归一化后复测通过；标题一致、正文归一化字符数均为 1194、无重复标题。
- TC-ZP-06：通过，第二次发布返回 `already_published`，文章 ID 未变化。
- TC-ZP-07：通过，官方 `quick_validate.py` 返回 `Skill is valid!`；真实敏感值扫描无命中；两个包装器权限均为 `-rwxr-xr-x`。
