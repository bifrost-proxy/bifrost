# ASR Speech Pipeline Orchestrator 顶层方案

## 功能模块说明

验证 `design/asr-speech-pipeline-orchestrator.md` 已把实时语音、离线单文件字幕、ASR Directory Task、唤醒词、资源优先级和旧支持降级策略整理成完整可执行技术方案。

覆盖目标：

- 三条链路统一到 `Speech Pipeline Orchestrator`。
- 明确 `/api/voice/listen-ws` 是实时主链路，旧 `/api/asr/transcribe-ws` 只允许降级、迁移提示或下线，不再作为兼容服务。
- 离线单文件和 Directory Task 共用 `OfflineSubtitlePipeline`。
- `AsrUnitPlanner` 是第一优先级，负责合并、拆分、过滤 diarization segment。
- Directory Task 在离线文件 artifacts 生成后，不能丢失输出合并、Daily Docs、Daily Agent / AI Runner、report/IM/sync 后处理。
- ASR 核心能力必须从 `bifrost-admin` 抽到独立 `crates/bifrost-asr`，用包依赖和 feature matrix 控制平台编译。
- 旧 Directory Task 字段只做必要迁移推导，`transcribe-stream` preview 支持级别可降低，旧 timeline/text 不阻碍新 schema。
- 方案包含 API、CLI、WebUI、artifact schema、分阶段落地、测试计划和两轮 Review/Fix/Test。

## 前置条件

1. 在仓库根目录执行。
2. 所有命令必须以 `source ~/.zshrc` 开头。
3. 静态方案用例不需要启动服务。
4. 真实服务回归用例必须启动当前构建产物，使用临时 `BIFROST_DATA_DIR`，启动命令必须包含 `--no-system-proxy`。
5. 真实在线 ASR 产物用例需要 Apple Silicon 和已初始化的 Qwen3-ASR 模型资产；资产缺失时必须先执行 ASR 初始化，不能把缺失资产当通过。

## 测试用例列表

### TC-ASPO-01：顶层 Orchestrator 架构完整

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "Speech Pipeline Orchestrator|SpeechEngineRegistry|SpeechPipelineProfile|EngineDecisionResolver|ResourceLeaseManager|RealtimeVoicePipeline|OfflineSubtitlePipeline|DirectoryTaskPipelineAdapter" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确统一调度层的模块边界。
- 文档明确 Orchestrator 输出 `EngineDecision`。

### TC-ASPO-02：实时链路和旧 WebSocket 降级边界明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "/api/voice/listen-ws|Qwen3-ASR-0.6B|qwen3_stateful_streaming|deprecated_upload_like_realtime|/api/asr/transcribe-ws|410 gone|use_voice_listen_ws" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确 `/api/voice/listen-ws` 是唯一实时听写主链路。
- 文档明确旧 `/api/asr/transcribe-ws` 不再作为兼容服务，只允许返回迁移错误或下线。

### TC-ASPO-03：离线字幕和 Directory Task 共用 OfflineSubtitlePipeline

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "offline-speaker-subtitle-local|scheduled-speaker-subtitle-local|OfflineSubtitlePipeline::run_file|Directory Task 接入 OfflineSubtitlePipeline|/api/asr/offline-jobs|artifacts/srt|artifacts/vtt" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确离线单文件和 Directory Task 共用正式 pipeline。
- 文档明确 `offline-jobs` 是正式 artifact 接口，不复用 preview stream。

### TC-ASPO-04：AsrUnitPlanner 规则可执行

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "AsrAudioUnit|merge_same_speaker_gap_ms = 800|max_unit_ms = 30000|min_unit_ms = 500|min_rms = 0.008|source_segment_ids|overlap=true" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确 ASR Unit 的字段和 planner 的合并、拆分、过滤规则。
- 文档明确重叠语音 V1 保留标记但不猜第二人。

### TC-ASPO-05：旧任务字段迁移和产物优先级明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "pipeline_profile: Option<String>|diarization.enabled == false|offline-plain-asr-local|diarization.enabled == true|offline-speaker-subtitle-local|一次性迁移|旧脚本字段展示可以降低|.timeline.json|.diarization.json|TranscriptTimeline 是唯一数据源|旧 JSON shape 不作为长期兼容承诺" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确旧 Directory Task 不写 `pipeline_profile` 时只做迁移推导。
- 文档明确产品输出以新 timeline/subtitle schema 为准，旧 JSON shape 不作为长期兼容承诺。

