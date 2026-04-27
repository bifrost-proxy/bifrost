# Remote 命令同构化执行方案

## 背景

当前 `bifrost remote search` / `bifrost remote traffic` 与本地 `bifrost search` / `bifrost traffic` 已经出现明显漂移，问题不只体现在少几个 flag，而是 remote 侧从架构上走成了第二套命令实现：

- 本地链路：`clap -> typed options -> 本地命令实现 -> 本地格式化输出`
- remote 链路：`clap -> 远程专用字符串命令 + args_json -> remote executor -> 远程专用格式化输出`

这导致：

1. 每新增一个查询命令或过滤条件，都要维护本地和 remote 两套实现
2. allowlist 白名单绑定的是 remote 专用字符串命令，而不是主命令体系
3. 远端执行结果是格式化后的文本，调用端无法复用本地 formatter
4. `search.get`、`traffic.search` 这类 remote 内部命名与 CLI 主命令语义不一致
5. `remote search`、`remote traffic search` 只能靠持续补丁追赶本地能力，无法从机制上消除漂移

## 目标

本方案把 remote `search/traffic` 查询命令改造成同构方案：

1. 本地命令和远程命令共享同一套语义命令模型
2. remote 层只负责传输、鉴权、白名单和流式转发，不再重复实现命令语义
3. allowlist 基于统一命令 ID / capability 控制，而不是 remote 专用字符串
4. 调用端复用本地 formatter，远端返回结构化事件流，不再返回 remote 专用渲染文本
5. remote query 统一切换到 typed payload，不保留历史 query 协议
6. 保持 remote invoke 当前“有输出就立即推送”的流式能力，不允许为了统一架构退化成命令结束后一次性返回

## 非目标

本方案当前不覆盖：

1. 一次性把所有 CLI 命令都纳入同构化，第一阶段只覆盖 `search/traffic`
2. 重写 relay 整体远程调用协议
3. 首轮就把写操作命令默认开放到 remote
4. 首轮就把 `shell.exec` 与查询命令完全收敛到同一执行框架

说明：

- `status` 是特殊命令，保持现状定义、现状协议和现状执行路径，不纳入这轮同构化改造
- remote invoke 的流式 `frame/exit` 合同必须与当前其他远程命令保持一致，查询命令不能成为例外

## 同构定义

这里的“同构”固定为四层统一：

1. 统一命令模型：本地和远端都构造同一个 typed command
2. 统一执行语义：本地和远端都通过同一套 command runner 执行
3. 统一输出事件模型：本地和远端都产出同一种 structured event stream
4. 统一能力控制：allowlist 基于 canonical command id / capability

固定差异边界如下：

- 调用发起入口不同：本地直连，远端通过 relay
- 传输载体不同：本地 admin API / remote invoke
- 展示方式不同：本地 TTY 输出 / 远端 caller TTY 输出
- 授权、在线性、审计元数据保持 remote 专属
- 本地交互式选择器和 TTY 细节保持本地专属

## 目标架构

### 分层模型

`search/traffic` 查询类命令按四层拆分：

1. `CLI Parse Layer`
2. `Canonical Command Layer`
3. `Command Runner Layer`
4. `Render Layer`

目标链路：

- 本地：`clap -> CanonicalCommand + RenderOptions -> QueryRunner(LocalAdapter) -> structured event stream -> LocalRenderer`
- 远端：`clap -> CanonicalCommand + RenderOptions -> remote transport -> QueryRunner(ClientAdapter) -> structured event stream -> caller LocalRenderer`

### 共享命令模块

新增共享模块：

- `crates/bifrost-command`

职责：

- 定义 canonical command types
- 定义 command id / capability descriptor
- 定义 structured event / stream types
- 提供 remote typed payload 的编解码

## Canonical Command 设计

### 命令枚举

第一阶段纳入以下命令：

```rust
enum CanonicalQueryCommand {
    Search(SearchArgs),
    TrafficList(TrafficListArgs),
    TrafficGet(TrafficGetArgs),
    TrafficClear(TrafficClearArgs),
}
```

说明：

- `SearchArgs` 同时服务于 `bifrost search`、`bifrost traffic search`、`bifrost remote search`、`bifrost remote traffic search`
- `traffic search` 只是 CLI alias，不再拥有独立语义命令 ID
- `traffic.search` 不再作为 canonical id 存在
- `search.get` 不再作为协议命令存在

### 命令 ID 与 capability

```rust
struct CommandDescriptor {
    id: &'static str,
    capability: CommandCapability,
    output_mode: OutputMode,
}

enum CommandCapability {
    Readonly,
    Mutating,
    SensitiveBody,
}
```

