# Async Traffic Writer

## 功能模块说明

`crates/bifrost-admin/src/async_traffic.rs` 负责把流量记录写入和后续更新从请求处理路径异步投递到后台 processor。processor 会批量写入 `TrafficDbStore`，并在没有订阅者时按记录 ID 合并 update，减少数据库写入压力。

## 2026-05-04：异步处理单测稳定性

### 背景

`cargo test --workspace --all-features` 中 `async_traffic::tests::test_async_traffic_update` 和 `async_traffic::tests::test_batch_processing` 偶发失败：

- update 用例在固定 `50ms` sleep 后读取数据库，processor 可能还没消费 update。
- batch 用例一次发送 100 条记录，但 processor 每轮最多批量处理 64 条；固定 `100ms` sleep 在慢机器或并发测试环境下可能只观察到第一批。

这类失败属于测试等待策略不完整，不是生产逻辑需要扩大同步阻塞。正确行为是等待异步 processor 在合理 timeout 内达成可观测状态。

### 实现逻辑

- 测试模块新增 `wait_until(timeout, condition)`，每 10ms 轮询一次条件直到达成或超时。
- `test_async_traffic_writer` 等待记录真实可查。
- `test_async_traffic_update` 先等待记录落库，再投递 update，并等待 status/duration 更新可见。
- `test_batch_processing` 等待数据库记录数达到 100，覆盖跨两个 processor batch 的完整消费。

### 测试方案

- 单元测试：执行 `cargo test -p bifrost-admin async_traffic::tests -- --nocapture`，验证异步写入、更新和批处理稳定通过。
- 聚合测试：执行 `cargo test --workspace --all-features`，确认 workspace 并发测试环境下不再因固定 sleep 误判。
- 真实场景测试：创建并执行 `human_tests/async-traffic.md`，用 CLI 命令验证异步流量写入测试在真实本地环境中通过。

### 校验要求

- rules E2E 与 human_tests 先执行，再进入 rust-project-validate。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`bash scripts/ci/local-ci.sh --skip-e2e`。
