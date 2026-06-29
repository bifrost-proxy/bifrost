# Qwen3-ASR 本地 API Server

## 功能模块说明

本模块把 Qwen3-ASR 系列模型（默认 `Qwen3-ASR-0.6B`，可选 `Qwen3-ASR-1.7B`，详见 `crates/bifrost-admin/src/handlers/asr.rs` 的 model 枚举）在 Apple Silicon Mac 上的资源准备、启动与验证固化为 Bifrost 内置 Rust 能力，并在 WebUI 中提供语音转文字入口。1.7B 推荐 32GB 以上内存。目标路线是：

```text
Qwen3-ASR-0.6B/1.7B + qwen3_asr_rs + MLX/Metal + 本地 OpenAI-compatible API Server
```

该链路只负责本地语音模型运行验证与本地 WebUI 语音转写，不修改 Bifrost 代理运行时，不启动系统代理，不依赖 CUDA/vLLM。

## 实现逻辑

- ASR 初始化由 Bifrost Admin Rust 模块直接编排，不再依赖仓库脚本。
- 新增通用资源下载能力 `bifrost-admin::resource_download`，上层只提交 URL、目标文件和展示标签；下载模块负责后台下载、断点续传、进度、总量、速度和预计剩余时间。
- `preflight` 检查：
  - 仅 macOS Apple Silicon (`macos-aarch64`) 启用本地 ASR；其它系统的 Web/API/CLI 入口直接提示不支持当前操作系统；
  - macOS Apple Silicon 使用上游 `asr-macos-aarch64` release；
  - 内存小于 16GB 时失败，16GB 到 31GB 给出风险提示，32GB 可直接运行 1.7B；
  - `ffmpeg` 用于音频预处理；初始化自检或启动服务自检发现缺失时，在 macOS Apple Silicon 上由 Rust 流程调用 Homebrew 自动安装；如果 Homebrew 不可用或安装失败，错误信息必须包含 `brew install ffmpeg` 和重试说明。
- `install` 非交互安装：
  - 固定安装到 `~/.bifrost/asr/qwen3_asr_rs`；
  - `--home` 和 `QWEN3_ASR_HOME` 不改变安装目录，避免测试或 WebUI 使用不同目录导致重复下载；
  - 当前 CLI 与 API 的默认模型为 `Qwen3-ASR-0.6B`；安装/启动时可通过 `--model Qwen3-ASR-1.7B` 切换到 1.7B（仍由 `crates/bifrost-admin/src/handlers/asr.rs` 中的 model 表枚举）；
  - 通过 Rust 通用下载模块下载 qwen3_asr_rs 最新 release 二进制；
  - 通过 Rust 通用下载模块下载 Hugging Face 权重文件；下载 client 必须使用 `bifrost_core::direct_reqwest_client_builder()` 绕过 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 环境代理，避免用户当前 shell 或正在运行的 Bifrost 代理反向影响 `Qwen3-ASR-0.6B` / `Qwen3-ASR-1.7B` 初始化；
  - 从 release 内置 tokenizer 复制对应 `tokenizer.json`；
  - 下载上游 sample 音频，其中 `sample3.wav` 是中文验证样例。
- Rust 初始化流程直接调用下载后的 `asr` 二进制做中文样例转写验证。
- WebUI 托管服务不使用默认端口，点击 Start Service 时由 Bifrost 后台动态选择空闲 loopback 端口并直接启动 `asr-server` 二进制；启动模型不需要脚本。
- `health` 验证 `/health` 返回 `{"status":"ok"}`。
- `transcribe` 通过 `/v1/audio/transcriptions` 上传音频并支持 `text/json/verbose_json` 输出。
- `stream-file` 默认临时启动或复用本地 `asr-server`，按 30 秒窗口、2 秒 overlap 对同一个文件顺序复用模型服务并输出 JSON Lines；命令临时启动的服务会在文件结束后停止，这是 CLI 侧长音频真实验收入口。
- `chunk`、`batch-transcribe`、目录任务和 WebUI 文件上传链路的模型推理窗口统一为 30 秒；浏览器麦克风 WebSocket 采集是实时交互例外，保留 1 秒 MediaRecorder timeslice 和快速 flush。
- `verify` 串联 preflight、install、CLI 中文样例、API server、health、models、multipart transcription，用真实模型完成端到端验收。
- CI 环境默认禁止下载、安装或启动 Qwen3-ASR 模型运行时。当前实现以 `e2e-tests/tests/test_qwen3_asr_local_server.sh` 为入口：脚本检测到 `CI=true` 或未显式设置 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 时直接跳过在线模型段。`install`、`prepare`、`run-sample`、`start-server`、`verify` 在原始设计中是计划暴露的 CLI 动词，但当前仓库实际把它们实现为 `crates/bifrost-admin/src/handlers/asr.rs` 的内部 Rust 函数（`install_release` / `prepare_model` / `verify_cli_sample` 等），由 `/api/asr/init-stream`、`/api/asr/service/start` 等接口编排；独立 CLI 动词以及 `BIFROST_QWEN3_ASR_ALLOW_CI_MODEL` 环境变量目前未实现（planned, not yet shipped as of 2026-06-16）。仓库 E2E 即使误设 `BIFROST_QWEN3_ASR_E2E_ONLINE=1`，在 CI 中也只执行结构/错误路径验证，然后跳过在线模型段。
- Bifrost Admin 新增 `/api/asr` 后台 API：
  - `GET /api/asr/status` 只做短超时健康探测和本地安装目录检查，不触发下载，也不阻塞 Bifrost 启动。
  - `GET /api/asr/init-stream` 在用户从 AI -> Tools -> ASR 点击初始化后启动或订阅后台初始化任务，返回 `text/event-stream`。页面刷新或连接中断后再次进入会回放任务历史并继续接收当前进度，不会中断后台下载。
  - 初始化流程只准备和验证本地资产：Rust 模块完成预检、断点续传下载、release zip 解压、runtime 安装、tokenizer 复制和 CLI 中文样例验证，不启动常驻模型服务。
  - 后台 API 和 CLI 都固定使用 `~/.bifrost/asr`，不接受 WebUI 或 API 指定其它模型目录。
  - 初始化失败时把依赖缺失、模型下载失败、解压/安装失败、验证退出码、外部源不可达等信息作为 `error` 事件返回给 WebUI。
  - `POST /api/asr/service/start` 启动前执行同一套自检：平台不支持时直接返回 unsupported；模型、runtime 或样例文件缺失时自动复用 Rust 断点续传下载与安装流程补齐；模型下载同样绕过环境代理；`ffmpeg` 缺失时自动尝试 Homebrew 安装。自检通过后再从 `~/.bifrost/asr/qwen3_asr_rs/asr-server` 启动 Bifrost 托管模型服务，等待 `/health` ready 后返回；托管进程日志写入固定 ASR 目录。托管 `asr-server` 进程同样被放入独立进程组，并注册 Bifrost 外层 physical-footprint watchdog，避免 Start Service 后长驻模型服务绕过目录任务的内存保护。
  - `POST /api/asr/service/stop` 只停止当前 Bifrost 实例启动的托管服务，用于释放内存和 GPU 资源；如果端口上是用户外部启动的进程，页面明确提示不能由 Bifrost 停止。
  - WebUI 默认不传端口；状态查询、转写和停止都解析当前 Bifrost 托管进程的动态端口，停止后页面显示“动态端口，启动时选择”，避免刷新后探测旧固定端口。
  - `POST /api/asr/transcribe-stream` 接收 WebUI multipart 音频上传，先用 FFmpeg 转为 16kHz mono WAV，再由 Bifrost 按 30 秒窗口、2 秒 overlap 顺序切片调用本地 qwen3_asr_rs OpenAI-compatible `/v1/audio/transcriptions`，并以 SSE 输出 `progress`、`final`、`text`、`error`、`done` 事件。`text` 是整次请求结束后的稳定文本汇总；如果模型返回 timestamp segments，`final` 会把 chunk 内时间平移到整段音频时间线。