第一阶段命令 ID：

- `search.stream`
- `traffic.list`
- `traffic.get`
- `traffic.clear`

### 语义参数与展示参数分离

- `SearchArgs`：关键字、scope、filters、cursor、limit、max_scan、max_results
- `SearchRenderOptions`：format、no_color、interactive
- `TrafficListArgs`：过滤和分页
- `TrafficRenderOptions`：table、compact、json、json-pretty

remote 只传语义参数，本地 CLI 负责展示参数和交互参数。

## Structured Event Stream 设计

### 原则

1. `stream-first`：命令一旦产出可展示事件，就立即 emit
2. 不允许等命令结束后再统一渲染并一次性回传
3. remote transport 继续沿用当前 `frame/exit` 的实时推送语义
4. 本地与远端的差异只能体现在 transport，不允许体现在是否流式输出

### 事件模型

```rust
enum CommandEvent {
    Begin(CommandBegin),
    SearchResult(SearchResultItem),
    TrafficRow(TrafficRow),
    TrafficDetail(TrafficDetailEvent),
    Progress(CommandProgress),
    Done(CommandDone),
}
```

说明：

- `search` 持续产出 `SearchResult` / `Progress`
- `traffic list` 逐条产出 `TrafficRow`
- `traffic get` 按 detail / request_body / response_body 片段产出 `TrafficDetail`
- `status` 不纳入这轮 canonical event stream

### 现有 `frame/exit` 协议要求

- `frame`：承载 `CommandEvent` 的 JSON 编码
- `exit`：只表示命令终态，不承担最终整段文本回传

保证：

1. caller 在收到每个 frame 后立即渲染
2. relay / caller / client 三侧保持现有实时输出体验
3. `shell.exec` 与查询命令继续复用同一套流式传输骨架

## Query Runner 设计

### 统一 backend 接口

```rust
trait QueryBackend {
    fn execute(
        &self,
        command: &CanonicalQueryCommand,
        sink: &mut dyn CommandEventSink,
    ) -> Result<()>;
}
```

`QueryRunner` 只负责：

- 接收 `CanonicalQueryCommand`
- 分发到 backend
- 统一错误模型
- 统一 structured event stream
- 约束 backend 遵守“有事件就立刻 emit”

### Backend 实现

`bifrost-admin` 内部 service backend 是唯一执行内核。

实现拆成三层角色：

1. `AdminQueryService / DirectAdminServiceBackend`
   - 位于 `bifrost-admin`
   - 作为 `search/traffic` canonical command 的唯一执行内核
   - 对外只暴露 typed command 输入和 `CommandEventSink` 输出
2. `Local CLI Transport Adapter`
   - 位于 `bifrost-cli`
   - 负责把 canonical command 发送给运行中的 admin 进程，并接收结构化事件流
3. `Remote Executor Adapter`
   - 位于 `bifrost-admin::remote_invoke`
   - 在 client 进程内直接调用 `AdminQueryService`
   - 不再自行拼 URL、构造查询参数或渲染文本

关键原则：

1. 执行内核只有一份，在 `bifrost-admin`
2. 本地 CLI 和 remote executor 都只是这份内核的适配层
3. HTTP handler 改为调用同一个 `AdminQueryService`

## Remote Transport 设计

### 协议定义

remote invoke 查询命令统一改为 typed payload：

```rust
struct RemoteCommand {
    kind: CommandKind,
    query: CanonicalQueryCommand,
}
```

规则：

1. caller 只发送 `query`
2. client 只执行 `query`
3. remote query 路径不再接受 `command.command + args_json` 形式的 legacy 查询协议
4. 现有 remote query 字符串命令实现直接删除，不保留兼容转换层

### 流式约束

1. client 每产生一个 `CommandEvent`，就立刻通过 `frame` 推送给 caller
2. caller 不等待 `Done/Exit` 才开始渲染
3. 任何 canonical command 都不能退化成先缓冲、后整包返回

`status` 不参与这组 typed query 协议收敛，继续走现有特殊命令路径。

## Allowlist / Policy 设计

allowlist 面向 canonical command descriptor。

示例：

```toml
[remote_query]
allowed_commands = [
  "search.stream",
  "traffic.list",
  "traffic.get",
]

[remote_mutating]
allowed_commands = [
  "traffic.clear",
]
```

采用两层控制：

1. relay / caller 前置校验
2. client 本地最终校验

`status` 保持现有 allowlist / policy 定义，不并入这轮 canonical allowlist。

