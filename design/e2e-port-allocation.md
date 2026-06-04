# E2E Port Allocation

## 功能模块说明

Rust E2E 单元测试会在同一进程内并发启动多个 `ProxyInstance`。测试不能只用 `portpicker::pick_unused_port()` 作为最终保证，因为端口从探测到实际 bind 之间存在竞态窗口。

## 实现逻辑

- `crates/bifrost-e2e/src/tests/request_modification.rs` 的 request modification 用例通过 helper 启动 proxy。
- `crates/bifrost-e2e/src/tests/protocols.rs` 的 protocol 用例通过 helper 覆盖 `start`、`start_with_rules_text` 与 `start_with_values` 三种 proxy 启动方式。
- `crates/bifrost-e2e/src/tests/routing.rs` 的 routing 用例通过 helper 覆盖普通 proxy 与带 userpass auth 的 upstream proxy 启动方式。
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs` 的 rule merge strategy 用例也必须通过同类 helper 启动 proxy，避免 `cargo test --workspace --all-features` 在主干 CI 中偶发失败。
- `crates/bifrost-e2e/src/tests/response_modification.rs` 的 response modification 用例通过 helper 启动 proxy，覆盖 workspace 复测暴露的 `test_combined` 端口抢占。
- helper 每次先选择空闲端口，再实际启动 `ProxyInstance`。
- 如果启动失败原因是端口 bind 竞态，例如 `Failed to bind` 或 `already listening on this port`，helper 会重新选择端口并重试。
- 非端口竞态错误不吞掉，直接返回原始失败原因，避免隐藏真实业务断言失败。

## 依赖项

- `crates/bifrost-e2e/src/tests/request_modification.rs`
- `crates/bifrost-e2e/src/tests/protocols.rs`
- `crates/bifrost-e2e/src/tests/routing.rs`
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs`
- `crates/bifrost-e2e/src/tests/response_modification.rs`
- `crates/bifrost-e2e/src/proxy.rs`

## 测试方案

### 单元测试

- 运行 `cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture`，验证本轮失败用例可通过。
- 运行 `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`，验证 request modification 模块并发用例可通过。
- 运行 `cargo test -p bifrost-e2e tests::rule_merge_strategy::tests::test_resheaders_merge -- --exact --nocapture`，验证主干 CI 失败用例在真实代理启动路径下可通过。
- 运行 `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`，验证 rule merge strategy 模块内所有代理启动点均通过 retry helper。
- 运行 `cargo test -p bifrost-e2e tests::protocols::tests::test_host_basic -- --exact --nocapture`，验证 workspace 复测暴露的 protocol 基础用例在真实代理启动路径下可通过。
- 运行 `cargo test -p bifrost-e2e --lib protocols -- --nocapture`，验证 protocols 模块内所有代理启动方式均通过 retry helper。
- 运行 `cargo test -p bifrost-e2e tests::routing::tests::test_proxy_chain_upstream_auth_correct -- --exact --nocapture`，验证 workspace 复测暴露的 upstream auth proxy chain 用例可通过。
- 运行 `cargo test -p bifrost-e2e --lib routing -- --nocapture`，验证 routing 模块内普通 proxy 和 upstream proxy 启动均通过 retry helper。
- 运行 `cargo test -p bifrost-e2e tests::response_modification::tests::test_combined -- --nocapture`，验证 workspace 复测暴露的 response modification combined 用例可通过。
- 运行 `cargo test -p bifrost-e2e --lib response_modification -- --nocapture`，验证 response modification 模块内所有代理启动点均通过 retry helper。

### E2E 测试

- 运行 `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`，验证 Rust E2E runner 中 rule merge strategy 的真实代理链路可通过。

### 真实场景测试

- 更新 `human_tests/e2e-port-allocation.md`，记录 request modification、rule merge strategy、protocols、routing 与 response modification 端口 bind 竞态回归与真实执行结果。

## Review/Fix/Test 闭环方案

- 第 1 轮复核 `rule_merge_strategy.rs` 是否还存在裸 `portpicker::pick_unused_port()` + `ProxyInstance::start()` 启动点，复跑主干 CI 失败的 `test_resheaders_merge`。
- 追加轮复核 workspace 复测暴露的 `protocols.rs` 同类 bind race，覆盖 `start_with_rules_text` 与 `start_with_values`。
- 再追加轮复核 workspace 复测暴露的 `routing.rs` upstream proxy bind race，覆盖带认证的 upstream proxy chain。
- 追加轮复核 workspace 复测暴露的 `response_modification.rs` combined response modification bind race，覆盖本模块所有裸 `ProxyInstance::start`。
- 第 2 轮复核 `design/`、`human_tests/` 与 `human_tests/readme.md` 是否同步，复跑整个 `rule_merge_strategy` lib test 模块。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`
- `cargo test -p bifrost-e2e --lib protocols -- --nocapture`
- `cargo test -p bifrost-e2e --lib routing -- --nocapture`
- `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`
- `cargo test -p bifrost-e2e --lib response_modification -- --nocapture`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/e2e-port-allocation.md`
- 更新 `human_tests/readme.md`
