# Audio Diarization 与 ASR 离线任务集成方案

## 背景与目标

本方案把“说话人分离 / diarization”落到 Bifrost 当前 ASR Directory Task 的离线处理链路中。V1 不做实时麦克风、不做代理流量自动分析、不默认上传音频，只改造离线任务：用户给一个音频文件或目录任务后，Bifrost 先识别说话人并切片，再把切好的语音片段流式送入 ASR，最后生成带时间轴和说话人标签的转录文件。

本节仅陈述目标；具体的验证清单条目见下方 `用户目标验证清单` 独立章节。

## 用户目标验证清单

### 必须实现

- 仓库记录「双引擎 + 可插拔 profile」的工程选型，默认轻量引擎为 `sherpa-onnx-balanced`，高质量引擎为 `pyannote-community-quality` sidecar，DiariZen / Sortformer 只作为 lab profile。
- V1 集成点放在 ASR Directory Task 离线处理环节，当前音频文件在进入 ASR 前先做 diarization、speaker 切片和角色信息整理。
- ASR 输入不再只按固定 30 秒原始 chunk，而是优先消费 diarization 产生的 speech segment / merged ASR unit；每个 unit 处理完成后增量写入 timeline。
- 最终 `.timeline.json`、`.txt`、Daily Docs 和 WebUI timeline 都能表达 `speaker_00` / `speaker_01` 这类角色；当前版本未录入声纹时展示「用户A/用户B」，不得冒认真实身份。
- 声纹初始化、profile 存储、enroll 与跨任务身份匹配接口作为后续阶段能力预留；当前 MR 不把未来声纹身份识别描述成已交付功能。

### 必须不破坏

- 未启用 diarization 的现有 Directory Task、外接设备导入、retry failed chunks、pause/resume、Daily Agent report、ASR model owner 隔离和 16k mono normalize 快路径。
- 现有 `TranscriptTimeline` / `TimelineSegment` 序列化契约：新增 `speaker` 字段以 serde `default` 兼容旧 timeline。
- ASR 任务系统底层不重写：diarization 是在 `normalize_to_temp()` 之后、`run_chunked_transcription()` 之前插入的可暂停/落盘/恢复 stage。
- 现有 `asr-diarization-worker` / `/api/asr/diarization/*` / `/api/asr/speaker-profiles/*` HTTP 路由与 manifest 读写。

### 必须真实验证

- 设计、human_tests 与当前代码路径一致；后续实现必须有单元测试、E2E、human_tests 和 workspace all-features 校验。
- 每个阶段（模型选型、离线任务接入、Admin API/CLI/WebUI、声纹录入与匹配）的 human_tests 用例必须真实跑通，不能只做静态验收。

## 当前代码基线

当前 ASR Directory Task 主链路在 `crates/bifrost-admin/src/handlers/asr_jobs.rs` 通过 `include!` 拆分；实际拆分文件位于 `crates/bifrost-admin/src/handlers/asr_jobs/`，已经包含 `state.rs`、`runner.rs`、`audio_processing.rs`、`chunk_runtime.rs`、`diarization.rs`、`voiceprint.rs`、`store.rs`、`retry.rs`、`memory_bisect.rs`、`external_import.rs`、`api.rs`、`daily_agent*.rs`、`tests.rs` 等：

- `state.rs`：`AsrDirectoryTask` 保存 `audio_dir`、`language`、`model`、`runtime_strategy`、`daily_agent`、`external_devices`，并已包含 `pub diarization: AsrDiarizationConfig`；`FileRecord` 保存单个音频文件状态、输出文本、metadata、timeline、chunk metrics、failed chunks、`memory_limit_hints` 等。diarization 状态/manifest 路径/speaker 数不直接写在 `FileRecord` 上，而是由 `diarization_file_state()` 从 manifest 目录推导，序列化到 `FileRecordWithKey` 等包装结构暴露给 API。
- `runner.rs`：`run_directory_task()` 扫描 pending 文件、准备 ASR target、启动 task/file 级 ASR server，然后调用 `process_pending_files()`。
- `audio_processing.rs`：`normalize_to_temp()` 把输入转成 16 kHz mono PCM WAV；这里是 diarization 前置处理的最佳复用点。
- `chunk_runtime.rs`：`run_chunked_transcription()` 当前把 normalized WAV 按 30 秒窗口切分，调用 fork/server ASR，再合并为 `WholeFileTranscription`。
- `diarization.rs` / `voiceprint.rs`：已落地 diarization profile registry、`/api/asr/diarization/*` 和 `/api/asr/speaker-profiles/*` HTTP 路由、manifest 读写以及 `asr-diarization-worker` 子进程调度。
- `asr_streaming.rs`：`WholeFileTranscription` 目前是 `text + Vec<(audio_start_ms, audio_end_ms, text)>`，不含 speaker。
- `asr_jobs_timeline.rs`：`TranscriptTimeline` 和 `TimelineSegment` 是 timeline JSON、文本渲染、Daily Docs 的统一来源。
- `store.rs`：`output_paths()` 把输出写到 `<data_dir>/asr/data/text/<task_id>/<relative>.txt|json|timeline.json`，保留输入目录相对结构。

因此 V1 不应重写 ASR 任务系统，而是在 `normalize_to_temp()` 之后、`run_chunked_transcription()` 之前插入一个可暂停、可落盘、可恢复的 diarization stage。

```text
discover_audio_files
  -> inspect_source_audio
  -> normalize_to_temp(input.wav)
  -> diarize_normalized_audio(input.wav)
  -> slice/merge ASR units by speaker
  -> transcribe each ASR unit through existing ASR runtime
  -> append speaker-aware timeline segments
  -> render text / metadata / Daily Docs / Daily Agent input
```

## 技术选型落库

### Profile 分层

```text
default lightweight (shipped):
  id: sherpa-onnx-balanced
  engine: sherpa-onnx (in-process inside asr-diarization-worker subprocess)
  segmentation: segmentation/model.int8.onnx
    (source: huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0)
  embedding:    embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx
    (source: github.com/k2-fsa/sherpa-onnx releases / speaker-recongition-models)
  fallback_embedding: nemo-titanet-small.onnx (planned, not yet shipped as of 2026-06-16)

quality sidecar (planned, not yet shipped as of 2026-06-16):
  id: pyannote-community-quality
  engine: pyannote-sidecar
  model: pyannote/speaker-diarization-community-1
  install: explicit user action + Hugging Face token
  status: registry entry exists in diarization.rs but install/run path returns
          unsupported / missing assets

lab (planned, not yet shipped as of 2026-06-16):
  id: diarizen-lab | sortformer-lab
  engine: external-command | local-http-sidecar
  distribution: user-provided model only
```

