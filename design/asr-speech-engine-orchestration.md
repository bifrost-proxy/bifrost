# ASR Speech Engine 编排与可插拔语音模型方案

## 背景与结论

本方案基于当前 Bifrost 已有的 Qwen3-ASR 本地转写链路、Directory Task 离线任务、speaker diarization 草案，以及 sherpa-onnx / pyannote community-1 的能力调研，设计一套更清晰的语音处理编排层。

核心结论：

- 可行：sherpa-onnx 适合作为 Bifrost 默认轻量语音结构化引擎，负责 VAD、speaker diarization、speaker embedding 和后续可选 speech enhancement。
- 不替代：sherpa-onnx 不应该替代当前内置 Qwen3-ASR。它更适合做 ASR 前的音频理解与切段，Qwen3-ASR 继续承担默认转写。
- 默认策略：已有任务保持 diarization 默认关闭；新建 speaker-aware Directory Task 或用户显式启用 diarization 时，默认 profile 使用 `sherpa-onnx-balanced`。
- 高质量出口：`pyannote-community-quality` 作为显式安装的 Python sidecar profile，面向更高 diarization 质量；不随默认发行自动启用。
- 自定义模型出口：通过统一 `SpeechEngineRegistry + SpeechPipelineProfile` 支持用户自定义语音模型，而不是在 WebUI、CLI、Directory Task 里为每个模型复制一条业务路径。

一句话架构：

```text
Audio source
  -> normalize / optional enhancement / VAD
  -> diarization engine(sherpa by default, pyannote/custom optional)
  -> ASR unit planner
  -> ASR model(Qwen3 by default, Cohere/custom optional)
  -> speaker-aware timeline/text/Daily Docs/WebUI
```

## 当前 Bifrost 基线

当前分支已经具备以下基础：

- `crates/bifrost-admin/src/handlers/asr_jobs/state.rs`
  - `AsrDirectoryTask` 已包含 `model`、`language`、`runtime_strategy` 和 `diarization`。
  - `AsrDiarizationConfig` 已包含 `enabled/profile/min_speakers/max_speakers/known_speaker_count/voiceprint_matching`。
  - 默认 diarization profile 为 `sherpa-onnx-balanced`。
- `crates/bifrost-admin/src/handlers/asr_jobs/runner.rs`
  - Directory Task 已在 normalize 后进入转写流程。
  - 开启 diarization 时，`transcribe_diarized_segments_for_task()` 已是正式路径：先调 `run_sherpa_diarization`，再用 `plan_asr_units(&diarization_segments, &AsrUnitPlannerConfig::default())` 规划 ASR unit，最后逐 unit 切 WAV 送入 Qwen3-ASR `run_chunk_with_strategy`。
  - `bifrost-asr` 已提供 `plan_asr_units` / `AsrUnitPlannerConfig` / `SpeechPipelineProfile` / `builtin_speech_pipeline_profiles` 等抽象，本方案的多层编排已部分落地。
- `crates/bifrost-admin/src/handlers/asr_jobs_timeline.rs`
  - `TimelineSegment` 已有 `speaker`、`speaker_display_name`、`overlap`。
  - `TranscriptTimeline` 已有 `diarization_profile` 和 `speakers`。
  - 文本渲染已可输出 speaker label。
- `design/qwen3-asr-local-server.md`
  - Qwen3-ASR 负责本地 OpenAI-compatible ASR server、30 秒窗口、目录任务和 WebUI/CLI 入口。
- `design/asr-multi-model-local-service.md`
  - 已提出 ASR 模型 registry、默认模型配置和多 ASR provider 边界。
- `design/audio-diarization-asr-offline.md`
  - 已提出默认 sherpa-onnx、pyannote sidecar、DiariZen / Sortformer lab profile 的基本方向。

因此本方案不建议重写 ASR 模块，而是把已有方向收敛为一层统一的 Speech Pipeline 编排。

## 外部能力判断

### sherpa-onnx

官方 Rust crate 说明其提供 safe Rust bindings，覆盖 offline/streaming ASR、TTS、VAD、speaker embeddings and diarization、punctuation、denoising、audio tagging 等能力；默认静态链接，未设置 `SHERPA_ONNX_LIB_DIR` 时会下载匹配的预构建 runtime archive。

工程含义：

- 与 Rust 项目集成路径顺。
- 适合本地 CPU、桌面和端侧场景。
- 模型质量取决于 segmentation / embedding 模型组合，不应包装成“最强 diarization”。
- 构建期自动下载 runtime 对 Bifrost release/CI 不够理想，生产集成应优先显式管理 runtime/model assets，避免隐式网络行为。

sherpa-onnx 官方 diarization 文档列出 speaker segmentation 与 speaker embedding extraction 预训练模型，并提供 pyannote segmentation 3.0、3D-Speaker、NeMo 等组合。

