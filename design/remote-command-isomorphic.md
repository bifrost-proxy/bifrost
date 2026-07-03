# Remote 命令同构化执行方案

> 实施进展（截至 2026-07-03）：
> - Phase 1 已落地：共享 crate `crates/bifrost-command`（`crates/bifrost-command/src/lib.rs` 存在，`CanonicalQueryCommand` 于第 250 行定义）与 `AdminQueryService`（`crates/bifrost-admin/src/query_service.rs` 第 31 行定义）已作为唯一查询内核，`bifrost-admin` 的 HTTP `search/traffic` handler 与 remote executor（当 `query_service` 注入时）均直接调用它。
> - Allowlist 收敛：`ALLOWED_COMMANDS = ["status", "search.stream", "traffic.list", "traffic.get"]`（见 `crates/bifrost-admin/src/remote_invoke/types.rs:1511`），`traffic.search` / `search.get` 等 legacy 别名已不在白名单中。
> - `RemoteCommand` wire 结构（`types.rs:404-412`）仍同时保留 legacy `command: String + args_json: Option<String>` 字段与新增的 `query: Option<CanonicalQueryCommand>` 字段——两条协议路径并存。
> - Phase 2-5 仍在推进：CLI 仍以 HTTP（本地 admin）作为传输、`CommandEvent` 结构化事件流、`QueryRunner` / `QueryBackend` trait、`SearchRenderOptions` / `TrafficRenderOptions` 拆分以及统一 `CommandDescriptor` 结构体目前均为 (planned, not yet shipped as of 2026-07-03)，executor 仍保留 legacy HTTP fallback 与文本渲染分支。

## 背景

当前 `bifrost remote search` / `bifrost remote traffic` 与本地 `bifrost search` / `bifrost traffic` 已经出现明显漂移，问题不只体现在少几个 flag，而是 remote 侧从架构上走成了第二套命令实现：

- 本地链路：`clap -> typed options -> 本地命令实现 -> 本地格式化输出`
- remote 链路：`clap -> 远程专用字符串命令 + args_json -> remote executor -> 远程专用格式化输出`

这导致：

1. 每新增一个查询命令或过滤条件，都要维护本地和 remote 两套实现。
2. allowlist 白名单绑定的是 remote 专用字符串命令（历史值：`search.get` / `traffic.search`），而不是主命令体系。
3. 远端执行结果是格式化后的文本，调用端无法复用本地 formatter。
4. `search.get`、`traffic.search` 这类 remote 内部命名与 CLI 主命令语义不一致。
5. `remote search`、`remote traffic search` 只能靠持续补丁追赶本地能力，无法从机制上消除漂移。

## 用户目标验证清单

### 必须实现

- 本地 `bifrost search` 与远程 `bifrost remote traffic search` 共享同一个 canonical 查询命令模型，添加新 filter 只需要改一处。
- Allowlist 基于 canonical command id / capability 判断，不再基于 remote 专用字符串。
- `AdminQueryService` 是唯一的查询执行内核，HTTP handler、remote executor、以及未来的本地直连都通过它执行。
- Remote executor 返回结构化事件（当前仍为文本渲染，目标状态为 typed events），调用端复用本地 formatter。
- Remote invoke 保持 “emit 即推送” 的流式语义，查询命令不能倒退为一次性返回。
- Legacy `command + args_json` 查询协议在最终 Phase 完成后被删除，`RemoteCommand` 只保留 typed `query` 字段。

### 必须不破坏

- `status` 命令作为特殊命令，保持现状协议、执行路径、渲染语义，不进入本次同构化。
- `shell.exec` 及其它非 `search/traffic` 命令的行为、鉴权、白名单不受影响。
- `relay` / `grant` / `frame` / `exit` 总体骨架保持不变。
- 本地 CLI 的交互式选择器与 TTY 展示细节保持在 presentation 层，不进入 canonical command。
- 现有 `admin/remote_invoke_call_history/*.jsonl` 历史文件继续可读，即使包含 legacy `search.get` / `traffic.search` 命令名。

### 必须真实验证

