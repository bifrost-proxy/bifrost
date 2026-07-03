# ASR Directory Task 文件级动态并发

## 背景

ASR Directory Task 过去按文件串行处理：任务级运行锁保证同一任务不会重复启动，单个任务内的多个待处理音频文件依次进入 Qwen3-ASR 转写。这一模型能给出稳定的资源占用曲线，但当用户批量导入几十个音频（会议录音、外接设备）时，本机的多进程推理能力被浪费——`fork_per_chunk` 每 chunk 会 spawn 独立子进程，理论上完全可以并行地为多个文件分别 fork，只有共享 runtime 场景才存在状态耦合。

用户从 `/_bifrost/ai?aiSection=tools-asr&asrTask=...&asrTaskTab=files` 观察到吞吐偏低，反馈"CPU 明显没吃满"。因此本轮目标是让 Directory Task 支持文件级动态并发：任务持久化用户期望的并发上限、运行中可以调节、并按 runtime strategy 与实时语音资源让路自动降级到安全值。

顺带修复两个隐藏的状态一致性问题：并发写入 `run_progress.json` / artifact 时固定 `<path>.tmp` 抢 rename 导致的持久化警告；同一路径旧 `pending/processing` 记录在 mtime 变更后仍污染 Files tab 的"最后一片不结束"感知。

## 用户目标验证清单

### 必须实现

- `AsrDirectoryTask` 持久化字段 `max_concurrent_files: u8`，旧任务反序列化时默认 `1`，避免历史行为突变。
- API 创建、更新任务时接受 `max_concurrent_files`，运行中 PATCH 也能生效，下一轮调度周期开始按新配置补 worker。
- WebUI 创建/编辑任务表单包含并发数输入，展示 desired 与 effective 值，突出差异原因。
- `runtime_strategy=fork_per_chunk` 任务真实并行执行多个文件 worker，包含启用 diarization 的目录任务。
- `reuse_per_file` / `reuse_server` / `auto` / `compare` 场景，effective 强制降级为 `1`，避免共享托管 ASR server 状态竞争。
- watch/detail/summary 输出 `active_file_count`，任意 worker 遇到 pause 或实时语音资源让路时整体任务收敛为 paused。
- 并发写入 `files.json` 使用按 file key 合并的保存路径，多个 worker 同时保存不会互相覆盖对方的最新记录。
- `atomic_text_write()` 使用同目录唯一临时文件名，`run_progress.json` 等高频写路径不再因 `.tmp` 抢 rename 输出"No such file or directory"。
- 完整且干净的 `processing` 历史记录（无 error、无 failed chunks、chunk metrics 全 `ok`、artifact 存在）在启动或读取时自动收敛为 `success`。
- Task detail、watch 和 summary 展示前按 `source_path` 折叠同路径历史记录：优先展示当前发现的 source key，再依次退回 success / processing / failed / pending。

### 必须不破坏

- pause / force pause / resume 语义、实时语音资源让路（realtime resource yield）、外接设备导入 pipeline。
- 文件状态持久化、chunk-level 进度回调、失败文件继续处理下一个、Daily Docs 与 Daily Agent 后处理。
- Directory Task 单文件顺序处理的原有逻辑保留为安全路径，effective=1 时行为与历史版本一致。
- ASR Admin API 现有字段兼容性，`max_concurrent_files` 在旧客户端缺省仍返回 `1`。
- WebUI Files tab、Runs tab、Overview tab 现有筛选/分页/状态徽标语义。

### 必须真实验证

- `cargo test -p bifrost-admin` 覆盖 clamp、默认值、共享 runtime 降级、running 状态下 PATCH 允许并发调节但拒绝 runtime 策略切换等场景。
- 前端 `npm --prefix web run typecheck` / build 通过，新字段和文案编译无误。
- E2E：真实临时 Bifrost 启动后通过 Admin API 创建 `fork_per_chunk,max_concurrent_files=3` 任务，运行中 PATCH 为 `2`，验证 watch/detail 反映最新值。
- E2E：预置带 stale `processing` 及同路径历史 key 的 task store，启动 Bifrost 后 Files tab 收敛为单条 `success`。
- human_tests：`human_tests/asr-task-concurrency.md` 覆盖 API 创建、动态调节、共享 runtime effective 降级、WebUI 字段展示、状态自动恢复和历史折叠。

