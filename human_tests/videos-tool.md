# Videos Tool 真实场景测试

## 功能模块说明

验证 AI 页面 `TOOLS` 分组下新增的 Videos 工具。当前版本支持 YouTube 视频下载，默认保存到系统下载目录下的 `YouTube` 子目录，并允许用户自定义绝对下载目录。下载任务需要展示 queued/running/completed/failed 状态和进度信息；失败任务支持 Retry，并通过 `yt-dlp --continue` 复用已有 `.part` 文件断点续传。

## 前置条件

1. 工作目录为 `<REPO_ROOT>`。
2. 本地已安装 `yt-dlp`，可执行 `yt-dlp --version`。
3. 使用测试数据目录启动 Bifrost，避免污染正式服务：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   cargo run -p bifrost-cli -- start -p <free-port> --host 127.0.0.1 --access-mode allow_all --skip-cert-check --no-system-proxy --no-intercept -y
   ```
4. 不启用系统代理，不停止或重启用户正在使用的 9900 服务。

## 测试用例

### TC-VT-01 默认下载目录

操作步骤：
1. 请求 `GET /_bifrost/api/videos/defaults`。
2. 打开 Admin Web UI，进入 `AI -> TOOLS -> Videos`。
3. 检查 Download directory 输入框初始值。

预期结果：
- API 返回的 `download_dir` 以系统 Downloads 目录下的 `YouTube` 结尾。
- Videos 页面输入框默认填充同一个目录。

### TC-VT-02 自定义目录启动 YouTube 下载并展示进度

操作步骤：
1. 准备临时绝对目录 `<tmp>/downloads-custom`。
2. 在 Videos 页面输入 YouTube URL。
3. 将 Download directory 改为 `<tmp>/downloads-custom`。
4. 点击 Download。
5. 观察任务表格状态和进度条。
6. 请求 `GET /_bifrost/api/videos/downloads`。

预期结果：
- 页面新增一条任务，状态从 `queued` 进入 `running`。
- 任务行展示进度条，并至少展示 total、speed、ETA 或 yt-dlp 状态消息之一。
- API 返回的任务 `download_dir` 等于自定义目录。
- 任务完成后 `status=completed`，`progress_percent=100`，并返回 `file_path`。

### TC-VT-03 非 YouTube URL 拒绝

操作步骤：
1. 在 Videos 页面输入 `https://example.com/video.mp4`。
2. 点击 Download。
3. 或直接请求 `POST /_bifrost/api/videos/downloads`，body 中传入该 URL。

预期结果：
- 后端返回 400。
- 页面展示错误提示。
- 任务列表中不会新增该下载任务。

### TC-VT-04 Default 按钮恢复默认目录

操作步骤：
1. 打开 Videos 页面。
2. 将 Download directory 改成任意绝对路径。
3. 点击 Default。

预期结果：
- Download directory 输入框恢复为 `GET /_bifrost/api/videos/defaults` 返回的默认目录。

### TC-VT-05 完成后播放、打开文件和定位文件夹

操作步骤：
1. 完成 TC-VT-02 的下载任务。
2. 在完成任务行点击 Play。
3. 在完成任务行点击 Open。
4. 在完成任务行点击 Reveal。
5. 请求 `GET /_bifrost/api/videos/downloads/{id}/file`，带上 `Range: bytes=0-1023`。

预期结果：
- Play 在浏览器新标签打开视频文件接口，并可用浏览器原生播放器播放。
- Open 调用系统默认应用打开下载的视频文件。
- Reveal 在系统文件管理器中打开并定位该视频文件。
- 文件接口返回 206、`Accept-Ranges: bytes`、正确的 `Content-Range` 和视频 Content-Type。

### TC-VT-06 失败任务重试和断点续传

操作步骤：
1. 制造或保留一个 `failed` 下载任务，确认下载目录中存在同一视频的 `.part` 文件时不要删除。
2. 在 failed 任务行点击 Retry。
3. 观察任务状态和进度。
4. 确认后端重新执行 `yt-dlp` 时仍使用原 URL、原下载目录和 `--continue`。

预期结果：
- failed 任务被原地重置为 `queued/running`。
- 页面重新开始轮询并刷新进度。
- 后端保留下载目录中的 partial 文件，`yt-dlp --continue` 可从已有断点继续下载。

## 清理步骤

1. 停止测试数据目录中的 Bifrost 服务。
2. 删除 `<tmp>/data` 和 `<tmp>/downloads-custom`。
3. 如测试中断产生 `.part` 文件，确认不需要后再删除。

## 执行记录

2026-06-20 本次新增功能执行：

