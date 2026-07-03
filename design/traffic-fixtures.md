# Traffic Fixtures 产品方案

## 背景

Bifrost 当前已经具备较强的流量录制与回放基础能力：

- `Traffic` 完整记录请求 / 响应信息（`client_app`、`client_path`、`host`、`path`、headers、body、状态码、SSE / WS 状态等）。
- `Replay` 能把流量导入为请求模板并再次执行，支持 group、history 三张核心表。
- `Rules` 能按域名、路径、方法、header、body、状态码做过滤与改写。

但从“测试稳定性 / 接口虚拟化”角度看，仍存在明显缺口：

1. `Replay` 更像“手工再次发请求”，不是“真实请求经过代理时自动命中历史录制结果”。
2. `Rules` 能做静态 mock，但配置偏工程规则，缺“围绕真实录制流量生成的回放夹具”产品抽象。
3. 缺少面向测试的命中策略配置：哪些应用生效、哪些域名 / 路径生效、query / header / body 哪些字段参与匹配、未命中时应报错 / 透传 / 补录。
4. 缺少命中解释与调试视图，测试失败时不易判断是“没录到”还是“匹配条件过严”。

因此本方案设计新产品能力 **Traffic Fixtures**，面向测试稳定性提供“基于真实流量的自动命中回放系统”。

> 实现状态（2026-07-03 复核）：`Traffic Fixtures` **尚未在仓库中落地**。仓库中未发现 `bifrost fixtures` CLI 子命令、`/_bifrost/api/fixtures/...` 管理 API、`FixtureEngine` 运行时模块以及 `fixture_sets / fixtures / fixture_hit_history` 存储对象。文中提到的 `Traffic` / `Replay` / `Rules` 基础能力已具备（`replay_db_store`、Traffic `client_app` / `listener_port` 字段、SSE frame store 等）。本文件按“产品定稿 + 交付计划”维护，落地后按此文件真实实现路径回填。

## 用户目标验证清单

### 必须实现（V1）

- 用户可通过 Web `Fixtures` 页面创建、编辑、启停 fixture set。
- 用户可从 `Traffic` 单条 / 批量导入 HTTP / HTTPS / SSE 流量为 fixture。
- 支持 `Scope = app + host + path + method` 粗筛。
- 支持 `Match Policy = query + header + path param` 精筛，query / header 支持 include / ignore / exists。
- 支持 `Replay Policy`：`on_miss ∈ {fail, passthrough, passthrough_and_record}`；`response_mode = latest`；`rule_mode ∈ {apply_rules, bypass_rules}`；受控降级 `allow_fallback + dev_only`。
- SSE fixture：返回 `text/event-stream`，按录制事件序列 flush，支持 `keep_open` / `close_after_last_event`、`preserve` 心跳。
- 命中 fixture 后仍进入 `Traffic` 展示链路并标注命中的 fixture set / item / rule_mode。
- CLI 独立 `bifrost fixtures` 顶层命令，覆盖 set / item / import / export / hit / debug / run。
- 命中、未命中、冲突、受控降级均可在 UI 与 CLI 中查询。
- fixture 存储独立于 traffic，不受 traffic 数据库清理、文件清理、保留策略影响。

### 必须不破坏

- `Traffic` 采集、Push、Replay 手工执行、Rules 匹配 / 改写、Group 规则、Sync 均按现有语义工作。
- 未启用 fixture set 时代理链路零开销、零行为变化。
- fixture 命中后的响应仍作为一次代理事件写入 traffic，不制造“隐形通道”。
- CLI `traffic list --port` 表示 Admin API 端口，`fixtures` 命令必须使用 `--set / --item` 等新语义，不与 traffic 命令混用。

### 必须真实验证