- CLI 真实执行 `bifrost search` 与 `bifrost remote traffic search`，同一 seed 集合上结果一致。
- CLI 真实执行 `bifrost traffic list` / `remote traffic list`，过滤语义一致。
- CLI 真实执行 `bifrost traffic get --request-body --response-body` 与 `remote traffic get`，body 输出一致。
- allowlist 拒绝路径：未开放的 canonical command 会在 caller 前置校验、client 最终校验两处均被拒绝。
- 流式回归：远端 `traffic search` 大结果集下第一个 `frame` 事件早于命令 `exit` 事件到达。

## 产品语义

### 同构定义

- 本地命令与远端命令共享同一套 canonical 查询命令。用户在本地和远端看到的 flag、过滤条件、输出结构完全一致。
- 远端命令 = 本地命令 + 传输 + 鉴权 + allowlist + 流式转发。不允许 remote executor 存在独立的“搜索/流量业务代码”。
- 交互式能力属于 CLI presentation，只在本地生效；canonical command 不承载交互语义。

### canonical command 模型

`crates/bifrost-command/src/lib.rs:250` 已定义：

```rust
pub enum CanonicalQueryCommand {
    Search(CommandSearchArgs),
    TrafficList(TrafficListArgs),
    TrafficGet(TrafficGetArgs),
    TrafficClear(TrafficClearArgs),
    // ... capability = Readonly | Mutating
}
```

`TrafficClear` capability 标记为 `Mutating`，因此不会通过 `ALLOWED_COMMANDS` 暴露到 remote CLI 面。

## 技术细节

### 共享 crate `bifrost-command`

- 位置：`crates/bifrost-command/src/lib.rs`（已落地）。
- 内容：`CanonicalQueryCommand` + 相关参数结构体 + serde。
- 待补：`CommandEvent` 事件流（结构化 hit / traffic 行 / 分页游标 / done 边界）、`CommandDescriptor`（id / display name / capability / allowlist key）、`SearchRenderOptions` / `TrafficRenderOptions` presentation-only 结构体。

### AdminQueryService

- 位置：`crates/bifrost-admin/src/query_service.rs:31`。
- 已实现方法：`execute`、`search`、`search_stream`、`list_traffic`、`query_traffic_params`、`get_traffic_record`、`get_traffic_json`、`clear_traffic`。
- 使用方：HTTP `handlers/search.rs`、`handlers/traffic.rs`、`remote_invoke/executor.rs`（仅当运行时注入 service，否则回退到 legacy HTTP fallback）。

### Remote wire 协议

- `RemoteCommand`（`crates/bifrost-admin/src/remote_invoke/types.rs:404`）：
  - `command: String`（第 408 行）—— legacy 通道，仍在使用。
  - `args_json: Option<String>`（第 410 行）—— legacy 参数字符串。
  - `query: Option<CanonicalQueryCommand>`（第 412 行）—— 新 typed 通道。
- 转发规则：优先使用 `query` 字段；若为 `None`，fallback 到 `command + args_json` 解析。Phase 5 后 legacy 字段整个删除。

### Allowlist

`ALLOWED_COMMANDS = ["status", "search.stream", "traffic.list", "traffic.get"]`（`types.rs:1511`）。

- `is_allowed_command`（`types.rs:1513`）唯一入口。
- caller 侧在 `cli` 生成 `RemoteCommand` 时前置校验。
- client 侧 executor 在真正执行前二次校验。
- 未开放命令返回 `PermissionDenied`。

## CLI

### 本地命令（本次改造范围）

- `bifrost search` — 主搜索入口，clap typed options。
- `bifrost traffic search` — search 别名，共用同一 canonical command 构造函数。
- `bifrost traffic list` — 按 filter/pagination 列表。
- `bifrost traffic get <id> [--request-body] [--response-body]` — 明细。
- `bifrost traffic clear` — 破坏性，capability=`Mutating`，只本地入口。

### 远端命令