## 产品语义

### desired vs effective 并发

任务持久化两个并发概念，禁止合并：

- `max_concurrent_files`：用户期望值，1..=16。持久化在 task JSON 里，也在 API 请求体、WebUI 表单里出现。
- `effective_max_concurrent_files`：当前实际生效值，由 `effective_max_concurrent_files(&task)` 函数从 `runtime_strategy + max_concurrent_files` 派生：
  - `fork_per_chunk`：返回 clamp 后的用户期望值。
  - 其它 strategy：强制返回 `1`。

WebUI 明确用不同的字段展示这两者，且当二者不一致时给出简短原因（"共享 runtime 策略暂不支持并发"）。用户看到的是"desired=3, effective=1"这样的差异，不是被静默降级为 1 却看不到线索。

### 动态调节而不是重启任务

允许用户在任务 running 状态下 PATCH `max_concurrent_files`。语义：

- 增加并发：下一轮调度 tick 补新的文件 worker，直到达到新的 effective 上限。
- 降低并发：不杀掉已启动 ASR 子进程，只停止补新 worker，直到活跃数自然降到新上限以下。
- 切换 `runtime_strategy`：running 状态下拒绝，避免既有 chunk 处理路径中途变换。

这个语义能让用户看着实际吞吐"边试边调"，符合 batch 处理场景的操作习惯。

### 共享 runtime 降级的原因

`reuse_per_file`、`reuse_server`、`auto`、`compare` 都共享同一个托管 ASR server 或存在 server fallback / restart context，多文件并发进入这条路径会把性能优化变成状态一致性风险：

- 托管 asr-server 生命周期由 task 级 lease 持有，跨 worker 并发共享同一个 lease 的 lifecycle 无法保证正确 shutdown 顺序。
- fallback state 与 restart context 是每 task 一份，并发写入会互相覆盖 last error / retry count。

`fork_per_chunk` 天然隔离：每个 chunk 拉起独立子进程，diarization 产物按文件路径隔离写入，因此真正 speaker-aware 目录任务也可以文件级并发。后续若要放开 `reuse_per_file` 并发，需要先把托管 server 生命周期迁移为 per-worker lease 或明确支持 server 多请求并发，属于另一个专题。

## 技术细节

### 配置模型

`crates/bifrost-admin/src/handlers/asr_jobs/state.rs`：

```rust
pub struct AsrDirectoryTask {
    // ...
    #[serde(default = "default_max_concurrent_files")]
    pub max_concurrent_files: u8,
    // ...
}

fn default_max_concurrent_files() -> u8 { 1 }

fn normalize_max_concurrent_files(value: u8) -> u8 {
    value.clamp(1, 16)
}

fn effective_max_concurrent_files(task: &AsrDirectoryTask) -> u8 {
    if matches!(task.runtime_strategy, RuntimeStrategy::ForkPerChunk) {
        normalize_max_concurrent_files(task.max_concurrent_files)
    } else {
        1
    }
}
```

API summary / detail / watch 中同时暴露 `max_concurrent_files` 和 `effective_max_concurrent_files`。

### 调度策略

保留 `process_pending_files_sequential_*` 作为安全路径。新增 `process_pending_files_parallel_fork()`，包装 sequential 逻辑：

1. 每轮从 task store 读取最新 `effective_max_concurrent_files(&task)`。
2. 当 `effective > active` 时，从 pending 列表取下一个未启动文件补充 worker。
3. 每个 worker 通过 `tokio::task::spawn_blocking` 进入独立阻塞线程，线程内创建 current-thread Tokio runtime 复用 sequential 处理逻辑。这样重 CPU 与 ASR 子进程等待不会饿死 admin runtime。
4. 任意 worker 遇到 pause / realtime resource yield，标记整体任务 paused，等待恢复后重进调度。
5. 调度循环每 2 秒 tick，避免长文件 worker 未完成时无法响应用户升并发。

