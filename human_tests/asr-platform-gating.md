# ASR Platform Gating 真实场景测试

## 功能模块说明

验证新增 ASR 能力只在 macOS Apple Silicon 暴露和编译：支持平台显示 Qwen3-ASR、本地转写、Speech Workbench、Directory Tasks、Speaker Diarization、Voiceprint、Voice Wake ASR；非支持平台隐藏入口，并且只通过独立 `bifrost-asr` crate 管理 `qwen3-asr`、`sherpa-onnx`、native speaker 相关依赖路径和从 admin 迁出的 ASR 纯业务逻辑。

## 前置条件

1. 在仓库根目录执行所有命令。
2. 每条命令前执行 `source ~/.zshrc`。
3. 不启动系统代理；本测试不需要启动 Bifrost 服务。
4. 当前分支为 `codex/asr-pipeline-orchestrator`。

## 测试用例列表

### TC-ASR-PG-01 macOS aarch64 capability 支持矩阵

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin asr_platform_support_matrix_is_apple_silicon_macos_only
   ```
2. 检查测试输出。

预期结果：

- 测试通过。
- `macos-aarch64` 为支持平台。
- `macos-x86_64`、`linux-x86_64`、`windows-x86_64` 均为不支持平台。

### TC-ASR-PG-02 capability API flag 与当前平台一致

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin asr_capabilities_are_hidden_on_unsupported_current_platform
   ```
2. 检查测试输出。

预期结果：

- 测试通过。
- 当前平台如果是 macOS aarch64，则 `qwen3_asr`、`speaker_diarization`、`voiceprint`、`voice_wake_asr` 为 enabled 且 hidden=false。
- 当前平台如果不是 macOS aarch64，则上述能力均为 enabled=false 且 hidden=true。

### TC-ASR-PG-03 非支持平台不解析大资源/native 依赖

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && bash e2e-tests/tests/test_asr_platform_gating.sh
   ```
2. 检查脚本输出。

预期结果：

- 脚本输出 `[asr-platform-gating] PASS`。
- `bifrost-admin` 只直接依赖 `bifrost-asr`，不直接依赖 `qwen3-asr` 和 `sherpa-onnx`。
- `x86_64-unknown-linux-gnu` 的 workspace package resolve 中没有 `qwen3-asr` 和 `sherpa-onnx`。
- `x86_64-apple-darwin` 的 workspace package resolve 中没有 `qwen3-asr` 和 `sherpa-onnx`。
- `aarch64-apple-darwin` 的 workspace package resolve 中包含 `qwen3-asr` 和 `sherpa-onnx`，依赖归属来自 `bifrost-asr/full-local-asr`。
- `diarization.rs`、`voiceprint.rs`、`voice_stateful.rs` 的 native cfg fence 均为 `all(target_os = "macos", target_arch = "aarch64")`。

### TC-ASR-PG-06 bifrost-asr 编译边界归属

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n 'bifrost-asr|full-local-asr|qwen3-asr|sherpa-onnx' Cargo.toml crates/bifrost-admin/Cargo.toml crates/bifrost-asr/Cargo.toml crates/bifrost-asr/src crates/bifrost-admin/src/handlers/asr.rs crates/bifrost-admin/src/handlers/voice_stateful.rs crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs
   ```
2. 检查 workspace 成员包含 `crates/bifrost-asr`。
3. 检查 admin 常规依赖只启用 `bifrost-asr/core`，macOS aarch64 target 才启用 `bifrost-asr/full-local-asr`。
4. 检查 native 引用通过 `bifrost_asr::native::*` 访问。

预期结果：

- ASR platform/capability 逻辑可以从 `bifrost-asr/core` 引用。
- `qwen3-asr` 与 `sherpa-onnx` 只出现在 `crates/bifrost-asr/Cargo.toml`。
- admin 不再直接配置 native ASR crate，跨平台构建选择由是否依赖/启用 `bifrost-asr` features 决定。

