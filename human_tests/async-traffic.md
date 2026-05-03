# Async Traffic Writer 真实场景测试

## 功能模块说明

Async Traffic Writer 将流量记录和后续更新异步写入 traffic 数据库。测试目标是确认后台 processor 在真实本地 CLI 环境中可以稳定完成记录写入、更新合并和跨批次处理，不依赖固定 sleep 的偶然时序。

## 前置条件

- 在仓库根目录 `/Users/eden/work/github/bifrost` 执行。
- 已安装 Rust toolchain。
- 本用例不启动 Bifrost 代理，不占用正式 `9900` 端口，不修改系统代理。

## 测试用例列表

### TC-AT-01：异步流量写入、更新和批处理单测稳定性

**操作步骤**：
1. 执行 async traffic 单测：
   ```bash
   cargo test -p bifrost-admin async_traffic::tests -- --nocapture
   ```
2. 观察 `test_async_traffic_writer`、`test_async_traffic_update`、`test_batch_processing` 的结果。

**预期结果**：
- 三个 async traffic 单测全部通过。
- `test_async_traffic_update` 能观察到 status=200、duration_ms=100。
- `test_batch_processing` 能观察到 100 条记录全部落库，而不是停留在第一批 64 条。

### TC-AT-02：workspace 聚合测试不再触发 async traffic 时序误判

**操作步骤**：
1. 执行 workspace 全 feature 测试：
   ```bash
   cargo test --workspace --all-features
   ```
2. 观察 `bifrost-admin` 测试阶段中 async traffic 相关用例结果。

**预期结果**：
- `async_traffic::tests::test_async_traffic_update` 通过。
- `async_traffic::tests::test_batch_processing` 通过。
- 若 workspace 中其他无关用例失败，应单独归因，不应再出现 async traffic 固定等待导致的 `left: 0/right: 200` 或 `left: 64/right: 100`。

## 清理步骤

1. async traffic 单测只创建系统临时目录，测试进程结束后无需手动关闭服务。
2. 如需清理历史临时目录，可执行：
   ```bash
   rm -rf "${TMPDIR:-/tmp}"/bifrost-async-traffic-*
   ```

## 执行记录

- 2026-05-04：通过。补充并执行 TC-AT-01、TC-AT-02，用于验证 async traffic 单测从固定 sleep 改为 timeout 轮询后，在本地 CLI 和 workspace 聚合测试中均稳定通过。
  - TC-AT-01：执行 `cargo test -p bifrost-admin async_traffic::tests -- --nocapture`，3 个 async traffic 单测全部通过，更新合并和 100 条批处理写入均达到预期。
  - TC-AT-02：执行 `cargo test --workspace --all-features`，workspace 聚合测试全部通过，未再出现 async traffic 固定等待导致的 `left: 0/right: 200` 或 `left: 64/right: 100`。
