# Qwen3-ASR 本地 API Server

## 背景

Bifrost 需要在完全离线、无 CUDA、无 vLLM 的前提下让用户在自己的 Apple Silicon Mac 上跑通 Qwen3-ASR 系列语音识别模型，并把它接进 WebUI 的“语音转文字”入口、CLI 目录任务、Daily Agent 报告链路。历史上这条链路依赖仓库脚本 (`prepare/install/verify` shell) 与用户手动 `qwen3_asr_rs` 部署，路径易漂移、下载重复、启动/停止无法与 Bifrost daemon 一体化。

本方案把整条链路收敛为 Bifrost 内置的 Rust 能力（`crates/bifrost-admin/src/handlers/asr.rs`、`crates/bifrost-admin/src/asr_runtime.rs`、`crates/bifrost-admin/src/asr_jobs.rs`、`crates/bifrost-cli/src/commands/asr.rs`），固定安装到 `~/.bifrost/asr`，通过 Admin API `/api/asr/*` 与 CLI `bifrost ai asr` 统一编排，覆盖：

- 资源准备（模型/runtime/tokenizer/样例音频）
- 常驻本地 OpenAI-compatible API server 生命周期
- 文件流式、麦克风实时、目录定时任务、Daily Agent 报告
- 内存/进程 watchdog（physical footprint guard、memory-limit hint bisect）
- WebUI 独立子路由与单文件时间轴阅读器

真实实现涉及：`crates/bifrost-admin/src/handlers/asr.rs`（HTTP handler + preflight + ffmpeg autoinstall）、`crates/bifrost-admin/src/asr_runtime.rs`、`crates/bifrost-admin/src/asr_jobs.rs`（目录任务/schedule/pause/resume/startup recovery/runtime strategy）、`crates/bifrost-admin/src/resource_download.rs`（断点续传 + `direct_reqwest_client_builder()`）、`crates/bifrost-cli/src/commands/asr.rs`（`start/stop/status/stream-file/task/subtitle/diarization`）、`web/src/pages/ASR/*`。

## 用户目标验证清单

### 必须实现

- 默认模型 `Qwen3-ASR-0.6B`；`--model Qwen3-ASR-1.7B` 显式切换到 1.7B（推荐 32GB+ 内存）。
- 仅 macOS Apple Silicon (`macos-aarch64`) 启用本地 ASR；其它系统 Web/CLI/API 直接给出 unsupported 提示，不落任何模型资产。
- 固定安装目录 `~/.bifrost/asr/qwen3_asr_rs`；`--home` / `QWEN3_ASR_HOME` / API 参数都不能改变实际路径，避免 WebUI/CLI/测试之间重复下载。
- 下载走 Rust `resource_download` 模块（断点续传 + 进度 + 总量 + 速度 + ETA），HTTP client 使用 `bifrost_core::direct_reqwest_client_builder()` 绕过环境代理与自身运行中的 Bifrost 代理。
- `ffmpeg` 缺失时，macOS Apple Silicon 由 Rust 流程调用 Homebrew 自动安装；Homebrew 不可用或失败时错误信息必须包含 `brew install ffmpeg` 与重试建议。
- Admin API：`/api/asr/status`、`/api/asr/init-stream`、`/api/asr/service/start`、`/api/asr/service/stop`、`/api/asr/transcribe-stream`、`/api/asr/transcribe-ws`、`/api/asr/tasks`、`/api/asr/tasks/<id>/{run,pause,resume,cleanup-source-audio,daily,daily/<YYYY-MM-DD>,files/<file_key>/source}`。
- CLI：`bifrost ai asr {start,stop,status,stream-file,subtitle,diarization,task {create,list,show,files,run,watch,tui,daily list,daily show}}`。
- WebUI 独立子路由 `AI -> Tools -> ASR`，含 Speech Converter 状态、Speech to Text 工作卡、Directory Tasks 首屏入口、Daily Agent / Daily Agent Records 平级 tab。
- 长音频统一 30 秒最大 chunk，串行处理（切当前 chunk → ASR → 删除 → 下一个），只有浏览器麦克风实时链路为 1000ms/300ms overlap 明确例外。
- 目录任务 `runtime_strategy` 支持 `reuse_per_file`（默认）/`fork_per_chunk`/`reuse_server`/`auto`/`compare`。
- Physical-footprint watchdog：`Qwen3-ASR-0.6B` 默认 8192 MiB，`Qwen3-ASR-1.7B` 默认 18432 MiB，二级安全阀 = 宿主机内存 × 90%；`BIFROST_ASR_MAX_FOOTPRINT_MB` 只能向下收紧；`BIFROST_ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD=1` 才允许关闭。
- 目录任务 pause/resume：`mode=temporary`（下次 schedule 自动恢复）/`mode=long_term`（手动 Resume）/`force=true`（kill 正在跑的子进程与 ffmpeg）。
- Startup recovery：Bifrost 重启后清理 stale `run.lock`、回填 orphan `processing` 状态，enabled 未暂停任务重新入队，paused 任务不自动恢复。

