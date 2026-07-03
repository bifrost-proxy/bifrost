# Traffic Fuzzy Search Filter Isolation 设计方案

## 背景

Bifrost WebUI Traffic 页支持两种筛选路径：

- **列表筛选**：`FilterBar` 的 `Add Filter` + 左侧 Filter Panel + Toolbar 输入，直接在前端 store `records` 上过滤，不去后端二次查询。
- **Fuzzy Search**：`SearchMode` 组件通过 `POST /api/traffic/search` 请求后端全文 / 精细搜索，覆盖 headers / body 深层字段。

两条路径的语义并不一致：

- 列表筛选 = 我想暂时看少一点，条件随手切换。
- Fuzzy Search = 我要在全库中找一批目标请求，条件应稳定且显式。

历史实现里，`SearchMode` 的 `buildFilters()` 把 `FilterBar` 的 `filterConditions` 也写进搜索请求的 `filters.conditions`。这会造成：

1. 用户在列表页做了一个临时 filter，切到 Fuzzy Search 时搜索被误缩小，结果与全库不一致。
2. 用户经常“搜不到明明存在的请求”，实际是被列表 filter 拦住了。
3. 关闭 filter 会重新触发一次 Fuzzy Search，形成非预期请求风暴。

本方案把两路语义拆开：

- Fuzzy Search 只受 **关键词 + scope + 显式筛选面板（Toolbar / Filter Panel）** 影响。
- 列表侧的 `Add Filter` `filterConditions` 不再进入 Fuzzy Search 请求。

同时 CLI 侧补齐等价能力：`bifrost search` / `bifrost traffic search` 补 `--host / --path / --listener-port` 等条件参数，修正 `--method` 未真正下发的历史 bug，并新增细粒度 scope 参数（`--url / --req-header / --res-header / --req-body / --res-body`），保留 `--headers / --body` 兼容别名。

> 实现状态（2026-07-03 复核）：CLI 补充参数已在 `crates/bifrost-cli/src/commands/search.rs`（`filter_host / filter_path / filter_listener_port`）与 `main.rs`（默认值）落地；Web 端 `web/src/components/SearchMode/index.tsx` 目前 `buildFilters` 仍写入 `filterConditions`，Fuzzy Search 与列表 filter 解耦仍处于 planned, not yet shipped 状态，需按本方案实施。

## 用户目标验证清单

### 必须实现

- Web：Fuzzy Search 构造的搜索请求不再包含 `FilterBar` 的 `filterConditions`。
- Web：Fuzzy Search 仍消费关键词、scope 与显式筛选面板（Toolbar + 左侧 Filter Panel）。
- Web：切换 `Add Filter` 不再触发新的 Fuzzy Search 请求。
- CLI：`bifrost search` / `bifrost traffic search` 支持 `--host / --path / --method / --listener-port` 并真正下发到 `filters.conditions`。
- CLI：`--url / --req-header / --res-header / --req-body / --res-body` 可独立限定搜索范围；`--headers` / `--body` 作为双侧兼容别名。
- CLI：细粒度 scope 参数任一被设置时下发 `scope.all = false`，只搜请求的目标范围。

### 必须不破坏

- Fuzzy Search 关键词、`status / domain / protocol / content_type` 过滤保持既有行为。
- 列表筛选（Toolbar + Add Filter + Filter Panel）继续在前端 `records` 上工作，不因本次改动去后端查询。
- CLI 原有 `--headers / --body / --status / --domain / --protocol / --content-type / --format` 用法保持兼容。
- Remote 调用（`bifrost remote traffic search`）与本地行为等价。

### 必须真实验证

- Web UI E2E：设置一个不命中任何记录的 `Add Filter` → 切到 Fuzzy Search → 搜索已存在请求 → 断言 `filters.conditions` 为空 → 断言仍返回目标请求。
- CLI 单测：请求体组装校验 `method / host / path / listener_port` 都进入 `filters.conditions`。
- CLI 单测：`--headers` 同时启用请求 / 响应两侧 scope；`--req-header` 只启用请求侧。
- Remote CLI：`bifrost remote traffic search --host X --listener-port Y` 得到与本地 CLI 等价结果。

## 产品语义

### 两路筛选的边界

| 路径 | 输入 | 数据源 | 何时触发新请求 |
| --- | --- | --- | --- |
| 列表筛选 | Toolbar / Filter Panel / Add Filter | 前端 store `records` | 不触发后端；本地过滤 |
| Fuzzy Search | 关键词 + scope + Toolbar / Filter Panel | 后端 `/traffic/search` | 关键词或 scope 或显式筛选面板变化 |

**Add Filter 不再影响 Fuzzy Search**。用户如果想在 Fuzzy Search 里加限制，请使用 scope 或 Toolbar / Filter Panel。

### Fuzzy Search Scope 精细维度

后端 `scope` 支持四个开关：`request_headers` / `response_headers` / `request_body` / `response_body`。当 `scope.all = true` 时不区分四路，全部检索；当 `all = false` 时按开关组合精确检索。

