# ASR Speech Pipeline Orchestrator 顶层技术方案

## 背景与目标

本方案把 Bifrost 当前分散的实时语音、离线文件转字幕、ASR Directory Task 和唤醒词能力收敛到一套统一的 `Speech Pipeline Orchestrator`。目标不是重写现有 ASR，而是在现有真实代码基础上抽出决策层、资源层和离线字幕主链路，让三条业务入口共享同一套能力选择、资产检查、资源仲裁和 artifact schema。

用户目标验证清单：

- 必须实现：整理出完整、可执行、repo-backed 的顶层技术方案，而不是停留在讨论稿。
- 必须实现：统一实时链路、离线单文件链路、定时目录任务链路的模式、引擎、资源和产物决策。
- 必须实现：明确遗留降级、迁移和下线策略；不为旧 `/api/asr/transcribe-ws`、旧 Directory Task 写法或现有 `/api/asr/transcribe-stream` 维持并行兼容服务。
- 必须实现：把当前最大缺口 `AsrUnitPlanner` 和 `OfflineSubtitlePipeline` 作为第一优先级落地项。
- 必须实现：Directory Task 在离线产物落盘后，仍要继续执行现有输出合并、Daily Docs 刷新、Daily Agent / AI Runner 后处理和 IM/report 同步流程，不能被新 pipeline 吞掉。
- 必须实现：把 ASR 核心能力从 `bifrost-admin` 抽成独立 `crates/bifrost-asr` 包；不同平台通过是否依赖该包、启用哪些 feature 来决定编译方案，避免在 admin 内到处写跨平台 `cfg` / `if else`。
- 必须不破坏：新产品能力需要复用的 Qwen3-ASR、本地 ASR service/runtime strategy、speaker diarization、voiceprint 和字幕产物链路；旧入口支持级别可以降低。
- 必须真实验证：本方案、human_tests 索引和静态验收命令能在当前仓库真实执行。
- 必须交付：设计文档、human_tests 用例、两轮 Review/Fix/Test 记录和最终验证矩阵。

## 当前代码基线

本方案基于 2026-05-28 当前仓库状态：

- 实时语音主能力已经存在于 `crates/bifrost-admin/src/handlers/voice/mod.rs`：
  - `/api/voice/listen-ws` 接收 16kHz mono PCM16。
  - 默认模型是 `Qwen3-ASR-0.6B`。
  - provider 走 `qwen3_stateful_streaming`。
  - `StatefulVoiceSession.feed_pcm16()` 输出 partial、stable delta 和 final utterance。
  - 1.7B stateful streaming 已被默认拦截，需要显式允许。
- 旧 ASR WebSocket 仍存在于 ASR handler：
  - `/api/asr/transcribe-ws` 更像上传式实时预览，会积累 `session_bytes`，flush 时重转完整会话再截取新增窗口。
  - 它不能继续作为真正实时主链路，也不再作为必须维持的产品入口。
- Directory Task 已经具备离线任务基础：
  - `AsrDirectoryTask` 已有 `model`、`language`、`runtime_strategy`、`diarization`。
  - `AsrDiarizationConfig` 已有 `enabled/profile/min_speakers/max_speakers/known_speaker_count/voiceprint_matching`。
  - `normalize_to_temp()` 已经把音频规范成 16kHz mono WAV。
  - diarization assets 缺失时应明确失败，不允许任务运行时偷偷下载。
- Directory Task 还具备 ASR 完成后的后处理链路：
  - 音频处理、failed chunk retry 合并、daily markdown 刷新和 ASR 状态持久化完成后，才允许排队 Daily Agent Runner。
  - `maybe_enqueue_daily_agent_after_asr_run()`、Daily Agent workspace、`daily_agent_processed.json`、report 生成、Git/IM/report sync 都属于 Directory Task 的下游契约。
  - 新 `OfflineSubtitlePipeline` 只能替换“单文件音频 -> 标准 artifacts”的处理阶段，不能截断这些后续 AI 进程。
- speaker-aware 输出基础已存在：
  - `TranscriptTimeline` 和 `TimelineSegment` 支持 `speaker`、`speaker_display_name`、`overlap`、`diarization_profile`、`speakers`。
  - 当前缺口是 diarization 后仍倾向于直接按原始 speaker segment 切 WAV 送 ASR，没有独立 `AsrUnitPlanner` 做合并、拆分、过滤和 debug 元数据保留。
- 现有设计文档已覆盖局部能力：
  - `design/audio-diarization-asr-offline.md` 是离线 diarization/voiceprint 落地文档。
  - `design/asr-realtime-voice-input.md` 是实时 voice input 文档。
  - `design/asr-speech-engine-orchestration.md` 是语音引擎可插拔与声纹方向文档。
  - 本文作为顶层“最新版本”方案，负责把三条链路、旧支持降级策略和落地顺序统一。
- 当前跨平台编译痛点：
  - ASR 能力长期放在 `bifrost-admin` 内，导致 admin 为 Qwen3、sherpa-onnx、voice stateful、diarization、voiceprint 分散维护 target dependency、stub 和 `cfg`。
  - 后续应以 `bifrost-asr` 作为唯一 ASR 能力边界；需要 ASR 的二进制或平台显式依赖它，不需要 ASR 的平台不依赖它，从 Cargo graph 上直接切断 native 依赖。
  - `bifrost-admin` 只保留 HTTP API、Directory Task 状态机、FileStore、Daily Agent 后处理等产品编排，不再直接拥有 ASR engine/native provider 实现。

