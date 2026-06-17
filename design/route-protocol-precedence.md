# 路由协议优先级修正方案

## 功能模块详细描述

- 修正同一请求同时命中多个转发类协议时的最终 upstream 选择。
- 受影响协议包括 `host`、`xhost`、`http`、`https`、`ws`、`wss` 等共享实际转发目标的路由协议。
- 用户场景中，路径级裸目标规则会解析为 `host`，域名级兜底 `https://...` 会解析为 `https`；两条规则都会命中，但转换层把后命中的域名兜底覆盖成最终 upstream。

## 实现逻辑

- 保留 core resolver 现有匹配和排序逻辑：规则命中列表继续完整展示，便于 Traffic、Network 导出和诊断看到所有命中规则。
- 在转换到代理执行结构时，把转发目标视为一个共享槽位。
- core resolver 输出会按 matcher 优先级排序，因此路径更具体的规则先写入 `resolved_rules.host`。
- 保留既有 `xhost` 特殊语义：同一请求同时命中 `host` 与 `xhost` 时，`xhost` 可以覆盖 `host`。
- 除 `xhost` 覆盖 `host` 外，后续命中的路由协议不能覆盖已经选中的 upstream，避免域名级 `https` 兜底覆盖路径级 `host` 排除规则。
- 后续命中的其他路由协议仍进入 `resolved_rules.rules` 诊断列表，但不再覆盖 `host` / `host_protocol`。
- 同步更新 bifrost-e2e 中的同构转换逻辑，避免 E2E runner 和生产路径行为不一致。
- 实现入口：`crates/bifrost-cli/src/parsing/rules.rs` 中的 `should_update_route_target`（rules.rs:793）守门，配合 `Protocol::Host|XHost|Http|Https|Ws|Wss` 分支（rules.rs:490 起）写入 `result.host` / `result.host_protocol`；`crates/bifrost-e2e/src/proxy.rs` 中复刻同名函数（proxy.rs:53）与协议分支（proxy.rs:296/491/495/499/503/507）。`ProxyResolvedRules.host_protocol` 字段定义在 `crates/bifrost-proxy/src/server.rs:434`。

## 依赖项

- `crates/bifrost-cli/src/parsing/rules.rs`
- `crates/bifrost-e2e/src/proxy.rs`
- `crates/bifrost-e2e/src/tests/routing.rs`

## 测试方案

- 单元测试：覆盖 `Host` 路径规则与 `Https` 域名兜底同时命中时，最终 upstream 保持优先级最高的路径规则。已落地为 `test_merge_keeps_first_route_target_across_protocols`（`crates/bifrost-cli/src/parsing/rules.rs:1232`）。
- 单元测试：覆盖路径规则未命中时，域名级 `Https` 兜底仍正常生效（覆盖在同文件的 `test_merge_*` 系列回归中，含纯 `Https` 兜底用例）。
- 单元测试：覆盖 `xhost` 仍可覆盖同 pattern 的 `host`，避免破坏既有路由语义。已落地为 `test_merge_keeps_xhost_priority_over_host`（`crates/bifrost-cli/src/parsing/rules.rs:1281`）。
- E2E 测试：新增 `routing_path_host_vs_domain_https`，使用本地 mock server 验证路径级 `host` 规则不会被域名级 `https` 兜底覆盖。已注册于 `crates/bifrost-e2e/src/tests/routing.rs:91`，实现位于同文件 `routing.rs:456`。
- E2E 测试：复跑 `routing_xhost_priority`（`crates/bifrost-e2e/src/tests/routing.rs:37/251`）与 `test_xhost_over_host`（`crates/bifrost-e2e/src/tests/rule_priority.rs:476`），确认 `xhost` 优先级不回退。
- human_tests：在 `human_tests/proxy-rules-advanced.md`（约第 2460 行起）增加路由协议跨协议优先级回归用例，并按文档执行；2026-06-09 已记录 PASS 执行结果。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户抓包场景、转换层 diff、单元测试和 E2E 用例；运行定向测试并修复失败。
- 第 2 轮：基于最新 diff 再次检查跨协议路由共享槽位是否有遗漏，复跑定向单元和 E2E 测试。
- 如果格式、clippy、workspace test 或 E2E 出现失败，先归因到实现、测试或环境，再追加复查轮次。

## 校验要求

- `cargo test -p bifrost-cli route_target -- --nocapture` 或等价定向测试必须通过。
- `cargo run -p bifrost-e2e -- --test routing_path_host_vs_domain_https` 必须通过。
- 收尾前按项目要求执行 `rust-project-validate` 覆盖的格式、lint、构建和 workspace 测试；如果受环境阻塞，必须记录阻塞证据。

## 文档更新要求

- 不新增协议和 CLI 参数，不需要更新 README 协议表。
- 更新 `human_tests/proxy-rules-advanced.md` 和 `human_tests/readme.md`，纳入真实场景回归。