伪码：

```rust
loop {
    let task = store.read(task_id)?;
    let effective = effective_max_concurrent_files(&task);
    let active = worker_pool.active_count();
    if paused_signal.received() { break; }
    while active < effective as usize {
        if let Some(file) = pending_queue.pop() {
            worker_pool.spawn_blocking_worker(file);
        } else { break; }
    }
    tokio::time::sleep(TICK_INTERVAL).await;
    if worker_pool.all_finished() && pending_queue.is_empty() { break; }
}
```

### 并发边界

- `runtime_strategy=fork_per_chunk` 生效多文件并发。
- 其它 runtime strategy 强制 effective=1，即便用户传 `max_concurrent_files=8`，实际只跑 1。
- WebUI 应在切换 runtime strategy 时同步给出提示：切到共享 runtime 会退化为串行。

### 文件状态一致性

并发 worker 同时写 `files.json`，`save_file_store()` 改为按 file key 合并写入：

- Worker 只持有当前文件 record 进入保存路径。
- `save_file_store()` 先读取磁盘最新 store，再按 key 覆盖本 worker 的记录，最后 atomic write 全量回盘。
- chunk progress/metric 回调只保存当前文件 key，避免旧快照覆盖其他 worker 最新状态。

`atomic_text_write()` 使用同目录唯一临时文件名（`<path>.<uuid>.tmp`），不再共享 `.tmp` 后缀。这样并发写同一目标文件时不会互相 rename 覆盖，也不会出现"目标 tmp 消失"的 warning。

### 状态恢复与历史记录折叠

两个隐藏问题：

1. `run_progress.json` 使用固定 `<path>.tmp` 临时文件名，并发写入时会出现 `No such file or directory` 的持久化警告。
2. `source_key()` 包含路径、size 和 mtime。外接导入或手动拷贝会改变 mtime，产生多条历史 key，旧的 `pending/processing` 记录继续出现在 Files tab，让用户误以为"最后一片不结束"。

修复策略：

1. `atomic_text_write()` 使用唯一临时文件名。
2. 读取、保存和启动恢复时识别完整的 `processing` 记录：无 error、无 failed chunks、chunk metrics 全 `ok`、text artifact 存在、timeline/metadata 记录（如有）也存在。满足条件时自动收敛为 `success`，补齐 `finished_at_ms` 与完成进度。
3. Task detail、watch 和 summary 展示前按 `source_path` 折叠同路径历史记录：优先当前 source key；否则展示已完成记录；再退回 processing/failed/pending。这样保留持久化恢复能力，同时避免旧 key 污染用户感知。

## Admin API

### 创建任务

`POST /api/asr/jobs/directory` 接受：

```json
{
  "root_dir": "<USER_HOME>/audio",
  "runtime_strategy": "fork_per_chunk",
  "max_concurrent_files": 3,
  "diarization": {"enabled": true, "profile": "sherpa-onnx-balanced"}
}
```

服务端归一化 `max_concurrent_files` 到 1..=16，与 runtime_strategy 组合派生 effective。响应体包含两个字段。

### 更新任务

`PATCH /api/asr/jobs/directory/{id}` 接受 `max_concurrent_files: Option<u8>`。running 状态下：

- 单独调整 `max_concurrent_files` 或 `diarization.*` 允许。
- 修改 `runtime_strategy` / `root_dir` 返回 400 + 明确 code `task.locked_field_running`。

### 详情与 watch

`GET /api/asr/jobs/directory/{id}` 与 `GET /api/asr/jobs/directory/{id}/watch` 增加：

```json
{
  "summary": {
    "max_concurrent_files": 3,
    "effective_max_concurrent_files": 3,
    "active_file_count": 2
  }
}
```

## CLI