### pyannote community-1

pyannote community-1 官方模型卡说明输入 16kHz mono audio，输出 speaker diarization，支持离线使用；同时需要用户接受条件并使用 Hugging Face token 下载。官方说明其相对旧版 3.1 改进了 speaker assignment/counting，并提供 exclusive speaker diarization，方便与 ASR timestamp 对齐。

工程含义：

- 质量上更适合作为高质量 profile。
- Python/PyTorch/Hugging Face 条款使其不适合做默认内置无感能力。
- exclusive speaker diarization 很适合后续高质量对齐模式。

### Qwen3-ASR

当前 Bifrost 内置 Qwen3-ASR 链路已经承担：

- 本地 ASR model assets 初始化。
- OpenAI-compatible `/v1/audio/transcriptions`。
- CLI `bifrost ai asr stream-file`。
- WebUI Speech Converter。
- Directory Task 30 秒窗口、pause/resume、runtime strategy、chunk metrics、Daily Docs。

工程含义：

- Qwen3-ASR 仍是默认 ASR text provider。
- sherpa-onnx 不需要先接入 ASR 能力；优先接入 VAD/diarization/embedding。
- speaker-aware 任务应复用当前 Qwen3-ASR server/fork-per-chunk/reuse-per-file 机制。

## 目标架构

### 分层

```text
业务入口
  WebUI Speech Converter
  WebUI Directory Tasks
  CLI bifrost ai asr
  Admin API /api/asr/*
        |
        v
Speech Pipeline 编排层
  SpeechEngineRegistry
  SpeechPipelineProfile
  AudioPreprocessStage
  DiarizationStage
  AsrUnitPlanner
  AsrTranscriptionStage
  SpeakerAlignmentStage
  TimelineWriter
        |
        v
Provider Adapter 层
  SherpaOnnxDiarizationProvider
  PyannoteSidecarDiarizationProvider
  ExternalDiarizationCommandProvider
  Qwen3AsrProvider
  CohereAsrProvider
  ExternalAsrProvider
        |
        v
Runtime / Model Assets
  sherpa-onnx + ONNX Runtime
  qwen3_asr_rs + MLX/Metal
  pyannote.audio + PyTorch
  custom command / local HTTP sidecar
```

分层原则：

- Directory Task、WebUI、CLI 只识别 pipeline profile，不直接拼模型文件、binary 名或 provider 私有参数。
- diarization engine 和 ASR model 分开注册，允许组合。
- 默认组合是 `sherpa-onnx-balanced + qwen3-asr-1.7b`。
- 自定义模型通过 adapter 接入，不侵入现有 task runner。

### Profile

新增概念 `SpeechPipelineProfile`：

```text
id: local-balanced-speaker-asr
label: Local balanced speaker ASR
preprocess:
  normalize: ffmpeg-16k-mono
  vad: sherpa-onnx-vad(optional)
  enhancement: off
diarization:
  engine: sherpa-onnx
  profile: sherpa-onnx-balanced
asr:
  model_id: qwen3-asr-1.7b
  runtime_strategy: reuse_per_file
alignment:
  mode: diarization-first
  merge_same_speaker_gap_ms: 800
  max_asr_unit_ms: 30000
```

高质量 profile：

```text
id: local-quality-speaker-asr
diarization:
  engine: pyannote-sidecar
  profile: pyannote-community-quality
asr:
  model_id: qwen3-asr-1.7b
alignment:
  mode: pyannote-exclusive-speaker-diarization
```

自定义 profile：

```text
id: custom-call-center-zh
diarization:
  engine: external-command
  command: /path/to/diarize
  output_schema: bifrost-diarization-manifest-v1
asr:
  model_id: custom-http-asr
  endpoint: http://127.0.0.1:18080/v1/audio/transcriptions
postprocess:
  command: /path/to/domain-corrector
```

## 推荐流水线

### 1. Normalize

继续复用当前 `normalize_to_temp()`：

```text
source audio
  -> ffmpeg
  -> 16kHz mono PCM WAV
```

这是 sherpa-onnx、pyannote community-1 和 Qwen3-ASR 都能接受的共同输入层。

### 2. Optional VAD / Enhancement

V1 可以先不启用 speech enhancement，但要把 stage 留出来：

```text
normalized wav
  -> optional VAD
  -> optional denoise/enhancement
```

原因：

- VAD 可减少 diarization 和 ASR 的无效计算。
- 对会议/通话强噪声场景，enhancement 可能显著影响 diarization 和 ASR。
- 这些能力 sherpa-onnx 已有本地实现，可逐步启用。

### 3. Diarization

默认使用 sherpa：

```text
normalized wav
  -> sherpa-onnx speaker diarization
  -> speaker_00 / speaker_01 segments
```

输出必须归一化成 Bifrost manifest，而不是把 sherpa 原始输出直接暴露给上层：