### TC-ASPO-06：落地顺序、测试计划和 Review/Fix/Test 完整

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "Phase 1|Phase 2|Phase 3|Phase 4|Phase 5|resolve_realtime_dictation_uses_stateful_0_6b|test_asr_offline_jobs_artifacts.sh|test_scheduled_task_yields_to_realtime_voice.sh|Review/Fix/Test 闭环方案" design/asr-speech-pipeline-orchestrator.md
   ```
2. 执行：
   ```bash
   source ~/.zshrc
   rg -n "asr-speech-pipeline-orchestrator.md|ASR Speech Pipeline Orchestrator" human_tests/readme.md
   ```

预期结果：

- 两条命令均返回成功。
- 文档明确 Phase 1 到 Phase 5 的可执行顺序。
- 文档明确单元、E2E、human_tests 和项目校验计划。
- `human_tests/readme.md` 已索引本文件。

### TC-ASPO-07：Directory Task 后处理链路不被 OfflineSubtitlePipeline 吞掉

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "Directory Task 后处理契约|refresh_task_daily_summaries|maybe_enqueue_daily_agent_after_asr_run|DailyAgentChangePlanner|daily_agent_processed.json|report write|IM delivery|report sync|speaker-aware Daily Docs|独立并发锁" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确 `OfflineSubtitlePipeline` 只负责单文件标准产物。
- 文档明确 Directory Task runner 在 artifacts 落盘后仍要刷新 Daily Docs，并按既有 contract 排队 Daily Agent / AI Runner。
- 文档明确 Daily Agent 失败不回滚 ASR 文件成功状态，Runner 成功后才更新 `daily_agent_processed.json`。

### TC-ASPO-08：ASR 独立 crate 和跨平台编译边界明确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n "crates/bifrost-asr|bifrost-admin.*只保留适配层|不能直接依赖 `qwen3-asr`|不能直接依赖 .*`sherpa-onnx`|full-local-asr|cargo metadata --filter-platform|cargo tree -p bifrost-admin --target|asr_unavailable_in_this_build" design/asr-speech-pipeline-orchestrator.md
   ```

预期结果：

- 命令返回成功。
- 文档明确 ASR engine、pipeline、planner、subtitle writer、asset/profile decision 都属于 `bifrost-asr`。
- 文档明确 `bifrost-admin` 只保留 HTTP/API/任务状态和后处理适配层，不直接拥有 native ASR provider。
- 文档明确不同平台通过是否依赖 `bifrost-asr` 以及启用哪些 feature 决定编译方案。
- 文档明确后续实现必须用 Cargo metadata/tree 验证不支持平台不解析 `qwen3-asr` / `sherpa-onnx`。

### TC-ASPO-09：真实服务回归覆盖 ASR Speech Pipeline 全入口

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh
   ```
2. 在 CI 或非 Apple Silicon 环境下，脚本只验证真实服务的 orchestrator/API/wake/legacy 路径；如需执行真实 Qwen3-ASR 产物链路，执行：
   ```bash
   source ~/.zshrc && BIFROST_ASR_PIPELINE_E2E_ONLINE=1 bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh
   ```

预期结果：

- 脚本启动临时 Bifrost 服务，启动参数包含 `--no-system-proxy`。
- `/api/speech/pipelines/status` 返回 realtime/offline/scheduled profiles 和 resource snapshot。
- `/api/speech/decision?mode=realtime_dictation` 返回 `qwen3_stateful_streaming + Qwen3-ASR-0.6B`。
- `/api/speech/decision?mode=offline_file&speaker_aware=true` 返回 `offline-speaker-subtitle-local`、diarization decision 和默认 `Qwen3-ASR-0.6B`。
- `/api/asr/transcribe-ws` 返回 410，并提示使用 `/api/voice/listen-ws`。
- wake phrase-only 配置可以启动 lightweight listener，且不会启动后台 ASR worker pid。
- 当 `BIFROST_ASR_PIPELINE_E2E_ONLINE=1` 且本地 Qwen3-ASR 资产可用时，`POST /api/asr/offline-jobs` 对真实语音音频生成 `txt/srt/vtt/timeline_json/metadata` artifacts。
- 当在线 ASR 产物链路启用时，`bifrost ai asr subtitle` 通过正式 offline-jobs API 下载同一组 artifacts。
- 当在线 ASR 产物链路启用时，Directory Task 对同一真实语音音频生成 artifacts，并且 Daily Agent 配置接口仍可访问，后处理入口未丢失。

### TC-ASPO-10：Admin ASR 业务逻辑迁移到 `bifrost-asr`

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n "pub mod (decision|resources|planner|offline|subtitle|timeline|artifacts|profiles)" crates/bifrost-asr/src/lib.rs
   ```