## 顶层架构

新增统一调度层：

```text
Speech Pipeline Orchestrator
  ├─ SpeechEngineRegistry
  ├─ SpeechPipelineProfile
  ├─ EngineDecisionResolver
  ├─ ResourceLeaseManager
  ├─ RealtimeVoicePipeline
  ├─ OfflineSubtitlePipeline
  └─ DirectoryTaskPipelineAdapter
```

它只回答四个问题：

```text
1. 当前是什么模式？
   realtime_dictation / realtime_wake / offline_file / directory_task

2. 当前需要什么能力？
   wake / vad / diarization / speaker_embedding / asr / subtitle_writer

3. 当前应该用什么引擎？
   sherpa-onnx / qwen3_stateful / qwen3 offline HTTP or CLI / pyannote sidecar / external provider

4. 当前能不能跑？
   assets ready / platform supported / resource available / priority allowed
```

统一输出：

```rust
pub struct EngineDecision {
    pub mode: SpeechMode,
    pub pipeline_profile: String,
    pub asr: AsrDecision,
    pub vad: Option<VadDecision>,
    pub wake: Option<WakeDecision>,
    pub diarization: Option<DiarizationDecision>,
    pub speaker_embedding: Option<SpeakerEmbeddingDecision>,
    pub subtitle: Option<SubtitleDecision>,
    pub runtime: RuntimeDecision,
    pub resource: ResourceDecision,
    pub reasons: Vec<DecisionReason>,
}
```

原则：

- WebUI、CLI、API 和 Directory Task 只问 Orchestrator “这个模式该怎么跑”，不再各自判断 provider、模型、资产和资源。
- Orchestrator 不直接操作 UI，也不直接下载模型；它只做能力决策、旧配置迁移推导和可执行性解释。
- 资产初始化仍由显式 init API/CLI/WebUI 完成；任务运行时只检查 ready 状态。

## 三条链路定位

### 实时链路：Voice Runtime

实时听写主链路固定为：

```text
WebMic / CLI Mic / Voice Helper
  -> 16kHz mono PCM16
  -> VAD / endpointing
  -> qwen3_stateful_streaming
  -> partial / stable_delta / final_utterance
  -> InputMethod / WebUI / CLI / Agent
```

默认 profile：

```text
id: realtime-dictation-local
mode: realtime_dictation
input: pcm16_16k_mono
vad: rms-vad-v1 / sherpa-vad-v2
asr:
  provider: qwen3_stateful_streaming
  model: Qwen3-ASR-0.6B
  chunk_size_ms: 500
endpointing:
  silence_commit_ms: 500
  max_utterance_ms: 30000
output:
  partial
  stable_delta
  final_utterance
```

实时链路不做 diarization、不写正式字幕、不使用 1.7B 作为默认模型。1.7B 只作为显式实验或高精度选项。

### 离线链路：Offline Subtitle Pipeline

离线单文件链路的目标不是返回一段 text，而是生成标准字幕产物：

```text
source audio
  -> normalize 16kHz mono WAV
  -> optional VAD / enhancement
  -> optional diarization
  -> ASR Unit Planner
  -> ASR transcription
  -> speaker alignment
  -> subtitle writer
  -> artifacts
```

默认 profile：

```text
id: offline-speaker-subtitle-local
mode: offline_file, directory_task
preprocess:
  normalize: ffmpeg-16k-mono
  vad: optional
diarization:
  engine: sherpa-onnx
  profile: sherpa-onnx-balanced
asr:
  provider: qwen3-offline
  model: Qwen3-ASR-0.6B
  optional_model: Qwen3-ASR-1.7B
  runtime_strategy: reuse_per_file
planner:
  merge_same_speaker_gap_ms: 800
  max_unit_ms: 30000
  min_unit_ms: 500
  min_rms: 0.008
subtitle:
  formats: srt, vtt, txt, timeline_json
```

离线文件转字幕和 Directory Task 必须共用 `OfflineSubtitlePipeline`，不能一条写在上传 preview handler，另一条写在 `asr_jobs/runner.rs`。

### 定时任务：Directory Task 批量适配器

Directory Task 只负责：

```text
discover files
dedupe / hash
pause / resume
schedule
progress / FileStore
调用 OfflineSubtitlePipeline
merge per-file outputs
refresh Daily Docs
enqueue Daily Agent / AI Runner
report / IM / sync post-processing
```

它不再直接关心 sherpa、pyannote、Qwen chunk、subtitle writer 的内部细节。后续 speaker-aware 复杂逻辑必须下沉到 `OfflineSubtitlePipeline` 和 `AsrUnitPlanner`。

### Directory Task 后处理契约

`OfflineSubtitlePipeline` 的边界必须收在单个文件的标准产物：

```text
input audio file
  -> TranscriptTimeline
  -> text / metadata / diarization manifest
  -> srt / vtt / optional ass
```

它返回后，Directory Task runner 必须继续执行现有后处理：

```text
OfflineSubtitlePipeline::run_file()
  -> persist FileRecord / output paths / summary metrics
  -> retry failed chunks / merge partial success evidence
  -> refresh_task_daily_summaries()
  -> persist ASR terminal state
  -> maybe_enqueue_daily_agent_after_asr_run()
  -> DailyAgentChangePlanner
  -> Runner execution
  -> report write / processed state / Git / IM delivery / report sync
```

强制约束：

