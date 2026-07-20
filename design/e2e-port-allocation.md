# E2E Port Allocation 设计方案

## 背景

Rust E2E 测试（`crates/bifrost-e2e`）在同一进程内并发启动多个 `ProxyInstance`。测试端口通常使用 `portpicker::pick_unused_port()` 探测空闲端口，但探测到实际 `bind()` 之间存在竞态窗口：并发多个测试同时探测到同一个端口后，只有一个能真正 bind，其它失败并报 `Failed to bind ... another process is already listening on this port`。

主干 CI 与本地 `cargo test --workspace --all-features` 曾多次遇到这类偶发失败，且不同测试模块（request modification、rule merge strategy、protocols、routing、response modification、rule priority）都独立踩过。本方案在每个受影响模块内内联一致的 helper：先 `pick_unused_port`，启动 `ProxyInstance`，若失败原因判定为 bind race 则重新探测并重试，重试上限统一控制。

## 用户目标验证清单

### 必须实现

- `crates/bifrost-e2e/src/tests/request_modification.rs` 通过 helper 启动 proxy。
- `crates/bifrost-e2e/src/tests/protocols.rs` 通过 helper 覆盖 `start` / `start_with_rules_text` / `start_with_values` 三种启动方式。
- `crates/bifrost-e2e/src/tests/routing.rs` 通过 helper 覆盖普通 proxy 与带 userpass auth 的 upstream proxy。
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs` 通过 helper 启动 proxy。
- `crates/bifrost-e2e/src/tests/response_modification.rs` 通过 helper 启动 proxy，覆盖 `test_combined` 端口抢占。
- `crates/bifrost-e2e/src/tests/rule_priority.rs` 通过 helper 启动 proxy，覆盖 `test_xhost_over_host` 端口抢占。
- helper 每次先 `pick_unused_port`，再启动 `ProxyInstance`；bind race 时重新探测并重试。
- 非 bind race 错误不吞掉，直接返回原始失败原因。

### 必须不破坏

- 现有 `ProxyInstance::start / start_with_rules_text / start_with_values / start_with_userpass` 启动入口签名不变。
- 测试断言语义不变；helper 只处理启动失败重试，不改测试逻辑。
- 测试不使用固定端口（禁止 9900），不修改系统代理。
- 测试之间的端口独立性维持不变；helper 每次重试都重新 `pick_unused_port`，不复用旧端口。
- `cargo test --workspace --all-features` 通过；不引入长阻塞或阻塞式 sleep。

### 必须真实验证

- 每个受影响模块的关键单用例真实通过。
- 每个受影响模块的完整 lib test 真实通过。
- workspace 兜底 `cargo test --workspace --all-features` 通过。
- human_tests 记录真实执行结果。

## 产品语义

### helper 内联在每个测试文件

历史上曾考虑把 helper 抽到 `crates/bifrost-e2e/src/proxy.rs`，但为了避免破坏现有 `ProxyInstance` API 与保持每个测试模块独立演进，helper **内联在每个测试文件中**。共享点仅为约定：

- 常量 `START_PROXY_MAX_ATTEMPTS: usize = 10`（`request_modification.rs` 当前仍写死 10 次，属于待统一项，语义等价）。
- 判定函数 `fn is_bind_race(error: &str) -> bool`，匹配 `Failed to bind` / `already listening on this port` 等已知 bind race 关键词。
- helper 名字统一：
  - `async fn start_proxy_with_owned_rules(rules: Vec<String>) -> Result<(u16, ProxyInstance), String>`
  - `async fn start_proxy_with_rules_text(rules_text: &str) -> Result<(u16, ProxyInstance), String>`
  - `async fn start_proxy_with_values(...) -> Result<(u16, ProxyInstance), String>`
  - `async fn start_proxy_with_userpass(...) -> Result<(u16, ProxyInstance), String>`
- 便捷宏 `macro_rules! start_proxy_with_rules!`：把变参 `&str` 转成 `Vec<String>` 再调用 `start_proxy_with_owned_rules`。

### 重试语义

伪代码：

```rust
for attempt in 1..=START_PROXY_MAX_ATTEMPTS {
    let port = portpicker::pick_unused_port().ok_or("no free port")?;
    match ProxyInstance::start(port, rules.clone()).await {
        Ok(proxy) => return Ok((port, proxy)),
        Err(e) if is_bind_race(&e.to_string()) && attempt < START_PROXY_MAX_ATTEMPTS => {
            continue;
        }
        Err(e) => return Err(e.to_string()),
    }
}
```

关键点：

- 只吞 bind race 错误，其它错误（例如 rule 解析错误、内部 IO 错误）立即返回原始错误。
- 每次重试都重新 `pick_unused_port`，避免复用被抢占的端口。
- 达到 `START_PROXY_MAX_ATTEMPTS` 时返回最后一次错误，测试直接失败便于排查。

### 判定关键词

`is_bind_race` 匹配当前已知的 bind 失败错误消息片段：

- `Failed to bind`
- `already listening on this port`
- （未来若出现新变体，各文件同步扩展关键词）

## 技术细节

### 现状代码路径

- `crates/bifrost-e2e/src/tests/request_modification.rs`：`start_proxy_with_rules` / `start_proxy_with_values` / `start_proxy_with_rules_text` 已到位，`START_PROXY_MAX_ATTEMPTS` 硬编码 10。
- `crates/bifrost-e2e/src/tests/protocols.rs`：常量 + 宏 + 三个 helper 到位。
- `crates/bifrost-e2e/src/tests/routing.rs`：常量 + 宏 + `start_proxy_with_owned_rules` + `start_proxy_with_userpass` 到位。
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs`：常量 + 宏 + `start_proxy_with_owned_rules` 到位。
- `crates/bifrost-e2e/src/tests/response_modification.rs`：常量 + 宏 + `start_proxy_with_owned_rules` 到位。
- `crates/bifrost-e2e/src/tests/rule_priority.rs`：常量 + 宏 + `start_proxy_with_owned_rules` 到位。
- `crates/bifrost-e2e/src/proxy.rs`：提供 `ProxyInstance::start` / `start_with_rules_text` / `start_with_values` / `start_with_userpass` 启动入口；端口探测/重试 helper 仍内联在各测试文件，未抽到此模块。

