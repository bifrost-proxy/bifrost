# ASR 双模型本地服务与默认模型配置方案

> 实施状态(截至 2026-06-16):本文档描述的多 provider 注册表、`/api/asr/config`、Cohere provider、CDP 授权下载、`service.json` schema v2、`AsrModelProfile` 等能力**均为 planned, not yet shipped as of 2026-06-16**。当前仓库只实现了 Qwen3-ASR 单 provider 链路(参见下文「当前实现基线」);除非显式标注「已落地」,本文其余章节都属于尚未实现的设计稿。

## 背景

ASR 能力当前只支持 `Qwen3-ASR-0.6B` / `Qwen3-ASR-1.7B` 两个模型（同一 provider），WebUI、Directory Tasks、CLI 均通过同一 `qwen3_asr_rs` runtime 启动 `asr-server`。用户希望：

- 在 Qwen3 之外可以并行下载/初始化 `CohereLabs/cohere-transcribe-03-2026`（`cohere_transcribe_rs` runtime）。
- WebUI 可视化切换当前使用的模型，任务与 CLI 通过默认模型配置或显式参数选择。
- 同一时刻只运行一个 `asr-server` 进程，避免多模型抢占 GPU/统一内存。
- 保持既有文件上传、麦克风 WebSocket、Directory Task、CLI 流式转写、timeline/文本落盘路径不变；新模型不做旁路产品，走同一套业务流。

## 用户目标验证清单

### 必须实现

- WebUI 可以切换 ASR 模型，并分别展示每个模型的安装、初始化、下载、启动和错误状态。
- Qwen3 与 Cohere 两类模型可以单独下载和初始化；初始化不启动常驻模型服务。
- 同时只能运行一个模型服务，切换模型启动时必须先停止或替换当前托管服务，避免两个大模型同时占用内存和 Metal/GPU 资源。
- 服务端存在默认 ASR 模型配置；WebUI 转写、Directory Task 与 `bifrost ai asr stream-file` 在未显式指定模型时都使用该默认模型。
- 保留当前文件上传、麦克风 WebSocket、Directory Task、CLI 流式输出、timeline 和文本落盘路径，不把 Cohere 做成另一套旁路产品。
- 新增 `AsrModelRegistry` / `AsrModelProfile` / Provider Adapter 抽象；业务层不出现 `if provider == "cohere"` 的分支。
- Cohere gated 权重通过 CDP 授权下载流程处理；Hugging Face token/cookie 不落盘、不出日志。
- `AsrServiceState` schema v2 引入 `provider` / `model_id` / `runtime` / `display_model` 字段，向后兼容旧 `service.json`。

### 必须不破坏

- 现有 Qwen3-ASR 端到端能力：WebUI 上传/麦克风、Directory Task、CLI `stream-file/start/status/stop`、timeline/text 落盘 schema 全部保持工作。
- 已保存的 Directory Task `model` 字段（例如 `Qwen3-ASR-1.7B`）可读，服务端归一化为 canonical `model_id` 后继续运行。
- 现有 CLI 输出格式对下游脚本兼容；schema 迁移只新增字段，不改字段语义。
- 现有 `~/.bifrost/asr/` 目录布局兼容；`qwen3_asr_rs/` 位置不变，新增 `cohere_transcribe_rs/` 平级目录。
- `test_qwen3_asr_local_server.sh` 与相关 human_tests 继续通过。

### 必须真实验证

- WebUI 切换 Qwen3 / Cohere 模型，服务真实重启（同 model 复用）。
- Directory Task 创建时未显式指定模型则写入服务端默认模型。
- `bifrost ai asr stream-file` 无 `--model` 时使用默认模型。
- Cohere gated 权重未授权时 UI 明确提示登录/接受协议；授权成功后下载继续。
- 单服务互斥：启动 Qwen3 后启动 Cohere，旧 Qwen3 被停止，`service.json` 只记录 Cohere。
- 两轮 Review/Fix/Test。

## 产品语义

### V1 模型清单

## 功能模块说明

本方案在当前 Qwen3-ASR 本地服务实现基础上，扩展为可同时管理多个本地 ASR 模型资产、但同一时刻只运行一个模型服务的统一 ASR 能力。V1 目标模型为：

- `Qwen3-ASR-1.7B` / `Qwen3-ASR-0.6B`：沿用当前 `qwen3_asr_rs` + MLX/Metal 本地 runtime。
- `cohere-transcribe-03-2026`：新增 `second-state/cohere_transcribe_rs` + MLX/Metal runtime，使用 `CohereLabs/cohere-transcribe-03-2026` 官方权重。

核心用户目标：

- WebUI 可以切换 ASR 模型，并分别展示每个模型的安装、初始化、下载、启动和错误状态。
- 两类模型可以单独下载和初始化；初始化不启动常驻模型服务。
- 同时只能运行一个模型服务，切换模型启动时必须先停止或替换当前托管服务，避免两个大模型同时占用内存和 Metal/GPU 资源。
- 增加默认 ASR 模型配置。WebUI 转写、目录定时任务和 `bifrost ai asr stream-file` 在未显式指定模型时都使用同一个默认模型。
- 保留当前文件上传、麦克风 WebSocket、目录任务、CLI 流式输出、时间线和文本落盘路径，不把 Cohere 做成另一套旁路产品。

本方案只描述设计，不包含代码实现。

## 当前实现基线

当前分支已经具备一套 Qwen3-ASR 专用链路：

- `crates/bifrost-admin/src/asr_runtime.rs`(re-exports from `crates/bifrost-asr/src/runtime.rs` since refactor)
  - `DEFAULT_ASR_MODEL = "Qwen3-ASR-0.6B"`、`DEFAULT_ASR_HOST = "127.0.0.1"`、`DEFAULT_ASR_LANGUAGE = "chinese"`、`ASR_INSTALL_NAME = "qwen3_asr_rs"`。
  - 固定 ASR home 为 `~/.bifrost/asr`(`fixed_asr_home()`);`service.json` 路径为 `<bifrost_storage::data_dir()>/asr/service.json`,在默认配置下解析为 `~/.bifrost/asr/service.json`。
  - `AsrServiceState` 记录 `host/port/model/language/home/pid/managed_by/owner_module/owner_id/started_at_ms`(`owner_module`、`owner_id` 为后加的 lease 字段,旧 state 通过 `legacy_owner_module` 回填)。
- `crates/bifrost-admin/src/handlers/asr.rs`
  - `/api/asr/status`
  - `/api/asr/init-stream`
  - `/api/asr/service/start`
  - `/api/asr/service/stop`
  - `/api/asr/transcribe-stream`
  - `/api/asr/transcribe-ws`
  - 当前 `AsrTarget` 包含 `host/port/language/model/home/owner_module/owner_id`,模型目录和必需文件仍由模型名直接推导;`owner_module` 默认 `model_management`,与 `service.json` 的 lease 归属字段联动。
  - 当前初始化固定下载 `second-state/qwen3_asr_rs` release、Qwen Hugging Face 权重和 sample 音频。
  - 当前启动固定执行 `~/.bifrost/asr/qwen3_asr_rs/asr-server --model-dir ... --language ...`。
