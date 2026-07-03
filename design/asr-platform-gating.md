# ASR Platform Gating

## 背景

当前新增 ASR 能力（Qwen3-ASR、sherpa-onnx speaker diarization、voiceprint embedding、Voice Wake ASR/声纹触发器）都依赖 macOS Apple Silicon 上的 native 库、Metal/MLX 加速与本地下载的模型权重。这些依赖体积大、平台特化强，编进 Linux / Windows / macOS x86_64 构建时既会显著增加二进制体积，也会因缺 native 库而链接失败或运行时 panic。

Platform gating 的目标不是"不支持平台点击后回错误"，而是：

- 编译期：不支持平台完全不编 ASR 相关 native crate 与 stub 之外的实现。
- 运行期：不支持平台的 ASR API 返回 capability=hidden；WebUI/CLI 不给用户看到 ASR 入口，也不下发意外命令。

同时把 ASR 相关业务模型（timeline、planner、artifacts、subtitle、offline、profiles）从 `bifrost-admin` 迁移到独立 `crates/bifrost-asr`，形成清晰的编译边界：`bifrost-admin` 只依赖 `bifrost-asr` 的 core feature 与 API 层，native/model runtime 只在 macOS aarch64 target dependency 中启用。

## 用户目标验证清单

### 必须实现

- Qwen3-ASR、本地转写、Speech Workbench、Directory Task ASR 模型选择、Speaker Diarization、Voiceprint、Voice Wake ASR/声纹相关入口只在 macOS aarch64 显示。
- 其他平台（macOS x86_64、Linux x86_64、Windows x86_64）的 ASR/Voice 入口在 WebUI 与 CLI help 中隐藏；直接 API 调用返回 `enabled=false, hidden=true` 与可读原因。
- ASR native 依赖归属迁移到独立 `crates/bifrost-asr`，`bifrost-admin` 只依赖 `bifrost-asr` 作为编译边界，不直接引用 `qwen3-asr`、`sherpa-onnx`。
- `bifrost-admin` 下与 ASR runtime 路径、timeline/字幕文本标准化、artifact 输出路径、diarization profile/config 相关的纯业务逻辑迁移到 `crates/bifrost-asr`，admin 只保留 API、任务状态机、FileStore 与 Daily Agent 后处理编排。
- `crates/bifrost-asr/Cargo.toml` 只在 macOS aarch64 target dependency 中声明可选 `qwen3-asr` 和 `sherpa-onnx`，并通过 feature `full-local-asr` 打开。
- `diarization.rs` 的 sherpa native 实现、`voiceprint.rs` 的 speaker embedding/identify native 实现、`voice_stateful.rs` 的 Qwen3 stateful worker 只在 macOS aarch64 编译。
- Admin API 提供 `/api/asr/capabilities`，前端以 capability 判断是否显示 ASR Tools 入口。
- CLI `bifrost ai asr` / `bifrost ai voice` 在非 macOS aarch64 通过 clap `hide = true` 从 help 中隐藏，同时保留 `ensure_supported_platform()` 兜底防止手输隐藏命令。

### 必须不破坏

- macOS aarch64 现有 ASR runtime、Directory Task、Diarization、Voiceprint、Voice Wake、Speech Workbench 支持路径不变。
- 非 ASR 的 macOS 集成能力（system proxy、CA install、tray、mDNS 等）仍按 `target_os = "macos"` 编译，不误伤。
- `bifrost-admin` 其他 handler、CLI 其他子命令、Web 其他页面不受迁移影响。

### 必须真实验证

- Cargo `metadata --filter-platform` 校验：非支持平台不解析 `qwen3-asr`/`sherpa-onnx`，支持平台正常解析。
- CLI help 校验：非支持平台 `bifrost ai asr --help` / `bifrost ai voice --help` 隐藏子命令。
- 前端隐藏逻辑校验：非支持平台 mock capability=hidden 时 Tools 导航不出现 ASR。
- human_tests 覆盖平台矩阵（至少 macOS aarch64 + 一非支持平台）。
- 两轮 Review/Fix/Test。

## 产品语义

### 支持平台矩阵

