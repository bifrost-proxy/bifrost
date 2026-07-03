# Traffic Port Filter 设计方案

## 背景

Bifrost 支持多个代理入口端口：主代理端口 + 若干临时端口（`bifrost port bind` 绑定的显式规则集合端口）。每一条 traffic summary 里都带 `listener_port` 字段，用于标记这条请求是从哪个代理入口进来的。

在多端口调试场景下，用户经常需要“只看某个临时端口产生的流量”或“把主端口 vs 临时端口的流量分开对比”。历史实现里既没有 UI 主筛选器 `Port`，也没有 CLI `--listener-port`，只能人肉在 traffic 详情里看 `listener_port` 字段判断。

本方案为 Traffic 页面主筛选器、Fuzzy Search、CLI（本地 + 远端）与 Admin API 补齐按代理入口端口过滤的完整能力。

> 实现状态（2026-07-03 复核）：本能力已完整落地：
> - `crates/bifrost-admin/src/traffic_db/query.rs:50` 定义 `listener_port: Option<u16>` 并在 `build_where_clause` 中生成 `listener_port = ?` 条件；单元测试 `build_where_clause_supports_listener_port_filter`（`traffic_db/query.rs:317`）已通过。
> - `crates/bifrost-admin/src/openapi.rs:452` 声明 `listener_port` query 参数。
> - `crates/bifrost-cli/src/cli/remote.rs:1064,1180` 定义 `--listener-port` 与 `visible_alias = "proxy-port"`；`crates/bifrost-cli/src/main.rs:379,548,629` 与 `commands/remote.rs:2959,4033,7560+` 将 CLI 参数映射到 `filter_listener_port` / `SearchCondition{field:"listener_port"}`。
> - `crates/bifrost-cli/tests/cli_help.rs:184-209` 覆盖 `traffic search help lists --listener-port / --proxy-port`。
> - `crates/bifrost-admin/src/search/engine.rs:490,660` 将 `listener_port equals` 下推到 SQL 并在 compact 层校验 `lp` 字段。
> - `e2e-tests/tests/test_temporary_port_bindings.sh:494,501` 用 `bifrost traffic list/search --listener-port` 覆盖临时端口筛选。
> - Web `web/src/components/FilterBar/index.tsx` 已支持 `Port` 主筛选（`listener_port`），Traffic 详情 `Overview` 展示 `Proxy Port`，Playwright `web/tests/ui/traffic.spec.ts:369-529` 覆盖端口筛选与详情展示。

## 用户目标验证清单

### 必须实现

- WebUI `FilterBar` 主筛选器提供 `Port` 字段，内部字段名为 `listener_port`。
- 选择 `Port` 默认 `Equals` 操作符，展示端口输入框。
- 前端本地过滤把 `record.listener_port` 转字符串参与 `contains / equals / regex / not_contains / is_empty / is_not_empty`。
- Admin API `/traffic` 查询支持 `?listener_port=<port>`，用于刷新、API 校验、服务端查询路径。
- Fuzzy Search `filters.conditions` 支持 `listener_port`，`equals` 下推到 SQL，其它操作符走 compact 层用 `lp` 字段校验。
- CLI `traffic list` / `traffic search` / 顶层 `search` / `remote traffic list` / `remote traffic search` 新增 `--listener-port <PORT>`，同时提供 `--proxy-port` 可见别名。
- CLI `traffic list/get --port` 保留原语义（Admin API 端口选择），端口筛选统一使用 `--listener-port` 避免冲突。
- Traffic 详情页展示 `Proxy Port`。

### 必须不破坏

- `traffic list --port` 仍表示“连接到哪台 Bifrost Admin API”。
- 现有 Toolbar / Filter Panel / Add Filter 行为不受影响。
- Fuzzy Search scope、其它 `filters.conditions` 字段（host / path / method / status ...）行为不变。
- Traffic Push、SSE frame store、Replay 导入路径都读同一 `listener_port` 字段，不因新增筛选器改动。

### 必须真实验证

