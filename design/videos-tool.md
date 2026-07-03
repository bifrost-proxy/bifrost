# Videos Tool 设计方案

## 背景

Bifrost Admin Web UI 在 `AI -> TOOLS` 分组下提供 Videos 工具。首个支持的 provider 是 YouTube：用户粘贴一条 YouTube URL，可选修改下载目录，即可后台下载视频，实时看到进度、失败时可断点续传（`yt-dlp --continue`），完成后浏览器直接播放或调起系统默认播放器 / 在文件管理器里定位。

后端在 `crates/bifrost-admin/src/handlers/videos.rs` 用 `yt-dlp` 作为运行时依赖，把命令行 `--newline --progress` 输出解析为结构化 `progress_percent / total / speed / eta / file_path`，任务保存在进程内存 `DashMap<String, VideoDownloadTask>`（`VIDEO_DOWNLOADS`）。前端 `web/src/pages/AI/VideosTool.tsx` 每 1s 轮询一次直到所有任务终态。整个能力目标是让用户在 Bifrost 里一键抓取 YouTube 优质源，替代手工拼 `yt-dlp` 参数。

## 用户目标验证清单

### 必须实现

- `AI -> TOOLS -> Videos` 入口默认展示，与 `ASR` 同级列在 `TOOLS` 分组下。
- 默认目录取系统 Downloads 目录并追加 `YouTube`（例如 macOS 上 `/Users/<user>/Downloads/YouTube`）。
- 支持自定义目录：接受绝对路径，同时展开 `~` 与 `~/...`。
- `POST /api/videos/downloads` 校验 URL 只允许 http/https 且 host 为 `youtu.be`、`youtube.com` 或 `*.youtube.com`；相对路径直接拒绝。
- 后台执行 `yt-dlp`，参数固定包含 `--no-playlist --continue --newline --progress --retries 10 --fragment-retries 10 --concurrent-fragments 8 --merge-output-format mp4`。
- 目标质量选择器：`bv*[height>=4320]+ba/bv*[height>=2160]+ba/bv*+ba/b`，优先 8K，其次 4K，其次最佳视频+音频，兜底 best。
- 输出模版：`%(title).180B [%(id)s].%(ext)s`；`--print after_move:filepath` 用于收集最终文件路径。
- 失败任务支持 `POST /api/videos/downloads/{id}/retry`，复用同 URL + 同目录，`.part` 文件断点续传。
- 完成后的文件：
  - `GET /api/videos/downloads/{id}/file` 支持字节 Range 播放，`Accept-Ranges: bytes`。
  - `POST /.../open`（macOS `open`、Windows `explorer`、Linux `xdg-open`）。
  - `POST /.../reveal`（macOS `open -R`、Windows `explorer /select,`、Linux 打开父目录）。
- 未安装 `yt-dlp` 时任务立即失败，返回带平台安装提示的可读错误（Homebrew / winget / 系统包管理器）。

### 必须不破坏

- 现有 Admin API 路由前缀 `/api/videos` 仅 Videos 使用；不占用其他 handler 命名空间。
- 任务只保存在进程内存，服务重启即清空；避免引入新持久化格式。
- 不修改系统代理、不劫持网络、不需要 Bifrost 主端口。
- 不改动 ASR 等其他 `AI/TOOLS` 子页面路由。

### 必须真实验证

- Rust 单元测试覆盖 URL 白名单、目录解析、`yt-dlp` 进度解析、retry 状态机、文件路径推断、Range 响应。
- Web lint/build 覆盖 `VideosTool.tsx` 类型正确性。
- `human_tests/videos-tool.md` 覆盖真实浏览器流程（默认目录、自定义目录、下载完成、Play/Open/Reveal、非 YouTube 拒绝）。

## 产品语义

### 任务状态机

```
Queued → Running → Completed
                 ↘ Failed → (Retry) → Queued → ...
```

- `queued`：任务已创建，等待后台 spawn。
- `running`：`yt-dlp` 进程已启动，`progress_percent / speed / eta` 会随 stdout 更新。
- `completed`：`yt-dlp` 退出成功，`file_path` 指向合并后的最终文件（`.mp4/.webm/.mov/.mkv/.m4v`），`progress_percent = 100.0`，`downloaded/total` 显示合成文件大小的人类可读格式（`format_bytes`）。
- `failed`：`yt-dlp` 非零退出或前置校验失败；`error` 与 `message` 都携带 stderr 尾部（最多 `MAX_TAIL_LINES = 20` 行）。仅 `failed` 允许 retry；`running` 状态 retry 返回 409。