### 后续统一项（不阻塞交付）

- `request_modification.rs` 的重试上限迁移到 `START_PROXY_MAX_ATTEMPTS` 常量。
- `is_bind_race` 关键词若跨模块出现漂移，考虑收敛到 `crates/bifrost-e2e/src/testing.rs` 或类似公共模块；本次保持内联，避免动 API。

## CLI / Admin API

无 CLI、Admin API 变更。

## Sync / 导入导出边界

无变更。仅影响测试代码。

## 实现切分

### Phase 1：request modification & rule merge strategy 试点

- 引入 helper 与 `is_bind_race`。
- 覆盖本轮 CI 失败用例 `test_headers_value_ref` 与 `test_resheaders_merge`。

### Phase 2：protocols & routing

- 引入宏 `start_proxy_with_rules!`。
- 覆盖 `start_with_rules_text`、`start_with_values`、`start_with_userpass` 三种启动路径。
- 覆盖 `test_host_basic`、`test_proxy_chain_upstream_auth_correct` 等偶发失败用例。

### Phase 3：response modification & rule priority

- 覆盖 `test_combined`、`test_xhost_over_host` 等 workspace 复测暴露的用例。
- 所有裸 `ProxyInstance::start` 替换为 helper。

### Phase 4：文档与 human_tests

- 更新 `design/e2e-port-allocation.md`（本文档）。
- 更新 `human_tests/e2e-port-allocation.md` 与 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `cargo test -p bifrost-e2e tests::request_modification::tests::test_headers_value_ref -- --nocapture`
- `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`
- `cargo test -p bifrost-e2e tests::rule_merge_strategy::tests::test_resheaders_merge -- --exact --nocapture`
- `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`
- `cargo test -p bifrost-e2e tests::protocols::tests::test_host_basic -- --exact --nocapture`
- `cargo test -p bifrost-e2e --lib protocols -- --nocapture`
- `cargo test -p bifrost-e2e tests::routing::tests::test_proxy_chain_upstream_auth_correct -- --exact --nocapture`
- `cargo test -p bifrost-e2e --lib routing -- --nocapture`
- `cargo test -p bifrost-e2e tests::response_modification::tests::test_combined -- --nocapture`
- `cargo test -p bifrost-e2e --lib response_modification -- --nocapture`
- `cargo test -p bifrost-e2e tests::rule_priority::tests::test_xhost_over_host -- --nocapture`
- `cargo test -p bifrost-e2e --lib rule_priority -- --nocapture`

