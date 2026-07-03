# Temporary Port Rule Bindings

## 背景

Bifrost 主端口在启动时把所有全局启用的本地规则和 Group 规则合并到 `DynamicRulesResolver`。用户在调试/隔离 API mock/多环境并行时，希望在同一进程内开一个额外监听端口，用一批**显式**规则集，而不受主端口 enabled/disabled 状态影响。

新增 `bifrost port bind` 系列命令：所有端口共享同一 `BIFROST_DATA_DIR`（配置、证书、values、scripts、traffic、Group 本地缓存），差异只在"这个监听端口用哪些显式绑定规则集 + 全局 `Default`"。临时端口生命周期独立于主端口，用完可以销毁；主端口不中断、不重启，也不会隐式继承临时端口。

## 用户目标验证清单

### 必须实现

- `bifrost port bind` 支持 `--rule` / `--group-rule` / `--rule-file` / `--rule-text` 四类规则来源；至少提供一种，未提供时错误提示同时列出四种。
- 端口 `0` 自动分配可用端口并输出实际端口号。
- 临时端口 resolver 只加载：全局 `Default` + 显式绑定规则；不加载主端口其他 enabled 规则。
- 更新绑定规则内容触发对应临时端口 resolver 热更新；启用/禁用未绑定规则不影响临时端口。
- 删除已绑定规则时临时端口标为 `degraded` 并在 `port show` 展示 missing refs。
- Traffic 每条记录写入 `listener_port`（compact API `lp`）；HTTP、CONNECT、TLS intercept、WebSocket、SOCKS5 TCP/UDP 与连接错误都写入。
- HTML Badge 注入根据 `RequestContext.port` 选端口视图；临时端口 Badge 只显示该端口绑定规则。
- CLI `traffic list --port <p>` / `traffic get --port <p> <id>` 接受子命令级 `--port`。
- Settings / Proxy 页面下方展示临时端口卡片（监听地址、状态、绑定规则来源、缺失引用、resolved active rules、端口级 merged content）。
- `bifrost port destroy` 只关闭该 listener + resolver，不影响主端口 / 共享数据。

### 必须不破坏

- 主端口 PID/host/port 与 `bifrost status` 展示不变。
- 主端口 enable/disable 规则不影响任何临时端口。
- 临时端口不写系统代理、不写 shell rc、不修改共享规则文件。
- `bifrost rule add/update/delete` 只作用于共享规则数据；端口绑定只保存引用。
- 全局 `Default` 规则在所有端口自动生效；`--rule Default` 显式绑定被拒绝以避免 active summary 重复。

### 必须真实验证

- CLI 真实创建/绑定/更新/销毁；`--port 0` 输出的端口能被真实请求命中。
- 主端口 + 临时端口共享 values/scripts/certs，真实请求路径通过端口区分。
- Traffic 视图（CLI + Web）能按 listener_port 区分主/临时；未命中规则的直连请求也记录端口。
- Badge 视图在主端口和临时端口页面都真实展示对应规则集合。

## 产品语义

### 临时端口是"进程内规则视图隔离"

- 一个 Bifrost 主进程 = 一个 `ProxyServer` + 多个 `TemporaryPortManager` 管理的额外 `ProxyServer` listener。
- 每个 listener 有独立 `DynamicRulesResolver`，共享同一 `AdminState / RulesStorage / ValuesStorage / TrafficDbStore / ConnectionRegistry / TlsConfig`。
- Admin API 仍只挂在主端口；临时端口只服务代理流量 + Badge JSON 注入。

### 规则来源枚举

```rust
pub enum RuleSetRef {
    LocalRule { name: String },
    GroupRule { group_id: String, name: String },
    RuleFile { path: String },   // 端口私有一次性输入，不写入共享目录
    InlineRule { content: String }, // 同上
}
```

`LocalRule` / `GroupRule` 从共享 `RulesStorage` 读取；`RuleFile` / `InlineRule` 只存在于 binding 内存对象，进程重启后消失。

### 全局 Default 自动继承

- 每个 resolver 加载时先注入全局 `Default`（`RulesStorage::ensure_default_rule()`），再叠加显式绑定规则。
- `Default` 不受端口绑定语义影响，也不能被 `--rule Default` 显式绑定。
- 更新 `Default` 内容触发主端口和所有临时端口 resolver 重建。

