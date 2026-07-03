# Async Traffic Writer

## 背景

`crates/bifrost-admin/src/async_traffic.rs` 是 Bifrost 主端口和临时端口写入流量记录、后续更新（状态、耗时、错误、response body）到 `TrafficDbStore` 的异步管道。请求路径上不能同步等待 SQLite/RocksDB 写入完成，否则会推高代理延迟并把 IO 抖动放大到用户流量上；因此所有 `TrafficRecord` 和 `TrafficUpdater` 都通过一个 tokio mpsc channel 投递给后台 processor，由 processor 按批合并写入。

在 2026-05-04 前后，`cargo test --workspace --all-features` 会偶发失败：

- `async_traffic::tests::test_async_traffic_update`：固定 `50ms` sleep 后读库，processor 尚未消费 update。
- `async_traffic::tests::test_batch_processing`：固定 `100ms` sleep 观察 100 条记录写入，但 processor 一轮最多批 512 条，测试机器慢时仍可能只观察到第一批。

这些失败是测试等待策略过于粗糙，不是生产逻辑需要扩大同步阻塞。本设计把 processor 的“合适 timeout 内达成可观测状态”固化下来，并把 async_traffic 模块的产品语义、配置常量、边界规则也一并写清楚，让后续新增流量写入行为不再靠固定 sleep 假设。

## 用户目标验证清单

### 必须实现

- `AsyncTrafficWriter` 提供 `record(TrafficRecord)` 与 `update(id, updater)` 两个非阻塞入口；channel 满时最多阻塞一次，绝不悄悄丢记录。
- `start_async_traffic_processor` 在后台 tokio task 中运行，按批消费 record 与 update，批大小上限为 `RECORD_BATCH_SIZE = 512` / `UPDATE_BATCH_SIZE = 256`。
- update 到达时若 record 尚未落库，进入 `pending_updates` 缓存；record 落库时 processor 会 drain 该 record id 上所有 pending updater 并合并写入。
- `pending_updates` 用 `MAX_PENDING_UPDATES = 256` 做上限，超限时按 100 轮循环周期批量清理最老一半 key，避免因“update 永远等不到 record”造成无界内存增长。
- processor 检测到 `traffic_db_store.has_traffic_event_subscribers() == false` 时，把同一 record id 的多次 update 合并成一次 `update_by_id` 写入，减少 SQLite/事件广播压力。
- 单元测试统一用 `wait_until(timeout, condition)` 轮询等待可观测状态（记录数、字段值、更新完成），不再依赖固定 sleep。
- channel 断开或 `rx.recv() -> None` 时 processor 优雅退出，并把最后一批未落库的记录处理完。

### 必须不破坏

- 主端口和临时端口的请求处理路径不能被 async writer 反压阻塞：普通场景是 `try_send`，退化才落到 `send.await`。
- `TrafficDbStore` 原有的 `record_batch`、`update_by_id`、`has_traffic_event_subscribers` 接口不改。
- WebUI / CLI `traffic list|get|search` 语义保持一致：读到的记录仍然是 processor 已落库、事件也已经广播的状态。
- Devtools / Replay / Breakpoint 依赖 `TrafficRecord` 字段的行为不受影响。
- 现有 `TrafficUpdater = Arc<dyn Fn(&mut TrafficRecord) + Send + Sync>` 类型保持不变，避免打断上层调用点。

### 必须真实验证

- 单元测试在慢机 / 高并发 `cargo test --workspace --all-features` 下稳定通过，不再出现 flaky。
- E2E 覆盖：真实代理一次请求，`bifrost traffic get <id>` 能读到已落库、且 update 已合并的 status/duration。
- human_tests 使用真实 CLI 命令记录并复核异步写入路径。

## 产品语义

### async writer 是主端口与临时端口共享的单一写入通道