- `bifrost remote traffic search` — 与本地 `search` 等价，走 canonical command。
- `bifrost remote traffic list` — 与本地 `traffic list` 等价。
- `bifrost remote traffic get` — 与本地 `traffic get` 等价，`--request-body` / `--response-body` 均支持。
- `bifrost remote traffic clear` — 显式**不暴露**为 CLI 子命令；实际清理必须通过 `remote exec` 且需要 shell 授权。

注意：仓库确认 `bifrost remote search` 顶层子命令并不存在，`RemoteCommands` 枚举只包含 `Conn / File / Traffic / Exec / Job / KeepAwake`。远端搜索一律通过 `remote traffic search` 入口。

## Web

不适用。本方案不改变 Web UI。

## Admin API

- `GET /_bifrost/api/search/*` — 由 `handlers/search.rs` 处理，内部委托 `AdminQueryService`。
- `GET /_bifrost/api/traffic/*` — 由 `handlers/traffic.rs` 处理，内部委托 `AdminQueryService`。
- `POST /_bifrost/api/remote-invoke/*` — remote executor 入口，参数中的 `query` 字段（typed）走 `AdminQueryService`；`command + args_json`（legacy）走 legacy HTTP 拼接 fallback。
- `GET /_bifrost/api/remote-invoke/calls?limit=N&before=<cursor>` — 调用历史分页；历史文件位于 `admin/remote_invoke_call_history/<client-key>.jsonl`，向后兼容 legacy 命令 id。

## Sync 边界

- 本方案不涉及 sync 上下行同步。
- `admin/remote_invoke_call_history/*.jsonl` 属于本地历史，不同步。
- allowlist 与 grant 状态属于本地 relay/client，不同步。

## Phase 1：canonical command 内核（已落地）

- 新增 crate `crates/bifrost-command`。
- 新增 `CanonicalQueryCommand` + 参数结构体 + serde round-trip 测试。
- 新增 `AdminQueryService` 并接入 HTTP handler。
- Remote executor 支持在 `query_service` 已注入时直接调用 service。
- Allowlist 收敛到 canonical id：`status` / `search.stream` / `traffic.list` / `traffic.get`。

完成标志：源码存在 `crates/bifrost-command/src/lib.rs` + `crates/bifrost-admin/src/query_service.rs`，`ALLOWED_COMMANDS` 常量已收敛。

## Phase 2：CLI 与 executor 完全走 typed query（进行中）

执行项：

1. 让 CLI 在构造 `RemoteCommand` 时始终填充 `query`，`command + args_json` 只作为 legacy 兼容分支。
2. Remote executor 在有 `query` 时不再进入 legacy 分支；同步补齐 typed 路径对 `search.stream` / `traffic.list` / `traffic.get` 的分页、body flag、format 参数处理。
3. `admin/remote_invoke_call_history/*.jsonl` 记录 typed command id，legacy id 只在历史读取时兼容展示。

完成标志：`git log --grep=isomorphic` 中出现将 `worker.rs` / `call_history_store.rs` 中所有 `search.get` / `traffic.search` 引用替换为 canonical id 的提交。

状态：(planned, not yet shipped as of 2026-07-03)。当前 `worker.rs` 与 `call_history_store.rs` 测试仍引用 legacy id 字符串。

## Phase 3：结构化事件流

执行项：

1. 新增 `CommandEvent`（`crates/bifrost-command`）：`Frame(Hit | TrafficRow | ProgressCursor) | Error | Done`。
2. Remote executor 直接把 service 迭代结果转成 `CommandEvent`，不再在远端做文本渲染。
3. CLI presentation 层新增 `SearchRenderOptions` / `TrafficRenderOptions`，本地与远端复用同一渲染器。

完成标志：远端 executor 中不存在 search/traffic 专用格式化字符串代码。

状态：(planned, not yet shipped as of 2026-07-03)。

## Phase 4：QueryRunner / QueryBackend 抽象

执行项：

1. 定义 `QueryRunner` trait（同步/异步 + 流式）。
2. `QueryBackend`：`LocalHttp` / `RemoteInvoke`（typed）两种实现。
3. CLI 命令根据参数选择 backend，命令实现本身不感知本地还是远端。

完成标志：`commands/search.rs` / `commands/traffic.rs` / `commands/remote.rs` 之间不再存在实现重复。

