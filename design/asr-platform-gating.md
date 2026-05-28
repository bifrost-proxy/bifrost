# ASR Platform Gating

## 功能模块说明

当前新增 ASR 能力只支持 Apple Silicon macOS，对应 Rust cfg 为 `all(target_os = "macos", target_arch = "aarch64")`。平台 gating 的目标不是在用户点击后返回 unsupported，而是在不支持的平台隐藏 ASR 入口，并且不把 Qwen3-ASR、sherpa-onnx、native speaker diarization、voiceprint embedding 等大资源或 native 依赖编进非支持平台构建。

## 用户目标验证清单

- 必须实现：Qwen3-ASR、本地转写、Speech Workbench、Directory Task ASR 模型选择、Speaker Diarization、Voiceprint、Voice Wake ASR/声纹相关入口只在 macOS aarch64 显示。
- 必须实现：ASR native 依赖归属迁移到独立 `crates/bifrost-asr`，`bifrost-admin` 只依赖 `bifrost-asr` 作为编译边界。
- 必须实现：`bifrost-admin` 下与 ASR 运行时路径、timeline/字幕文本标准化、artifact 输出路径和 diarization profile/config 相关的主要纯业务逻辑迁移到 `crates/bifrost-asr`，admin 只保留 API、任务状态机、FileStore 和 Daily Agent 后处理编排。
- 必须实现：`crates/bifrost-asr/Cargo.toml` 只在 macOS aarch64 target dependency 中声明可选 `qwen3-asr` 和 `sherpa-onnx`，并通过 feature 打开。
- 必须实现：`diarization.rs` 的 sherpa native 实现、`voiceprint.rs` 的 speaker embedding/identify native 实现、`voice_stateful.rs` 的 Qwen3 stateful worker 只在 macOS aarch64 编译。
- 必须实现：Admin API 提供 `/api/asr/capabilities`，前端以 capability 判断是否显示 ASR 工具入口。
- 必须不破坏：macOS aarch64 现有 ASR runtime、Directory Task、Diarization、Voiceprint、Voice Wake 的支持路径。
- 必须真实验证：Cargo platform metadata、CLI help gating、前端隐藏逻辑、human_tests 平台矩阵、两轮 Review/Fix/Test。

## 实现逻辑

- 后端 capability：
  - `GET /api/asr/capabilities` 返回当前 `platform`、`arch`、`supported_target` 和各 ASR 子能力的 `enabled/hidden/platform_supported/reason`。
  - 当前所有新增 ASR 子能力使用同一平台门禁：`macos-aarch64` 为 enabled，其他平台为 hidden。
- Cargo 依赖：
  - `bifrost-admin` 常规依赖 `bifrost-asr` 的 `core` feature，用于 capability/platform 纯 Rust 逻辑。
  - `bifrost-admin` 在 macOS aarch64 target dependency 中启用 `bifrost-asr/full-local-asr`。
  - `qwen3-asr` 与 `sherpa-onnx` 只放在 `crates/bifrost-asr` 的 `[target.'cfg(all(target_os = "macos", target_arch = "aarch64"))'.dependencies]`，并保持 optional。
  - macOS UI/系统集成依赖仍保留在 `target_os = "macos"`，因为它们不是 ASR native 模型资源。
- 业务边界：
  - `crates/bifrost-asr/src/runtime.rs` 拥有 ASR service state、固定安装目录、数据目录和本地 health probe/stop helper。
  - `crates/bifrost-asr/src/timeline.rs` 拥有 `TranscriptTimeline`、`TimelineSegment`、source audio inspection、timeline text render、legacy oversized segment normalization 和 daily summary generation。
  - `crates/bifrost-asr/src/planner.rs` 拥有 diarization segment -> `AsrAudioUnit` 的合并、拆分、过滤计划规则。
  - `crates/bifrost-asr/src/artifacts.rs` 拥有 ASR text/timeline/source/subtitle output path 规则。
  - `crates/bifrost-asr/src/subtitle.rs` 拥有 SRT/VTT subtitle writer。
  - `crates/bifrost-asr/src/offline.rs` 拥有离线 timeline/text/srt/vtt/metadata artifact 落盘逻辑。
  - `crates/bifrost-asr/src/profiles.rs` 拥有 diarization config/default profile。
  - `bifrost-admin` 通过 re-export 或直接调用 `bifrost-asr`，不再在 admin 内重复维护这些 ASR 业务模型和规则。
