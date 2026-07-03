# Network 导出生效规则快照 设计方案

## 背景

Bifrost 的 Network 面板支持把选中请求右键导出成 `.bifrost` 文件用于分享或复盘。历史上导出内容只有请求/响应体、header、时间戳和运行时命中的 `matched_rules`。

问题是：`matched_rules` 只告诉你 "这条请求 A 命中了规则 R1、R2"，但如果用户反馈 "为什么这条请求没有走 host 规则"，排查方仍需要知道 "请求进来时对应端口正在生效的完整规则集合"。特别是 Bifrost 支持临时端口 + 端口绑定规则的场景，同一份请求文件在不同接收方本地打开时，本地的规则集合完全不同，导致复盘困难。

本方案在导出 `.bifrost` network record 时附上 **该请求进入端口时正在生效的完整规则快照**，包含默认端口场景与自定义端口场景的分支，并且在无法获取快照时用显式 `unavailable_reason` 标记，避免静默降级。

## 用户目标验证清单

### 必须实现

- `.bifrost` network record 新增 `listener_port` 与 `active_rules` 两个字段。
- `active_rules` 区分 `default_port` 与 `custom_port` 两个 source。
- 默认端口场景导出主端口 enabled 的所有本地规则 + group 规则内容；`merged_content` 是运行时加载顺序拼接的完整文本。
- 自定义端口场景通过 `TemporaryPortManager::active_summary(listener_port)` 读取该端口绑定的规则集合，不混入默认端口规则。
- 快照获取失败时写 `unavailable_reason`，不吞异常。
- 导出请求文件在没有 `active_rules` 字段的旧版本 Bifrost 中仍可解析（serde 向后兼容）。

### 必须不破坏

- `matched_rules`、`request`、`response` 等既有字段结构不变。
- 现有导入路径（parser.rs / writer.rs）对旧文件缺字段可继续导入。
- 无 group 规则 / 无自定义端口时导出行为与旧版本等价，仅多一个 `default_port` 快照。
- Traffic panel 显示、右键菜单、批量导出保持一致。

### 必须真实验证

- 单元测试证明默认端口不带禁用规则、自定义端口不混入默认端口规则。
- E2E 在 `test_temporary_port_bindings.sh` 中同时跑默认端口与临时端口流量并对比导出内容。
- 真实场景：`human_tests/network-export.md` 覆盖导出 → 导入 → 校验快照完整性闭环。

## 产品语义

### `.bifrost` 新字段

Network record（在 `crates/bifrost-core/src/bifrost_file/types.rs` 定义）新增：

- `listener_port: Option<u16>` — 请求实际进入 Bifrost 的端口。旧文件为 `None`，兼容读入 `0`。
- `active_rules: Option<ActiveRulesExport>` — 生效规则快照，缺失表示旧文件。

```rust
struct ActiveRulesExport {
    source: ActiveRulesSource,          // default_port | custom_port
    admin_port: u16,
    listener_port: u16,
    total: usize,                       // 规则文件总数聚合
    rules: Vec<ActiveRuleFileEntry>,    // 明细
    merged_content: String,             // 按加载顺序拼接
    unavailable_reason: Option<String>, // 快照获取失败时的显式原因
}

struct ActiveRuleFileEntry {
    name: String,
    rule_count: usize,
    group_id: Option<String>,
    group_name: Option<String>,
    content: String,
}
```

### 默认端口分支

`build_default_port_active_rules_export(state)`：

- 从 `AdminState.rules_storage.load_enabled()` 读取所有 enabled 本地规则。
- 从 group 目录读取 enabled group 规则文件（`handlers/group_rules.rs`）。
- 按运行时加载顺序（`Default` 优先，其它按 `sort_order`）拼接成 `merged_content`。
- 禁用规则不进入。

### 自定义端口分支

`build_custom_port_active_rules_export(listener_port, state)`：

- 优先通过 `TemporaryPortManager::active_summary(listener_port)` 读取该端口绑定的规则。
- 结果按 summary 顺序展开到 `ActiveRuleFileEntry`；`merged_content` 与之对齐。
- 未获取到 manager 或未找到该 port 时走 `unavailable_active_rules_export`，写 `unavailable_reason`。

### 分支选择逻辑

`build_active_rules_export(listener_port, state)`（`handlers/bifrost_file.rs:853`）：

```rust
if listener_port != 0 && listener_port != admin_port {
    return build_custom_port_active_rules_export(listener_port, state).await;
}
build_default_port_active_rules_export(state)
```

`listener_port == 0`（旧 traffic 无来源信息）或 `== admin_port` 时按默认端口处理。