工程判断：

- `sherpa-onnx-balanced` 是 Bifrost 内置默认 profile，因为它适合 Rust、本地离线、CPU、小模型和跨平台发行。
- `pyannote-community-quality` 是用户主动安装的本地 sidecar，因为它质量更好但需要 Python/PyTorch、Hugging Face token 和模型条款确认。
- DiariZen / Sortformer 不随默认发行，不作为普通用户默认选项；只允许 lab profile 或用户自带外部 engine。

### 配置边界

V1 在 ASR task 上新增 diarization 配置，默认关闭，避免隐私和资源成本突然改变现有任务行为。用户启用后默认 profile 是 `sherpa-onnx-balanced`。

实际落地结构在 `crates/bifrost-asr/src/profiles.rs`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrDiarizationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_diarization_profile")] // -> "sherpa-onnx-balanced"
    pub profile: String,
    #[serde(default)]
    pub min_speakers: Option<u8>,
    #[serde(default)]
    pub max_speakers: Option<u8>,
    #[serde(default)]
    pub known_speaker_count: Option<u8>,
    #[serde(default)]
    pub voiceprint_matching: bool,
}
```

`AsrDiarizationConfig::speaker_aware_default()` 把 `enabled=true`、`max_speakers=Some(DEFAULT_AUTO_MAX_SPEAKERS=4)`、`voiceprint_matching=true`，被多个 speaker-aware profile 默认引用。

`AsrDirectoryTask` 增加：

```rust
#[serde(default)]
pub diarization: AsrDiarizationConfig,
```

实际实现没有把 diarization 状态写进 `FileRecord` 字段，而是在 `store.rs::diarization_file_state()` 中按 task config + 是否存在 manifest 文件推导，序列化到响应包装 `FileRecordWithKey` 上的 `diarization_status`、`diarization_manifest_path`、`speaker_count`：

```rust
struct FileRecordWithKey {
    key: String,
    #[serde(flatten)] record: FileRecord,
    #[serde(skip_serializing_if = "Option::is_none")] diarization_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] diarization_manifest_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")] speaker_count: Option<usize>,
}
```

`diarization_error`、`speaker_labels` 字段当前未持久化在 `FileRecord` 上；speaker label 直接来自 manifest 与 timeline。后续如需把错误固化到 record，请使用 serde default，避免破坏旧 FileRecord。

## 新增 crate 与模块边界

实际落地复用 `crates/bifrost-asr/`（不是独立的 `bifrost-audio` crate）。当前模块结构：

```text
crates/bifrost-asr/src/
  artifacts.rs
  decision.rs
  offline.rs
  planner.rs
  platform.rs
  profiles.rs       # AsrDiarizationConfig + speech-mode profile registry
  resources.rs
  runtime.rs
  speaker.rs        # speaker / diarization 数据结构
  subtitle.rs
  timeline.rs       # TranscriptTimeline / TimelineSegment
  wake.rs
  native::sherpa_onnx  # cfg(diarization-sherpa) 重导出 sherpa-onnx crate
```

Diarization 调度、manifest 落盘、worker 子进程编排在 `crates/bifrost-admin/src/handlers/asr_jobs/{diarization.rs,voiceprint.rs,runner.rs,store.rs}`；pyannote sidecar / DiariZen / Sortformer engine 当前未实现（planned, not yet shipped as of 2026-06-16）。

职责划分：

- `bifrost-asr` 只提供 ASR/diarization profile、timeline、speaker 数据结构与平台/资源决策，不依赖 Admin HTTP；真实 ONNX 推理通过 `feature = "diarization-sherpa"` 重导出 sherpa-onnx crate，并在 worker 子进程内调用。
- `bifrost-admin` 的 `asr_jobs` 负责调度、暂停、FileStore、ASR runtime、API、manifest 读写和 `asr-diarization-worker` 子进程编排。
- `bifrost-cli` 调用 Admin API 或通过隐藏的 `bifrost asr-diarization-worker --request <json>` 入口跑 worker，不直接知道 sherpa-onnx 内部参数。

核心 trait：

```rust
pub trait DiarizationEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn prepare(&self, profile: &AudioProfile) -> anyhow::Result<()>;
    fn diarize(
        &self,
        input: NormalizedAudio,
        options: DiarizationOptions,
    ) -> anyhow::Result<DiarizationResult>;
}
```

V1 不提供 mock / heuristic speaker engine。默认 profile 必须使用真实 sherpa-onnx `OfflineSpeakerDiarization`，profile 初始化必须落盘真实 ONNX 模型文件；模型缺失时任务失败并给出 `diarization_missing_assets`，禁止静默退回到“按段落猜 speaker”。

## Diarization Manifest

每个源音频生成一个 manifest，路径跟随现有 ASR 输出结构：

```text
<data_dir>/asr/data/text/<task_id>/<relative>.diarization/
  manifest.json
  segments/
    seg_000001.wav
    seg_000002.wav
  samples/
    speaker_00.wav
    speaker_01.wav
