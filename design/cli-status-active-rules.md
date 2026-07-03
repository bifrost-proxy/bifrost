# CLI `status` 活跃规则摘要

## 背景

`bifrost status` 是排查“这台机器上的 Bifrost 正常么”时的第一条命令。历史输出偏向进程状态和 Rule Groups 列表；用户实际排查时经常需要三条信息：

1. **服务对外能力**：`127.0.0.1:<port>` 是否在监听？LAN 上别的设备能不能连？系统代理有没有指向这台服务？TLS 拦截打开到什么边界？
2. **默认端口的活跃规则**：主端口现在实际生效的规则合并结果长什么样？（跟 `bifrost rule active` 一致）
3. **临时端口的绑定**：有没有临时端口在跑？它们各自绑了什么规则？有没有 missing refs？

三条信息分散在 `start` 参数、Settings API、`system-proxy status`、TLS 配置页、`bifrost rule active`、`bifrost port list` 里，用户要来回切换命令。本方案把这三块信息合并进 `bifrost status`，让它变成“运行态一屏概览”。

同时，随着临时端口能力上线，`status` 需要区分“默认/主代理端口”和“临时端口绑定规则”两个视角，避免混淆。

## 用户目标验证清单

### 必须实现

- 服务运行时 `bifrost status` 顶部输出 `Service Overview`：本机地址、LAN 地址、监听地址、客户端 fallback 地址、系统代理指向、TLS 拦截 + 各类白名单 / passthrough。
- 服务运行时 `bifrost status` 保留现有基础状态区块，且原“Rule Groups”区块改名为 `Default Port Rule Groups: <port>`。
- 服务运行时在 `Default Port Rule Groups` 之后追加 `Default Port Active Rules: <port>` 区块，展示活跃规则摘要，并在顶部加一行 `Scope: default/main proxy port <port>; temporary port bindings are listed separately at the bottom.`。
- 服务运行时最底部输出 `Temporary Port Bindings` 区块，每个端口展示 host:port、运行状态、可选名称、绑定规则来源、缺失引用。
- 服务未运行时：`Service Overview` 服务字段展示不可用；系统代理仍显示当前 OS 真实状态；不追加 `Default Port Active Rules`；`Temporary Port Bindings` 明确不可用。
- 活跃规则摘要复用 `bifrost rule active` 的现有实现，两处显示格式一致。
- 临时端口区块的数据源是 `GET /_bifrost/api/ports`，与 `bifrost port list/show` 一致。
- 合并规则详情标题为 `Default Port Merged Rules (in parsing order): <port>`。
- TLS 域名白名单、应用白名单、IP 白名单、以及三类 passthrough 各列出完整列表和 count，不做省略。
- 服务运行但 active-summary / TLS / 地址 API 拉取失败时，对应字段输出 `Unknown` 或 `none detected`，不伪造。

### 必须不破坏

- `bifrost rule active` 命令行为不变（输出内容和退出码）。
- `bifrost port list` / `bifrost port show` 命令行为不变。
- `bifrost status` 停止态输出的进程/端口/系统代理判断逻辑不变。
- `runtime.json` 结构不变；新增字段（如果有）向后兼容。
- Admin API `GET /_bifrost/api/rules/active-summary`、`GET /_bifrost/api/config/tls`、`GET /_bifrost/api/proxy/address`、`GET /_bifrost/api/ports` 契约不变。
- 现有 `--json` 类结构化输出（如果有）保持字段命名一致，只新增字段。

### 必须真实验证

- 真实运行一次：起服务、写启用规则、创建一个临时端口绑定，`bifrost status` 输出必须同时包含 `Service Overview`、`Default Port Active Rules`、`Temporary Port Bindings` 三段。
- 服务停止时跑一次：`Service Overview` 服务字段明确不可用；系统代理仍反映 OS 真实状态；不出现 `Default Port Active Rules` 区块。
- TLS 白名单里塞若干条：完整列出且不做省略。
- 拿一个临时端口绑定引用一个已删除规则：`Missing Rules` 展示出来。
- Active-summary API 强制失败：`status` 中输出显式提示。

## 产品语义

### `Service Overview` 是排查的起点

排查 Bifrost 时最常见的问题：

- “我这台机器上 Bifrost 起了没？”
- “局域网别的设备能不能通过我这台代理？”
- “系统代理有没有指向 Bifrost？”
- “TLS 拦截开着么？哪些域名/App 会被绕过？”