- `crates/bifrost-cli/src/commands/asr.rs`
  - `bifrost ai asr start/status/stop/stream-file` 复用同一个 `service.json`。
  - `stream-file` 如果服务未运行会临时启动，用完停止。
  - CLI 当前也固定寻找 `qwen3_asr_rs/asr` 和 `qwen3_asr_rs/asr-server`。
- `web/src/api/asr.ts`
  - `defaultAsrParams()` 默认 `Qwen3-ASR-0.6B` 和 `chinese`(`DEFAULT_ASR_MODEL` 常量同步为 0.6B)。
  - WebUI 本地存储用户选择，但没有服务端默认模型配置。
- `web/src/pages/Settings/tabs/SpeechTab.tsx`
  - `Speech Converter` 当前只展示 Qwen 两个模型选项。
  - 初始化、Start Service、Stop Service 都围绕 Qwen 文案和状态。
- `crates/bifrost-admin/src/handlers/asr_jobs.rs`
  - 目录任务持久化 `model` 和 `language`，执行时调用统一 ASR streaming 窗口和本地模型服务。

因此 V1 不应该新增一个独立 Cohere 页面或独立 CLI 命令，而应该把当前 Qwen 专用结构抽象成模型 profile。

## 外部依赖事实

### Cohere Transcribe

`CohereLabs/cohere-transcribe-03-2026` 是 2B 参数 ASR 模型，架构为 Conformer-based encoder-decoder，输入音频会转成 log-Mel spectrogram，输出文本，支持包含英文和中文普通话在内的 14 种语言，许可证为 Apache-2.0。模型仓库是 gated，需要用户登录 Hugging Face 并接受条件后访问文件。

模型限制对产品设计有直接影响：

- 最适合单一、预先指定语言。
- 没有显式自动语言检测。
- code-switch 音频表现不稳定。
- 不提供 timestamp 或 speaker diarization。
- 对非语音声音可能产生幻觉，前置 noise gate 或 VAD 有必要。

### cohere_transcribe_rs

`second-state/cohere_transcribe_rs` 已提供 Rust CLI 和 OpenAI-compatible API server：

- release asset 包含 macOS Apple Silicon 版本 `transcribe-macos-aarch64.zip`。
- macOS Apple Silicon 源码构建使用 `cargo build --release --no-default-features --features mlx`。
- server 暴露 `GET /health` 和 `POST /v1/audio/transcriptions`。
- multipart 字段兼容 OpenAI Whisper API；`prompt` 和 `temperature` 会被接受但忽略。
- 音频会自动转 16kHz mono。
- 超过约 35 秒的文件会自动切成带 overlap 的 chunks。
- release 中带预生成 `vocab.json`；源码构建时可用 `tools/extract_vocab.py` 从 SentencePiece tokenizer 生成。

这意味着 Cohere V1 可以复用当前 Bifrost 的上层流式窗口、目录任务和 OpenAI-compatible transcription 调用，只需要新增模型 profile、资产初始化和启动参数适配。

参考来源：

