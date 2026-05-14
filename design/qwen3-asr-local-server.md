# Qwen3-ASR 本地 API Server

## 功能模块说明

本模块把 Qwen3-ASR-1.7B 在 32GB Apple Silicon Mac 上的资源准备、启动与验证固化为 Bifrost 内置 Rust 能力，并在 WebUI 中提供语音转文字入口。目标路线是：

```text
Qwen3-ASR-1.7B + qwen3_asr_rs + MLX/Metal + 本地 OpenAI-compatible API Server
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
  - 默认模型为 `Qwen3-ASR-1.7B`；
  - 通过 Rust 通用下载模块下载 qwen3_asr_rs 最新 release 二进制；
  - 通过 Rust 通用下载模块下载 Hugging Face 权重文件；
  - 从 release 内置 tokenizer 复制对应 `tokenizer.json`；
  - 下载上游 sample 音频，其中 `sample3.wav` 是中文验证样例。
- Rust 初始化流程直接调用下载后的 `asr` 二进制做中文样例转写验证。
- WebUI 托管服务不使用默认端口，点击 Start Service 时由 Bifrost 后台动态选择空闲 loopback 端口并直接启动 `asr-server` 二进制；启动模型不需要脚本。
- `health` 验证 `/health` 返回 `{"status":"ok"}`。
- `transcribe` 通过 `/v1/audio/transcriptions` 上传音频并支持 `text/json/verbose_json` 输出。
- `stream-file` 通过固定 1 秒窗口、默认 0.3 秒 overlap 调用本地 API server，按 JSON Lines 输出 `partial` 与 `final` 片段，作为 CLI 侧真实流式验收入口。
- `chunk` 和 `batch-transcribe` 支持长音频先切 60 秒片段，再逐段调用 API 服务。
- `verify` 串联 preflight、install、CLI 中文样例、API server、health、models、multipart transcription，用真实模型完成端到端验收。
- CI 环境默认禁止下载、安装或启动 Qwen3-ASR 模型运行时。`install`、`prepare`、`run-sample`、`start-server`、`verify` 在 `CI=true` / `GITHUB_ACTIONS=true` 等环境下会直接失败并提示原因，除非显式设置 `BIFROST_QWEN3_ASR_ALLOW_CI_MODEL=1`。仓库 E2E 即使误设 `BIFROST_QWEN3_ASR_E2E_ONLINE=1`，在 CI 中也只执行结构/错误路径验证，然后跳过在线模型段。
- Bifrost Admin 新增 `/api/asr` 后台 API：
  - `GET /api/asr/status` 只做短超时健康探测和本地安装目录检查，不触发下载，也不阻塞 Bifrost 启动。
  - `GET /api/asr/init-stream` 在用户从 AI -> Tools -> ASR 点击初始化后启动或订阅后台初始化任务，返回 `text/event-stream`。页面刷新或连接中断后再次进入会回放任务历史并继续接收当前进度，不会中断后台下载。
  - 初始化流程只准备和验证本地资产：Rust 模块完成预检、断点续传下载、release zip 解压、runtime 安装、tokenizer 复制和 CLI 中文样例验证，不启动常驻模型服务。
  - 后台 API 和 CLI 都固定使用 `~/.bifrost/asr`，不接受 WebUI 或 API 指定其它模型目录。
  - 初始化失败时把依赖缺失、模型下载失败、解压/安装失败、验证退出码、外部源不可达等信息作为 `error` 事件返回给 WebUI。
  - `POST /api/asr/service/start` 启动前执行同一套自检：平台不支持时直接返回 unsupported；模型、runtime 或样例文件缺失时自动复用 Rust 断点续传下载与安装流程补齐；`ffmpeg` 缺失时自动尝试 Homebrew 安装。自检通过后再从 `~/.bifrost/asr/qwen3_asr_rs/asr-server` 启动 Bifrost 托管模型服务，等待 `/health` ready 后返回；托管进程日志写入固定 ASR 目录。
  - `POST /api/asr/service/stop` 只停止当前 Bifrost 实例启动的托管服务，用于释放内存和 GPU 资源；如果端口上是用户外部启动的进程，页面明确提示不能由 Bifrost 停止。
  - WebUI 默认不传端口；状态查询、转写和停止都解析当前 Bifrost 托管进程的动态端口，停止后页面显示“动态端口，启动时选择”，避免刷新后探测旧固定端口。
  - `POST /api/asr/transcribe-stream` 接收 WebUI multipart 音频上传，先用 FFmpeg 转为 16kHz mono WAV，再由 Bifrost 按窗口切片调用本地 qwen3_asr_rs OpenAI-compatible `/v1/audio/transcriptions`，并以 SSE 输出 `progress`、`partial`、`final`、`text`、`error`、`done` 事件。`text` 是整次请求结束后的稳定文本汇总，`partial`/`final` 是真正按窗口产生的增量事件。
- Bifrost CLI 新增 `bifrost ai asr`：
  - `bifrost ai asr start` 在启动前执行平台、模型/runtime 和 `ffmpeg` 自检；缺模型或 runtime 时直接使用 Rust 通用下载模块断点续传补齐，缺 `ffmpeg` 时自动尝试 Homebrew 安装。自检通过后从固定 `~/.bifrost/asr/qwen3_asr_rs/asr-server` 启动模型服务，动态选择 loopback 端口，把 `pid/host/port/model/language/home/managed_by` 写入 `BIFROST_DATA_DIR/asr/service.json`。
  - `bifrost ai asr stop` 读取同一个 service state，停止对应 pid 并删除状态文件。
  - `bifrost ai asr status --json` 输出 CLI 和 WebUI 共享的模型服务状态。
  - `bifrost ai asr stream-file <audio>` 确保模型服务可用后直接调用本地 `asr` 二进制并输出 CLI JSON Lines；如果命令临时启动了模型服务，结束后恢复为停止状态。临时启动前同样执行自检与自动修复。
  - CLI 不依赖仓库脚本，不允许用户指定模型目录；除权重/runtime 下载外，启动、停止、状态和流式输出均由 Bifrost 内置命令编排。
- ASR 目录定时任务：
  - API 新增 `/api/asr/tasks`：创建、列表、详情、删除和手动运行目录任务。任务配置存储在 `BIFROST_DATA_DIR/asr/tasks.json`。
  - 任务绑定一个本机音频目录，默认递归扫描 `wav/mp3/m4a/webm/ogg/flac/aac/opus/mp4/aiff` 等常见音频文件；每次运行只处理尚未成功转写的文件。
  - 每个任务的文件状态存储在 `BIFROST_DATA_DIR/asr/tasks/<task_id>/files.json`，记录源文件路径、大小、mtime、录音创建时间、媒体时长、状态、错误、输出文本路径、timeline 路径和元数据路径。文件 key 使用 canonical path + size + mtime，源目录出现同名新文件时会重新进入 pending，而不是误复用旧 transcript。
  - 录音创建时间按优先级解析：`ffprobe` 容器 tags 的 `date + creation_time` 或 RFC3339 `creation_time`、文件名中的 `YYYYMMDD_HHMMSS`、filesystem birthtime、filesystem mtime。用户真实 `TX02_MIC001_20260514_114433_orig.wav` 样本包含 WAV tags `date=2026-05-14`、`creation_time=11:44:33`，文件名和 filesystem 时间只差 0-1 秒；0 字节坏文件会保留可解析的文件名时间并作为单文件 failed record，不中断整个任务。
  - 转写完成的文本保存到 `BIFROST_DATA_DIR/asr/data/text/<task_id>/<source_hash>.txt`，内容按时间片段渲染为 `[absolute start - absolute end] 文本`；同名 `.timeline.json` 保存结构化 segments，包含 `audio_start_ms/audio_end_ms` 与可选 `absolute_start_ms/absolute_end_ms`；同名 `.json` 保存元数据。即使源音频之后被删除，已完成文本、timeline 和 metadata 仍保留；进度统计会单独展示 `deleted_after_processing`。
  - 调度配置使用显式墙钟周期 `schedule`，不再让用户填写秒级 interval。支持 `hourly`（每小时第几分钟）、`daily`（每天 HH:mm）、`weekly`（ISO 周一到周日 + HH:mm）和 `monthly`（每月第几天 + HH:mm，短月份自动钳制到月末）。创建任务时如果选择的当前分钟已经到达，会立即进入一次 due 状态；执行完成后按下一周期推进，避免同一分钟内反复运行。
  - 定时任务运行时会检查模型服务状态。如果服务已经健康运行，则复用并在结束后保持原状态；如果服务未运行，则在任务独占锁内临时启动，运行完成后停止并清理 service state，避免模型长期占用资源。
  - 任务运行使用进程内全局 ASR job lock 和每任务 `run.lock` 文件，避免多个定时任务同时竞争模型服务 start/stop。任务并发进入时会记录明确错误而不是互相覆盖状态。
  - WebUI ASR 页面新增 Directory Tasks 区域，支持创建 hourly/daily/weekly/monthly 周期任务、手动 Run、删除任务、查看 processed/pending/failed/deleted-after-processing 总体进度和下一次运行时间。点击任务详情会打开 Drawer，展示任务 schedule、last/next run、总体进展和每个音频文件的状态、录音时间来源、媒体时长、错误、文本输出路径、metadata 路径和 timeline 路径；详情打开后会自动加载第一个成功文件的 File Timeline 阅读区，成功文件在文件名旁也提供 Open timeline 入口，避免在宽表格最右侧横向寻找。File Timeline 顶部保留文件元信息，左侧按音频相对时间和绝对时钟时间展示分段文本，右侧展示完整合并 transcript，方便人工快速检查识别质量。页面每 10 秒刷新一次任务状态。
- 流式转写架构：
  - 默认窗口为 1000ms，默认 overlap 为 300ms；API query 可传 `window_ms` / `overlap_ms` 调整，窗口下限 300ms，overlap 最大为窗口的一半。
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
  - `https://huggingface.co/Qwen/Qwen3-ASR-1.7B`