### URL 白名单

`validate_youtube_url` 只允许：

- scheme 是 http/https；
- host 是 `youtu.be`；或
- host 以 `.youtube.com` 结尾；或
- host 是 `youtube.com`。

其他站点返回 `Only YouTube URLs are supported`。这是产品层收敛，后续扩展新 provider 需另开专门的 validate 分支。

### 目录规则

`resolve_download_dir` 决策：

1. 空 / 缺省 → `default_download_dir()` = `dirs::download_dir() or ~/Downloads` + `YouTube`。
2. 非空 → `expand_home`（`~` 与 `~/rest`）→ 必须是绝对路径，否则报错 `Download directory must be an absolute path`。
3. `GET /api/videos/defaults` 返回默认目录字符串，前端展示 Restore 按钮。

### Range 播放语义

`video_file_response` 解析 `Range: bytes=start-end`：

- 无 Range header → 返回前 1 MiB (`DEFAULT_VIDEO_CHUNK_BYTES = 1_048_576`)，`206 Partial Content`。
- `bytes=-N` → suffix，`start = total_len - N`。
- `bytes=X-` → 一直读到 EOF。
- `bytes=X-Y` → 截取 `[X, min(Y, total_len-1)]`。
- 越界或格式错 → `416` + `Content-Range: bytes */total`。
- 空文件（`total_len == 0`）+ 无 Range → `200 OK` 空 body。
- Content-Type 按扩展名匹配 `video/mp4 / video/webm / video/quicktime / video/x-matroska`；未知扩展 `application/octet-stream`。
- `Content-Disposition: inline; filename="..."`，方便浏览器原生播放器直接吃。

## 技术细节

### 路由

`handle_videos(req, path)` 内部匹配：

| Method | Path                                       | 用途 |
| ------ | ------------------------------------------ | ---- |
| GET    | `/api/videos/defaults`                     | 返回默认目录 |
| GET    | `/api/videos/downloads`                    | 按 `created_at_ms` 倒序返回全部任务 |
| POST   | `/api/videos/downloads`                    | 创建任务，返回 `202 Accepted` + 任务体 |
| GET    | `/api/videos/downloads/{id}`               | 单任务详情 |
| GET    | `/api/videos/downloads/{id}/file`          | Range 流式播放 |
| POST   | `/api/videos/downloads/{id}/open`          | 调用系统默认播放器 |
| POST   | `/api/videos/downloads/{id}/reveal`        | 在文件管理器中定位 |
| POST   | `/api/videos/downloads/{id}/retry`         | 重跑失败任务 |

路径解析 `parse_download_action_path` 拒绝额外层级（`/api/videos/downloads/{id}/file/extra` → 404）。

### 数据结构

```rust
enum VideoDownloadStatus { Queued, Running, Completed, Failed }

struct VideoDownloadTask {
    id: String, url: String, download_dir: String,
    status: VideoDownloadStatus,
    progress_percent: Option<f32>,
    downloaded: Option<String>, total: Option<String>,
    speed: Option<String>, eta: Option<String>,
    file_path: Option<String>,
    message: Option<String>, error: Option<String>,
    created_at_ms: u64, updated_at_ms: u64,
}
```

`serde(rename_all = "snake_case")` → JSON 中字段 `queued/running/completed/failed`。全局 `VIDEO_DOWNLOADS: Lazy<DashMap<String, VideoDownloadTask>>` 用作进程内存存储。

### yt-dlp 进程

`run_download_task`：

1. 标记 `Running`；`create_dir_all(download_dir)`。
2. `ensure_command_available("yt-dlp")`：失败时以 `missing_command_message` 提示。
3. `Command::new("yt-dlp")` 携带全部参数、`stdout/stderr = Piped`。
4. 分别 spawn：
   - `read_ytdlp_stdout`：识别 `[download] pct% of total at speed ETA eta` 更新 progress；识别 `[Merger] Merging formats into "..."` 或以 `download_dir` 开头的行提取 final path。
   - `read_ytdlp_stderr`：把 stderr 尾部塞进 `stderr_tail`（deque，最长 `MAX_TAIL_LINES`），任务 message 也同步刷新。
