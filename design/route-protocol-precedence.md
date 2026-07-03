# 路由协议优先级修正方案

## 背景

Bifrost 的规则支持多种"路由目标"类协议（`host`、`xhost`、`http`、`https`、`ws`、`wss`），它们都会决定同一个请求最终转发到哪个 upstream。用户经常同时写路径级具体规则（例如 `example.com/api/foo host://10.0.0.53`）和域名级兜底（例如 `example.com https://backup.example.net`）。core resolver 会把两条规则都算作命中，让 Traffic 面板和 Network 导出可以完整看到匹配情况；但转换到 proxy 执行结构时，只能选一个 upstream。

历史 bug：转换层没有守门，后命中的域名兜底会覆盖前命中的路径规则，导致用户"精细路径规则"被"域名兜底"吞掉，抓包 upstream 与预期不符。修复思路是：在 resolved rules 转换成 proxy 执行结构时，把 upstream 视为共享槽位，只允许第一次写入生效；仅保留 `xhost` 覆盖 `host` 的既有语义。

## 用户目标验证清单

### 必须实现

- 同一请求同时命中路径级 `host`（更具体）与域名级 `https` 兜底时，最终 upstream 使用路径规则的 host。
- `xhost` 依然能覆盖同 pattern 的 `host`，保持既有语义（xhost 表示 external host，为一种"更强的意图"）。
- resolver 输出的规则命中列表继续完整展示所有命中规则，Traffic / Network 导出无变化。
- 转换层给 `resolved_rules.host` / `resolved_rules.host_protocol` 只写一次（第一次胜出），后续 `Host|Http|Https|Ws|Wss` 类协议不再覆盖。
- CLI 和 bifrost-e2e runner 使用完全一致的转换逻辑，避免 E2E 与线上分叉。

### 必须不破坏

- 单独命中一条路由协议时行为不变。
- 域名级 `https` 单独命中时仍能作为 upstream 生效。
- `dns` / `status` / `resBody` / `reqHeader` 等非路由类协议行为不受本次改动影响。
- `xhost` 优先级 > `host` 的既有语义保留。
- 循环 / 冲突规则的既有诊断和 traffic matched rules 展示保持一致。

### 必须真实验证

- 单元测试 `test_merge_keeps_first_route_target_across_protocols` 通过。
- 单元测试 `test_merge_keeps_xhost_priority_over_host` 通过。
- E2E `routing_path_host_vs_domain_https` 与 `routing_xhost_priority` 通过。
- 手工用 curl 实际抓包验证抓到路径规则指向的 upstream。

## 产品语义

### 路由类协议

以下协议都表示"决定最终 upstream 或 upstream scheme"：

- `host`：改写 upstream host（保留原 scheme）。
- `xhost`：external host，语义上比 `host` 更强，允许覆盖已有 host。
- `http` / `https`：改写 upstream scheme + host。
- `ws` / `wss`：WebSocket upstream 改写。

其他协议（`dns`、`status`、`resBody`、`reqHeader`、`reqBody`、`script`、`breakpoint` 等）不参与共享槽位。

### 匹配 vs 生效

- **匹配**：core resolver 按 matcher 优先级排序输出所有命中规则；命中列表用于 Traffic 展示与诊断。
- **生效**：转换到 `ProxyResolvedRules` 时，路由类协议使用共享槽位；先写入者胜出。
- 命中列表不因"生效槽位已占用"而剔除后续规则；后续路由类规则仍会出现在 `resolved_rules.rules` 里，只是不覆盖 host/host_protocol。

### 具体规则优先原则

`RulesResolver` 已经按 matcher priority 从高到低排序，路径级规则会先命中。所以"先写入胜出" ≡ "matcher 更具体的规则胜出"，与 Bifrost 现有 rule specificity 语义一致。

### xhost 例外

`xhost` 保留可覆盖同 pattern `host` 的既有语义。守门函数 `should_update_route_target` 里显式判断 `(Some(Protocol::Host), Protocol::XHost)` 允许覆盖，其他组合不允许。

## 技术细节

### 核心守门函数

