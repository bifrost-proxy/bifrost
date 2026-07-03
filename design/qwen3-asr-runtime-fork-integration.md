# Qwen3-ASR Runtime Fork 集成设计方案

## 背景

Bifrost 的 ASR 目录任务与实时转写能力依赖 `qwen3_asr_rs`（second-state 上游）提供的 `asr` CLI 与 `asr-server` HTTP 服务。当前 Bifrost 通过 release 二进制下载并使用外部进程 watchdog 监督其生命周期，本身不参与该运行时的编译。

原始设计（2026-04）目标是维护一份 `vendor/qwen3_asr_rs` 源码镜像并向上游打补丁，最终切换到私有 fork release，用以：

- 暴露 MLX 内存/缓存/wired 上限控制。
- 每次请求后主动 `clear_cache()` 并复位 peak。
- 提供 `--max-requests`、`--max-peak-memory-mb` 等生命周期钮，让 Bifrost 能在长跑批任务时清洁地重启 server。

截至 2026-07-03（本次核对）：**vendor 源码镜像仍未落地**。`ls /home/mira/bifrost-src/vendor/` 只返回 `sysproxy`，没有 `qwen3_asr_rs` 目录；`ASR_RELEASE_REPO` 常量在 `crates/bifrost-admin/src/handlers/asr.rs:50` 与 `crates/bifrost-cli/src/commands/asr.rs:31` 上仍指向 `"second-state/qwen3_asr_rs"`。fork 相关 FFI/CLI/lifecycle 改动未合入上游，也未在 Bifrost 侧启用。

Bifrost 端的一系列稳态补偿逻辑（策略切换、breaker、watchdog 加固、bulk 重试、ASR jobs 模块拆分）**已经落地**并有单元测试覆盖，需要在本设计中作为“既成事实”与 fork 目标区分记录。

## 用户目标验证清单

### 必须实现（fork 相关，尚未 ship）

- 维护 `second-state/qwen3_asr_rs` 的 fork 源码镜像，Cargo workspace **不引入**该目录做常规编译（CI/开发构建不涉及 MLX + CMake）。
- 在 fork 中新增 `mlx_set_memory_limit`、`mlx_set_cache_limit`、`mlx_set_wired_limit`、`mlx_clear_cache`、`mlx_get_active_memory`、`mlx_get_cache_memory`、`mlx_get_peak_memory`、`mlx_reset_peak_memory` 的 FFI 与 safe wrapper。
- `asr` / `asr-server` 均支持 `--mlx-memory-limit-mb`、`--mlx-cache-limit-mb`、`--mlx-wired-limit-mb` 及对应 env `QWEN3_ASR_MLX_*_MB`，在 `AsrInference::load(...)` 之前应用。
- `asr-server` 每次请求返回后可 `synchronize + clear_cache + reset_peak_memory`，默认开启，可用 `--mlx-clear-cache-after-request=false` 关闭做基准。
- `asr-server` 支持 `--max-requests <N>` 与 `--max-peak-memory-mb <N>`：达到阈值仍要完成当前请求再退出，供 Bifrost 干净重启。
- `/health` 附带可选 MLX 内存字段，供 Bifrost 采样。
- 上述 fork 发布 release 后，将 Bifrost 常量切换到 fork 仓库，并保留一个 config field 作为可回退开关。

### 必须实现（Bifrost 端，已 ship）