```

Manifest schema：

```json
{
  "version": 1,
  "task_id": "task_001",
  "file_key": "source-key",
  "profile": "sherpa-onnx-balanced",
  "source_path": "/recordings/call.wav",
  "normalized_wav_path": null,
  "duration_ms": 128430,
  "speakers": [
    {
      "id": "speaker_00",
      "display_name": "用户A",
      "embedding_model": "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
      "embedding_path": "speaker-profiles/cache/speaker_00.embedding",
      "mapped_profile_id": null,
      "confidence": null
    }
  ],
  "segments": [
    {
      "id": "seg_000001",
      "speaker": "speaker_00",
      "speaker_display_name": "用户A",
      "mapped_profile_id": null,
      "start_ms": 1200,
      "end_ms": 5860,
      "overlap": false,
      "audio_path": "segments/seg_000001.wav",
      "asr_status": "pending",
      "asr_text": null
    }
  ]
}
```

注意：

- `normalized_wav_path` 不持久保存临时目录路径；如需要重跑 diarization，重新 normalize。
- `audio_path` 使用 manifest 相对路径，避免 data dir 移动后失效。
- speaker id 在单文件内稳定；跨文件同人合并只能通过未来 `mapped_profile_id` 完成。
- V1 可用 `用户A/用户B` 作为默认 display name，UI 允许重命名但不冒充真实身份。

## ASR 离线任务改造

### 1. process_pending_files 插入 stage

在 `process_pending_files()` 中，`normalize_to_temp()` 成功后进入：

```text
if task.diarization.enabled {
  update progress: diarizing
  manifest = run_diarization_stage(task, file, wav_path, source_info)
  update FileRecord.diarization_* fields
  asr_units = plan_asr_units_from_manifest(manifest)
  transcript = transcribe_diarized_units(asr_units)
} else {
  transcript = run_chunked_transcription(wav_path)
}
```

`run_progress.json` 增加 stage 字段：

```rust
pub stage: Option<String>, // discover | normalize | diarize | slice | asr | write_outputs | daily
pub current_speaker: Option<String>,
pub current_segment_id: Option<String>,
pub current_segment_done: usize,
pub current_segment_total: usize,
```

暂停规则：diarization、ffmpeg 切片、每个 ASR unit 完成后都检查 `pause_check()`；暂停时 FileRecord 回到可恢复状态，已写 manifest 保留，下一次运行可从 manifest 继续。

### 1.1 流式产物持久化边界

长音频和定时目录任务需要尽早暴露处理进展，但优化边界必须服从准确性：

- 不能把 full-file diarization / voiceprint matching 改成边听边猜。声纹识别、speaker embedding、speaker 稳定化和 `plan_asr_units()` 仍然基于完整 normalized WAV 的 diarization 结果，避免因为局部窗口导致角色重排、阈值漂移或同一个人被拆成多个身份。
- 可流式化的阶段是 speaker timeline 已确定之后的 ASR unit 转写。每个 diarized ASR unit 完成后，立即用当前累计 segments 写出 `.txt`、`.timeline.json`、`.srt`、`.vtt` 和 `.metadata.json`，并同步 `FileStore.output_*_path`、`text_chars`、`chunk_metrics`、`fallback_reason`。
- partial metadata 必须带 `partial=true`、`partial_started_at_ms` 和 `partial_segment_count`。最终任务成功时再写完整 artifact，并把 `partial` 状态替换为正式完成状态。
- 暂停、失败或页面刷新时，已经写出的 partial artifact 不能丢失。失败 FileRecord 必须保留 partial 路径和已输出字符数，让 WebUI、CLI 和恢复后的任务仍然能看到已经产出的片段。
- 上传文件的 `/api/asr/transcribe-stream` 在 full-file diarization 完成后，按 speaker-aware ASR unit 逐段推送 SSE `final` segment；不再等所有 segment 都转写完后一次性回放。最终 `done` 事件仍携带完整文本。
- 纯 ASR fallback chunking 也可以在每个 chunk 完成后写 partial artifact，但它只用于未启用或不可用 speaker-aware pipeline 的场景；启用 diarization 时不能绕过 speaker-aware unit planner。

### 2. ASR unit 规划

Diarization 原始 segment 可能太短、太密或重叠。V1 增加 `AsrAudioUnit`：

```rust
struct AsrAudioUnit {
    unit_id: String,
    speaker: String,
    speaker_display_name: String,
    mapped_profile_id: Option<String>,
    source_start_ms: u64,
    source_end_ms: u64,
    audio_path: PathBuf,
    source_segment_ids: Vec<String>,
    overlap: bool,
}
```

规划规则：

- 同 speaker 且间隔小于 `merge_same_speaker_gap_ms` 的相邻片段合并，减少 ASR 调用次数。
- 单个 unit 不超过当前 `ASR_TASK_SEGMENT_MAX_MS` 的 30 秒边界；超过则再按 speaker 内部切分。
- 小于 `min_duration_on_ms` 的短片段默认丢弃，除非它夹在同 speaker 两段之间可合并。
- overlapped speech V1 先保留 `overlap=true` 标记，ASR 仍处理主 speaker 片段；未来可追加 overlap 专门策略。

### 3. 复用现有 ASR runtime

不要为 diarization 另起一套 ASR 调用。实现时把当前 `run_chunked_transcription()` 拆成两层：

```text
transcribe_audio_unit(unit_audio_path, source_start_ms, unit_duration_ms)
transcribe_whole_normalized_file(wav_path)
```

`transcribe_audio_unit()` 继续使用现有 fork/server strategy、memory-limit hint、chunk metric、silence RMS、pause/retry 机制，只是在生成 timeline segment 时把返回的局部时间加回 `source_start_ms`，并附加 speaker 字段。

### 4. Timeline schema

`TimelineSegment` 增加：

```rust
pub(super) speaker: Option<String>,
pub(super) speaker_display_name: Option<String>,
pub(super) mapped_profile_id: Option<String>,
pub(super) diarization_segment_id: Option<String>,
pub(super) overlap: bool,
```

`TranscriptTimeline` 增加：

```rust
pub(super) diarization_profile: Option<String>,
pub(super) diarization_manifest_path: Option<PathBuf>,
pub(super) speakers: Vec<TimelineSpeaker>,
```

`render_timeline_text()` 改成：

```text
[00:00:01.200 - 00:00:05.860] 用户A: 你好，我想咨询一下订单。
```

Daily Docs 生成从 timeline 读取 speaker 字段：

```markdown
**[2026-05-27 10:00:01.200 -> 2026-05-27 10:00:05.860] 用户A**

你好，我想咨询一下订单。
```

如果 timeline 没有 speaker 字段，保持现有渲染，保证未启用 diarization 的任务不变。

### 5. Retry failed chunks

现有 retry 逻辑按 `FailedChunkRecord { offset_secs, duration_secs }` 从原始 source 重新提取 chunk。Diarization V1 需要新增：

```rust
pub diarization_segment_id: Option<String>,
pub speaker: Option<String>,
pub unit_audio_path: Option<PathBuf>,
```

Retry 优先从 manifest 的 segment/unit 重建临时 wav；如果 manifest 缺失但 source 仍存在，可按 offset fallback 重切原始音频并保留 speaker unknown。这个 fallback 只用于恢复用户数据，不作为正常执行路径。

## Admin API / CLI / WebUI

### 模型初始化原则

Diarization 和声纹模型必须复用 ASR 的“资产初始化”和“业务运行”分离原则：初始化入口负责下载、校验和自检；任务运行时只读取已准备好的本地资产。任务运行时如果发现 profile 缺失，必须把 FileRecord 标成可操作错误，并提示用户去 WebUI 或 CLI 初始化，不能在后台偷偷下载 gated 或大体积模型。

资产目录沿用 ASR home，避免另起一个用户难找的位置：

```text
~/.bifrost/asr/
  diarization/
    profiles/
      sherpa-onnx-balanced/
        profile.json
        segmentation/model.int8.onnx
        embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx
        fallback/nemo-titanet-small.onnx   # planned, not yet shipped as of 2026-06-16
      pyannote-community-quality/
        profile.json
        sidecar/
        model/
    speaker-profiles/
      profiles.json
      embeddings/