- Daily Agent Runner 是 ASR Directory Task 的后处理阶段，不是独立 scheduler，也不是 OfflineSubtitlePipeline 的内部步骤。
- Daily Agent 不得在 ASR 音频处理、failed chunk retry、daily markdown 刷新和 ASR 状态持久化前启动。
- speaker-aware timeline/text 必须成为 Daily Docs 的输入；Daily Agent 读取 Daily Docs 时自然获得 speaker label，不能从纯文本反推 speaker。
- Daily Agent 失败不回滚 ASR 文件成功状态；Runner 成功后才更新 `daily_agent_processed.json`。
- `ResourceLeaseManager` 只能协调 ASR/voice/sherpa 等语音资源；Daily Agent / external Runner 使用独立并发锁和状态集，避免因为语音资源让出而丢失后处理。

定时任务 profile：

```text
id: scheduled-speaker-subtitle-local
mode: directory_task
inherits: offline-speaker-subtitle-local
resource_policy:
  priority: background
  preemptible: true
  max_concurrent_files: 1
  pause_on_realtime_voice: true
```

## 遗留降级与迁移策略

### `/api/asr/transcribe-ws`

降级定位：

```text
deprecated_upload_like_realtime
```

策略：

- 不为该路径新增兼容服务、资源调度、模型 warmup、speaker-aware、字幕或抢占能力。
- WebUI 和新 CLI 必须移除对它的默认依赖。
- 如果端点短期仍存在，只允许返回明确迁移错误或低成本 deprecation 响应，例如 `410 gone` / `400 use_voice_listen_ws`，并指向 `/api/voice/listen-ws`。
- 只允许安全修复和删除前的迁移提示，不再以“旧脚本继续可用”作为验收目标。
- 后续版本可以直接移除端点；产品验收只看新 realtime voice runtime 是否满足要求。

迁移目标：

```text
/api/asr/transcribe-ws -> /api/voice/listen-ws
```

### `/api/asr/transcribe-stream`

降级定位：

```text
quick_preview_upload
```

策略：

- 继续作为 WebUI Speech Workbench 拖入文件的快速预览路径。
- 如果实现成本低且不影响主链路，可启用 speaker-aware preview。
- 不作为正式字幕 artifact 主接口，也不要求和 `offline-jobs` 保持完全等价。
- 新的正式产物接口是 `/api/asr/offline-jobs`。
- 后续可以被 `offline-jobs` 的 preview mode 替换；替换后无需保留旧 preview 行为。

### 旧 Directory Task 字段

读取迁移字段：

```rust
pub struct AsrDirectoryTask {
    pub model: String,
    pub language: String,
    pub runtime_strategy: AsrRuntimeStrategy,
    pub diarization: AsrDiarizationConfig,

    #[serde(default)]
    pub pipeline_profile: Option<String>,
}
```

推导规则：

```text
pipeline_profile != None
  -> 使用 profile；model/language/runtime_strategy 只作为显式 override 输入

pipeline_profile == None && diarization.enabled == false
  -> offline-plain-asr-local

pipeline_profile == None && diarization.enabled == true
  -> offline-speaker-subtitle-local
```

降级要求：

- 新建任务优先写入 `pipeline_profile`。
- 旧任务读取时可以做一次性迁移或 resolver 推导，但不要求完整复刻旧 WebUI/CLI 表达。
- 产品验收只要求迁移后的任务进入正确新 pipeline；旧脚本字段展示可以降低。
- 如果 profile 与旧字段冲突，profile 优先；任务详情页和 CLI 展示 resolver reason，避免静默使用错误模型。

### Timeline 与字幕输出

产品保留输出：

```text
.txt
.json
.timeline.json
```

新增字幕产物：

```text
.diarization.json
.metadata.json
.srt
.vtt
.ass optional
```

`.txt` 仍作为产品输出；speaker-aware 任务的 `.txt` 使用时间范围和 speaker 前缀。`TranscriptTimeline` 是唯一数据源，subtitle writer 只消费 timeline，不重新猜 speaker。旧 JSON shape 不作为长期兼容承诺；如与新 timeline schema 冲突，以新 schema 为准。

## 核心模块设计

ASR 核心能力必须抽到独立 crate：

```text
crates/bifrost-asr/
  src/
    lib.rs
    platform.rs
    runtime.rs
    pipeline.rs
    decision.rs
    profiles.rs
    resources.rs
    manifest.rs
    planner.rs
    subtitle.rs
    artifacts.rs
    timeline.rs
    realtime.rs
    offline.rs
    engines/
      sherpa_onnx.rs
      pyannote_sidecar.rs
      qwen3_offline.rs
      qwen3_stateful.rs
      external.rs
```

`bifrost-admin` 只保留适配层：

```text
crates/bifrost-admin/src/handlers/
  asr.rs                  # HTTP route / request parsing / response shaping
  asr_jobs/               # task state, scheduler, FileStore, Daily Agent post-processing
  voice/                  # websocket route and session API adapter
```

依赖规则：