- ASR 目录任务的 5 种运行时策略：`fork_per_chunk`、`reuse_server`、`reuse_per_file`（默认）、`auto`、`compare`，见 `crates/bifrost-admin/src/handlers/asr_jobs/state.rs` 中 `AsrRuntimeStrategy` 枚举。
- 每策略必须发出 `ASR chunk metric` 日志并落 `chunk_metrics` 到 `files.json` 及 metadata，crash/watchdog kill 也留证据。
- 单文件/任务级 failed chunk 重试：`POST /api/asr/tasks/{task_id}/retry-failed-chunks` 与 `POST /api/asr/tasks/{task_id}/files/{file_key}/retry-chunks`，见 `asr_jobs/api.rs`、`asr_jobs/retry.rs`。
- 服务失败断路器：`ServerRunnerState.server_failures`、`restart_required`、`force_fork_for_remaining`，见 `asr_jobs/chunk_runtime.rs`；受 env `BIFROST_ASR_MAX_SERVER_FAILURES_PER_FILE`、`BIFROST_ASR_MAX_SERVER_FAILURES_PER_TASK` 调节。
- 请求超时上界：`BIFROST_ASR_TEXT_REQUEST_TIMEOUT_SECS`（默认 45s，见 `asr_streaming.rs:37`）、`BIFROST_ASR_SERVER_REQUEST_TIMEOUT_SECS`（duration-aware，见 `asr_streaming.rs:36`）、fork 分片重试用 `BIFROST_ASR_CHUNK_TIMEOUT_SECS` / `BIFROST_ASR_MIN_CHUNK_TIMEOUT_SECS` / `BIFROST_ASR_TIMEOUT_MULTIPLIER`（`asr_cli_invoke.rs:31-33`）。
- watchdog 只在**可靠的 physical footprint** 采样超限时才 kill 管理进程；RSS-only fallback 与采样错误只做 rate-limited 告警。
- `asr_jobs/` 目录化拆分（`state.rs / api.rs / retry.rs / runner.rs / chunk_runtime.rs / memory_bisect.rs / audio_processing.rs / store.rs / diarization.rs / voiceprint.rs / external_import.rs / daily_agent*.rs / tests.rs`），通过 `include!` 保持原 visibility。

### 必须不破坏

- 不引入 MLX + CMake 到 Bifrost 常规编译路径；跨平台开发/CI 构建速度与稳定性保持现状。
- release 下载与 watchdog 生命周期保持外部进程模型；不将 ASR 变为内嵌 crate。
- 现有 `reuse_per_file` 默认策略与已知稳态补偿（服务死亡后的 current-chunk fork fallback + 延迟 server 重启）语义不变。
- 已通过的单元测试断言不回归（列举见测试方案）。

### 必须真实验证

- Apple Silicon 上使用 fork 二进制的启动、日志内存限额、`clear_cache=true/false` 对比、`~/Downloads/we` 长跑稳定性、1801s 音频 <5 分钟 wall 时间、`--max-requests` 干净退出后 Bifrost 能自动重启。

## 产品语义

### 双轨路线

1. **上游 fork release 路线（planned）**：所有 MLX/lifecycle 补丁进入 fork，通过 release 二进制交付给 Bifrost。
2. **Bifrost 集成路线（shipped）**：Bifrost 只做“运行时选择 + 稳态补偿 + 断路器 + 重试 + 观测”，不参与 MLX 编译。

### 运行时策略语义（已实现）

`AsrRuntimeStrategy` 定义在 `asr_jobs/state.rs`：

- `ForkPerChunk`：每 chunk 一个原生 `asr` 进程，最强隔离。
- `ReuseServer`：整个任务运行期间共用一个 `asr-server`，最快但需要 breaker 保护。
- `ReusePerFile`（默认）：每个源文件启一个 file-scoped `asr-server`，文件结束后停止。
- `Auto`：以 server 起手，出错回退当前 chunk 到 fork，并对后续 chunks 调度 server 重启。
- `Compare`：以 fork 结果为准，同时用 server 跑一份 shadow 记录 metrics 与 text hash 差异。

### 稳态补偿（已实现）

- **单 chunk 服务错误恢复**：立刻用 fork 重跑当前 chunk；标记 `restart_required=true`；下一 server-eligible chunk 前重启 server；`fallback_reason` 明确写为 `<strategy> strategy <transport|server> failure; retrying current chunk via fork_per_chunk and scheduling managed ASR server restart for later chunks: <error>`。
- **任务级共享 state**：`reuse_server` / `auto` / `compare` 使用 task-scoped `ServerRunnerState`；`reuse_per_file` 保留 file-scoped。
- **watchdog 三态**：可靠 physical footprint 超限 → kill；RSS-only fallback → 建议日志；sampler 错误 → 保活并 rate-limit 告警。
- **断路器**：`server_failures` 达阈值 → `force_fork_for_remaining=true`，剩余 chunks 走 fork 隔离；`fallback_reason` 显式含 `switching remaining chunks to fork_per_chunk isolation`。
- **timeout**：流式文本 45s、whole-file `verbose_json` duration-aware、fork 分片 `chunk_duration_secs * multiplier` 夹 `[min, 120]`。

## 技术细节

### fork runtime patch（planned）

`src/backend/mlx/ffi.rs` 增加 extern 声明；`src/backend/mlx/memory.rs` 提供：