状态：(planned, not yet shipped as of 2026-07-03)。

## Phase 5：删除 legacy remote query DSL

执行项：

1. `RemoteCommand` 上删除 `command: String` 与 `args_json: Option<String>` 字段，只保留 `query`。
2. Executor `list_traffic` / `get_traffic` / `search_stream` legacy HTTP 拼接分支删除。
3. `worker.rs` / `call_history_store.rs` 测试中的 `search.get` / `traffic.search` 字符串替换为 canonical id。
4. `human_tests/remote-invoke.md` 中所有 legacy 命令示例更新为 typed 协议。

完成标志：仓库中 `rg "search\.get|traffic\.search"` 只在历史记录 fixture 中出现。

状态：(planned, not yet shipped as of 2026-07-03)。

## 测试方案

### 单元测试

- `crates/bifrost-command/src/lib.rs` 内 serde round-trip：`CanonicalQueryCommand::{Search,TrafficList,TrafficGet,TrafficClear}` 序列化/反序列化稳定。
- `crates/bifrost-admin/src/query_service.rs` 内 `execute`、`search`、`list_traffic`、`get_traffic_json` 正确路由。
- `crates/bifrost-admin/src/remote_invoke/types.rs` 内 `is_allowed_command` 拒绝 `search.get` / `traffic.search` / `traffic.clear` 等非白名单命令。
- `crates/bifrost-admin/src/remote_invoke/worker.rs` 与 `call_history_store.rs`：legacy 与 typed 命令历史 round-trip。

### E2E 测试（`crates/bifrost-e2e`）

- 覆盖 `bifrost search` 与 `bifrost remote traffic search` 在同一 seed traffic 集合下结果一致。
- `traffic list` filter/pagination 本地与远端一致。
- `traffic get` body flag 本地与远端一致。
- 流式回归：大结果集下第一个 frame 早于 exit 到达 caller。
- 未开放 canonical command 被拒绝：`traffic.clear` 通过 `bifrost remote` 入口被拒绝。

### human_tests

必须在 `human_tests/remote-invoke.md` 追加或更新以下用例，并同步 `human_tests/readme.md` 索引：

1. `TC-RI-ISO-01`：`remote traffic search` 与本地 `search` 支持相同过滤条件，结果一致。
2. `TC-RI-ISO-02`：`remote traffic list` 与本地 `traffic list` 支持相同过滤条件，结果一致。
3. `TC-RI-ISO-03`：`remote traffic get --request-body --response-body` 与本地行为一致。
4. `TC-RI-ISO-04`：未开放到 allowlist 的 canonical command 被拒绝（`traffic.clear`、任意未列入 `ALLOWED_COMMANDS` 的 id）。
5. `TC-RI-ISO-05`：仓库内所有 remote query 入口都已切换到 typed query 协议且可正常执行。
6. `TC-RI-ISO-06`：远程调用执行后，输出内容一旦产生就立即推送到 caller，而不是等待命令结束。

状态（2026-07-03）：`human_tests/remote-invoke.md` 尚未出现 `TC-RI-ISO-*` 编号（`rg "TC-RI-ISO" human_tests/` 无匹配），以上均为 (planned, not yet shipped as of 2026-07-03)。TC-RI-ISO-01 中的 `remote search` 应理解为 `bifrost remote traffic search`。

### 影响关联的既有 human_tests

- `human_tests/cli-start-stop-status.md`（`admin/remote_invoke_call_history/*.jsonl` 兼容读取）— 不允许因 legacy 命令 id 删除而破坏历史读取。
- `human_tests/ci-windows-unit-tests.md` TC-CWUT-05（`cargo test -p bifrost-admin remote_invoke::file_access_roots::tests`）— allowlist 收敛不得破坏 file access root 相关测试。

## Review / Fix / Test 闭环

- 每个 Phase 落地前必须先跑：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- Phase 3 / 4 涉及 executor 与 renderer 拆分，必须在 review 阶段确认远端 executor 内不存在 search / traffic 专用格式化字符串。
- Phase 5 删除 legacy 字段前，必须通过 `rg "args_json|command\.command"` 扫描仓库，确认所有调用点已切换。
- human_tests 更新与代码提交同 PR，避免文档与实现漂移。