- `bifrost-asr` 是 ASR engine、pipeline、planner、subtitle writer、asset/profile decision 的所有者。
- `bifrost-asr` 也是 ASR 纯业务规则的所有者：service runtime path/state、capability platform matrix、diarization profile/config、timeline schema/normalization/render、daily summary generation 和 artifact output path 都不能继续散落在 admin。
- `bifrost-admin` 不能直接依赖 `qwen3-asr`、`sherpa-onnx`、pyannote sidecar 细节或 native symbols。
- `bifrost-admin` 通过 `bifrost-asr` 的 trait/API 或 re-export 调用能力；不支持 ASR 的构建可以不启用 `bifrost-admin/asr` feature，或让 admin 编译轻量 no-ASR adapter。
- `bifrost-admin` 可以保留 HTTP route、request/response shaping、Directory Task 状态机、FileStore、scheduler、Daily Docs/Daily Agent 后处理和权限边界；这些属于产品编排，不放进底层 ASR crate。
- `bifrost-cli` 如果需要本地单文件 ASR 命令，可以直接依赖 `bifrost-asr`；如果只需要远程调用 Admin API，则不依赖 `bifrost-asr`。

推荐 feature 边界：

```toml
[features]
default = ["core"]
core = []
subtitle = ["core"]
diarization-sherpa = ["core", "dep:sherpa-onnx"]
qwen3-offline = ["core", "dep:qwen3-asr"]
qwen3-stateful = ["qwen3-offline"]
pyannote-sidecar = ["core"]
voiceprint = ["diarization-sherpa"]
full-local-asr = ["subtitle", "diarization-sherpa", "qwen3-offline", "qwen3-stateful", "voiceprint"]
```

平台编译策略：

```text
macOS aarch64 local ASR build:
  bifrost-admin = { features = ["asr"] }
  bifrost-asr = { features = ["full-local-asr"] }

Linux / Windows / old glibc no local ASR build:
  bifrost-admin = { default-features = ..., features = [] }
  no dependency edge to qwen3-asr / sherpa-onnx

server/API only build with remote ASR:
  bifrost-admin = { features = ["asr-api"] }
  bifrost-asr = { features = ["core", "subtitle"] }
```

验收门禁：

- `cargo metadata --filter-platform` 在不支持平台上不能解析 `qwen3-asr` / `sherpa-onnx`。
- `cargo tree -p bifrost-admin --target <unsupported>` 不能出现 `qwen3-asr` / `sherpa-onnx`。
- 绝大多数跨平台差异收敛在 `bifrost-asr/Cargo.toml` feature 和 target dependency 中，admin 代码不再散落 native provider `cfg`。
- 不支持 ASR 的平台隐藏 ASR 本地入口或返回 `asr_unavailable_in_this_build`，而不是链接失败。

### EngineDecisionResolver

入口：

```rust
pub enum SpeechMode {
    RealtimeDictation,
    RealtimeWake,
    OfflineFile,
    DirectoryTask,
}

pub fn resolve_engine_decision(
    mode: SpeechMode,
    request: SpeechRequest,
    env: SpeechRuntimeEnv,
) -> EngineDecision
```

输出必须包含：

- profile 来源：显式指定、旧字段推导、默认 profile。
- assets ready 状态。
- platform support 状态。
- resource 可用性。
- fallback 和降级原因。
- legacy downgrade/migration reason。

### AsrUnitPlanner

新增结构：

```rust
pub struct AsrAudioUnit {
    pub unit_id: String,
    pub speaker: Option<String>,
    pub speaker_display_name: Option<String>,
    pub mapped_profile_id: Option<String>,
    pub source_start_ms: u64,
    pub source_end_ms: u64,
    pub source_segment_ids: Vec<String>,
    pub overlap: bool,
    pub unit_kind: AsrUnitKind,
}
```

规划流程：

```text
DiarizationSegment[]
  -> remove invalid / zero-length segments
  -> merge same speaker nearby segments
  -> split units over 30s
  -> skip too-short low-energy units
  -> preserve overlap flag
  -> produce AsrAudioUnit[]
```

默认规则：

```text
merge_same_speaker_gap_ms = 800
max_unit_ms = 30000
min_unit_ms = 500
min_rms = 0.008
```

V1 重叠语音：

- `overlap=true` 保留到 timeline。
- 仍按主 speaker 生成 unit。
- 字幕不额外猜第二个人。

V2 再做：

- pyannote exclusive diarization。
- overlap-aware subtitle lane。
- source separation。

### Speaker Stabilizer 和声纹优先级

`max_speakers` 只限制聚类数量，不等于真实人数判断。真实音频里 sherpa diarization 仍可能把同一个人拆成多个短碎片 cluster，尤其是短句、笑声、重叠语音、远近麦和音色变化明显时。因此离线 speaker-aware pipeline 必须在 diarization 后增加稳定化阶段：

```text
DiarizationSegment[]
  -> compute per-speaker embedding
  -> merge embedding-similar short clusters into dominant speaker
  -> absorb fragmentary temporal-neighbor clusters
  -> densify local speaker ids/display names
  -> voiceprint matching
  -> AsrUnitPlanner
```

默认规则：

```text
speaker_merge_similarity_threshold = 0.78
short_speaker_merge_similarity_threshold = 0.66
short_speaker_max_duration_ms = 10000
short_speaker_max_segments = 4
fragment_neighbor_max_gap_ms = 5000
voiceprint_match_threshold = 0.60
single_registered_self_priority_threshold = 0.52
single_registered_self_priority_min_duration_ms = 5000
```

声纹匹配策略：

- 正式匹配仍以 `voiceprint_match_threshold=0.60` 为默认可信阈值。
- 如果只有一个已注册声纹，且没有 cluster 达到 0.60，则允许把得分最高、时长足够的 cluster 标为本人。这是“用户自己录入声纹后优先识别自己”的产品语义。
- 低于正式阈值的最佳候选仍写入 `candidate_profile_id/candidate_display_name/candidate_confidence`，用于 UI 解释和后续调参；字幕文本不使用候选身份，避免低置信度误标。
- 多注册声纹场景不走单人 self-priority，需要后续加入 profile 间 margin 和冲突仲裁。

