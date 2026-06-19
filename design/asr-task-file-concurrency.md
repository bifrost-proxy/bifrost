# ASR Directory Task 文件级动态并发

## 背景与目标

ASR Directory Task 过去按文件串行处理，任务级运行锁保证同一任务不会重复启动，但单个任务内多个待处理音频无法利用本机多进程推理能力。用户在 `/_bifrost/ai?aiSection=tools-asr&asrTask=...&asrTaskTab=files` 观察到批量文件吞吐偏低，因此本轮目标是让任务可以动态调节文件级并发数，在设备允许时并行处理多个 ASR 文件。

用户目标验证清单：

- 必须实现：Directory Task 持久化 `max_concurrent_files`，旧任务默认保持 `1`，避免历史行为突变。
- 必须实现：WebUI 创建/编辑任务时可设置并发数，运行中保存配置可动态影响后续 worker 补位。
- 必须实现：`fork_per_chunk` 支持多文件并发，包含启用 diarization 的真实目录任务；共享 runtime 场景先降级为 effective `1`，避免共享 server 状态竞争。
- 必须不破坏：pause/force pause、实时语音资源让路、文件状态持久化、chunk 进度、失败文件继续处理、Daily Docs/Daily Agent 后处理。
- 必须真实验证：Rust 单测、前端类型/构建、API 创建/更新和 human_tests 场景能在当前仓库真实执行。

## 设计

### 配置模型

`AsrDirectoryTask` 新增：

```rust
max_concurrent_files: u8
```

输入通过 `normalize_max_concurrent_files()` 裁剪到 `1..=16`。后端同时输出：

- `summary.max_concurrent_files`：用户期望值。
- `summary.effective_max_concurrent_files`：本次任务当前实际生效值。
- watch/run progress 中的 `active_file_count`：当前活跃文件 worker 数。

### 调度策略

保留原有串行处理函数作为安全路径。新增包装调度器：

1. 每轮从 task store 读取最新 `max_concurrent_files`。
2. 当 `effective > active` 时补充新文件 worker。
3. 当用户运行中降低并发时，不杀掉已启动 ASR 子进程，只停止补新 worker，直到活跃数自然降到新上限以下。
4. 任意 worker 遇到 pause / realtime resource yield，整体任务返回 paused。
5. 调度循环每 2 秒 tick 一次，避免长文件 worker 未完成时无法响应升并发配置。

每个文件 worker 通过 `tokio::task::spawn_blocking` 进入独立阻塞线程，并在线程内创建 current-thread Tokio runtime 复用原有顺序处理逻辑。这样 ASR/diarization 子进程和本地重 CPU 路径不会饿住主 admin runtime，WebUI/API 在高并发处理时仍可响应暂停、调参和状态查询。

### 并发边界

文件级并发只对 `fork_per_chunk` 任务启用。以下场景 effective 固定为 `1`：

- `reuse_per_file`、`reuse_server`、`auto`、`compare`

原因是这些共享 runtime 路径存在共享托管 ASR server、server fallback state 和 restart context；贸然并发会把性能优化变成状态一致性风险。`fork_per_chunk` 每个 chunk 使用隔离进程，diarization 产物按文件路径隔离写入，因此真实 speaker-aware 目录任务也可以使用文件级并发。后续若要让 `reuse_per_file` 并发，需要先把托管 server 生命周期迁移为每 worker 独立 lease 或明确支持 server 多请求并发。

### 文件状态一致性

并发 worker 会同时写 `files.json`，因此保存逻辑改为按 file key 合并写入：

- 每个 worker 只携带当前文件记录进入保存路径。
- `save_file_store()` 在写入前读取最新 store，再按 key 覆盖本 worker 的记录。
- chunk progress/metric 回调也只保存当前文件 key，避免旧快照覆盖其他 worker 最新状态。

### 状态恢复与历史记录折叠

高并发真实任务中还存在两个状态一致性风险：

- `run_progress.json` 和部分 artifact 使用固定 `<path>.tmp` 临时文件名，并发写入时会互相抢 rename，产生 `No such file or directory` 的持久化警告。
- `source_key()` 包含路径、size 和 mtime。同一路径在外接导入或手动拷贝过程中 mtime 变化时会产生多条历史 key，旧的 `pending/processing` 记录会继续出现在 Files tab，让用户误以为“最后一片不结束”。

修复策略：

1. `atomic_text_write()` 使用同目录唯一临时文件名，避免并发写同一目标时共享 `.tmp`。
2. 读取、保存和启动恢复时识别完整的 `processing` 记录：没有 error、没有 failed chunks、chunk metrics 全为 `ok`、text artifact 存在、timeline/metadata 如有记录也存在。满足条件时自动收敛为 `success`，补齐 `finished_at_ms` 和完成进度。
3. Task detail、watch 和 summary 展示前按 `source_path` 折叠同路径历史记录：优先展示当前发现的 source key；否则展示已完成记录；再退回 processing/failed/pending。这样保留持久化恢复能力，同时避免旧 key 污染用户感知进度。

## 测试方案

- 单元测试：验证旧 JSON 默认 `max_concurrent_files=1`；验证 `normalize_max_concurrent_files()` 裁剪到 `16`；验证 `effective_max_concurrent_files()` 在 `fork_per_chunk` 返回用户期望值，且共享 runtime 降级为 `1`。
- 单元测试：验证并发 atomic 写不会出现固定 tmp rename 竞争；验证完整 `processing` 记录可从 artifact 自动恢复为 `success`；验证同一路径旧 `pending/processing` 记录不会污染 task detail 和 summary。
- 前端测试：`npm --prefix web run typecheck` 或构建路径验证 TS 类型与 UI 编译。
- E2E 测试：通过 ASR task API 创建 `runtime_strategy=fork_per_chunk,max_concurrent_files=3` 的任务，确认详情返回期望值；运行中 PATCH 为 `2` 后 watch/detail 返回更新值。
- E2E 测试：预置 stale `processing` + 同路径旧记录的 task store，启动临时 Bifrost 后通过真实 Admin API 验证 Files tab 数据收敛为单条 `success`。
- human_tests：`human_tests/asr-task-concurrency.md` 覆盖 API 创建、运行中调节、共享 runtime effective 降级、WebUI 字段展示、状态恢复和历史记录折叠。
- coverage 90% 门禁：收尾运行 `make coverage`；若本地 ASR/E2E 环境不可用，退化为 `make coverage-unit` 并记录原因。

## Review/Fix/Test 闭环

第 1 轮：

- 复核目标：确认并发配置、动态调整、effective 降级和 files.json 合并写入都在 diff 中。
- 代码 review：检查 `run_directory_task`、`process_pending_files_parallel_fork`、blocking worker 隔离、调度 tick、`save_file_store`、API create/update、WebUI 表单。
- 测试运行：分别执行 `cargo test -p bifrost-admin max_concurrent_files_is_clamped_and_effective_for_fork_per_chunk`、`cargo test -p bifrost-admin runtime_strategy_defaults_to_reuse_per_file_for_old_task_json` 和 `cargo test -p bifrost-admin running_task_allows_concurrency_update_but_rejects_runtime_risk`，以及前端类型/构建。

第 2 轮：

- 复核第 1 轮修复后的最新 diff，重点检查运行中 PATCH 是否被 high-risk guard 错误拦截。
- 复查 human_tests 索引和 WebUI 字段文案。
- 复跑受影响 Rust/Web 测试，确认无需第 3 轮。