```json
{
  "version": 1,
  "engine": "sherpa-onnx",
  "profile": "sherpa-onnx-balanced",
  "source_sample_rate": 16000,
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
      "id": "seg_000001",
      "speaker": "speaker_00",
      "start_ms": 1200,
      "end_ms": 5860,
      "overlap": false
    }
  ]
}
```

### 4. ASR Unit Planner

这是当前草稿里最需要优雅化的一层。不要简单地“每个 diarization segment 调一次 ASR”，因为 speaker segment 可能过碎、过短、包含 backchannel 或重叠语音。

应增加 `AsrUnitPlanner`：

```text
diarization segments
  -> merge same-speaker nearby segments
  -> split units over max_asr_unit_ms
  -> skip or mark too-short silence/noise units
  -> preserve source segment ids
  -> produce ASR units
```

规划规则：

- 同 speaker、间隔小于 `merge_same_speaker_gap_ms` 的相邻 segment 合并。
- 单个 ASR unit 不超过当前 30 秒窗口，继续复用 Qwen3-ASR 的稳定边界。
- 重叠语音保留 `overlap=true`，V1 可选择主要 speaker；高质量 profile 可使用 pyannote exclusive diarization 降低对齐复杂度。
- 每个 unit 必须保留 `source_segment_ids`，方便 debug 和未来回放。

### 5. ASR Transcription

ASR provider 默认使用当前 Qwen3-ASR：

```text
ASR unit wav
  -> existing run_chunk_with_strategy()
  -> Qwen3-ASR server or fork mode
  -> WholeFileTranscription
```

复用现有能力：

- `reuse_per_file` 默认策略。
- `fork_per_chunk` 保守隔离策略。
- `auto/compare` 诊断策略。
- physical-footprint guard。
- chunk metrics / fallback reason。
- pause/resume / force pause。

关键边界：

- diarization engine 不负责文本。
- ASR model 不负责 speaker 身份。
- alignment stage 负责把二者归并到 timeline。

### 6. Timeline / Daily Docs / WebUI

最终输出继续用当前 `TranscriptTimeline`：

```json
{
  "diarization_profile": "sherpa-onnx-balanced",
  "model": "Qwen3-ASR-1.7B",
  "speakers": [
    {"id": "speaker_00", "display_name": "用户A"}
  ],
  "segments": [
    {
      "audio_start_ms": 1200,
      "audio_end_ms": 5860,
      "speaker": "speaker_00",
      "speaker_display_name": "用户A",
      "overlap": false,
      "text": "..."
    }
  ]
}
```

文本渲染：

```text
[00:00:01.200 - 00:00:05.860] 用户A: ...
```

Daily Docs 应直接消费 speaker-aware timeline，不再重新猜 speaker。

## 声纹录入与身份映射（后续阶段）

声纹录入是 speaker diarization 之后的身份映射层。当前 MR 不交付真实身份识别，只交付真实 sherpa diarization、speaker-aware ASR 输出和预留字段；模型仍然先输出 `speaker_00/speaker_01` 这样的匿名 speaker cluster。后续只有当用户显式录入了声纹 profile，并且当前文件中的 speaker embedding 与已录入 profile 达到阈值时，Bifrost 才把 `speaker_00` 映射为明确姓名。

目标体验：

```text
没有声纹 profile:
  speaker_00 -> 用户A
  speaker_01 -> 用户B

已录入声纹 profile:
  speaker_00 -> Eden
  speaker_01 -> 客户A

低置信度或冲突:
  speaker_00 -> 用户A
  suggestion: 可能是 Eden, confidence 0.71
```

### 数据模型

声纹 profile 必须独立于某一个 ASR task 存储，供后续任务复用：

```json
{
  "id": "spk_eden",
  "display_name": "Eden",
  "embedding_model": "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
  "embedding_dim": 512,
  "embedding_version": 1,
  "centroid_path": "speaker-profiles/spk_eden/centroid.f32",
  "samples": [
    {
      "id": "sample_001",
      "source": "task-file-speaker",
      "task_id": "task_x",
      "file_key": "file_y",
      "speaker_id": "speaker_00",
      "audio_start_ms": 1200,
      "audio_end_ms": 5860,
      "embedding_path": "speaker-profiles/spk_eden/samples/sample_001.f32",
      "duration_ms": 4660,
      "snr_hint": null,
      "created_at_ms": 1780000000000
    }
  ],
  "sample_count": 1,
  "total_duration_ms": 4660,
  "created_at_ms": 1780000000000,
  "updated_at_ms": 1780000000000
}
```

设计要点：