```rust
pub fn set_memory_limit(bytes: usize) -> Result<usize>;
pub fn set_cache_limit(bytes: usize) -> Result<usize>;
pub fn set_wired_limit(bytes: usize) -> Result<usize>;
pub fn clear_cache() -> Result<()>;
pub fn stats() -> MlxMemoryStats;
pub fn reset_peak_memory() -> Result<()>;
```

启动顺序：初始化 MLX device → 应用 limits → 日志 effective stats → `AsrInference::load(...)`。

`asr-server` 请求闭合流程：

1. 加锁执行 `AsrInference::transcribe(...)`
2. 归还 mutex（drop 请求 local tensors）
3. `synchronize()`
4. 若启用则 `clear_cache()`
5. 采样 `active/cache/peak`
6. `reset_peak_memory()`

### Bifrost 侧关键类型/文件（已 ship）

| 组件 | 位置 |
| --- | --- |
| `AsrRuntimeStrategy`、请求/响应结构 | `crates/bifrost-admin/src/handlers/asr_jobs/state.rs` |
| 路由与 API 响应 | `crates/bifrost-admin/src/handlers/asr_jobs/api.rs` |
| 单文件/任务级失败重试 | `crates/bifrost-admin/src/handlers/asr_jobs/retry.rs` |
| 调度器与文件流水线 | `crates/bifrost-admin/src/handlers/asr_jobs/runner.rs` |
| chunk 策略调度、`ServerRunnerState`、metrics | `crates/bifrost-admin/src/handlers/asr_jobs/chunk_runtime.rs` |
| 内存 hint 与递归 bisect | `crates/bifrost-admin/src/handlers/asr_jobs/memory_bisect.rs` |
| ffmpeg/normalize/WAV RMS | `crates/bifrost-admin/src/handlers/asr_jobs/audio_processing.rs` |
| task/file store | `crates/bifrost-admin/src/handlers/asr_jobs/store.rs` |
| 流式/CLI 请求 timeout env | `crates/bifrost-admin/src/handlers/asr_streaming.rs`、`asr_cli_invoke.rs` |
| ASR release repo 常量 | `crates/bifrost-admin/src/handlers/asr.rs:50`、`crates/bifrost-cli/src/commands/asr.rs:31` |

### CLI / Web / Admin API

- CLI：`bifrost asr install/update` 等命令沿用当前 release-based 拉取；fork 切换后仅改动 `ASR_RELEASE_REPO` 常量与 install 目标校验。
- Web：任务详情提供 “Retry all failed chunks” 按钮，仅在 `summary.failed_chunk_count > 0` 时可点，polling 显示 `bulk_retry`（`status/queued_files/processed_files/current_file_key/current_source_path/total_failed_chunks/recovered_chunks/still_failed_chunks`）。
- Admin API：
  - `POST /api/asr/tasks/{task_id}/retry-failed-chunks`（任务级 bulk）
  - `POST /api/asr/tasks/{task_id}/files/{file_key}/retry-chunks`（单文件精细）
  - 现有 `POST /api/asr/tasks` 等入口不变，`strategy` 字段接受 `AsrRuntimeStrategy` 变体。

### Sync 边界

- 本设计不涉及 Rules/Group 同步，不参与 sync-server 语义。
- 任务 metadata、files.json、daily agent Markdown 归档在本机数据目录，跨设备复制不属于本方案。

## Phase 1 – Bifrost 侧稳态补偿（已完成）

- 策略枚举、metrics、fallback_reason、断路器、watchdog rate-limit、timeout env、asr_jobs 目录拆分、bulk retry。

## Phase 2 – fork 源码镜像与补丁（planned）

- fork `second-state/qwen3_asr_rs`；补 MLX 内存 FFI + safe wrapper；补 CLI/env flag；补 `clear_cache` 与生命周期钮。

## Phase 3 – fork release 与 Bifrost 切换（planned）

- 从 fork 出 release；本机验证四类场景通过；将 `ASR_RELEASE_REPO` 迁移到 fork；保留 config field 支持回退。

## Phase 4 – 文档与迁移收尾（planned）

- 更新 README/human_tests 索引；将 fork release URL 写入 install/update 校验；日志中记录“已从 fork xxx@commit 拉取”。

## 测试方案

### 已通过的单元测试

