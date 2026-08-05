# Videos Tool 下线设计

## 背景

AI 工作台曾提供基于 `yt-dlp` 的 Videos 下载工具。该能力不是 Bifrost 代理与 Agent 主路径的一部分，用户已明确要求从 AI 工具和代码中移除，避免继续暴露低价值入口、维护外部命令依赖与进程内下载任务状态。

## 用户目标验证清单

### 必须实现

- AI 左侧工作入口不再展示 `Videos`。
- 删除 Videos 专用前端页面、API 客户端和 Playwright 功能测试。
- 删除后端 `/api/videos` 路由、handler、下载任务状态以及 `yt-dlp` 调用代码。
- 旧 `view=videos` 与 `aiSection=tools-videos` 深链安全回退到 New Chat，不留下空白内容区。
- 旧 `/_bifrost/api/videos/*` 使用统一未知 API 行为返回 404。

### 必须不破坏

- New Chat、ASR、IM、Threads 与 Settings 的导航、路由和内容保持可用。
- 亮色与暗色主题下 AI 左栏结构一致。
- 不删除代理对普通 `video/*` 响应、HTML `<video>` 元素、流量 Content-Type 展示或语音设备探测中的通用视频语义；这些能力与 Videos 下载工具无关。

### 必须真实验证

- `aiLayout` 单元测试覆盖两种旧 Videos 路由回退到 New Chat。
- `ai-layout-redesign.spec.ts` 在亮色和暗色主题下断言 Videos 入口和页面均不存在，并验证旧深链回退。
- `e2e-tests/tests/test_videos_tool_removed.sh` 启动隔离 Bifrost 服务并断言旧 API 返回 404。
- `human_tests/videos-tool.md` 按当前下线契约逐条执行。

## 实现边界

删除专用实现：

- `crates/bifrost-admin/src/handlers/videos.rs`
- `web/src/api/videos.ts`
- `web/src/pages/AI/VideosTool.tsx`
- `web/tests/ui/videos-tool.spec.ts`
- `handlers/mod.rs` 的 `videos` module 和 Admin router 的 `/api/videos` 分支
- AI route state 中的 `videos` 主视图、左栏按钮与内容挂载

保留通用媒体与代理代码。删除判断以“是否只服务 `/api/videos` 下载工具”为准，不能按单词 `video` 全局清理。

## 路由退场语义

`videos` 不再属于 `AiMainView` 和 `MAIN_VIEWS`。因此：

- `/ai?view=videos` 按未知主视图回退到 `{ view: "chat", chatMode: "new" }`。
- `/ai?aiSection=tools-videos` 不再进入兼容映射，同样回退到 New Chat。
- `/api/videos/*` 不再匹配专用 handler，进入 Admin router 的统一 `API endpoint not found` 404 响应。

不保留重定向页面或 `410 Gone` 专用 API，避免为已移除功能继续维护额外契约。

## 测试方案

- 单元测试：`pnpm --dir web test:unit -- src/pages/AI/aiLayout.test.ts`。
- UI E2E：`pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --grep "removes Videos" --workers=1`。
- 后端 E2E：`bash e2e-tests/tests/test_videos_tool_removed.sh`。
- Web 静态验证：TypeScript build、ESLint，并检索确认专用标识、路由和导入已清零。
- Rust 验证：按 `rust-project-validate` 执行 fmt、clippy、build 和 workspace all-features tests。
- 覆盖率：本地不运行全量覆盖率，交由远端 `coverage-all.sh --json --gate` 90% 门禁兜底。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照用户目标检查专用前后端代码是否完整删除。
- 执行 `git status --short`、`git diff`，确认通用视频代理代码未被误删。
- 运行 route 单测、专项 Playwright 与旧 API E2E。

### 第 2 轮

- 复查最新 diff、文档和 human_tests 索引。
- 全仓检索 `VideosTool`、`/api/videos`、`tools-videos`、`view=videos` 与专用测试标识，只允许下线设计、回归用例和历史执行记录中的说明性引用。
- 复跑受影响测试并进入项目级校验。