- `display_name` 是用户确认的名字，可以是人名、角色名或业务称呼。
- `centroid` 是同一人的多条样本 embedding 均值，后续匹配用 centroid，而不是只用最后一段样本。
- `embedding_model` 与 `embedding_dim` 必须持久化；模型切换后旧 profile 不能静默混用。
- 每个 sample 保留来源，方便用户删除误录样本、重建 centroid。
- `mapped_profile_id` 写入 timeline/manifest，`speaker_display_name` 使用 profile 的 `display_name`。

### 实时朗读录入

声纹录入的主路径必须是实时朗读指定文本，不是上传或选择一段现成音频。Bifrost 负责生成提示文本、采集用户朗读、检查录音质量、提取 embedding，再创建或追加 speaker profile。

推荐录入流程：

```text
create enrollment session
  -> server returns prompt script and phrase list
  -> user reads phrases aloud in WebUI or CLI
  -> client streams microphone PCM/WebM chunks to Bifrost
  -> server runs VAD / level / duration / clipping checks
  -> server extracts speaker embeddings per phrase
  -> server computes centroid and quality score
  -> user confirms display_name
  -> profile is created or appended
```

提示文本要求：

- 每次录入生成 5-8 句短文本，总朗读目标 20-40 秒。
- 中文默认文本覆盖常见声母、韵母、数字、英文短词和 Bifrost 业务词，例如“Bifrost 正在采集我的本地声纹样本”“今天是二零二六年五月二十七日”“请确认代理、录音、转写和说话人识别状态正常”。
- CLI 和 WebUI 使用同一个 prompt-set，由后端返回，避免两端提示不一致。
- 文本不需要作为声纹算法输入，但可用实时 ASR 做“是否大致读对”的可选质量检查。
- 用户可以重新录某一句；低音量、削波、静音过多、时长不足的句子不能进入 centroid。

保留但不作为默认产品路径：

- 从已处理文件 speaker 录入：用于“我已经确认这段 `用户A` 就是 Eden”的补救路径，需要二次确认。
- 从本地音频文件导入：只作为调试、迁移或无麦克风环境的高级入口，不出现在默认 WebUI/CLI 引导主流程中。

### 匹配策略

任务处理时的顺序应为：

```text
diarization segments
  -> extract embedding per speaker cluster
  -> load compatible speaker profiles
  -> compute similarity
  -> apply thresholds
  -> write mapped_profile_id/display_name/confidence
  -> transcribe ASR units
  -> render named timeline
```

阈值建议：

- `auto_match_threshold`: 默认 0.78，超过后自动绑定为 profile 名称。
- `suggestion_threshold`: 默认 0.68，只展示 suggestion，不自动覆盖 display name。
- `conflict_margin`: 默认 0.06，第一名和第二名差距小于该值时标记为 ambiguous。
- 单个 speaker 可用语音少于 3 秒时，不自动匹配，只给 suggestion 或保持匿名。

匹配结果写入 manifest：

```json
{
  "id": "speaker_00",
  "display_name": "Eden",
  "mapped_profile_id": "spk_eden",
  "confidence": 0.84,
  "match_status": "matched"
}
```

低置信度时：

```json
{
  "id": "speaker_00",
  "display_name": "用户A",
  "mapped_profile_id": null,
  "confidence": 0.71,
  "match_status": "suggested",
  "suggested_profile_id": "spk_eden",
  "suggested_display_name": "Eden"
}
```

### Admin API

新增或扩展 API（标注 shipped vs planned，as of 2026-06-16）：

```http
# shipped
GET    /api/asr/speaker-profiles
POST   /api/asr/speaker-profiles
GET    /api/asr/speaker-profiles/{profile_id}
PATCH  /api/asr/speaker-profiles/{profile_id}
DELETE /api/asr/speaker-profiles/{profile_id}
POST   /api/asr/speaker-profiles/identify
POST   /api/asr/speaker-profiles/enrollment-sessions
GET    /api/asr/speaker-profiles/enrollment-sessions/{session_id}
POST   /api/asr/speaker-profiles/enrollment-sessions/{session_id}/audio    # HTTP POST chunk
POST   /api/asr/speaker-profiles/enrollment-sessions/{session_id}/verify
POST   /api/asr/speaker-profiles/enrollment-sessions/{session_id}/finish

# planned, not yet shipped as of 2026-06-16
POST   /api/asr/speaker-profiles/enroll-from-task-speaker
WS     /api/asr/speaker-profiles/enrollment-sessions/{session_id}/audio-ws
POST   /api/asr/speaker-profiles/import-audio
POST   /api/asr/speaker-profiles/{profile_id}/samples
DELETE /api/asr/speaker-profiles/{profile_id}/samples/{sample_id}
POST   /api/asr/tasks/{task_id}/files/{file_key}/speakers/{speaker_id}/match-profile
DELETE /api/asr/tasks/{task_id}/files/{file_key}/speakers/{speaker_id}/match-profile
```

`enrollment-sessions` 请求：