- 只想搜请求头：`--req-header` → `scope = { request_headers: true }`
- 只想搜响应体：`--res-body`
- 兼容语义：`--headers` = `request_headers + response_headers`；`--body` = `request_body + response_body`。

## 技术细节

### Web：`SearchMode.buildFilters` 改造

`web/src/components/SearchMode/index.tsx`：

```ts
const buildFilters = useCallback((): SearchFilters => {
  return {
    keyword,
    scope,
    // 保留：来自 Toolbar / 左侧面板的显式条件
    panel_conditions: panelConditions,
    // 移除：来自 FilterBar 的临时列表过滤
    // conditions: filterConditions,   ❌ 不再写入
  };
}, [keyword, scope, panelConditions]);
```

- 依赖数组不再包含 `filterConditions`，切换 `Add Filter` 不会触发新的搜索。
- 仅当 `keyword / scope / panelConditions` 变化时才重跑 `search(buildFilters())`。
- 保留 `filterConditions.some(isFilterConditionApplicable)` 用于本地列表过滤，不再进入搜索请求。

### CLI：`SearchOptions` 增补字段

`crates/bifrost-cli/src/commands/search.rs`：

```rust
pub struct SearchOptions {
    // 现有：keyword, status, domain, protocol, content_type, format ...
    pub filter_host: Option<String>,        // --host
    pub filter_path: Option<String>,        // --path
    pub filter_method: Option<String>,      // --method（修正下发）
    pub filter_listener_port: Option<u16>,  // --listener-port / --proxy-port

    // scope 精细维度
    pub scope_url: bool,
    pub scope_req_header: bool,
    pub scope_res_header: bool,
    pub scope_req_body: bool,
    pub scope_res_body: bool,
    // 兼容别名
    pub scope_headers_compat: bool,  // --headers → req+res headers
    pub scope_body_compat: bool,     // --body → req+res body
}
```

构造请求体：

```rust
if let Some(ref host) = options.filter_host {
    conditions.push(FilterCondition {
        field: "host".into(),
        operator: "contains".into(),
        value: host.clone(),
    });
}
if let Some(ref path) = options.filter_path { /* contains */ }
if let Some(ref method) = options.filter_method { /* equals */ }
if let Some(port) = options.filter_listener_port {
    conditions.push(FilterCondition {
        field: "listener_port".into(),
        operator: "equals".into(),
        value: port.to_string(),
    });
}

// scope：任一细粒度被设置就把 scope.all 关掉
let mut scope = SearchScope::default_all();
if any_scope_flag_set(&options) {
    scope.all = false;
    scope.request_headers = options.scope_req_header || options.scope_headers_compat;
    scope.response_headers = options.scope_res_header || options.scope_headers_compat;
    scope.request_body = options.scope_req_body || options.scope_body_compat;
    scope.response_body = options.scope_res_body || options.scope_body_compat;
    scope.url = options.scope_url;
}
```

### Remote CLI：等价参数

`crates/bifrost-cli/src/cli/remote.rs` 的 `remote traffic search` 与本地 `traffic search` 保持同名同别名（`--listener-port` / `--proxy-port`），构造请求前把 `listener_port` 打到 `SearchCondition{ field: "listener_port", operator: "equals", value }` 并交给远端 `bifrost-admin` 服务执行（见 `crates/bifrost-cli/src/commands/remote.rs`）。

## CLI + Web + Admin API

### CLI

- `bifrost search "<keyword>" [--host ...] [--path ...] [--method ...] [--listener-port ...] [--url] [--req-header] [--res-header] [--req-body] [--res-body] [--headers] [--body] [--json]`
- `bifrost traffic search ...` 与顶层同参。
- `bifrost remote traffic search ...` 参数镜像本地。

### Web

- `SearchMode` 组件搜索请求 payload：`{ keyword, scope, panel_conditions }`。
- FilterBar 的 `Add Filter` 只作用于列表本地过滤。

### Admin API

`POST /_bifrost/api/traffic/search`：

- `filters.conditions` 支持字段 `host / path / method / listener_port / status / domain / protocol / content_type`。
- `scope = { all: bool, url: bool, request_headers: bool, response_headers: bool, request_body: bool, response_body: bool }`。
- `all: true` 时忽略其它 scope 字段。
- `listener_port` `equals` 操作符会下推到 SQL WHERE，其它操作符走前端 / compact 层。

## Sync 边界

- 本能力仅影响“搜索请求组装 + CLI 参数解析”，不涉及规则、Group、设备同步。
- Push 通道不参与 Fuzzy Search；Fuzzy Search 只走 HTTP 请求。

## Phase 1：Web 解耦

- `SearchMode.buildFilters` 移除 `filterConditions` 写入。
- 依赖数组同步收敛，避免副作用。
- 添加 UI E2E 覆盖“Add Filter 不影响搜索”。

## Phase 2：CLI 补齐条件参数