- Playwright：主筛选器出现 `Port`，选中后默认 `Equals`，输入端口后列表只展示匹配记录。
- Admin API：`/traffic?listener_port=<port>` 只返回目标端口流量。
- Fuzzy Search：`filters.conditions` 含 `listener_port equals <port>` 只返回目标端口流量。
- Shell E2E：`bifrost traffic list --listener-port` 与 `bifrost traffic search --listener-port` 只返回目标端口记录（`test_temporary_port_bindings.sh` 覆盖）。
- CLI 单测：`--listener-port` 与 `--proxy-port` 别名等价，请求体 `condition.field == "listener_port"`。

## 产品语义

### 什么是 `listener_port`

- `listener_port` 是 traffic summary / detail 中记录“该请求从哪个代理入口进来的”的 u16 字段。
- 主端口默认在 `bifrost start --port <MAIN>` 指定。
- 临时端口由 `bifrost port bind --port <TEMP> --rule <RULE>` 创建，用于隔离调试。
- Traffic 详情页把 `listener_port` 显式渲染为 `Proxy Port`。

### 为什么用 `--listener-port` 而不是 `--port`

`--port` 已经用于选择 `traffic list / get` 的 Admin API 端口，语义是“连哪台 Bifrost”。若同名将导致：

- 语义混淆：`bifrost traffic list --port 18888` 是“连到 18888 的 Bifrost”还是“只看走 18888 的流量”。
- 已有脚本回归。

因此新增 `--listener-port <PORT>` 表示“过滤走某个代理入口的流量”，同时提供 `--proxy-port` 可见别名，让 CLI 命名更直观。

### 前端本地 vs 后端下推

- Fuzzy Search / 后端 `/traffic?listener_port=` 只支持 `equals` 下推到 SQL WHERE。
- 其它操作符（`contains / regex / not_contains / is_empty / is_not_empty`）：前端本地过滤 + Fuzzy Search compact 层校验 `lp` 字段。

## 技术细节

### Admin API 侧

`crates/bifrost-admin/src/traffic_db/query.rs`：

```rust
pub struct TrafficQuery {
    // ... 现有 host / method / status ...
    pub listener_port: Option<u16>,
}

impl TrafficQuery {
    fn build_where_clause(&self, conditions: &mut Vec<String>, params: &mut Params) {
        // ...
        if let Some(port) = self.listener_port {
            conditions.push("listener_port = ?".into());
            params.push(port as i64);
        }
    }
}

#[test]
fn build_where_clause_supports_listener_port_filter() {
    let q = TrafficQuery { listener_port: Some(50831), ..Default::default() };
    let (where_clause, _params) = q.build_where_clause_for_test();
    assert!(where_clause.contains("listener_port = ?"));
}
```

`crates/bifrost-admin/src/openapi.rs:452`：

```json
{"name": "listener_port", "in": "query", "schema": {"type": "integer"}, "description": "Proxy port filter"}
```

### Fuzzy Search Engine

`crates/bifrost-admin/src/search/engine.rs`：

- 解析 `filters.conditions` 阶段：`"listener_port" | "port"` + `operator == "equals"` 时把值 parse 成 `u16` 并写入 `params.listener_port`（`engine.rs:490`）。
- Compact 校验阶段：条件字段 `"listener_port" | "port"` 从 compact record 的 `lp` 字段读取字符串值（`engine.rs:660`），执行 `contains / regex / not_contains / equals / is_empty / is_not_empty`。

### CLI 侧

`crates/bifrost-cli/src/cli/remote.rs`：

```rust
#[arg(long = "listener-port", visible_alias = "proxy-port", value_name = "PORT")]
pub listener_port: Option<u16>,
```

同时在 `traffic list / traffic search / remote traffic list / remote traffic search` 四处结构体中定义。

`crates/bifrost-cli/src/main.rs` 把 `listener_port` 映射到 `SearchOptions.filter_listener_port` / `TrafficListOptions.filter_listener_port`。

`crates/bifrost-cli/src/commands/remote.rs`：

```rust
if let Some(listener_port) = args.listener_port {
    conditions.push(SearchCondition {
        field: "listener_port".into(),
        operator: "equals".into(),
        value: listener_port.to_string(),
    });
}
```

单元测试位于 `commands/remote.rs:7560+`：`condition.field == "listener_port"` 断言，JSON 反序列化断言 `listener_port` u64 值等。

### Web 侧

`web/src/components/FilterBar/index.tsx`：