```json
{
  "display_name": "Eden",
  "profile_id": null,
  "language": "zh-CN",
  "prompt_set": "default-zh-cn",
  "target_duration_ms": 30000,
  "source": "web_mic"
}
```

响应：

```json
{
  "session_id": "enroll_001",
  "display_name": "Eden",
  "prompt_set": "default-zh-cn",
  "phrases": [
    {"id": "p1", "text": "Bifrost 正在采集我的本地声纹样本。", "min_duration_ms": 2500},
    {"id": "p2", "text": "请确认代理、录音、转写和说话人识别状态正常。", "min_duration_ms": 3000}
  ],
  "audio_format": "pcm_s16le_16k_mono",
  "quality_requirements": {
    "min_total_speech_ms": 20000,
    "max_clip_ratio": 0.02,
    "min_rms": 0.015
  }
}
```

`finish` 响应：

```json
{
  "profile": {
    "id": "spk_eden",
    "display_name": "Eden",
    "sample_count": 6,
    "total_duration_ms": 28400
  },
  "quality": {
    "status": "accepted",
    "speech_ms": 28400,
    "phrase_count": 6,
    "rejected_phrase_count": 1
  }
}
```

`enroll-from-task-speaker` 仍保留为人工确认后的补救入口；`import-audio` 是高级导入入口，不作为默认录入 UX。

### WebUI

UI 必须有两个入口：

- ASR 顶部 `Speaker Profiles` 管理卡片：查看、创建、编辑、删除 profile，展示 sample 数、总时长、embedding model 兼容性。
- `Enroll Voiceprint` 向导：提示用户朗读后端下发的指定文本，通过浏览器麦克风实时采集音频。
- Transcript 文件详情页 speaker 列表：每个 speaker 行提供 `Rename`、`Match existing`、`Unmatch`；从该 speaker 片段创建 profile 只作为高级确认入口。

WebUI 录入向导：

```text
Speaker Profiles
  [Enroll Voiceprint]
  Display name: [Eden]

Prompt 1 / 6
  请朗读：Bifrost 正在采集我的本地声纹样本。
  [Start Recording] [Re-record phrase]
  live microphone level: ||||||||||
  quality: accepted

Prompt 2 / 6
  请朗读：请确认代理、录音、转写和说话人识别状态正常。
  ...

Finish
  speech 28.4s, accepted phrases 6/7
  [Create Profile]
```

实现要求：

- 复用当前 ASR 页面已有的浏览器麦克风采集模块：`getUserMedia` + AudioWorklet/ScriptProcessor，把音频转为 16k mono PCM 或 WebM chunks。
- WebUI 不展示“上传文件”作为声纹录入的默认按钮。
- 录制过程中显示实时电平、每句录制状态、质量检查结果和重录按钮。
- 浏览器权限失败时给出麦克风权限修复提示，而不是降级成上传文件。

录入成功后：

- profile 列表新增 `Eden`，显示 sample 数、总时长、quality。
- profile 列表必须支持显式删除；删除是本地生物特征数据的第一等操作。
- WebUI 提供实时语音验证入口：浏览器采集一小段当前说话声，调用 `speaker-profiles/identify`；命中声纹时显示真实姓名，未命中时显示匿名 `用户A`。多人文件处理继续按 diarization speaker cluster 显示 `用户A/B/C/D`，命中声纹后替换为真实姓名。
- 后续 `voiceprint_matching=true` 的 task 可以自动把命中的 speaker 显示为 `Eden`。
- 如果在 Transcript 页手动 `Match existing`，speaker tag 才会立即从 `用户A` 更新为 `Eden` 并重写 `.timeline.json` / `.txt`。

### CLI

后续 CLI 必须覆盖与 UI 等价的声纹录入能力：

```bash
# 管理 profile
bifrost ai asr diarization speakers list [--json]
bifrost ai asr diarization speakers show <profile-id> [--json]
bifrost ai asr diarization speakers rename <profile-id> --name "Eden"
bifrost ai asr diarization speakers delete <profile-id> --yes

# 实时朗读录入，默认使用 system_default 麦克风
bifrost ai asr diarization speakers enroll-live \
  --name "Eden"

# 指定设备或追加到已有 profile
bifrost ai asr diarization speakers enroll-live \
  --profile spk_eden \
  --device system_default \
  --prompt-set default-zh-cn

# 高级：从已处理文件 speaker 创建或追加样本，需要二次确认
bifrost ai asr diarization speakers enroll-from-task-speaker \
  --task <task-id> --file <file-key> --speaker speaker_00 --name "Eden" --confirm

# 高级：从本地音频导入，仅用于调试/迁移
bifrost ai asr diarization speakers import-audio ./eden.wav --name "Eden" --confirm

# 手动绑定或撤销当前文件里的 speaker
bifrost ai asr diarization speakers match \
  --task <task-id> --file <file-key> --speaker speaker_00 --profile spk_eden
bifrost ai asr diarization speakers unmatch \
  --task <task-id> --file <file-key> --speaker speaker_00
```