- Bifrost CLI 新增 `bifrost ai asr`：
  - `bifrost ai asr start` 在启动前执行平台、模型/runtime 和 `ffmpeg` 自检；缺模型或 runtime 时直接使用 Rust 通用下载模块断点续传补齐，下载 client 绕过环境代理，缺 `ffmpeg` 时自动尝试 Homebrew 安装。自检通过后从固定 `~/.bifrost/asr/qwen3_asr_rs/asr-server` 启动模型服务，动态选择 loopback 端口，把 `pid/host/port/model/language/home/managed_by` 写入 `BIFROST_DATA_DIR/asr/service.json`。
  - `bifrost ai asr stop` 读取同一个 service state，停止对应 pid 并删除状态文件。
  - `bifrost ai asr status --json` 输出 CLI 和 WebUI 共享的模型服务状态；当下游命令（如 `grep -q`）命中后提前关闭 stdout 管道时，CLI 按普通 Unix 管道语义静默结束，不因 `Broken pipe` panic。
  - `bifrost ai asr stream-file <audio>` 确保资产可用后默认启动或复用本地 `asr-server`，按 30 秒窗口顺序发送 chunk 并输出 CLI JSON Lines；如果命令临时启动了模型服务，结束后恢复为停止状态。临时启动前同样执行自检与自动修复。
  - `bifrost ai asr task create|list|show|files|run|watch|tui|daily list|daily show` 通过当前 Bifrost Admin API 创建/检查目录定时任务、任务文件、watch TUI 和按日聚合 Markdown 文档；`watch`/`tui` 提供终端中实时刷新的任务面板（`crates/bifrost-cli/src/cli.rs` 中 `AiAsrCommands::Task` -> `AiAsrTaskCommands::Watch|Tui`）；未显式传 `-p` 时沿用 CLI 的 runtime port 解析，读不到 runtime 时回退 9900。`daily show` 默认把完整 Markdown 输出到 stdout，`--output` 可写入文件；`run --wait` 用于触发任务并等待后台运行结束，即使没有 pending 文件也会走后端的 daily 文档刷新路径。
  - 除上述 task 子树外，`bifrost ai asr` 当前还提供 `subtitle`（离线字幕产物：srt/vtt/txt/timeline_json/metadata）与 `diarization`（说话人分离 profile 初始化/管理）两组子命令；`stream-file` 与 `subtitle` 均支持 `--speaker-aware` 调用 Bifrost Admin 的 speaker diarization 与已登记声纹。
  - CLI 不依赖仓库脚本，不允许用户指定模型目录；除权重/runtime 下载外，启动、停止、状态和流式输出均由 Bifrost 内置命令编排。