5. 等待 child exit：
   - 成功 → `infer_completed_file_path`（按 `[<video_id>]` 匹配、跳过 `.part`）→ `format_bytes(metadata.len())` 填 downloaded/total → `status = Completed`。
   - 失败 → `fail_task` 追加 stderr tail。

### YouTube video id 提取

`youtube_video_id`：

- `youtu.be/<id>` 从第一段 path segment 取。
- `youtube.com/watch?v=<id>` 从 query 取。
- 空值直接返回 `None`（用于 `Live` / `Shorts` 等未来扩展）。

## CLI + Web + Admin API

### CLI

第一版**不提供 CLI 子命令**；下载通过 Admin API 或 WebUI 触发。CLI 后续版本可增加 `bifrost ai videos download <url>` 一键包装 POST。

### Web UI

`web/src/pages/AI/VideosTool.tsx`：

- 页面 section: `tools-videos`；`AI/index.tsx` 的 tools 分组下新增菜单项，与 ASR 并列。
- 组件：URL 输入 + 目录输入 + Restore default 按钮 + Download 按钮 + 任务表。
- 任务表列：URL / Target path / Progress bar / Status / Updated at / Actions。
- Actions：
  - `queued`/`running`：无操作按钮。
  - `failed`：Retry。
  - `completed`：Play（新 tab 打开 `/api/videos/downloads/{id}/file`）、Open、Reveal。
- 轮询：`setInterval(fetch("/api/videos/downloads"), 1000)`；当所有任务 `completed | failed` 时停止。

### Admin API 一览

见「路由」节，全部 handler 在 `handle_videos` 内部集中匹配，未挂 auth；由外层 admin 通用中间件（CORS/CSRF/access）保护。

## Sync 边界

任务只存进程内存，不落盘、不参与规则/Group Sync，也不写入 traffic/rules 数据目录。多设备协作、跨机分享的下载队列超出本方案范围。

## Phase 1-4

### Phase 1：后端骨架

- 新建 `handlers/videos.rs`，实现 `handle_videos` 路由分发。
- 数据结构、`VIDEO_DOWNLOADS` 全局 map、`now_ms`、`format_bytes`、`resolve_download_dir`、`default_download_dir`、`expand_home`、`validate_youtube_url`。
- 单元测试：URL 白名单、目录解析、`format_bytes`、`missing_command_message`。

### Phase 2：yt-dlp 执行 + 进度解析

- `run_download_task` / `read_ytdlp_stdout` / `read_ytdlp_stderr` / `parse_ytdlp_progress` / `extract_output_path` / `infer_completed_file_path` / `youtube_video_id`。
- 单元测试：`parses_ytdlp_progress`、`extracts_youtube_video_ids`、`infer_completed_file_path_skips_partials`。

### Phase 3：Range 播放 + Open/Reveal + Retry

- `video_file_response`、`parse_video_byte_range`、`read_file_range`、`video_content_type`、`content_disposition_inline`。
- `open_video_file` + `open_path_command`（macOS / Windows / Linux 三平台分支）。
- `retry_download` + `prepare_retry_download` 状态复位。
- 单元/异步测试：`video_file_response_serves_byte_ranges`、`video_file_response_defaults_to_initial_partial_chunk`、`completed_file_path_rejects_unfinished_tasks`、`open_path_command_targets_platform_file_manager`、`reveal_path_command_targets_platform_file_manager`、`prepare_retry_download_resets_failed_task_for_resume`、`prepare_retry_download_rejects_running_task`、`parse_download_action_path_handles_detail_and_actions`。

### Phase 4：前端页面 + 文档

