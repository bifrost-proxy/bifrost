# CLI `status` 活跃规则摘要

## 背景

当前 `bifrost status` 会展示进程状态与 Rule Groups 信息，但如果用户想直接查看“当前运行实例实际生效的活跃规则合并结果”，还需要额外执行一次 `bifrost rule active`。

同时，排查正在运行的代理服务时，用户最先需要确认的不是规则列表，而是“这个服务能被谁连上、系统代理有没有指向它、TLS 拦截当前打开到什么边界”。这些信息过去分散在 `start` 参数、Settings API、`system-proxy status` 和 TLS 配置页面里，`status` 顶部无法一眼给出服务能力边界。

这会让 `status` 作为运行态概览命令的信息不完整，排查规则优先级或规则是否真正生效时需要来回切换命令。

## 目标

- 在 `bifrost status` 输出末尾追加活跃规则摘要
- 在 `bifrost status` 输出顶部追加运行服务能力概览
- 在 `bifrost status` 输出最底部追加临时端口绑定规则摘要
- 将默认/主代理端口的启用规则、活跃规则摘要和合并规则详情明确标注为 `Default Port`，避免与临时端口绑定规则混淆
- 显式展示 `127.0.0.1:<port>` 本机代理地址与可用局域网代理地址
- 显式展示系统代理配置是否指向当前 Bifrost 服务
- 显式展示 TLS 全局开关、上游证书校验，并区分 TLS 域名白名单、应用白名单、IP 白名单和 passthrough 边界
- 仅当服务处于运行状态时展示该摘要
- 复用现有 `rule active` 的管理端接口与展示格式，避免两处实现漂移
- 复用现有 `bifrost port` 管理端接口，避免端口绑定规则在 `status` 和 `port list/show` 之间漂移

## 实现方案

### 复用 active-summary

继续使用已有运行时接口：

```text
GET /_bifrost/api/rules/active-summary
```

该接口已经返回：

- 活跃规则文件数量
- 本地规则与 group 规则列表
- 变量冲突信息
- `merged_content`（按解析顺序合并后的规则内容）

### CLI 结构调整

- 在 `crates/bifrost-cli/src/commands/rule.rs` 中抽取可复用的：
  - active-summary 拉取逻辑
  - active-summary 文本格式化逻辑
- `bifrost rule active` 继续使用同一套逻辑输出
- `crates/bifrost-cli/src/commands/status.rs` 将现有规则区块改为 `Default Port Rule Groups: <port>`，并说明这是默认/主代理端口规则，临时端口规则在底部独立展示
- `crates/bifrost-cli/src/commands/status.rs` 在确认服务运行后，现有 `Default Port Rule Groups` 输出后追加默认端口活跃规则摘要
  - 标题为 `Default Port Active Rules: <port>`
  - 在摘要顶部输出 `Scope: default/main proxy port <port>; temporary port bindings are listed separately at the bottom.`
  - 合并规则详情标题为 `Default Port Merged Rules (in parsing order): <port>`
- `crates/bifrost-cli/src/commands/status.rs` 在原 Runtime 区块前输出 `Service Overview`：
  - `Proxy Local Address`：固定展示当前运行服务的 `http://127.0.0.1:<port>`
  - `Proxy LAN Addresses`：当监听地址是 `0.0.0.0` / `::` 时，读取 `GET /_bifrost/api/proxy/address` 并列出非 loopback 地址；若只监听 localhost，则明确输出不可用
  - `Proxy Listen Address`：展示实际监听 host:port，帮助判断 LAN 地址是否可达
  - `System Proxy`：只读 `SystemProxyManager::get_current()`，展示 enabled/disabled/unsupported/unknown，并判断 host+port 是否指向当前服务
  - `TLS Interception`：读取 `GET /_bifrost/api/config/tls`，展示全局开关、`unsafe_ssl` 上游证书校验策略和配置变更断连策略
  - `TLS Domain Whitelist`：展示域名 include 列表计数和完整列表，不省略任何条目
  - `TLS App Whitelist`：展示应用 include 列表计数和完整列表，不省略任何条目
  - `TLS IP Whitelist`：展示 IP include 列表计数和完整列表，不省略任何条目
  - `TLS Domain Passthrough`：展示域名 exclude 列表计数和完整列表，不省略任何条目
  - `TLS App Passthrough`：展示应用 exclude 列表计数和完整列表，不省略任何条目
  - `TLS IP Passthrough`：展示 IP exclude 列表计数和完整列表，不省略任何条目