### 必须不破坏

- Bifrost 主 daemon 启动路径不同步下载/加载模型；初始化只由 WebUI 点击或 CLI 手动触发。
- 目录任务、CLI stream-file 与托管 `asr-server` 各自的失败不允许把主代理 HTTP/HTTPS 服务拖到 CPU 100%（缺失 hash 的历史文件降级为普通路径，不在主线程 BLAKE3）。
- 转写完成产物（transcript/timeline/metadata、daily markdown）在源音频被删除后仍保留；`cleanup-source-audio` 只删 `success + transcript/timeline 已存在 + audio_dir 内` 文件。
- Runtime strategy 变更、fallback、chunk 失败必须持久化到 `files.json` + metadata JSON + `ASR chunk metric` 日志，可事后追溯。
- `bifrost ai asr status --json | grep -q` 这类下游提前关闭 stdout 时按普通 Unix 管道语义静默退出，不 `BrokenPipe` panic。

### 必须真实验证

- `cargo test -p bifrost-admin resource_download` 覆盖 Range 续传头、续传总量合并与进度百分比边界。
- `cargo test -p bifrost-admin asr` 覆盖 host/port 校验、Rust 初始化任务、流式切片、overlap、能量边界、尾段 flush、去重、WAV 解析、query 参数边界。
- `cargo test -p bifrost-admin asr_jobs --lib` 覆盖递归发现、pause/resume、daily 聚合、cleanup-source-audio、runtime strategy 默认与 chunk metric。
- `cargo test -p bifrost-admin startup_recovery --lib` 覆盖 stale `run.lock` 恢复。
- `cargo test -p bifrost-admin normalize_task_audio_dir_path --lib` / `load_tasks_normalizes_legacy_home_and_relative_audio_dirs --lib`。
- `cargo test -p bifrost-admin asr_cli_invoke --lib`（native `asr` 输出解析 + `vmmap` footprint 单位解析）。
- `cargo test -p bifrost-cli asr --lib`、`cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands`。
- `bash e2e-tests/tests/test_qwen3_asr_local_server.sh`（离线结构 + `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 在线段）。
- `bash e2e-tests/tests/test_asr_task_pause_resume.sh`、`bash e2e-tests/tests/test_asr_task_startup_recovery.sh`、`bash e2e-tests/tests/test_asr_task_cli.sh`。
- `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts`。
- 真人回归 `human_tests/qwen3-asr-local-server.md`。

## 产品语义

### 平台闸门与安装目录

- macOS Apple Silicon 之外的平台，任何入口（WebUI 初始化按钮、`/api/asr/status`、`bifrost ai asr start`）都返回 unsupported，不下载资产、不落 service state。
- 安装目录固定 `~/.bifrost/asr`；CLI/API 展示为固定路径，WebUI 不可修改；已存在 `~/ai/asr` 的历史内容已同步过来。

### 初始化按需触发 + 断点续传

- Bifrost daemon 启动不做任何模型下载/加载；`GET /api/asr/status` 是短超时健康探测 + 目录检查，不阻塞主服务。
- `GET /api/asr/init-stream` 是 SSE，用户在 WebUI 点击初始化才启动或订阅后台任务；页面刷新/断线会回放任务历史并继续接收进度，不打断后台下载。
- 下载失败、依赖缺失、解压失败、验证退出码、外部源不可达都作为 `error` 事件发到 WebUI；成功后隐藏初始化按钮和进度模块。

### 常驻服务生命周期

- `POST /api/asr/service/start` 先跑一次自检（platform/model/runtime/ffmpeg），缺失则触发断点续传或 Homebrew 补齐；ready 后从 `~/.bifrost/asr/qwen3_asr_rs/asr-server` 启动 Bifrost 托管进程，等待 `/health` ready 后返回。
- 动态选择空闲 loopback 端口，写入 `BIFROST_DATA_DIR/asr/service.json`（含 `pid/host/port/model/language/home/managed_by`）。
- `POST /api/asr/service/stop` 只停 Bifrost 托管进程；外部启动进程不由 Bifrost 停。
- 托管进程放入独立进程组，注册 Bifrost 外层 physical-footprint watchdog。

### 长音频统一 30 秒窗口

- 目录任务、CLI stream-file、WebUI 文件上传：默认 30 秒 chunk，串行处理，force-pause / 超限时可打断 ffmpeg 与 `asr` 子进程。
- WebUI 麦克风：唯一例外，默认 1000ms 窗口 + 300ms overlap；窗口下限 300ms，overlap ≤ 窗口一半，用于快速反馈。

### 目录任务 schedule 与调度

- 使用显式墙钟周期：`hourly`（分钟）/`daily`（HH:mm）/`weekly`（ISO 周一到周日 + HH:mm）/`monthly`（每月第几天 + HH:mm，短月钳制到月末）。
- 不再暴露秒级 interval。创建任务时当前分钟已到达会立即入队一次；执行后按下一周期推进，不重复运行。
- 任务运行时若托管服务已 ready 就复用并保留原状态；未运行则在任务独占锁内临时启动，运行完停止，避免长期占用。

### Pause / Resume 语义

- `pause?mode=temporary`：`paused=true`，保留或计算 `next_run_at_ms`；下一次 schedule 到点自动清 pause 恢复。
- `pause?mode=long_term`：`paused=true`，清空 `next_run_at_ms`，必须手动 Resume。
- `pause?force=true`：额外登记 force-pause；主动 kill 当前 native `asr` 子进程和 `ffmpeg` normalize/split 子进程；默认走长期暂停。
- `resume`：清 pause 与 force-pause 后立即派发后台 run 去扫描 pending/failed；主服务不阻塞。
- 暂停期间 Run 按钮 disabled、API Run 返回 409。

### Startup Recovery

- 每次 scheduler 启动前修复文件级 `processing` 遗留：无进程内运行任务且 `run.lock` 不属于仍存活 Bifrost 时，删 stale lock、把孤儿 `processing` 恢复为 `pending`（清旧进度/旧 transient error）。
- 对 `enabled=true, paused=false` 且仍有 `pending/failed` 的中断任务，立即重新入队，不等下次 schedule；`paused=true` 任务只清 lock 不自动恢复。
- 运行中标记用 RAII guard 持有，panic/提前 return/lock 拒绝都释放 `RUNNING_TASKS`，避免 UI 长期 fake Running。

### Runtime Strategy 对照

| 策略 | 语义 | 使用场景 |
| --- | --- | --- |
| `reuse_per_file`（默认） | 每文件启动/复用一个 `asr-server`，文件内 chunk 复用，文件结束停 | 2026-05-18 实测比 `fork_per_chunk` 快约 11.1% |
| `fork_per_chunk` | 每个 30 秒 chunk fork native `asr` CLI 子进程 | 强隔离，配合 physical-footprint guard |
| `reuse_server` | 一次 run 启动/复用一个托管 `asr-server`，全部文件/chunk 共用 | 高吞吐 |
| `auto` | 先 `reuse_server`；启动失败或 RTF 相对稳定样本 >1.5× 时 fallback `fork_per_chunk`；单 chunk 失败只降当前 chunk | 生产稳定 + 高吞吐折中 |
| `compare` | 同 chunk 同时 fork canonical + reuse shadow；最终采用 fork 文本，但记录两侧耗时/RTF/hash/error | 定位复用路径是否退化 |

WebUI Runtime 下拉展开时每个策略下方展示简短说明；选中后输入框只显示短标题。

### Physical-footprint Watchdog

- 1.7B 在特定 30 秒 chunk 上可能出现 `ps` RSS 仅 3-4 GiB 但 `vmmap` physical footprint 涨到 20 GiB+ 的 Metal/MLX 异常。
- 优先按模型规模设 footprint 上限：0.6B → 8192 MiB、1.7B → 18432 MiB、未知模型 → 12288 MiB；二级安全阀 = 宿主机内存 × 90%。
- `BIFROST_ASR_MAX_FOOTPRINT_MB` 只能向下收紧。关闭 watchdog 必须显式 `BIFROST_ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD=1`。
- second-state `qwen3_asr_rs` v0.2.0 内部只 `init_mlx(true)` + `mlx_set_default_device`，未暴露 `set_memory_limit/cache_limit/wired_limit`，因此靠外层 watchdog + chunk cap + 失败记忆三层防护。
- 采样：force-pause/退出仍每 500ms 轮询；`vmmap -summary` physical footprint 首次采样延后一个周期，之后默认 5s；`BIFROST_ASR_PHYSICAL_SAMPLE_INTERVAL_SECS` 可在 2-60s 调整。
- 触发时 kill 当前进程组并交给 bisect/pause/status fallback；采样报错时只 warn，不用 RSS-only fallback 过早杀托管服务。

## 技术细节

### 后台 API

- `GET /api/asr/status`：短超时 `/health` 探测 + 目录检查；结构化返回 `installed/ready/model/home/port/managed_by/error`。
- `GET /api/asr/init-stream`：SSE。事件 `preflight/download/extract/install/tokenizer/verify/error/done/installed`。
- `POST /api/asr/service/start` / `POST /api/asr/service/stop`：见上文；state 落 `BIFROST_DATA_DIR/asr/service.json`。
- `POST /api/asr/transcribe-stream`：multipart 上传 → ffmpeg → 30 秒/2 秒 overlap 切片 → 顺序调 `/v1/audio/transcriptions` → SSE 输出 `progress/final/text/error/done`；模型返回 timestamp segment 时 `final` 把 chunk 内时间平移到整段时间线。
- `POST /api/asr/transcribe-ws`：真实 WebSocket。控制帧 `start`/`finish` + 二进制 WebM chunk；顶层 `type` 直接是 `connected/stream/partial/final/text/done/error`，事件 detail 含递增 `processed_ms`；后端保留同一 MediaRecorder 会话完整 WebM 字节流用于 ffmpeg 解复用，每次 flush 转 16kHz mono WAV，只切“上次确认时间点 - overlap”到当前可解码时长送模。
- `POST /api/asr/tasks`：CRUD 目录任务；state 落 `BIFROST_DATA_DIR/asr/tasks.json` 与 `asr/tasks/<task_id>/files.json`。
- `POST /api/asr/tasks/<id>/run`：手动运行；paused 时 409。
- `POST /api/asr/tasks/<id>/pause` / `POST /api/asr/tasks/<id>/resume`：见 pause/resume 语义。
- `POST /api/asr/tasks/<id>/cleanup-source-audio?confirm_name=<task_name>`：任务名精确匹配后
  只删可清理集合；缺失/错误确认返回 400，运行中或 failed-chunk 批量重试中返回 409。
- `GET /api/asr/tasks/<id>/daily` / `/daily/<YYYY-MM-DD>`：读前从 timeline 刷新 `BIFROST_DATA_DIR/asr/data/text/<task_id>/daily/<date>.md`；日期只接受 `YYYY-MM-DD`，防路径穿越。
- `GET /api/asr/tasks/<id>/files/<file_key>/source`：源音频回放；用于 WebUI 单文件详情。

### CLI

- `bifrost ai asr start` / `stop` / `status`：CLI/WebUI 共享同一 service state；`status --json` 遇到管道提前关闭时按 Unix 语义静默退出，其它 IO 错误仍返回。
- `bifrost ai asr stream-file <audio>`：临时启动或复用本地 `asr-server`，30 秒/2 秒 overlap 顺序发 chunk 输出 JSON Lines；临时启动的进程结束后恢复停止状态。
- `bifrost ai asr task {create,list,show,files,run,watch,tui,daily {list,show}}`：通过 Admin API 编排；不显式传 `-p` 时沿用 runtime port 解析，读不到 runtime 回退 9900。`daily show` 默认输出 Markdown 到 stdout，`--output` 写文件；`run --wait` 触发并等后台运行结束（即使无 pending 也会 daily 刷新）。
- `bifrost ai asr subtitle`：离线字幕产物（srt/vtt/txt/timeline_json/metadata）。
- `bifrost ai asr diarization`：说话人分离 profile 初始化/管理；`stream-file` 与 `subtitle` 均支持 `--speaker-aware`。

### WebUI

- 独立子路由 `AI -> Tools -> ASR`；URL 状态承载：`asrTask=<task_id>&asrTaskTab=<tab>&asrDay=<YYYY-MM-DD>&asrFile=<file_key>&asrDailyReport=<date>`。
- 首屏顺序：`Speech Converter` 状态面板 → `Directory Tasks` → `Speech to Text` 工作卡（Audio Input 顶部 + Transcript/错误/stream events 下方）。
- 任务详情页顶部 tab：`Overview` / `Files`（Recorded 时间倒序） / `Daily Docs` / `Daily Agent`（只承载配置/执行/AGENTS.md 编辑）/ `Daily Agent Records`（承载运行记录/结果表/report 链接/Refresh）。
- 单文件详情：源音频播放器 + timeline segment 阅读器；点击时间点跳转、播放/拖动自动高亮 + 滚动到当前 segment；用户手动滚动时自动跟随暂停 5s，超时恢复；暂停期间操作播放轴/点时间点立即恢复跟随。
- Directory Tasks 状态：`Ready / Running / Pausing / Paused / Paused until schedule`；Pause 按钮提供 `Pause until next schedule` 与 `Pause indefinitely`；paused/running 时 Run disabled。
- `audio_dir` 输入允许绝对路径与 `~/xxx`；后端在创建/编辑/加载旧 `tasks.json` 时统一展开 `~` 到 home 目录，普通相对路径按 home 目录解析为绝对路径；WebUI 保存后回显绝对路径。
- 长文档不允许嵌套纵向滚动，走 ASR 页最外层滚动条；文件表长路径列走表格内部横向滚动，不撑出内容区。

### 麦克风实时链路（1000ms 窗口的例外）

- 默认 1000ms 窗口 + 300ms overlap；`window_ms`/`overlap_ms` 可传，窗口下限 300ms，overlap ≤ 窗口一半。
- 后台只保留当前上传、规范化 WAV、当前窗口临时文件；每次模型调用后删除窗口文件；上传体 512MB 上限。
- 规范化 WAV 解析 16kHz mono PCM 后计算窗口；非尾段窗口在目标 1s 边界前 250ms / 后 125ms 内寻找 50ms frame 最低能量点作为稳定边界，找不到则退回固定 1s 边界。
- 每窗口实际送模 `stable_start - overlap` 到 `stable_end`；`stable_start..stable_end` 是本窗口可确认范围。
- 先发 `partial`（含候选文本和相对已确认的 `delta`），边界确认后发 `final`；尾段不足 1s 但超过最小窗口则 EOF flush 为 final；短于 300ms 或空音频返回确定的空文本 done。
- 文本连续性用最长 suffix/prefix overlap 去重；重复返回时 delta 空；中文字符不额外插空格，英文片段之间补空格。
- 单窗口模型 5xx / 网络错误重试一次；仍失败发带 window index 的 `error` 事件继续后续窗口；整次都没有稳定文本且出错则最终返回错误。
- 前端从同一 `MediaStream` 创建 Web Audio `AnalyserNode`，约 30fps 采样频域能量渲染 40 电平条；Stop/Cancel/WebSocket 错误/组件卸载时取消 `requestAnimationFrame`、关闭 `AudioContext`、电平归零。

### 目录任务 chunk 处理与内存记忆

- 每处理完一批 pending 后重新递归扫描 `audio_dir`、同步新增 source record、应用外部导入 hash 与内容去重，未在本次 run 尝试过的 `pending/processing/failed` 加入下一批。
- 队列按 `source_created_at_ms` 从早到晚，缺时间用 `source_modified_ms` + 路径排序；避免同 run 中失败文件被无限重试。
- 长音频不预先并发切全部 chunk；按 `切当前 chunk → ASR → 删除 → 下一个` 流式处理，避免几小时录音一次拉起大量 `ffmpeg` 或堆积临时 WAV。
- 每个文件的 `memory_limit_hints` 持久化 bisect 结果；同文件同 chunk offset/duration 再处理时直接用较小 window，不再撞完整 30 秒高风险路径。
- 录音创建时间优先级：`ffprobe` 容器 tags `date + creation_time` 或 RFC3339 `creation_time` → 文件名 `YYYYMMDD_HHMMSS` → filesystem birthtime → filesystem mtime；0 字节坏文件保留可解析文件名时间并作为单文件 failed record，不中断整个任务。

### 资源下载模块

`bifrost-admin::resource_download` 通用：上层只提交 URL + 目标文件 + 展示标签；模块负责后台下载、断点续传（Range 头 + 已下总量合并）、进度、总量、速度、ETA、失败错误分类。所有 HTTP client 通过 `bifrost_core::direct_reqwest_client_builder()` 构造，绕过 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY` 与 Bifrost 自身代理。

