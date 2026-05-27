# ASR Platform Gating 真实场景测试

## 功能模块说明

验证新增 ASR 能力只在 macOS Apple Silicon 暴露和编译：支持平台显示 Qwen3-ASR、本地转写、Speech Workbench、Directory Tasks、Speaker Diarization、Voiceprint、Voice Wake ASR；非支持平台隐藏入口，并且不解析 `qwen3-asr`、`sherpa-onnx` 和 native speaker 相关依赖路径。

## 前置条件

1. 在仓库根目录执行所有命令。
2. 每条命令前执行 `source ~/.zshrc`。
3. 不启动系统代理；本测试不需要启动 Bifrost 服务。
4. 当前分支为 `codex/asr-diarization-offline`。

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
- `x86_64-unknown-linux-gnu` 的 `bifrost-admin` dependency resolve 中没有 `qwen3-asr` 和 `sherpa-onnx`。
- `x86_64-apple-darwin` 的 `bifrost-admin` dependency resolve 中没有 `qwen3-asr` 和 `sherpa-onnx`。
- `aarch64-apple-darwin` 的 `bifrost-admin` dependency resolve 中包含 `qwen3-asr` 和 `sherpa-onnx`。
- `diarization.rs`、`voiceprint.rs`、`voice_stateful.rs` 的 native cfg fence 均为 `all(target_os = "macos", target_arch = "aarch64")`。

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
| TC-ASR-PG-03 | 通过 | 2026-05-28 执行 `source ~/.zshrc && bash e2e-tests/tests/test_asr_platform_gating.sh`，输出 `[asr-platform-gating] PASS`，验证 Linux 与 macOS x86_64 不解析 `qwen3-asr/sherpa-onnx`，macOS aarch64 解析二者。 |
| TC-ASR-PG-04 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'not\(all\(target_os = "macos", target_arch = "aarch64"\)\)\|command\(hide = true\)' crates/bifrost-cli/src/cli.rs`，输出覆盖 `AiCommands::Asr`、`AiCommands::Voice` 和 ASR 子命令 hide fence。 |
| TC-ASR-PG-04 | 通过 | 2026-05-28 执行 `source ~/.zshrc && CI=1 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`，确认 macOS arm64 help 仍展示 `stream-file`；脚本已同步为非支持平台断言 `stream-file` 不出现在 `ai asr --help`，并保留 unsupported CLI guard。 |
| TC-ASR-PG-05 | 通过 | 2026-05-28 执行 `source ~/.zshrc && rg -n 'getAsrCapabilities\|asrEntryEnabled\|capabilities.*qwen3_asr\|asrSupported' web/src/pages/AI/index.tsx web/src/pages/ASR/index.tsx web/src/api/asr.ts`，确认 AI 导航和 ASR 页面均由 capability 控制入口渲染。 |