## 分阶段实施

### Phase 0：冻结继续扩散

1. 不再新增 remote 专用查询字符串命令
2. 现有 remote 查询字符串命令停止继续扩展
3. `remote-search-limit-split.md`、`remote-traffic-cli-enum-size.md` 等存量改动只作为临时补丁

### Phase 1：抽出 canonical command、event stream 与 admin service 内核

执行项：

1. 新增共享 `CanonicalQueryCommand`
2. 在 `bifrost-admin` 中新增 `AdminQueryService`
3. 拆分 `search.rs` / `traffic.rs` 中的语义参数、service 执行、事件流发射、输出渲染
4. admin HTTP handler 改为调用 `AdminQueryService`
5. 删除 remote query 路径中的 legacy 查询协议解析入口

完成标准：

- admin 内部存在唯一的 `search/traffic` 执行内核
- HTTP handler 与后续 remote executor 复用该内核
- `bifrost search` 与 `bifrost traffic search` 底层共享同一语义命令

### Phase 2：本地 CLI 改为 thin transport adapter，remote client 改为直接执行 canonical command

执行项：

1. 本地 CLI 改为 thin transport adapter
2. remote caller 构造 `query`
3. client executor 反序列化 `query`
4. client executor 直接调用 `QueryRunner(DirectAdminServiceBackend)`

完成标准：

- remote `search/traffic` 功能与本地语义对齐
- 新增过滤条件时无需再改两套实现
- client executor 发出的每个事件都能实时推送到 caller

### Phase 3：caller 侧复用本地 renderer

执行项：

1. remote 返回 structured event stream
2. caller 使用与本地相同的 renderer，并按事件到达顺序增量渲染
3. 删除 remote executor 中的文本渲染逻辑

完成标准：

- local/remote 默认 summary 一致
- local/remote `table|compact|json|json-pretty` 行为一致
- remote 输出首帧不会被延迟到命令结束后

### Phase 4：切换 allowlist 到 canonical command id

执行项：

1. allowlist 改为配置 canonical command id / capability
2. 删除 legacy 查询字符串命令对应的 allowlist 控制项

完成标准：

- 白名单与产品能力一一对应
- 新能力开放只改 policy，不再新增 remote 专用命令实现

### Phase 5：删除 legacy remote query DSL

执行项：

1. 删除 `search.get` / `traffic.search` / `traffic.list` / `traffic.get` / `traffic.clear` 这类 remote query legacy 主路径
2. 删除 legacy 查询协议解析、转换、测试和文档

完成标准：

- remote 查询命令完全使用 typed command
- remote executor 不再包含 legacy 搜索/流量专用格式化逻辑

## 影响范围

### 本次改造纳入范围

本次改造只纳入下面两组命令：

- 本地：`bifrost search`、`bifrost traffic search`、`bifrost traffic list`、`bifrost traffic get`、`bifrost traffic clear`
- 远端：`bifrost remote search`、`bifrost remote traffic search`、`bifrost remote traffic list`、`bifrost remote traffic get`

### 必须修改的模块

#### 共享命令与事件模型

- 新增 `crates/bifrost-command`
- 新增 `CanonicalQueryCommand`、`CommandDescriptor`、`CommandEvent`
- 新增 remote typed payload 的 serde 编解码

#### CLI 入口与参数解析

- `crates/bifrost-cli/src/cli.rs`
- `crates/bifrost-cli/src/main.rs`

#### 本地命令执行与渲染

- `crates/bifrost-cli/src/commands/search.rs`
- `crates/bifrost-cli/src/commands/traffic.rs`
- `crates/bifrost-cli/src/commands/remote.rs`

#### Admin 执行内核

- `crates/bifrost-admin/src/...` 下新增 `AdminQueryService`
- `crates/bifrost-admin/src/search/engine.rs`
- `crates/bifrost-admin/src/handlers/search.rs`
- `crates/bifrost-admin/src/handlers/traffic.rs`

#### Remote Query 执行链路

- `crates/bifrost-admin/src/remote_invoke/types.rs`
- `crates/bifrost-admin/src/remote_invoke/executor.rs`

### 必须删除的实现

- `search.get`、`traffic.search` 这类 remote query 字符串协议入口
- remote query 的 `command.command + args_json` 查询协议解析路径
- remote executor 内部 search/traffic 专用 URL 构造
- remote executor 内部 search/traffic 专用文本渲染
- legacy 查询协议对应的 allowlist 控制项
- legacy 查询协议对应的测试、文档、脚本引用

### 明确不改的范围