`AsyncTrafficWriter` 是 `AdminState` 上的共享单例（`SharedAsyncTrafficWriter = Arc<AsyncTrafficWriter>`）。主端口、每个临时端口、devtools capture、replay 结果注入都通过这一个 writer 投递。processor 内部按 batch 消费，跨端口共用同一 SQLite writer 事务，避免每个端口自建一个 writer 造成写放大。

### record 与 update 的顺序不能倒置

一条流量的生命周期是：请求头就绪 → `record()` 投递 record → 响应完成 → `update()` 投递若干 updater（写入 status/duration/response body/error）。processor 必须允许 update 在 record 之前到达（尤其在极短请求 + 高并发下），并通过 `pending_updates` 缓存等待 record 落库后一次性合并。这是必须实现，不是可选优化。

### 无订阅者时合并 update

`traffic_db_store.has_traffic_event_subscribers()` 为 `false` 时，说明没有 WebUI / CLI stream 在订阅事件。同一 record id 上的多次 update 会先按 id 分组，再对每个 id 只发一次 `update_by_id`，updater 顺序保持 FIFO。这样能显著减少 SQLite 写入次数与广播开销，同时保持业务字段最终一致。

### 有订阅者时逐次 update

有事件订阅者时，每个 update 都要 `update_by_id` 一次，让订阅者能观察到中间状态（例如 status = 100 → 200 → 200+body）。这是 DevTools / CDP mirror 的语义要求。

### pending_updates 是有界内存

`pending_updates` 上限 `MAX_PENDING_UPDATES = 256`；processor 每 100 轮循环检查一次，超过则丢弃最老一半 key。这样即使某些 update 的 record 永远丢失（例如 record 被上游 dedupe 掉），也不会让 processor 内存无界。丢弃时 `warn!("Purged stale pending_updates to prevent memory leak")`，便于运维。

## 技术细节

### 数据结构

```rust
pub type TrafficUpdater = Arc<dyn Fn(&mut TrafficRecord) + Send + Sync>;

pub enum TrafficCommand {
    Record(Box<TrafficRecord>),
    Update { id: String, updater: TrafficUpdater },
}

pub struct AsyncTrafficWriter {
    tx: mpsc::Sender<TrafficCommand>,
}

pub type SharedAsyncTrafficWriter = Arc<AsyncTrafficWriter>;
```

`TrafficRecord` 用 `Box` 装箱进入 channel，避免 `TrafficCommand` 枚举变体尺寸失衡导致内存浪费。

### 关键常量

```rust
const RECORD_BATCH_SIZE: usize = 512;
const UPDATE_BATCH_SIZE: usize = 256;
const MAX_PENDING_UPDATES: usize = 256;
```

Buffer size 由 `AsyncTrafficWriter::new(buffer_size)` 传入，`AdminState` 默认给 4096。

### processor 主循环

```text
loop {
    batch.clear(); updates.clear();
    cycle += 1;
    if cycle % 100 == 0 && pending_updates.len() > MAX_PENDING_UPDATES {
        drop_oldest_half(&mut pending_updates);
        warn!("Purged stale pending_updates");
    }
    match rx.recv().await {
        Some(cmd) => {
            push_to_batch_or_updates(cmd);
            while batch.len() < RECORD_BATCH_SIZE && updates.len() < UPDATE_BATCH_SIZE {
                match rx.try_recv() {
                    Ok(cmd) => push_to_batch_or_updates(cmd),
                    Err(Empty) => break,
                    Err(Disconnected) => { info!("channel disconnected"); break; }
                }
            }
            flush_records_with_pending_merge(&mut batch, &mut pending_updates);
            flush_updates_with_grouping(&mut updates, &mut pending_updates);
        }
        None => { info!("channel closed"); break; }
    }
}
```

### record flush 细节

1. `batch.drain(..)` 拿到 `Vec<TrafficRecord>`。
2. 对每条 record，若 `pending_updates.remove(&record.id)` 存在，逐个 updater 应用到 record，再一次性 batch 写入 SQLite。
3. 走 `tokio::task::spawn_blocking` 交给 blocking 池写入，避免占用 async runtime worker。