```

`sherpa-onnx-balanced` 初始化内容：

- 检查 sherpa-onnx runtime 是否可用；Rust crate 方案走构建依赖，外部 runtime 方案走本地 asset。
- 下载或导入 sherpa-onnx pyannote segmentation int8 模型，落到 `segmentation/model.int8.onnx`。
- 下载或导入 speaker embedding model，默认 `embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx`；`nemo-titanet-small.onnx` 等可选 fallback 属于后续阶段（planned, not yet shipped as of 2026-06-16）。
- 运行一段短音频 self-check，确认 segmentation、embedding、clustering 都能返回合法 manifest。

`pyannote-community-quality` 初始化内容：

- 明确要求用户接受 Hugging Face 条款并提供 token；token 只从 UI 输入的一次性请求体或 CLI 环境变量读取，不写日志、不写配置明文。
- 准备 Python sidecar venv / runtime、下载模型到本地目录、执行 self-check。
- 初始化完成只代表模型可用，不启动长期 sidecar；真正任务运行时按需启动并受并发/CPU/GPU 资源限制。

Speaker profile / voiceprint 初始化内容：

- 创建 `speaker-profiles/` 目录和 `profiles.json` 空索引。
- 校验当前默认 embedding model 可生成固定维度向量。
- 不要求已有用户声纹样本；enroll 是后续用户动作，不是初始化必需条件。
- 如果用户切换 embedding model，必须在 profile 状态里标出旧 profile 需要重建或不可混用。
- 声纹 profile 存储用户确认的 `display_name`、embedding centroid、sample 列表、embedding model 和更新时间；未达到匹配阈值时不能自动冒认真实身份。

V1 Admin API：

```http
GET  /_bifrost/api/asr/diarization/profiles
GET  /_bifrost/api/asr/diarization/status?profile=sherpa-onnx-balanced
GET  /_bifrost/api/asr/diarization/init-stream?profile=sherpa-onnx-balanced
GET  /_bifrost/api/asr/diarization/init-stream?profile=pyannote-community-quality
GET  /_bifrost/api/asr/tasks/{task_id}/files/{file_key}/diarization
PATCH /_bifrost/api/asr/tasks/{task_id}/files/{file_key}/speakers/{speaker_id}
POST /_bifrost/api/asr/speaker-profiles
```

已落地的声纹 profile / 实时朗读录入 HTTP 路由（实现在 `asr_jobs/diarization.rs` + `asr_jobs/voiceprint.rs`）：

```http
GET    /_bifrost/api/asr/speaker-profiles
POST   /_bifrost/api/asr/speaker-profiles
GET    /_bifrost/api/asr/speaker-profiles/{profile_id}
DELETE /_bifrost/api/asr/speaker-profiles/{profile_id}
POST   /_bifrost/api/asr/speaker-profiles/identify
POST   /_bifrost/api/asr/speaker-profiles/enrollment-sessions
POST   /_bifrost/api/asr/speaker-profiles/enrollment-sessions/{session_id}/audio
POST   /_bifrost/api/asr/speaker-profiles/enrollment-sessions/{session_id}/finish
DELETE /_bifrost/api/asr/speaker-profiles/enrollment-sessions/{session_id}
```

PATCH on a single speaker-profile，以及 `enroll-from-task-speaker` / `enroll-from-audio` / `/{profile_id}/samples` / `tasks/.../speakers/.../match-profile` 跨任务匹配 API 属于后续阶段（planned, not yet shipped as of 2026-06-16）。

`init-stream` 复用现有 `/api/asr/init-stream` 的 SSE 事件形态：`phase`、`message`、`detail`、`download`、`ready`、`error`。区别是 diarization 初始化的 owner 是 `diarization_model_management`，不占用 ASR server 租约。

Task create/update 请求增加：

```json
{
  "diarization": {
    "enabled": true,
    "profile": "sherpa-onnx-balanced",
    "known_speaker_count": 2,
    "voiceprint_matching": false
  }
}
```

CLI V1：

```bash
# 查看可用 profile 和资产状态
bifrost ai asr diarization profiles
bifrost ai asr diarization status --profile sherpa-onnx-balanced

# 初始化默认轻量 diarization / speaker embedding 模型
bifrost ai asr diarization init --profile sherpa-onnx-balanced

# 初始化高质量 sidecar；token 只从环境变量读取或交互输入
HF_TOKEN=... bifrost ai asr diarization init --profile pyannote-community-quality

# 运行已启用 diarization 的目录任务
bifrost ai asr task run <task-id>
bifrost ai asr task show <task-id>
bifrost ai asr task files <task-id>
```

已落地的声纹 profile 管理与实时朗读录入 CLI（实现在 `crates/bifrost-cli/src/cli.rs::AiAsrDiarizationSpeakerCommands`）：

```bash
bifrost ai asr diarization speakers list [--json]
bifrost ai asr diarization speakers show <profile-id> [--json]
bifrost ai asr diarization speakers enroll-live --name "Eden" \
    [--profile sherpa-onnx-balanced] [--phrase-seconds 4] [--device :0] [--json]