- HTTP fixture：真实浏览器 / curl 通过代理请求命中 fixture 返回录制响应；未命中默认失败。
- SSE fixture：真实客户端持续读取事件序列；`keep_open` 生效；心跳保留。
- 存储独立性：将 traffic 清理策略触发（record 数量或文件保留期），验证 fixture 仍可命中。
- CLI 全流程：`import traffic → set enable → debug traffic → hit list → set disable` 无 Web UI 即可完成。

## 产品语义

### 产品定位

- 产品名：**Traffic Fixtures**。
- 定位：测试稳定性导向的自动命中回放系统。
- 与 Replay / Rules 边界：Replay 手工执行调试；Rules 通用改写；Fixtures 自动命中真实录制响应，服务于稳定测试。

### 概念模型

- `Fixture Set`：顶层容器，一个测试场景（例：`nextoncall-login-stable`）。
- `Fixture`：一条可命中的录制条目，来源于真实 traffic 记录。
- `Scope`：粗筛（app + host + path + method）。
- `Match Policy`：精筛（query + header + path param）。
- `Replay Policy`：命中后如何回放，未命中如何处理。
- `Stream Fixture`：SSE 事件序列 + timeline + 结束行为。

### Scope

维度固定：`client_apps / hosts / path_patterns / methods`。同维度 OR，跨维度 AND。V1 不含 body / query 参与 scope。

### Match Policy

- Query：`include` / `ignore` / `exists`，默认全忽略。
- Header：同上，默认忽略易变头，仅保留业务头。
- Path Param：精确匹配 / 模板变量化。
- Body：V1 不支持。

系统预置易变字段忽略名单：`timestamp / ts / trace_id / x-request-id / x-b3-traceid / nonce / uuid`。

### Replay Policy

| 配置项 | 取值 | 默认 |
| --- | --- | --- |
| `on_miss` | `fail` / `passthrough` / `passthrough_and_record` | `fail` |
| `response_mode` | `latest` / `ordered_sequence`（V2） | `latest` |
| `rule_mode` | `apply_rules` / `bypass_rules` | `apply_rules` |
| `allow_fallback` | bool | `false` |
| `fallback_scope` | `dev_only` | `dev_only` |
| `record_hit_history` | bool | `true` |
| `fixture_priority` | int | 0 |

受控降级 = `allow_fallback && dev_only`，每次降级记录为 `miss → fallback`。

### SSE Policy

| 配置项 | 取值 | 默认 |
| --- | --- | --- |
| `stream_mode` | `recorded_timeline` / `compressed_timeline` / `immediate_flush` | `recorded_timeline` |
| `end_behavior` | `keep_open` / `close_after_last_event` | `keep_open` |
| `first_event_timeout_ms` | int | `30000` |
| `heartbeat_mode` | `preserve` / `drop` / `synthesize` | `preserve` |

## 技术细节

### 命中时机

固定为：

1. 请求进入代理并完成 URL / method / headers / body 基础解析。
2. 进入规则匹配阶段，fixture rule 作为规则系统的一种规则类型参与执行。
3. 命中则由 fixture rule 构造响应；未命中按 `on_miss` 执行。
4. `rule_mode = apply_rules` 时该响应继续走规则链；`bypass_rules` 时直接返回。
5. 无论哪种模式，该次请求都必须写入 `Traffic` 并标注命中元信息。

### 冲突处理

1. 匹配维度更完整的条目优先。
2. `fixture_priority` 高者优先。
3. 更新时间更新者优先。
4. 仍冲突则返回明确错误，Debug 视图展示冲突候选。

### 存储设计（独立于 Traffic）

固定存储对象：

- `fixture_sets`
- `fixtures`
- `fixture_hit_history`
- `fixture_match_policies`
- `fixture_bodies`
- `fixture_streams`

导入行为：从 traffic 读元数据 + body + SSE 事件流做完整快照复制，写入 fixtures 专属存储。导入后 `traffic_id` 仅作来源追踪，不作为运行时读取主键。

验收：