- 主字段下拉新增 `Port` 项，内部 key 为 `listener_port`。
- 选中后默认 `Equals`，展示端口输入框（`number` 或严格字符串）。
- 提交 filter 时以 `{ field: "listener_port", operator, value }` 加入 `filterConditions`。

`web/src/stores/useTrafficStore.ts`：

- 本地过滤 `applyFilters` 里把 `record.listener_port.toString()` 参与 `contains / equals / regex / not_contains / is_empty / is_not_empty`。

`web/src/components/TrafficDetail/panes/Overview/index.tsx`：

- 详情 Overview 展示 `Proxy Port: <listener_port>`。

`web/src/types/index.ts`：

- `TrafficSummary` 类型新增 `listener_port: number`。

## CLI + Web + Admin API

### CLI

```bash
# 本地
bifrost traffic list --listener-port 18888
bifrost traffic search "" --host api.example.com --listener-port 18888
bifrost search "" --listener-port 18888 --json

# 别名等价
bifrost traffic list --proxy-port 18888

# 远端
bifrost remote traffic list --listener-port 18888
bifrost remote traffic search "" --listener-port 18888
```

### Web

- FilterBar → `+ Add Filter` → 选择 `Port` → 输入端口 → 列表只展示匹配记录。
- Fuzzy Search 里在显式条件面板中添加 `listener_port equals 18888`。
- Traffic 详情：Overview 展示 `Proxy Port`。

### Admin API

- `GET /_bifrost/api/traffic?listener_port=<port>&limit=...&direction=...&cursor=...`
- `POST /_bifrost/api/traffic/search`：`filters.conditions` 支持 `{"field": "listener_port", "operator": "equals", "value": "<port>"}`。

## Sync 边界

- 端口筛选不涉及规则、Group、设备同步。
- Push 通道透传 `listener_port` 字段，前端本地过滤即可，不新增 Push 字段。
- 远端调用（bifrost remote）通过既有 remote invoke 通道，`--listener-port` 参数镜像本地语义。

## Phase 1：Admin API + 存储

- `TrafficQuery` 新增 `listener_port`，`build_where_clause` 生成 `listener_port = ?`。
- OpenAPI 描述新增 query 参数。
- 单元测试 `build_where_clause_supports_listener_port_filter` 覆盖。

## Phase 2：Fuzzy Search Engine

- `search/engine.rs` 解析 `listener_port | port` + `equals` 下推到 `params.listener_port`。
- Compact 层用 `lp` 字段处理其它操作符。
- 单元测试覆盖 equals 与 compact 校验路径。

## Phase 3：CLI（本地 + 远端）

- `--listener-port` + `--proxy-port` 别名。
- `traffic list / search / 顶层 search / remote traffic list / search` 五处入口。
- `cli_help.rs` 断言 help 中出现 `--listener-port / --proxy-port`。
- 请求体单元测试覆盖 `condition.field == "listener_port"` 与 u16 解析。

## Phase 4：Web + human_tests

- FilterBar `Port` 主筛选器 + Traffic 详情 `Proxy Port`。
- `web/tests/ui/traffic.spec.ts` 端口筛选与详情展示。
- `human_tests/webui-traffic.md` 与 `human_tests/readme.md` 补端口筛选用例。
- `e2e-tests/tests/test_temporary_port_bindings.sh` 追加 `traffic list/search --listener-port` 断言。

## 测试方案

### 单元测试

- `bifrost-admin::traffic_db::query::tests::build_where_clause_supports_listener_port_filter`（已存在，`crates/bifrost-admin/src/traffic_db/query.rs:317`）
- `bifrost-admin::search::engine`：`filters.conditions` 中 `listener_port equals` 下推。
- `bifrost-cli`：
  - `traffic_list_help_lists_listener_port_filter`（`cli_help.rs:184`）
  - `traffic_search_help_lists_listener_port_filter`（`cli_help.rs:201`）
  - `remote traffic list / search` 请求体断言（`commands/remote.rs:7560+`）
  - `listener_port_alias_proxy_port_equivalent`

### E2E

- `e2e-tests/tests/test_temporary_port_bindings.sh`：
  - `bifrost traffic list --port <MAIN> --listener-port <TEMP>` 只返回临时端口记录（脚本 `:494`）
  - `bifrost traffic search "port" --listener-port <TEMP>` 只返回临时端口记录（`:501`）