| 平台 | ASR 入口 | bifrost-asr feature | Qwen3-ASR dependency | sherpa-onnx dependency | native diarization/voiceprint | capability |
| --- | --- | --- | --- | --- | --- | --- |
| macOS aarch64 | 显示 | `full-local-asr` | 编译 | 编译 | 编译 | enabled=true / hidden=false |
| macOS x86_64 | 隐藏 | `core` | 不编译 | 不编译 | stub | enabled=false / hidden=true |
| Linux x86_64 | 隐藏 | `core` | 不编译 | 不编译 | stub | enabled=false / hidden=true |
| Linux aarch64 | 隐藏 | `core` | 不编译 | 不编译 | stub | enabled=false / hidden=true |
| Windows x86_64 | 隐藏 | `core` | 不编译 | 不编译 | stub | enabled=false / hidden=true |

V1 只承诺 macOS aarch64 作为 enabled 平台。未来扩展平台（例如 Linux CUDA、Windows CUDA）需要额外资源与验证矩阵，本文档不承诺。

### capability 语义

- `enabled`：当前平台是否支持该能力。
- `hidden`：UI 是否隐藏入口。V1 中 `!enabled` 等价于 `hidden=true`。
- `platform_supported`：编译期 target 是否属于支持矩阵。
- `reason`：非 enabled 时的可读原因，例如 `"ASR requires macOS on Apple Silicon"`。

### 迁移语义

- `bifrost-admin` 继续暴露 ASR HTTP handler，但 handler 内部调用 `bifrost_asr::...`，不直接 use `qwen3_asr` 或 `sherpa_onnx`。
- 非支持平台上 `bifrost-admin` 仍能编译并启动，`/api/asr/capabilities` 返回 hidden，其它 ASR handler 返回 `501 not_supported` 或空列表。

## 技术细节

### capability API

```text
GET /api/asr/capabilities
```

响应：

```json
{
  "platform": "macos",
  "arch": "aarch64",
  "supported_target": "macos-aarch64",
  "capabilities": {
    "qwen3_asr":        { "enabled": true,  "hidden": false, "platform_supported": true },
    "speech_workbench": { "enabled": true,  "hidden": false, "platform_supported": true },
    "directory_task":   { "enabled": true,  "hidden": false, "platform_supported": true },
    "diarization":      { "enabled": true,  "hidden": false, "platform_supported": true },
    "voiceprint":       { "enabled": true,  "hidden": false, "platform_supported": true },
    "voice_wake_asr":   { "enabled": true,  "hidden": false, "platform_supported": true }
  }
}
```

非支持平台上所有子能力返回 `enabled=false, hidden=true, reason="ASR requires macOS on Apple Silicon"`。

### Cargo 依赖布局

`crates/bifrost-asr/Cargo.toml`：

```toml
[features]
default = ["core"]
core = []
full-local-asr = ["qwen3-asr", "sherpa-onnx"]

[dependencies]
serde = { workspace = true }
# 平台无关的 ASR 业务逻辑依赖

[target.'cfg(all(target_os = "macos", target_arch = "aarch64"))'.dependencies]
qwen3-asr   = { version = "...", optional = true }
sherpa-onnx = { version = "...", optional = true }
```

`crates/bifrost-admin/Cargo.toml`：

```toml
[dependencies]
bifrost-asr = { path = "../bifrost-asr" }

[target.'cfg(all(target_os = "macos", target_arch = "aarch64"))'.dependencies]
bifrost-asr = { path = "../bifrost-asr", features = ["full-local-asr"] }
```

关键约束：

- `bifrost-admin` 与 CLI 不直接依赖 `qwen3-asr`/`sherpa-onnx`；只依赖 `bifrost-asr`。
- native 依赖必须放在 `bifrost-asr` 的 target dependency 中，避免非支持平台被 Cargo resolver 触碰。
- macOS 系统集成依赖（例如 `cocoa`、`objc`、`core-foundation`）继续按 `target_os = "macos"` 编译，与 ASR 无关。

### 业务边界

`crates/bifrost-asr/src/` 拆分：

- `runtime.rs`：ASR service state、固定安装目录、数据目录、本地 health probe / stop helper、`AsrServiceState` 结构与 `service.json` I/O。
- `timeline.rs`：`TranscriptTimeline`、`TimelineSegment`、source audio inspection、timeline text render、legacy oversized segment normalization、daily summary generation。
- `planner.rs`：diarization segment → `AsrAudioUnit` 的合并、拆分、过滤计划规则。
- `artifacts.rs`：ASR text / timeline / source / subtitle output path 规则。
- `subtitle.rs`：SRT / VTT subtitle writer。
- `offline.rs`：离线 timeline / text / srt / vtt / metadata artifact 落盘逻辑。
- `profiles.rs`：diarization config / default profile。
- `resources.rs`：`pause_on_realtime_voice` / owner yield 策略。
- `capabilities.rs`：`AsrCapabilities` 结构与 `current_platform_capabilities()`。