### 绑定不看全局 enabled

`--rule local-dev` 即使 `local-dev` 全局 disabled 也进入临时 resolver — 绑定本身就是启用声明。这样：

- 主端口默认关掉的 mock 规则可以只在临时端口打开。
- 主端口打开的 dev 规则不会隐式污染临时端口。

### 传入顺序即执行顺序

同一命令中 `--rule a --rule b --group-rule g/x` 保序进入 resolver；同一文件内规则按文件内顺序解析。匹配仍走 `RulesResolver` 的 matcher priority。

## 技术细节

### 关键源码

| 文件 | 责任 |
| --- | --- |
| `crates/bifrost-cli/src/cli.rs` | `PortCommands { Bind, Update, Destroy, List, Show, Active }` |
| `crates/bifrost-cli/src/commands/port.rs` | CLI → Admin API 客户端；`CliTemporaryPortManager::reload_binding` |
| `crates/bifrost-admin/src/temp_ports.rs` | `RuleSetRef` / `TemporaryPortBindRequest` / `TemporaryPortBinding` / `TemporaryPortActiveSummary` / trait `TemporaryPortManager` |
| `crates/bifrost-admin/src/handlers/ports.rs` | `/api/ports` CRUD + `active-summary` |
| `crates/bifrost-admin/src/state.rs` | `AdminState.temp_ports` |
| `crates/bifrost-admin/src/openapi.rs` | OpenAPI 定义 |
| `crates/bifrost-cli/src/commands/start.rs` | `load_stored_rules(...)` 主端口路径 |
| `web/src/api/ports.ts` | Web 客户端 |
| `web/src/pages/Settings/tabs/ProxyTab.tsx` | Settings / Proxy 临时端口卡片 |
| `e2e-tests/tests/test_temporary_port_bindings.sh` | shell E2E |

### `TemporaryPortBinding`（当前实现）

```rust
pub struct TemporaryPortBinding {
    pub port: u16,             // 端口即 binding key（未额外维护 id）
    pub host: String,
    pub name: Option<String>,
    pub status: TemporaryPortStatus,   // Ready | Degraded
    pub rule_refs: Vec<RuleSetRef>,
    pub missing_refs: Vec<RuleSetRef>, // 已删除或不存在的引用
    pub created_at: i64,
    pub updated_at: i64,
}
```

设计原 `id` / `inherit_values` 未实装：`inherit_values` 恒为 true（共享 values），端口本身作为 binding key。

### `RuleSelection`（未收敛的 planned 状态）

主端口继续走 `load_stored_rules(...)`；临时端口走 `CliTemporaryPortManager::load_bound_rules(...)`。设计原 `RuleSelection { EnabledGlobalRules, ExplicitRefs(...) }` 枚举**尚未落地**（planned, not yet shipped as of 2026-06-17）。两条路径共享 parser + values 合并 + resolver `update_stored_rules(...)` 的下游流程。

### 热更新

| 事件 | 主端口 | 临时端口 |
| --- | --- | --- |
| 更新 Default 内容 | 重建 | 全部重建 |
| 启用/禁用非绑定规则 | 重建 | 不动 |
| 更新已绑定规则内容 | 视是否 enabled 决定 | 对应端口 resolver 重建 |
| 删除已绑定规则 | 视是否 enabled 决定 | 对应端口标 `degraded` + `missing_refs` |
| Group name/dir 变化 | 主端口重建 | 对应端口重新解析 group_id |

### Badge 注入端口视图

- HTTP/TLS intercept 响应体注入路径从 `RequestContext.port` 获取 listener port。
- 主端口 → `AdminState::badge_rules_json()`（默认缓存）。
- 临时端口 → `TemporaryPortManager::active_summary(listener_port)` 现算 JSON。
- TLS intercept 重建 `RequestContext` 时必须保留 `listener_port`（2026-05-08 回归修复）。

### Traffic listener_port 记录

- `TrafficRecord` + SQLite `traffic_records.listener_port`。
- Compact API 字段 `lp`；Detail 字段 `listener_port`；`has_rule_hit=false` 时仍写入端口。
- Web `VirtualTrafficTable` 行级 memo 比较器包含 `listener_port`（2026-05-07 回归修复）。

## CLI + Web + Admin API

### CLI