CLI `enroll-live` 交互：

```text
Display name: Eden
Microphone: System Default

Phrase 1/6:
  Bifrost 正在采集我的本地声纹样本。
Press Enter to start, speak after the beep, press Enter again to stop.
Quality: accepted, speech 3.2s, level ok

Phrase 2/6:
  请确认代理、录音、转写和说话人识别状态正常。
...

Create speaker profile Eden? [y/N]
```

CLI 采集要求：

- CLI 自身不要求用户上传文件。
- 优先复用 Bifrost Voice Input Runtime / `bifrost-voice-helper` 做麦克风采集、权限探测和设备选择。
- 没有 helper 时，CLI 可以启动前台本地录音 session；macOS 验证阶段可使用 CoreAudio/AVFoundation 或现有 voice runtime 的 PCM path。
- 采集的音频通过本地 API/WS 送入 enrollment session，与 WebUI 走同一后端质量检查和 embedding 提取逻辑。

CLI 文本输出应直接显示明确姓名：

```text
SPEAKER     NAME   PROFILE   CONFIDENCE  DURATION  SEGMENTS
speaker_00  Eden   spk_eden  0.84        12:31     42
speaker_01  -      -         -           08:10     37
```

`--json` 输出必须包含 `mapped_profile_id`、`match_status`、`confidence`、`suggestions`，方便脚本消费。

### 隐私与安全

- 声纹 profile 默认只存在本机 `~/.bifrost/asr/diarization/speaker-profiles/`。
- 不上传到云端、不自动同步、不写入 Daily Docs 原文之外的敏感 embedding。
- 导出 profile 必须是用户显式动作，并提示包含生物特征数据。
- 删除 profile 时必须同时删除 centroid、sample embedding 和 sample 音频缓存。
- task 中只保存 `mapped_profile_id/display_name/confidence`，不复制完整 embedding。
- 未达到阈值或有冲突时不能自动冒认；只展示 suggestion。

## 自定义模型接入

### 自定义 diarization provider

自定义 diarization model 只需输出 Bifrost manifest：

```text
external command:
  stdin/path: normalized wav
  stdout/file: bifrost-diarization-manifest-v1 JSON
```

最小 contract：

- 输入必须是 16kHz mono WAV 或由 provider 声明自带转换。
- 输出必须包含 speaker id、start_ms、end_ms。
- speaker id 只要求单文件内稳定。
- 不允许声称真实身份，除非显式使用已注册 voiceprint profile。

### 自定义 ASR provider

自定义 ASR model 建议先支持 OpenAI-compatible endpoint：

```text
POST /v1/audio/transcriptions
multipart file=<wav>
optional model/language/response_format
```

返回至少支持：

```json
{"text": "..."}
```

可选支持 verbose timestamp segments。若不支持 timestamp，Bifrost 用 ASR unit 的时间范围合成 timeline segment。

### 自定义 postprocess

postprocess 不应改变时间轴，只能修改文本或添加 metadata：

```text
speaker-aware timeline
  -> domain correction / punctuation / role hint
  -> corrected timeline
```

身份映射可以分三类：

- 手动重命名：`speaker_00 -> 客服`。
- 声纹注册：`speaker_00 -> profile_id -> Eden`。
- 文本/业务上下文推断：只能作为 suggestion，不能自动覆盖 speaker id。

## 与现有代码的收敛建议

### 短期

保留当前 `AsrDiarizationConfig`，但把执行路径从“直接每个 speaker segment 切片 ASR”收敛为：

```text
run_sherpa_diarization()
  -> build_diarization_manifest()
  -> plan_asr_units_from_manifest()
  -> transcribe_asr_units_with_existing_qwen3_runtime()
  -> write timeline / manifest / metadata
```

同时把 `apply_diarization_to_timeline()` 作为兼容 fallback：

- 新路径：diarization-first，先切 speaker ASR unit 再转写。
- 兼容路径：先整段 ASR 后用 diarization overlap 贴 speaker，只用于导入旧 timeline 或 provider 不支持切段时。

### 中期

引入 registry：

```rust
SpeechEngineRegistry {
    diarization_profiles: Vec<DiarizationProfile>,
    asr_profiles: Vec<AsrModelProfile>,
    pipeline_profiles: Vec<SpeechPipelineProfile>,
}
```

Admin API（部分已落地，部分 planned, not yet shipped as of 2026-06-16）：

```text
GET /api/speech/pipelines              # shipped (builtin_speech_pipeline_profiles)
GET /api/speech/pipelines/status       # shipped (profiles + runtime + resources)
GET /api/speech/decision                # shipped (resolve_engine_decision)
GET /api/speech/resources               # shipped
POST /api/speech/pipelines/{id}/init-stream   # planned
PATCH /api/speech/config/default-pipeline     # planned
```