- ASR 目录定时任务：
  - API 新增 `/api/asr/tasks`：创建、列表、详情、删除和手动运行目录任务。任务配置存储在 `BIFROST_DATA_DIR/asr/tasks.json`。
  - 任务绑定一个本机音频目录，默认递归扫描 `wav/mp3/m4a/webm/ogg/flac/aac/opus/mp4/aiff` 等常见音频文件；每次运行只处理尚未成功转写的文件。
  - 每个任务的文件状态存储在 `BIFROST_DATA_DIR/asr/tasks/<task_id>/files.json`，记录源文件路径、大小、mtime、录音创建时间、媒体时长、状态、错误、输出文本路径、timeline 路径和元数据路径。文件 key 使用 canonical path + size + mtime，源目录出现同名新文件时会重新进入 pending，而不是误复用旧 transcript。
  - 录音创建时间按优先级解析：`ffprobe` 容器 tags 的 `date + creation_time` 或 RFC3339 `creation_time`、文件名中的 `YYYYMMDD_HHMMSS`、filesystem birthtime、filesystem mtime。用户真实 `TX02_MIC001_20260514_114433_orig.wav` 样本包含 WAV tags `date=2026-05-14`、`creation_time=11:44:33`，文件名和 filesystem 时间只差 0-1 秒；0 字节坏文件会保留可解析的文件名时间并作为单文件 failed record，不中断整个任务。
  - 转写完成的文本保存到 `BIFROST_DATA_DIR/asr/data/text/<task_id>/<source_hash>.txt`，内容按时间片段渲染为 `[absolute start - absolute end] 文本`；同名 `.timeline.json` 保存结构化 segments，包含 `audio_start_ms/audio_end_ms` 与可选 `absolute_start_ms/absolute_end_ms`；目录任务长音频以 30 秒为最大 segment 窗口，即使原生 ASR CLI 只返回整段纯文本、不返回 timestamp，后端也按实际 chunk window 合成 timeline segment，且 timeline 读取时会兼容拆分旧版本遗留的超长单段数据；同名 `.json` 保存元数据。即使源音频之后被删除，已完成文本、timeline 和 metadata 仍保留；进度统计会单独展示 `deleted_after_processing`。
  - 任务详情 summary 同时展示当前音频目录下仍存在的音频原文件磁盘占用：`audio_source_bytes/audio_source_file_count`；并展示可安全清理的已转写原文件：`cleanable_source_bytes/cleanable_source_file_count`。可清理文件必须满足：文件记录为 `success`、源文件仍存在且 canonical path 位于任务 `audio_dir` 内、transcript/timeline 产物仍存在。`partial_success`、`failed`、`pending`、目录外路径和缺失输出产物都不进入可清理集合，避免破坏失败 chunk 重试或误删用户目录外文件。
  - Admin API 新增 `POST /api/asr/tasks/<task_id>/cleanup-source-audio`，用于一键清理已成功转写的音频原文件。接口在任务运行中或 failed-chunk 批量重试运行中返回 409；清理动作只删除满足上述可清理规则的源音频文件，保留 transcript、metadata、timeline、daily docs 和 file store 记录。接口返回 deleted/skipped/failed 文件数量、释放字节数和清理后的 summary，重复调用应幂等返回 0 个删除文件。
  - 调度配置使用显式墙钟周期 `schedule`，不再让用户填写秒级 interval。支持 `hourly`（每小时第几分钟）、`daily`（每天 HH:mm）、`weekly`（ISO 周一到周日 + HH:mm）和 `monthly`（每月第几天 + HH:mm，短月份自动钳制到月末）。创建任务时如果选择的当前分钟已经到达，会立即进入一次 due 状态；执行完成后按下一周期推进，避免同一分钟内反复运行。
  - 定时任务运行时会检查模型服务状态。如果服务已经健康运行，则复用并在结束后保持原状态；如果服务未运行，则在任务独占锁内临时启动，运行完成后停止并清理 service state，避免模型长期占用资源。
  - 任务运行使用进程内全局 ASR job lock 和每任务 `run.lock` 文件，避免多个定时任务同时竞争模型服务 start/stop。任务并发进入时会记录明确错误而不是互相覆盖状态。
  - `run.lock` 文件写入持有者 pid、进程启动时间和获取时间。服务重启后如果遗留旧格式锁、损坏锁或 pid 已不存在/已换代的锁，下一次任务运行会清理 stale lock 并重新获取；只有确认持有者进程仍存活时才返回“任务正在运行”，避免一次重启后目录任务永久无法再跑。
  - 目录任务新增可配置 `runtime_strategy`，用于对照 Qwen3-ASR-1.7B 在 MLX/Metal 下的批量推理稳定性和性能：
    - `reuse_per_file` 是默认生产策略，每个文件启动/复用一个 `asr-server`，文件内 chunk 复用该 server，文件结束后停止本次任务新启动的 server；2026-05-18 在同一 1801s/65 chunk 文件上实测比 `fork_per_chunk` 快约 11.1%；
    - `fork_per_chunk` 是保守隔离策略，每个 30 秒 chunk fork 一个 native `asr` CLI 子进程，并继续使用 physical-footprint guard、force-pause abort 和 memory-limit hint；
    - `reuse_server` 在一次任务运行内启动/复用一个 Bifrost 托管 `asr-server`，全部文件和 chunk 走同一个 server；
    - `auto` 先尝试 `reuse_server`；server 启动失败或 RTF 相对前三个稳定样本恶化超过 1.5 倍时记录 fallback reason 并切到 `fork_per_chunk`，运行中单个 chunk 的 server 调用失败时只让当前 chunk 降级，后续 chunk 在 fork 完成后串行重启 managed `asr-server` 再尝试 server 路径；
    - `compare` 同一 chunk 同时运行 `fork_per_chunk` canonical 输出和 `reuse_server` shadow 输出，最终文本采用 fork 结果，但持久化两边的耗时、RTF、文本 hash 和错误，便于定位复用路径是否出现性能退化或内容差异。
    WebUI 创建/编辑 Directory Task 时继续允许高级用户选择这些策略，但下拉菜单中每个选项必须直接展示简短说明：默认适用场景、性能/隔离取舍、fallback 行为或诊断用途；选中后输入框只显示短标题，避免把普通任务表单撑高。
  - 目录任务状态、单文件 metadata 和 WebUI 文件表会持久化 `runtime_strategy`、`fallback_reason` 和 `chunk_metrics`。每个 chunk metric 包含 `chunk_index`、offset/duration、runner、status、elapsed_ms、RTF、text_chars、text_sha1、server_url、fallback_reason、error 和 recorded_at_ms；后端同时输出 `ASR chunk metric` 日志。即使某个策略导致子进程或 server 被 watchdog kill、server 调用失败或整机任务异常中断，已写入的 `files.json`、metadata JSON 和日志仍能指出最后一个 chunk、runner、RTF、错误和 fallback 决策。
  - 服务启动和任务每次真正进入运行前都会修复上一次进程中断遗留的文件级 `processing` 状态：如果没有当前进程内运行任务，且 `run.lock` 不属于仍存活的其它 Bifrost 进程，则先删除 stale `run.lock`，再把孤儿 `processing` 文件恢复为 `pending`，清空旧开始时间、旧进度和旧 transient error。这样 daemon 重启、用户手动重启或进程崩溃后不会在 WebUI 留下假 processing 文件，也不会因为当前文件没有回到 pending 而跳过继续处理。
  - 启动恢复不仅修文件状态，还会形成运行恢复计划：对 `enabled=true`、`paused=false` 且仍有 `pending/failed` 文件的中断任务，scheduler 启动后立即重新入队执行，不依赖下一次墙钟周期；对 `paused=true` 的任务只清理 stale lock 和 orphan processing，不自动恢复运行，避免绕过用户主动资源让路。运行中标记使用 RAII guard 持有，后台任务 panic、提前错误返回或被 stale `run.lock` 拒绝时都会释放进程内 `RUNNING_TASKS`，防止 UI 长期展示假 `Running`。
  - 如果 Bifrost 重启前已启动同一 Directory Task owner 的健康 `asr-server`，重启后的 `start_managed_service` 会在分配新动态端口之前读取 `service.json` 并复用这个 persisted service；同 owner、同模型、同 home 的动态端口请求不会因为旧 server 端口不同而被误判为 busy。若上一次运行已经把 `managed ASR server start failed: Qwen3-ASR service is busy` 等可判定为服务获取临时失败的文件落成 `failed`，scheduler 启动恢复会把这些文件恢复为 `pending` 并立即重试；普通坏音频、ffmpeg 失败或模型永久错误不会因重启被无限自动重试。
  - `trigger_policy=after_asr_run` 的 Daily Agent 只在 ASR summary 没有 `pending`、`failed`、`partial_success`、`failed_chunk_count` 后触发；如果一次 ASR run 因 server busy、server acquisition 失败或 chunk 失败留下未完成工作，Daily Agent 不会基于不完整 daily markdown 自动生成报告，等待 ASR 恢复重试后再触发。Bifrost 重启后，如果上一次 Daily Agent 在旧进程中已写入 `last_status=running` 但当前进程没有对应运行锁，API 对外显示 `interrupted`，避免 WebUI 误以为仍有活跃 Daily Agent。对于存在 report 文件但 processed state 缺失的历史/中断报告，Run Results 使用任务绑定的 runner 名称作为展示 runner，同时保留 `last_run_id=filesystem-scan` 表示该行由 report 文件扫描补齐。
  - 目录任务支持资源让路暂停/继续：`POST /api/asr/tasks/<task_id>/pause?mode=temporary` 持久化 `paused=true` 但保留或计算下一次 `next_run_at_ms`，让正在运行的任务在文件边界和长音频 chunk 边界释放资源，并在下一次 scheduler 到点时自动清除 `paused` 后恢复后台运行；`POST /api/asr/tasks/<task_id>/pause?mode=long_term` 持久化 `paused=true` 并清空下一次调度时间，整个计划暂停，必须手动 Resume。`POST /api/asr/tasks/<task_id>/pause?force=true` 额外登记 force-pause 标记，正在执行的 native `asr` 子进程和当前 `ffmpeg` normalize/split 子进程都会被主动 kill，默认保持长期暂停语义；`POST /api/asr/tasks/<task_id>/resume` 只在请求线程内清除暂停与 force-pause 状态，并立即派发后台 run，由后台 run 去扫描目录和继续处理 pending/failed 文件。控制接口和运行中任务列表使用 `files.json` cached summary，不能在 WebUI 主流程里递归扫描音频目录、重建 heavy summary 或对历史大音频同步计算内容 hash。导入复制期间的 BLAKE3 只能在后台阻塞 worker 的全局内容哈希队列里串行执行；ASR run 遇到缺少 hash 的历史文件时退化为普通路径处理，避免 Resume、启动恢复或自动刷新把代理主服务拖到 CPU 100%。暂停期间手动 Run 返回 409 并提示先 Resume；只有临时暂停会在下一次计划调度时自动恢复。
  - 目录任务 run 不再把启动瞬间扫描到的 pending 列表视为固定全集。每处理完一批 pending 文件后，后台 run 会重新递归扫描 `audio_dir`、同步新增 source record、应用外部导入 hash 与内容去重，再把尚未在本次 run 尝试过的 `pending` / `processing` / 历史 `failed` 文件加入下一批；如果没有新增待处理文件才刷新 daily markdown、更新任务运行状态并按配置触发 Daily Agent。每批队列按 `source_created_at_ms` 从早到晚处理，缺少创建时间时退回 `source_modified_ms` 和路径排序，使用户先看到更早录音的 transcript/daily 内容，也避免同一次 run 中失败文件被无限重试。
  - 长音频仍以 30 秒为默认最大 chunk，但不再预先并发切出所有 chunk；后端现在按 `切当前 chunk -> ASR -> 删除当前 chunk -> 进入下一个 chunk` 的顺序流式处理，避免几小时录音一次性拉起大量 `ffmpeg` 进程或堆积临时 WAV 文件。normalize 与 split 均使用可中断子进程，force-pause 会尽快释放 CPU、磁盘 IO 和 Metal/MLX 计算资源。WebUI 文件上传链路也按 30 秒窗口顺序切片送 `asr-server`，不再 whole-file 推给模型。
  - 目录任务的 `fork_per_chunk` native `asr` CLI 调用和 WebUI/CLI 托管 `asr-server` 启动都带 macOS physical-footprint guard。1.7B 在特定 30 秒音频 chunk 上可能出现 `ps` RSS 只有 3-4 GiB、但 `vmmap` physical footprint 持续涨到 20 GiB+ 的 Metal/MLX 异常路径；后端优先按模型官方规模和实测 30 秒 chunk 峰值设定 footprint 上限：`Qwen3-ASR-0.6B` 默认 8192 MiB，`Qwen3-ASR-1.7B` 默认 18432 MiB，未知模型默认 12288 MiB，并用宿主机总内存的 90% 作为二级安全阀（所以 64GB 机器不会仅因内存更大就把 1.7B 放宽到 20GB+，16GB 机器也会收敛到约 14.4 GiB）。`BIFROST_ASR_MAX_FOOTPRINT_MB` 只能在安全上限内向下收紧；如果确实要关闭 watchdog，必须显式设置 `BIFROST_ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD=1`。当前 second-state `qwen3_asr_rs` v0.2.0 release 自身没有设置 MLX memory/cache/wired limit：源码只调用 `init_mlx(true)`、`mlx_set_default_device` 和 stream 初始化，未暴露 `set_memory_limit` / `set_cache_limit` / `set_wired_limit` 参数；因此 qwen 二进制默认实际沿用 MLX runtime 策略（Metal memory limit 默认为 recommended working set size 的 1.5 倍，cache 默认跟随 memory limit，wired 默认不设）。Bifrost 不能只通过环境变量直接限制其内部 MLX allocator，因此生产防护采用外层 watchdog + chunk/input cap + 失败记忆三层策略。为避免 watchdog 自身拖慢 30 秒正常推理，force-pause/进程退出仍每 500ms 轮询，但重型 `vmmap -summary` physical-footprint 首次采样会延后一个采样周期，之后默认每 5 秒一次；`BIFROST_ASR_PHYSICAL_SAMPLE_INTERVAL_SECS` 可在 2-60 秒范围内调整。可靠 physical footprint 超限或 force-pause 触发时会 kill 当前 `asr`/`asr-server` 进程组并把错误交给 bisect/pause/status fallback；physical footprint 暂时不可用或 sampler 报错时只记录 warning，不再用 RSS-only fallback 过早杀掉托管服务。正常 chunk 仍保持 30 秒默认窗口，不改变最佳性能路径。`bifrost ai asr start` 直接拉起长驻 daemon 后 CLI 进程会退出，当前无法由同一 CLI 进程持续 watchdog；该入口后续应收敛到 Bifrost daemon/supervisor 托管。
  - 目录任务会把某个文件、模型、chunk offset/duration 上发生过的 memory-limit bisect 结果持久化到该文件的 `memory_limit_hints`。后续同一文件再次处理时，匹配 chunk 会直接使用已学习的较小 window，不再先重撞完整 30 秒高风险路径。
  - WebUI ASR 页面新增 Directory Tasks 区域，支持创建 hourly/daily/weekly/monthly 周期任务、手动 Run、删除任务、查看 processed/pending/failed/deleted-after-processing 总体进度和下一次运行时间。该区域在 AI -> Tools -> ASR 首页中固定放在 `Speech Converter` 状态面板下方、`Speech to Text` 工作区上方，保证目录任务入口优先可见。点击任务详情会进入 AI -> Tools -> ASR 的任务子页面，并通过 `asrTask=<task_id>` 查询参数承载状态；子页面不再使用 Drawer/弹窗承载详情，避免后续继续查看文件详细内容时被遮罩、宽度和滚动限制影响。任务详情页最顶部是 tab 导航，第一个 `Overview` tab 展示任务 schedule、last/next run、总体进展、运行状态、音频占用和 last error；后续 tab 包含 Files、Daily Docs、Daily Agent 和 Daily Agent Records。Files 表格始终按 Recorded 时间倒序展示，最新录音/文件在最前，状态筛选只缩小集合、不改变排序语义；Daily Docs tab 按 `YYYY-MM-DD` 列出后端从 timeline 聚合生成的日文档，点击某一天进入 `asrTask=<task_id>&asrDay=<date>` 子页面，展示完整 Markdown 内容、文档路径、大小与更新时间；Daily document 详情正文不允许嵌套纵向滚动，长文档自然撑开页面并只使用 ASR 页面最外层滚动条。后端暴露 `GET /api/asr/tasks/<task_id>/daily` 与 `GET /api/asr/tasks/<task_id>/daily/<YYYY-MM-DD>`，读取前会从现有 timeline 产物刷新 `BIFROST_DATA_DIR/asr/data/text/<task_id>/daily/<date>.md`，日期参数只接受 `YYYY-MM-DD`，避免路径穿越。文件开始时间由后端在文件进入 processing 时写入并在 success/partial_success/failed/paused 重建 FileRecord 时保留；执行耗时由 WebUI 用 `finished_at_ms - started_at_ms` 或 processing 状态下的当前时间滚动计算，避免只有结束时间而无法判断单文件性能。任务文件表格的长路径列通过表格内部横向滚动承载，不允许撑出页面主内容区域；文件表分页大小为受控状态，切换后必须即时按新 page size 渲染。成功文件在文件名旁提供 Open transcript 入口，点击后进入 `asrTask=<task_id>&asrFile=<file_key>` 的单文件详情页。单文件详情页顶部通过 `/api/asr/tasks/<task_id>/files/<file_key>/source` 播放源音频，下面按 timeline segment 展示模型转写文本；点击任意 segment 的时间点会把播放器跳转到对应 `audio_start_ms`，播放器播放或拖动进度条时会高亮当前 segment 并自动滚动到对应字幕位置；用户手动滚动字幕时自动滚动暂停 5 秒，超时后恢复跟随当前播放段；如果用户在暂停窗口内操作音频播放轴、点击播放或点击字幕时间点，则用户指定位置优先，自动跟随立即恢复并滚动到当前播放段，便于人工双向对照听到的原始内容和模型输出是否一致。页面每 10 秒刷新一次任务状态；当任务处于 running 状态时，列表和详情 summary 走 cached summary，避免自动刷新与 Resume 点击后立即触发重型目录扫描。
  - Directory Task 的 `audio_dir` 输入允许绝对路径和 `~/xxx`。后端在创建、编辑和加载旧 `tasks.json` 时统一把 `~` 展开到运行用户的 home 目录，并把普通相对路径按 home 目录解析为绝对路径，禁止再把 `~/audio` 或 `audio` 当作相对路径落到 `BIFROST_DATA_DIR` / `.bifrost` 下面。API、CLI 和 WebUI 列表/详情展示的 `audio_dir` 必须是规范化后的绝对路径；WebUI 表单仍可输入 `~/Recordings`，保存后回显绝对路径。旧数据兼容策略是读取时自动规范化，后续任何保存都会把绝对路径写回 store。
  - 任务详情页中的 Daily Agent 能力拆成两个平级 tab：`Daily Agent` 只承载配置、IM delivery、AGENTS.md 指令编辑、Run Now/Force Run/Send Report/Refresh 等执行入口；`Daily Agent Records` 只承载已处理文档、运行结果、report 链接和刷新入口。打开 `asrDailyReport=<date>` 的 report 详情时，`asrTaskTab` 默认回到 `daily-agent-records`，避免记录内容混在配置执行 tab 中。
  - WebUI Directory Tasks 列表和任务详情页展示独立 Run State：Ready、Running、Pausing、Paused、Paused until schedule。Running/Ready 时 Pause 按钮提供 `Pause until next schedule` 与 `Pause indefinitely` 两个选项；Paused 时显示 Resume 按钮；Run 在 paused/running 状态下不可用，避免重复启动或绕过资源让路状态。Schedule 的 disabled 状态只表示后续定时调度关闭，不再和资源暂停混用文案。