## CLI+Web+Admin API

### Admin API

`/api/asr/status`、`/api/asr/init-stream`、`/api/asr/service/start`、`/api/asr/service/stop`、`/api/asr/transcribe-stream`、`/api/asr/transcribe-ws`、`/api/asr/tasks`、`/api/asr/tasks/<id>`、`/api/asr/tasks/<id>/{run,pause,resume,cleanup-source-audio,daily,daily/<YYYY-MM-DD>,files/<file_key>/source}`。

### CLI

见“技术细节 - CLI”。子命令主入口位于 `crates/bifrost-cli/src/cli.rs` 的 `AiAsrCommands`；task 子树覆盖 `Task::{Create,List,Show,Files,Run,Watch,Tui,Daily{List,Show}}`。

### WebUI

见“技术细节 - WebUI”。核心页面位于 `web/src/pages/ASR/*`；ASR 页面上下滚动使用最外层容器，禁止内部嵌套纵向滚动。

## Sync 边界

- ASR service state、任务 state、文件 record、text/timeline/metadata、daily markdown 全部本机文件，不参与云端 sync。
- 权重与 runtime 从 GitHub release 与 Hugging Face 下载，属于外部依赖，非用户配置。
- `~/.bifrost/asr` 目录不 sync；`BIFROST_DATA_DIR/asr/*` 只在本机可见。