`crates/bifrost-cli/src/parsing/rules.rs:978` 起：

```rust
fn should_update_route_target(result: &ProxyResolvedRules, protocol: Protocol) -> bool {
    match (result.host_protocol, protocol) {
        (None, _) => true, // 槽位空，第一次写入
        (Some(Protocol::Host), Protocol::XHost) => true, // xhost 可覆盖 host
        _ => false, // 已被非 xhost 组合占用，或后续路由类协议不能覆盖
    }
}
```

### 协议分支

`crates/bifrost-cli/src/parsing/rules.rs:472-481`：

```rust
Protocol::Host
| Protocol::XHost
| Protocol::Http
| Protocol::Https
| Protocol::Ws
| Protocol::Wss
    if should_update_route_target(&result, protocol) =>
{
    result.host = ...;
    result.host_protocol = Some(protocol);
}
```

未通过守门的分支：仍进入 `result.rules.push(rule)`，但不修改 `host` 与 `host_protocol`。

### `ProxyResolvedRules.host_protocol`

`crates/bifrost-proxy/src/server.rs:434` 定义 `pub host_protocol: Option<Protocol>`；后续 upstream 构造读取该字段决定 upstream scheme。

### bifrost-e2e runner 复刻

`crates/bifrost-e2e/src/proxy.rs:53` 提供同名 `should_update_route_target`；协议分支 `proxy.rs:296/491/495/499/503/507` 一一对应 `Host/XHost/Http/Https/Ws/Wss`。这样 E2E runner 与 CLI/proxy 保持行为一致。

### 单元测试位置

- `test_merge_keeps_first_route_target_across_protocols`：`crates/bifrost-cli/src/parsing/rules.rs:1661`
- `test_merge_keeps_xhost_priority_over_host`：`crates/bifrost-cli/src/parsing/rules.rs:1710`
- 相关 assert：`resolved.host_protocol == Some(Protocol::Https|Host|XHost|Tunnel)` 见 `rules.rs:1554/1657/1682/1705/1725`。

### E2E 测试位置

- `routing_xhost_priority`：`crates/bifrost-e2e/src/tests/routing.rs:37`，函数 `test_routing_xhost_priority` at line 251，收敛于 `routing.rs:739`。
- `routing_path_host_vs_domain_https`：`crates/bifrost-e2e/src/tests/routing.rs:91`，函数 `test_routing_path_host_vs_domain_https` at line 456，收敛于 `routing.rs:763`。
- `test_xhost_over_host`：`crates/bifrost-e2e/src/tests/rule_priority.rs:476`。

## CLI + Web + Admin API

- CLI：无新增子命令；`bifrost port active <port>` 输出保持既有格式。
- Web：Traffic 面板 matched rules 列表继续展示所有命中规则；无 UI 语义变化。
- Admin API：无新增端点；`GET /api/traffic/:id` 返回的 matched rules 字段保持不变。

## Sync 边界

- 本次改动是转换层逻辑，不涉及规则文件字段变化，rule sync / group sync 不受影响。
- 已同步的规则在本机应用本次逻辑后，行为与用户预期一致。
- 分享 URL 不涉及。

## Phase 1 — CLI 转换层守门

- 引入 `should_update_route_target`。
- Protocol 分支加 `if should_update_route_target(...)` 守卫。
- 单元测试：`test_merge_keeps_first_route_target_across_protocols`、`test_merge_keeps_xhost_priority_over_host`。

## Phase 2 — E2E runner 对齐

- `crates/bifrost-e2e/src/proxy.rs` 复刻 `should_update_route_target` 与协议分支守卫。
- 新增 `routing_path_host_vs_domain_https` 测试。
- 复跑 `routing_xhost_priority` 与 `test_xhost_over_host`。

## Phase 3 — Traffic 显示对齐

- 保持 matched rules 列表完整展示；转换层未生效的路由规则仍进入 traffic 命中列表。
- Web / CLI Traffic 展示无改动，但需回归确认 rule id 顺序稳定。

## Phase 4 — 文档与真实回归