- 浏览器麦克风实时转写架构：
  - 这是 30 秒批处理窗口的明确例外：默认窗口为 1000ms，默认 overlap 为 300ms；API query 可传 `window_ms` / `overlap_ms` 调整，窗口下限 300ms，overlap 最大为窗口的一半，用于保证录音时的快速反馈。
  - 后台只保留当前上传、规范化 WAV 和当前窗口临时文件；窗口文件每次模型调用后删除，上传体有 512MB 上限，避免长音频导致无限内存或临时文件堆积。
  - 规范化 WAV 解析为 16kHz mono PCM 后计算窗口。每个非尾段窗口会在目标 1 秒边界前 250ms、后 125ms 范围内寻找 50ms frame 的最低能量点，作为更自然的稳定边界；如果无法找到可靠低能量点，则退回固定 1 秒边界。
  - 每个窗口实际送模范围为 `stable_start - overlap` 到 `stable_end`，用于给模型保留 trailing context；`stable_start..stable_end` 是本窗口可确认范围。
  - 后台先发送 `partial`，包含当前 overlapped window 的候选文本和相对已确认文本的 `delta`；随后在该窗口边界确认后发送 `final`。尾段不足 1 秒但超过最小窗口时会在 EOF flush 为 final；短于 300ms 或空音频返回确定的空文本 done。
  - 文本连续性使用最长 suffix/prefix overlap 去重；如果模型重复返回已提交文本则 delta 为空，不重复提交。中文字符之间不额外插入空格，英文片段之间补空格。
  - 单个窗口模型请求对 5xx 或网络失败重试一次。窗口仍失败时发送带 window index 的 `error` 事件并继续后续窗口；如果整次请求没有任何稳定文本且出现模型错误，则最终返回错误。