- traffic record 被清理后 fixture 仍可命中。
- traffic body 文件保留期到期被清理后 fixture 仍可回放。
- fixture set 删除后其独占数据被释放。
- hit history 清理不影响 fixture 本体。

### 运行时模块

- `FixtureEngine::match(request_ctx) -> MatchResult`
- `FixtureEngine::replay(fixture_id) -> Response`（含 SSE stream 变体）
- `FixtureEngine::record_hit(hit_meta)`
- `FixtureEngine::explain_miss(request_ctx, set_id) -> MissExplanation`

## CLI + Web + Admin API

### Web UI

新增一级导航 `Fixtures`。页面包括：

- Fixture Sets 列表页：`[New Fixture Set]`、`[Import from Traffic]`、`[Only Active]`。
- Fixture Set 详情：Tabs = `Scope / Match Policy / Replay Policy / Fixtures / Hit History / Debug`。
- Fixture Inspector：查看单条请求 / 响应；SSE 视图展示事件时间线 `#idx +Δms event data`。
- Fixture Debug：给定 request 或 traffic id，输出候选 / 匹配字段 / 未命中字段。

现有页面入口：

- Traffic 页：单条 `保存为 Fixture`、多条 `批量生成 Fixture Set`、详情标注命中信息。
- Replay 页：`用当前请求调试 Fixture 命中`。

### CLI

顶层命令 `bifrost fixtures`，子树：

```text
bifrost fixtures
  set    { list | get | create | update | enable | disable | delete | clone }
  item   { list | get | add | update | delete }
  import { traffic | file }
  export { set | item }
  hit    { list | stats | clear }
  debug  { request | traffic <id> }
  run    { status | activate | deactivate | activate-only }
```

固定要求：

- 所有核心命令必须无交互可用（CI 场景）。
- 每个读操作支持 `--json / --yaml / table` 三种输出。
- SSE 专用：`item get --show-events --event-limit`、`item update --sse-end`、`set update --sse-mode`。

CLI 与配置文件联动：

- `bifrost fixtures set create --file fixtures.yaml`
- `bifrost fixtures set update <set> --file fixtures.yaml`
- `bifrost fixtures export set <set> --format yaml --output fixtures.yaml`

典型工作流：

```bash
# 生成
bifrost fixtures import traffic \
  --new-set login-stable \
  --host api.example.com --path /api/login --method POST \
  --infer-scope --infer-match-policy

# 严格模式
bifrost fixtures set update login-stable --miss fail
bifrost fixtures run activate-only login-stable

# 未命中排障
bifrost fixtures hit list --status miss --limit 10
bifrost fixtures debug traffic trf_18273 --set login-stable
```

### Admin API

管理 API：

- `GET /_bifrost/api/fixtures/sets`
- `POST /_bifrost/api/fixtures/sets`
- `GET /_bifrost/api/fixtures/sets/:id`
- `PUT /_bifrost/api/fixtures/sets/:id`
- `DELETE /_bifrost/api/fixtures/sets/:id`
- `POST /_bifrost/api/fixtures/import-from-traffic`
- `POST /_bifrost/api/fixtures/debug-match`
- `GET /_bifrost/api/fixtures/sets/:id/hits`

必须提供 CSRF 保护、Content-Type 严格 JSON、错误码稳定。

## Sync 边界

- **不进入远端 Sync**：fixture 数据是本地测试快照资产，跨设备语义模糊，V1 不参与 rule sync / group sync。
- 允许通过 `fixtures export / import file` 手工在设备间迁移。
- Traffic 采集侧的 Push / SSE 帧存储保持不变，fixture 命中请求走既有 Push delta 路径以在其他浏览器 tab 同步展示。

## Phase 1：存储与 Match Engine

- 新增 `fixture_*` schema、`FixtureRepo`、body / stream 独立存储路径。
- 实现 `FixtureEngine::match`（Scope + Match Policy）。
- 实现从 traffic 单条 / 批量导入的快照复制路径。
- 拦截 traffic cleanup / file retention，验证独立性。
- 单元测试：scope 匹配、include / ignore / exists、path param 模板、易变字段忽略名单。