```

`speakers delete <profile-id>` 以及 `speakers match` / `speakers unmatch` 跨任务匹配 CLI 属于后续阶段（planned, not yet shipped as of 2026-06-16）。

V1 不新增单独 `bifrost audio diarize <file>` 作为验收入口；但 CLI 已经支持 diarization profile 的 `profiles/status/init`，并在 task/file 输出中展示 diarization status、speaker count、profile、speaker label。`import-audio` 只作为调试/迁移高级入口，不是默认录入体验；独立文件级 `bifrost audio diarize <file>` 放到 V1.5。

WebUI V1：

- ASR 页面顶部的模型管理区增加 `ASR Models` / `Speaker Diarization` 两个 tab 或两个并列卡片；`Speaker Diarization` 卡片展示 profile、资产状态、初始化进度和 self-check 结果。
- `Speaker Diarization` 卡片必须有 `Initialize` / `Refresh`，高质量 profile 需要显示 HF token 输入和条款提示；初始化完成后显示 `Ready`，但不启动 ASR service 或 pyannote sidecar 常驻进程。
- Directory Task 创建/编辑弹窗增加 `Speaker Diarization` 开关、profile 下拉、known/min/max speaker 输入。
- Task 文件列表增加 diarization status、speaker count、失败原因。
- Timeline 展示 speaker lane 或 speaker label；支持 speaker 重命名。
- ASR 页面顶部预留 `Speaker Profiles` 管理入口的产品位置；后续声纹录入必须通过“指定文本朗读 + 实时麦克风采集”向导完成，WebUI 复用浏览器麦克风模块，绑定已有 profile 和撤销绑定放到后续身份识别阶段。
- 亮色/暗色主题必须用现有 CSS 变量，不硬编码颜色。

### WebUI 交互改造详图

当前 ASR 页面入口在 `web/src/pages/ASR/index.tsx`，已经组合了 `SpeechTab`、`SpeechWorkbench`、`DirectoryTasksPanel` 和 `DirectoryTaskDetailPage`。Diarization V1 不新增独立页面，而是改造现有 ASR 页面结构。

#### 1. ASR 页面顶部模型资产区

位置：`web/src/pages/ASR/index.tsx` 当前直接复用 `SpeechTab` 展示 ASR Model Management。V1 应把该区域改成紧凑的双 tab 或双卡片：

```text
ASR
  Model Assets
    [ASR Models] [Speaker Diarization]
```

`ASR Models` 保持现有 ASR 初始化能力；`Speaker Diarization` 新增以下状态：

| 字段 | 来源 | 展示 |
| --- | --- | --- |
| Profile | `/api/asr/diarization/profiles` | Select，默认 `sherpa-onnx-balanced` |
| Asset Status | `/api/asr/diarization/status` | Tag: Missing / Installed / Ready / Error / Unsupported |
| Runtime | status response | `sherpa-onnx` / `pyannote-sidecar` |
| Segmentation Model | status response | 模型名、大小、checksum 状态 |
| Embedding Model | status response | 模型名、维度、checksum 状态 |
| Speaker Profiles | status response | profile 数、embedding model 是否匹配 |
| Self-check | status response | last run time、success/error detail |
| Actions | API | Refresh、Initialize、Open model folder |

高质量 `pyannote-community-quality` 选中后额外展示：

- Hugging Face token 输入框，使用 password input，只随初始化请求发送。
- 条款提示和用户确认 checkbox。
- CPU/GPU device 选择：`auto` / `cpu` / `cuda`，V1 macOS 默认 `cpu` 或 `auto`。

初始化 SSE 事件展示方式复用现有 `SpeechTab` 的 progress panel，但标题和文案改为 Speaker Diarization，事件流中展示下载项、self-check 阶段、profile ready 结果。初始化中不允许切换 profile；可以取消请求，但取消只停止前端等待，不保证已经开始的下载立即删除。

#### 2. Directory Task 创建/编辑弹窗

现有 task form 在 `web/src/pages/ASR/index.tsx` 通过 `taskForm` 管理，创建/更新走 `createAsrTask()` / `updateAsrTask()`。V1 在表单中新增一个 `Speaker Diarization` 分组，放在 Model / Language / Runtime Strategy 附近。

字段：

| 字段 | 控件 | 默认 | 说明 |
| --- | --- | --- | --- |
| Enable speaker diarization | Switch | off | 开启后任务先 diarization 再 ASR |
| Profile | Select | `sherpa-onnx-balanced` | 仅显示 status 非 unsupported 的 profile |
| Known speaker count | InputNumber | empty | 用户知道人数时传入，提升 clustering 稳定性 |
| Min speakers | InputNumber | empty | 和 known speaker count 互斥 |
| Max speakers | InputNumber | empty | 和 known speaker count 互斥 |
| Use speaker profiles | Switch | off | 未来声纹匹配；V1 开启时只尝试已存在 profile |
| Merge same speaker gap | InputNumber ms | 700 | 高级折叠项 |
| Min speech duration | InputNumber ms | 300 | 高级折叠项 |

表单校验：

- 开启 diarization 时，如果 selected profile status 不是 `ready`，表单顶部显示 warning，并提供 `Initialize profile` 链接跳到顶部 Speaker Diarization 卡片。
- `known_speaker_count` 与 `min_speakers/max_speakers` 互斥；`min_speakers <= max_speakers`。
- 未开启 diarization 时不提交 diarization advanced 字段，只提交 `{ enabled: false }`。

#### 3. Directory Task 列表与详情 summary

`DirectoryTasksPanel` 列表行增加轻量信息：

```text
Task name
Qwen3-ASR-1.7B · chinese · diarization: sherpa ready · 2 speakers last run
```

`DirectoryTaskDetailPage` 的 `Descriptions` summary 增加：

| Label | 内容 |
| --- | --- |
| Diarization | Off / `sherpa-onnx-balanced` / `pyannote-community-quality` |
| Diarization Assets | Ready / Missing / Error |
| Speakers | task summary 聚合的 distinct speaker count |
| Current Stage | `discover -> normalize -> diarize -> slice -> asr -> write_outputs -> daily` |

运行中时，当前顶部 run progress alert 要显示新阶段：

```text
Processing meeting.wav
Stage: Diarizing audio
Segments: 12 / 48
Current speaker: 用户A
```

`run_progress.json` 中新增的 `stage/current_speaker/current_segment_*` 字段必须透出到 task detail/watch API，WebUI 和 CLI TUI 都读同一份状态，不各自猜。

#### 4. Files 表格

`DirectoryTaskDetailPage` 的 Files tab 当前列为 File / Status / Text / Runtime / Recorded 等。V1 增加一列 `Diarization`，放在 Status 后、Text 前。

展示规则：

| 状态 | 展示 |
| --- | --- |
| disabled/skipped | 灰色 `Off` |
| pending | `Pending` |
| running | active progress：`segments 8/42` |
| success | 绿色 tag：`2 speakers`，副文本 profile |
| failed | 红色 tag + tooltip 展示 `diarization_error` |
| missing_assets | 黄色 warning，按钮 `Initialize profile` |

文件行 secondary actions 增加：

- `Open transcript` 保持现有。
- `Open diarization manifest`：调试/导出用，V1 可只下载 JSON。
- `Rename speakers`：打开当前文件 speaker 列表 modal。
- `Retry diarization`：只重跑 diarization + 后续 ASR，危险操作需确认会覆盖该文件现有 transcript。

#### 5. Transcript 文件详情页

`TaskFileTranscriptPage` 当前展示 Original Audio、File Timeline、Segments 和 Full Transcript。V1 增加 speaker-aware UI：

```text
Original Audio