- AI 模块新增 Tools 分组，首个工具为 ASR，并内嵌“Speech Converter”状态面板：
  - 明确显示未安装、初始化中、下载中、模型 ready、错误状态；
  - 未安装或初始化失败时显示初始化下载进度条、当前下载文件、已下载体积、总体积、速度、预计剩余时间和错误详情；已安装后隐藏初始化按钮和整个下载进度模块；
  - 初始化动作按需触发，不影响 Bifrost 启动或其它 AI 页面加载。
  - Start Service / Stop Service 由 Bifrost 后台管理本地模型进程，用户不需要单独部署或手工启动 qwen3_asr_rs；
  - 页面明确提示当前仍依赖外部下载源：GitHub release 提供原生 qwen3_asr_rs runtime，Hugging Face 提供 Qwen3-ASR 权重。任一海外源不可达时，下载进度区域和错误区域展示具体失败原因。
  - 存储目录展示为固定的 `~/.bifrost/asr`，不可在 WebUI 中修改。
- ASR 工具输入与转写区：
  - 文件/麦克风输入和 Transcript 输出合并在同一个 `Speech to Text` 工作卡片中，Audio Input 输入模块固定放在卡片顶部，Transcript、错误详情和 stream events 放在同一卡片下方，避免左右双卡片在窄屏或截图中割裂；
  - 支持拖入音频文件或点击按钮选择音频；
  - 文件上传/文件流式转写时显示 `File transcription progress` 进度条；浏览器麦克风实时输入是持续流，只显示实时电平音轨和 stream events，不显示百分比处理进度，避免把实时录音误导成固定长度任务；
  - 支持浏览器麦克风录音，`MediaRecorder` 以 1 秒 timeslice 产生音频块，WebUI 通过 `/api/asr/transcribe-ws` WebSocket 持续发送二进制音频 chunk；后端保留同一 MediaRecorder 会话的完整 WebM 字节流用于 FFmpeg 解复用，每次 flush 将完整会话转为 16kHz mono WAV，再只切出“上次已确认时间点 - overlap”到当前可解码时长的新增片段送模型，避免把后续 WebM timeslice 当作独立文件导致 FFmpeg 解析失败；
  - 麦克风实时输入启用后，Audio Input 面板显示 live input level 音轨。前端从同一个 `MediaStream` 创建 Web Audio `AnalyserNode`，以约 30fps 采样频域能量并渲染固定 40 个电平条；Stop Mic、Cancel、WebSocket 错误或组件卸载时取消 `requestAnimationFrame`、关闭 `AudioContext` 并把电平归零，避免录音停止后继续占用麦克风分析资源；
  - 展示 WebSocket `connected`、`stream`、`partial`、`final`、`text`、`done` 与 `error` 事件，Transcript 继续使用 suffix/prefix 去重避免 overlap 或模型重复输出；
  - 音频输入区消费同一工具板块里的模型状态，未 ready 时提示在 AI -> Tools -> ASR 初始化。