## Phase 2：运行时命中 + Replay

- 将 fixture rule 接入规则匹配链，产出 `Response` / SSE stream。
- `on_miss` 三态、`rule_mode` 两态、受控降级路径。
- SSE：事件序列 flush、`keep_open` / `close_after_last_event`、`preserve` 心跳。
- Traffic 详情侧标记命中的 set / item / rule_mode / miss reason。

## Phase 3：Web UI + CLI

- Web：Fixtures 导航、列表、详情 tabs、Inspector、Debug 视图。
- Traffic / Replay 页嵌入入口。
- CLI 全套命令，YAML 配置文件互操作。
- `--json / --yaml / --quiet / --no-color`。

## Phase 4：可观测性 + 文档

- 观测指标：命中次数、未命中次数、冲突次数、最近命中 / 未命中样本、未命中原因分布。
- 更新 `human_tests/traffic-fixtures.md`、`README.md` CLI 段、`ADMIN_API.md`。
- 迁移边界：明确 V1 不支持 JSON body 匹配、ordered sequence、WebSocket 帧级回放。

## 测试方案

### 单元测试

- `fixtures::scope::match_app_host_path_method`
- `fixtures::match::query_include_ignore_exists`
- `fixtures::match::header_include_ignore_exists`
- `fixtures::match::path_param_template`
- `fixtures::replay::on_miss_fail_default`
- `fixtures::replay::rule_mode_apply_rules`
- `fixtures::replay::rule_mode_bypass_rules`
- `fixtures::replay::controlled_fallback_dev_only`
- `fixtures::sse::recorded_timeline_playback`
- `fixtures::sse::keep_open_after_last_event`
- `fixtures::sse::preserve_heartbeat`
- `fixtures::storage::independent_from_traffic_cleanup`
- `fixtures::storage::body_file_lifecycle_local_to_fixtures`

### E2E 测试

新增 `e2e-tests/tests/test_fixtures_e2e.sh`：

- 导入 traffic 生成 set → 通过代理请求命中 → 校验响应内容与 headers。
- `--miss fail` 时未命中请求返回 fixture miss 错误。
- 受控降级：`allow_fallback + dev_only` 触发真实下游并写 miss → fallback 记录。
- SSE：客户端 EventSource 读取事件序列，间隔 ≥ 录制间隔，`keep_open` 时连接不关闭。
- traffic 清理触发后 fixture 命中仍成功。

`e2e-tests/tests/test_fixtures_storage_independence_e2e.sh`：

- 导入 fixture → 手动删除 traffic body 目录 → 命中仍读到 fixture 自有 body。

### Web UI E2E（Playwright）

`web/tests/ui/fixtures.spec.ts`（新增）：

- Fixtures 页创建 → 编辑 Scope / Match / Replay → 保存后详情反显。
- Traffic 页 `Save as Fixture` 触发单条导入并跳转。
- Debug 视图给定 traffic id → 显示 matched / mismatched 字段。
- 列表 `Only Active` 开关只留 active set。

### 人工验证 human_tests

`human_tests/traffic-fixtures.md`（新增）：

- TC-TFX-01 从 traffic 导入 → 命中 → 详情标注命中 set / item。
- TC-TFX-02 SSE 命中：事件按录制间隔回放。
- TC-TFX-03 未命中默认失败；打开受控降级后回落真实下游。
- TC-TFX-04 冲突：多 fixture 命中同一请求时按优先级决胜并给 Debug 报告。
- TC-TFX-05 traffic 清理后 fixture 仍可命中。
- TC-TFX-06 CLI-only 全流程（无 Web）：import → activate-only → debug → hit list。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：Scope / Match / Replay / SSE / 存储独立 / CLI+Web+API 全覆盖。
- 复核 diff：`fixture_*` schema、`FixtureEngine`、规则链接入点、Traffic 展示层、Fixtures 导航、CLI。
- 复核边界：fixture 命中是否绕过 traffic 记录 / 是否与 rules 双写 / 是否与 Sync 混淆。
- 复测：单元 + E2E + Playwright。