```bash
# 创建 / 绑定
bifrost port bind --port 18888 --name nextagent-local \
  --rule local-dev --rule mock-login \
  --group-rule 7152084678483132446/abc

bifrost port bind --port 0 --rule local-dev            # 自动分配
bifrost port bind --port 0 --rule-file ./temp.bifrost   # 私有文件
bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"

# 查看
bifrost port list
bifrost port show 18888
bifrost port active 18888

# 更新绑定，监听端口不变
bifrost port update 18888 --rule local-dev --group-rule 7152084678483132446/abc

# 销毁
bifrost port destroy 18888

# Traffic 按端口过滤
bifrost traffic list --port 18888 --format json
bifrost traffic get --port 18888 <id> --format json
```

Help 要求：

- `bifrost --help` 列出 `port`。
- `bifrost port --help` 说明多端口模型。
- `bifrost port bind --help` / `update --help` 列全 4 类来源，提示至少一种。
- `docs/cli.md` 与 `SKILL.md` 提供多端口工作流手册。

### Web UI

- Settings / Proxy：主 Proxy Address 下方渲染临时端口卡片（`/_bifrost/api/ports` + `/_bifrost/api/ports/:port/active-summary`）。
- Traffic：`Port` 列 + Detail `Proxy Port` 使用 `listener_port` / compact `lp`。
- 亮/暗色主题都要人工验证。

### Admin API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/ports` | 列出所有临时端口绑定 |
| POST | `/api/ports` | 创建绑定（body: port/host/name/rule_refs） |
| GET | `/api/ports/:port` | 单个绑定详情 |
| PATCH | `/api/ports/:port` | 更新 name / rule_refs（原子替换） |
| DELETE | `/api/ports/:port` | 销毁 |
| GET | `/api/ports/:port/active-summary` | 端口级 active summary（rules + merged content） |

错误约束：端口冲突返回可读的 `Port <n> is the main proxy port` / `Port already bound` / `Rule <name> not found`；任一引用缺失时创建失败且不启动半成品 listener。

## Sync 边界

- 端口绑定关系不落盘（第一版）；`data_dir/runtime/temp_ports.json` + `bifrost port restore` 是 planned 未上线状态。
- Sync 只覆盖规则/values/scripts 共享数据，与临时端口无关。
- `RuleFile` / `InlineRule` 是端口私有内存态，进程重启即消失。

## Phase 1-4

### Phase 1：模型 + CLI/API 骨架

- `PortCommands` + `commands/port.rs`。
- `handlers/ports.rs` CRUD。
- `AdminState` 挂 `SharedTemporaryPortManager`。

### Phase 2：运行时受管监听

- `CliTemporaryPortManager` 管理 binding map + listener task + shutdown channel。
- 复用 `TlsConfig / AccessControl / TrafficDbStore / ConnectionRegistry`；每端口独立 `DynamicRulesResolver`。
- Admin API 只在主端口挂载。

### Phase 3：规则绑定解析 + 热更新

- `load_bound_rules(rule_refs)` 从共享 `RulesStorage` + Group 目录读取。
- 全局 `Default` 自动注入。
- watcher `RulesChanged` / `ValuesChanged` → 通知 manager 重载对应 resolver。
- `active-summary` 按端口视角返回。

### Phase 4：Traffic 端口归因 + Web UI + 文档

- `TrafficRecord` + SQLite `listener_port`；compact API `lp`；覆盖 CONNECT/TLS/WebSocket/SOCKS 无规则命中的直连。
- Web `VirtualTrafficTable` 行级 memo 包含 `listener_port`。
- Badge 注入按 listener port 选视图。
- Settings / Proxy 临时端口卡片。
- CLI `traffic list/get` 挂子命令级 `--port`。
- `docs/cli.md` + `SKILL.md` 多端口手册。

## 测试方案

### 单元测试

- `test_parse_port_binding_local_rules`
- `test_parse_port_binding_group_rule_ref`
- `test_parse_port_binding_file_and_inline_refs`
- `test_parse_port_binding_requires_any_rule_input`（错误同时列出四类）
- `test_load_bound_rules_uses_shared_rule_storage`
- `test_load_bound_rules_ignores_global_enabled_state`
- `test_load_bound_rules_missing_ref_fails_atomically`
- `test_temp_port_resolver_does_not_include_main_enabled_rules`
- `test_temp_port_resolver_includes_global_default_rule`（Default 自动进入 + `--rule Default` 被拒）
- `test_temp_port_update_replaces_bound_resolver_only`