2. 执行：
   ```bash
   source ~/.zshrc && rg -n "resolve_engine_decision|ResourceLeaseManager|write_offline_subtitle_artifacts|plan_asr_units|render_srt|render_vtt" crates/bifrost-asr/src
   ```
3. 执行：
   ```bash
   source ~/.zshrc && rg -n "bifrost_asr::(decision|resources|offline|planner|subtitle|timeline|profiles)" crates/bifrost-admin/src/handlers
   ```

预期结果：

- `bifrost-asr` 暴露 decision/resources/planner/offline/subtitle/timeline/artifacts/profiles 等 ASR 业务模块。
- engine decision、资源租约、字幕产物写入、ASR Unit Planner、SRT/VTT writer 均位于 `bifrost-asr`。
- `bifrost-admin` 通过 `bifrost_asr::*` 调用核心业务，只保留 HTTP、任务状态和 Directory Task 后处理适配。

### TC-ASPO-11：旧实时 ASR WebSocket 不再作为兼容服务

操作步骤：

1. 启动临时服务：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18998 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 另开命令执行：
   ```bash
   source ~/.zshrc && curl -sS -o /tmp/bifrost-asr-legacy-ws.txt -w "%{http_code}" http://127.0.0.1:18998/_bifrost/api/asr/transcribe-ws
   ```

预期结果：

- HTTP 状态码为 `410`。
- 响应正文包含 `/api/voice/listen-ws`。
- 服务日志不出现旧 `AsrRealtimeBuffer` 全会话重转码链路被启动的记录。

### TC-ASPO-12：Directory Task 后处理没有被单文件 Offline Pipeline 覆盖掉

操作步骤：

1. 使用 TC-ASPO-09 脚本完成真实 Directory Task 转写。
2. 检查脚本输出目录和接口响应：
   ```bash
   source ~/.zshrc && rg -n "daily-agent|artifacts|timeline_json|metadata" e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh
   ```

预期结果：

- Directory Task 文件级 artifacts 可通过 `/api/asr/tasks/{task_id}/files/{file_key}/artifacts/{format}` 读取。
- Daily Agent 配置接口仍返回有效 JSON。
- OfflineSubtitlePipeline 只负责单文件标准产物，Directory Task runner 仍负责 Daily Docs、Daily Agent / AI Runner、report/IM/sync 后处理。

### TC-ASPO-13：WebUI 使用正式 offline-jobs 生成文件字幕

操作步骤：

1. 启动临时服务：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 18999 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 在浏览器打开 `http://127.0.0.1:18999/_bifrost/`。
3. 进入 AI / ASR 页面。
4. 在 Speech to Text 区域选择真实音频文件上传。
5. 等待文件转写完成。

预期结果：

- 页面展示 offline subtitle job 进度。
- 完成后 transcript 区域展示文本。
- 页面出现 `txt/srt/vtt/timeline_json` 下载按钮。
- 顶部 pipeline 状态显示 realtime/offline resource 状态，不要求用户手动调用旧 streaming preview 接口。
- 亮色和暗色主题下状态、按钮、进度和下载区域均可读，无重叠。

### TC-ASPO-14：ASR runtime 跨 Workbench / Workflows 共享且默认 0.6B

操作步骤：