- `human_tests/proxy-rules-advanced.md` 约第 2460 行起增加跨协议优先级用例：
  - `TC-PRA-RT-01`：路径 host + 域名 https，upstream 为路径 host。
  - `TC-PRA-RT-02`：路径 host + 同 pattern xhost，upstream 为 xhost。
  - `TC-PRA-RT-03`：只有域名 https 时，upstream 为域名 https。
- `human_tests/readme.md` 同步。
- 无 README 协议表变化。

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli test_merge_keeps_first_route_target_across_protocols`。
- `cargo test -p bifrost-cli test_merge_keeps_xhost_priority_over_host`。
- 同文件相关 `test_merge_*` 系列回归，含单独 `Https` 兜底用例，确保没有回归。

### E2E

- `cargo run -p bifrost-e2e -- --test routing_path_host_vs_domain_https`。
- `cargo run -p bifrost-e2e -- --test routing_xhost_priority`。
- `cargo run -p bifrost-e2e -- --test test_xhost_over_host`。

### 真实场景 human_tests

- `human_tests/proxy-rules-advanced.md`：`TC-PRA-RT-01/02/03` 已记录 2026-06-09 的 PASS 执行结果，本次刷新沿用。
- `human_tests/readme.md`：同步用例编号。

### 环境约束

- 使用 bifrost-e2e mock server 与临时数据目录；不使用 9900 端口，`--no-system-proxy`。
- Rust 单测无需网络。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 CLI 与 bifrost-e2e 两处守门函数是否完全同名同逻辑，包括 xhost 例外。
- 复核 traffic matched rules 是否仍包含所有命中规则，不误剔除。
- 运行三条 E2E 与两条单元测试。

### 第 2 轮

- 基于第 1 轮修复复查 diff：确认没有额外协议误加入路由槽位（比如误把 dns 加入）。
- 复跑 `test_xhost_over_host` 保证既有语义不回退。
- 抓包对比 upstream 与预期一致。

## 风险与决策

- 决策：只让 `xhost` 可以覆盖 `host`；其他组合一律不覆盖。原因是 `xhost` 表达用户"我真的要走外部 host"的强意图，历史行为不能破坏。
- 决策：转换层守门放在 CLI 侧的 `parsing::rules`，是因为该模块是所有 resolver 输出到 proxy 结构的必经路径；无需修改 core resolver。
- 决策：bifrost-e2e 复刻同名函数而非提取到公共 crate，是保持 e2e runner 与生产 crate 依赖解耦；后续如果频繁分叉，再考虑抽公共。
- 风险：如果新增未来路由类协议（如 `quic://`），必须同步加入两处 protocol 分支与 xhost 例外表；漏加会导致同类 bug 复现。
- 风险：matched rules 顺序被 traffic UI 依赖；本次改动只影响生效槽位，不影响 rules 列表顺序，需要单测覆盖以防未来 refactor 破坏。

## 实现现状（截至 2026-07-03）

- `should_update_route_target` 已在 `crates/bifrost-cli/src/parsing/rules.rs:978` 实现，Protocol 分支于 490 起使用守卫（472-481 覆盖 Host/XHost/Http/Https/Ws/Wss）。
- bifrost-e2e 侧同名函数位于 `crates/bifrost-e2e/src/proxy.rs:53`，协议分支于 296/491/495/499/503/507。
- `ProxyResolvedRules.host_protocol` 字段定义于 `crates/bifrost-proxy/src/server.rs:434`。
- 单元测试 `test_merge_keeps_first_route_target_across_protocols`（`crates/bifrost-cli/src/parsing/rules.rs:1661`）与 `test_merge_keeps_xhost_priority_over_host`（`rules.rs:1710`）覆盖两类关键路径。
- E2E `routing_path_host_vs_domain_https`（`crates/bifrost-e2e/src/tests/routing.rs:91`）、`routing_xhost_priority`（`routing.rs:37`）与 `test_xhost_over_host`（`crates/bifrost-e2e/src/tests/rule_priority.rs:476`）全部落地。
- `human_tests/proxy-rules-advanced.md` 已包含 2026-06-09 PASS 记录。
- 本设计文档无待落地项。