## 依赖项

- macOS Apple Silicon (`arm64`)。
- Homebrew 依赖：`ffmpeg`。下载、断点续传和 zip 解压由 Rust 模块完成，不再依赖 `curl` 或 `unzip` 命令。
- 外部下载源：
  - `https://github.com/second-state/qwen3_asr_rs`
  - `https://huggingface.co/Qwen/Qwen3-ASR-0.6B`（默认）
  - `https://huggingface.co/Qwen/Qwen3-ASR-1.7B`（可选，推荐 32GB 以上）
- Rust 依赖使用 bifrost-admin 现有 `reqwest`、`tokio-stream`、`http-body-util` 和 `url`。
- 共享 ASR service state 使用 `bifrost-admin::asr_runtime`，CLI 和 Admin API 共享同一 JSON schema。
- WebUI 使用现有 React、Ant Design 与 `@ant-design/icons`，不新增 Node package。
- 本机既有 `~/ai/asr` 下载内容已同步到 `~/.bifrost/asr`，后续验证直接复用固定目录。

## 测试方案

### 单元测试

- `bash -n e2e-tests/tests/test_qwen3_asr_local_server.sh`
- `cargo test -p bifrost-admin resource_download` 覆盖 Range 续传头、续传总量合并和下载百分比边界。
- `cargo test -p bifrost-admin asr` 覆盖本地 host/port 校验、Rust 初始化资源任务、流式切片、overlap、能量边界、尾段 flush、去重/连续性、WAV 解析和 query 参数边界。
- `cargo test -p bifrost-admin asr_jobs --lib` 覆盖递归音频发现、输出目录、源文件删除后仍保留已处理元数据、日文档从 timeline 产物聚合生成与日期路径校验。
- `cargo test -p bifrost-admin asr_jobs --lib` 覆盖任务详情中的音频原文件磁盘占用统计、可清理字节统计，以及 `cleanup-source-audio` 只删除 `success + transcript/timeline 已存在 + audio_dir 内` 的源音频，保留 partial-success、pending/failed 和目录外文件。
- `cargo test -p bifrost-admin asr_jobs --lib` 同时覆盖旧任务 JSON 缺少 pause 字段的兼容反序列化、pause/resume 对 `paused/paused_at_ms/next_run_at_ms/last_error` 的持久化影响。
- `cargo test -p bifrost-admin startup_recovery --lib` 覆盖 stale `run.lock` 启动恢复：enabled 未暂停任务会重置 `processing` 并加入恢复计划，paused 任务只清理状态不自动运行，仍存活 owner lock 不会被抢占。
- `cargo test -p bifrost-admin asr_jobs --lib` 覆盖旧任务 JSON 缺少 `runtime_strategy` 时默认 `reuse_per_file`，以及 chunk metric 的 runner、RTF、文本 hash、fallback reason 和 error 记录。
- `cargo test -p bifrost-admin normalize_task_audio_dir_path --lib` 和 `cargo test -p bifrost-admin load_tasks_normalizes_legacy_home_and_relative_audio_dirs --lib` 覆盖 Directory Task `audio_dir` 的 `~/xxx` 展开、普通相对路径按 home 目录解析、绝对路径保持不变、空输入拒绝，以及旧 `tasks.json` 自动兼容为绝对路径且不落到 `BIFROST_DATA_DIR`。
- `cargo test -p bifrost-admin asr_cli_invoke --lib` 覆盖 native ASR CLI 输出解析和 `vmmap` footprint 单位解析，保证 memory guard 的阈值计算可回归。
- `cargo test -p bifrost-cli asr --lib` 覆盖 CLI 读取共享 ASR service state，以及 `status --json` 管道提前关闭时忽略 stdout `BrokenPipe`、其它 IO 错误仍返回。
- `cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands` 覆盖 `bifrost ai asr` 子命令解析。
- WebUI 类型检查或构建覆盖 AI -> Tools -> ASR 初始化面板和音频输入区编译。

### E2E 测试