- native 代码路径：
  - `run_sherpa_diarization`、`compute_speaker_embedding`、`compute_diarization_speaker_embeddings`、`identify_speaker_voice_from_wav_file` 只在 macOS aarch64 编译 native 实现。
  - 非支持平台只保留轻量 stub，避免引用 sherpa symbols。
  - `voice_stateful` 的 qwen worker module 只在 macOS aarch64 编译。
- 前端入口：
  - AI 页面加载 capability，只有 `qwen3_asr.enabled && !hidden` 时才把 ASR 放入 Tools 导航。
  - ASR 页面直接访问时，capability 未加载或 unsupported 时不渲染 Model Management、Diarization、Voiceprint、Voice Wake、Directory Tasks、Speech Workbench。
- CLI help：
  - `bifrost ai asr` 与 `bifrost ai voice` 在非 macOS aarch64 通过 clap `hide = true` 从 help 中隐藏。
  - 关键 ASR 子命令继续在执行时保留 `ensure_supported_platform()`，防止用户手输隐藏命令后触发 native 路径。

## 依赖项

- Rust cfg：`all(target_os = "macos", target_arch = "aarch64")`
- 后端 API：`crates/bifrost-admin/src/handlers/asr.rs`
- native ASR 代码：`diarization.rs`、`voiceprint.rs`、`voice_stateful.rs`
- ASR 编译边界：`crates/bifrost-asr`
- 前端入口：`web/src/pages/AI/index.tsx`、`web/src/pages/ASR/index.tsx`、`web/src/api/asr.ts`
- CLI help：`crates/bifrost-cli/src/cli.rs`

## 平台矩阵

| 平台 | ASR 入口 | bifrost-asr feature | Qwen3-ASR dependency | sherpa-onnx dependency | native diarization/voiceprint | capability |
| --- | --- | --- | --- | --- | --- | --- |
| macOS aarch64 | 显示 | full-local-asr | 编译 | 编译 | 编译 | enabled/hidden=false |
| macOS x86_64 | 隐藏 | core | 不编译 | 不编译 | stub | enabled=false/hidden=true |
| Linux x86_64 | 隐藏 | core | 不编译 | 不编译 | stub | enabled=false/hidden=true |
| Windows x86_64 | 隐藏 | core | 不编译 | 不编译 | stub | enabled=false/hidden=true |

## 测试方案

- 单元测试：
  - `asr_platform_support_matrix_is_apple_silicon_macos_only` 验证平台矩阵。
  - `asr_capabilities_are_hidden_on_unsupported_current_platform` 验证 capability flag 与当前平台一致。
- E2E 测试：
  - `e2e-tests/tests/test_asr_platform_gating.sh` 静态检查 Cargo target dependency、`bifrost-asr` feature 边界、native cfg、CLI hide fence，并用 `cargo metadata --filter-platform` 验证 Linux 与 macOS x86_64 不解析 `qwen3-asr/sherpa-onnx`、macOS aarch64 会解析，且 admin 不直接依赖 native crate。
- 真实场景测试：
  - `human_tests/asr-platform-gating.md` 覆盖 macOS aarch64 支持平台入口显示、非支持平台入口隐藏、Cargo metadata 资源边界、CLI help 隐藏和 admin ASR 纯业务逻辑迁移边界。

## Review/Fix/Test 闭环方案

- 第 1 轮：
  - 复核用户目标、`git status --short`、`git diff` 和新增文件。
  - Review Cargo target dependency、Rust cfg、capability API、AI/ASR 页面隐藏逻辑、CLI help hide。
  - 运行单元测试、E2E 平台 gating 脚本和前端相关测试。
- 第 2 轮：
  - 基于第 1 轮修复后的最新 diff 再次检查非支持平台是否仍有入口或 native dependency。
  - 复跑受影响测试，检查 `design/`、`human_tests/`、`human_tests/readme.md` 是否同步。

## 校验要求

- 必须执行 `cargo fmt --all -- --check`。
- 必须执行 `cargo test -p bifrost-admin asr_platform`。
- 必须执行 `bash e2e-tests/tests/test_asr_platform_gating.sh`。
- 必须执行相关前端单元测试或类型检查。
- 最终按 rust-project-validate 执行 clippy、workspace all-features 和必要 build/test；如 workspace all-features 或 local-ci 因时间/环境阻塞，最终交付记录证据与风险。

## 文档更新要求

- 新增本设计文档。
- 新增 `human_tests/asr-platform-gating.md` 并更新 `human_tests/readme.md` 索引。
