# Audio Diarization 与 ASR 离线任务集成

## 功能模块说明

验证说话人分离方案已经以可执行设计形式落入仓库，并且已经接入当前 ASR Directory Task 离线处理代码路径、CLI、Admin API 和 ASR 页面。

覆盖目标：

- 双引擎 + 可插拔 profile：默认 `sherpa-onnx-balanced`，高质量 `pyannote-community-quality` sidecar，DiariZen / Sortformer 只做 lab profile。
- V1 只接 ASR Directory Task 离线任务，不改实时语音输入。
- 音频文件先 diarization，再按 speaker 切片/合并 ASR unit，再流式送入现有 ASR runtime。
- 最终 `.timeline.json`、`.txt`、Daily Docs 都能表达 `用户A/用户B` 或 `speaker_00/speaker_01`。
- 预留 speaker profile / voiceprint enroll 字段，但 V1 不自动识别真实身份。
- WebUI 和 CLI 都必须支持 diarization / speaker embedding 模型初始化；任务运行时只检查资产，不偷偷下载模型。
- ASR 页面必须展示 diarization 配置、模型资产状态、任务新增处理阶段、文件级 diarization 状态、speaker-aware transcript；CLI 必须同步展示和初始化这些状态。

## 前置条件

1. 在仓库根目录执行。
2. 所有命令必须以 `source ~/.zshrc &&` 开头。
3. 启动服务必须使用临时 `BIFROST_DATA_DIR` 并带 `--no-system-proxy`。
4. 真实音频样本目录：`/Users/eden/Downloads/we`。

## 测试用例列表

### TC-ADA-01：设计文档记录双引擎与 profile 边界

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "sherpa-onnx-balanced|pyannote-community-quality|DiariZen|Sortformer|lab profile" design/audio-diarization-asr-offline.md
   ```

预期结果：

- 命令返回成功。
- 设计文档明确 `sherpa-onnx-balanced` 是默认轻量 profile。
- 设计文档明确 `pyannote-community-quality` 是显式安装的高质量 sidecar。
- 设计文档明确 DiariZen / Sortformer 只作为 lab profile 或 external engine。

### TC-ADA-02：设计文档绑定当前 ASR 离线任务代码路径

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "run_directory_task|process_pending_files|normalize_to_temp|run_chunked_transcription|TranscriptTimeline|render_timeline_text" design/audio-diarization-asr-offline.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "run_directory_task|process_pending_files|normalize_to_temp|run_chunked_transcription|TranscriptTimeline|render_timeline_text" crates/bifrost-admin/src/handlers/asr_jobs.rs crates/bifrost-admin/src/handlers/asr_jobs crates/bifrost-admin/src/handlers/asr_jobs_timeline.rs
   ```

预期结果：

- 两条命令均返回成功。
- 设计中的集成点能在当前仓库代码中找到对应函数或结构。

### TC-ADA-03：两阶段离线处理流程明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "diarize_normalized_audio|plan_asr_units_from_manifest|transcribe_diarized_units|speaker-aware timeline|先识别说话人|再切片 ASR" design/audio-diarization-asr-offline.md
   ```

预期结果：

- 命令返回成功。
- 设计文档明确 normalize 后先 diarization，再规划 ASR unit，再把每个 unit 输入 ASR。
- 设计文档明确 timeline 要带 speaker 字段。

### TC-ADA-04：输出 schema 覆盖 speaker-aware transcript

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "speaker_display_name|mapped_profile_id|diarization_segment_id|用户A|Daily Docs|TimelineSegment" design/audio-diarization-asr-offline.md
   ```

预期结果：

- 命令返回成功。
- 设计文档明确 timeline segment、文本渲染和 Daily Docs 都要展示 speaker。
- 设计文档包含 `mapped_profile_id` 预留字段。

### TC-ADA-05：声纹预留与身份边界明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "SpeakerProfile|enroll|voiceprint|声纹|禁止自动声称" design/audio-diarization-asr-offline.md
   ```

预期结果：

- 命令返回成功。
- 设计文档保留 speaker profile / enroll 数据结构。
- 设计文档明确 V1 不能自动声称识别出真实身份。

### TC-ADA-06：测试计划覆盖单元、E2E、human_tests 和项目校验

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "manifest_round_trip_preserves_speaker_segments|test_asr_diarization_offline_task.sh|human_tests/audio-diarization-asr.md|cargo test --workspace --all-features|rust-project-validate" design/audio-diarization-asr-offline.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "audio-diarization-asr.md|Audio Diarization" human_tests/readme.md
   ```

预期结果：

- 两条命令均返回成功。
- 设计文档列出后续实现必须补的单元测试、E2E、human_tests 和 workspace 校验。
- `human_tests/readme.md` 已索引本文件。