### update flush 细节

- 有订阅者 → 遍历 `drained`，逐条 `update_by_id`；miss 的 update 回到 `pending_updates`。
- 无订阅者 → 按 id 分组 + 保序（`order: Vec<String>`），每个 id 一次 `update_by_id`，updater 按 FIFO 顺序累加；miss 的整组 update 回到 `pending_updates`。

### wait_until 测试等待器

```rust
async fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut cond: F) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() { return true; }
        if Instant::now() >= deadline { return false; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
```

`timeout` 默认取 `Duration::from_secs(2)`，慢机场景取 `5s`（例如 `test_full_channel_defers_without_dropping_records_or_updates`）。轮询周期 `10ms`，确保 processor 在两个 batch 内一定可观测。

## CLI 与 Admin API 接触面

- CLI：`bifrost traffic list|get|search|export|auth-status|replay` 全部读取 `TrafficDbStore`。async writer 只影响写入路径，读接口无差异。
- Admin API：`GET /_bifrost/api/traffic`、`GET /_bifrost/api/traffic/{id}`、事件 stream `GET /_bifrost/api/traffic/events` 都基于 `TrafficDbStore`。
- 调试：可通过 `RUST_LOG=bifrost_admin::async_traffic=debug` 观察 `Processed N traffic records` / `Processed N traffic updates` 日志，`warn!` 层面观察 pending_updates 清理。

不新增用户可见 CLI 或 API：async_traffic 是内部模块，用户目标是稳定性。

## Sync 边界

async_traffic 不参与 Sync：

- record 与 update 都是本机流量，不会推送到 relay。
- pending_updates 是本机运行时缓存，重启即丢弃，属于预期行为；重启后未落库的 record 认为已丢失，不做磁盘 WAL。
- 若后续要做 traffic 跨设备镜像，需要在 processor 之外新增 pipeline，不得侵入 async_traffic 内部合并逻辑。

## 实现切分

### Phase 1：async writer 与 processor 底座

- `AsyncTrafficWriter::new(buffer_size)` 与 `record/update` 入口。
- `start_async_traffic_processor` 主循环 + record flush + update flush。
- `TrafficCommand::Record(Box<TrafficRecord>)` 装箱化避免枚举尺寸失衡。
- `AdminState` 挂上 `SharedAsyncTrafficWriter`。

### Phase 2：pending updates 与合并优化

- `pending_updates: HashMap<String, Vec<TrafficUpdater>>` 保存孤儿 update。
- `record_batch` 前从 `pending_updates` remove 匹配 id 并合并。
- `has_traffic_event_subscribers()` 分支控制“逐次 update”还是“按 id 合并”。
- 100 轮清理最老一半 pending，`warn!` 埋点。

### Phase 3：测试稳定化

- 引入 `wait_until` 测试等待器。
- 覆盖 `test_async_traffic_writer` / `_update` / `_update_before_record_is_applied` / `_batch_processing` / `_full_channel_defers_without_dropping_records_or_updates`。
- 移除所有固定 `sleep_ms` 断言。

### Phase 4：文档与运维

- 更新本设计文档。
- `human_tests/async-traffic.md` 增补真实 CLI 验证步骤。
- 在 `AGENTS.md` / `project.md` 中若涉及流量记录写入路径，指向 `async_traffic.rs`。

## 测试方案

### 单元测试

位于 `crates/bifrost-admin/src/async_traffic.rs::tests`：

- `test_async_traffic_writer`：投递单条 record 后 `wait_until(2s, || db_store.get_by_id(id).is_some())`。
- `test_async_traffic_update`：先 record，`wait_until` 落库；再投递 update，`wait_until` 观察 status/duration 更新可见。
- `test_async_traffic_update_before_record_is_applied`：先 update 后 record，`wait_until` 观察 record 落库时字段已合并。
- `test_batch_processing`：投递 100 条 record，`wait_until(2s, || db_store.count() == 100)` 覆盖跨批消费。
- `test_full_channel_defers_without_dropping_records_or_updates`：buffer_size 极小，投递 64 条 record + update，`wait_until(5s, ...)` 断言全部落库且 update 生效，证明满 channel 只是 defer 不丢数据。

