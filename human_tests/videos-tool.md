# Videos Tool 下线真实场景测试

## 功能模块说明

验证原 AI Videos 下载工具已完整下线：用户在亮色和暗色主题下都看不到 Videos 入口，旧前端深链安全回退到 New Chat，旧 `/api/videos` 接口返回 404，专用前后端实现已删除，同时不影响 ASR、IM、Settings 和通用视频流量处理。

## 前置条件

1. 工作目录为 `<REPO_ROOT>`，当前分支包含本次下线改动。
2. 已安装 `web/` 依赖和 Playwright 浏览器。
3. 后端 E2E 使用临时 `BIFROST_DATA_DIR`、动态端口、`--no-system-proxy` 和 `--no-intercept`，不得停止、重启或调用正式 9900 服务。
4. 本用例不需要 `yt-dlp`、`ffmpeg` 或真实 YouTube 网络访问。

## 测试用例

### TC-VT-REMOVED-01 AI 导航与旧深链退场

操作步骤：

1. 执行：
   ```bash
   pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --grep "removes Videos" --workers=1
   ```
2. 测试分别设置亮色和暗色主题打开 `/_bifrost/ai?view=videos`。
3. 再打开旧链接 `/_bifrost/ai?aiSection=tools-videos`。

预期结果：

- 两种主题下均不存在 `ai-nav-tools-videos`、名为 `Videos` 的导航按钮和 `videos-tool-page`。
- `view=videos` 与 `aiSection=tools-videos` 都展示 New Chat 输入态，不出现空白内容区。
- New Chat 入口为选中态，页面不存在 `ai-videos-content`。

### TC-VT-REMOVED-02 旧 Admin API 返回 404

操作步骤：

1. 执行：
   ```bash
   bash e2e-tests/tests/test_videos_tool_removed.sh
   ```
2. 脚本构建最新二进制，使用临时数据目录和动态端口启动隔离 Bifrost。
3. 脚本确认目标 PID 监听目标端口后，请求 `GET /_bifrost/api/videos/defaults`。

预期结果：

- HTTP 状态码为 404。
- JSON 为 `{"error":"API endpoint not found","status":404}`。
- 脚本只清理自己创建的进程和临时目录，不影响正式服务。

### TC-VT-REMOVED-03 专用实现清零且通用媒体能力保留

操作步骤：

1. 执行以下专用实现检索：
   ```bash
   test ! -e crates/bifrost-admin/src/handlers/videos.rs
   test ! -e web/src/api/videos.ts
   test ! -e web/src/pages/AI/VideosTool.tsx
   test ! -e web/tests/ui/videos-tool.spec.ts
   ! rg -n 'pub mod videos|videos::handle_videos|path\.starts_with\("/api/videos"\)|VideoCameraOutlined|setMainView\("videos"\)' crates/bifrost-admin/src web/src
   ```
2. 执行通用媒体能力检索：
   ```bash
   rg -n 'video/mp4|ct\.starts_with\("video/"\)|<video' crates/bifrost-proxy/src
   ```

预期结果：

- 四个 Videos 专用文件均不存在，module、router、图标和主视图挂载检索无结果。
- 通用代理代码仍包含视频 Content-Type、`video/*` 处理或 HTML `<video>` 解析逻辑，证明没有按关键词误删代理能力。

### TC-VT-REMOVED-04 其它 AI 工作入口回归

操作步骤：

1. 执行：
   ```bash
   pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --grep "left rail switches|Settings does not trap" --workers=1
   ```
2. 依次切换 New Chat、ASR、IM、Settings 和历史线程。

预期结果：

- ASR、IM、Settings 与历史线程仍能切换并展示对应内容。
- 离开 Settings 时不被残留参数拉回 Settings。
- Videos 入口不存在，删除后导航结构在桌面布局中保持稳定。

## 清理步骤

- Playwright 结束后由测试框架停止临时前端。
- 后端 E2E 的 trap 定向停止测试 PID 并删除自己的临时数据目录。
- 不产生下载文件，也不修改用户的 Downloads 目录。

## 执行记录

- 2026-08-05：TC-VT-REMOVED-01 通过。执行 `pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --grep "removes Videos|left rail switches|Settings does not trap" --workers=1`，结果 `3 passed`；其中专项用例在亮色和暗色主题分别打开 `view=videos`，均确认 Videos 按钮、`videos-tool-page` 和内容轨道不存在，页面回退到选中的 New Chat；旧 `aiSection=tools-videos` 同样回退。
- 2026-08-05：TC-VT-REMOVED-02 通过。执行 `bash e2e-tests/tests/test_videos_tool_removed.sh`，最新 debug 二进制在临时数据目录和动态端口启动，PID/监听检查通过；`GET /_bifrost/api/videos/defaults` 返回 404 与 `{"error":"API endpoint not found","status":404}`，脚本退出时只清理测试进程和临时目录。
- 2026-08-05：TC-VT-REMOVED-03 通过。四个专用实现文件均不存在，module/router/icon/view 挂载检索无结果；通用代理仍命中 `ct.starts_with("video/")`、`video/mp4` 和 HTML `<video>` 测试，未误删通用媒体能力。
- 2026-08-05：TC-VT-REMOVED-04 通过。同一 Playwright 命令中的 `AI left rail switches ASR, IM, Settings, and history threads` 与 `AI Settings does not trap left rail navigation` 均通过，确认其它 AI 主入口与 Settings 退场行为未回归。
- 历史 Videos 下载功能的执行记录由 Git 历史保留，不再作为当前产品契约。