- 新增 `e2e-tests/tests/test_qwen3_asr_local_server.sh`。
- 新增 `e2e-tests/tests/test_asr_task_pause_resume.sh`，使用临时 `BIFROST_DATA_DIR` 和空音频目录覆盖不依赖模型下载的 pause/resume Admin API：创建任务、pause 后 `next_run_at_ms=null`、paused 状态下 Run 返回 409、resume 清除 paused 并快速派发后台 run，空目录后台 run 快速结束且不启动模型资产检查。
- 新增 `e2e-tests/tests/test_asr_task_startup_recovery.sh`，使用临时 `BIFROST_DATA_DIR` 预置 stale `run.lock` 和 orphan `processing` 文件，启动最新 bifrost 后访问 ASR API 触发 scheduler startup，断言 stale lock 被删除、文件状态恢复 `pending`、paused 任务不会展示 running。
- 新增 `e2e-tests/tests/test_asr_task_cli.sh`，使用临时 `BIFROST_DATA_DIR` 启动当前 Bifrost 二进制，不下载 ASR 模型：创建空目录任务、写入 `asr/data/text/<task_id>/daily/<YYYY-MM-DD>.md`，验证 `bifrost ai asr task list` 不传 `-p` 能读取 runtime port，`show/files/daily list/daily show --output/run --wait` 均可通过真实 Admin API 返回预期结果。
  - 同一脚本使用临时 `HOME` 创建 `audio_dir="~/bifrost-asr-home-audio"` 的任务，断言创建响应、详情和 `tasks.json` 都保存为临时 home 下的绝对路径；再 PATCH `audio_dir="relative-audio"`，断言普通相对路径也转换到 home 下，且不在 `BIFROST_DATA_DIR` 下创建 `~/audio` 或 `relative-audio`。
- 默认做离线结构验证：脚本语法、帮助输出、缺参失败、CI 模型运行时 guard、preflight 可执行；CI shard 缺少 `ffmpeg` 时验证依赖错误可读后跳过在线段。
- CI 环境无条件跳过在线模型段，即使误设置 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 也不会下载权重、安装 runtime、启动 `asr-server` 或部署 Bifrost 托管 ASR 服务。
- 当 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 时执行真实部署验证：
  - 安装 Qwen3-ASR 默认模型（当前 `Qwen3-ASR-0.6B`，可显式覆盖为 1.7B）；
  - 运行中文 sample CLI 转写；
  - 通过 Bifrost Admin 启动托管本地 API server；
  - 验证 `/health`、`/v1/models`；
  - 调用 `/v1/audio/transcriptions` 验证中文转写文本包含 `Qwen3`。
- 扩展 E2E 覆盖 Bifrost Admin `/api/asr`：
  - 调用 `/api/asr/init-stream` 验证资产已安装时不会重新下载，事件包含 `installed`/`done`；
  - 调用 `/api/asr/service/start` 验证 Bifrost 托管服务启动后 `/api/asr/status` 返回 ready；
  - 通过 `/api/asr/transcribe-stream` 上传中文样例，验证 SSE 中包含 `final` 事件和最终中文文本；长音频验证每个模型请求窗口不超过 30 秒；
  - 调用 `bifrost ai asr stream-file`，验证 CLI 侧输出 30 秒窗口的 segment/final JSON Lines；
  - 通过 FFmpeg 把中文样例转换为 WebM，模拟浏览器麦克风产物，验证后台 `preprocess` 事件、WAV 归一化和最终中文文本；
  - 通过 `/api/asr/transcribe-ws` 发起真实 WebSocket 握手，发送 `start` 控制帧、切成多个 binary frame 的 WebM 音频帧和 `finish` 控制帧，验证顶层 `type` 直接包含 `connected`、`stream`、`partial`、`final`、`text`、`done` 事件及中文文本，且事件 detail 包含递增的 `processed_ms`，避免实时阶段事件被统一折叠为 `progress` 或后续 WebM chunk 被当作独立文件解析失败；
  - 调用 `/api/asr/service/stop` 验证托管服务停止后状态变为 not ready；
  - 端口错误时验证 AI -> Tools -> ASR 可展示的错误事件。
  - 验证 `bifrost ai asr --help`、`bifrost ai asr stream-file /missing.wav` 错误路径、`status --json` 共享状态输出，并覆盖 `status --json | grep -q '"ready"'` 这类管道消费者提前退出的回归路径。
  - 验证 `/api/asr/tasks` 在临时 `BIFROST_DATA_DIR` 下可以创建目录任务、列表展示 pending/processed 统计、手动 run 在模型不可用时返回明确错误且不会删除已保存文本元数据。
  - 验证 `/api/asr/tasks/<task_id>` summary 返回 `audio_source_bytes` 和 `cleanable_source_bytes`；调用 `/api/asr/tasks/<task_id>/cleanup-source-audio` 后，成功源音频被删除、text/timeline 产物仍存在、partial-success 源音频仍保留，二次调用删除数量为 0。
  - 验证 `/api/asr/tasks/<task_id>/daily` 可以列出已有按天 Markdown 文档，`/api/asr/tasks/<task_id>/daily/<YYYY-MM-DD>` 返回完整内容，非法日期返回可读错误。
  - 验证目录任务创建响应默认包含 `runtime_strategy=reuse_per_file`；对 `auto`、`reuse_server`、`fork_per_chunk`、`compare` 的真实模型对照实验需要检查 `files.json`、metadata JSON 和 `ASR chunk metric` 日志中的 runner/RTF/text hash/fallback reason。
  - 验证 WebUI Directory Task 创建/编辑弹窗展开 Runtime 下拉后，每个策略名称下方都有面向用户的说明，并且默认选中值仍为 `reuse_per_file`。
  - E2E 固定使用 `~/.bifrost/asr`，不再创建或传入临时模型 home。

### 真实场景测试

