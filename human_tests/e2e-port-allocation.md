# E2E Port Allocation

## 功能模块说明

验证 Rust E2E 测试在并发执行时不会因为 `portpicker::pick_unused_port()` 到 `ProxyInstance` 实际 bind 之间的端口竞态导致偶发失败。

## 前置条件

- 工作目录：项目根目录
- 已安装 Rust toolchain
- 不需要启动本机 Bifrost 服务；测试会使用临时端口和临时数据目录

## 测试用例

### TC-EPA-01: request modification 用例端口 bind 竞态重试

**操作步骤**：
1. 执行本轮 CI 本地失败的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture
   ```
2. 执行 request modification 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib request_modification -- --nocapture
   ```
3. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 `Failed to bind ... another process is already listening on this port` 失败。
- request modification 模块全部通过。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

## 本轮执行记录

测试日期：2026-05-26

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-EPA-01 | 通过 | 已定位原始失败为 `tests::request_modification::tests::test_headers_value_ref` 启动 proxy 时端口 `20836` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`，8 个 request modification 用例全部通过；执行 `cargo test --workspace --all-features`，workspace 全量测试通过。 |

## 清理步骤

- 无特殊清理需求；测试使用临时数据目录，异常残留可通过 `ps` / `lsof` 检查后清理。