### OfflineSubtitlePipeline

统一方法：

```rust
impl OfflineSubtitlePipeline {
    pub async fn run_file(&self, req: OfflineSubtitleRequest) -> Result<OfflineSubtitleArtifacts>;
}
```

内部步骤：

```text
normalize_to_temp()
  -> run_diarization_if_enabled()
  -> plan_asr_units()
  -> transcribe_units()
  -> align_speakers()
  -> write_timeline()
  -> write_subtitles()
  -> write_metadata()
```

Directory Task 接入后：

```text
runner.rs
  -> OfflineSubtitlePipeline::run_file()
```

而不是：

```text
runner.rs
  -> transcribe_diarized_segments_for_task()
  -> for each diarization segment run ASR
```

### ResourceLeaseManager

资源类型：

```rust
pub enum SpeechResourceKind {
    MlxAsrModel,
    StatefulVoiceWorker,
    OfflineAsrServer,
    SherpaDiarizationCpu,
    PyannoteSidecar,
    WakeListener,
}

pub struct ResourceLeaseRequest {
    pub owner_module: String,
    pub owner_id: String,
    pub kind: SpeechResourceKind,
    pub model_id: Option<String>,
    pub priority: u8,
    pub preemptible: bool,
}
```

优先级：

```text
100 realtime dictation
90  wake listener action
70  user-triggered offline subtitle
40  manual directory task run
20  scheduled directory task
```

策略：

- 实时听写启动时，如果后台 scheduled task 正在跑 ASR，后台任务在 unit 边界 pause/yield。
- 定时任务运行时，如果 realtime voice session active，不启动新文件；已启动文件在当前 unit 完成后让出。
- 唤醒词监听默认不得持有 Qwen3 大模型，只允许轻量 KWS/VAD 常驻。
- 离线字幕默认使用 0.6B；用户显式选择 1.7B 时，不能和 realtime stateful worker 同时抢 MLX。

## API 改造

### Pipeline 状态与决策

新增：

```http
GET  /_bifrost/api/speech/pipelines
GET  /_bifrost/api/speech/pipelines/status
POST /_bifrost/api/speech/pipelines/{id}/init
GET  /_bifrost/api/speech/decision?mode=offline_file&profile=offline-speaker-subtitle-local
```

返回示例：

```json
{
  "mode": "offline_file",
  "profile": "offline-speaker-subtitle-local",
  "engines": {
    "diarization": {
      "provider": "sherpa-onnx",
      "profile": "sherpa-onnx-balanced",
      "ready": true
    },
    "asr": {
      "provider": "qwen3-offline",
      "model": "Qwen3-ASR-0.6B",
      "ready": true
    }
  },
  "resource": {
    "available": true,
    "reason": null
  }
}
```

### 实时链路 API

保留并强化：

```http
GET  /_bifrost/api/voice/sources
GET  /_bifrost/api/voice/status
POST /_bifrost/api/voice/sessions
GET  /_bifrost/api/voice/listen-ws
```

修正：

- `/api/voice/listen-ws` 是唯一实时听写主链路。
- `/api/asr/transcribe-ws` 标记 deprecated，不再被新 UI 使用；可以返回迁移错误或后续删除。

唤醒词：

```http
GET  /_bifrost/api/voice/wake/status
POST /_bifrost/api/voice/wake/listener/start
POST /_bifrost/api/voice/wake/listener/stop
GET  /_bifrost/api/voice/wake/bindings
POST /_bifrost/api/voice/wake/bindings
GET  /_bifrost/api/voice/wake/events
```

listener 默认引擎从 `backend_asr_listener` 升级为 `lightweight_kws_listener`，旧 ASR phrase match 仅作为 fallback。

### 单文件离线字幕 API

新增正式 artifact 接口：

```http
POST /_bifrost/api/asr/offline-jobs
GET  /_bifrost/api/asr/offline-jobs/{job_id}
GET  /_bifrost/api/asr/offline-jobs/{job_id}/events
GET  /_bifrost/api/asr/offline-jobs/{job_id}/artifacts
GET  /_bifrost/api/asr/offline-jobs/{job_id}/artifacts/{format}
```

请求：

```json
{
  "source": {
    "type": "upload"
  },
  "pipeline_profile": "offline-speaker-subtitle-local",
  "language": "chinese",
  "model": "Qwen3-ASR-0.6B",
  "speaker_aware": true,
  "subtitle_formats": ["srt", "vtt", "txt", "timeline_json"]
}
```

### Directory Task API

扩展创建/更新：

```json
{
  "model": "Qwen3-ASR-0.6B",
  "language": "chinese",
  "pipeline_profile": "scheduled-speaker-subtitle-local",
  "diarization": {
    "enabled": true,
    "profile": "sherpa-onnx-balanced",
    "known_speaker_count": 2,
    "voiceprint_matching": false
  },
  "subtitle": {
    "formats": ["srt", "vtt", "txt", "timeline_json"]
  }
}
```

新增 artifact 查询：

```http
GET /_bifrost/api/asr/tasks/{task_id}/files/{file_key}/artifacts
GET /_bifrost/api/asr/tasks/{task_id}/files/{file_key}/artifacts/srt
GET /_bifrost/api/asr/tasks/{task_id}/files/{file_key}/artifacts/vtt
```