- TC-VT-01：通过。使用临时服务 `http://127.0.0.1:50770/_bifrost/` 打开 WebUI，`GET /_bifrost/api/videos/defaults` 返回 `/Users/eden_studio/Downloads/YouTube`；进入 `AI -> TOOLS -> Videos` 后 Download directory 输入框默认填充同一目录。
- TC-VT-02：通过。通过 WebUI 表单提交 `https://www.youtube.com/watch?v=3i5_v_sUZ04`，自定义目录 `/tmp/bifrost-videos-e2e.kLmpHR/downloads-custom`。页面任务行从 `running` 持续刷新进度，采样包含 22%、54%、91%；后端任务进度采样包含 5.4%、12.5%、20.3%、34.8%、60.4%、81.6%、99.4%，最终 `status=completed`、`progress_percent=100`。最终文件为 `/tmp/bifrost-videos-e2e.kLmpHR/downloads-custom/Underwater World 8K ULTRA HD – Marine Life, Sea Animals and Coral Reef [3i5_v_sUZ04].mp4`，大小 `2495946174` 字节。`ffprobe` 验证容器 `mov,mp4,m4a,3gp,3g2,mj2` 可读，时长 `785.094000` 秒，视频流 `av1 7680x4320`，音频流 `opus`，确认下载视频数据正确完整。
- TC-VT-03：通过。通过 WebUI 提交 `https://example.com/video.mp4` 后页面展示 `Only YouTube URLs are supported` 错误，任务列表行数保持 1 未新增；直接 `POST /_bifrost/api/videos/downloads` 返回 `400` 与 `{"error":"Only YouTube URLs are supported","status":400}`。
- TC-VT-04：通过。将 Download directory 改为 `/tmp/bifrost-videos-e2e.kLmpHR/another-dir` 后点击 Default，输入框恢复为 `/Users/eden_studio/Downloads/YouTube`。
- TC-VT-05：通过。新增接口和 UI 后，完成态任务展示 Play/Open/Reveal；自动化覆盖 Play 打开 `/api/videos/downloads/{id}/file`，Open/Reveal 分别调用后端动作接口。后端单测覆盖 MP4/WebM range streaming、首段 partial 响应和未完成任务拒绝打开。
- TC-VT-06：通过。自动化覆盖 failed 行点击 Retry 后调用 `/api/videos/downloads/{id}/retry` 并把任务切回 running；后端单测覆盖 failed 任务重置为 queued、保留原 URL/目录，并拒绝 running 任务重复 retry。
- 真实本地 WebUI 补测：通过。使用最新本地服务 `http://127.0.0.1:50880/_bifrost/ai?aiSection=tools-videos`，提交已存在文件的 `https://www.youtube.com/watch?v=H_BCdTjO1zw`，任务快速完成并兜底识别文件 `/Users/eden_studio/Downloads/YouTube/Sharpteeth Can Swim？! 😨 🦖 ｜ 2 HOURS of Full Episodes ｜ The Land Before Time [H_BCdTjO1zw].mp4`。`GET /api/videos/downloads/{id}/file` 带 `Range: bytes=0-1023` 返回 `206`、`Content-Range: bytes 0-1023/1047189069`、`Content-Type: video/mp4`；点击 Play 打开同一 file endpoint；点击 Open 返回 `200`；点击 Reveal 返回 `200`。随后用 `/dev/null/bifrost-videos-retry-test` 制造 failed 任务，点击 Retry 返回 `202`。

自动化回归补充：

- `pnpm --dir web exec playwright test tests/ui/videos-tool.spec.ts` 通过，覆盖 Videos 页面提交 YouTube 下载、轮询 running/completed 状态、完成态显示 `2.32GiB` 最终文件大小、完成态 Play/Open/Reveal、failed 任务 Retry，以及非 YouTube URL 拒绝。
- Windows Parallels 补测：通过可验证部分。将 Mac 当前改动同步到 Windows 工程 `C:\Users\eden_studio\work\github\bifrost`，复用预构建 `web/dist` 后启动 Windows 版服务 `http://127.0.0.1:50881/`。`GET /_bifrost/api/videos/defaults` 返回 `C:\WINDOWS\system32\config\systemprofile\Downloads\YouTube`（测试进程由 Parallels 以 SYSTEM 身份执行；用户态启动会落到用户 Downloads）。页面入口 `GET /_bifrost/ai?aiSection=tools-videos` 在带 gzip header 时返回 200 且包含前端 assets。`cargo test -p bifrost-admin videos --lib` 在 Windows 通过 15 个 Videos 单测，覆盖 Windows Open/Reveal 的 `explorer` 参数。Windows VM 当前缺少 `yt-dlp`、`ffmpeg`、`ffprobe`、`winget`、`msedge`，因此无法在 Windows 内完成真实 YouTube 下载；通过 API 提交 `https://www.youtube.com/watch?v=H_BCdTjO1zw` 到自定义目录 `C:\Users\eden_studio\Downloads\YouTube` 后任务按预期进入 `failed`，错误提示为 `Missing required command yt-dlp. Install it with winget or from the official release.`；调用 `/api/videos/downloads/{id}/retry` 返回 `202`，任务原地重置为 `queued` 后再次因缺少依赖失败，验证失败重试链路可用。