- [CohereLabs/cohere-transcribe-03-2026](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026)
- [second-state/cohere_transcribe_rs README](https://github.com/second-state/cohere_transcribe_rs)

## 目标架构

### 分层原则

本方案采用“多 Provider + Adapter + 统一业务能力”的架构，而不是在业务层为每个模型写分支。分层边界如下：

```text
业务能力层
  WebUI Speech Converter
  ASR 页面文件/麦克风转写
  Directory Tasks
  CLI stream-file/start/status/stop
  Timeline/Text output
        |
        v
统一 ASR 应用服务层
  AsrModelRegistry
  AsrTargetResolver
  AsrServiceManager
  AsrStreamingPipeline
  AsrTaskRunner
  AsrConfigStore
        |
        v
Provider Adapter 层
  Qwen3AsrProvider
  CohereTranscribeProvider
  future providers...
        |
        v
底层模型服务/runtime
  qwen3_asr_rs asr/asr-server
  cohere_transcribe_rs transcribe/transcribe-server
```

业务能力层必须保持一致：

- WebUI 不直接知道某个 runtime 的 binary 名称、模型文件列表或下载细节。
- 目录任务不关心底层是 Qwen3 还是 Cohere，只记录 `model_id/language` 并调用统一 `AsrTaskRunner`。
- CLI 不直接拼 `asr`、`transcribe`、`asr-server`、`transcribe-server` 路径，只通过统一 ASR 服务层启动、停止和转写。
- 文件上传、麦克风 WebSocket、SSE stream events、timeline 和文本落盘 schema 保持统一。
- 模型差异只允许出现在 Provider Adapter 层或更低层，不能渗透到顶层业务流程。

Provider Adapter 负责吸收差异：

- 模型安装源、gated 授权、下载文件清单和断点续传。
- runtime binary、server binary、启动参数、健康检查兼容性。
- 模型目录布局、必需文件、tokenizer/vocab 准备。
- 语言码归一化，例如 Qwen3 的 `chinese/english/auto` 与 Cohere 的 `zh/en/...`。
- prompt/context 能力差异，例如 Qwen3 可作为强 prompt/context 候选，Cohere 的 `prompt` 只能走后处理。
- 输出文本规范化、空文本/噪声处理、可选 VAD/热词后处理。

统一 ASR 应用服务层暴露稳定能力：

```text
list_models()
get_config()
set_default_model(model_id, language?)
get_status(model_id?)
initialize_model(model_id)
start_service(model_id, language?)
stop_service()
transcribe_stream(model_id?, language?, audio, options)
transcribe_ws(model_id?, language?, audio_chunks, options)
run_directory_task(task_id)
stream_file_cli(audio, model_id?, language?)
```

因此后续继续增加新的 ASR 模型时，只应新增 Provider Adapter 和 registry profile；顶层 WebUI、CLI、目录任务和流式业务流程不应复制一套新实现。

### 模型注册表

新增服务端 ASR 模型注册表，所有入口只传 `model_id`，不直接推导 runtime：

```text
AsrModelProfile
  id: "qwen3-asr-1.7b"
  display_name: "Qwen3-ASR-1.7B"
  provider: "qwen3"
  runtime: "qwen3_asr_rs"
  default_language: "chinese"
  supported_languages: ["chinese", "english", "auto"]
  install_profile: Qwen3

AsrModelProfile
  id: "cohere-transcribe-03-2026"
  display_name: "Cohere Transcribe 03-2026"
  provider: "cohere"
  runtime: "cohere_transcribe_rs"
  default_language: "zh"
  supported_languages: ["zh", "en", "fr", "de", "es", "it", "pt", "nl", "pl", "el", "ar", "ja", "vi", "ko"]
  install_profile: Cohere
```

兼容策略：

- API query 和已有任务中的旧值 `Qwen3-ASR-1.7B` / `Qwen3-ASR-0.6B` 继续接受，并归一化到 profile id。
- WebUI 展示 `display_name`，服务端存储 canonical `model_id`。
- `AsrServiceState` 增加 `provider`、`model_id`、`display_model`、`runtime`。读取旧 `service.json` 时，如果没有这些字段，按 Qwen3 旧模型名补齐。

### 存储布局

V1 保持 `~/.bifrost/asr` 为唯一 ASR home，但把 runtime 和模型资产逻辑分层：

```text
~/.bifrost/asr/
  config.json
  qwen3_asr_rs/
    asr
    asr-server
    tokenizers/
    sample3.wav
  cohere_transcribe_rs/
    transcribe
    transcribe-server
    vocab.json
  models/
    qwen3/
      Qwen3-ASR-1.7B/
      Qwen3-ASR-0.6B/
    cohere/
      cohere-transcribe-03-2026/
        config.json
        model.safetensors
        tokenizer_config.json
        vocab.json
```

兼容现状：

- Qwen3 当前模型路径 `~/.bifrost/asr/qwen3_asr_rs/<model>` 可以继续作为 legacy 路径读取。
- 新实现的路径 helper 应优先返回新 `models/qwen3/<model>`，但如果 legacy 路径已完整安装，可直接复用，避免用户重新下载。
- Cohere 使用新路径，不污染 Qwen3 runtime 目录。

### 默认模型配置

新增 `~/.bifrost/asr/config.json`：

```json
{
  "version": 1,
  "default_model_id": "qwen3-asr-1.7b",
  "default_language_by_model": {
    "qwen3-asr-1.7b": "chinese",
    "qwen3-asr-0.6b": "chinese",
    "cohere-transcribe-03-2026": "zh"
  }
}
```

配置读写要求：

- `GET /api/asr/config` 返回模型 registry、默认模型、每个模型默认语言、当前运行服务。
- `PATCH /api/asr/config` 只允许设置已注册模型为默认模型，并校验默认语言属于该模型支持列表。
- 没有配置文件时默认 `qwen3-asr-0.6b`(对齐当前 `DEFAULT_ASR_MODEL = Qwen3-ASR-0.6B`),保持当前行为。
- CLI、目录任务创建和 WebUI 都从服务端 config 读默认值，不再各自硬编码。

默认模型使用规则：

- WebUI Speech Converter 进入页面时读取服务端默认模型；用户切换模型但不保存默认时，只影响当前操作。
- 用户点击 `Set as default` 后写入 `config.json`。
- ASR 页面文件转写和麦克风转写默认使用 `config.json.default_model_id`。
- 创建目录任务时，如果用户没有选择模型，任务记录当时的默认模型；之后默认模型变化不改变已创建任务。
- `bifrost ai asr stream-file <audio>` 未传 `--model` 时读取默认模型；传了 `--model` 时只覆盖本次执行。

## WebUI 方案

### Settings -> AI -> Speech Converter

页面从单模型状态板升级为模型选择 + 模型状态板：

- Model Select 使用 provider 分组：
  - Qwen3
    - Qwen3-ASR-1.7B
    - Qwen3-ASR-0.6B
  - Cohere
    - Cohere Transcribe 03-2026
- 每个模型展示：
  - installed / missing / initializing / ready / error
  - provider
  - runtime
  - model dir
  - supported languages
  - external sources
  - gated model requirement
- 操作按钮：
  - `Initialize Selected Model`
  - `Start Service`
  - `Stop Service`
  - `Set as Default`
  - `Refresh`

交互规则：

- 初始化只初始化当前选中模型，不启动常驻服务。
- Start Service 启动当前选中模型。如果已有不同模型由 Bifrost 托管运行，则页面弹出确认：停止当前模型并启动新模型。
- Stop Service 只能停止 Bifrost 托管服务；外部进程只展示不可停止提示。
- 当 selected model 与 default model 不同时，页面用明确标签展示 `Default` / `Selected` / `Running` 三个状态，避免用户误判。
- Cohere 的 `prompt` 字段不能作为模型 prompt 宣传；如果 UI 后续支持热词，必须标为 post-correction。

### ASR 工具页

ASR 页面继续消费当前统一 `AsrConnectionParams`，但模型选择来源改为服务端 registry：

- 默认使用服务端默认模型。
- 页面允许临时选择模型和语言，用于本次文件或麦克风转写。
- 若选择的模型未安装，提示去 Speech Converter 初始化该模型。
- 若选择的模型不是当前运行服务，且当前服务由 Bifrost 托管，允许一键切换运行服务。
- 文件转写、麦克风 WebSocket 和 stream events 不新增 Cohere 专属分支，只通过统一 `model_id/language` query 进入后端。

### 主题与可用性

新增或修改的 WebUI 必须保持亮色和暗色主题一致：

- 状态标签、下载进度、错误详情、默认模型标识使用现有主题变量或 Ant Design token。
- 不引入硬编码主题色。
- 模型列表在窄屏下不得依赖宽表格横向滚动才能找到关键操作。

## Admin API 方案

### 新增/调整接口

```text
GET   /api/asr/models
GET   /api/asr/config
PATCH /api/asr/config
GET   /api/asr/status?model_id=...
GET   /api/asr/init-stream?model_id=...
POST  /api/asr/service/start?model_id=...
POST  /api/asr/service/stop?model_id=...
POST  /api/asr/transcribe-stream?model_id=...
GET   /api/asr/transcribe-ws?model_id=...
```

兼容：

- `model=Qwen3-ASR-1.7B` 继续支持。
- 新入口优先使用 `model_id`；如果同时传 `model_id` 和 `model`，以 `model_id` 为准。
- 未传模型时使用 `config.json.default_model_id`。

### 初始化任务隔离

当前 `ASR_INIT_TASK` 是全局单任务缓存。双模型后必须改为按 logical target 隔离：

```text
AsrInitTaskKey
  provider
  model_id
  language_for_verify
  home
```

要求：

- Qwen3 初始化和 Cohere 初始化可以分别订阅自己的 SSE history。
- 同一个模型重复打开页面时复用该模型的后台初始化任务和历史事件。
- 不同模型不能互相复用初始化任务；否则会出现 Cohere 页面看到 Qwen3 下载进度，或一个模型失败污染另一个模型状态。
- 下载器可以并行下载不同模型资产，但启动 verify 时仍应避免并行拉起多个模型 server；V1 可选择初始化任务串行化，优先保证资源稳定。

### 状态返回

`/api/asr/status` 应返回 selected model 的状态，同时带全局 running service：

```json
{
  "status": "installed",
  "ready": false,
  "installed": true,
  "managed": false,
  "selected_model_id": "cohere-transcribe-03-2026",
  "selected_provider": "cohere",
  "default_model_id": "qwen3-asr-1.7b",
  "running_model_id": "qwen3-asr-1.7b",
  "server_url": "dynamic port (managed by Bifrost)",
  "install_dir": "~/.bifrost/asr/cohere_transcribe_rs",
  "model_dir": "~/.bifrost/asr/models/cohere/cohere-transcribe-03-2026",
  "message": "Cohere Transcribe files are installed, but the model service is stopped."
}
```

### 单服务互斥

服务启动逻辑统一收敛到 `start_managed_service(target)`：

- 如果同一 `model_id/language` 已健康运行，直接复用。
- 如果不同模型正在由 Bifrost 托管运行：
  - WebUI 显示确认后调用 start；
  - 后端 start 在确认参数存在时先 stop 旧服务，再启动新服务。
- 如果不同模型是外部进程或非 Bifrost 托管服务，后端不能 kill，只返回冲突错误和可操作提示。
- `service.json` 始终只记录一个当前服务。
- 目录任务继续使用全局 ASR job lock；如果服务由任务临时启动，任务结束后恢复原状态。

### 统一 transcription contract

无论底层是 Qwen3、Cohere 还是后续其它 provider，Bifrost 上层只依赖统一 ASR 应用服务层和统一 transcription contract：

- `GET /health` 返回成功。
- `POST /v1/audio/transcriptions` 支持 multipart `file/language/response_format=text`。
- 返回文本可被 `normalize_asr_text` 和当前 suffix/prefix 去重逻辑处理。

如果某个底层 runtime 不是 OpenAI-compatible，仍不能把差异暴露给业务层；该 provider adapter 必须在本地封装成同等 contract，或者在统一服务层内转换为同等的 `AsrTranscriptResult`。

Provider adapter 需要覆盖以下能力：

```text
AsrProviderAdapter
  id()
  display_name()
  capabilities()
  runtime_binary()
  server_binary()
  install_dir()
  model_dir()
  required_model_files()
  download_requests()
  authorize_downloads()
  prepare_model()
  verify_install()
  health_check()
  start_args(host, port, model_dir, language)
  cli_transcribe_args(model_dir, audio, language)
  normalize_language(input)
  build_transcription_request(audio, language, options)
  parse_transcription_response(response)
  normalize_transcript(text)
  postprocess_transcript(text, hotwords?)
```

统一业务层只处理 `AsrTranscriptResult`：

```text
AsrTranscriptResult
  text
  language
  provider
  model_id
  segments?
  warnings?
```

## Cohere 初始化方案

### 依赖与预检门禁

根据 Hugging Face 模型卡、模型文件树和 `second-state/cohere_transcribe_rs` README/源码，Cohere provider 初始化前必须显式检查以下依赖，避免实现阶段出现模糊失败：

#### 重要纠偏：不能按 Hugging Face Python 栈实施

Qwen3-ASR 和 Cohere Transcribe 在产品集成上应被视为同一类本地 ASR provider：都走 second-state 的 Rust CLI + OpenAI-compatible API server + safetensors 权重 + Apple Silicon MLX/Metal 后端。Hugging Face 模型卡里的 Transformers、vLLM、Docker、Gradio 或 demo 依赖只能作为模型信息参考，不能成为 Bifrost V1 的运行方案或预检要求。

V1 的依赖基线应与当前 Qwen3-ASR 本地服务保持一致：

- 同一平台：macOS Apple Silicon。
- 同一加速路线：MLX/Metal。
- 同一服务形态：本地 OpenAI-compatible `/v1/audio/transcriptions` + `/health`。
- 同一下载/初始化模式：Bifrost 管理 runtime release、模型权重、tokenizer/vocab 资产。
- 同一业务入口：WebUI、CLI、目录任务、文件/麦克风流式都通过统一 ASR 应用服务层调用 provider。

因此实现时禁止因为 Cohere 的 Hugging Face 模型卡写了 `transformers>=5.4.0`、`torch`、`vllm[audio]` 或 `trust_remote_code=True`，就在 Bifrost 产品路径里引入这些 Python 依赖。它们只属于“非 V1 备选路线说明”。

#### 必需运行环境

- 操作系统：V1 只支持 macOS Apple Silicon。
- 芯片：Apple M-series。
- 系统版本：macOS 14+；`cohere_transcribe_rs` 的 MLX backend 面向 macOS Apple Silicon。
- 内存：至少 8GB；模型权重运行时约展开到 5.6GB。Bifrost 产品建议继续沿用 Qwen3 的分级提示：16GB 以下失败，16GB 到 31GB 风险提示，32GB 以上推荐。
- 网络：需要访问 GitHub release 和 Hugging Face gated model 文件；模型大文件通过 Hugging Face Xet/LFS 分发，下载器必须支持大文件断点续传和失败重试。
- 浏览器授权：如果没有 HF token，需要本机 Edge 或 Chrome，且能以独立 profile 启动 CDP。

#### 预编译 release 路线依赖

V1 优先使用 `second-state/cohere_transcribe_rs` 预编译 release，避免用户本机编译 MLX runtime：

- release asset：macOS Apple Silicon 使用 `transcribe-macos-aarch64.zip`。
- release 内必须包含：
  - `transcribe`
  - `transcribe-server`
  - `vocab.json`
  - macOS MLX runtime 所需的同目录资源，例如 `mlx.metallib`。
- 不要求用户安装 Rust、CMake、Xcode CLI tools 或 Python runtime 来运行 release。
- 不需要 `LD_LIBRARY_PATH` 或 `DYLD_LIBRARY_PATH`；上游说明二进制会从同目录找到 macOS MLX 资源。

#### 源码构建路线依赖

源码构建只作为开发者/兜底路线，不作为普通 WebUI 初始化默认路径：

- Rust stable 1.70+。
- `cmake`。
- `git submodule update --init --recursive`，因为 MLX backend 会从 `mlx-c` submodule 构建。
- macOS Apple Silicon 构建命令：

```bash
cargo build --release --no-default-features --features mlx
```

Linux/libtorch backend 不是 V1 目标，但如果未来纳入 provider：

- 需要 libtorch C++ library。
- Linux ARM64 还要求 SVE；Apple Silicon Linux VM 只暴露 NEON，不适合该 libtorch ARM64 build。
- Docker on macOS 构建时 libtorch 和 `CARGO_TARGET_DIR` 不能放在 macOS volume mount 上，否则可能遇到链接或 SIGBUS 问题。

#### Hugging Face 模型文件依赖

Hugging Face 模型页显示该仓库是 gated model，文件可列出但未授权不能访问内容。文件树中与 Rust/MLX 路线相关的关键文件包括：

- `config.json`：Rust `ModelConfig::load()` 必需。
- `model.safetensors`：权重文件，约 4.13GB，Xet-backed。
- `tokenizer_config.json`：Rust `SpecialTokens::from_tokenizer_config()` 必需，用于语言 token 和特殊 token。
- `vocab.json`：Rust `Vocab::load()` 必需；可从 release 复制，也可由 `tokenizer.model` 生成。
- `tokenizer.model`：只有在需要现场生成 `vocab.json` 时必需。

模型仓库还包含 Transformers remote-code 相关文件：

- `configuration_cohere_asr.py`
- `modeling_cohere_asr.py`
- `processing_cohere_asr.py`
- `tokenization_cohere_asr.py`
- `preprocessor_config.json`
- `processor_config.json`
- `generation_config.json`
- `tokenizer.json`
- `special_tokens_map.json`

这些文件是 Transformers/vLLM 路线的重要依赖；Rust/MLX release 路线不应直接执行 remote Python code，但下载器和权限探测要允许这些文件存在，不能把它们误判为异常。

#### vocab.json 生成依赖

优先从 `cohere_transcribe_rs` release 复制预生成 `vocab.json` 到模型目录。只有 release 缺失或校验失败时才启用生成路线：

- Python 3。
- `sentencepiece`。
- HF 模型目录内必须有 `tokenizer.model`。
- 运行上游 `tools/extract_vocab.py --model_dir <model_dir>`。

生成后运行时不再需要 Python 或 SentencePiece。

#### 音频处理依赖

`cohere_transcribe_rs` 使用 Rust 依赖处理音频：

- `symphonia`：解码 WAV、FLAC、MP3、AAC、OGG。
- `rubato`：重采样。
- `rustfft`：mel spectrogram FFT。

因此 Cohere runtime 自身不要求系统 `ffmpeg`。但是 Bifrost 当前上层文件/麦克风流式管线已经使用 FFmpeg 做上传音频规范化、WebM 解析和窗口切片，所以产品层仍然需要保留当前 `ffmpeg` 预检和 Homebrew 安装逻辑。不要因为 Cohere runtime 不依赖 ffmpeg 而删除 Bifrost 业务层的 ffmpeg 依赖。

#### 非 V1 路线依赖说明，禁止进入产品预检

Hugging Face 模型卡还列出其它官方/生态路线，但 V1 不采用：

- Transformers 离线推理：需要 `transformers>=5.4.0`、`torch`、`huggingface_hub`、`soundfile`、`librosa`、`sentencepiece`、`protobuf`；长音频/非英文示例还使用 `datasets`；模型加载示例使用 `trust_remote_code=True`。
- vLLM 在线服务：需要 Python 3.12 环境、`vllm==0.19.0`、`vllm[audio]`、`librosa`，并以 `vllm serve ... --trust-remote-code` 启动。
- mlx-community/appautomaton 的 MLX 8-bit 权重：面向 Python `mlx-speech`，不是 `cohere_transcribe_rs` 当前加载格式，V1 不直接使用。
- CoreML/GGUF/ONNX 社区转换：依赖各自 runtime，不纳入 V1。

这些依赖不得出现在 V1 WebUI 初始化门禁、CLI 启动检查、human_tests 前置条件或普通用户安装说明中。若未来新增非 Rust/MLX provider，必须作为新的 Provider Adapter 单独设计，而不是混入 Cohere Rust/MLX provider。

### Runtime 下载

使用 `second-state/cohere_transcribe_rs` release：

- macOS Apple Silicon asset：`transcribe-macos-aarch64.zip`。
- 安装目录：`~/.bifrost/asr/cohere_transcribe_rs`。
- 需要可执行文件：
  - `transcribe`
  - `transcribe-server`
- release 自带 `vocab.json` 时，复制到模型目录。

如后续选择源码构建，则需要 `git submodule update --init --recursive` 和 `cargo build --release --no-default-features --features mlx`；V1 不建议在 Bifrost 初始化流程中源码构建，因为耗时长且对 Xcode/CMake/Rust 版本更敏感。

### 权重下载

模型目录：

```text
~/.bifrost/asr/models/cohere/cohere-transcribe-03-2026/
  config.json
  model.safetensors
  tokenizer_config.json
  vocab.json
```

下载源：

```text
https://huggingface.co/CohereLabs/cohere-transcribe-03-2026
```

关键约束：

- 这是 gated model。初始化失败时必须明确提示用户需要登录 Hugging Face 并接受模型访问条件。
- 当前通用下载器如果无法携带 HF token，需要新增浏览器 CDP 授权下载、HF token 配置输入或引导用户通过本机 `huggingface-cli login` 完成凭据准备。
- 不应把 HF token 写入日志、SSE detail 或 WebUI 错误详情。

### CDP 授权下载模式

技术验证结论：该模式可行。使用独立 Edge profile 通过 CDP 访问 Hugging Face 后，未带浏览器 cookie 下载 Cohere `config.json` 返回 `401`；用户在浏览器中完成 Hugging Face 登录和模型条款接受后，CDP 读取同一 profile 下的 Hugging Face cookie，再请求同一个 `config.json` 返回 `200`，下载内容可解析为模型配置，`model_type=cohere_asr`、`max_audio_clip_s=35`、`vocab_size=16384`。

V1 授权下载流程：

1. 初始化 Cohere 前先请求小文件 `config.json` 做权限探测。
2. 如果未带授权即可返回 `200`，直接走普通下载器。
3. 如果返回 `401/403`，WebUI 显示 `Authorize with browser`。
4. 用户点击后，Bifrost 启动本机浏览器并开启 CDP：

```text
Microsoft Edge 优先，其次 Google Chrome
--remote-debugging-port=<dynamic_loopback_port>
--user-data-dir=~/.bifrost/asr/browser-profiles/huggingface
--no-first-run
--no-default-browser-check
https://huggingface.co/CohereLabs/cohere-transcribe-03-2026
```

5. 用户在浏览器窗口中完成 Hugging Face 登录。
6. Bifrost 通过 CDP `Network.getCookies` 读取 `https://huggingface.co/` 作用域 cookie，只在内存中使用，并持续探测 `config.json` 下载权限。
7. 如果探测状态从 `401` 变为 `403`，说明登录态已生效但模型条款尚未接受。此时 Bifrost 必须通过 CDP `Page.navigate` 自动把浏览器页面切回 `https://huggingface.co/CohereLabs/cohere-transcribe-03-2026`，并在 WebUI/SSE 中显示 `needs_terms`：提示用户在该模型页点击接受协议后才能开始下载。
8. Bifrost 在 `needs_terms` 状态下继续轮询 `config.json`；如果用户点击接受协议后返回 `200` 且内容可解析为模型配置，则标记授权可用。
9. 后续权重下载请求复用同一内存 cookie header，不把 cookie 写入日志、SSE、配置文件或 service state。
10. 下载完成或用户取消后关闭 CDP browser；托管 profile 保留，以便后续再次初始化时复用登录态。

CDP 授权状态机：

```text
probe config.json without auth
  200 -> authorized -> start normal download
  401/403 -> open browser authorization

browser authorization loop
  401 -> needs_login
       -> keep browser open
       ->提示用户登录 Hugging Face
  403 -> needs_terms
       -> CDP Page.navigate(original_model_page)
       ->提示用户点击模型页接受协议
  200 + valid config json -> authorized
       -> close browser if no longer needed
       -> continue model file download
  timeout/cancel -> authorization_failed
```

必须遵守的边界：

- 默认不复用系统 Chrome/Edge 主 profile，避免读取用户主浏览器隐私数据。
- 使用 Bifrost 管理 profile：`~/.bifrost/asr/browser-profiles/huggingface`。
- 只有用户显式点击授权下载时才启动浏览器。
- CDP 端口只绑定 loopback，页面只打开 Hugging Face 模型页。
- 错误状态必须可操作：
  - `browser_missing`：未找到 Edge/Chrome。
  - `browser_launch_failed`：浏览器或 CDP 启动失败。
  - `needs_login`：页面仍未登录或下载仍是 `401`。
  - `needs_terms`：已登录但模型文件仍是 `403`，需要接受模型条款；进入该状态时必须自动导航回原始 Cohere 模型页，避免登录后跳转到首页、settings、OAuth callback 或其它页面导致用户找不到接受入口。
  - `auth_expired`：之前 profile 存在但 cookie 已失效。
  - `download_forbidden`：授权后仍无法访问文件。
- WebUI 应提供 `Clear Hugging Face browser profile`，用于清理托管 profile 和重新授权。

这一路径和 HF token 配置可以并存：

- 如果用户配置了 HF token，优先用 token 下载。
- 如果没有 token 或 token 失败，再提供 CDP browser authorization。
- 如果用户已在托管 profile 中授权过，后续初始化不需要重复登录，除非 cookie 失效或模型条款变化。

### vocab.json

优先策略：

1. release zip 自带 `vocab.json`，初始化时复制到 Cohere 模型目录。
2. 如果 release 未带或用户选择源码模式，再提示需要 Python `sentencepiece` 并运行 `tools/extract_vocab.py`。

V1 推荐只支持策略 1，降低初始化复杂度。

### 安装验证

Cohere 的验证不能复用 Qwen3 中文 sample 断言。需要新增 provider-specific verify：

- assets 完整性：
  - `transcribe` 存在且可执行。
  - `transcribe-server` 存在且可执行。
  - `config.json/model.safetensors/tokenizer_config.json/vocab.json` 存在。
- CLI smoke：
  - 使用 Cohere release 或模型仓库 demo 中可下载的短音频。
  - 中文模型验证用 `--language zh`。
  - 英文模型验证用 `--language en`。
- server smoke：
  - 启动临时 `transcribe-server --model-dir ... --host 127.0.0.1 --port <dynamic> --language zh`。
  - 验证 `/health`。
  - 上传短音频到 `/v1/audio/transcriptions`，`response_format=text`。
  - 停止临时服务。

如果因为 gated 权重或外部网络失败无法验证，初始化状态必须停在 error，不得标记 installed。

## Qwen3 适配方案

Qwen3 作为现有能力，V1 应尽量保持行为不变：

- 现有 `Qwen3-ASR-1.7B` 仍是默认模型。
- 当前 `qwen3_asr_rs` release、Qwen Hugging Face 权重、tokenizer 复制、sample3 中文验证逻辑保留。
- `language` 继续支持 `chinese`、`english`、`auto`。
- 当前 `stream-file` 的 partial/final JSON Lines 输出不改变。

需要改造的只是把硬编码逻辑移入 Qwen3 provider adapter。

## CLI 方案

### 默认模型

`bifrost ai asr stream-file <audio>`：

- 未传 `--model` 时读取 `~/.bifrost/asr/config.json.default_model_id`。
- 未传 `--language` 时读取该模型默认语言。
- 如果默认模型未安装，输出明确错误：

```text
ASR model cohere-transcribe-03-2026 is not initialized. Initialize it from AI > Speech Converter or run bifrost ai asr init.
```

### 命令扩展

V1 可保留现有命令，但参数从 Qwen3-only 变为 registry-aware：

```text
bifrost ai asr status --json
bifrost ai asr start --model cohere-transcribe-03-2026 --language zh
bifrost ai asr stop
bifrost ai asr stream-file audio.wav
bifrost ai asr stream-file audio.wav --model qwen3-asr-1.7b --language chinese
```

后续可加：

```text
bifrost ai asr models
bifrost ai asr config get
bifrost ai asr config set-default cohere-transcribe-03-2026 --language zh
```

### CLI 启动差异

Qwen3:

```text
asr-server --model-dir <qwen_model_dir> --host 127.0.0.1 --port <port> --language <language>
```

Cohere:

```text
transcribe-server --model-dir <cohere_model_dir> --host 127.0.0.1 --port <port> --language <language>
```

CLI 不直接拼可执行文件路径，必须通过 provider adapter 获取。

## 目录定时任务方案

目录任务必须能正确使用默认模型：

- 创建任务时，如果 request 没有传 `model_id`，使用当前默认模型并持久化到任务配置。
- 任务详情显示模型 display name、provider、language。
- 任务运行前解析任务中的 `model_id`：
  - 若缺失或为旧 `model` 字段，按兼容表归一化。
  - 若模型未安装，单个 task run 失败并记录明确 `last_error`。
  - 若模型已安装但服务未运行，在全局 job lock 内临时启动对应模型，结束后恢复原状态。
- 如果多个任务绑定不同模型且同时 due，只允许串行运行；第二个任务等待 lock 或记录可读的排队/冲突状态，不能同时启动两个模型服务。

输出文件和 timeline schema 保持不变，但 metadata 中建议新增：

```json
{
  "provider": "cohere",
  "model_id": "cohere-transcribe-03-2026",
  "model_display_name": "Cohere Transcribe 03-2026",
  "language": "zh"
}
```

## 流式音频方案

当前 Bifrost 的“流式”是在上层把音频规范化为 16kHz mono WAV，再按窗口切片调用本地 OpenAI-compatible transcription endpoint，并发送 `partial/final/text/done`。V1 继续沿用该策略：

- Qwen3 和 Cohere 都不要求模型原生 streaming。
- 文件上传走 `/api/asr/transcribe-stream`。
- 浏览器麦克风走 `/api/asr/transcribe-ws`。
- `window_ms/overlap_ms`、能量边界、suffix/prefix 去重和 EOF flush 保持统一。

Cohere 特别注意：

- Cohere 对 silence / non-speech 可能更容易产生幻觉，应保留当前能量边界，并在 Cohere profile 中允许后续启用更强 VAD。
- Cohere 不支持真正 prompt/context 注入，热词只能作为后处理能力；V1 不把 `prompt` 暴露为“模型提示词”。
- Cohere 语言最好明确指定 `zh` 或 `en`，不把 Qwen3 的 `auto` 透传给 Cohere。

## 数据结构调整

### AsrServiceState

建议 schema v2：

```json
{
  "schema_version": 2,
  "host": "127.0.0.1",
  "port": 51234,
  "provider": "cohere",
  "model_id": "cohere-transcribe-03-2026",
  "model": "Cohere Transcribe 03-2026",
  "language": "zh",
  "home": "~/.bifrost/asr",
  "pid": 12345,
  "managed_by": "admin",
  "started_at_ms": 1760000000000
}
```

兼容旧 schema：

- 旧 `model` 为 `Qwen3-ASR-1.7B` 时补 `provider=qwen3`、`model_id=qwen3-asr-1.7b`。
- 旧 `model` 为 `Qwen3-ASR-0.6B` 时补 `provider=qwen3`、`model_id=qwen3-asr-0.6b`。

### AsrDirectoryTask

新增字段：

```json
{
  "model_id": "qwen3-asr-1.7b",
  "provider": "qwen3"
}
```

保留旧 `model` 字段用于读旧任务；写新任务时写 `model_id` 和 `model_display_name`。

### Frontend AsrConnectionParams

新增：

```ts
interface AsrConnectionParams {
  host?: string;
  port?: number;
  language?: string;
  model?: string;    // legacy
  model_id?: string; // canonical
}
```

`defaultAsrParams()` 不再硬编码，改为来自 `/api/asr/config`；localStorage 只保存最近选择，不充当全局默认配置。

## 安全与边界

- 所有模型服务只允许 loopback host。
- Start Service 继续动态选择端口。
- 初始化下载不得在 Bifrost 启动时自动触发。
- HF token 不得出现在日志、SSE、WebUI、`service.json`、`config.json`。
- Hugging Face cookie 只能在 CDP 授权下载流程内存中使用，不得写入 Bifrost 配置、日志、SSE、任务 metadata 或 service state。
- 单服务互斥是硬约束。任何入口尝试并行启动第二个模型时，都必须走 stop/replace 或返回冲突。
- 不复用系统浏览器登录态来获取 gated 权重；如需浏览器授权，只使用 Bifrost 管理的独立 Chrome/Edge profile，且必须由用户显式触发。
- 不修改系统代理。

## Sync 边界

- 服务端默认模型配置 `~/.bifrost/asr/config.json` 是本机偏好，不参与 Rules/Values sync。
- `service.json` 是本机运行时状态，不参与 sync；每台机器独立记录当前运行的 provider/model_id。
- 模型资产（Qwen3、Cohere 权重、tokenizer/vocab）本机下载，不通过 sync 传播；未来若考虑跨机资产分发，需要独立离线包/CDN 方案，不复用 Rules sync。
- Hugging Face token / CDP cookie 只留在本机 Bifrost 管理的 Chrome/Edge profile 内，不落盘到任何 Bifrost 配置、日志或 SSE。
- Directory Task 内 `model_id` 属于任务字段，未来若做任务云同步需要考虑不同机器模型资产是否可用；本方案不承诺 Directory Task sync。

## 实施阶段拆分

### 阶段 1：模型注册表与默认配置

- 新增 registry/profile。
- 新增 `/api/asr/models`、`/api/asr/config`、`PATCH /api/asr/config`。
- WebUI 显示服务端默认模型。
- CLI 和目录任务创建读取默认模型。
- 收敛统一 ASR 应用服务层接口，确保 WebUI、CLI、目录任务和流式转写只依赖 `model_id/language`，不直接调用 provider runtime。
- 不接 Cohere 下载，只完成 Qwen3 行为迁移和兼容。

### 阶段 2：Provider Adapter 抽象

- 把 Qwen3 的下载、安装、必需文件、启动参数、CLI 参数、verify 迁移到 adapter。
- `AsrTarget` 改为包含 canonical `model_id/provider/runtime`。
- `service.json` 升级到 schema v2 并兼容读取旧格式。
- 把语言码、启动参数、下载授权、输出解析、热词/后处理能力全部放入 adapter，不允许业务层出现 `if provider == "cohere"` 这类模型特例。
- 保证现有 Qwen3 human_tests 全部仍能通过。

### 阶段 3：Cohere 初始化与启动

- 新增 Cohere profile。
- 下载/安装 `cohere_transcribe_rs` release。
- 下载 gated Cohere 权重并处理 `vocab.json`。
- Cohere CLI/server smoke verify。
- WebUI 支持 Cohere 初始化、状态、Start/Stop。

### 阶段 4：流式与任务联调

- 文件转写、麦克风 WebSocket、CLI `stream-file`、目录任务全部支持 `model_id`。
- 不改变上层 stream event contract。
- metadata 和 timeline 补充 provider/model_id。
- 验证新增 Cohere 后 ASR 页面、目录任务、CLI 仍走同一套业务流程，而不是 Cohere 专用 endpoint 或专用任务 runner。

### 阶段 5：体验与热词后处理

- 针对 Cohere 增加可选 VAD 强化。
- 增加热词 post-correction 配置，但明确不是 decoder prompt。
- 根据真实中文、英文、中英混说样本决定默认模型推荐策略。

## 测试方案

### 单元测试

- model registry：
  - 旧模型名归一化到 canonical `model_id`。
  - 不支持的 `model_id` 返回可读错误。
  - Cohere 不接受 `auto` language。
- config：
  - 无 `config.json` 时默认 Qwen3 1.7B。
  - 设置默认模型时必须是已注册模型。
  - 默认语言必须属于该模型支持列表。
- service state：
  - 旧 `service.json` 能读成 Qwen3 provider。
  - v2 state 能区分 running model 与 selected model。
- provider adapter：
  - Qwen3 required files 与现有行为一致。
  - Cohere required files 包含 `config.json/model.safetensors/tokenizer_config.json/vocab.json`。
  - Qwen3/Cohere start args 分别生成正确 binary 和参数。
  - Qwen3/Cohere 语言码归一化、请求构造、响应解析和 transcript normalize 都通过 adapter 完成。
- service layer：
  - WebUI/API/CLI/task runner 只调用统一 ASR 应用服务接口。
  - 业务层不能出现 provider-specific binary path、download URL、model file list 或 response parsing。
  - V1 普通产品路径不能出现 Transformers、vLLM、torch、Gradio、Docker demo 等 Hugging Face 官方 Python 栈依赖检查。
- tasks：
  - 创建任务未传模型时捕获当前默认模型。
  - 旧任务 `model` 字段能迁移。

### E2E 测试

- 离线结构验证：
  - `/api/asr/models` 返回 Qwen3 与 Cohere profile。
  - `/api/asr/config` 默认值正确。
  - `PATCH /api/asr/config` 校验非法模型、非法语言。
  - 未安装 Cohere 时 `/api/asr/status?model_id=cohere-transcribe-03-2026` 返回 missing。
  - CLI `stream-file` 在默认模型未安装时返回明确错误。
- Qwen3 在线回归：
  - 现有 `test_qwen3_asr_local_server.sh` 行为不变。
  - Qwen3 默认模型下 WebUI 文件转写、麦克风、目录任务仍通过。
- Cohere 在线验证：
  - 初始化 Cohere runtime 和权重。
  - 初始化前置检查只要求 second-state Rust/MLX release 路线所需依赖，不要求安装 `transformers`、`torch`、`vllm`、`qwen-asr`、`librosa` 或 Python 3.12。
  - Cohere gated 权重未授权时返回 `needs_login` 或 `needs_terms`。
  - 用户登录后如果下载探测从 `401` 变为 `403`，验证 CDP 自动 `Page.navigate` 回 Cohere 原始模型页，并且 WebUI 明确提示点击接受协议。
  - 使用 CDP browser authorization 完成登录/条款接受后，`config.json` 权限探测从 `401/403` 变为 `200`，随后权重下载继续。
  - 启动 Cohere service。
  - `/health` ready。
  - `/v1/audio/transcriptions` 中文 `zh` 和英文 `en` 样例可返回文本。
  - 文件流式、CLI `stream-file`、目录任务可通过统一 Bifrost ASR 管线调用 Cohere。
  - 代码结构检查确认 Cohere 没有新增独立 WebUI 业务流程、独立目录任务 runner 或独立 CLI stream implementation。
- 单服务互斥：
  - 启动 Qwen3 后启动 Cohere，验证旧 Bifrost-managed Qwen3 被停止，`service.json` 只记录 Cohere。
  - Cohere 运行时请求 Qwen3 文件转写，如果未确认切换，应返回模型不匹配或需要切换的可读错误。

### 真实场景测试

实施时需要新增或更新 `human_tests/asr-multi-model-local-service.md`，覆盖：

- Speech Converter 中 Qwen3/Cohere 模型切换。
- 设置默认模型后刷新页面仍保持。
- 默认模型影响 ASR 页面文件转写。
- 默认模型影响 `bifrost ai asr stream-file`。
- 创建目录任务时未显式选模型会写入当前默认模型。
- Qwen3 和 Cohere 可以分别初始化。
- 同时只能运行一个模型服务；模型切换启动会停止旧托管服务或提示外部服务冲突。
- Cohere gated 权重未授权时错误可读，且不泄露 token。
- Cohere CDP 授权下载：首次打开独立 Edge/Chrome profile，登录 Hugging Face 并接受模型条款后，初始化继续下载；刷新页面或重试初始化时复用托管 profile 登录态。
- Cohere CDP 登录跳转回归：登录完成后即使 Hugging Face 跳转到非模型页，下载探测进入 `403` 时也会自动回到 Cohere 模型页，并提示用户点击接受协议。
- 清理 Hugging Face browser profile 后重新初始化，页面再次要求授权。
- Cohere `zh` / `en` 语言选择可用，`auto` 不可用或有明确提示。
- 亮色和暗色主题下模型状态、默认标签、下载进度、错误详情均可读。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：
  - WebUI 支持模型切换。
  - 两个模型可单独下载初始化。
  - 同时只运行一个模型服务。
  - 有默认模型配置。
  - 定时任务和 CLI 流式音频处理使用默认模型。
  - 本次只落设计文档，不改代码。
- 复核当前实现：
  - `asr_runtime.rs` 默认模型硬编码。
  - `SpeechTab.tsx` 模型列表只有 Qwen。
  - `asr.rs` 初始化和启动逻辑 Qwen-only。
  - `asr_jobs.rs` 任务记录模型但没有服务端默认模型概念。
- 检查方案是否把 Cohere 接入统一管线，而不是旁路服务。
- 执行 `git status --short` 和 `git diff --no-index -- /dev/null design/asr-multi-model-local-service.md`，确保新增未跟踪文件内容也被 review。
- 修复发现的方案缺口。

### 第 2 轮

- 再次复核新增文档 diff。
- 检查方案是否明确：
  - provider adapter 边界。
  - config schema。
  - service state 兼容。
  - 单服务互斥。
  - Cohere gated 权重和 `vocab.json`。
  - Cohere 不支持 prompt/context 的产品表达边界。
  - human_tests、E2E 和单元测试计划。
- 再次执行 `git status --short` 和 `git diff --no-index -- /dev/null design/asr-multi-model-local-service.md`。
- 如仍发现缺口，追加第 3 轮。

## 校验要求

本方案阶段只做文档验证：

- 必须确认只新增独立 `design/` 方案文件，不修改代码。
- 必须确认方案引用的当前实现文件与真实分支一致。
- 不运行 Rust/WebUI/模型 E2E，因为本次没有实现代码，也不应启动或下载模型。

实施阶段必须执行：

- `cargo test -p bifrost-admin asr`
- `cargo test -p bifrost-admin asr_jobs --lib`
- `cargo test -p bifrost-cli asr --lib`
- WebUI typecheck/build。
- Qwen3 现有 E2E。
- Cohere 在线初始化和 server smoke。
- `cargo test --workspace --all-features`。
- `human_tests/asr-multi-model-local-service.md` 逐条真实执行。

## 文档更新要求

实施阶段需要同步更新：

- `design/qwen3-asr-local-server.md`：标注 Qwen3 已成为 ASR provider 之一。
- `human_tests/qwen3-asr-local-server.md`：保留 Qwen3 回归用例。
- `human_tests/asr-multi-model-local-service.md`：新增双模型和默认模型真实场景。
- `human_tests/readme.md`：新增索引。
- WebUI 可见文案如 `Speech Converter` 中的外部依赖说明和模型限制提示。

## 风险与决策点

- Cohere 权重是 gated model，WebUI 初始化如何安全获取 Hugging Face 凭据需要单独评审。
- CDP 授权下载依赖用户完成浏览器登录和模型条款接受；产品上必须把 `401`、`403`、浏览器缺失、CDP 启动失败分成不同可操作状态，并在 `403` 时强制回跳模型页，否则用户可能停在登录后的非模型页面而找不到接受协议入口。
- `cohere_transcribe_rs` release 的 asset 命名和打包内容如果变动，初始化流程需要做版本探测或锁定 release。
- Cohere 与 Qwen3 的语言码不同，所有入口必须使用 provider adapter 归一化，避免把 `chinese` 传给 Cohere 或把 `zh` 传给 Qwen3。
- Cohere 不支持自动语言检测和 prompt/context 注入，中英混说或强热词场景需要继续保留 Qwen3 或后处理 fallback。
- 单服务互斥涉及 Admin API、CLI 和目录任务三条入口，实施时必须用同一个 lock 和同一个 `service.json`，否则容易出现状态漂移。