Speakers
  用户A  00:12:31 total  42 segments  [Rename] [Enroll]
  用户B  00:08:10 total  37 segments  [Rename] [Enroll]

File Timeline
  [用户A] 00:00:01.200  你好...
  [用户B] 00:00:05.900  我这边看一下...
```

交互细节：

- Segment 行左侧显示 speaker tag，tag 使用 CSS variables / token color，不硬编码色值。
- 点击 speaker tag 可过滤当前 timeline，只看某个 speaker；再次点击清除过滤。
- `Rename` 写入 speaker display name，只影响 manifest/timeline display 字段，不改变 raw `speaker_00` id。
- `Enroll` 只有 embedding asset ready 时可用；V1 点击后创建 speaker profile 或追加样本，成功后 `mapped_profile_id` 绑定。
- Overlap segment 显示小型 `Overlap` tag；V1 不做复杂多轨编辑。
- Full Transcript 渲染为 `用户A: ...`，复制时保留 speaker label。

#### 6. Daily Docs / Daily Agent

Daily Docs tab 不新增复杂 UI，但 Daily markdown 内容必须从 speaker-aware timeline 渲染。Daily Agent 读取 Daily Docs 时自然获得 speaker label；如果后续 Daily Agent 需要结构化 speaker 信息，再从 timeline manifest 读取，不从纯文本反推。

#### 7. Web API TypeScript 类型

`web/src/api/asr.ts` 需要新增：

```ts
export interface AsrDiarizationConfig {
  enabled: boolean;
  profile?: string;
  known_speaker_count?: number;
  min_speakers?: number;
  max_speakers?: number;
  voiceprint_matching?: boolean;
}

export interface AsrDiarizationStatus {
  profile: string;
  engine: string;
  installed: boolean;
  ready: boolean;
  status: "missing" | "installed" | "ready" | "error" | "unsupported";
  models: Array<{ kind: string; name: string; path?: string; ready: boolean; checksum?: string }>;
  speaker_profiles: { count: number; embedding_model?: string; compatible: boolean };
  message?: string;
  detail?: string;
}

export interface AsrTimelineSpeaker {
  id: string;
  display_name: string;
  mapped_profile_id?: string;
  total_duration_ms?: number;
  segment_count?: number;
}
```

并扩展：

- `AsrDirectoryTask.diarization`
- `AsrTaskSummary.diarization_ready / diarization_running / speaker_count`
- `AsrTaskFileRecord.diarization_status / diarization_manifest_path / speaker_count / speaker_labels / diarization_error`
- `AsrTranscriptTimeline.speakers / diarization_profile / diarization_manifest_path`
- `AsrTimelineSegment.speaker / speaker_display_name / mapped_profile_id / diarization_segment_id / overlap`

新增 API functions：

```ts
listDiarizationProfiles()
getDiarizationStatus(profile)
streamDiarizationInitialization(profile, options, onEvent, signal)
getTaskFileDiarization(taskId, fileKey)
renameTaskFileSpeaker(taskId, fileKey, speakerId, displayName)
enrollTaskFileSpeaker(taskId, fileKey, speakerId, profileName)
```

#### 8. 暗色/亮色主题

所有新增 status tag、speaker tag、timeline lane、progress background 必须使用 antd token 或现有 CSS 变量。禁止新增硬编码 speaker 颜色数组；如果需要区分 speaker，使用 token 派生的低饱和边框和背景，并在 text label 上保证可读性。

### CLI 改造清单

当前 CLI ASR 命令定义在 `crates/bifrost-cli/src/cli.rs` 的 `AiAsrCommands` / `AiAsrTaskCommands`，实现位于 `crates/bifrost-cli/src/commands/asr.rs`。V1 增加一个 `diarization` 子命令，同时扩展现有 task 输出。

#### 1. 命令树

```text
bifrost ai asr diarization profiles [--json]
bifrost ai asr diarization status [--profile <PROFILE>] [--json]
bifrost ai asr diarization init --profile <PROFILE> [--hf-token-env HF_TOKEN] [--device auto|cpu|cuda] [--json]
bifrost ai asr diarization speakers list [--json]
bifrost ai asr diarization speakers show <PROFILE_ID> [--json]
bifrost ai asr diarization speakers delete <PROFILE_ID> [--yes]
```

V1 不做 standalone diarize 文件命令，但预留：

```text
bifrost ai asr diarization run <audio> --profile <PROFILE>   # V1.5
```

#### 2. task 命令扩展

`bifrost ai asr task list` 增加列：

```text
NAME  STATUS  MODEL  DIARIZATION  FILES  SPEAKERS  NEXT RUN
Call  idle    Qwen3  sherpa:ready  12/12  2         2026-05-28 02:00
```

`bifrost ai asr task show <task>` 增加：

```text
Diarization
  enabled: true
  profile: sherpa-onnx-balanced
  assets: ready
  speaker profiles: disabled
  last speakers: 2
  current stage: diarize segments 8/42
```

`bifrost ai asr task files <task>` 增加列：

```text
STATUS  DIARIZATION        SPEAKERS  TEXT  FILE
done    sherpa success     2         2048  meeting.wav
failed  missing assets     -         0     call.wav
```

`bifrost ai asr task watch/tui` 增加：

- stage line：`stage diarize | slice | asr | daily`
- current segment progress：`segments 8/42`
- current speaker：`用户A`
- diarization error panel：missing assets、profile unsupported、sidecar failed。

#### 3. JSON 输出兼容

所有新增 CLI JSON 字段直接透传 Admin API，不改变旧字段名。旧脚本只读原字段时不受影响；文本表格新增列属于可接受的人类输出变更。

#### 4. CLI 初始化安全边界

- `pyannote-community-quality` 的 token 默认从 `HF_TOKEN` 读取；`--hf-token-env NAME` 可指定环境变量名。
- CLI 不接受 `--hf-token <plain>`，避免 shell history 泄露。
- 初始化过程打印下载进度，但不打印 signed URL、token 或 Authorization header。
- `--json` 模式输出 NDJSON event，字段对齐 SSE：`phase/message/detail/download/ready/error`。

## 声纹实时录入与匹配（基础已落地，跨任务匹配为后续阶段）

截至 2026-06-16，已实现：默认 `sherpa-onnx-balanced` profile 的下载/自检、`AsrDiarizationConfig.voiceprint_matching` 默认 true、speaker-profile CRUD + identify + enrollment-sessions 系列 HTTP 路由、`speakers list/show/enroll-live` CLI、WebUI `DiarizationSetupCard` 与 directory task `Speaker Diarization` 表单分组、`asr-diarization-worker` 子进程承载 diarization / identify / finish-enrollment 推理。跨任务匹配 API、speaker-profile PATCH、显式 `speakers match/unmatch` CLI、pyannote-community-quality sidecar 实际推理路径属于（planned, not yet shipped as of 2026-06-16）。


声纹能力必须让 UI 和 CLI 都能完成基础声纹录入、删除和实时验证。默认录入体验不是上传音频文件，而是 Bifrost 下发指定文本，用户实时朗读，Bifrost 本地采集音频并提取声纹。没有录入 profile 时，Bifrost 只能展示匿名 speaker；录入并匹配后，后续处理才允许把 speaker 映射成用户确认的明确姓名。

数据结构：

```rust
pub struct SpeakerProfile {
    pub id: String,
    pub display_name: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub centroid_path: PathBuf,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub sample_count: usize,
    pub total_duration_ms: u64,
}
```

实时 Enroll 流程：

```text
user starts WebUI or CLI enroll-live
  -> server creates enrollment session
  -> server returns 5-8 prompted phrases
  -> user reads each phrase aloud
  -> WebUI browser mic or CLI voice helper streams local microphone audio
  -> VAD / level / clipping / duration checks
  -> extract speaker embedding with current embedding model
  -> create or append speaker profile
  -> recompute centroid
  -> future diarization maps speaker_00 -> profile_id when threshold passes