### TC-ASR-PG-07 admin ASR 纯业务逻辑迁移归属

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n 'pub mod (artifacts|offline|planner|platform|profiles|runtime|subtitle|timeline)|TranscriptTimeline|AsrDiarizationConfig|AsrAudioUnit|plan_asr_units|write_offline_subtitle_artifacts|render_srt|render_vtt|output_paths_in|AsrServiceState|ASR_TASK_SEGMENT_MAX_MS' crates/bifrost-asr/src crates/bifrost-admin/src/asr_runtime.rs crates/bifrost-admin/src/handlers/asr_jobs_timeline.rs crates/bifrost-admin/src/handlers/asr_jobs.rs crates/bifrost-admin/src/handlers/asr_jobs/state.rs crates/bifrost-admin/src/handlers/asr_jobs/store.rs crates/bifrost-admin/src/handlers/asr_jobs/runner.rs crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs
   ```
2. 检查 `crates/bifrost-admin/src/asr_runtime.rs` 是否只 re-export `bifrost_asr::runtime::*`。
3. 检查 `crates/bifrost-admin/src/handlers/asr_jobs_timeline.rs` 是否只 re-export `bifrost_asr::timeline::*` 的任务所需类型和函数。
4. 检查 `AsrDiarizationConfig`、`DEFAULT_DIARIZATION_PROFILE`、`AsrAudioUnit`、`plan_asr_units`、`write_offline_subtitle_artifacts`、`render_srt`、`render_vtt`、`output_paths_in`、`ASR_TASK_SEGMENT_MAX_MS` 的 owner 是否都在 `crates/bifrost-asr/src`。

预期结果：

- ASR runtime path/state、timeline schema/render/normalization、daily summary generation、AsrUnitPlanner、subtitle writer、offline artifact writer、artifact output paths 和 diarization config 均归属于 `bifrost-asr`。
- admin 仅保留 API、任务状态机、FileStore、scheduler 和 Daily Agent 后处理编排，不再重复维护上述 ASR 纯业务模型或常量。
- `ASR_TASK_SEGMENT_MAX_MS` 在 admin runner 中通过 `bifrost-asr` re-export 使用，避免 chunk split 与 timeline normalization 两套规则漂移。

### TC-ASR-PG-04 CLI help 非支持平台隐藏 ASR/Voice 入口

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n 'not\(all\(target_os = "macos", target_arch = "aarch64"\)\)|command\(hide = true\)' crates/bifrost-cli/src/cli.rs
   ```
2. 检查输出中 `AiCommands::Asr`、`AiCommands::Voice` 和 ASR 子命令附近存在 `command(hide = true)` cfg_attr。

预期结果：

- 非 macOS aarch64 构建的 `bifrost ai --help` 不展示 `asr` 和 ASR-backed `voice` 入口。
- 用户手动输入隐藏命令时，执行路径仍保留 `ensure_supported_platform()` 防护。

### TC-ASR-PG-08 Directory Task artifact API 与 subtitle writer 归属

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n 'GET, _.*artifacts|get_task_file_artifacts_response|record_artifact_paths|artifact_content_type|write_offline_subtitle_artifacts|subtitle_path_from_timeline|render_srt|render_vtt' crates/bifrost-admin/src/handlers/asr_jobs/api.rs crates/bifrost-admin/src/handlers/asr_jobs/runner.rs crates/bifrost-asr/src/offline.rs crates/bifrost-asr/src/artifacts.rs crates/bifrost-asr/src/subtitle.rs
   ```
2. 检查 Directory Task 文件级 artifact 列表和单格式下载 API 是否存在。
3. 检查 `.srt` 和 `.vtt` writer 是否归属于 `bifrost-asr`，admin runner 是否通过 `write_offline_subtitle_artifacts` 写入 `.txt/.json/.timeline.json/.srt/.vtt`。

预期结果：

- `GET /api/asr/tasks/{task_id}/files/{file_key}/artifacts` 能列出已有 txt、metadata_json、timeline_json、srt、vtt。
- `GET /api/asr/tasks/{task_id}/files/{file_key}/artifacts/{format}` 能返回对应 artifact 内容。
- subtitle writer 和 artifact path 规则在 `bifrost-asr` 内，admin 只负责 API 响应。

### TC-ASR-PG-05 WebUI 非支持平台不展示 ASR 能力入口

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc && rg -n 'getAsrCapabilities|asrEntryEnabled|capabilities.*qwen3_asr|asrSupported' web/src/pages/AI/index.tsx web/src/pages/ASR/index.tsx web/src/api/asr.ts
   ```
2. 检查 AI Tools 导航只在 `qwen3_asr.enabled && !hidden` 时加入 ASR。
3. 检查 ASR 页面在 capability 未加载或 unsupported 时返回空容器，不渲染 Model Management、Diarization、Voiceprint、Voice Wake、Directory Tasks、Speech Workbench。

预期结果：

- AI 导航不会在非支持平台展示 ASR 工具入口。
- 直接访问 ASR 页面时，非支持平台不会展示 Qwen3-ASR、本地转写、Speech Workbench、Directory Task ASR 模型选择、Speaker Diarization、Voiceprint、Voice Wake ASR 相关入口。

## 清理步骤