- `SearchOptions` 增加 `filter_host / filter_path / filter_listener_port`。
- 修正 `--method` 真正下发。
- `traffic search` 与顶层 `search` 共用同一构造。
- CLI 单测覆盖请求体组装。

## Phase 3：CLI 补齐细粒度 Scope

- 增加 `--url / --req-header / --res-header / --req-body / --res-body`。
- `--headers / --body` 保留为双侧兼容别名。
- 任一开启时下发 `scope.all = false`。

## Phase 4：Remote 镜像 + 文档

- `remote traffic search` 参数与本地对齐。
- 更新 `README.md` CLI 示例、CLI help 中 `search` 段。
- 更新 `human_tests/cli-traffic-search.md`。

## 测试方案

### 单元测试

- `crates/bifrost-cli`：
  - `build_search_body_includes_host_condition`
  - `build_search_body_includes_path_condition`
  - `build_search_body_includes_method_condition`
  - `build_search_body_includes_listener_port_condition`
  - `scope_headers_compat_alias_enables_request_and_response_headers`
  - `scope_req_body_only_enables_request_body_scope`
  - `scope_all_true_when_no_flag_set`
- `crates/bifrost-cli/tests/cli_help.rs`：
  - `traffic_search_help_lists_listener_port_filter`（已存在，需保留通过）
  - `search_help_lists_scope_fine_grained_flags`

### E2E

- `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`：CLI 与远程等价校验，含 `--host / --path / --listener-port` 场景。
- `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`：远端 remote 场景等价校验。
- `e2e-tests/tests/test_temporary_port_bindings.sh`：`traffic search --listener-port` 只返回目标端口记录（已覆盖）。

### Web UI E2E

`web/tests/ui/traffic.spec.ts` 追加：

- `fuzzy search ignores add-filter list conditions` — 设置一个不命中记录的 Add Filter，Fuzzy Search 仍能返回目标；断言请求 payload 中 `filters.conditions` 为空。
- `fuzzy search reruns only on keyword or scope change` — 切换 Add Filter 后没有新的 `/traffic/search` 请求。

### 人工验证 human_tests

- `human_tests/cli-traffic-search.md` 更新：`--host / --path / --method / --listener-port` 用例与 scope 精细验证。
- `human_tests/webui-traffic.md` 更新：Fuzzy Search 与 Add Filter 隔离用例。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：Web 解耦 + CLI 条件参数 + 细粒度 scope 三块都完成。
- 复核 diff：`SearchMode` 依赖数组、`SearchOptions` 字段、CLI help、`remote traffic search`。
- 复核语义：`--headers` / `--body` 兼容语义未变；只要任一细粒度开则 `all = false`。
- 复测：CLI 单元 + isomorphic E2E + Playwright。

### 第 2 轮

- 复核第 1 轮修复：Web 端切换 Add Filter 不再触发额外搜索请求；CLI 帮助文本更新。
- 检查错误路径：`--method GET --host api` 组合下 `filters.conditions` 顺序与后端 SQL 兼容。
- 复测：完整 CLI + Web + human_tests 复跑。

## 风险与决策

- **决策**：Fuzzy Search 只接受“显式筛选面板”条件，不接受 Add Filter 的临时列表过滤。这样避免用户误以为“看不到 = 没搜到”。
- **风险**：既有用户可能已经依赖“Add Filter 收窄 Fuzzy Search”的行为。缓解：在 Search 结果侧展示当前生效的显式筛选条件，让用户明确知道搜索范围。
- **决策**：CLI `--headers / --body` 保留为兼容别名，避免既有脚本报错。
- **决策**：`--listener-port` / `--proxy-port` 用可见别名，避免与 `traffic list --port`（Admin API 端口）冲突。
- **风险**：Remote CLI 与本地 CLI 参数漂移。缓解：`cli_help.rs` 覆盖两侧同参断言，isomorphic E2E 双通验证。

## 依赖文件

- `web/src/components/SearchMode/index.tsx`
- `web/src/stores/useSearchStore.ts`
- `web/src/stores/useTrafficStore.ts`
- `web/tests/ui/traffic.spec.ts`
- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/cli/remote.rs`
- `crates/bifrost-cli/src/commands/search.rs`
- `crates/bifrost-cli/src/commands/remote.rs`
- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-admin/src/search/types.rs`
- `crates/bifrost-admin/src/search/engine.rs`
- `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`
- `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `human_tests/cli-traffic-search.md`
- `human_tests/webui-traffic.md`

## 文档更新要求

- 更新 `README.md` 中 CLI 搜索示例，补充 `--host / --path / --method / --listener-port` 与细粒度 scope 参数。
- 更新 CLI help 中 `search` 与 `traffic search` 的参数说明。
- 更新 `human_tests/cli-traffic-search.md`：条件过滤 + scope 精细验证。
- 更新 `human_tests/webui-traffic.md`：Fuzzy Search 与 Add Filter 隔离用例。