`Service Overview` 就是回答这四条问题的“一屏”。字段：

- `Proxy Local Address: http://127.0.0.1:<port>`：本机可达地址。
- `Proxy LAN Addresses`：仅当监听 `0.0.0.0` / `::` 时列出非 loopback 地址；如果只监听 localhost，明确输出 `not applicable (listening on localhost only)`。
- `Proxy Listen Address`：实际监听 host:port，判断 LAN 地址是否可达。
- `Proxy Client Fallback Address`：客户端可用回退地址（`http://<client_proxy_host>:<port>`），监听 `0.0.0.0` / `::` 时回退为 `127.0.0.1`。
- `System Proxy`：`SystemProxyManager::get_current()` 读，展示 enabled/disabled/unsupported/unknown + 是否指向当前服务。
- `TLS Interception`：`GET /_bifrost/api/config/tls`，展示全局开关、`unsafe_ssl` 上游证书校验、配置变更断连策略。
- `TLS Domain Whitelist` / `TLS App Whitelist` / `TLS IP Whitelist`：include 列表 count + 完整列表。
- `TLS Domain Passthrough` / `TLS App Passthrough` / `TLS IP Passthrough`：exclude 列表 count + 完整列表。

### `Default Port Active Rules` 是主端口的“真相视图”

默认端口活跃规则是 `bifrost rule active` 的复用；`status` 只是把它嵌到运行态概览里，避免用户再敲一条命令。它明确用 `Default Port` 前缀强调作用域，防止和临时端口混淆。

`Scope:` 一行是刚性的：告诉用户下面的合并规则只作用于默认/主代理端口，临时端口有独立分区。

### `Temporary Port Bindings` 独立分区

临时端口通过 `bifrost port bind/update` 创建；每个端口有独立规则集，与主端口不共享（除非未来实现 default 全局兜底规则）。`status` 底部单独展示：

- `host:port`、运行状态、可选名称；
- 绑定规则 `Rules:` 列表，按来源展示：`local:<name>`、`group:<group_id>/<name>`、`file:<path>`、`inline:<preview>`；
- 绑定处于 degraded 且存在缺失引用时，额外展示 `Missing Rules: <names>`。

停止态或者没有临时端口时，展示：

- 停止态：`Temporary Port Bindings: not available (service not running)`。
- 空列表：`No temporary port bindings.`。

### 失败降级的显式化

任何 admin API 拉取失败都不能伪装成“已知状态”：

- Active-summary 拉不到：`Default Port Active Rules: <port>` 下输出 `Failed to fetch active summary from the running service: <reason>`。
- TLS API 拉不到：`TLS Interception: Unknown (<reason>)`，各白名单 `Unknown (<reason>)`。
- 地址 API 拉不到：`Proxy LAN Addresses: Unknown (<reason>)`。

## 技术细节

### 复用 active-summary

服务端接口：

```text
GET /_bifrost/api/rules/active-summary
```

返回：活跃规则文件数量、本地规则与 group 规则列表、变量冲突、`merged_content`（按解析顺序合并后的规则内容）。

`bifrost rule active` 与 `bifrost status` 走同一个 `crates/bifrost-cli/src/commands/rule.rs` 的抽取模块：

- `fetch_active_summary(client) -> ActiveSummary`：拉取。
- `format_active_summary(summary, opts) -> String`：格式化。`opts` 里带 `title_prefix`（`""` for `rule active`, `"Default Port "` for `status`）以及 `scope_line`（`Some(<line>)` for `status`, `None` for `rule active`）。

两条命令共享格式化函数，避免漂移。

### `status` 组装顺序

`crates/bifrost-cli/src/commands/status.rs::run_status`：

```
1. 尝试拉 runtime.json + is_process_running
2. 组装 Service Overview 区块：
   a. Proxy Local / LAN / Listen / Client Fallback（服务停止时字段 = "not available (service not running)"，但系统代理仍读 OS）
   b. System Proxy: SystemProxyManager::get_current() 与 runtime host:port 比对
   c. TLS Interception + 六个白名单 / passthrough（服务停止时 "Unknown (server not running)")
3. 组装现有 Runtime / Process / Rule Groups 区块（Rule Groups 改标题为 "Default Port Rule Groups: <port>"）
4. 服务运行时组装 Default Port Active Rules 区块（含 Scope 行 + Merged Rules 子标题）
5. 组装 Temporary Port Bindings 区块（GET /_bifrost/api/ports；停止态显式不可用）
6. 打印全部
```