## Phase 1 —— 平台闸门 + 资源下载 + 初始化 SSE（shipped）

- macOS Apple Silicon 闸门；其它平台任何入口 unsupported。
- `resource_download` 断点续传；`direct_reqwest_client_builder()` 绕代理。
- `ffmpeg` 缺失时 macOS Apple Silicon 由 Rust 调 Homebrew 自动安装。
- `GET /api/asr/init-stream` SSE + 刷新可回放。
- 单测：`cargo test -p bifrost-admin resource_download`、`cargo test -p bifrost-admin asr`。

## Phase 2 —— 常驻服务 + 30 秒切片 + WebUI 麦克风（shipped）

- `POST /api/asr/service/start|stop`；动态端口 + `service.json`。
- `POST /api/asr/transcribe-stream` SSE 30 秒/2 秒 overlap。
- `POST /api/asr/transcribe-ws` 真实 WebSocket + 顶层事件 + 递增 `processed_ms`；MediaRecorder 多 chunk 复用 ffmpeg 解复用。
- Speech to Text 工作卡；音频输入 + Transcript + stream events 合并同卡。
- 单测：`cargo test -p bifrost-admin asr`；E2E `test_qwen3_asr_local_server.sh`。

## Phase 3 —— 目录任务 + Runtime Strategy + Physical Footprint Watchdog（shipped）