## 风险与决策

### 1. 切换窗口风险

- 风险：caller 与 client 必须同步升级到 typed query 协议；切换窗口内旧脚本或旧调用方会立即失效。
- 决策：本次改造与 caller/client 同步发布；合并前清点仓库内所有 remote query 调用入口并一次性迁移；删除 legacy 实现前先完成仓库内脚本、测试、文档的全量替换。

### 2. 重构范围过大

- 风险：一次性同时改 parse、runner、renderer、transport，回归面太大。
- 决策：按 Phase 分步推进；先把 admin service 内核收敛出来，再让本地 CLI / remote / HTTP handler 逐步接入。

### 3. 进程边界与所有权不清

- 风险：容易误把“本地 CLI 直接调用 service”理解成跨进程直连。
- 决策：明确 `AdminQueryService` 只存在于 `bifrost-admin` 进程内；本地 CLI 通过 transport 与运行中的 admin 进程通信；remote executor 由于与 admin 同进程，可以直接调用 service。

### 4. 交互式能力边界不清

- 风险：本地 `--interactive`、`traffic get` 交互选择器不适合直接进入 remote。
- 决策：交互仅属于 CLI presentation；canonical command 不包含交互语义。

### 5. 写操作命令开放过快

- 风险：在 capability 和 policy 没稳定前，把 `traffic clear` 放进 remote 命令面，安全边界不清。
- 决策：第一阶段只让同构框架支持写命令类型定义；默认 policy 仍只开放 readonly；`traffic.clear` 明确不出现在 `ALLOWED_COMMANDS`。

### 6. legacy id 历史兼容

- 风险：`admin/remote_invoke_call_history/*.jsonl` 中存量数据仍是 `search.get` / `traffic.search` 命名。
- 决策：Phase 5 删除 legacy wire 字段时，历史读取路径必须继续兼容，仅在写入时使用 canonical id。

## 影响范围

### 必须修改的模块

- `crates/bifrost-command/src/lib.rs`（新增 CommandEvent / CommandDescriptor / render options）。
- `crates/bifrost-admin/src/query_service.rs`（Phase 3 typed event 迭代器）。
- `crates/bifrost-admin/src/remote_invoke/executor.rs`（删除 legacy 分支）。
- `crates/bifrost-admin/src/remote_invoke/types.rs`（Phase 5 删除 legacy 字段）。
- `crates/bifrost-admin/src/remote_invoke/worker.rs`（历史记录使用 canonical id）。
- `crates/bifrost-admin/src/remote_invoke/call_history_store.rs`（读旧写新）。
- `crates/bifrost-cli/src/commands/search.rs` / `traffic.rs` / `remote.rs`（QueryRunner 抽象）。
- `crates/bifrost-admin/src/handlers/search.rs` / `traffic.rs`（保持委托 service）。
- `crates/bifrost-e2e` 相关 E2E 用例。

### 必须删除的实现

- `search.get` / `traffic.search` 等 legacy remote query 字符串入口。
- Remote executor 内部 search / traffic 专用 URL 构造与文本渲染。
- Legacy 查询协议对应的 allowlist 控制项（已完成）。
- Legacy 协议相关的测试 fixture 中的字符串命令名。

### 明确不改的范围

- `status` 命令定义、协议、执行路径、allowlist 与渲染逻辑。
- `shell.exec` 及其他非 `search/traffic` 查询命令。
- relay / grant / 鉴权总体模型。
- 当前 remote invoke 的 `frame/exit` 流式骨架。
- 本地 CLI 的交互式选择器与 TTY 展示细节。

## 与现有设计的关系

本方案是对以下文档的上层收敛：

- `design/remote-command-bridge.md`（relay / grant / 鉴权总体模型）。
- `design/remote-search-limit-split.md`、`design/remote-traffic-cli-enum-size.md`（过渡期增量补丁，本方案落地后归档）。
- `design/remote-invoke-call-args-preview.md`（args 预览与 masking 语义）。