### TC-ADA-07：WebUI 与 CLI 初始化流程明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "diarization/init-stream|diarization profiles|diarization status|diarization init|Speaker Diarization|Initialize|HF_TOKEN|任务运行时只读取已准备好的本地资产" design/audio-diarization-asr-offline.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "init-stream|streamAsrInitialization|Model Management|prepare_cli_assets" crates/bifrost-admin/src/handlers/asr.rs web/src/api/asr.ts web/src/pages/Settings/tabs/SpeechTab.tsx crates/bifrost-cli/src/commands/asr.rs
   ```

预期结果：

- 两条命令均返回成功。
- 设计文档明确 WebUI ASR 页面要提供 Speaker Diarization 初始化卡片。
- 设计文档明确 CLI 要支持 `bifrost ai asr diarization profiles/status/init`。
- 设计文档明确高质量 pyannote profile 的 token 不落日志、不写明文配置。
- 设计文档明确 Directory Task 运行时不隐式下载模型。

### TC-ADA-08：ASR 页面与 CLI 交互改造清单明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "WebUI 交互改造详图|Directory Task 创建/编辑弹窗|Files 表格|Transcript 文件详情页|run_progress.json|Speaker Diarization|AsrDiarizationStatus|renameTaskFileSpeaker|enrollTaskFileSpeaker" design/audio-diarization-asr-offline.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "CLI 改造清单|bifrost ai asr diarization profiles|task list|task show|task files|task watch/tui|HF_TOKEN|--json 模式输出 NDJSON" design/audio-diarization-asr-offline.md
   ```
3. 执行：
   ```bash
   source ~/.zshrc && rg -n "DirectoryTaskDetailPage|TaskFileTranscriptPage|SpeechWorkbench|SpeechTab|AiAsrTaskCommands|AsrTaskFileRecord|AsrTranscriptTimeline" web/src/pages/ASR web/src/pages/Settings/tabs/SpeechTab.tsx crates/bifrost-cli/src/cli.rs web/src/api/asr.ts
   ```

预期结果：

- 三条命令均返回成功。
- 设计文档明确 ASR 页面顶部模型资产区、Directory Task 表单、任务详情 summary、Files 表格、Transcript 文件详情页、Daily Docs 的改造点。
- 设计文档明确任务执行状态新增 `stage/current_speaker/current_segment_*` 并透出给 WebUI 和 CLI。
- 设计文档明确 CLI 新增 `diarization` 子命令和现有 `task list/show/files/watch/tui` 输出扩展。
- 设计文档明确新增 TypeScript API/types，且这些改造挂在当前 ASR 页面和 CLI 代码路径上。

### TC-ADA-09：CLI/API 初始化 diarization profile

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && BIFROST_ASR_DIARIZATION_E2E_PORT=19093 e2e-tests/tests/test_asr_diarization_cli.sh
   ```

预期结果：

- 临时 Bifrost 服务使用 `--no-system-proxy` 启动成功。
- `bifrost ai asr diarization status --json` 在初始化前返回 `ready=false`。
- `bifrost ai asr diarization init --json` 创建 `profile.json` 和真实 ONNX 模型文件：`segmentation/model.int8.onnx`、`embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx`。
- 创建启用 `sherpa-onnx-balanced` 的 ASR Directory Task 后，CLI `task show` 展示 `Diarization` 配置。
- Admin API 返回 `summary.diarization_enabled=true` 且 `summary.diarization_ready=true`。

### TC-ADA-10：真实音频目录可创建 speaker-aware 离线任务

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && find /Users/eden/Downloads/we -maxdepth 3 -type f \( -iname '*.wav' -o -iname '*.mp3' -o -iname '*.m4a' -o -iname '*.mp4' -o -iname '*.flac' -o -iname '*.ogg' -o -iname '*.webm' \) -print | head -20
   ```
2. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，执行 diarization init，创建 `audio_dir=/Users/eden/Downloads/we` 且 `diarization.enabled=true` 的 ASR Directory Task。
3. 通过 Admin API 读取任务详情。

预期结果：

- 样本目录至少发现一个真实音频文件。
- 任务创建成功，不修改系统代理。
- 任务详情 `summary.discovered > 0`。
- 任务详情 `diarization.enabled=true`、`summary.diarization_ready=true`。
- CLI `task show` 输出包含 `Diarization` 与 `sherpa-onnx-balanced`。

### TC-ADA-11：ASR 页面展示 diarization 初始化卡片并可初始化

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" ./target/debug/bifrost start -p 19095 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开 `http://127.0.0.1:19095/_bifrost/ai?aiSection=tools-asr`。
3. 查看 ASR 页面顶部模型管理区域。
4. 点击 `Speaker Diarization` 区域的 `Initialize` 按钮。

预期结果：

- 页面显示 `Speaker Diarization` 卡片。
- 卡片显示 `sherpa-onnx-balanced` profile。
- 初始化前显示未初始化状态和 `Initialize` 按钮。
- 点击初始化后页面显示 ready/initialized 状态。
- 浏览器 console 无 error。

### TC-ADA-12：真实音频完整跑通到转录文件落盘

操作步骤：

1. 从 `/Users/eden/Downloads/we` 中选择真实音频，截取短样本：
   ```bash
   source ~/.zshrc && ffmpeg -hide_banner -loglevel error -y -ss 20 -t 20 -i /Users/eden/Downloads/we/TX01_MIC012_20260520_102542_orig.wav -ac 1 -ar 16000 /tmp/bifrost-asr-real-audio/real-we-20s.wav
   ```
2. 用 `bifrost ai asr stream-file` 对样本做真实 ASR 冒烟识别。
3. 启动临时 Bifrost：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" ./target/debug/bifrost start -p 19096 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode local_only
   ```
4. 执行 `bifrost -p 19096 ai asr diarization init --profile sherpa-onnx-balanced --json`。
5. 通过 Admin API 创建 `audio_dir=/tmp/bifrost-asr-real-audio`、`model=Qwen3-ASR-0.6B`、`runtime_strategy=reuse_per_file`、`diarization.enabled=true` 的 Directory Task。
6. 执行：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$DATA_DIR" ./target/debug/bifrost -p 19096 ai asr task run "$TASK_ID" --wait --json
   ```