### E2E 测试

`crates/bifrost-e2e/src/tests/temporary_ports.rs`：

- `temporary_port_uses_default_plus_bound_rules`
- `temporary_port_destroy_does_not_stop_main_port`
- `temporary_port_multiple_rule_sets_preserve_order`
- `temporary_port_rule_content_hot_reload`
- `temporary_port_includes_global_default_rule`
- `temporary_port_group_rule_binding`
- `temporary_port_shares_values_scripts_and_certs`
- `temporary_port_file_and_inline_inputs`
- `temporary_port_records_listener_port`
- `temporary_port_records_listener_port_without_rule_hit`
- `temporary_port_cli_error_quality`
- `temporary_port_bind_script_allocates_distinct_ports`
- `temporary_port_web_list_rerenders_on_listener_port_change`
- `temporary_port_settings_proxy_cards_show_bound_rules`
- `temporary_port_traffic_cli_accepts_port_on_list_get`
- `temporary_port_badge_uses_listener_port_rules`

Shell：`e2e-tests/tests/test_temporary_port_bindings.sh`（使用 `assign_local_test_port <var>` 避免子 shell offset 丢失）。

### human_tests

`human_tests/temporary-port-rule-bindings.md`：

- TC-TPRB-01：创建本地规则绑定，请求命中。
- TC-TPRB-02：主端口有其他 enabled 规则，临时端口不命中。
- TC-TPRB-02B：主端口切换规则 enabled 后临时端口结果和 `port active` 不变。
- TC-TPRB-03：destroy 后不可访问，主端口仍可用。
- TC-TPRB-04：多规则集按传入顺序生效。
- TC-TPRB-05：Group 规则绑定。
- TC-TPRB-06：绑定规则内容更新触发热更新。
- TC-TPRB-06-回归-01：`test_temporary_port_bindings.sh` 各测试端口互不相同，不再报 `Port <same_port> is the main proxy port`。
- TC-TPRB-07：端口冲突 / 缺失引用清晰错误且无半启动端口。
- TC-TPRB-08：共享 values/scripts/certs。
- TC-TPRB-09：自动分配 + 指定端口冲突。
- TC-TPRB-10：`--rule-file` / `--rule-text` 直接绑定；文件读取 + 原文解析失败错误可定位。
- TC-TPRB-10-回归-01：`bifrost traffic list --help` / `traffic get --help` + `test_temporary_port_bindings.sh` 均暴露 `--port`。
- TC-TPRB-10-回归-02：主端口 Badge 只含主端口规则，临时端口 Badge 只含该端口规则。
- TC-TPRB-11：Traffic list/detail、CLI + Web 都展示 listener_port。
- TC-TPRB-11-回归-01：`pnpm --dir web test:ui traffic.spec.ts -g "加载流量列表并显示详情"` — `listener_port` 更新即时刷新。
- TC-TPRB-11-回归-02：`pnpm --dir web test:ui admin-settings.spec.ts -g "Settings Proxy 展示临时端口绑定规则详情卡片"` — Settings / Proxy 卡片来自 `/api/ports/:port/active-summary`。
- TC-TPRB-11B：无规则命中的直连请求也记录 `listener_port` / `lp`，`has_rule_hit=false`。
- TC-TPRB-12：Web Traffic 页面 light/dark 主题下 Port 列 + Detail Proxy Port 可读。
- TC-TPRB-13：`bifrost --help` / `bifrost port --help` / `bind --help` / `update --help` 完整。
- TC-TPRB-14：`SKILL.md` 多端口指南可指导创建主端口 + 多临时端口。
- TC-TPRB-15：destroy 后主端口仍可用，`bifrost status` 仍只报告主端口。

约束：临时 `BIFROST_DATA_DIR`、非 9900、`--no-system-proxy`。