1. 启动临时服务，并在 WebUI 手动启动 `Qwen3-ASR-0.6B`。
2. 执行：
   ```bash
   source ~/.zshrc && curl -fsS "http://127.0.0.1:19010/_bifrost/api/asr/service/start?model=Qwen3-ASR-0.6B&owner_module=voice_wake_listener"
   ```
3. 执行：
   ```bash
   source ~/.zshrc && curl -fsS "http://127.0.0.1:19010/_bifrost/api/speech/decision?mode=offline_file&speaker_aware=true"
   ```
4. 执行：
   ```bash
   source ~/.zshrc && curl -fsS "http://127.0.0.1:19010/_bifrost/api/asr/service/stop?model=Qwen3-ASR-0.6B&owner_module=voice_wake_listener"
   ```

预期结果：

- 第 2 步返回 `ready: true`，不会因为 active owner 是 `speech_workbench` 而返回 409。
- 第 3 步返回默认 `Qwen3-ASR-0.6B`，不会默认要求 `Qwen3-ASR-1.7B`。
- 第 4 步可以停止同一个共享 ASR runtime，不会因为 owner 不一致留下 stale state。

### TC-ASPO-15：Directory Task 新建默认开启说话人和声纹匹配

操作步骤：

1. 启动真实服务：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p 19010 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 只传 `name/audio_dir` 调用后端创建任务：
   ```bash
   source ~/.zshrc && TMP_DIR="$(mktemp -d)" && curl -sS -X POST http://127.0.0.1:19010/_bifrost/api/asr/tasks -H 'content-type: application/json' --data-binary "{\"name\":\"defaults-smoke\",\"audio_dir\":\"$TMP_DIR\"}"
   ```
3. 通过 CLI 创建任务：
   ```bash
   source ~/.zshrc && TMP_DIR="$(mktemp -d)" && cargo run --bin bifrost -- -p 19010 ai asr task create --name cli-defaults-smoke --dir "$TMP_DIR" --json
   ```
4. 用浏览器打开 `http://127.0.0.1:19010/_bifrost/ai?aiSection=tools-asr`，点击 Directory Tasks 的 `New`。

预期结果：

- API 创建任务默认返回 `model=Qwen3-ASR-0.6B`。
- API 创建任务默认返回 `diarization.enabled=true`、`diarization.voiceprint_matching=true`、`profile=sherpa-onnx-balanced`。
- CLI 创建任务默认返回同样的 0.6B、diarization enabled、voiceprint matching enabled。
- WebUI 新建任务弹窗默认展示 `Task Model=Qwen3-ASR-0.6B`，`Speaker Diarization` 与 `Voiceprint Matching` 开关均为打开。

### TC-ASPO-16：未知说话人数时不会把短句炸成大量用户

操作步骤：

1. 使用真实服务创建一个 speaker-aware Directory Task，不填写 `known_speaker_count`，只传 `max_speakers=4` 或使用产品默认：
   ```bash
   source ~/.zshrc && TMP_DIR="$(mktemp -d)" && cp /Users/eden/Downloads/demo/TX01_MIC012_20260520_102542_orig.wav "$TMP_DIR/" && curl -sS -X POST http://127.0.0.1:19010/_bifrost/api/asr/tasks -H 'content-type: application/json' --data-binary "{\"name\":\"diarization-cap-real\",\"audio_dir\":\"$TMP_DIR\",\"enabled\":false,\"model\":\"Qwen3-ASR-0.6B\",\"diarization\":{\"enabled\":true,\"profile\":\"sherpa-onnx-balanced\",\"max_speakers\":4,\"voiceprint_matching\":true}}"
   ```
2. 对任务执行 run，等待完成：
   ```bash
   source ~/.zshrc && curl -sS -X POST "http://127.0.0.1:19010/_bifrost/api/asr/tasks/<task-id>/run"
   ```
3. 查询任务详情和 timeline：
   ```bash
   source ~/.zshrc && curl -sS "http://127.0.0.1:19010/_bifrost/api/asr/tasks/<task-id>"
   ```

预期结果：

- 同一真实 5 分 30 秒单声道音频不会生成 20 个 speaker。
- `summary.speaker_count <= 4`。
- 文件级 `speaker_count <= 4`。
- timeline speakers 显示为稳定的 `用户A/B/C/D` 本地角色；只有声纹匹配置信度达到阈值时才覆盖为已注册真人名。