- `POST /api/asr/tasks` CRUD；`schedule=hourly/daily/weekly/monthly`；`runtime_strategy` 5 档。
- `run.lock` + 全局 job lock；chunk metrics 持久化到 `files.json` + metadata + `ASR chunk metric` 日志。
- Physical-footprint watchdog + memory-limit hints bisect + 每文件 `memory_limit_hints`。
- Startup recovery：stale lock 清理 + 中断计划恢复。
- Pause/Resume：temporary/long_term/force；resume 快速返回，后台 run 扫描 pending/failed。
- `cleanup-source-audio` 安全清理。
- 单测：`cargo test -p bifrost-admin asr_jobs --lib`、`startup_recovery --lib`、`normalize_task_audio_dir_path --lib`、`load_tasks_normalizes_legacy_home_and_relative_audio_dirs --lib`、`asr_cli_invoke --lib`。
- E2E：`test_asr_task_pause_resume.sh`、`test_asr_task_startup_recovery.sh`、`test_asr_task_cli.sh`。

## Phase 4 —— WebUI 子路由 + Daily Agent + Daily Docs + 单文件时间轴阅读器（shipped）

- 独立子路由 + URL 状态承载；任务详情顶部 tab 拆 Overview/Files/Daily Docs/Daily Agent/Daily Agent Records。
- Daily Agent 与 Daily Agent Records 平级 tab；打开 `asrDailyReport=<date>` 时 `asrTaskTab` 默认回到 `daily-agent-records`。
- 单文件详情：源音频播放器 + timeline segment；点击 + 播放/拖动 + 手动滚动 5s 暂停后自动恢复；用户操作播放轴时立即恢复跟随。
- `bifrost ai asr task watch/tui`：终端实时任务面板。
- 前端 E2E：`pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts`。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin resource_download`
- `cargo test -p bifrost-admin asr`
- `cargo test -p bifrost-admin asr_jobs --lib`
- `cargo test -p bifrost-admin startup_recovery --lib`
- `cargo test -p bifrost-admin normalize_task_audio_dir_path --lib`
- `cargo test -p bifrost-admin load_tasks_normalizes_legacy_home_and_relative_audio_dirs --lib`
- `cargo test -p bifrost-admin asr_cli_invoke --lib`
- `cargo test -p bifrost-cli asr --lib`
- `cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands`
- `bash -n e2e-tests/tests/test_qwen3_asr_local_server.sh`
- WebUI 构建/类型检查覆盖 ASR 子路由与音频输入区。

### E2E

- `bash e2e-tests/tests/test_qwen3_asr_local_server.sh`（默认离线结构；`BIFROST_QWEN3_ASR_E2E_ONLINE=1` 触发在线段：安装默认 0.6B / 显式 1.7B、中文样例 CLI 转写、`/health`、`/v1/models`、`/v1/audio/transcriptions` 验证含 `Qwen3`）。
- `bash e2e-tests/tests/test_asr_task_pause_resume.sh`：pause 后 `next_run_at_ms=null`、paused Run 返回 409、resume 快速派发后台 run、空目录快速结束不启动模型资产检查。
- `bash e2e-tests/tests/test_asr_task_startup_recovery.sh`：预置 stale `run.lock` + orphan `processing` 后启动最新 bifrost，断言 lock 删除、文件 `pending`、paused 任务不 fake Running。
- `bash e2e-tests/tests/test_asr_task_cli.sh`：临时 `BIFROST_DATA_DIR` + 空目录，验证 `task list/show/files/daily list/daily show --output/run --wait` 通过 Admin API 返回预期；同脚本用临时 `HOME` 验证 `audio_dir="~/bifrost-asr-home-audio"` 与 `PATCH audio_dir="relative-audio"` 都规范化为 home 下绝对路径，不落 `BIFROST_DATA_DIR`。
- `/api/asr/transcribe-stream` 上传中文样例，SSE 含 `final` + 中文文本，长音频每窗口 ≤ 30s。
- `/api/asr/transcribe-ws` 真实 WebSocket：`start` + 多个二进制 WebM frame + `finish`，顶层事件 `connected/stream/partial/final/text/done`，detail 含递增 `processed_ms`，不因后续 WebM chunk 独立解析失败。
- `/api/asr/tasks/<id>` summary 含 `audio_source_bytes` / `cleanable_source_bytes`；调 `cleanup-source-audio` 后 success 源被删、text/timeline 存在、partial-success 保留、二次调用删除数 0。
- `/api/asr/tasks/<id>/daily` 列文档、`/daily/<YYYY-MM-DD>` 返回完整内容、非法日期可读错误。
- 目录任务创建默认 `runtime_strategy=reuse_per_file`；对 `auto/reuse_server/fork_per_chunk/compare` 的真实模型对照实验通过 `files.json` + metadata + `ASR chunk metric` 日志验证 runner/RTF/text hash/fallback reason。
- WebUI Runtime 下拉展开每个策略有面向用户说明，默认仍 `reuse_per_file`。
- CLI 回归：`bifrost ai asr --help`、`stream-file /missing.wav` 错误路径、`status --json | grep -q '"ready"'` 管道提前退出。
- `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts`。

### 真人回归

`human_tests/qwen3-asr-local-server.md` 覆盖：

- Apple Silicon + 32GB 内存检查；依赖安装；非交互安装（默认 0.6B；显式 1.7B）；CLI 中文样例转写；API server `/health` 与中文转写；stream-file 长音频文件产物。
- AI -> Tools -> ASR 初始化状态、下载进度、外部依赖提示、错误详情；刷新页面后重新订阅初始化流仍在继续；Start/Stop 生命周期；临时移动固定目录下某模型文件触发下载展示后恢复。
- 文件拖入/选择流式输出；麦克风 WebSocket 1s timeslice + WebM → WAV + 权限错误路径；麦克风实时电平条：未录音归零 / 录音波动 / 停止后归零，亮暗主题可读。
- `bifrost ai asr`：启动/状态/单文件流式/停止。
- Directory Tasks 首屏顺序（Speech Converter 下方、Speech to Text 上方）；创建绑定目录任务，`daily/weekly/monthly` 周期不退化为秒级 interval；查看总体进度；点击进入 `asrTask=<task_id>` 子页面（无 Drawer/dialog）；Daily Docs tab 按日期列文档 → `asrDay=<YYYY-MM-DD>` 完整 Markdown；单文件 `asrFile=<file_key>` 详情：源音频播放器 + timeline 文本、segment ≤ 30s、点击时间点跳转、播放/拖动自动高亮 + 滚动、手动滚动 5s 暂停后恢复、暂停期间操作播放轴或点击时间点立即恢复；手动 Run，验证文本保存在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/`；删除源音频后文本/metadata 保留在统计中；模拟服务重启后旧 `run.lock`，任务不永久报 `ASR task is already running or lock is stale`。
- Cleanup originals：任务详情页展示当前音频占用与可清理占用；点击 Clean originals 有确认文案说明 transcript/timeline 保留且 partial-success 不删；确认后已成功转写原音频从 `audio_dir` 删除、页面刷新占用下降、`deleted_after_processing` 增加、单文件 transcript/daily docs 仍可打开；重复点击不报错删除数 0。
- Daily Agent tab 拆分：`Daily Agent` 只展示配置/执行/AGENTS.md 编辑；`Daily Agent Records` 只展示运行记录/结果表/report 链接/Refresh；点击 report 链接进 `asrDailyReport=<date>`，返回时仍停留 `daily-agent-records`。
- Directory Tasks 资源让路：Running 点击 `Pause until next schedule` → Pausing/Paused until schedule；后台在文件/chunk 边界释放；下一次 schedule 到点自动恢复；`Pause indefinitely` → 长期 Paused 不会自动恢复；Run 按钮不可用 & API Run 409；Resume 请求快速返回并切 Running，后台扫描 pending/failed，主服务轻量 API 不阻塞；无 pending/failed 时后台 run 快速回 Ready，已 success 不重跑。
- Directory Tasks 内存保护：用 `~/Downloads/we` 中可复现的 1.7B 问题录音，30 秒 chunk 保持默认；native `asr` 子进程 physical footprint 超阈值时日志出现 footprint limit/bisect 提示，任务继续拆小子段而不是让系统进入 20 GiB+ 卡死状态。