Directory Task：

```text
model/language: 保留，用于兼容和 ASR 默认值
diarization: 保留，用于显式开关
pipeline_profile: 新增，可为空；为空时由 model + diarization 推导
```

### 长期

把 ASR、VAD、diarization、enhancement、punctuation 统一建模为语音能力插件：

```text
SpeechCapability:
  vad
  diarization
  speaker_embedding
  asr
  punctuation
  enhancement
  source_separation
```

这样 sherpa-onnx 可以作为多 capability provider，而不是只被当作 diarization 的内部库。

## WebUI / CLI 体验

### WebUI

在 ASR 页面模型管理区增加三层展示：

- ASR Model：Qwen3-ASR-1.7B / Qwen3-ASR-0.6B / custom。
- Speaker Engine：off / sherpa-onnx-balanced / pyannote-community-quality / custom。
- Pipeline Profile：balanced local / quality local / custom。

新建 Directory Task：

- 普通转写：默认 diarization off，保持现有成本与行为。
- Speaker-aware 转写模板：默认选 `sherpa-onnx-balanced + 当前默认 ASR model`。
- 已启用 speaker-aware 时，若 sherpa assets 未 ready，创建表单应提示 Initialize，不应在任务运行时偷偷下载。

文件详情页：

- 展示 speaker 列表。
- 支持重命名 speaker display name。
- 当前 MR 支持 speaker label 展示与重命名；从当前 speaker segments 录入声纹 profile、绑定已有 profile、自动身份命中放到后续阶段。
- 后续已录入 profile 命中后，timeline、文本和 Daily Docs 使用 profile 的明确姓名；未命中时继续显示 `用户A/用户B`。

### CLI

已落地：

```text
bifrost ai asr diarization profiles
bifrost ai asr diarization status
bifrost ai asr diarization init
bifrost ai asr diarization speakers list [--json]
bifrost ai asr diarization speakers show <profile-id> [--json]
bifrost ai asr diarization speakers enroll-live --name <name> [--profile <id>] [--phrase-seconds N] [--device :0]
```

后续声纹身份识别阶段扩展（planned, not yet shipped as of 2026-06-16）：

```text
bifrost ai asr diarization speakers rename <profile-id> --name "Eden"
bifrost ai asr diarization speakers delete <profile-id> --yes
bifrost ai asr diarization speakers enroll-from-task-speaker --task <task> --file <file> --speaker speaker_00 --name "Eden" --confirm
bifrost ai asr diarization speakers import-audio ./eden.wav --name "Eden" --confirm
bifrost ai asr diarization speakers match --task <task> --file <file> --speaker speaker_00 --profile spk_eden
bifrost ai asr diarization speakers unmatch --task <task> --file <file> --speaker speaker_00
```

后续 pipeline 入口（planned, not yet shipped as of 2026-06-16）：

```text
bifrost ai asr pipelines list
bifrost ai asr pipelines status
bifrost ai asr pipelines init local-balanced-speaker-asr
bifrost ai asr task create --speaker-aware
bifrost ai asr task create --pipeline custom-call-center-zh
```

CLI 输出要求：

- 机器可读模式输出 JSON/NDJSON。
- 不输出 Hugging Face token。
- 明确区分 ASR model ready 与 diarization engine ready。
- 当前 MR 明确区分匿名 speaker 和已手动命名 speaker；后续声纹阶段再区分已声纹匹配 speaker 和低置信度 suggestion。

## 可行性评估

### 适合默认做

- 离线 Directory Task。
- 通话录音、短音频、个人会议录音。
- 用户愿意接受匿名 speaker label。
- 对本地隐私、跨平台和轻量部署要求高。

### 不适合作为默认承诺

- 强噪声远场多人会议的高精度 diarization。
- 大量重叠说话的会议。
- 自动识别“谁是谁”。
- 未经用户录入或确认就自动声称真实身份。
- 不经用户确认自动下载大模型或 gated 模型。

### 主要风险

- sherpa-onnx Rust crate 默认构建期下载 runtime，需要改成 Bifrost 可控的资产初始化流程或锁定 release artifact。
- 当前草稿若逐个 speaker segment 调 ASR，可能产生过多短请求，影响速度和文本质量；必须加 ASR Unit Planner。
- pyannote community-1 有 Hugging Face 条款与 token，不能做静默默认依赖。
- 自定义 provider 如果 contract 太宽，会让 WebUI/CLI 很难解释状态；必须用 manifest 和 OpenAI-compatible ASR endpoint 收敛接口。
- speaker display name 与真实身份必须严格分离，避免产品误导。
- 声纹录入属于生物特征数据，必须保持本地存储、显式删除和导出提示。

## 推荐实施阶段