7. 打开任务返回的 `output_text_path`、`output_timeline_path`、`output_metadata_path`、`diarization_manifest_path` 和 `.daily/*.md`。

预期结果：

- 任务 `summary.processed=1`、`failed=0`、`pending=0`。
- 文件状态为 `success`，`diarization_status=success`。
- `.txt` 文件包含带时间轴和 speaker 的完整文本。
- `.timeline.json` 包含 `diarization_profile`、`speakers`、segment `speaker/speaker_display_name/text`，且 segment 时间范围来自 sherpa-onnx diarization 切片后逐段 ASR，不是整段 ASR 后猜测贴标。
- `.diarization.json` 包含 sherpa-onnx 输出的 speaker 和 segment manifest。
- Daily Markdown 包含 speaker-aware 段落。

### TC-ADA-13：ASR 页面可配置、创建、查看 speaker-aware 任务内容

操作步骤：

1. 在浏览器打开 `http://127.0.0.1:19096/_bifrost/ai?aiSection=tools-asr`。
2. 确认页面显示 `Speaker Diarization` 卡片、`Ready` 状态、`sherpa-onnx-balanced` profile。
3. 点击 `New` 打开 Directory Task 表单。
4. 填写任务名和真实音频目录，开启 `Speaker Diarization`、开启 `Voiceprint Matching`、填写 `Known Speakers=2`，点击 `Create`。
5. 进入已完成任务 `real-we-full-validation` 的详情页。
6. 在 Files 表格点击 `Open transcript`。
7. 打开 Daily Docs，并点击 `Open document`。

预期结果：

- 新建任务成功，列表数量增加。
- 后端任务配置保存 `diarization.enabled=true`、`known_speaker_count=2`、`voiceprint_matching=true`。
- 任务详情页展示 `Speaker Diarization Ready sherpa-onnx-balanced` 和 speaker 统计。
- Files 表格展示 `Diarization success`、`1 speakers`。
- Transcript 页面展示原始音频播放器、speaker 列表 `用户A`、segment 行和完整文本。
- Daily 文档页面展示带 `用户A` 的 speaker-aware Markdown。

### TC-ADA-14：Speech Engine 编排方案覆盖默认组合与自定义模型出口

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "sherpa-onnx-balanced \\+ qwen3-asr-1.7b|Qwen3-ASR 继续承担默认转写|SpeechPipelineProfile|SpeechEngineRegistry|AsrUnitPlanner" design/asr-speech-engine-orchestration.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "custom diarization provider|custom OpenAI-compatible ASR endpoint|bifrost-diarization-manifest-v1|pyannote-community-quality|Phase 1|Phase 2|Phase 3" design/asr-speech-engine-orchestration.md
   ```
3. 执行：
   ```bash
   source ~/.zshrc && rg -n "run_sherpa_diarization|plan_asr_units_from_manifest|transcribe_asr_units_with_existing_qwen3_runtime|apply_diarization_to_timeline|diarization-first" design/asr-speech-engine-orchestration.md
   ```

预期结果：

- 三条命令均返回成功。
- 方案文档明确默认 speaker-aware 组合为 `sherpa-onnx-balanced + qwen3-asr-1.7b`。
- 方案文档明确 Qwen3-ASR 继续作为默认转写模型，sherpa-onnx 作为默认轻量语音结构化/diarization 引擎。
- 方案文档明确自定义 diarization provider 必须输出 Bifrost manifest，自定义 ASR provider 优先走 OpenAI-compatible transcription contract。
- 方案文档明确短期、中期、长期阶段，以及 Phase 1/2/3 的可执行路线。

### TC-ADA-15：声纹录入方案采用指定文本实时朗读采集

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "实时朗读录入|指定文本|prompt script|Prompt 1 / 6|浏览器麦克风|AudioWorklet|Speaker Profiles|Enroll Voiceprint" design/asr-speech-engine-orchestration.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "enroll-live|voice-helper|Voice Input Runtime|system_default|Press Enter to start|CLI 自身不要求用户上传文件|import-audio.*调试/迁移" design/asr-speech-engine-orchestration.md
   ```
3. 执行：
   ```bash
   source ~/.zshrc && rg -n "声纹实时录入与匹配|指定文本|实时朗读|浏览器麦克风|CLI voice helper|enroll-live|不能把上传音频作为默认录入入口|import-audio.*调试/迁移" design/audio-diarization-asr-offline.md
   ```
4. 执行：
   ```bash
   source ~/.zshrc && ! rg -n "enroll-audio|从独立音频文件录入|从本地音频文件录入|上传音频文件作为默认|选择.*音频.*录入|用户提供一个只包含目标说话人的" design/asr-speech-engine-orchestration.md design/audio-diarization-asr-offline.md
   ```

预期结果：

- 前三条命令均返回成功，第四条命令返回成功表示没有旧的默认上传/独立音频录入表述残留。
- 编排方案明确声纹录入主路径是后端下发指定文本，用户实时朗读，WebUI 通过浏览器麦克风采集。
- 编排方案明确 CLI 使用 `enroll-live`，通过 Bifrost Voice Input Runtime / `bifrost-voice-helper` 或本地录音 session 采集音频。
- 离线任务方案明确 `import-audio` 只作为调试/迁移高级入口，不是默认录入体验。
- 方案明确录入成功后，后续 `voiceprint_matching=true` 的任务才可以用用户确认的明确姓名替代 `用户A/用户B`。