- `status` 命令定义、协议、执行路径、allowlist 与渲染逻辑
- `shell.exec` 及其他非 `search/traffic` 查询命令
- relay / grant / 鉴权总体模型
- 当前 remote invoke 的 `frame/exit` 流式骨架
- 本地 CLI 的交互式选择器与 TTY 展示细节

### 文档与测试范围

本次改造同步覆盖以下文档与测试资产：

- `human_tests/remote-invoke.md`
- `human_tests/readme.md`
- `search` / `traffic` / `remote` 相关 CLI 文档
- remote query 协议相关 E2E、单元测试和 human_tests 用例

## 风险与应对

### 1. 切换窗口风险

风险：

- caller 与 client 必须同步升级到 typed query 协议
- 切换窗口内旧脚本或旧调用方会立即失效

应对：

- 本次改造与 caller/client 同步发布
- 合并前清点仓库内所有 remote query 调用入口并一次性迁移
- 删除 legacy 实现前先完成仓库内脚本、测试、文档的全量替换

### 2. 重构范围过大

风险：

- 一次性同时改 parse、runner、renderer、transport，回归面太大

应对：

- 强制按 Phase 分步推进
- 先把 admin service 内核收敛出来，再让本地 CLI / remote / HTTP handler 逐步接入

### 3. 进程边界与所有权不清

风险：

- 容易误把“本地 CLI 直接调用 service”理解成跨进程直连

应对：

- 明确 `AdminQueryService` 只存在于 `bifrost-admin` 进程内
- 本地 CLI 通过 transport 与运行中的 admin 进程通信
- remote executor 由于与 admin 同进程，可以直接调用 service

### 4. 交互式能力边界不清

风险：

- 本地 `--interactive`、`traffic get` 交互选择器不适合直接进入 remote

应对：

- 明确交互仅属于 CLI presentation
- canonical command 不包含交互语义

### 5. 写操作命令开放过快

风险：

- 在 capability 和 policy 没稳定前，把 `traffic clear` 放进 remote 命令面，安全边界不清

应对：

- 第一阶段只让同构框架支持写命令类型定义
- 默认 policy 仍只开放 readonly

## 测试方案

### 单元测试

1. `search` 与 `traffic search` 构造出相同的 `CanonicalQueryCommand::Search`
2. canonical command 的 serde 编解码稳定
3. remote typed payload 的 serde 编解码稳定
4. command descriptor / capability / allowlist 匹配正确
5. local renderer 对本地与 remote 返回的 structured event 渲染一致
6. runner 和 transport 遵守“emit 即推送”的流式约束

### E2E 测试

1. `bifrost search` 与 `bifrost remote search` 在同一批 seed traffic 上返回相同结果集
2. `bifrost traffic list` 与 `bifrost remote traffic list` 的过滤语义一致
3. `bifrost traffic get` 与 `bifrost remote traffic get` 的 body 选项一致
4. remote `search` / `traffic list` / `traffic get` 的首个输出事件会在命令完成前到达 caller
5. 仓库内所有 remote query 调用入口都使用 typed query 协议

### 真实场景测试（human_tests）

实现阶段必须更新 `human_tests/remote-invoke.md`，并新增以下用例：

1. `TC-RI-ISO-01`：`remote search` 与本地 `search` 支持相同过滤条件，结果一致
2. `TC-RI-ISO-02`：`remote traffic list` 与本地 `traffic list` 支持相同过滤条件，结果一致
3. `TC-RI-ISO-03`：`remote traffic get --request-body --response-body` 与本地行为一致
4. `TC-RI-ISO-04`：未开放到 allowlist 的 canonical command 会被拒绝
5. `TC-RI-ISO-05`：仓库内所有 remote query 入口都已切换到 typed query 协议且可正常执行
6. `TC-RI-ISO-06`：远程调用执行后，输出内容一旦产生就立即推送到 caller，而不是等待命令结束

同时必须同步更新 `human_tests/readme.md`。

### 改造后全面回归验证

改造完成后，除上述专项测试外，必须执行完整回归验证，覆盖以下范围：

#### 1. 本地命令回归

1. `bifrost search` 主路径可用
2. `bifrost traffic search` 与 `bifrost search` 结果一致
3. `bifrost traffic list` 的过滤、分页、格式化输出保持可用
4. `bifrost traffic get` 的详情、请求体、响应体输出保持可用
5. `bifrost traffic clear` 的行为、返回码和提示文案保持可用

#### 2. 远端命令回归