### E2E 测试

- `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`：Rust E2E runner 中 rule merge strategy 的真实代理链路。
- `cargo test --workspace --all-features`：workspace 兜底，验证端口 helper 不引入回归。

### 真实场景测试 human_tests

更新 `human_tests/e2e-port-allocation.md`：

- TC-EPA-01：request modification 用例端口 bind 竞态重试。
- TC-EPA-02：rule merge strategy 主干 CI 端口 bind 竞态重试。
- TC-EPA-03：protocols `start_with_rules_text` / `start_with_values` 端口 bind 竞态重试。
- TC-EPA-04：routing 普通 proxy 与 upstream userpass proxy 端口 bind 竞态重试。
- TC-EPA-05：response modification `test_combined` 端口 bind 竞态重试。
- TC-EPA-06：rule priority `test_xhost_over_host` 端口 bind 竞态重试。
- TC-EPA-07：coverage job 中 TLS intercept mode `test_passthrough_rule` 端口 bind 竞态重试，并覆盖该模块三个代理启动点。

每个用例记录真实执行命令、真实执行结果、失败重试次数（若有）。

同步更新 `human_tests/readme.md` 用例总数与说明。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-e2e --lib request_modification -- --nocapture`
- `cargo test -p bifrost-e2e --lib protocols -- --nocapture`
- `cargo test -p bifrost-e2e --lib routing -- --nocapture`
- `cargo test -p bifrost-e2e --lib rule_merge_strategy -- --nocapture`
- `cargo test -p bifrost-e2e --lib response_modification -- --nocapture`
- `cargo test -p bifrost-e2e --lib rule_priority -- --nocapture`
- `cargo test --workspace --all-features`

本机 no-local-coverage 约定下不跑 `make coverage`；在交付备注中说明依赖远端 CI。

## 边界与非目标

- 本方案不覆盖 shell E2E 脚本（`e2e-tests/tests/*.sh`）的端口分配问题；shell 脚本一般使用固定环境变量端口或独立 helper。
- 本方案不覆盖桌面壳 E2E 的端口冲突问题；桌面端有专门的 `port-conflict-restart.md`。
- 本方案不改动 `ProxyInstance` 内部 bind 逻辑；只在测试 helper 层做重试兜底。
- 本方案不引入端口预分配池；若未来 workspace 并行度大幅提高再评估。
- 本方案不解决 `portpicker::pick_unused_port()` 在容器内探测偶发失败的问题；`ok_or("no free port")?` 分支保留原始失败语义。

## 与其它设计文档的关系

- `port-conflict-restart.md`：桌面端主端口冲突后的自愈策略，与本文档解耦，共享“端口探测无法保证 bind 原子性”这一根因认知。
- `e2e-test-fast-build.md`：workspace 并行编译 / 并行测试的加速方案，本文档确保并行度提高后端口 bind race 不成为主要失败模式。
- `e2e-script-startup.md`：shell E2E 启动策略，参考本文档的 `is_bind_race` 关键词判定思路，但独立实现。

## 常见问题排查

- **重试仍然失败**：`START_PROXY_MAX_ATTEMPTS` 达上限说明连续 10 次都遇到 bind race 或系统端口极度紧张；检查 CI runner 是否被其它进程占满端口，或提高 `--test-threads` 后再评估。
- **非 bind race 错误被吞**：不应发生；helper 严格匹配 `is_bind_race`，其它错误直接返回。若发生说明 helper 或错误关键词判定错误。
- **helper 使用固定端口**：禁止；每次重试必须重新 `pick_unused_port`。
- **测试使用 9900 端口**：禁止；9900 是桌面主端口默认值，测试固定使用会与本机 bifrost 冲突。
- **`is_bind_race` 漏匹配新错误变体**：底层错误消息演进时需要同步扩展关键词；建议长期改为结构化错误类型。

## 已知问题与后续演进

- helper 内联导致代码重复，未来若关键词漂移需要多处修改；可考虑抽到 `crates/bifrost-e2e/src/testing.rs`，但需要评估对现有测试的破坏性。
- `request_modification.rs` 的 `START_PROXY_MAX_ATTEMPTS` 仍硬编码 10 次，待统一到常量。
- 若未来 `ProxyInstance::start` 提供结构化错误 `enum ProxyStartError { BindRace, ... }`，helper 可以摆脱字符串匹配。
- 若 workspace 并行度提升（例如 `--test-threads=32`），可能需要提高 `START_PROXY_MAX_ATTEMPTS` 或引入端口预分配池；先观察真实失败率再决策。
- 若容器化 CI 中 `portpicker::pick_unused_port()` 偶发返回 None，helper 会返回 `no free port` 错误，需要单独兜底而不是无限循环。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `rule_merge_strategy.rs` 是否还存在裸 `portpicker::pick_unused_port()` + `ProxyInstance::start()` 启动点，复跑主干 CI 失败的 `test_resheaders_merge`。
- 追加复核 workspace 复测暴露的 `protocols.rs` 同类 bind race，覆盖 `start_with_rules_text` 与 `start_with_values`。
- 追加复核 workspace 复测暴露的 `routing.rs` upstream proxy bind race，覆盖带认证的 upstream proxy chain。
- 追加复核 workspace 复测暴露的 `response_modification.rs` combined 用例 bind race，覆盖本模块所有裸 `ProxyInstance::start`。
- 追加复核 workspace 复测暴露的 `rule_priority.rs` xhost priority bind race，覆盖本模块所有裸 `ProxyInstance::start`。

### 第 2 轮

- 复核 `design/`、`human_tests/`、`human_tests/readme.md` 是否同步。
- 复跑整个 `rule_merge_strategy` lib test 模块与 `cargo test --workspace --all-features`。
- 重点 review：`is_bind_race` 关键词是否漏匹配新变体；`START_PROXY_MAX_ATTEMPTS` 是否被误改成 1；非 bind race 错误是否被吞。

## 依赖项

- `crates/bifrost-e2e/src/tests/request_modification.rs`
- `crates/bifrost-e2e/src/tests/protocols.rs`
- `crates/bifrost-e2e/src/tests/routing.rs`
- `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs`
- `crates/bifrost-e2e/src/tests/response_modification.rs`
- `crates/bifrost-e2e/src/tests/rule_priority.rs`
- `crates/bifrost-e2e/src/tests/tls_intercept_mode.rs`
- `crates/bifrost-e2e/src/proxy.rs`（提供 `ProxyInstance::start` / `start_with_rules_text` / `start_with_values` / `start_with_userpass` 启动入口；端口探测/重试 helper 仍内联在各测试文件，未抽到此模块）
- `portpicker` crate（探测空闲端口）

## 文档更新要求

- 更新 `human_tests/e2e-port-allocation.md`
- 更新 `human_tests/readme.md`
- 本设计文档

## 风险与决策点

- helper 内联 vs. 抽取公共模块：本次保持内联，避免动 `ProxyInstance` API 与其它测试；缺点是重复代码，未来若关键词漂移需要多处修改，可再评估抽取。
- `pick_unused_port` 依赖 OS 端口探测；极端情况下 10 次重试仍失败会让测试真的挂掉，视为需要人工排查的资源问题。
- `is_bind_race` 依赖错误消息字符串匹配；未来底层错误类型演进时需要同步更新关键词，最好在 `ProxyInstance` 提供结构化错误 kind 以取代字符串匹配。
- 测试并发度与 CI runner 端口密度：若未来 workspace 并行度大幅提高（例如 `--test-threads=32`），可能需要提高 `START_PROXY_MAX_ATTEMPTS` 或引入端口预分配池。
- 禁止在测试中使用 9900 或其它固定端口，避免与本机 bifrost 主端口冲突。