更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：1.7B、Apple Silicon、MLX/Metal、本地 API server、中文样例、长音频 30 秒切片、AI -> Tools -> ASR 初始化状态/进度/错误、Start/Stop、WebUI 文件上传 30 秒送模、浏览器麦克风保留 1s 实时响应、麦克风实时电平音轨、overlap/能量边界/去重/尾段 flush。
- 检查脚本不会修改 `~/.zshrc` 或系统代理。
- 检查 Bifrost 启动路径没有同步下载/加载模型；初始化只由 WebUI 点击触发；常驻服务只由 Start Service 启动。
- 执行 `git status --short`、`git diff`。
- 运行脚本语法、帮助、preflight、真实 `verify`、后台 ASR API 最小测试、前端构建。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff。
- 检查 `design/`、`human_tests/`、E2E 脚本、AI Tools 面板文案、实际部署命令是否一致。
- 复跑受影响测试；确认 `/health`、`/v1/audio/transcriptions`、`/api/asr/*` 都来自常驻 server 或清晰错误事件。
- 复查 `web/src/pages/ASR/index.tsx` 已按功能拆分且单文件 <1500 行、无残留 Drawer/dialog 依赖；亮/暗主题下任务详情与单文件详情继续使用 Ant Design token + 现有 CSS 变量。
- 真实 MediaRecorder 多 chunk 录制 5-8s 期间 Stop Mic 前必须出现 connected/stream + partial/final，事件 detail 含递增 `processed_ms`；不得出现“后续 WebM chunk 不能被单独 ffmpeg -i 解析”。
- 如有缺口追加第 3 轮。