```

WebUI 采集要求：

- 使用浏览器 `getUserMedia`、AudioWorklet/ScriptProcessor 和现有 ASR 页面麦克风音频采集模块。
- 每句提示词单独录制，显示实时电平、录制时长、质量状态和重新录制按钮。
- 浏览器没有麦克风权限时提示用户授权，不降级成上传文件。

CLI 采集要求：

- `bifrost ai asr diarization speakers enroll-live --name "Eden"` 启动交互式朗读流程。
- CLI 通过 Bifrost Voice Input Runtime / `bifrost-voice-helper` 或同等本地录音 session 采集麦克风音频。
- CLI 显示每句提示文本、开始/停止录音提示、质量检查结果和最终确认。
- `import-audio ./eden.wav --confirm` 只作为调试/迁移入口，不是默认声纹录入方式。

匹配规则：

- 已录入 profile 且 `voiceprint_matching=true` 时，任务运行中对每个 speaker cluster 计算 embedding 并匹配 compatible profile centroid。
- 当前默认 speaker 声纹命中阈值为 `0.60`；达到阈值时自动写入 `mapped_profile_id`、profile `display_name` 和 `confidence`。
- 低于自动阈值但高于 suggestion 阈值时，只写 suggestion，不自动改名。
- `mapped_profile_id` 为空时仍视为未知角色，只能展示“用户A/用户B”或手动重命名。
- V1 禁止在没有录入 profile 或低置信度时自动声称“识别出张三”。
- WebUI/CLI 单文件上传的 speaker-aware 链路先按 sherpa-onnx speaker segment 切片；如果某个 speaker segment 超过 30 秒，必须在该 speaker segment 内继续按 30 秒上限、2 秒 overlap 分片后再调用 Qwen3-ASR，避免长音频连续说话段绕过服务端分片保护。上传 SSE 命中已录入声纹时必须同时输出 `speaker_profile_id` 和 `speaker_confidence`，最终 transcript 标签显示 `真实姓名 (匹配度% match)`，避免只展示匿名 speaker 与分数。
- 实时验证不能把短音频或静音直接当作 0% 未匹配；后端需返回 `insufficient_audio`，前端继续累计有效语音，并在识别前裁剪首尾静音，避免固定时长录音中的空白稀释 embedding。
- 参考 sherpa-onnx speaker identification 示例，同一用户的多句朗读不直接拼接成长音频，而是每句独立提取 speaker embedding 后做归一化平均，降低某一句录音质量波动对 profile centroid 的影响。

进程隔离要求：

- Admin 主进程只负责 API 编排、任务状态持久化、轻量音频格式校验和结果落盘；不得在主进程内加载 sherpa-onnx diarization 或 speaker embedding 模型。
- 离线 speaker diarization、speaker profile identify、实时 voice wake 声纹校验和 enrollment finish 都必须通过隐藏命令 `bifrost asr-diarization-worker --request <json>` 在独立子进程中执行。
- 子进程必须在命令参数中包含可观察的场景标识，例如 `asr-diarization-worker --request <json>`，便于 `ps args`、日志和 WebUI 状态区分 Admin 主进程与重模型推理 worker；不为了 Activity Monitor 展示名创建 symlink、hard link、copy 或额外 shim 可执行文件。
- 主进程与 worker 通过 `runtime/asr-diarization-worker/request-*.json` 交换持久化请求，worker stdout 只返回结构化 JSON；刷新页面、重启 WebUI 或恢复对话不影响已经写入的 ASR job 状态。
- 单元测试可以使用 in-process fallback，但生产路径和真实 CLI/WebUI 路径必须走 worker，避免声纹识别、diarization 或 enrollment 让 Admin 主进程承担模型内存和 CPU。

## 实现切分（实施顺序）

1. 数据结构与测试骨架：新增 profile config、manifest schema、真实模型 ready 检查、slicer/overlap 单元测试；`TimelineSegment` 加 speaker 字段且 serde default 不破坏旧 timeline。
2. 初始化闭环：新增 diarization profile registry、status API、init-stream API、CLI `diarization profiles/status/init`，WebUI ASR 页面展示 Speaker Diarization 初始化卡片。
3. 真实 sherpa-onnx profile：实现 model pack 下载/检查、CPU 线程限制、segmentation + embedding + clustering；失败时 FileRecord 记录明确错误，不静默退回无 speaker ASR。
4. Runner 接入 diarization-first 流程：`process_pending_files()` 在 ASR 前先运行 sherpa-onnx，按真实 speaker segments 切分 WAV，再逐分片送入 ASR，最终汇总 speaker-aware timeline/text/Daily Docs。
5. WebUI/API：Task 表单、文件列表、timeline speaker label、speaker rename API、profile prepare 状态。
6. 声纹录入闭环：推荐从已转录历史录音的 speaker-aware timeline 生成候选片段，用户试听并标注本人片段后生成多模板、多原型 profile；浏览器麦克风与 CLI voice helper 实时朗读保留为备选。历史录音必须通过 task/file 引用解析，不接受客户端传入的任意本地路径。
7. 高质量 sidecar：`pyannote-community-quality` 只做显式安装，token 不落日志，sidecar 通过本地进程或 local HTTP 调用。
8. Lab profile：只注册 external-command contract，不随默认发行下载权重。

## 测试方案

### 单元测试

- `manifest_round_trip_preserves_speaker_segments`：manifest schema 可序列化/反序列化，speaker、segment、relative audio_path 保持。
- `speaker_segments_merge_by_gap_and_split_by_max_duration`：同 speaker 小间隔合并，超过 30 秒再切分。
- `timeline_render_includes_speaker_label_when_present`：speaker-aware timeline 文本渲染包含 `用户A:`。
- `legacy_timeline_without_speaker_still_renders`：旧 timeline 不含 speaker 字段仍按现有格式渲染。
- `asr_unit_time_offsets_remap_to_source_timeline`：unit 局部 ASR 时间正确加回 source start。
- `diarization_pause_keeps_manifest_and_marks_record_pending`：暂停后可恢复，不丢 manifest。
- `diarization_status_reports_missing_assets_without_download`：任务运行时缺失模型只返回可操作错误，不隐式下载。
- `diarization_init_stream_reuses_asr_sse_event_contract`：初始化 SSE 事件包含 phase/message/download/ready/error。
- 后续声纹阶段追加 `speaker_profile_enroll_live_session_writes_centroid`：用户朗读指定文本后，录入 session 写入 centroid、sample index 和 display name。
- 后续声纹阶段追加 `speaker_profile_enroll_live_rejects_low_quality_phrase`：低音量、削波、静音过多或时长不足的句子被拒绝并要求重录。
- 后续声纹阶段追加 `speaker_profile_match_low_confidence_only_suggests`：低置信度只 suggestion，不自动回写明确姓名。
- 后续声纹阶段追加 `speaker_profile_unmatch_restores_anonymous_display`：撤销绑定后 timeline 回到匿名 speaker display。
- `speaker_voiceprint_identify_runs_in_diarization_worker`：实时 voice wake 和 `/speaker-profiles/identify` 都只通过 `asr-diarization-worker` 子进程执行 embedding 推理，Admin 主进程不直接调用 in-process identify。

### E2E

新增/更新 `e2e-tests/tests/test_asr_diarization_cli.sh`：

- 使用临时 `BIFROST_DATA_DIR`，启动服务必须带 `--no-system-proxy`。
- 调用 `/api/asr/diarization/status` 和 `/api/asr/diarization/init-stream` 真实初始化路径，断言状态从 missing 变为 ready。
- 执行 `bifrost ai asr diarization status/init --profile sherpa-onnx-balanced`，断言 CLI 能初始化并展示 ready。
- 断言 profile 目录中存在真实 `segmentation/model.int8.onnx` 和 `embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx`，且文件大小超过最低阈值。
- 创建启用 diarization 的 ASR Directory Task，触发 `/run`。
- 使用真实音频执行完整验证时，断言 `files.json` 包含 `diarization_status=success`、真实 speaker_count、manifest path。
- 断言 `.timeline.json` 每个 segment 包含 speaker 字段。
- 断言 `.txt` 和 `.daily/YYYY-MM-DD.md` 包含真实模型输出的 speaker label，例如 `用户A:`。
- 断言未启用 diarization 的任务输出与现有格式一致。
- 断言离线 diarization、声纹 identify、实时 voice wake 声纹校验和 enrollment finish 的 worker 请求均通过当前 `bifrost` 二进制的 `asr-diarization-worker --request <json>` 子进程执行，不创建额外 alias/link/copy 文件。
- 后续声纹阶段追加 WebUI 真实录入验证：浏览器打开 Speaker Profiles，按指定文本朗读 5-8 句，实时电平和质量检查通过后写入 `Eden` profile。
- 后续声纹阶段追加 CLI 真实录入验证：`bifrost ai asr diarization speakers enroll-live --name Eden` 通过本地 voice helper 采集麦克风，按指定文本朗读后写入 profile；后续启用 `voiceprint_matching=true` 的任务可在高置信度时自动展示 `Eden`。

### human_tests

新增 `human_tests/audio-diarization-asr.md` 并立即执行静态验收：

- 验证设计文档绑定当前 `run_directory_task`、`process_pending_files`、`normalize_to_temp`、`run_chunked_transcription`、`TranscriptTimeline`、`render_timeline_text` 路径。
- 验证设计包含双引擎 profile、pyannote sidecar、lab profile、V1 offline-only 边界。
- 验证设计包含“先 diarization，再切片 ASR，再 speaker-aware transcript”的两阶段流程。
- 验证设计包含 speaker profile / voiceprint 预留。
- 验证设计明确当前 MR 不交付真实身份识别；后续 UI/CLI 声纹录入必须通过指定文本实时朗读采集，WebUI 走浏览器麦克风，CLI 走本地 voice helper/录音 session；跨任务匹配、低置信度 suggestion 和撤销绑定属于后续阶段边界。

### 项目校验

后续代码实现提交前按顺序执行：

1. 相关 Rust 单元测试。
2. `e2e-tests/tests/test_asr_diarization_offline_task.sh`。
3. `human_tests/audio-diarization-asr.md` 全部用例。
4. `cargo test --workspace --all-features`。
5. `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
6. `rust-project-validate`。
7. 如改动覆盖 WebUI，追加 Web 单测和真实亮色/暗色 human_tests。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：确认方案覆盖模型选型、ASR 离线任务、speaker 切片、speaker-aware transcript、声纹预留。
- 变更范围复核：执行 `git status --short`、`git diff`，确认只改 design/human_tests/readme。
- 代码路径 review：检查方案引用的当前文件、函数、输出路径是否真实存在。
- 测试运行：执行 `human_tests/audio-diarization-asr.md` 中的静态验收命令。
- 修复：补齐遗漏的 ASR workflow、API、测试或 voiceprint 预留。

### 第 2 轮

- 再次目标复核：检查第 1 轮修复后的文档是否仍然是可执行实现计划。
- 再次变更范围复核：执行 `git status --short`、`git diff`，确认索引和用例数量一致。
- 再次 review：检查未把 pyannote/DiariZen/Sortformer误写成默认发行模型，未绕开现有 ASR runtime。
- 复跑测试：复跑静态验收命令。
- 结论：若没有新增阻塞问题，进入最终交付；若发现缺口，追加第 3 轮。

## 文档更新要求

- 本设计文档是后续实现的主计划；实现时必须保持它与代码、E2E、human_tests 同步。
- `human_tests/readme.md` 必须索引 `audio-diarization-asr.md`。
- 真正新增 CLI/API 后，再同步 README 或 CLI help reference。