### 项目校验

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
SKIP_FRONTEND_BUILD=1 bash scripts/ci/local-ci.sh --skip-static --e2e-only rules
```

Web 改动需补 lint/build + 亮暗色 human_tests。

### 2026-05-07 回归修复补充

- Shell E2E：`allocate_local_test_port` 子 shell offset 丢失 → 改为 `assign_local_test_port <var>` 直接写回目标变量。
- Web UI：`VirtualTrafficTable` 行级 memo 比较器漏 `listener_port` → 显式比较 `prevRecord.listener_port !== nextRecord.listener_port`。
- 沙箱 `127.0.0.1` 监听权限限制导致 shell E2E 和 Playwright 只跑到启动阶段并记录 `EPERM/Operation not permitted`；设计上保留"需在允许本地监听的环境中重跑 human_tests"的门禁说明。

### 2026-05-08 CLI `traffic --port` 回归修复

- `TrafficCommands::List` / `Get` 未在 clap 挂子命令级 `--port` → 补 `port: Option<u16>`；子命令 `--port` 优先，否则沿用全局 `-p/--port` 或 runtime port fallback。

### 2026-05-08 临时端口 Badge 视图回归修复

- Badge 注入直接调 `AdminState::badge_rules_json()`（只按主端口视图）→ 改为根据 `RequestContext.port` 选视图；TLS intercept 重建 context 时保留 listener port。

### 2026-05-08 Settings Proxy 临时端口卡片补充

- 新增 `/_bifrost/api/ports` + `/_bifrost/api/ports/:port/active-summary` 客户端调用。
- Settings / Proxy 页面在 Proxy Address 下方渲染临时端口卡片：监听地址、状态、绑定规则来源、缺失引用、resolved active rules、端口级 merged content；只读，重启随内存态清空。

### 2026-05-08 Traffic 端口归因补充

- `listener_port` 是流量入口元数据，非规则命中结果。CONNECT tunnel、WebSocket、SOCKS5 TCP/UDP、无规则命中的直连、连接错误记录都必须写入端口。
- E2E 增加主端口和临时端口无规则直连请求，断言 detail `listener_port` 正确且 `has_rule_hit=false`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 diff：cli/admin/state/handlers/ports/temp_ports/e2e/web。
- 重点 review：临时端口是否复用 `AdminState`；`Default` 是否在 CLI 层被 `--rule Default` 拒绝；Badge 视图是否严格按 listener_port 选；Traffic 是否所有代理路径都写 `listener_port`。
- 复测：`test_temporary_port_bindings.sh`；`cargo test -p bifrost-admin temp_ports`；Playwright traffic + admin-settings。

### 第 2 轮

- 复核第 1 轮修复。
- 再次 `git diff`；human_tests 与设计文档一致。
- 重点 review：Web `VirtualTrafficTable` 行级 memo 是否覆盖新增字段；`--rule-file` / `--rule-text` 是否只在内存态；Group name 变更后 group_id 是否重新解析。
- 复测：真实主+临时端口混合流量；destroy 后主端口不受影响；Badge 页面 light/dark。

## 风险与决策

- **admin 路由挂载**：临时端口 `ProxyConfig` 必须设 `admin_enabled=false`（或等价 flag），避免暴露 admin API。当前 router 若统一挂载需要额外拦截。
- **SOCKS5 统一端口**：跟随 `enable_socks`；`--socks5-port` 单独指定第一版不支持。
- **access control**：复用主配置；端口级 access profile 不在第一版。
- **`--port 0` 端口输出**：CLI 必须输出最终端口并写入 binding map，否则脚本无法拿到端口。
- **不落盘**：符合"临时"心智，避免主进程重启后自动占用旧端口；`--persist` + `port restore` 是 planned 未上线。
- **共享 traffic DB**：所有端口共享同一 DB，需要靠 `listener_port` 才能区分；查询/UI 必须默认按端口过滤展示。
- **Default 显式绑定**：拒绝 `--rule Default`，避免 active summary 双计；同时保留自动继承，向用户解释"全局 Default 自动生效"。
- **Group 规则依赖 group_id**：Group 同步后 group_id 与本地目录映射变化时 manager 必须重新解析，不能缓存过期 path。

## 依赖项

- `RulesStorage` / `ValuesStorage`（共享数据）。
- `DynamicRulesResolver::update_stored_rules(...)` 已有的热更新能力。
- Group 本地 name cache。
- 主进程 `TlsConfig` / `AccessControl` / `TrafficDbStore` / `ConnectionRegistry` / `AdminState`。
- Web `useTlsConfigStore` / Traffic store。