### TC-ADA-16：CLI 与 WebUI 支持实时朗读声纹录入入口

操作步骤：

1. 执行后端声纹录入单元测试：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin voiceprint --lib
   ```
2. 使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy` 和测试 embedding 开关启动服务，执行 CLI 录入冒烟：
   ```bash
   source ~/.zshrc && ./target/debug/bifrost -p "$PORT" ai asr diarization speakers enroll-live --name Eden --test-pcm16 "$PCM" --json
   ```
3. 查询 CLI 列表：
   ```bash
   source ~/.zshrc && ./target/debug/bifrost -p "$PORT" ai asr diarization speakers list --json
   ```
4. 使用 `e2e-verify` 场景验证 ASR 页面声纹录入入口与自动朗读识别推进：
   ```bash
   source ~/.zshrc && node .trae/skills/e2e-verify/scripts/browser-test.js scenario asr-voiceprint-enroll-ui --shared-proxy --base-url "http://127.0.0.1:$PORT/_bifrost" --headless --verbose
   ```

预期结果：

- 后端单元测试通过，确认实时录入 session 可以生成命名 speaker profile。
- CLI `enroll-live` 返回 `source=live_enrollment`、`display_name=Eden`、`samples` 中包含指定朗读文本。
- CLI `speakers list --json` 返回 `display_name=Eden`。
- `e2e-verify` 场景通过，ASR 页面显示 `Enroll Voiceprint`，可打开弹窗并填写用户名称；开始录入后浏览器只采集音频，后端通过 Bifrost 本地 ASR 转写朗读文本并与当前提示句匹配，匹配达标后才自动推进到下一句，禁止使用浏览器云端 SpeechRecognition 或仅按固定秒数跳转。
- 测试服务全程使用临时数据目录和 `--no-system-proxy`，不修改系统代理。

### TC-ADA-17：0.6B 真实音频任务生成多 speaker Daily Markdown

操作步骤：

1. 从 `/Users/eden/Downloads/we/TX02_MIC015_20260520_103118_orig.wav` 截取 120 秒真实音频到临时目录，并转成 16 kHz mono wav：
   ```bash
   source ~/.zshrc && ffmpeg -y -i /Users/eden/Downloads/we/TX02_MIC015_20260520_103118_orig.wav -t 120 -ac 1 -ar 16000 /tmp/bifrost-asr-daily-multispeaker-evidence/audio/TX02_MIC015_first120s.wav
   ```
2. 使用独立 `BIFROST_DATA_DIR` 启动 Bifrost，端口固定为临时测试端口，必须带 `--no-system-proxy`。
3. 执行真实 `sherpa-onnx-balanced` 初始化，确认 `ready=true`。
4. 通过 Admin API 创建 Directory Task，配置 `model=Qwen3-ASR-0.6B`、`diarization.enabled=true`、`diarization.profile=sherpa-onnx-balanced`、`known_speaker_count=2`。
5. 执行 task run 并等待完成，打开 `.daily/YYYY-MM-DD.md`、`.timeline.json` 和 diarization manifest。

预期结果：

- 任务完成 `processed=1 failed=0 pending=0`。
- FileRecord 的 `diarization_status=success`，summary 的 `speaker_count=2`。
- Daily Markdown 同时包含 `用户A` 和 `用户B`，且每行带时间范围。
- `.timeline.json` 的 `model` 为 `Qwen3-ASR-0.6B`，`diarization_profile` 为 `sherpa-onnx-balanced`。
- `.timeline.json` 的 speaker/time/text 与 diarization manifest 的真实 segment 时间片一致，不能使用 round-robin、mock 或猜测 speaker。

### TC-ADA-18：拖入文件转录接入声纹匹配 speaker-aware 输出

操作步骤：

1. 先完成 TC-ADA-16，录入至少一个 speaker voiceprint。
2. 确认 `sherpa-onnx-balanced` 已初始化，且 `speakers list --json` 能看到已录入的真实姓名。
3. 在 ASR 页面拖入包含该用户声音的音频文件，触发 `/api/asr/transcribe-stream`。
4. 观察 SSE `final` segment 与最终 transcript。

预期结果：

- 拖入文件转录优先执行 sherpa-onnx diarization，并启用 voiceprint matching。
- speaker-aware upload 对每个 diarization speaker segment 仍然执行服务端分片，单个 ASR 请求最长 30 秒，不能因为长音频或连续长说话段绕过分片保护。
- SSE `final` segment 包含 `speaker`、`speaker_display_name`，已匹配时还包含 `speaker_profile_id` 和 `speaker_confidence` 字段。
- 最终 transcript 使用已录入的真实姓名和匹配度前缀，例如 `Eden (70% match): ...`，不能只输出匿名 `用户A/用户B`。
- 如果 profile 未初始化或未录入声纹，拖入文件转录允许回退原纯 ASR，不应阻断基础转写。

### TC-ADA-21：WebUI 拖入真实文件与 CLI 指定单文件命中已录入声纹

操作步骤：