## 风险与决策

- **外部下载源不可达**：GitHub release 与 Hugging Face 任一不可达时初始化失败；错误必须包含具体源、状态码与建议；不做静默重试掩盖。
- **1.7B Metal/MLX 内存异常路径**：second-state `qwen3_asr_rs` v0.2.0 不暴露 MLX memory/cache/wired limit，只能靠外层 watchdog + 30 秒 chunk cap + `memory_limit_hints` 三层防护；`BIFROST_ASR_MAX_FOOTPRINT_MB` 只允许向下收紧，避免大内存机器一律放宽到 20GB+。
- **`bifrost ai asr start` daemon 化**：当前拉起长驻 daemon 后 CLI 退出，无法由同一 CLI watchdog；后续应收敛到 Bifrost daemon/supervisor 托管。
- **BLAKE3 阻塞主服务**：Resume/启动恢复/自动刷新不能在主流程递归扫描 heavy summary 或对历史大音频同步算 hash；导入复制期间的 BLAKE3 只在后台阻塞 worker 全局内容哈希队列串行执行。
- **CI 稳定性**：CI 无条件跳过在线模型段，即使误设 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 也不下权重、不装 runtime、不启动 `asr-server`；`install/prepare/run-sample/start-server/verify` 独立 CLI 动词与 `BIFROST_QWEN3_ASR_ALLOW_CI_MODEL` 环境变量 planned，not yet shipped。
- **文件级 `processing` 遗留**：所有恢复路径都必须走同一处 startup recovery，避免旧格式 lock 或 pid 已换代的 lock 让任务永久 fake Running。