### TC-ASPO-17：短碎片角色稳定化且单注册声纹优先识别本人

操作步骤：

1. 保持真实服务运行，并确认已经存在一个注册声纹 `eden`：
   ```bash
   source ~/.zshrc && curl -sS http://127.0.0.1:19010/_bifrost/api/asr/speaker-profiles
   ```
2. 使用同一段真实会议音频创建 speaker-aware Directory Task，模型使用 `Qwen3-ASR-0.6B`，不填写 `known_speaker_count`，只设置 `max_speakers=4`：
   ```bash
   source ~/.zshrc && TMP_DIR="$(mktemp -d)" && cp /Users/eden/Downloads/demo/TX01_MIC012_20260520_102542_orig.wav "$TMP_DIR/meeting.wav" && curl -sS -X POST http://127.0.0.1:19010/_bifrost/api/asr/tasks -H 'content-type: application/json' --data-binary "{\"name\":\"speaker-stabilize-real\",\"audio_dir\":\"$TMP_DIR\",\"model\":\"Qwen3-ASR-0.6B\",\"runtime_strategy\":\"fork_per_chunk\",\"recursive\":false,\"diarization\":{\"enabled\":true,\"profile\":\"sherpa-onnx-balanced\",\"max_speakers\":4,\"voiceprint_matching\":true}}"
   ```
3. 执行任务并等待完成：
   ```bash
   source ~/.zshrc && curl -sS -X POST "http://127.0.0.1:19010/_bifrost/api/asr/tasks/<task-id>/run"
   ```
4. 查询 timeline：
   ```bash
   source ~/.zshrc && FILE_KEY="$(curl -sS http://127.0.0.1:19010/_bifrost/api/asr/tasks/<task-id> | jq -r '.files[0].key')" && curl -sS "http://127.0.0.1:19010/_bifrost/api/asr/tasks/<task-id>/files/$FILE_KEY/timeline"
   ```

预期结果：

- 短碎片 speaker 不再作为独立 `用户C/用户D` 大量出现在字幕中，同一真实音频稳定到 2 个主要角色。
- `speakers[0].display_name` 或主要候选角色显示为已注册声纹名 `eden`。
- 低于正式阈值但最接近已注册声纹的角色会写入 `candidate_profile_id/candidate_display_name/candidate_confidence`，页面可解释“为什么没有正式匹配”。
- 未被正式识别为 `eden` 的角色仍保留本地角色名，不会因为低置信度候选误标成本人。

### TC-ASPO-18：WebUI 实时麦克风展示多人 Timeline

操作步骤：

1. 使用临时数据目录启动真实 Bifrost 服务，端口使用 `19010` 或其他未占用端口：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR=/Users/eden/work/github/bifrost-asr-pipeline-orchestrator/.bifrost-asr-manual-test SKIP_FRONTEND_BUILD=1 RUST_LOG=bifrost_admin::voice=info,bifrost_admin::asr_jobs=info,info cargo run --bin bifrost -- start -p 19010 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 打开 WebUI：
   ```text
   http://127.0.0.1:19010/_bifrost/ai?aiSection=tools-asr
   ```
3. 在 Speech to Text 页面确认 Workbench Model 为 `Qwen3-ASR-0.6B`，点击 `Start Service`。
4. 点击 `Start Mic`，让 2 到 4 个人轮流说话，每个人至少说 2 轮，每轮 2 秒以上。
5. 观察 Transcript 下方的 `Live Timeline`，并点击 `SRT`、`TXT`、`JSON` 导出按钮。

预期结果：

- Transcript 仍显示连续完整文本。
- `Live Timeline` 每条稳定 utterance 都展示 `mm:ss.mmm - mm:ss.mmm` 时间范围、说话人标签和该句文本。
- 已注册声纹本人通过阈值时优先显示注册名；低于正式阈值时显示候选名和置信度，不误覆盖其他人。
- 未注册的人按本次录音 session 内聚类为 `用户A/B/C/D`，不会出现 20 个用户这种爆炸。
- 导出的 `live-realtime.srt`、`live-realtime.txt`、`live-realtime.timeline.json` 包含同一组稳定 utterance。
- 如果多人重叠同时说话，实时 timeline 可以只保留单主说话人；最终高质量多人字幕仍由离线字幕 pipeline 产出。