`bifrost-admin` re-export 或直接调用 `bifrost-asr`：

- `crates/bifrost-admin/src/handlers/asr.rs` 通过 `use bifrost_asr::{AsrServiceState, ...}` 引用。
- `handlers/asr_jobs.rs` 通过 `bifrost_asr::planner`、`bifrost_asr::timeline`、`bifrost_asr::artifacts` 调用业务能力。

### native cfg

`crates/bifrost-asr/src/diarization.rs`：

```rust
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native {
    pub fn run_sherpa_diarization(...) -> Result<...> { /* sherpa-onnx impl */ }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
mod native {
    pub fn run_sherpa_diarization(_: ...) -> Result<...> {
        Err(anyhow!("diarization requires macOS on Apple Silicon"))
    }
}
```

`voiceprint.rs`、`voice_stateful.rs` 采用相同模式。stub 版本必须完全不引用 sherpa/qwen3 symbols，保证非支持平台链接通过。

### CLI hide

`crates/bifrost-cli/src/cli.rs`：

```rust
#[derive(Subcommand)]
enum AiSubcommand {
    #[command(hide = !cfg!(all(target_os = "macos", target_arch = "aarch64")))]
    Asr(AsrArgs),
    #[command(hide = !cfg!(all(target_os = "macos", target_arch = "aarch64")))]
    Voice(VoiceArgs),
}
```

关键 ASR 子命令继续保留 `ensure_supported_platform()` 兜底：即使用户手输 `bifrost ai asr start`，非支持平台也应返回 `ASR requires macOS on Apple Silicon`，而不是 panic 或崩溃。

### 前端入口

- `web/src/pages/AI/index.tsx` 首次加载调用 `/api/asr/capabilities`。
- `qwen3_asr.enabled && !qwen3_asr.hidden` 才把 ASR 加入 Tools 导航。
- ASR 页面被直接访问（例如深链）时，capability 未加载或 unsupported 时不渲染 Model Management、Diarization、Voiceprint、Voice Wake、Directory Tasks、Speech Workbench，展示 "ASR requires macOS on Apple Silicon" 提示。
- `web/src/api/asr.ts` 提供 `useAsrCapabilities()` hook，缓存到会话。

## CLI + Web + Admin API

- Admin API：`GET /api/asr/capabilities` 无鉴权（与其他 read-only capability 一致）；其他 `/api/asr/*` 在非支持平台返回 `501 not_supported`。
- Web：Tools 导航 + ASR 页面按 capability 隐藏；主题 token 不新增。
- CLI：help 隐藏 + 命令入口平台守卫；隐藏命令 `--help` 仍能看到（clap 语义）；执行时兜底。

## Sync 边界

- capability 是本机运行时属性，不参与 sync。
- Directory Task/ASR 服务状态也不通过 sync 传播；每台机器独立。

## 实现切分

### Phase 1：编译边界迁移

- 新建 `crates/bifrost-asr`，把 runtime/timeline/planner/artifacts/subtitle/offline/profiles/resources 迁入。
- `bifrost-admin` 改为通过 `bifrost-asr` 调用。
- 补齐 stub cfg，保证非支持平台可编译。

### Phase 2：capability API

- 新增 `capabilities.rs` 与 `GET /api/asr/capabilities`。
- 单元测试覆盖平台矩阵。
- CLI 状态命令输出 capability 便于诊断。

### Phase 3：前端隐藏

- Tools 导航 + ASR 页面按 capability 隐藏。
- 已注入 ASR 深链的用户看到 unsupported 提示，避免白屏。

### Phase 4：CLI hide + human_tests

- `bifrost ai asr` / `bifrost ai voice` clap hide + 平台守卫。
- 新增 `e2e-tests/tests/test_asr_platform_gating.sh` 静态校验。
- 新增 `human_tests/asr-platform-gating.md` 并更新 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `asr_platform_support_matrix_is_apple_silicon_macos_only`：常量矩阵测试。
- `asr_capabilities_are_hidden_on_unsupported_current_platform`：当前 target 决定 capability 输出。
- `bifrost_admin_does_not_link_native_qwen3_on_stub_targets`（编译期 assert 或 cargo metadata 校验）。
- `cli_asr_command_is_hidden_on_stub_targets`。