- `crates/bifrost-cli/src/commands/status.rs` 在所有现有状态区块之后输出 `Temporary Port Bindings`：
  - 数据源为 `GET /_bifrost/api/ports`
  - 每个端口显示 `host:port`、运行状态、可选名称
  - `Rules` 列表按绑定来源展示 `local:<name>`、`group:<group_id>/<name>`、`file:<path>` 或 `inline:<preview>`
  - 若绑定处于 degraded 且存在缺失引用，额外展示 `Missing Rules`

### 输出边界

- 服务运行中：
  - 顶部展示 `Service Overview`
  - `status` 保留现有基础状态信息
  - 保留默认端口 `Default Port Rule Groups` 区块
  - 追加 `Default Port Active Rules` 区块
  - 最底部追加 `Temporary Port Bindings` 区块；没有临时端口时显示 `No temporary port bindings.`
- 服务未运行：
  - 顶部展示服务相关字段不可用，系统代理字段仍按当前 OS 配置真实输出
  - 保持现有停止态输出
  - 不追加 `Default Port Active Rules` 区块
  - `Temporary Port Bindings` 区块显示服务未运行所以不可用
- 服务运行但 active-summary 拉取失败：
  - 在 `status` 中输出一行提示，说明无法从运行中的服务获取活跃规则摘要
- 服务运行但 TLS 或地址 API 拉取失败：
  - 对应字段输出 `Unknown` 或 `none detected`，不把未验证状态伪装成已配置

## 测试方案

### 单元测试

- `status` 运行态渲染时包含 `Default Port Active Rules: <port>`，并说明这是默认/主代理端口规则
- `status` 停止态渲染时不包含 `Default Port Active Rules`
- active-summary 格式化在有 `merged_content` 和空内容时都能稳定输出
- `Service Overview` 运行态渲染时包含本机地址、LAN 地址状态、系统代理状态、TLS 配置、TLS 域名/应用/IP 白名单与 passthrough 边界
- 停止态渲染时服务地址和 TLS 字段明确不可用，但系统代理状态不被隐藏
- `Temporary Port Bindings` 渲染时逐端口列出绑定规则引用、名称和 missing refs；空列表与停止态有明确提示

### E2E 测试

- 更新 `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- 启动服务、写入启用规则、创建一个临时端口绑定后执行 `bifrost status`
- 断言输出包含：
  - `Proxy Local Address: http://127.0.0.1:<port>`
  - `Proxy LAN Addresses:`
  - `System Proxy:`
  - `TLS Interception:`
  - `TLS Domain Whitelist:`
  - `TLS App Whitelist:`
  - `Temporary Port Bindings`
  - 临时端口号、绑定名称和 `local:<rule-name>`
  - `Default Port Active Rules: <port>`
  - `Scope: default/main proxy port <port>`
  - `Default Port Merged Rules (in parsing order): <port>`
  - 已启用规则对应的合并内容

### 真实场景测试

- 更新 `human_tests/cli-start-stop-status.md`
- 覆盖两类场景：
  - 服务运行时 `status` 顶部展示本机地址、LAN 地址状态、系统代理状态、TLS 状态和 TLS 域名/应用白名单边界
  - 服务运行且存在临时端口绑定时，`status` 最底部展示每个临时端口绑定的规则
  - 服务运行时 `status` 展示默认端口活跃规则摘要，并明确它与临时端口绑定规则分区
  - 服务停止时 `status` 不展示该区块

## 影响文件

- `crates/bifrost-cli/src/commands/rule.rs`
- `crates/bifrost-cli/src/commands/status.rs`
- `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- `human_tests/cli-start-stop-status.md`
- `human_tests/temporary-port-rule-bindings.md`
- `human_tests/readme.md`