- 新增 `web/src/pages/AI/VideosTool.tsx`；在 AI 侧栏 tools 分组注册 `tools-videos`。
- Playwright: `web/tests/ui/videos-tool.spec.ts` 覆盖 URL 校验错、默认目录、Retry 按钮出现在 failed 行。
- `human_tests/videos-tool.md`（TC-VT-01..06），同步 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/handlers/videos.rs::tests`）

- `parses_ytdlp_progress`
- `validates_youtube_only`
- `extracts_youtube_video_ids`
- `infer_completed_file_path_skips_partials`
- `expands_home_download_dir`
- `formats_completed_file_size`
- `missing_command_message_uses_platform_hint`
- `open_path_command_targets_platform_file_manager`
- `reveal_path_command_targets_platform_file_manager`
- `video_file_response_serves_byte_ranges`（tokio）
- `video_file_response_defaults_to_initial_partial_chunk`（tokio）
- `completed_file_path_rejects_unfinished_tasks`
- `prepare_retry_download_resets_failed_task_for_resume`
- `prepare_retry_download_rejects_running_task`
- `parse_download_action_path_handles_detail_and_actions`

运行：`cargo test -p bifrost-admin videos::`。

### Web 测试

- `pnpm --dir web lint` / `pnpm --dir web build` 保证 `VideosTool.tsx` 类型和渲染无警。
- `web/tests/ui/videos-tool.spec.ts`：Playwright 覆盖导航到 `AI/TOOLS/Videos`、非 YouTube URL 拒绝、失败行出现 Retry。

### 真实场景测试 human_tests

`human_tests/videos-tool.md`（现存）：

- **TC-VT-01** 默认下载目录（`GET /_bifrost/api/videos/defaults` 以 `YouTube` 结尾）。
- **TC-VT-02** 自定义目录（相对路径拒绝、`~/xxx` 展开成功）。
- **TC-VT-03** 提交并观察进度。
- **TC-VT-04** 完成后 Play/Open/Reveal 三种动作。
- **TC-VT-05** 失败后 Retry 复用 `.part`。
- **TC-VT-06** 非 YouTube URL 立即报错。

前置条件：临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，本地安装 `yt-dlp`。

## Review/Fix/Test 闭环

### 第 1 轮

- 用户目标核对：URL 白名单、默认目录、进度可见、Retry 断点续传、播放/打开/定位、`yt-dlp` 缺失的可读错误。
- Diff 复核：`handlers/videos.rs` + `web/src/pages/AI/VideosTool.tsx` + AI 侧栏注册 + `human_tests/videos-tool.md`。
- 单测 + Playwright + `cargo fmt --all -- --check` + `cargo clippy -p bifrost-admin -- -D warnings`。

### 第 2 轮

- 复核第 1 轮改动。重点：`parse_video_byte_range` 边界（越界、`bytes=-0`、超大 suffix）、`open_path_command` 三平台分支、`prepare_retry_download` 只允许 failed。
- 复跑 `videos::` 全部单测 + `pnpm --dir web build`。
- `bash e2e-tests/tests/test_videos_tool_e2e.sh` 若有；否则以 human_tests 收尾。

## 风险与决策

- **持久化缺失**：任务进程内存存储，服务重启丢失历史。后续版本需要引入 `videos.json` 或复用 `data/tasks.json` 时，注意迁移和 concurrency，且必须显式覆盖 `retry` 语义。
- **无取消能力**：目前没有 `POST /.../cancel`；已 spawn 的 `yt-dlp` 只能通过 kill 主进程结束。若引入 cancel，要额外持有 `Child` handle 或 process group，并处理 `.part` 清理。
- **format 选择器过于激进**：`bv*[height>=4320]+ba` 对多数 YouTube 视频无解，会回退到 `b`；这不是 bug，但用户可能感知不到实际选中的分辨率。后续可以在 completed task 里追加 `format_id` / `resolution` 字段。
- **平台差异**：`open_path_command` 在非 macOS/Windows/Linux 平台会走 `xdg-open`；未测试 BSD/Android。若增加新平台需扩展 `cfg` 分支。
- **依赖 `yt-dlp` 外部命令**：Bifrost 不打包 `yt-dlp`；缺失时给出 `Homebrew / winget / 包管理器` 提示，但仍是软失败。后续可以在启动时预探测并把状态暴露到 `/api/videos/defaults`。
- **无 auth 直连文件**：`GET /api/videos/downloads/{id}/file` 无独立 token，只靠外层 admin CORS/access 保护；如果部署到多用户场景，需要额外权限校验，避免 UUID 遍历。