### E2E 测试

`e2e-tests/tests/test_asr_platform_gating.sh`：

- 静态检查 Cargo target dependency、`bifrost-asr` feature 边界、native cfg、CLI hide。
- 用 `cargo metadata --filter-platform=x86_64-unknown-linux-gnu` 验证 Linux 不解析 `qwen3-asr` / `sherpa-onnx`。
- 用 `cargo metadata --filter-platform=aarch64-apple-darwin` 验证 macOS aarch64 会解析。
- 用 `cargo metadata --filter-platform=x86_64-apple-darwin` 验证 macOS x86_64 不解析。
- 用 `grep` / `rg` 保证 `bifrost-admin` 不直接依赖 native crate。

### 真实场景测试 human_tests

`human_tests/asr-platform-gating.md` 覆盖：

- TC-APG-01：macOS aarch64 上 Tools 导航显示 ASR 入口，ASR 页面正常渲染。
- TC-APG-02：非支持平台（Linux/Windows/macOS x86_64）Tools 导航不显示 ASR，直接访问 ASR 页面提示不支持。
- TC-APG-03：非支持平台 CLI `bifrost ai --help` 不显示 asr/voice；`bifrost ai asr start` 返回明确错误。
- TC-APG-04：Cargo metadata 校验 native crate 边界。
- TC-APG-05：`bifrost-admin` re-export/调用边界正确，无直接 native import。

启动 Bifrost 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-asr platform`
- `cargo test -p bifrost-admin asr_platform`
- `bash e2e-tests/tests/test_asr_platform_gating.sh`
- `pnpm --dir web exec tsc -b --pretty false`
- `rust-project-validate`
- 本机若沿用 no-local-coverage 约定，则不跑 `make coverage`；交付时说明依赖 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：平台矩阵、编译边界、capability API、CLI hide、前端隐藏。
- 复核 diff：`bifrost-asr` 新 crate、`bifrost-admin` 依赖迁移、`asr.rs` re-export、CLI clap 属性、Web hook。
- 重点 review：Cargo target dependency 是否放对位置、是否有遗漏的 native import、stub 是否引用了 native symbols。
- 复测：单元测试、`test_asr_platform_gating.sh`、前端类型检查。

### 第 2 轮

- 基于第 1 轮修复后的 diff 重新检查非支持平台是否仍有入口或 native dependency 泄漏。
- 复跑受影响测试；确认 `design/`、`human_tests/`、`human_tests/readme.md` 同步。
- 重点 review：`bifrost-admin` 是否仍有历史 `use qwen3_asr::*` 遗留。

## 风险与决策点

- Feature flag 与 target dependency 组合易踩坑：只用 target dependency 时 CI 上非支持平台不会自动开 feature，若手动 `--features full-local-asr` 会失败；本方案 default feature 为 `core`，`full-local-asr` 只在 admin 端 macOS aarch64 target dependency 中启用。
- 迁移范围大：timeline/planner/artifacts 拆分若不彻底会导致 admin 反过来引用 asr 内部 module；建议按上述模块表逐个迁移，每个模块迁移后跑一次 workspace build 兜底。
- Voice Wake 中的 ASR 触发 vs. 非 ASR 触发：Voice Wake 页面本身可能包含非 ASR 触发方式；本方案假设 Voice Wake 页面整体在非支持平台隐藏，如果未来出现"非 ASR 触发方式"需要独立 capability。
- CLI 隐藏但仍可执行：clap `hide = true` 只是不列在 help，用户手输 `bifrost ai asr start` 仍能路由到子命令；因此 `ensure_supported_platform()` 兜底必须保留。
- 非支持平台的 `bifrost-admin` 编译体积：迁移后 admin 只依赖 `bifrost-asr` core feature，不引 native；此约束需要在 CI 中通过 `cargo metadata --filter-platform=x86_64-unknown-linux-gnu` 断言。
- 未来平台扩展：如需支持 Linux CUDA 或 Apple Silicon 之外的 GPU，需要新增独立 target cfg 与 feature；capability API 已预留 `supported_target` 字段。