### TC-ASPO-19：WebUI Work Actions Start Listening 不因 KWS 资产缺失报 400

操作步骤：

1. 使用已有真实数据目录启动服务，确保 `/_bifrost/api/voice/wake/kws/status` 返回 `ready=false`：
   ```bash
   source ~/.zshrc && curl -sS http://127.0.0.1:19010/_bifrost/api/voice/wake/kws/status
   ```
2. 打开 WebUI：
   ```text
   http://127.0.0.1:19010/_bifrost/ai?aiSection=tools-asr
   ```
3. 在 `Voice Wake Actions` 中保存一个带声纹的 Wake phrase。
4. 点击 `Start Listening`。
5. 点击 `Stop Listening`。

预期结果：

- 点击 `Start Listening` 不返回 400，页面不会卡在 `Starting voice wake listener...`。
- KWS 资产缺失时，listener 返回 `running=true` 且 `engine=backend_asr_phrase_match`。
- listener 状态包含 `last_speaker_status=kws_missing_fallback_backend_asr_phrase_match`，用于解释当前不是 KWS 轻量路径。
- 页面显示 `Backend Listening`，点击 `Stop Listening` 后回到 `Idle`。
- KWS 资产初始化完成后，同一按钮仍优先使用 `lightweight_kws_listener`。

## 清理步骤