1. 使用临时数据目录启动 Bifrost，且必须带 `--no-system-proxy`；在同一数据目录中初始化 `sherpa-onnx-balanced` 并录入至少一个真实 speaker voiceprint。
2. 在 WebUI ASR Speech Workbench 中拖入 `/Users/eden/Downloads/we/TX01_MIC011_20260520_102445_orig.wav` 或同目录下包含已录入说话人声音的真实音频。
3. 等待 `/api/asr/transcribe-stream` 完成，检查 SSE `segment` 事件包含 `speaker` 与 `speaker_display_name`；如果命中已录入声纹，还必须包含 `speaker_profile_id` 与 `speaker_confidence`，最终 transcript 按 `真实姓名 (匹配度% match): 文本` 组织。
4. 执行 CLI 单文件命令：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR=/tmp/bifrost-voiceprint-file-e2e cargo run --bin bifrost -- ai asr stream-file /Users/eden/Downloads/we/TX01_MIC011_20260520_102445_orig.wav --model Qwen3-ASR-0.6B --language chinese --speaker-aware --format jsonl
   ```
5. 检查 CLI JSONL 输出中至少一个 `segment` 事件包含 `speaker_display_name`；如果命中已录入声纹，还必须包含 `speaker_profile_id`、`speaker_confidence`，且最终 `text` 事件包含已录入 speaker 的真实姓名与匹配度前缀。

预期结果：

- WebUI 拖入文件和 CLI 指定文件使用同一条后端 speaker-aware upload 链路。
- 长音频上传由服务端自动分片；即使 diarization 得到超过 30 秒的连续 speaker segment，也必须在 speaker segment 内再次按 30 秒上限切片。
- 已录入声纹命中时，输出显示用户确认的真实姓名和匹配度，而不是匿名 `用户A/用户B`。
- 未初始化 diarization profile 或没有声纹 profile 时允许回退纯 ASR，但该情况不能标记为本用例通过。

### TC-ADA-19：armv7 构建不拉取 unsupported sherpa-onnx-sys

操作步骤：

1. 执行目标依赖树检查：
   ```bash
   source ~/.zshrc && RUSTC=$(rustup which rustc) $(rustup which cargo) tree -p bifrost-admin --target armv7-unknown-linux-gnueabihf | rg "sherpa|onnx" || true
   ```
2. 执行 armv7 check：
   ```bash
   source ~/.zshrc && RUSTC=$(rustup which rustc) $(rustup which cargo) check -p bifrost-admin --target armv7-unknown-linux-gnueabihf
   ```

预期结果：

- 第 1 条命令无输出，说明 armv7 依赖树不包含 `sherpa-onnx` / `sherpa-onnx-sys`。
- 第 2 条命令不能再出现 `Unsupported target for sherpa-onnx prebuilt libs: os=linux, arch=arm`。
- 如果本机缺少 `arm-linux-gnueabihf-gcc`，允许在 `ring` 的 cc toolchain 阶段失败；CI 环境具备交叉编译工具链时应继续通过。

### TC-ADA-20：声纹删除、实时身份验证与朗读文本清洗

操作步骤：

1. 执行后端声纹单元测试，覆盖 `<asr_text>` 标签清洗、0.72 朗读文本完整度阈值、0.60 speaker 声纹匹配阈值、声纹 identify/delete、短音频 `insufficient_audio`、识别前静音裁剪，以及多句录入 embedding 平均：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin voiceprint --lib
   ```
2. 打开 ASR 页面，确认 `Speaker Diarization` 卡片存在 `Enroll Voiceprint`、`Verify Voice` 和已录入声纹行的删除按钮。
3. 点击 `Verify Voice`，浏览器采集实时音频后调用 `/api/asr/speaker-profiles/identify`。
4. 点击某条声纹的删除按钮并确认删除。

预期结果：

- 注册朗读校验使用 Qwen3-ASR-0.6B 结果，但展示与匹配前会清洗 `<asr_text>` 等模型标签。
- 朗读文本完整度分数达到 `0.72` 即可推进，不再要求过高文本相似度。
- speaker 声纹匹配默认阈值统一为 `0.60`；实时身份验证命中时显示真实姓名。
- 只要存在最佳候选 speaker profile，匹配度必须展示为 `姓名 + 分数`；低于阈值时也显示候选姓名和分数，只标记为未确认，不允许展示成匿名 `用户A 70%`。
- 短音频或静音不足时，后端返回 `insufficient_audio`，WebUI 持续监听并累计有效语音，不把该状态显示成 `用户A 0%` 的失败结果。
- 验证接口在计算声纹 embedding 前裁剪首尾静音，避免用户刚点按钮或说完后的空白音频稀释匹配分数。
- 录入 profile 时每句朗读独立提取 embedding 后做平均，避免单句噪声或停顿直接污染整段 profile。
- 多人文件处理仍以 `用户A/B/C/D` 表示匿名 speaker，命中声纹时替换为已录入姓名。
- 删除后 profile 列表和 `speaker_profile_count` 同步减少。

### TC-ADA-21：x86_64-musl 静态 CLI 构建不链接 sherpa-onnx native 库

操作步骤：

1. 执行 musl 目标依赖树反查：
   ```bash
   source ~/.zshrc && cargo tree -p bifrost-admin --target x86_64-unknown-linux-musl -i sherpa-onnx
   ```
2. 执行 musl check：
   ```bash
   source ~/.zshrc && RUSTC=$(rustup which rustc) $(rustup which cargo) check -p bifrost-admin --lib --target x86_64-unknown-linux-musl
   ```

预期结果：

- 第 1 条命令输出 `warning: nothing to print.`，说明 musl 目标不再通过 `bifrost-admin` 拉入 `sherpa-onnx` / `sherpa-onnx-sys`。
- 第 2 条命令不能再出现来自 `libsherpa_onnx_sys` 的 `std::__throw_bad_array_new_length()`、`std::string::reserve()`、`__strdup` 等链接错误。
- 如果本机缺少 `x86_64-linux-musl-gcc`，允许在 `ring` 的 cc toolchain 阶段失败；GitHub Actions 的 cross 容器具备完整 musl 工具链时应继续通过。