## 技术细节

### 关键文件与函数

| 位置 | 作用 |
| ---- | ---- |
| `crates/bifrost-core/src/bifrost_file/types.rs` | 新增 `listener_port`、`ActiveRulesExport`、`ActiveRuleFileEntry`、`ActiveRulesSource` |
| `crates/bifrost-core/src/bifrost_file/parser.rs` | 解析路径向后兼容缺失字段 |
| `crates/bifrost-core/src/bifrost_file/writer.rs` | 写出新字段 |
| `crates/bifrost-admin/src/handlers/bifrost_file.rs:796` | `traffic_to_network_record` 组装 record |
| `crates/bifrost-admin/src/handlers/bifrost_file.rs:849` | 组装 `active_rules` 字段 |
| `crates/bifrost-admin/src/handlers/bifrost_file.rs:853` | `build_active_rules_export` 分支入口 |
| `crates/bifrost-admin/src/handlers/bifrost_file.rs:864` | `build_custom_port_active_rules_export` |
| `crates/bifrost-admin/src/handlers/bifrost_file.rs` | `build_default_port_active_rules_export`、`unavailable_active_rules_export` |
| `crates/bifrost-admin/src/handlers/group_rules.rs` | group 规则目录读取 |
| `crates/bifrost-cli/src/commands/status.rs:343` | CLI status 消费同样的 `listener_port` 展示 |

### `TrafficRecord.listener_port` 来源

代理入口（`crates/bifrost-proxy`）在每条请求进入时把接收 socket 的 local port 写入 `TrafficRecord.listener_port`。默认端口和临时端口共享同一份 traffic 存储；导出时按 port 决定分支。

### 导出 API

- `POST /_bifrost/api/bifrost-file/export/network`：请求体传选中的 traffic IDs；返回 `.bifrost` 文件（应用/octet-stream 或 JSON zip）。
- 每条 record 独立生成 `active_rules`，因为不同请求可能来自不同端口。

## CLI + Web + Admin API

### CLI

无新增子命令。`bifrost status --format json`（`crates/bifrost-cli/src/commands/status.rs:343`）已经读 `listener_port`，输出对齐。

### Web UI

- Traffic 面板右键 "Export as .bifrost"（既有入口）。
- 无 UI 层新增；导出内容变化对用户完全透明，只在导入时能看到 `active_rules` 字段。
- 未来若引入 "预览 active_rules" UI，可基于 `merged_content` 直接展示。

### Admin API

- `POST /_bifrost/api/bifrost-file/export/network` 响应结构中 record 新增 `listener_port` + `active_rules`。
- 未新增独立 API；`active_rules` 是导出 record 的内嵌字段。

## Sync 边界

- 导出 `.bifrost` 是本地文件产物，不参与任何 sync。
- `active_rules` 快照是导出瞬间本机 rules_storage / TemporaryPortManager 的读取结果；接收方导入后不会把这些规则自动写回本地。
- 导入端只把 `.bifrost` 作为查看/回放素材；规则应用需要用户显式操作。

## Phase 拆分

### Phase 1：数据结构与 serde

- `bifrost_file/types.rs` 新增 `listener_port` / `active_rules`。
- parser/writer 向后兼容。
- 单元测试覆盖旧文件解析、新文件序列化对称。

### Phase 2：默认端口快照

- `build_default_port_active_rules_export`：读 rules_storage + group 规则，拼接 `merged_content`。
- 单元测试 `network_export_attaches_default_port_active_rules`。

### Phase 3：自定义端口快照

- `build_custom_port_active_rules_export`：`TemporaryPortManager::active_summary` 读端口绑定规则。
- `unavailable_active_rules_export` 兜底。
- 单元测试 `network_export_uses_custom_port_active_rules_for_request_port`。

### Phase 4：E2E + 真实场景

- 更新 `e2e-tests/tests/test_temporary_port_bindings.sh` 产生默认与临时端口流量，导出并断言。
- 新增 `human_tests/network-export.md`。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `network_export_attaches_default_port_active_rules`：默认端口请求导出 `default_port` 快照，只包含 enabled 规则，不包含 disabled。
- `network_export_uses_custom_port_active_rules_for_request_port`：自定义端口请求导出 `custom_port` 快照，不混入默认端口规则。
- `bifrost_file::types::active_rules_backward_compat`：旧文件缺 `active_rules` 字段可解析。
- `bifrost_file::types::listener_port_backward_compat`：旧文件缺 `listener_port` 字段解析成 `None`。

### E2E 测试