保持 CLI 主入口在 admin API 上。`bifrost ai asr task ...` 家族的 create/update 命令暴露 `--max-concurrent-files N`，help 文本说明只有 `fork_per_chunk` 会生效。`bifrost ai asr task show/watch` 输出 desired / effective / active count 三个数字。

```text
Task 42 (running):
  runtime_strategy: fork_per_chunk
  max_concurrent_files (desired): 3
  effective_max_concurrent_files:  3
  active_file_count:              2
```

## Web UI

Directory Task 创建/编辑表单：

- 新增"文件并发数"数字输入（1..=16），默认 1。
- runtime strategy 非 `fork_per_chunk` 时输入禁用并置灰，提示"共享 runtime 策略暂不支持并发，effective=1"。
- Files tab 顶栏显示 desired/effective/active 三值，effective 与 desired 不同时用 warning 色。
- Files tab 表格按 source_path 折叠同路径历史记录，只展示折叠后的一行；hover / 展开子行时可以看到被折叠掉的旧 key 供诊断。

## Sync 边界

Directory Task 不参与本地 Rule Sync。`max_concurrent_files` 是设备本地资源与吞吐的调节，跨设备同步没有意义。ASR task store 已在本地，本方案不引入新的 sync channel。

对未来"跨设备任务状态同步"如果发生（尚无设计），`max_concurrent_files` 属于本地覆盖字段，不应从远端拉取回本机。

## 实现切分

### Phase 1：字段与降级

- `AsrDirectoryTask` 增加 `max_concurrent_files`；反序列化默认 1。
- 实现 `normalize_max_concurrent_files` / `effective_max_concurrent_files`。
- API 创建/更新接受与暴露新字段；running PATCH 允许调节但拒绝 runtime 策略切换。
- 单元测试覆盖 clamp、旧任务默认、共享 runtime 降级、running PATCH 边界。

### Phase 2：并行调度器

- 新增 `process_pending_files_parallel_fork()`；封装 blocking worker。
- runner 根据 `effective_max_concurrent_files` 分派 parallel / sequential 路径。
- 调度 tick 2s；pause / yield 语义汇总到整体任务状态。
- watch/detail 上报 `active_file_count`。

### Phase 3：一致性修复

- `atomic_text_write()` 唯一临时文件名。
- `save_file_store()` 按 key 合并写入。
- 启动恢复识别完整 processing 记录并收敛为 success。
- 展示层按 `source_path` 折叠历史记录。

### Phase 4：Web UI 与文档

- Web 表单、Files tab、Overview tab 展示 desired/effective/active。
- 更新 `human_tests/asr-task-concurrency.md`，追加动态调节和一致性修复用例。
- 更新 CLI help 与 docs。

## 测试方案

### 单元测试

- `runtime_strategy_defaults_to_reuse_per_file_for_old_task_json`：旧任务 JSON 无 `max_concurrent_files` 时反序列化为 1，runtime 默认走安全路径。
- `max_concurrent_files_is_clamped_and_effective_for_fork_per_chunk`：clamp 到 1..=16；`fork_per_chunk` effective 等于 desired。
- `effective_max_concurrent_files_downgrades_for_shared_runtime`：`reuse_per_file`/`reuse_server`/`auto`/`compare` 全部 effective=1。
- `running_task_allows_concurrency_update_but_rejects_runtime_risk`：running 状态 PATCH 只允许并发字段；改 runtime strategy 或 root_dir 返回 400。
- `save_file_store_merges_per_key`：模拟两个 worker 同时保存不同 file key，最终 store 保留双方最新记录。
- `atomic_text_write_uses_unique_tmp_name`：并发写同一目标，没有出现 tmp 抢 rename。
- `stale_processing_record_recovers_when_artifacts_present`：预置完整 processing 记录，读取后自动收敛为 success。
- `folded_history_prefers_current_source_key`：同路径存在旧 pending + 新 success，展示层只输出 success 那条。

### E2E 测试