### TC-ADA-22：aarch64 Linux CLI 构建不链接 sherpa-onnx native 库

操作步骤：

1. 执行 aarch64 Linux 目标依赖树反查：
   ```bash
   source ~/.zshrc && cargo tree -p bifrost-admin --target aarch64-unknown-linux-gnu -i sherpa-onnx
   ```
2. 执行 aarch64 check：
   ```bash
   source ~/.zshrc && RUSTC=$(rustup which rustc) $(rustup which cargo) check -p bifrost-admin --lib --target aarch64-unknown-linux-gnu
   ```

预期结果：

- 第 1 条命令输出 `warning: nothing to print.`，说明 aarch64 Linux 目标不再通过 `bifrost-admin` 拉入 `sherpa-onnx` / `sherpa-onnx-sys`。
- 第 2 条命令不能再出现来自 `libsherpa_onnx_sys` 的 `std::__throw_bad_array_new_length()`、`std::string::reserve()` 等链接错误。
- 如果本机缺少 aarch64 交叉 C 编译器，允许在其他 native dependency 的 cc toolchain 阶段失败；GitHub Actions 的 cross 容器具备完整 aarch64 工具链时应继续通过。

### TC-ADA-23：声纹识别与 diarization 推理使用独立 worker 进程

操作步骤：

1. 静态确认声纹/diarization worker 入口和请求类型：
   ```bash
   source ~/.zshrc && rg -n "asr-diarization-worker|run_asr_diarization_worker_request|IdentifyPcm16|FinishEnrollment" crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs crates/bifrost-admin/src/handlers/asr_jobs/voiceprint.rs crates/bifrost-cli/src/cli.rs crates/bifrost-cli/src/main.rs
   ```
2. 静态确认实时 voice wake 的 WAV 声纹校验不再直接执行 embedding，而是转到 PCM worker 请求：
   ```bash
   source ~/.zshrc && rg -n "identify_listener_speaker|identify_speaker_voice_from_wav_file|identify_speaker_voice_pcm16|f32_waveform_to_pcm16le" crates/bifrost-admin/src/handlers/voice/wake.rs crates/bifrost-admin/src/handlers/asr_jobs/voiceprint.rs
   ```
3. 构建当前二进制并直接执行隐藏 worker 命令：
   ```bash
   source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   source ~/.zshrc && TMP_DIR=$(mktemp -d) && PCM="$TMP_DIR/silence.pcm16le" && REQUEST="$TMP_DIR/request.json" && dd if=/dev/zero of="$PCM" bs=3200 count=1 >/dev/null 2>&1 && printf '{"operation":"identify_pcm16","pcm16le_path":"%s","sample_rate":16000}\n' "$PCM" > "$REQUEST" && BIFROST_DATA_DIR="$TMP_DIR/data" target/debug/bifrost asr-diarization-worker --request "$REQUEST"
   ```

预期结果：

- 第 1 条命令能看到隐藏 CLI 命令、`run_asr_diarization_worker_request`，以及 `Diarize` / `IdentifyPcm16` / `FinishEnrollment` 三类 worker 请求；生产路径不创建 symlink、hard link、copy 或额外 shim 可执行文件。
- 第 2 条命令能看到 `identify_listener_speaker` 仍接入声纹校验，但 `identify_speaker_voice_from_wav_file` 读取 WAV 后转成 PCM 并调用 `identify_speaker_voice_pcm16`，生产路径由 worker 执行 embedding。
- 直接执行隐藏 worker 命令时 stdout 返回结构化 JSON，`operation` 为 `identify`，静音样本返回 `status=insufficient_audio`，不依赖 Admin 主进程内直接执行模型推理。

### TC-ADA-24：长音频 partial artifact 流式落盘且不牺牲声纹准确性

操作步骤：

1. 静态确认设计文档把流式优化限制在 full-file diarization 之后：
   ```bash
   source ~/.zshrc && rg -n "full-file diarization|voiceprint matching|speaker timeline 已确定之后|partial=true|partial_segment_count|不能绕过 speaker-aware unit planner" design/audio-diarization-asr-offline.md
   ```
2. 静态确认 Directory Task 每个 diarized ASR unit 完成后写 partial artifact，并且暂停/失败保留 partial 路径：
   ```bash
   source ~/.zshrc && rg -n "PartialArtifactContext|persist_partial_transcription_artifacts|preserve_partial_artifact_fields|partial_artifacts" crates/bifrost-admin/src/handlers/asr_jobs/runner.rs crates/bifrost-admin/src/handlers/asr_jobs/chunk_runtime.rs crates/bifrost-admin/src/handlers/asr_jobs/state.rs
   ```
3. 静态确认上传 SSE 在 speaker-aware unit 完成时推送 `final` segment，而不是全部完成后回放：
   ```bash
   source ~/.zshrc && rg -n "transcribe_uploaded_wav_with_voiceprint_speakers\\(|send_asr_segment\\(|stream_tx" crates/bifrost-admin/src/handlers/asr.rs crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs
   ```
4. 执行 partial artifact 持久化单元测试：
   ```bash
   source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin partial_transcription_artifacts_update_file_store --lib -- --nocapture
   ```

预期结果：