### E2E 测试

- 优先复用 `e2e-tests/tests/test_bifrost_file_syntax_admin_api.sh` 已有的 admin 起停脚手架，新增 `test_async_traffic_writer_e2e.sh`：
  - 启动临时 `BIFROST_DATA_DIR` 的真实 Bifrost。
  - 通过主端口发 5 次请求。
  - `bifrost traffic list --format json` 断言 5 条记录都到位、status/duration 已填充。
  - 断言事件订阅路径（`curl` streaming `/api/traffic/events`）能看到 5 条 finalized 事件。

- 全部服务启动使用临时目录、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 真实场景测试 human_tests

`human_tests/async-traffic.md` 保留并补齐以下用例：

- TC-ASYNC-01：新数据目录冷启动 + `curl -x http://127.0.0.1:<port> https://httpbin.org/get`；`bifrost traffic list` 立即可见记录，status/duration 有值。
- TC-ASYNC-02：连续 200 次请求；`bifrost traffic list --limit 200` 验证条数为 200，不丢数据。
- TC-ASYNC-03：`bifrost traffic get <id>` 与 `bifrost traffic auth-status <id>` 组合读取，验证 update 已合并（response body、headers、status）。
- TC-ASYNC-04：故意让某个 update 的 id 早于 record 到达（可以借助 replay 注入路径）；验证最终 record 字段包含 update 结果，日志无 `Purged stale pending_updates`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin async_traffic::tests -- --nocapture`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`（本地）
- `rust-project-validate`

本机当前有 no-local-coverage 约定时不运行 `make coverage`，交付说明 coverage 本地豁免、依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：非阻塞入口、批处理、pending update 合并、有界内存、稳定测试。
- 复核 diff：`async_traffic.rs`、`AdminState`、handlers 是否只在写入路径使用 writer，读路径仍是 `TrafficDbStore` 直查。
- 重点 review：`try_send` 满时是否走 `send.await` 阻塞而非丢弃；`pending_updates` 上限与清理策略是否符合 200+ 并发流量的实际压力。
- 复测：跑 `cargo test -p bifrost-admin async_traffic::tests --release` 至少 20 次验证稳定，跑 workspace test。

### 第 2 轮

- 复核第 1 轮问题修复。
- 复检 `git status --short`、`git diff`，确认没有把 async writer 的 batch flush 逻辑绕出模块之外。
- 重点 review：无订阅者合并分支的 updater 顺序、`warn!` 是否只有 pending 溢出时才输出、事件订阅者切换时是否会丢中间态。
- 复测：human_tests TC-ASYNC-01..04 全部真实跑通；`RUST_LOG=bifrost_admin::async_traffic=debug` 观察 `Processed N traffic records/updates` 稳定输出。

## 风险与决策

- `RECORD_BATCH_SIZE = 512` / `UPDATE_BATCH_SIZE = 256` 是当前流量峰值下的经验值；若未来支持 10k+ QPS 代理，需要基于真实压测调整，并把常量抽成 `AdminConfig` 字段。
- `MAX_PENDING_UPDATES = 256` 在极端场景（大量 update 先于 record 到达）可能导致部分 update 被丢；决定接受，因为这些场景是上游 record 丢失或 dedupe 已知问题。
- 事件订阅者切换（0 → 1、1 → 0）不会 replay 中间状态；订阅者只能看到订阅后的 update。这是产品可接受行为。
- 不实现磁盘 WAL：重启前未落库的 record 视为丢失。若未来需要“审计级不丢失”，需要单独 pipeline，不在 async_traffic 语义内。
- `spawn_blocking` 已把 SQLite 写入放到 blocking 池，若 blocking 池饱和，processor 会背压 channel 但不会崩溃；这是正确行为。