- `asr_runtime_timeouts_are_bounded_for_short_chunks`（`asr_jobs/tests.rs:1509`）
- `server_failure_recovery_reason_uses_fork_for_current_chunk`（`asr_jobs/tests.rs:1525`）
- `server_failure_breaker_switches_remaining_chunks_to_fork`（`asr_jobs/tests.rs:1536`）
- `reuse_server_failure_threshold_forces_remaining_fork_isolation`（`asr_jobs/daily_agent_tests.rs:851`）
- （文档提及的）`service_watchdog_warning_log_is_rate_limited` 归入 asr_jobs 测试组；若未在当前 checkout 命名一致，实现时需保持行为断言。

### fork ship 后需新增

- FFI wrapper roundtrip：`set_memory_limit/set_cache_limit/set_wired_limit/clear_cache/stats/reset_peak_memory`。
- CLI/env 解析优先级测试：flag > env > default。
- `--max-requests` 与 `--max-peak-memory-mb` 触发后仍完成当前请求再退出。

### E2E 测试

- `e2e-tests/tests/test_qwen3_asr_runtime_guards.sh`（已存在于当前 checkout，Bifrost 端 Rust 断言，不下载 Qwen3-ASR 模型）。
- fork ship 后：Apple Silicon 真机 build 与 30 分钟长跑基准。

### 真实场景测试（human_tests）

- `human_tests/asr-scheduled-task-plan-b.md`：已覆盖策略、timeout 与 breaker（第 15 行、988 行等）。
- 新增 `human_tests/qwen3-asr-runtime-fork.md`（fork ship 后）：
  - TC-QAF-01 fork 二进制在 Apple Silicon 启动，日志报告 effective limits。
  - TC-QAF-02 `~/Downloads/we` 长跑 ≥100 chunks 无失败、无 physical footprint 超限、无 RTF 递降。
  - TC-QAF-03 `--max-requests N` 完成当前请求后退出，Bifrost 自动重启。
  - TC-QAF-04 `clear_cache=false` 与 `true` 对比：peak / active / cache 三项曲线可复现。

### 验收门槛

- Build：`cargo build --release --no-default-features --features mlx`；`asr` 和 `asr-server` 均可 Apple Silicon 启动。
- Memory limit：过低值必须干净失败。
- Lifecycle：`--max-requests` 与 `--max-peak-memory-mb` 达阈值都是响应结束后退出；pause/force-pause 仍能 kill 整个进程组。

### 覆盖率与项目校验

- 实现阶段执行 `cargo fmt --all -- --check`、相关 crate 单元测试、`cargo test --workspace --all-features`、`rust-project-validate`。
- 本机不跑 `make coverage` 系列；依赖远端 CI 看护。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 核对 fork patch 是否漏 FFI symbol；`clear_cache` 是否在 `AsrInference::transcribe(...)` 返回后调用；`--max-requests` 是否在响应写回之后退出。
- 核对 Bifrost 侧策略回退：failed chunk 是否被同时算入 `server_failures`；`restart_required` 是否阻止并发 server/fork 竞争 unified memory。
- 复测：`asr_jobs/tests.rs`、`daily_agent_tests.rs` 相关用例；e2e_qwen3_asr_runtime_guards 脚本。

### 第 2 轮

- 复检 fork release 切换后的下载 URL、签名、install 目录布局。
- 复检 human_tests 中 30 分钟长跑与目录任务基准是否可复现。
- 复测：全量 asr_jobs 单测、`test_qwen3_asr_runtime_guards.sh`、人工目录任务。

## 风险与决策点

- **不 vendor 源码**：Bifrost 常规构建不引入 MLX + CMake 的决定是安全边界；未来即便有 patch 也走 fork release 分发。
- **默认策略保守**：`ReusePerFile` 是当前默认。fork 稳定前，目录批处理不切换回长跑 server；WebUI 单文件上传与 service 模式可先切换。
- **watchdog 阈值**：1.7B 模型保持 18 GiB 上限；fork release 未稳定前不上调。
- **参数命名冲突**：fork 引入 `QWEN3_ASR_MLX_*` env 系列，与 Bifrost 已有 `BIFROST_ASR_*` 系列分层清晰，避免语义混淆。
- **共存 upstream 与 fork**：`ASR_RELEASE_REPO` 保留 config field；升级失败时可即时回退到 upstream release。