### `Service Overview` 数据源

| 字段 | 数据源 | 停止态行为 |
|---|---|---|
| Proxy Local Address | runtime.json (host, port) | not available |
| Proxy LAN Addresses | GET /_bifrost/api/proxy/address（仅监听 0.0.0.0/::) | Unknown |
| Proxy Listen Address | runtime.json | not available |
| Proxy Client Fallback Address | runtime.json + fallback rule | not available |
| System Proxy | SystemProxyManager::get_current() | 仍显示 OS 真实状态 |
| TLS Interception + Whitelist / Passthrough | GET /_bifrost/api/config/tls | Unknown |

### 关键代码位点

- `crates/bifrost-cli/src/commands/status.rs:356`：`Service Overview` 标题。
- `crates/bifrost-cli/src/commands/status.rs:243`：`TLS Domain Whitelist` 格式化。
- `crates/bifrost-cli/src/commands/status.rs:418`：`Temporary Port Bindings` 标题。
- `crates/bifrost-cli/src/commands/status.rs:484`：`Default Port Active Rules: <default_port>` 标题拼装。
- `crates/bifrost-cli/src/commands/status.rs:504`：Active-summary 拉取失败时的降级路径。
- `crates/bifrost-cli/src/commands/rule.rs`：抽取 `fetch_active_summary` / `format_active_summary` 供 `status` 复用。

### Missing Rules 判定

临时端口绑定处于 degraded 时（例如引用的 local rule 已被删除、group 已 disable），`GET /_bifrost/api/ports` 返回 `missing_refs` 数组。`status` 逐条格式化：

```
Missing Rules: local:foo, group:team-shared/bar
```

## CLI / Web / Admin API 呈现

### CLI

- 新输出示例：

```
Service Overview:
  Proxy Local Address:            http://127.0.0.1:8888
  Proxy LAN Addresses:            192.168.1.20, 10.0.0.5
  Proxy Listen Address:           0.0.0.0:8888
  Proxy Client Fallback Address:  http://127.0.0.1:8888
  System Proxy:                   Enabled (points to current Bifrost)
  TLS Interception:               enabled (unsafe_ssl=false)
  TLS Domain Whitelist:           2 [api.example.com, admin.example.com]
  TLS App Whitelist:              1 [Google Chrome]
  TLS IP Whitelist:               0 []
  TLS Domain Passthrough:         1 [analytics.example.com]
  TLS App Passthrough:            0 []
  TLS IP Passthrough:             0 []

Runtime: ...
Default Port Rule Groups: 8888
  ...

Default Port Active Rules: 8888
  Scope: default/main proxy port 8888; temporary port bindings are listed separately at the bottom.
  Files: 3
  Rules:
    - local:foo
    - group:team-shared/bar
  Merged Rules (in parsing order):
    ...
  Default Port Merged Rules (in parsing order): 8888
    ...

Temporary Port Bindings:
  9901 (0.0.0.0:9901) status=running name="tmp-debug"
    Rules:
      - local:debug-only
    Missing Rules: local:deleted-rule
```

### Web

- 无 Web UI 改动。Settings / Rules / Ports 页面继续用各自 API 展示。

### Admin API

- 无 API 契约变化；`status` 只是聚合调用现有 API。

## Sync 边界

- `status` 是本机运行态视图，不查 sync。
- `Service Overview` 里显示的 LAN 地址、系统代理、TLS 状态都基于本机 OS 与 admin API，不涉及 relay。
- 临时端口绑定是本机状态；如果通过 sync 恢复的规则被临时端口引用又被 sync 撤销，`missing_refs` 会真实反映当下状态。

## 实现切分

### Phase 1：抽取复用 + 标题改造

- `rule.rs` 抽出 `fetch_active_summary` / `format_active_summary`。
- `status.rs` Rule Groups 标题改为 `Default Port Rule Groups: <port>`。

### Phase 2：`Service Overview` 顶部区块

- 组装本机地址 / LAN 地址 / 监听地址 / fallback。
- 拼装 System Proxy / TLS Interception + 六个白名单 / passthrough。
- 停止态字段显式不可用；系统代理仍显示 OS 真实状态。

### Phase 3：`Default Port Active Rules`

- 服务运行时追加区块 + `Scope:` 行 + Merged Rules 子标题。
- 拉取失败时显式提示。

### Phase 4：`Temporary Port Bindings` 底部区块