## CLI 改造

实时听写：

```bash
bifrost ai voice listen \
  --source mic \
  --model Qwen3-ASR-0.6B \
  --language chinese \
  --jsonl
```

唤醒词：

```bash
bifrost ai voice wake status
bifrost ai voice wake listener start --source mic
bifrost ai voice wake listener stop
bifrost ai voice wake bindings list
```

单文件字幕：

```bash
bifrost ai asr subtitle ./meeting.wav \
  --speaker-aware \
  --profile offline-speaker-subtitle-local \
  --format srt,vtt,txt,json \
  --out ./out
```

定时任务：

```bash
bifrost ai asr task create \
  --name "Meetings" \
  --dir ~/Recordings \
  --pipeline scheduled-speaker-subtitle-local \
  --speaker-aware \
  --format srt,vtt,txt,json

bifrost ai asr task run <task-id>
bifrost ai asr task files <task-id>
```

## WebUI 改造

ASR/Speech 页面拆成三个入口：

```text
Speech
  ├─ Realtime Voice
  │   ├─ microphone source
  │   ├─ listen-ws status
  │   ├─ wake listener status
  │   └─ vocabulary
  │
  ├─ Subtitle Converter
  │   ├─ upload file
  │   ├─ speaker-aware switch
  │   ├─ output format: SRT / VTT / TXT / JSON
  │   └─ artifact download
  │
  └─ Directory Tasks
      ├─ task list
      ├─ pipeline profile
      ├─ speaker-aware config
      ├─ file progress
      └─ artifacts
```

顶部统一状态：

```text
Speech Engines
  ASR Model: Qwen3-ASR-1.7B / 0.6B
  Realtime Provider: qwen3_stateful_streaming
  Speaker Engine: sherpa-onnx-balanced
  Wake Engine: lightweight KWS / fallback ASR
  Resource: idle / realtime active / offline running / scheduled paused
```

UI 规则：

- 实时语音默认用 0.6B。
- 字幕转换默认用 0.6B，1.7B 只作为显式高精度选项。
- Directory Task 默认按 resolved pipeline 执行；旧任务只做必要迁移，不为旧表现维持额外兼容逻辑。
- 资产缺失时提示 Initialize，不在任务运行时偷偷下载。
- 唤醒词常驻不默认拉起 Qwen3 大模型。
- WebUI 改动必须同时验证 light/dark 主题。

## Artifact Schema

每个离线任务输出：

```text
<output_dir>/
  source.timeline.json
  source.diarization.json
  source.metadata.json
  source.txt
  source.srt
  source.vtt
  source.ass optional
```

`TranscriptTimeline` 示例：

```json
{
  "version": 1,
  "task_id": "task_x",
  "source_path": "/audio/call.wav",
  "media_duration_ms": 128430,
  "model": "Qwen3-ASR-0.6B",
  "language": "chinese",
  "pipeline_profile": "offline-speaker-subtitle-local",
  "diarization_profile": "sherpa-onnx-balanced",
  "speakers": [
    {
      "id": "speaker_00",
      "display_name": "用户A",
      "mapped_profile_id": null,
      "confidence": null
    }
  ],
  "segments": [
    {
      "index": 0,
      "audio_start_ms": 1200,
      "audio_end_ms": 5860,
      "speaker": "speaker_00",
      "speaker_display_name": "用户A",
      "overlap": false,
      "text": "你好，我想咨询一下订单。"
    }
  ]
}
```

SRT：

```srt
1
00:00:01,200 --> 00:00:05,860
用户A: 你好，我想咨询一下订单。
```

WebVTT：

```vtt
WEBVTT

00:00:01.200 --> 00:00:05.860
用户A: 你好，我想咨询一下订单。
```

TXT：

```text
[00:00:01.200 - 00:00:05.860] 用户A: 你好，我想咨询一下订单。
```

这些 artifacts 是后续 daily markdown 与 Daily Agent 的输入事实源。Directory Task 的每日合并逻辑必须从 `TranscriptTimeline` 渲染 speaker-aware Daily Docs，再按既有 Daily Agent contract 做增量变更规划、Runner 投递和 report 生成。

## 分阶段落地计划

### Phase 1：统一离线 pipeline

改动：

```text
新增 speech/offline.rs
新增 speech/planner.rs
新增 speech/subtitle.rs
Directory Task 接入 OfflineSubtitlePipeline
保留 Directory Task 后处理 hook
```

验收：

- 普通 Directory Task 行为不变。
- speaker-aware task 输出 `.timeline.json`、`.diarization.json`、`.srt`、`.vtt`、`.txt`。
- sherpa assets 缺失时明确失败，不隐式下载。
- 真实 2 人对话样本能输出 `用户A/用户B: 文本` 字幕。
- Directory Task 文件产物落盘后仍刷新 Daily Docs，并在满足条件时调用 `maybe_enqueue_daily_agent_after_asr_run()` 排队 Daily Agent / AI Runner。

### Phase 2：单文件 offline-jobs API

改动：

```http
POST /api/asr/offline-jobs
GET  /api/asr/offline-jobs/{id}
GET  /api/asr/offline-jobs/{id}/artifacts/{format}
```

验收：

- 上传 `meeting.wav` 后可下载 srt/vtt/txt/json。
- 和 Directory Task 输出 schema 一致。
- WebUI 可展示 speaker timeline。

### Phase 3：实时链路收敛

改动：

