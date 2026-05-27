# E2E Port Allocation

## 功能模块说明

Rust E2E 单元测试会在同一进程内并发启动多个 `ProxyInstance`。测试不能只用 `portpicker::pick_unused_port()` 作为最终保证，因为端口从探测到实际 bind 之间存在竞态窗口。

## 实现逻辑

- `crates/bifrost-e2e/src/tests/request_modification.rs` 的 request modification 用例通过 helper 启动 proxy。
- helper 每次先选择空闲端口，再实际启动 `ProxyInstance`。
- 如果启动失败原因是端口 bind 竞态，例如 `Failed to bind` 或 `already listening on this port`，helper 会重新选择端口并重试。
- 非端口竞态错误不吞掉，直接返回原始失败原因，避免隐藏真实业务断言失败。

## 依赖项

- `crates/bifrost-e2e/src/tests/request_modification.rs`
- `crates/bifrost-e2e/src/proxy.rs`

## 测试方案

### 单元测试

- 运行 `cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture`，验证本轮失败用例可通过。
- 运行 `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`，验证 request modification 模块并发用例可通过。

### E2E 测试

- 运行最小 shell E2E shard，验证 CI shell 调度变更仍可通过，不受 Rust E2E helper 影响。

### 真实场景测试

- 新增 `human_tests/e2e-port-allocation.md`，记录端口 bind 竞态回归与真实执行结果。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/e2e-port-allocation.md`
- 更新 `human_tests/readme.md`