1. `bifrost remote search` 主路径可用
2. `bifrost remote traffic search` 与 `bifrost remote search` 结果一致
3. `bifrost remote traffic list` 的过滤行为与本地一致
4. `bifrost remote traffic get` 的详情、请求体、响应体输出与本地一致
5. `bifrost remote traffic clear` 不暴露为 CLI 子命令；清理远端流量需要明确 shell 授权后走 `remote exec`

#### 3. 流式输出回归

1. 本地查询命令保持增量输出
2. 远端查询命令保持 `frame` 实时推送
3. caller 在收到首个 `frame` 后立即开始渲染
4. 命令结束前已经输出的内容不会被延迟到 `exit` 后才显示
5. 大结果集场景下输出顺序稳定，`Done` 事件只在末尾出现一次

#### 4. 渲染与格式回归

1. local/remote 默认 summary 一致
2. local/remote `table|compact|json|json-pretty` 输出一致
3. `--no-color` 在本地和远端调用路径表现一致
4. 请求体与响应体分段输出在本地和远端表现一致

#### 5. HTTP/Admin API 回归

1. HTTP search handler 改造后仍返回正确数据
2. HTTP traffic handler 改造后仍返回正确数据
3. admin service 内核改造后，HTTP 路径与 CLI 路径结果一致

#### 6. Allowlist 与拒绝路径回归

1. 未开放的 canonical command 会被 caller 前置校验拒绝
2. 未开放的 canonical command 会被 client 最终校验拒绝
3. `traffic.clear` 不能通过 `remote traffic` CLI 执行

#### 7. 非改动范围回归

1. `status` 命令行为、协议和渲染保持不变
2. `shell.exec` 与其他非 `search/traffic` remote 命令保持不变
3. relay / grant / 鉴权总体模型保持不变
4. 现有 `frame/exit` 流式骨架在非 query 命令上保持不变

#### 8. 删除项回归

1. 仓库内不再存在 remote query legacy 协议调用入口
2. 仓库内不再存在 query `command.command + args_json` 解析路径
3. 仓库内不再存在 remote executor 的 search/traffic 文本渲染逻辑
4. 文档、测试、脚本中不再引用已删除的 remote query legacy 协议

#### 9. 文档回归

1. CLI 文档与实际参数一致
2. remote query 协议文档与 typed payload 实现一致
3. `human_tests/readme.md` 索引与新增用例一致
4. 设计文档中的改动范围、非改动范围与最终实现一致

## 校验要求

正式实施该方案时，至少执行：

1. 定向单元测试：canonical command / typed payload / renderer parity
2. 对应 remote invoke E2E 套件
3. `cargo fmt --all -- --check`
4. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
5. `cargo test --workspace --all-features`
6. `bash scripts/ci/local-ci.sh` 或按影响范围执行最小必要子集
7. `human_tests/remote-invoke.md` 全量相关用例
8. 全面回归验证清单 1-9 全量执行并逐项确认通过

## 文档更新要求

实施落地时，需要同步更新：

1. `human_tests/remote-invoke.md`
2. `human_tests/readme.md`
3. CLI 文档中与 `search` / `traffic` / `remote` 相关的参数说明
4. 若 allowlist / capability 对外可见，则更新对应 remote invoke 文档

## 特殊命令例外

`status` 在这轮改造中明确作为特殊命令保留现状：

1. 保持当前 remote `status` 的命令名与协议定义
2. 保持当前 `status` 的 caller / client 执行路径
3. 不把 `status` 收编进 `CanonicalQueryCommand`
4. 不为 `status` 新增 `status.get` 这类 canonical command id
5. 不要求 `status` 跟随本轮 `search/traffic` renderer 和 event schema 重构

`status` 的任何重构都必须另开独立设计，不与本方案绑定推进。

## 当前决策

1. 共享命令模型放在 `crates/bifrost-command`
2. `bifrost-admin` 内部 service backend 是唯一执行内核
3. `traffic clear` 保留本地 canonical command 模型，但不进入 `remote traffic` CLI 命令面
4. legacy 查询协议不保留兼容窗口，随本次改造一次性删除

## 与现有设计的关系

本方案是对以下文档的上层收敛：

- `design/remote-command-bridge.md`
- `design/remote-search-limit-split.md`
- `design/remote-traffic-cli-enum-size.md`

关系说明：

1. `remote-command-bridge.md` 继续负责 relay / grant / 鉴权 / 远程调用总体模型
2. 本文专门负责查询命令如何做到本地与远端同构
3. `remote-search-limit-split.md`、`remote-traffic-cli-enum-size.md` 在该方案落地后归类为过渡期增量补丁，不再作为长期架构依据