- `/api/voice/listen-ws` 作为唯一实时主链路。
- `/api/asr/transcribe-ws` 降级或下线。
- 增加 VoiceSessionManager 与资源 lease。

验收：

- 浏览器麦克风实时输入不再走 MediaRecorder 全会话重转码。
- CLI 可以推 PCM 到 `/api/voice/listen-ws`。
- 30 秒以上连续听写不会无限累积 state。
- 实时听写启动时，后台 scheduled task 会让出 ASR 资源。

### Phase 4：唤醒词轻量化

改动：

```text
backend_asr_listener
  -> vad/kws listener
  -> phrase candidate
  -> optional speaker verify
  -> action
```

验收：

- wake listener 不默认启动 Qwen3-ASR。
- 没有 voiceprint 时允许 phrase wake dry-run/debug mode。
- 敏感 action 仍要求 speaker verification。
- cooldown、events、bindings 继续保留。

### Phase 5：资源优先级和抢占

改动：

```text
ResourceLeaseManager
realtime dictation > manual offline subtitle > manual task > scheduled task
```

验收：

- scheduled task 运行中，用户开始实时听写，scheduled task 在 unit 边界暂停。
- 实时结束后，scheduled task 可继续。
- 同一时间不会启动两个 MLX/Qwen 大模型进程。
- WebUI 能解释任务等待、暂停和资源占用 owner。

## 测试计划

单元测试：

```text
resolve_realtime_dictation_uses_stateful_0_6b
resolve_offline_subtitle_enables_sherpa_diarization
resolve_directory_task_without_profile_maps_to_plain_pipeline
resolve_directory_task_with_diarization_maps_to_speaker_pipeline
planner_merges_same_speaker_gap
planner_splits_unit_over_30s
planner_skips_short_low_energy_segments
speaker_stabilizer_merges_short_similar_cluster
speaker_stabilizer_absorbs_fragmentary_neighbor
voiceprint_mapping_records_below_threshold_candidate
voiceprint_single_registered_profile_uses_self_priority
subtitle_writer_formats_srt_timecode
subtitle_writer_formats_vtt_timecode
resource_manager_realtime_preempts_scheduled
wake_trigger_requires_cooldown
deprecated_transcribe_ws_returns_migration_error
```

E2E 测试：

```text
test_speech_decision_api.sh
test_asr_offline_jobs_artifacts.sh
test_directory_task_offline_pipeline_artifacts.sh
test_voice_listen_ws_realtime_pcm.sh
test_deprecated_transcribe_ws_migration_error.sh
test_scheduled_task_yields_to_realtime_voice.sh
test_directory_task_offline_pipeline_keeps_daily_agent_postprocess.sh
test_asr_speech_pipeline_orchestrator_real_service.sh
```

human_tests：

- 2 人会议音频生成 SRT/VTT/TXT/TIMELINE。
- scheduled task 运行时启动实时听写，确认后台任务让出资源。
- 配置 wake binding，确认 dry-run event 正常落盘且不启动 Qwen3。
- 删除 diarization assets，确认任务失败信息可操作。
- 普通旧 Directory Task 迁移到 plain pipeline 后仍能完成转写；不要求旧 UI/CLI 表达完全保留。
- 旧 `/api/asr/transcribe-ws` 返回明确迁移错误或已下线状态；不要求继续转写成功。
- Directory Task speaker-aware artifacts 生成后，Daily Docs、Daily Agent Runner、report、Git/IM/report sync 后处理仍按既有 contract 继续执行。

项目校验：

- 文档方案阶段：执行 human_tests 静态验收；Rust 单元、E2E、workspace all-features 和 local-ci 标记为不适用。
- 实现阶段：按仓库规则执行 `e2e-test`、`rust-project-validate`、`cargo test --workspace --all-features`，并新增 `cargo metadata --filter-platform` / `cargo tree -p bifrost-admin --target <unsupported>` 验证 ASR native 依赖不进入非 ASR 构建，再按修改范围决定 `scripts/ci/local-ci.sh`。

## 最新推荐落地顺序

最高优先级：

```text
1. 抽出 crates/bifrost-asr 编译边界和 feature matrix
2. AsrUnitPlanner
3. OfflineSubtitlePipeline
4. 单文件 offline-jobs API
5. Directory Task 接入 OfflineSubtitlePipeline
6. /api/voice/listen-ws 成为实时主链路
7. wake listener 轻量化
8. ResourceLeaseManager
```

原因：

- `bifrost-asr` 先抽出来，才能从 Cargo graph 上解决跨平台 native 依赖和 admin 内散落 `cfg` 的问题。
- `AsrUnitPlanner + OfflineSubtitlePipeline` 同时解决离线单文件字幕和定时批处理两个核心场景。
- 实时链路已经有 `/api/voice/listen-ws` 和 stateful worker 基础，可以在离线主链路稳定后直接降级或移除旧 `/api/asr/transcribe-ws`。
- 唤醒词已有 API 和事件模型，真正要改的是默认引擎和资源策略，不需要推翻现有存储结构。
- ResourceLeaseManager 最后收敛，避免先做复杂抢占却没有统一离线 unit 边界。

## Review/Fix/Test 闭环方案

第 1 轮：

- 目标复核：检查本方案是否覆盖实时、离线、定时、旧支持降级、资源、API/CLI/WebUI 和落地顺序。
- 变更复核：执行 `git status --short`、`git diff`，确认只改 design/human_tests/readme。
- 文档 review：检查是否有“偷偷下载模型”“为了旧接口做并行兼容服务”“把 preview 当正式 artifact”“离线 pipeline 吞掉 Daily Agent 后处理”的表述。
- 测试运行：执行 `human_tests/asr-speech-pipeline-orchestrator.md` 中的静态验收命令。