- `e2e-tests/tests/test_temporary_port_bindings.sh`：
  1. 启动 Bifrost + 建临时端口 + bind 一条测试规则。
  2. 从默认端口发一次请求，从临时端口发一次请求。
  3. 调用 export API，解析 `.bifrost` 内容。
  4. 断言默认端口 record `active_rules.source == default_port`，`listener_port == admin_port`，包含所有 enabled 本地规则文件名。
  5. 断言临时端口 record `active_rules.source == custom_port`，`listener_port == 该端口`，只包含 bind 的规则，不包含默认端口规则。
- 关联：`test_rule_filter_routing_diagnostics.sh` 中的 traffic 记录也应包含 `listener_port`，导出后 `active_rules` 齐全。

### 真实场景测试

`human_tests/network-export.md`：

- TC-NEA-01：默认端口请求 → 右键导出 → 打开 `.bifrost` 文件确认 `active_rules.source == default_port` 与 `merged_content` 完整。
- TC-NEA-02：自定义端口请求 → 右键导出 → `active_rules.source == custom_port`，只含绑定规则。
- TC-NEA-03：空选择导出保护 → API 返回 400 / UI 禁用按钮。
- TC-NEA-04：导入兼容 → 旧版本导出的 `.bifrost`（无 `active_rules`）在新版本能被正确解析且不 crash。
- TC-NEA-05：TemporaryPortManager 不可用时 → `unavailable_reason` 显式写入，不吞异常。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：请求导出必须包含端口对应生效规则；默认端口与自定义端口必须区分；不静默降级。
- 复核 diff：序列化结构、端口判断、rules_storage 读取、TemporaryPortManager 读取、错误显式化、parser/writer 向后兼容。
- 重点 review：
  - `listener_port == 0` 或 `== admin_port` 都走默认端口分支的正确性；
  - group 规则内容是否按运行时顺序进入 `merged_content`；
  - Disabled 规则一定不进入；
  - 旧 `.bifrost` 文件缺字段的兼容路径。
- 复测：`cargo test -p bifrost-admin network_export`、`bash e2e-tests/tests/test_temporary_port_bindings.sh`。

### 第 2 轮

- 基于最新 diff 复查 types / handler / E2E / human_tests / 索引。
- 重点 review：
  - `.bifrost` 序列化大小（`merged_content` 可能较大，是否需要压缩）；
  - 大量选中请求批量导出的性能（每条 record 独立快照，重复开销）；
  - `TemporaryPortManager::active_summary` 并发下与端口销毁竞态时的表现（应返回 `unavailable_reason`）。
- 若发现 group 规则漏进快照、临时端口销毁瞬间导出报错、导入兼容失败，追加第 3 轮。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin network_export`
- `cargo test -p bifrost-core bifrost_file`
- `bash e2e-tests/tests/test_temporary_port_bindings.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 本机 no-local-coverage 约定时不跑 `make coverage`。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引，新增 Network 导出生效规则快照真实场景用例。
- 更新 `human_tests/network-export.md` 覆盖 5 个 TC。
- 本次变更不新增 CLI 参数、配置项或公开规则协议，README 无需同步。

## 风险与决策点

- **导出体积**：`merged_content` 是完整文本，大量请求批量导出时体积明显增加。当前不做压缩，理由是 `.bifrost` 通常包含 body/header 已经不小，规则文本占比可接受；若未来出现体积问题，可以引入 record 级或文件级 gzip。
- **重复快照**：批量导出同端口多条请求时，每条 record 都独立组装 `active_rules`，规则文本重复。第一版接受简单实现，避免共享引用带来的 parser 复杂度；后续可考虑 `.bifrost` 头部维护 rule snapshot 池 + record 引用 ID。
- **`listener_port == 0` 语义**：旧 traffic 或从磁盘回放的老 record 可能没有 port 信息，一律按默认端口处理。若未来引入更多端口维度（例如透明代理入口），需要扩展分支。
- **自定义端口销毁竞态**：请求命中的临时端口在导出瞬间可能已被销毁；此时 `TemporaryPortManager::active_summary` 返回 `None`，走 `unavailable_active_rules_export`，写清晰原因。不 fallback 到默认端口规则，避免误导。
- **敏感规则内容**：`merged_content` 会包含用户完整规则文本，可能包含 `[ph_TOKEN_1_ph]`、内部域名等。用户在分享 `.bifrost` 前必须自行确认；本方案不做自动脱敏，避免破坏排查场景。
- **未来 UI 消费**：Traffic panel 若增加 "View active rules snapshot" 视图，可以直接消费 `active_rules.rules` 数组渲染，无需再打后端。