- 静态验收用例不创建临时服务和临时数据目录。
- 真实服务用例执行后必须停止临时 Bifrost 进程并删除临时 `BIFROST_DATA_DIR`。
- 如果手动执行 TC-ASPO-11/13，结束后用 `lsof -nP -iTCP:18998 -sTCP:LISTEN`、`lsof -nP -iTCP:18999 -sTCP:LISTEN` 确认没有残留进程。

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-28 | TC-ASPO-01/02/03/04/05/06 | 已执行上述 7 条静态验收命令，验证顶层架构、旧实时 WebSocket 降级边界、离线 pipeline、ASR Unit Planner、迁移策略、分阶段计划、测试计划和 readme 索引 | 通过 |
| 2026-05-28 | TC-ASPO-07 | 已执行后处理链路静态验收命令，验证 `OfflineSubtitlePipeline` 不替代 Daily Docs / Daily Agent / AI Runner / report/IM/sync 后处理 | 通过 |
| 2026-05-28 | TC-ASPO-08 | 已执行独立 crate 与跨平台编译边界静态验收命令，验证 `bifrost-asr`、feature matrix、admin 适配层和 Cargo metadata/tree 门禁 | 通过 |
| 2026-05-28 | TC-ASPO-09 | 已执行 `bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh`，真实启动 Bifrost 服务并验证 speech decision、旧 ASR WS 410、wake lightweight、offline-jobs、CLI subtitle、Directory Task artifacts 和 Daily Agent 后处理入口 | 通过 |
| 2026-05-28 | TC-ASPO-09 | 已执行 `CI=1 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_ASR_PIPELINE_E2E_PORT=18998 bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh`，验证 CI/非在线 ASR 环境下不依赖 ffmpeg/say/Qwen3 资产，仍真实启动服务覆盖 orchestrator、legacy 410、wake lightweight，并确认 KWS 状态不包含 Qwen3-ASR 默认依赖 | 通过 |
| 2026-05-28 | TC-ASPO-09 | 已执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_ASR_PIPELINE_E2E_PORT=18999 BIFROST_ASR_PIPELINE_E2E_ONLINE=1 bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh`，验证 Apple Silicon 在线 ASR 真实语音、offline-jobs、CLI subtitle、Directory Task artifacts 和 Daily Agent 后处理入口 | 通过 |
| 2026-05-28 | TC-ASPO-10 | 已执行 `rg` 静态验收，确认 `bifrost-asr` 暴露 decision/resources/planner/offline/subtitle/timeline/artifacts/profiles，Admin 通过 `bifrost_asr::*` 接入核心业务 | 通过 |
| 2026-05-28 | TC-ASPO-11 | 已由 TC-ASPO-09 脚本覆盖真实服务 `/api/asr/transcribe-ws`，返回 410 且响应包含 `/api/voice/listen-ws` | 通过 |
| 2026-05-28 | TC-ASPO-12 | 已由 TC-ASPO-09 脚本覆盖真实 Directory Task artifacts 和 Daily Agent 配置接口，确认单文件 Offline Pipeline 未覆盖后处理入口 | 通过 |
| 2026-05-28 | TC-ASPO-13 | 已启动临时 Bifrost 服务并用 Playwright 打开 `/_bifrost/`，验证管理端可加载；同时通过浏览器请求 `/api/speech/pipelines/status` 确认 WebUI 后端使用的 realtime/offline/scheduled profiles 可用。WebUI 构建由本地/CI build 覆盖 | 通过 |
| 2026-05-28 | TC-ASPO-14 | 已在 `http://127.0.0.1:19010` 启动真实服务，验证 offline decision 默认 `Qwen3-ASR-0.6B`；`speech_workbench` 启动 0.6B 后 `voice_wake_listener` 再启动返回 200 并复用同一 `server_url`；通过 `voice_wake_listener` 停止后可重新启动，未残留 owner stale state | 通过 |
| 2026-05-28 | TC-ASPO-15 | 已在 `http://127.0.0.1:19010` 使用真实服务验证：API 只传 `name/audio_dir` 创建任务返回 `Qwen3-ASR-0.6B`、diarization enabled、voiceprint matching enabled；`cargo run --bin bifrost -- -p 19010 ai asr task create --json` 返回同样默认；Playwright 打开 WebUI 新建任务弹窗，确认 `Qwen3-ASR-0.6B` 可见且 4 个开关中包含 Speaker Diarization / Voiceprint Matching 默认打开 | 通过 |
| 2026-05-28 | TC-ASPO-16 | 已用真实 `TX01_MIC012_20260520_102542_orig.wav` 先验证旧产物为 30 个 diarization segment 被拆成 20 个 speaker；修复后在隔离任务中重跑同一音频，任务完成且 `speaker_count=4`；随后将原 `demo` 任务的同一文件重跑，原 URL 对应文件从 `speaker_count=20` 更新为 `speaker_count=4`，timeline speakers 为 `用户A/B/C/D` | 通过 |
| 2026-05-28 | TC-ASPO-17 | 已用真实 `TX01_MIC012_20260520_102542_orig.wav`、真实 sherpa diarization、真实 Qwen3-ASR-0.6B fork-per-chunk、真实已注册 `eden` 声纹重跑；修复后任务 `36c10c9a62654d23bd8414aaa765c05b` 完成，短碎片 `speaker_02/speaker_03` 被合并，`speaker_count=2`，主要角色 `speaker_00` 映射为 `eden` 且 `confidence=0.5412222`，另一个角色保留 `用户B` 并记录 eden candidate `0.49354425` | 通过 |
| 2026-05-28 | TC-ASPO-18 | 已执行 `BIFROST_ASR_PIPELINE_E2E_ONLINE=0 BIFROST_ASR_PIPELINE_E2E_PORT=18998 bash e2e-tests/tests/test_asr_speech_pipeline_orchestrator_real_service.sh`，真实启动服务并通过 `/api/voice/listen-ws` WebSocket 发送 16k PCM，验证 stable realtime event 包含 `window_start_ms/window_end_ms/speaker/speaker_display_name/delta`；WebUI 多人现场体验仍需用户与多人录音环境继续复测 | 自动化链路通过，现场多人待复测 |
| 2026-05-28 | TC-ASPO-19 | 已在 `http://127.0.0.1:19010` 重启真实服务；先用 curl 验证 `POST /_bifrost/api/voice/wake/listener/start` 返回 200、`running=true`、`engine=backend_asr_phrase_match`、`last_speaker_status=kws_missing_fallback_backend_asr_phrase_match`；再用浏览器真实点击 WebUI `Voice Wake Actions -> Start Listening`，页面显示 `Backend Listening`，点击 `Stop Listening` 后回到 `Idle` | 通过 |