### Phase 1：默认轻量 speaker-aware Directory Task

- 使用 `sherpa-onnx-balanced`。
- 使用当前 Qwen3-ASR 默认模型。
- 增加 ASR Unit Planner。
- 输出 manifest、speaker-aware timeline、text、Daily Docs。
- WebUI/CLI 支持初始化和状态展示。

验收重点：

- 一个真实短音频能生成 speaker-aware timeline。
- 未启用 diarization 的任务行为不变。
- 启用 diarization 但资产缺失时明确失败，不隐式下载。

### Phase 2：高质量 pyannote sidecar

- 用户显式安装。
- 支持 Hugging Face token 和条款确认。
- 支持 exclusive speaker diarization 对齐模式。
- 与 Qwen3-ASR 组合输出同一 timeline schema。

验收重点：

- 同一音频可在 sherpa / pyannote profile 间切换。
- 输出 schema 不变。
- token 不进日志、不进 task JSON。

### Phase 3：自定义语音模型 profile

- 支持 external command diarization manifest。
- 支持 custom OpenAI-compatible ASR endpoint。
- 支持 profile import/export。
- 支持按任务选择 pipeline profile。

验收重点：

- 自定义 provider 不需要改 Directory Task 核心代码。
- 错误、进度、ready 状态可解释。

### Phase 4：跨任务自动声纹匹配

- 新任务开启 `voiceprint_matching=true` 时，自动加载兼容 speaker profiles。
- 对 speaker cluster 计算 embedding 并与 profile centroid 匹配。
- 高置信度自动绑定明确姓名；低置信度只展示 suggestion。
- WebUI/CLI 支持确认 suggestion、撤销错误绑定、追加样本重建 centroid。

验收重点：

- 同一人的第二个音频任务可自动命中已录入 profile。
- 低置信度样本不会被自动冒认为某个用户。
- 删除 profile 后新任务不再自动显示该姓名。

## 测试计划

### 单元测试

- `plan_asr_units_from_manifest_merges_same_speaker_gap`
- `plan_asr_units_from_manifest_splits_over_max_duration`
- `diarization_manifest_round_trip_preserves_speaker_segments`
- `pipeline_profile_resolves_default_qwen3_asr_with_sherpa_diarization`
- `custom_asr_provider_requires_openai_compatible_contract`
- 后续声纹阶段追加 `speaker_profile_enroll_from_task_speaker_creates_centroid`
- 后续声纹阶段追加 `speaker_profile_match_requires_compatible_embedding_model`
- 后续声纹阶段追加 `speaker_profile_low_confidence_only_suggests_name`

### E2E

- `e2e-tests/tests/test_asr_diarization_cli.sh`
  - 初始化 `sherpa-onnx-balanced`。
  - 创建启用 diarization 的 Directory Task。
  - 验证 API summary、CLI task show、manifest/timeline 字段。
- 后续声纹阶段新增 `e2e-tests/tests/test_asr_voiceprint_enrollment_cli.sh`
  - 用真实短音频 fixture 创建 speaker-aware task。
  - CLI 从 `speaker_00` enroll 为 `Eden`。
  - 验证 task file timeline、text 和 speaker profile index 都写入明确姓名和 `mapped_profile_id`。
- 新增后续 E2E：
  - custom external diarization command fixture。
  - custom OpenAI-compatible ASR fixture。
  - pyannote sidecar 缺 token / token accepted / offline cache 三类路径。

### human_tests

- 更新 `human_tests/audio-diarization-asr.md`：
  - 验证本方案文档包含默认 sherpa + Qwen3-ASR 组合。
  - 验证本方案文档包含 custom diarization provider 和 custom ASR provider contract。
  - 验证本方案文档明确当前 MR 不交付真实身份识别，只预留 UI/CLI 声纹录入、从 task speaker enroll、从独立音频 enroll 和跨任务匹配策略。
  - 验证不启用 diarization 的 Directory Task 不改变原行为。
  - 验证 speaker-aware task 的 WebUI/CLI 入口能解释 ASR model 与 speaker engine 两类状态。

### 项目校验

设计阶段只修改文档时：

- 必须执行文档检索型 human_tests。
- Rust fmt/clippy/workspace test 可标记为不适用，原因是未修改 Rust/WebUI/脚本运行行为。

进入实现阶段后：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash e2e-tests/tests/test_asr_diarization_cli.sh`
- 按 `human_tests/audio-diarization-asr.md` 逐条真实执行。
- 最后执行 `rust-project-validate`。

## 参考资料

- sherpa-onnx Rust crate: https://docs.rs/sherpa-onnx/latest/sherpa_onnx/
- sherpa-onnx speaker diarization docs: https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html
- pyannote community-1 model card: https://huggingface.co/pyannote/speaker-diarization-community-1
- pyannote community-1 release note: https://www.pyannote.ai/blog/community-1
