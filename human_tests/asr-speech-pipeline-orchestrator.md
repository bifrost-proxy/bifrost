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
3. 本文件是方案文档静态验收，不需要启动 Bifrost 服务，不修改系统代理。

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

## 清理步骤

- 本组用例不创建临时服务和临时数据目录，无需清理。

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-28 | TC-ASPO-01/02/03/04/05/06 | 已执行上述 7 条静态验收命令，验证顶层架构、旧实时 WebSocket 降级边界、离线 pipeline、ASR Unit Planner、迁移策略、分阶段计划、测试计划和 readme 索引 | 通过 |
| 2026-05-28 | TC-ASPO-07 | 已执行后处理链路静态验收命令，验证 `OfflineSubtitlePipeline` 不替代 Daily Docs / Daily Agent / AI Runner / report/IM/sync 后处理 | 通过 |
| 2026-05-28 | TC-ASPO-08 | 已执行独立 crate 与跨平台编译边界静态验收命令，验证 `bifrost-asr`、feature matrix、admin 适配层和 Cargo metadata/tree 门禁 | 通过 |