- 新增 `human_tests/qwen3-asr-local-server.md`。
- 覆盖：
  - Apple Silicon 与 32GB 内存检查；
  - 依赖安装；
  - Qwen3-ASR 非交互安装（默认 0.6B；显式选择时也覆盖 1.7B）；
  - CLI 中文样例转写；
  - API server 健康检查和中文转写；
  - 长音频切片命令的文件产物验证；
  - AI -> Tools -> ASR 初始化状态、下载进度、外部依赖提示和错误详情；
  - 刷新页面后重新订阅初始化流，确认后台下载任务仍在继续且可看到当前下载进度；
  - AI -> Tools -> ASR Start Service / Stop Service 托管服务生命周期；
  - 临时移动固定目录下某个模型文件到备份目录，触发初始化下载进度展示，验证后恢复文件；
  - WebUI ASR 页面文件拖入/选择后流式输出文本；
  - 麦克风录音入口通过 WebSocket 按 1 秒 timeslice 持续发送音频、WebM 录音产物自动转 WAV、权限错误或实时链路错误展示路径。
  - 麦克风实时输入电平条：未录音时显示归零状态；Start Mic 后随输入音量波动；Stop Mic / Cancel / 错误后恢复归零，亮色和暗色主题下均可读。
  - `bifrost ai asr`：启动模型、查看状态、单文件流式转写标准输出、停止模型。
  - Directory Tasks 首页顺序：进入 AI -> Tools -> ASR 后确认 `Directory Tasks` 位于 `Speech Converter` 下方、`Speech to Text` 上方，且调整位置不影响任务创建、详情打开和手动 Run。
  - Directory Tasks：在 WebUI 创建绑定目录的递归任务，验证 daily/weekly/monthly 周期选择不会退化成秒级 interval；查看总体进度，点击任务进入 AI -> Tools -> ASR 子页面详情，确认 URL 包含 `asrTask=<task_id>` 且页面没有 dialog/Drawer，查看逐文件状态和输出路径；切换 Daily Docs tab，按日期查看聚合文档列表，点击某一天进入完整 Markdown 内容页并确认 URL 包含 `asrDay=<YYYY-MM-DD>`；点击已处理文件进入 `asrFile=<file_key>` 单文件详情，验证源音频播放器、timeline 文本、每个 segment 的 `audio_end_ms - audio_start_ms` 不超过 30 秒、点击时间点跳转播放位置、播放/拖动音频时字幕自动高亮并滚动到当前 segment、手动滚动字幕后自动跟随暂停 5 秒并随后恢复、暂停期间操作音频播放轴或点击字幕时间点会立即恢复自动跟随，手动运行，验证处理过的文本保存在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/`，删除源音频后文本和元数据仍可保留在进度统计中；模拟服务重启后的旧 `run.lock`，确认任务不会永久报 `ASR task is already running or lock is stale`。
  - Directory Tasks 原音频磁盘清理：任务详情页展示当前音频原文件总占用和可清理占用；点击 Clean originals 前有确认文案说明 transcript/timeline 保留且 partial-success 不删除；确认后已成功转写的原音频从 `audio_dir` 删除，页面刷新后占用下降、`deleted_after_processing` 增加，单文件 transcript/daily docs 仍可打开；重复点击不报错且删除数量为 0。
  - Daily Agent tab 拆分：进入任务详情后确认 `Daily Agent` tab 只显示配置、执行按钮和 AGENTS.md 编辑器，不显示 Processed Documents 表格；切换到 `Daily Agent Records` tab 后看到运行记录/结果表、report 链接和独立 Refresh；点击 report 链接进入 `asrDailyReport=<date>` 详情，再返回时仍停留在 `daily-agent-records`。
  - Directory Tasks 资源让路：运行中点击 `Pause until next schedule`，确认任务进入 Pausing/Paused until schedule，后台在当前文件或 chunk 边界释放运行状态，并在下一次计划调度到点后自动恢复；点击 `Pause indefinitely`，确认任务进入长期 Paused 且不会被 scheduler 自动恢复；暂停期间 Run 按钮不可用且 API Run 返回 409；点击 Resume 后请求快速返回并把任务切到 Running，目录扫描和 pending/failed 文件处理在后台进行，主服务轻量 API 不被阻塞；无 pending/failed 文件时后台 run 快速恢复 Ready，已 success 文件不重跑，未完成文件保持 pending 后继续处理。
  - Directory Tasks 内存保护：使用 `~/Downloads/we` 中可复现的 1.7B 问题录音创建任务，确认 30 秒 chunk 保持默认；当 native `asr` 子进程 physical footprint 超过阈值时，日志出现 footprint limit/bisect 提示，任务继续拆小子段而不是让系统进入 20 GiB+ 卡死状态。

### Directory Task 子页面与单文件时间轴闭环方案

- Review/Fix/Test 第 1 轮：复核 Directory Tasks 列表、`View details` URL 状态、任务详情返回、Run/Refresh、Daily Docs tab、`asrDay=<YYYY-MM-DD>` 完整文档页、Open transcript 单文件详情、源音频 `/source` endpoint、timeline 时间点点击跳转播放器、播放器播放或 seek 后字幕自动滚动、stale `run.lock` 清理；运行 `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts` 覆盖子页面、按天文档和单文件路径，运行 `cargo test -p bifrost-admin task_run_lock --lib` 覆盖锁恢复，若被其它模块既有编译问题阻塞需记录精确原因。
- Review/Fix/Test 第 2 轮：复查 `web/src/pages/ASR/index.tsx` 已按功能拆分且所有单文件低于 1500 行，没有残留 Drawer/dialog 依赖；`human_tests/qwen3-asr-local-server.md` 和索引与实际行为一致；复跑前端构建、目标 E2E、Rust fmt/check，确认亮色/暗色主题下任务详情和单文件详情依旧使用 Ant Design token 和现有 CSS 变量。
  - 回归验证真实 MediaRecorder 多 chunk：录制 5-8 秒期间 Stop Mic 前必须出现 connected/stream 和 partial/final 事件，后端事件 detail 应包含递增的 `processed_ms`，不得再出现“后续 WebM chunk 不能被单独 ffmpeg -i 解析”的错误。
- 更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：1.7B、Apple Silicon、MLX/Metal、本地 API server、中文样例、长音频 30 秒切片、AI -> Tools -> ASR 初始化状态/进度/错误、Start/Stop 托管服务、WebUI 文件上传 30 秒送模、浏览器麦克风输入保留 1 秒实时响应、麦克风实时电平音轨、overlap/能量边界/去重/尾段 flush。
- 检查新增脚本是否会修改 `~/.zshrc` 或系统代理。
- 检查 Bifrost 启动路径没有同步下载/加载模型，初始化只由 AI -> Tools -> ASR 点击触发，常驻模型服务只由 Start Service 启动。
- 执行 `git status --short`、`git diff`。
- 运行脚本语法、帮助、preflight、真实 `verify`、后台 ASR API 最小测试和前端构建。
- 修复发现的问题并复跑对应命令。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff。
- 检查 `design/`、`human_tests/`、E2E 脚本、AI Tools 面板文案和实际部署命令是否一致。
- 复跑受影响测试，确认 `/health`、`/v1/audio/transcriptions` 和 `/api/asr/*` 都来自常驻 server 或清晰错误事件。
- 如仍发现缺口，追加第 3 轮。

## 校验要求

- 必须执行 `BIFROST_QWEN3_ASR_E2E_ONLINE=1 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`，除非模型下载或运行环境阻塞。
- 必须验证 `/api/asr/transcribe-ws` 真实 WebSocket 链路，至少覆盖握手、二进制音频 chunk、`finish` final flush、顶层 `connected/stream/partial/final/text/done` 事件和错误可见性。
- WebUI/后台变更必须执行 Rust fmt/clippy/test、WebUI 构建或类型检查、真实浏览器 human_tests。
- 初始化必须证明是异步按需触发：Bifrost server 启动后不下载模型、不加载模型；只有 AI -> Tools -> ASR 初始化请求才启动下载/验证流程。
- 模型目录必须证明固定：`--home` 和 API query 不应改变状态返回的 install/model dir。

## 文档更新要求

- 更新 `human_tests/readme.md`。
- WebUI 在 AI 模块新增 Tools -> ASR 入口，必要时更新用户可见文档或 human_tests 入口说明。
- 不改变 Bifrost CLI、规则协议或代理转发语义。