1. 删除测试产生的临时目录；`test_asr_platform_gating.sh` 已通过 `trap` 自动清理。
2. 本测试不启动服务，不需要停止 Bifrost 进程。

## 本轮执行记录

| 用例 | 状态 | 证据 |
| --- | --- | --- |
| TC-ASR-PG-01 | 通过 | 2026-05-28 执行 `source ~/.zshrc && cargo test -p bifrost-admin asr_platform`，`asr_platform_support_matrix_is_apple_silicon_macos_only` 通过，验证仅 `macos-aarch64` 支持。 |
| TC-ASR-PG-02 | 通过 | 2026-05-28 执行 `source ~/.zshrc && cargo test -p bifrost-admin asr_capabilities`，`asr_capabilities_are_hidden_on_unsupported_current_platform` 通过，当前平台 capability flag 与平台矩阵一致。 |
| TC-ASR-PG-03 | 通过 | 2026-05-28 执行 `source ~/.zshrc && bash e2e-tests/tests/test_asr_platform_gating.sh`，输出 `[asr-platform-gating] PASS`，验证 admin 只直连 `bifrost-asr`，Linux 与 macOS x86_64 不解析 `qwen3-asr/sherpa-onnx`，macOS aarch64 解析二者。 |
| TC-ASR-PG-04 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'not\(all\(target_os = "macos", target_arch = "aarch64"\)\)\|command\(hide = true\)' crates/bifrost-cli/src/cli.rs`，输出覆盖 `AiCommands::Asr`、`AiCommands::Voice` 和 ASR 子命令 hide fence。 |
| TC-ASR-PG-04 | 通过 | 2026-05-28 执行 `source ~/.zshrc && CI=1 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`，确认 macOS arm64 help 仍展示 `stream-file`；脚本已同步为非支持平台断言 `stream-file` 不出现在 `ai asr --help`，并保留 unsupported CLI guard。 |
| TC-ASR-PG-05 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'getAsrCapabilities\|asrEntryEnabled\|capabilities.*qwen3_asr\|asrSupported' web/src/pages/AI/index.tsx web/src/pages/ASR/index.tsx web/src/api/asr.ts`，确认 AI 导航和 ASR 页面均由 capability 控制入口渲染。 |
| TC-ASR-PG-06 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'bifrost-asr\|full-local-asr\|qwen3-asr\|sherpa-onnx' Cargo.toml crates/bifrost-admin/Cargo.toml crates/bifrost-asr/Cargo.toml crates/bifrost-asr/src crates/bifrost-admin/src/handlers/asr.rs crates/bifrost-admin/src/handlers/voice_stateful.rs crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs`，确认 workspace 成员包含 `crates/bifrost-asr`，admin core/full-local-asr feature 边界存在，native crate 只在 `bifrost-asr` 配置，admin native 调用通过 `bifrost_asr::native::*`。 |
| TC-ASR-PG-07 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'pub mod (artifacts\|offline\|planner\|platform\|profiles\|runtime\|subtitle\|timeline)\|TranscriptTimeline\|AsrDiarizationConfig\|AsrAudioUnit\|plan_asr_units\|write_offline_subtitle_artifacts\|render_srt\|render_vtt\|output_paths_in\|AsrServiceState\|ASR_TASK_SEGMENT_MAX_MS' crates/bifrost-asr/src crates/bifrost-admin/src/asr_runtime.rs crates/bifrost-admin/src/handlers/asr_jobs_timeline.rs crates/bifrost-admin/src/handlers/asr_jobs.rs crates/bifrost-admin/src/handlers/asr_jobs/state.rs crates/bifrost-admin/src/handlers/asr_jobs/store.rs crates/bifrost-admin/src/handlers/asr_jobs/runner.rs crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs`，确认 ASR runtime/timeline/artifact/profile/platform/planner/subtitle/offline artifact 纯业务边界迁入 `bifrost-asr`，admin 只 re-export 或调用。 |
| TC-ASR-PG-08 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'GET, _.*artifacts\|get_task_file_artifacts_response\|record_artifact_paths\|artifact_content_type\|write_offline_subtitle_artifacts\|subtitle_path_from_timeline\|render_srt\|render_vtt' crates/bifrost-admin/src/handlers/asr_jobs/api.rs crates/bifrost-admin/src/handlers/asr_jobs/runner.rs crates/bifrost-asr/src/offline.rs crates/bifrost-asr/src/artifacts.rs crates/bifrost-asr/src/subtitle.rs`，确认 Directory Task 文件级 artifact API 存在，SRT/VTT writer 与 artifact path/write 规则归属 `bifrost-asr`。 |