## 校验要求

- 必须执行 `BIFROST_QWEN3_ASR_E2E_ONLINE=1 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`，除非模型下载或运行环境阻塞。
- 必须验证 `/api/asr/transcribe-ws` 真实 WebSocket 链路：握手、二进制音频 chunk、`finish` final flush、顶层 `connected/stream/partial/final/text/done`、错误可见性。
- 必须证明初始化是异步按需触发：Bifrost server 启动后不下载模型、不加载模型；只有 AI -> Tools -> ASR 初始化请求才启动下载/验证。
- 必须证明模型目录固定：`--home` 与 API query 都不改变 status 返回的 install/model dir。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`；WebUI 构建或类型检查。

## 文档更新要求

- 更新 `human_tests/qwen3-asr-local-server.md` 与 `human_tests/readme.md` 索引，覆盖 Directory Tasks 首屏顺序、pause/resume 三档、Daily Agent tab 拆分、单文件时间轴阅读器、Runtime Strategy 五档、physical-footprint watchdog 提示。
- WebUI AI -> Tools -> ASR 入口若新增用户可见文案变化，同步 `docs/` 或页面内说明。
- 不改变 Bifrost CLI 主命令、规则协议或代理转发语义；不新增 sync 字段；不改变 admin 通用 API 契约。