- `/traffic?listener_port=<TEMP>` API 断言只返回目标端口。
- Fuzzy Search `filters.conditions` 含 `listener_port equals <TEMP>` 只返回目标端口。

### Web UI E2E（Playwright）

`web/tests/ui/traffic.spec.ts`：

- `主筛选器支持按代理端口过滤 Traffic`（已实现覆盖，见 `:369`+）：
  - 通过 `updateTrafficListenerPort` 修改数据库中记录的 `listener_port`。
  - 打开 FilterBar 添加 `Port` 主筛选，默认 `Equals`。
  - 断言列表只保留匹配记录。
  - 断言详情页 `Proxy Port` 展示（`:529`）。

### 人工验证 human_tests

- `human_tests/webui-traffic.md`：TC-TPF-01 端口主筛选、TC-TPF-02 端口详情展示。
- `human_tests/readme.md`：追加索引。
- `human_tests/temporary-port-rule-bindings.md`：交叉引用 `--listener-port` 用例。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：Web / CLI（本地 + 远端）/ Admin API / Fuzzy Search 四路端口筛选统一。
- 复核 diff：`TrafficQuery.listener_port`、`search/engine.rs` 两处新增、CLI 五处入口、Web FilterBar + Overview。
- 复核语义：`--port` 与 `--listener-port` 不冲突；`--proxy-port` 只作为可见别名不重复注册。
- 复测：单元 + Shell E2E + Playwright。

### 第 2 轮

- 复核第 1 轮修复：CLI help 文案、错误场景（非法端口值）、compact `lp` 字段回退分支。
- 检查 Traffic 详情展示与 API 响应体一致；SSE / WS 请求详情也显示 `Proxy Port`。
- 复测：`e2e-tests/tests/test_temporary_port_bindings.sh` + `test_search_traffic_cli_isomorphic_e2e.sh` + Playwright。

## 风险与决策

- **决策**：新增独立参数 `--listener-port`，不复用 `--port`，避免语义冲突。`--proxy-port` 作为可见别名兼顾直觉。
- **决策**：Admin API 只支持 `equals` 下推，其它操作符走 compact 层；避免生成复杂 WHERE。
- **风险**：Windows / macOS / Linux 上 CLI 参数名字典序对帮助文本影响；缓解：`cli_help.rs` 显式断言两个可见名都出现。
- **风险**：Playwright 修改 traffic DB 需要 python3；缓解：`updateTrafficListenerPort` 已封装并在 CI 环境预置 python3。
- **风险**：Fuzzy Search compact 校验路径读取 `lp` 时值类型不一致（字符串 vs 数字）。缓解：`engine.rs:660` 强制 `compact.lp.to_string()`，测试覆盖。

## 依赖文件

- `web/src/components/FilterBar/index.tsx`
- `web/src/components/TrafficDetail/panes/Overview/index.tsx`
- `web/src/stores/useTrafficStore.ts`
- `web/src/types/index.ts`
- `web/tests/ui/traffic.spec.ts`
- `crates/bifrost-admin/src/traffic_db/query.rs`
- `crates/bifrost-admin/src/traffic_db/store.rs`
- `crates/bifrost-admin/src/traffic_db/schema.rs`
- `crates/bifrost-admin/src/traffic_db/types.rs`
- `crates/bifrost-admin/src/handlers/traffic.rs`
- `crates/bifrost-admin/src/search/engine.rs`
- `crates/bifrost-admin/src/openapi.rs`
- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/cli/remote.rs`
- `crates/bifrost-cli/src/commands/traffic.rs`
- `crates/bifrost-cli/src/commands/search.rs`
- `crates/bifrost-cli/src/commands/remote.rs`
- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-cli/tests/cli_help.rs`
- `crates/bifrost-command/src/lib.rs`
- `e2e-tests/tests/test_temporary_port_bindings.sh`
- `human_tests/webui-traffic.md`
- `human_tests/readme.md`

## 文档更新要求

- 本次改动属 WebUI + CLI Traffic 筛选能力补充，不涉及公开 README 协议 / 命令变更。
- 必须同步更新 `human_tests/webui-traffic.md` 与 `human_tests/readme.md`。
- CLI help 与 README 中 `traffic search / traffic list / search / remote traffic *` 段落需列出 `--listener-port` 与 `--proxy-port` 别名。