- Rust 依赖使用 bifrost-admin 现有 `reqwest`、`tokio-stream`、`http-body-util` 和 `url`。
- 共享 ASR service state 使用 `bifrost-admin::asr_runtime`，CLI 和 Admin API 共享同一 JSON schema。
- WebUI 使用现有 React、Ant Design 与 `@ant-design/icons`，不新增 Node package。
- 本机既有 `/Users/eden/ai/asr` 下载内容已同步到 `/Users/eden/.bifrost/asr`，后续验证直接复用固定目录。

## 测试方案

### 单元测试

- `bash -n e2e-tests/tests/test_qwen3_asr_local_server.sh`
- `cargo test -p bifrost-admin resource_download` 覆盖 Range 续传头、续传总量合并和下载百分比边界。
- `cargo test -p bifrost-admin asr` 覆盖本地 host/port 校验、Rust 初始化资源任务、流式切片、overlap、能量边界、尾段 flush、去重/连续性、WAV 解析和 query 参数边界。
- `cargo test -p bifrost-admin asr_jobs --lib` 覆盖递归音频发现、输出目录、源文件删除后仍保留已处理元数据。
- `cargo test -p bifrost-cli asr --lib` 覆盖 CLI 读取共享 ASR service state。
- `cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands` 覆盖 `bifrost ai asr` 子命令解析。
- WebUI 类型检查或构建覆盖 AI -> Tools -> ASR 初始化面板和音频输入区编译。

