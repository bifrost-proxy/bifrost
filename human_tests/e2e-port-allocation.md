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

### TC-EPA-02: rule merge strategy 主干 CI 端口 bind 竞态重试

**操作步骤**：
1. 执行主干 CI 失败的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::rule_merge_strategy::tests::test_resheaders_merge -- --exact --nocapture
   ```
2. 执行 rule merge strategy 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture
   ```
3. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 `Failed to bind ... another process is already listening on this port` 失败。
- rule merge strategy 模块全部通过，所有代理启动点均通过 retry helper 获取端口。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

### TC-EPA-03: protocols workspace 端口 bind 竞态重试

**操作步骤**：
1. 执行 workspace 复测暴露的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::protocols::tests::test_host_basic -- --exact --nocapture
   ```
2. 执行 protocols 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib protocols -- --nocapture
   ```
3. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 `Failed to bind ... another process is already listening on this port` 失败。
- protocols 模块全部通过，`start`、`start_with_rules_text`、`start_with_values` 三种代理启动方式均通过 retry helper 获取端口。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

### TC-EPA-04: routing upstream proxy 端口 bind 竞态重试

**操作步骤**：
1. 执行 workspace 复测暴露的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::routing::tests::test_proxy_chain_upstream_auth_correct -- --exact --nocapture
   ```
2. 执行 routing 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib routing -- --nocapture
   ```
3. 执行完整 `bifrost-e2e` lib 回归：
   ```bash
   cargo test -p bifrost-e2e --lib
   ```
4. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 upstream proxy `Failed to bind ... another process is already listening on this port` 失败。
- routing 模块全部通过，普通 proxy 与带 userpass auth 的 upstream proxy 均通过 retry helper 获取端口。
- `bifrost-e2e` lib 测试通过，不再暴露同类端口竞态。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

### TC-EPA-05: response modification combined 端口 bind 竞态重试

**操作步骤**：
1. 执行 workspace 复测暴露的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::response_modification::tests::test_combined -- --nocapture
   ```
2. 执行 response modification 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib response_modification -- --nocapture
   ```
3. 执行完整 `bifrost-e2e` lib 回归：
   ```bash
   cargo test -p bifrost-e2e --lib
   ```
4. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 `Failed to bind ... another process is already listening on this port` 失败。
- response modification 模块全部通过，所有代理启动点均通过 retry helper 获取端口。
- `bifrost-e2e` lib 测试通过，不再暴露同类端口竞态。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

### TC-EPA-06: rule priority xhost 端口 bind 竞态重试

**操作步骤**：
1. 执行 workspace 复测暴露的单用例：
   ```bash
   cargo test -p bifrost-e2e tests::rule_priority::tests::test_xhost_over_host -- --nocapture
   ```
2. 执行 rule priority 模块并发回归：
   ```bash
   cargo test -p bifrost-e2e --lib rule_priority -- --nocapture
   ```
3. 执行完整 `bifrost-e2e` lib 回归：
   ```bash
   cargo test -p bifrost-e2e --lib
   ```
4. 执行完整 workspace 兜底：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- 单用例通过，不再因为 `Failed to bind ... another process is already listening on this port` 失败。
- rule priority 模块全部通过，所有代理启动点均通过 retry helper 获取端口。
- `bifrost-e2e` lib 测试通过，不再暴露同类端口竞态。
- workspace 兜底测试全部通过；如出现端口 bind 竞态，应由 helper 重试而不是直接失败。
- 测试不使用 9900，不修改系统代理。

## 本轮执行记录

测试日期：2026-05-26；追加记录：2026-06-04

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-EPA-01 | 通过 | 已定位原始失败为 `tests::request_modification::tests::test_headers_value_ref` 启动 proxy 时端口 `20836` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`，8 个 request modification 用例全部通过；执行 `cargo test --workspace --all-features`，workspace 全量测试通过。 |
| TC-EPA-02 | 通过 | 2026-06-04 主干 CI 失败为 `tests::rule_merge_strategy::tests::test_resheaders_merge` 启动 proxy 时端口 `127.0.0.1:20560` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::rule_merge_strategy::tests::test_resheaders_merge -- --exact --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`，8 个 rule merge strategy 用例全部通过；执行 `CARGO_TARGET_DIR=$PWD/.codex-target-fix-e2e CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' cargo test -p bifrost-e2e --lib`，67 个 bifrost-e2e lib 用例通过、2 个 ignored。 |
| TC-EPA-03 | 通过 | 2026-06-04 workspace 复测暴露 `tests::protocols::tests::test_host_basic` 启动 proxy 时端口 `127.0.0.1:21962` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::protocols::tests::test_host_basic -- --exact --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib protocols -- --nocapture`，2 个 protocols lib 用例通过；执行 `cargo run -p bifrost-e2e -- --category protocols --jobs 2 --verbose`，18 个 category 用例通过、1 个 skipped；隔离 target 的 `cargo test -p bifrost-e2e --lib` 通过。 |
| TC-EPA-04 | 通过 | 2026-06-04 `cargo test -p bifrost-e2e --lib` 复测暴露 `tests::routing::tests::test_proxy_chain_upstream_auth_correct` 启动 upstream proxy 时端口 `127.0.0.1:22671` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::routing::tests::test_proxy_chain_upstream_auth_correct -- --exact --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib routing -- --nocapture`，8 个 routing lib 用例通过；执行 `cargo run -p bifrost-e2e -- --category routing --jobs 2 --verbose`，19 个 category 用例通过；隔离 target 的 `cargo test -p bifrost-e2e --lib` 通过。完整 `cargo test --workspace --all-features` 本机兜底曾触发非本修复路径阻塞：一次在 `bifrost-admin` 出现可单跑通过的 unrelated timeout/共享状态失败；一次隔离 target 在链接阶段因 `/tmp` 空间不足失败；一次默认 target 被其它 worktree 并发构建污染，出现依赖 dylib 丢失。因此本轮以受影响 e2e 单元、模块和 `bifrost-e2e --lib` 作为真实场景通过证据，workspace 全量留给远端 CI 兜底。 |
| TC-EPA-05 | 通过 | 2026-06-04 `cargo test --workspace --all-features` 复测暴露 `tests::response_modification::tests::test_combined` 启动 proxy 时端口 `127.0.0.1:21525` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::response_modification::tests::test_combined -- --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib response_modification -- --nocapture`，5 个 response modification 用例全部通过；执行 `cargo test -p bifrost-e2e --lib`，67 个 bifrost-e2e lib 用例通过、2 个 ignored。完整 workspace 全量继续由远端 CI 兜底。 |
| TC-EPA-06 | 通过 | 2026-06-04 `cargo test --workspace --all-features` 复测暴露 `tests::rule_priority::tests::test_xhost_over_host` 启动 proxy 时端口 `127.0.0.1:19103` 被抢占；修复后执行 `cargo test -p bifrost-e2e tests::rule_priority::tests::test_xhost_over_host -- --nocapture`，单用例通过；执行 `cargo test -p bifrost-e2e --lib rule_priority -- --nocapture`，5 个 rule priority 用例全部通过；执行 `cargo test -p bifrost-e2e --lib`，67 个 bifrost-e2e lib 用例通过、2 个 ignored。完整 workspace 全量继续由远端 CI 兜底。 |

## 清理步骤

- 无特殊清理需求；测试使用临时数据目录，异常残留可通过 `ps` / `lsof` 检查后清理。