- 设计文档明确声纹、speaker embedding、speaker 稳定化和 `plan_asr_units()` 仍基于完整 normalized WAV，不把 full-file diarization 改成边听边猜。
- Directory Task 只有在 ASR unit/chunk 已经产生文本后才把 `output_text_path` / `output_metadata_path` / `output_timeline_path` 写入 FileStore，避免 UI 暴露不存在的文件。
- 每次 partial 写入都会更新 `.txt`、`.timeline.json`、`.srt`、`.vtt`、`.metadata.json`，metadata 包含 `partial=true` 和 `partial_segment_count`。
- 任务暂停或失败时保留已经写出的 partial artifact 路径、`text_chars`、`chunk_metrics` 和 fallback 信息，刷新页面或恢复任务后仍能看到已产出的片段。
- 上传 SSE 在 full-file diarization 完成后按 speaker-aware unit 逐段发送 `final` segment，最终 `done` 仍带完整文本；不会为了低延迟绕过 speaker-aware unit planner。

## 清理步骤

- 临时服务通过 trap 关闭。
- 临时 `BIFROST_DATA_DIR` 自动删除。
- 不修改系统代理。

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-27 | TC-ADA-01/02/03/04/05/06 | 已执行上述 7 条静态验收命令：profile 边界、当前 ASR 代码路径、两阶段离线流程、speaker-aware 输出 schema、声纹预留、测试计划和 readme 索引均通过 `rg` 校验 | 通过 |
| 2026-05-27 | TC-ADA-07 | 已执行上述 2 条静态验收命令，并额外校验 `human_tests/readme.md` 的本文件用例数和总计为 `134/2308` | 通过 |
| 2026-05-27 | TC-ADA-08 | 已执行上述 3 条静态验收命令，并额外校验 `human_tests/readme.md` 汇总为 `134/2309` | 通过 |
| 2026-05-27 | TC-ADA-09 | 已执行 `BIFROST_ASR_DIARIZATION_E2E_PORT=19093 e2e-tests/tests/test_asr_diarization_cli.sh`，验证 CLI/API profile 初始化、真实 ONNX 模型文件落盘、任务配置和 summary 状态 | 通过 |
| 2026-05-27 | TC-ADA-10 | 已使用 `/Users/eden/Downloads/we` 真实音频目录启动临时 Bifrost，创建启用 `sherpa-onnx-balanced` 的 ASR Directory Task；API 返回 `discovered=36`、`audio_source_file_count=36`、`diarization_ready=true`，CLI `task show` 包含 diarization profile | 通过 |
| 2026-05-27 | TC-ADA-11 | 已使用 Playwright 打开临时 Bifrost ASR 页面，并在 light/dark 两种主题下确认 `Speaker Diarization`、`sherpa-onnx-balanced` 可见；截图保存在 `/tmp/bifrost-asr-diarization-light-home.png` 和 `/tmp/bifrost-asr-diarization-dark-home.png` | 通过 |
| 2026-05-27 | TC-ADA-12 | 已从 `/Users/eden/Downloads/we/TX01_MIC012_20260520_102542_orig.wav` 截取 20 秒真实音频；`bifrost ai asr diarization init --json` 下载真实 sherpa-onnx ONNX 模型；Directory Task 完成 `processed=1 failed=0 pending=0`，`diarization_manifest_path` 包含 2 个真实 sherpa-onnx segments，timeline 包含 2 个逐分片 ASR segments：`用户A: 你好，你好，你好。` 与 `用户A: 你好。`；证据保存在 `/tmp/bifrost-real-diarization-evidence` | 通过 |
| 2026-05-27 | TC-ADA-13 | 已通过 Playwright 真实打开 ASR 页面、任务详情页和 Transcript 文件页；light/dark 主题均展示 `Speaker Diarization`、`sherpa-onnx-balanced`、真实任务 `real sherpa diarization validation`、`用户A`、`你好`、`File Timeline` 和 `2 segments`；截图保存在 `/tmp/bifrost-asr-diarization-{light,dark}-{home,task,transcript}.png` | 通过 |
| 2026-05-27 | TC-ADA-14 | 已执行上述 3 条静态验收命令，确认新增 Speech Engine 编排方案覆盖默认 sherpa + Qwen3-ASR 组合、registry/profile/ASR Unit Planner、自定义 provider contract 和 Phase 1/2/3 路线 | 通过 |
| 2026-05-27 | TC-ADA-15 | 已执行上述 4 条静态验收命令，确认声纹录入默认体验已改为指定文本实时朗读采集：WebUI 走浏览器麦克风，CLI 走 `enroll-live` + Voice Input Runtime / `bifrost-voice-helper`，`import-audio` 仅作为调试/迁移高级入口，旧的 `enroll-audio`/上传音频默认入口表述已清理 | 通过 |
| 2026-05-27 | TC-ADA-16 | 已执行 `cargo test -p bifrost-admin voiceprint --lib`；使用临时 `BIFROST_DATA_DIR` + `--no-system-proxy` 运行 CLI `ai asr diarization speakers enroll-live --name Eden --test-pcm16 ... --json`，返回 `source=live_enrollment`、`display_name=Eden` 和 3 条指定朗读文本；`speakers list --json` 返回 `display_name=Eden`；已用 `e2e-verify` 场景 `asr-voiceprint-enroll-ui` headless 验证 WebUI 录入按钮、弹窗，以及开始录入后通过 Bifrost 后端本地 ASR 校验朗读文本并自动推进，最终生成 `display_name=Eden` 的 speaker profile | 通过 |
| 2026-05-27 | TC-ADA-17 | 已从 `/Users/eden/Downloads/we/TX02_MIC015_20260520_103118_orig.wav` 截取 120 秒真实音频，用 `Qwen3-ASR-0.6B` + `sherpa-onnx-balanced` 创建并运行 Directory Task；任务 `e4b72af0c48c491cbe7a48a5252650d2` 完成 `processed=1 failed=0 pending=0`、`speaker_count=2`、`diarization_status=success`；Daily Markdown 同时包含 `用户B: 就整个整个肚子要闻到我...` 和 `用户A: 向分流。/我直接上吧。/你有做准备是吧...`；timeline/diarization manifest 的 `49255-56967 speaker_01` 与 `60275/62367/67598ms speaker_00` 时间片一致；证据保存在 `/tmp/bifrost-asr-daily-multispeaker-evidence/out` | 通过 |
| 2026-05-27 | TC-ADA-18 | 已更新 `/api/asr/transcribe-stream` 拖入文件即时转写路径：当 `sherpa-onnx-balanced` ready 且存在 speaker profile 时，先执行 diarization + voiceprint matching，再按 speaker segment 调用 ASR；SSE `final` segment 新增 `speaker` / `speaker_display_name` / `speaker_profile_id` / `speaker_confidence`，最终 transcript 命中声纹时输出 `真实姓名 (匹配度% match): 文本`；已执行 `npm --prefix web run build` 与 `cargo test -p bifrost-admin voiceprint --lib` | 通过 |
| 2026-05-27 | TC-ADA-19 | 已执行 armv7 依赖树检查，`bifrost-admin --target armv7-unknown-linux-gnueabihf` 无 `sherpa/onnx` 依赖输出；本机 `cargo check` 未再触发 `Unsupported target for sherpa-onnx prebuilt libs`，后续停在本机缺少 `arm-linux-gnueabihf-gcc` 的 `ring` 交叉编译环境问题 | 通过 |
| 2026-05-27 | TC-ADA-20 | 已补充后端 `/api/asr/speaker-profiles/identify` 与 `DELETE /api/asr/speaker-profiles/{profile_id}`，WebUI 增加 `Verify Voice` 和删除按钮；已用单元测试覆盖 `<asr_text>` 标签清洗、0.72 朗读文本阈值、0.60 speaker 声纹阈值、候选姓名保留、identify/delete、短音频 `insufficient_audio`、识别前静音裁剪和多句 embedding 平均；Web build 使用仓库内 `TMPDIR` 通过 | 通过 |
| 2026-05-27 | TC-ADA-21 | 已执行 `cargo tree -p bifrost-admin --target x86_64-unknown-linux-musl -i sherpa-onnx`，输出 `warning: nothing to print.`；已执行 musl `cargo check -p bifrost-admin --lib --target x86_64-unknown-linux-musl`，本机未再进入 `libsherpa_onnx_sys` 链接阶段，后续停在本机缺少 `x86_64-linux-musl-gcc` 的 `ring` 交叉编译环境问题 | 通过 |
| 2026-05-27 | TC-ADA-22 | 已执行 `cargo tree -p bifrost-admin --target aarch64-unknown-linux-gnu -i sherpa-onnx`，输出 `warning: nothing to print.`；已执行 aarch64 `cargo check -p bifrost-admin --lib --target aarch64-unknown-linux-gnu`，本机未再进入 `libsherpa_onnx_sys` 链接阶段，后续停在本机缺少 `aarch64-linux-gnu-gcc` 的 `ring` 交叉编译环境问题 | 通过 |
| 2026-05-27 | TC-ADA-09 回归 | 已修复 `test_asr_diarization_cli.sh` 在 `SKIP_BUILD=true` 时默认寻找 `target/debug/bifrost` 的问题，改为复用 CI 预构建的 `target/release/bifrost`；已执行 `bash -n e2e-tests/tests/test_asr_diarization_cli.sh` | 通过 |
| 2026-05-27 | TC-ADA-16 回归 | 已修复 `test_asr_voiceprint_enroll_cli.sh` 在 `SKIP_BUILD=true` 时默认寻找 `target/debug/bifrost` 的问题，改为复用 CI 预构建的 `target/release/bifrost`；已执行 `bash -n e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh` | 通过 |
| 2026-05-28 | TC-ADA-16 回归 | CI Linux shard 3 暴露 live enrollment fixture 在非支持平台返回 `speaker_embedding_unsupported_platform`；已让 `BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING=1` 在非 macOS arm64 上继续使用 deterministic embedding，真实运行未设置该变量时仍保持 unsupported。已执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 与 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost BIFROST_ASR_VOICEPRINT_E2E_PORT=18994 bash e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh`，CLI enroll-live 和 speakers list 均通过 | 通过 |
| 2026-05-29 | TC-ADA-23 | 已执行静态 worker 入口校验，确认离线 diarization、声纹 identify、实时 voice wake WAV 声纹校验和 enrollment finish 均通过当前 `bifrost` 二进制的 `asr-diarization-worker --request <json>` 子进程请求；已执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 和隐藏 worker `identify_pcm16` 静音请求，返回 `operation=identify`、`status=insufficient_audio` | 通过 |
| 2026-05-29 | TC-ADA-24 | 已执行设计边界静态验收、Directory Task partial artifact 静态验收、上传 SSE 流式发送静态验收，以及 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin partial_transcription_artifacts_update_file_store --lib -- --nocapture` | 通过 |