- 扩展 `e2e-tests/tests/test_asr_task_concurrency.sh`（或等价脚本）：
  1. 启动临时 Bifrost（`BIFROST_DATA_DIR` 隔离、非 9900 端口、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`）。
  2. 创建 `fork_per_chunk,max_concurrent_files=3` Directory Task，指向预置的多文件目录。
  3. Poll watch，观察 `active_file_count` 上升到 3；PATCH desired=2，观察 active 自然下降。
  4. 创建 `reuse_per_file,max_concurrent_files=8` 任务，观察 effective=1。
- `test_asr_task_files_recovery.sh`：预置 stale processing + 同路径旧 key 的 task store，启动 Bifrost 后 Files tab（通过 API）收敛为单条 success。

### human_tests

`human_tests/asr-task-concurrency.md` 已存在 TC-ASR-CONC-01..06；本轮验证清单沿用同一编号：

- TC-ASR-CONC-01 创建任务时保存 desired 并发。
- TC-ASR-CONC-02 运行中 PATCH 动态调低 desired。
- TC-ASR-CONC-03 共享 runtime 策略 effective 降级为 1。
- TC-ASR-CONC-04 WebUI 展示 desired/effective/active。
- TC-ASR-CONC-05 完整 processing 记录自动恢复为 success。
- TC-ASR-CONC-06 同一路径旧记录不污染 Files tab。

若一致性修复需要额外真实场景（如 externally-imported 目录），新增 TC-ASR-CONC-07 覆盖 mtime 变化后的历史折叠展示。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin` 相关子集
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 前端 `npm --prefix web run typecheck` 与构建
- `make coverage`；ASR/E2E 本地不可用时退化 `make coverage-unit` 并在交付里记录原因

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标：并发字段、动态调节、effective 降级、files.json 合并写入、一致性修复五条都在 diff 里可查。
- 代码 review：`run_directory_task`、`process_pending_files_parallel_fork`、blocking worker 隔离、调度 tick、`save_file_store`、`atomic_text_write`、API create/update、WebUI 表单与 Files tab 折叠。
- 测试执行：三条关键单测 (`max_concurrent_files_is_clamped_and_effective_for_fork_per_chunk`、`runtime_strategy_defaults_to_reuse_per_file_for_old_task_json`、`running_task_allows_concurrency_update_but_rejects_runtime_risk`) 全绿；前端 typecheck 通过；两条 E2E 通过。

### 第 2 轮

- 复核第 1 轮修复后 diff，重点检查运行中 PATCH 是否被 high-risk guard 错误拦截。
- 复查 human_tests 索引和 WebUI 文案。
- 复跑受影响的 Rust/Web 测试；确认无需第 3 轮。
- 抽样跑 human_tests TC-ASR-CONC-02/03，验证 desired vs effective 差异展示与用户可感知信号。

## 风险与决策

- **共享 runtime 是否也可以并发**：短期决策不放开。托管 ASR server 目前是 per-task lease，跨 worker 共享需要重写生命周期管理和 server 侧多请求并发能力，收益 / 风险比不高。
- **`max_concurrent_files` 上限选 16**：本机 CPU / GPU 资源无法支持更多的 fork_per_chunk 并发；ASR 子进程内存足以让 8+ 并发在低端机器上 OOM。因此 clamp 上限保守。
- **动态调节而不是 restart**：优先支持用户 running 中调低并发，保护已启动 chunk 不被中断；接受"降低生效滞后到 worker 自然完成"的语义，避免残留半成品。
- **状态自动恢复的边界**：只有 chunk metrics 全 `ok`、artifact 存在、无 error 时才恢复为 success。任何缺失都保持 processing，等用户手动 retry。避免把损坏任务误标成成功。
- **历史折叠不删除旧记录**：折叠只影响展示层，磁盘上仍保留旧 key 便于诊断；未来加"清理旧历史"按钮时再动 store。
- **前端表单与 runtime 联动的 UX**：切换 runtime strategy 时同步禁用并发字段并给出提示，避免"填了 8 却只跑 1"的困惑。