- 拉 `/api/ports`；逐端口渲染绑定规则；展示 Missing Rules。
- 停止态显式不可用；空列表显式提示。

### Phase 5：测试与手工回归

- 单测覆盖运行态 + 停止态 + 失败降级。
- E2E 更新 `test_cli_online_commands_e2e.sh` 断言。
- 手工用例更新 `human_tests/cli-start-stop-status.md` + `temporary-port-rule-bindings.md`。

## 测试方案

### 单元测试（`status.rs` 内部 `#[cfg(test)]`）

- 运行态渲染包含 `Default Port Active Rules: 9900`（`status.rs:946`）。
- 停止态不包含 `Default Port Active Rules`。
- Active-summary 有 `merged_content` 和空内容两条路径都能稳定输出。
- 运行态包含本机地址、LAN 地址状态、系统代理状态、TLS 配置、TLS 六类白名单 / passthrough。
- TLS 白名单条目 6 条时全列出（`status.rs:1029`）；无省略、无 truncation。
- 停止态服务字段不可用，但系统代理不被隐藏；TLS 白名单展示 `Unknown (server not running)`（`status.rs:1063`）。
- `Temporary Port Bindings` 逐端口列绑定规则引用、名称、missing refs（`status.rs:1149`）；空列表和停止态各自明确提示。

### E2E 测试

- 更新 `e2e-tests/tests/test_cli_online_commands_e2e.sh`：
  - 启动服务；写入启用规则；创建一个临时端口绑定；执行 `bifrost status`。
  - 断言输出包含：
    - `Proxy Local Address: http://127.0.0.1:<port>`
    - `Proxy LAN Addresses:`
    - `System Proxy:`
    - `TLS Interception:`
    - `TLS Domain Whitelist:`
    - `TLS App Whitelist:`
    - `Temporary Port Bindings`
    - 临时端口号、绑定名称、`local:<rule-name>`
    - `Default Port Active Rules: <port>`
    - `Scope: default/main proxy port <port>`
    - `Default Port Merged Rules (in parsing order): <port>`
    - 已启用规则对应的合并内容

### 真实场景测试

- `human_tests/cli-start-stop-status.md`：
  - 服务运行时 `status` 顶部展示本机地址、LAN 地址状态、系统代理状态、TLS 状态 + TLS 域名/应用白名单边界。
  - 服务运行且存在临时端口绑定时，`status` 最底部展示每个临时端口绑定的规则。
  - 服务运行时 `status` 展示默认端口活跃规则摘要，并明确它与临时端口分区。
  - 服务停止时 `status` 不展示 `Default Port Active Rules`；`Temporary Port Bindings` 明确不可用；系统代理仍显示 OS 真实状态。
- `human_tests/temporary-port-rule-bindings.md`：新增用例覆盖 missing refs 展示。
- `human_tests/readme.md`：更新索引。

## Review / Fix / Test 闭环

- 提交前：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`。
- 单测：`cargo test -p bifrost-cli status`。
- E2E：`bash e2e-tests/tests/test_cli_online_commands_e2e.sh`。
- 手工：`human_tests/cli-start-stop-status.md` 相关用例。

## 风险与决策

- **决策 1**：合并进 `bifrost status`，不新增 `bifrost overview` 之类命令。理由：`status` 本来就是运行态入口，用户熟悉；两条命令会带来发现成本。
- **决策 2**：TLS 白名单完整列出、不省略。理由：白名单本身就是排查“TLS 有没有把某个域名/App 绕过”的关键信息；省略会让排查变成再敲一条命令。
- **决策 3**：`Temporary Port Bindings` 放在最底部而不是紧跟主端口区块。理由：临时端口是非常态资源，主视图是主端口；用户查主端口时不希望被临时端口“先入为主”。
- **风险**：TLS 白名单条目非常多（数百条）时输出会很长。缓解：当前不做省略；如果实际用户报告太长，再引入 `--brief` 或 `--json` 开关。
- **风险**：Admin API 短时不可达时会给用户造成“status 卡住”的感觉。缓解：给拉取加短 timeout（例如 500ms），超时后走降级路径显式提示。

## 影响文件

- `crates/bifrost-cli/src/commands/rule.rs`
- `crates/bifrost-cli/src/commands/status.rs`
- `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- `human_tests/cli-start-stop-status.md`
- `human_tests/temporary-port-rule-bindings.md`
- `human_tests/readme.md`