第 2 轮：

- 再次目标复核：对照用户给出的原始方案，确认所有关键项都被结构化落入文档。
- 再次变更复核：复查最新 diff、human_tests/readme 索引和测试用例数。
- 再次文档 review：检查旧支持降级策略、Phase 顺序、测试计划和不适用项说明。
- 复跑测试：复跑全部静态验收命令，确认无需第 3 轮。

## 当前实现收敛清单

本轮实现必须把方案落到真实产品能力，而不是继续停留在文档和适配壳：

- `crates/bifrost-asr` 承接 ASR 主要业务逻辑：profile/decision/resource lease/planner/offline artifact/subtitle/timeline，`bifrost-admin` 只保留 HTTP、任务状态、托管进程和 Directory Task 后处理适配。
- `/api/speech/pipelines/status`、`/api/speech/decision` 和 `/api/speech/resources` 暴露统一 pipeline 状态、引擎决策和资源租约状态。
- `/api/asr/offline-jobs` 是单文件字幕正式产物接口，输出 `txt/srt/vtt/timeline_json/metadata`，WebUI Speech Workbench 和 `bifrost ai asr subtitle` 都走这个接口。
- `/api/asr/transcribe-ws` 不再提供旧兼容转写服务，返回 410 和迁移指引。
- Directory Task 保持既有输出合并、Daily Docs、Daily Agent / AI Runner、report/IM/sync 后处理；OfflineSubtitlePipeline 只负责单文件标准 ASR 产物。
- wake listener 默认 `lightweight_kws_listener`，不默认拉起 Qwen3；无声纹配置只允许 dry-run，真实执行动作需要 speaker verification。
- `ResourceLeaseManager` 让 realtime voice、offline job 和 scheduled Directory Task 共用资源优先级，scheduled task 在 realtime active 时让出。
- 托管 Qwen3-ASR runtime 按 `host/home/model/port` 共享，不再按 `owner_module` 隔离；`speech_workbench` 手动启动的 0.6B 服务必须能被 Workflows / wake listener 复用，跨 owner 停止同一模型服务时也必须清理持久化 state，避免 stale owner 阻塞 recorder。
- 新增真实服务回归脚本 `e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh`，启动当前 Bifrost 服务并验证 speech API、旧 WS 下线、wake lightweight；在 Apple Silicon 且设置 `BIFROST_ASR_PIPELINE_E2E_ONLINE=1` 时继续验证真实语音、offline-jobs、CLI subtitle、Directory Task artifacts 和 Daily Agent 后处理入口。CI/非在线 ASR 环境不使用 mock ASR，只跳过需要本地 Qwen3-ASR 资产的产物链路。

## 实时麦克风多说话人 Timeline

WebUI Speech Workbench 的 `Start Mic` 链路继续走 `/api/voice/listen-ws`，但不能只展示一段纯文本。stateful Qwen3-ASR 每次提交 utterance 时，后端同时输出时间窗和说话人字段：

```json
{
  "type": "asr_stable_delta",
  "window_start_ms": 1200,
  "window_end_ms": 5860,
  "utterance_index": 7,
  "speaker": "speaker_00",
  "speaker_display_name": "eden",
  "speaker_profile_id": "spk_x",
  "speaker_confidence": 0.61,
  "candidate_display_name": "eden",
  "candidate_confidence": 0.61,
  "delta": "你好，我想咨询一下订单。"
}
```

实现策略：

- `VoiceTranscriptState` 记录 utterance 的真实起始时间，按 speech chunk 的开始时间而不是结束时间计。
- `VoiceRealtimeAudioBuffer` 保存最近 120 秒 PCM16 时间窗，只在 fresh frame 进入 stateful worker 后写入，避免 worker 启动期间的陈旧浏览器缓冲污染说话人识别。
- `RealtimeVoiceSpeakerTracker` 在 stable/final utterance 边界截取对应 PCM，复用 speaker embedding 能力。
- 已注册声纹优先：正式阈值通过时直接显示真人名；只有一个注册声纹时，允许短实时 utterance 使用较低 self-priority 阈值，但仍保留置信度。
- 未匹配声纹时，在本次 WebSocket session 内做 embedding 聚类，默认最多 4 个本地角色 `用户A/B/C/D`，避免实时链路也出现 20 个用户的爆炸。
- 重叠说话 V1 不做 source separation；事件保持单主说话人，离线 pipeline 仍是最终 who-said-what 的高质量产物链路。
- Work Actions 的 `Start Listening` 默认优先 `lightweight_kws_listener`；如果本地 KWS 资产未初始化，不再返回 400，而是启动 `backend_asr_phrase_match` fallback，并在 listener status 写入 `kws_missing_fallback_backend_asr_phrase_match` 解释当前资源路径。

WebUI 行为：

- Transcript 区域保留完整文本，同时新增 Live Timeline 列表。
- 每行展示时间范围、speaker tag、置信度或候选声纹、文本内容。
- 已稳定的实时 timeline 可直接导出 `live-realtime.srt`、`live-realtime.txt`、`live-realtime.timeline.json`。
- 实时 timeline 是低延迟产品体验；如果需要更准确的多人重叠语音、speaker 重排和字幕产物，应在录制后交给 offline subtitle pipeline 复跑。