### 第 2 轮

- 复核第 1 轮修复：`on_miss`、`rule_mode`、SSE `keep_open` 边界。
- 检查存储独立性回归：traffic cleanup / body retention / DB vacuum 之后 fixture 仍活。
- 检查 CLI 与 Web 等价：CLI 能完成所有 UI 能力，反之亦然。
- 复测：全部 E2E + human_tests 复跑。

## 风险与决策

- **误匹配风险**：默认字段过少导致跨业务误命中。缓解：创建时展示“候选冲突数”，Match Policy include 优先，Debug 视图给出候选。
- **匹配过严风险**：动态字段被纳入匹配导致频繁 miss。缓解：预置易变字段忽略名单 + 基于 miss 历史的忽略字段推荐。
- **与 Rules 语义冲突**：fixture / rule 双写响应容易乱。缓解：默认 `rule_mode = apply_rules`，UI 明示交互模式，set 级别可切换 `bypass_rules`。
- **长连接复杂度**：SSE / WS 不是简单的 “返回一个 body”。缓解：V1 只做 SSE 事件序列回放，WebSocket 帧级留 V3。
- **存储膨胀**：fixture 独立复制 body 会占空间。缓解：fixture 只受用户显式删除影响；提供 `set delete` / `item delete` / `hit clear` 三个明确清理入口。
- **决策**：fixture V1 不参与远端 Sync；跨设备通过 export/import 手工迁移。
- **决策**：fixture 命中必须在 traffic 中可见，禁止“隐形回放通道”。
- **决策**：CLI 与 UI 是等价入口，不做 CLI-only 或 UI-only 能力。

## V1 / V2 / V3 交付切分

### V1（本次交付范围）

- HTTP / HTTPS + SSE
- Scope = app + host + path + method
- Match = query + header + path param
- Replay：`on_miss ∈ {fail, passthrough, passthrough_and_record}`、`response_mode = latest`、`rule_mode ∈ {apply_rules, bypass_rules}`
- SSE：`recorded_timeline` + `keep_open` + `preserve heartbeat`
- 完整 CLI + Web + Admin API + hit stats + Debug 视图

### V2

- JSON body 字段匹配
- `ordered_sequence` 响应序列
- 事件级选择性忽略字段 / timeline 压缩 / 事件断言视图
- 更完整字段忽略模板

### V3

- WebSocket session fixture
- 数据脱敏 / 参数化
- 分享 / 导出导入包治理

## 依赖文件（V1 落地时）

- `crates/bifrost-admin/src/handlers/fixtures.rs`（新增）
- `crates/bifrost-admin/src/fixture_engine/`（新增）
- `crates/bifrost-admin/src/fixture_db/`（新增）
- `crates/bifrost-cli/src/commands/fixtures.rs`（新增）
- `crates/bifrost-cli/src/cli/fixtures.rs`（新增）
- `web/src/pages/Fixtures/`（新增）
- `web/src/components/FixtureInspector/`（新增）
- `web/tests/ui/fixtures.spec.ts`（新增）
- `e2e-tests/tests/test_fixtures_e2e.sh`（新增）
- `human_tests/traffic-fixtures.md`（新增）
- `human_tests/readme.md`（追加索引）

## 文档更新要求

- 本文件为产品定稿 + V1 交付切分，代码落地后按真实实现回填模块名 / API 路径 / 测试文件。
- 落地后必须同步更新：`README.md` CLI 段、`ADMIN_API.md`、`human_tests/traffic-fixtures.md`、`human_tests/readme.md`。
- 每个 Phase 结束都需回补测试 ID 与 CLI help 断言。