### E2E 测试

- 新增 `e2e-tests/tests/test_qwen3_asr_local_server.sh`。
- 默认做离线结构验证：脚本语法、帮助输出、缺参失败、CI 模型运行时 guard、preflight 可执行；CI shard 缺少 `ffmpeg` 时验证依赖错误可读后跳过在线段。
- CI 环境无条件跳过在线模型段，即使误设置 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 也不会下载权重、安装 runtime、启动 `asr-server` 或部署 Bifrost 托管 ASR 服务。
- 当 `BIFROST_QWEN3_ASR_E2E_ONLINE=1` 时执行真实部署验证：
  - 安装 Qwen3-ASR-1.7B；
  - 运行中文 sample CLI 转写；
  - 通过 Bifrost Admin 启动托管本地 API server；
  - 验证 `/health`、`/v1/models`；
  - 调用 `/v1/audio/transcriptions` 验证中文转写文本包含 `Qwen3`。
- 扩展 E2E 覆盖 Bifrost Admin `/api/asr`：
  - 调用 `/api/asr/init-stream` 验证资产已安装时不会重新下载，事件包含 `installed`/`done`；
  - 调用 `/api/asr/service/start` 验证 Bifrost 托管服务启动后 `/api/asr/status` 返回 ready；
  - 通过 `/api/asr/transcribe-stream` 上传中文样例，验证 SSE 中至少包含多个 `partial` 和多个 `final` 事件，并包含最终中文文本；
  - 调用 `bifrost ai asr stream-file`，验证 CLI 侧输出 partial/final JSON Lines；
  - 通过 FFmpeg 把中文样例转换为 WebM，模拟浏览器麦克风产物，验证后台 `preprocess` 事件、WAV 归一化和最终中文文本；
  - 通过 `/api/asr/transcribe-ws` 发起真实 WebSocket 握手，发送 `start` 控制帧、切成多个 binary frame 的 WebM 音频帧和 `finish` 控制帧，验证顶层 `type` 直接包含 `connected`、`stream`、`partial`、`final`、`text`、`done` 事件及中文文本，且事件 detail 包含递增的 `processed_ms`，避免实时阶段事件被统一折叠为 `progress` 或后续 WebM chunk 被当作独立文件解析失败；
  - 调用 `/api/asr/service/stop` 验证托管服务停止后状态变为 not ready；
  - 端口错误时验证 AI -> Tools -> ASR 可展示的错误事件。
  - 验证 `bifrost ai asr --help`、`bifrost ai asr stream-file /missing.wav` 错误路径、`status --json` 共享状态输出。
  - 验证 `/api/asr/tasks` 在临时 `BIFROST_DATA_DIR` 下可以创建目录任务、列表展示 pending/processed 统计、手动 run 在模型不可用时返回明确错误且不会删除已保存文本元数据。
  - E2E 固定使用 `~/.bifrost/asr`，不再创建或传入临时模型 home。

### 真实场景测试

- 新增 `human_tests/qwen3-asr-local-server.md`。
- 覆盖：
  - Apple Silicon 与 32GB 内存检查；
  - 依赖安装；
  - Qwen3-ASR-1.7B 非交互安装；
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
  - Directory Tasks：在 WebUI 创建绑定目录的递归任务，验证 daily/weekly/monthly 周期选择不会退化成秒级 interval；查看总体进度，点击任务进入详情查看逐文件状态和输出路径，手动运行，验证处理过的文本保存在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/`，删除源音频后文本和元数据仍可保留在进度统计中。
  - 回归验证真实 MediaRecorder 多 chunk：录制 5-8 秒期间 Stop Mic 前必须出现 connected/stream 和 partial/final 事件，后端事件 detail 应包含递增的 `processed_ms`，不得再出现“后续 WebM chunk 不能被单独 ffmpeg -i 解析”的错误。
- 更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：1.7B、Apple Silicon、MLX/Metal、本地 API server、中文样例、长音频切片、AI -> Tools -> ASR 初始化状态/进度/错误、Start/Stop 托管服务、WebUI 文件与麦克风输入、麦克风实时电平音轨、1 秒窗口 partial/final 真流式输出、overlap/能量边界/去重/尾段 flush。
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
