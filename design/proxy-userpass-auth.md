# 代理 `user:password` 鉴权兼容方案

> 实现状态（截至 2026-06-16）：核心配置模型、HTTP 407 鉴权、SOCKS5 IP+userpass 组合、运行时 `last_connected_at`、admin API、Web 设置页、CLI `start --proxy-user` 与 `config access.userpass` 均已落地；剩余 README/docs 更新等少量项标注为 “(planned, not yet shipped as of 2026-06-16)”。

## 功能模块描述

为 Bifrost 代理增加一套可选的 `user:password` 客户端鉴权能力，并且它不是新的“单选 access mode”，而是对现有 `local_only` / `whitelist` / `interactive` / `allow_all` 的补充。

目标语义：

- 保持现有基于来源 IP 的访问控制不变
- 当客户端未满足现有访问控制时，允许通过 `user:password` 继续完成授权
- HTTP 代理与 SOCKS5 统一支持同一套用户名密码
- 支持多个账号并行生效
- Web 管理端与 CLI 首期即支持配置与查看该能力
- 记录每个账号最近一次成功连接时间，并在管理端可见
- 未配置该能力时，行为与当前版本完全一致

## 现状分析

### 1. 当前访问控制是纯 IP 维度

- `ClientAccessControl` 只基于客户端 IP、白名单、LAN、交互式待审批做决策
- `AccessMode` 目前仅有 `allow_all`、`local_only`、`whitelist`、`interactive`
- 当前 `check_access()` 返回的也是纯连接级结果：`Allow` / `Deny` / `Prompt`

这意味着当前授权入口发生在“接受 TCP 连接之后、协议分流之前”，并不会读取 HTTP 请求头，也不会读取 SOCKS5 用户名密码。

### 2. HTTP 代理目前没有下游客户端鉴权

- HTTP 请求链路里已经支持“给上游代理追加 `Proxy-Authorization`”，但这是 Bifrost 作为客户端访问上游代理时的逻辑
- 当前没有“校验客户端发给 Bifrost 的 `Proxy-Authorization`”这条链路
- 当前上游转发前的 header 清洗没有显式移除下游客户端带来的 `proxy-authorization`

这说明现在项目里已有 Basic 编码与上游代理鉴权的经验，但没有把它用于“Bifrost 自身作为代理服务端”的鉴权。

### 3. SOCKS5 已经有用户名密码校验能力，但只是协议局部能力

- `SocksHandler` 已经实现 RFC1929 用户名密码握手与校验
- 这套能力是 SOCKS5 handler 内部逻辑，不与 `AccessMode` 组合，也不对 HTTP 生效
- 当前配置模型里存在 `server.socks5_auth`，但整体访问控制主链路并没有围绕它构建统一的“客户端代理鉴权”能力

结论：代码库已经具备一半能力——

- IP 访问控制：完整
- SOCKS5 用户密码校验：局部存在
- HTTP 客户端代理鉴权：缺失
- 跨协议统一语义：缺失

## 问题本质

本需求的关键不是“再加一个 access mode”，而是把“来源 IP 授权”和“用户名密码授权”组合成一套兼容策略。

如果简单新增 `basic_auth` 模式，会带来两个问题：

- 用户无法同时保留白名单 / 交互式机制
- 不符合“即使没有命中原有授权方案，也可以通过 `user:password` 放行”的诉求

因此推荐做成“叠加式授权”：

1. 先尝试现有 IP 规则
2. 未通过时，再尝试 `user:password`
3. 两者都未通过时，再进入原有 `interactive` / `deny` 结果

## 方案目标

- 不新增新的 access mode
- `user:password` 作为可选补充能力存在
- HTTP 与 SOCKS5 共享同一套客户端凭证配置
- 支持多个启用中的账号并行认证
- 现有管理端“访问控制”入口继续承载该能力
- 首期同时交付 admin API、Web、CLI 的配置能力
- 记录并展示每个账号最近一次成功鉴权时间
- 现有未配置凭证的部署零行为变化

## 非目标

- 本期不引入 OAuth、Token、双因子、SSO
- 本期不做角色、分组、权限域、多租户
- 本期不做外部身份源集成
- 本期不做旧数据兼容的复杂迁移，采用配置结构直接演进

## 实现逻辑

### 一、配置模型 (shipped)

在 `access` 配置下新增独立的用户名密码配置，而不是新增 mode，并且账号模型直接按多账号设计。

建议模型：

```toml
[access]
mode = "interactive"
allow_lan = false
whitelist = ["10.0.0.0/8"]

[access.userpass]
enabled = true

[[access.userpass.accounts]]
username = "demo"
password = "secret"
enabled = true

[[access.userpass.accounts]]
username = "ops"
password = "another-secret"
enabled = true
```

建议结构：

```rust
pub struct UserPassAccountConfig {
    pub username: String,
    pub password: Option<String>,
    pub enabled: bool,
}

pub struct UserPassAuthConfig {
    pub enabled: bool,
    pub accounts: Vec<UserPassAccountConfig>,
    pub loopback_requires_auth: bool, // 默认 false，本机免密
}

pub struct AccessControlConfig {
    pub mode: AccessMode,
    pub whitelist: Vec<String>,
    pub allow_lan: bool,
    pub userpass: Option<UserPassAuthConfig>,
}
```

说明：

- 放在 `access` 下，语义上明确它属于“客户端访问控制”
- 不直接复用现有 `server.socks5_auth`，避免 HTTP 和 SOCKS 出现两套配置源
- 每个账号以 `username` 作为唯一键，要求唯一且区分大小写
- 对外查询接口不返回明文 password，只返回账号级 `enabled`、`username`、`has_password`
- 账号允许禁用但保留，方便临时停用而不是删除

### 二、配置入口复用策略 (shipped)

现有 Web 设置页和 CLI `config` 都是围绕“whitelist/access”状态工作的，因此首期不建议再单独发明第二套配置入口，而是直接扩展现有 access 返回结构。

建议复用方式：

- `/api/whitelist` 状态响应扩展 `userpass` 字段
- `/api/whitelist` 相关更新接口旁新增 `userpass/accounts` 维护接口
- Web 继续在 Access Settings 页签中新增一块 `User/Password Auth`
- CLI `config` 继续归类到 `access.*`

建议响应结构：

```json
{
  "mode": "interactive",
  "allow_lan": false,
  "whitelist": ["10.0.0.0/8"],
  "temporary_whitelist": [],
  "userpass": {
    "enabled": true,
    "accounts": [
      {
        "username": "demo",
        "enabled": true,
        "has_password": true,
        "last_connected_at": "2026-04-02T12:34:56Z"
      },
      {
        "username": "ops",
        "enabled": false,
        "has_password": true,
        "last_connected_at": null
      }
    ]
  }
}
```

这样做的好处：

- Web 现有 `AccessControlTab` 与 store 改动集中在 access 页，不会引入第二套设置模型
- CLI 现有 `config` 与 `whitelist` 相关客户端可以增量扩展，而不是重建访问控制命令体系
- push scope 继续复用 `whitelist_status`

### 三、授权语义 (shipped)

统一授权优先级建议如下：

1. loopback 默认直接放行（`loopback_requires_auth = false` 时）
2. 若 `loopback_requires_auth = true`，loopback 也需要通过用户名密码认证
3. 现有 IP 访问控制通过时直接放行
4. 若配置了 `userpass`，则允许客户端通过任一启用账号的用户名密码完成授权
5. 若仍未通过且 mode=`interactive`，进入 pending authorization
6. 其他情况拒绝

等价表达：

```text
Allow = IpAccessAllowed(unless loopback_requires_auth) OR AnyEnabledCredentialAuthenticated
Fallback = existing Interactive / Deny behavior
```

这正好满足“不是选择题，而是兼容叠加”的诉求。

### 四、连接接入阶段调整 (shipped)

当前主端口在协议分流前就执行 `check_access()` 并直接拒绝，这会导致 HTTP 客户端连发送 `Proxy-Authorization` 的机会都没有。

因此需要把“最终拒绝”从 accept 阶段下沉到协议处理阶段，但只在“配置了 userpass 且当前 IP 未通过”时下沉。

建议改为：

- accept 阶段先得到 `initial_access_decision`
- 若 `initial_access_decision = Allow`，保持现状
- 若 `initial_access_decision = Deny/Prompt` 且未开启 `userpass`，保持现状
- 若 `initial_access_decision = Deny/Prompt` 且开启了 `userpass`，允许进入协议处理链路，交给 HTTP / SOCKS5 再判定

这样改动面最小，也不会改变未开启新能力时的行为。

### 五、HTTP 代理鉴权链路 (shipped; helper enum 命名以实际代码为准，未必叫 `CredentialCheckResult`)

在 HTTP 请求进入 `handle_request()` 之后、真正执行 admin/proxy 路由前，增加 HTTP 客户端鉴权检查：

- 仅对真实代理流量生效
- admin path、public cert path、loopback 管理流量继续豁免
- 从 `Proxy-Authorization` 解析 Basic 凭证
- 在启用账号列表中校验 `username:password`
- 成功则允许继续处理 `CONNECT` 或普通 HTTP 请求
- 失败则返回 `407 Proxy Authentication Required`
- 响应头带 `Proxy-Authenticate: Basic realm="Bifrost"`

建议补充一个统一 helper：

```rust
enum CredentialCheckResult {
    Passed,
    Missing,
    Invalid,
    NotConfigured,
}
```

HTTP 返回策略建议：

- `Missing` / `Invalid`：返回 407
- 若当前 mode=`interactive`，可以在首次失败时补充 pending 记录
- body 明确提示“可通过管理员审批或提供代理用户名密码访问”
- 成功时记录命中的 `username`，用于刷新该账号最近连接时间

### 六、HTTP 请求头清洗 (shipped — 见 `crates/bifrost-proxy/src/server.rs` 中对 `proxy-authorization` header 的清洗)

当前下游客户端携带的 `proxy-authorization` 不应继续透传给目标站点。

因此需要在“客户端鉴权完成后、转发到目标站点前”显式移除：

- `proxy-authorization`

例外：

- 若命中了 `proxy://user:pass@upstream` 这类上游代理规则，则由现有上游代理逻辑重新生成面向上游代理的 `Proxy-Authorization`

这样可以避免把下游客户端凭证泄露给真实业务目标。

### 七、SOCKS5 鉴权链路 (shipped — `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 中根据 `requires_userpass_auth()` 选择 `UsernamePassword` 或 `NoAuth`)

SOCKS5 已有用户名密码握手能力，因此方案重点不是“从零实现”，而是让它和 IP 访问控制组合起来。

建议语义：

- 若来源 IP 已通过现有 access control，则 SOCKS5 可以继续 `NoAuth`
- 若来源 IP 未通过，但开启了 `userpass`，则 SOCKS5 选择 `UsernamePassword`
- 用户名密码正确则放行
- 用户名密码失败后：
  - mode=`interactive` 时可记 pending
  - 其他模式直接拒绝
- 成功时同样记录命中的 `username`

这样可以让 SOCKS5 与 HTTP 的对外语义一致：

- IP 命中老规则，可以直接过
- IP 未命中，可以靠用户名密码补充过

### 八、账号最近连接时间与运行时状态 (shipped — 落地在 `crates/bifrost-storage/src/state.rs` 的 `userpass_last_connected_at: HashMap<String, u64>`，与设计建议一致)

“最近连接时间”不建议写回主配置文件。

原因：

- 每次成功请求都改 `config.json` 会造成高频磁盘写入
- 配置文件的职责是“声明式配置”，最近连接时间属于运行时状态
- 未来若增加更多访问统计，继续写配置文件会让配置语义变脏

建议把该信息落到现有 `state.json` 一类运行时状态存储中，而不是配置存储中。

建议模型：

```rust
pub struct UserPassRuntimeState {
    pub last_connected_at_by_username: HashMap<String, u64>,
}
```

建议语义：

- 仅在用户名密码鉴权成功时更新对应账号的 `last_connected_at`
- 若请求是因为 IP 规则已通过而未使用用户名密码，则不刷新该时间
- 删除账号时，移除对应 username 的最近连接时间
- 修改 username 时，旧 username 对应的最近连接时间清空
- 禁用账号时保留历史时间，便于审计展示
- 该状态允许跨重启保留

这样展示的就是“每个账号上一次真正靠用户名密码成功接入的时间”。

### 九、热更新 (shipped — 通过 `ClientAccessControl` 的 generation 机制；`set_userpass_config` 触发后续请求按新配置校验)

当前 `ClientAccessControl` 已经有 generation，用于访问控制变化后让已有连接重新评估。

新增 `userpass` 后建议：

- 把凭证配置纳入同一代际变更语义
- 更新账号列表、用户名、密码或启用状态后，新的 HTTP 请求立即按新配置校验
- 更新最近连接时间不参与 generation 递增，避免每次成功连接都触发连接级重评估
- 现有已建立 CONNECT / SOCKS 通道不强制中断
- 现有 keep-alive HTTP 连接上的后续请求按最新配置重新校验

这与现有“配置变化影响后续请求”的语义保持一致。

### 十、管理端、Web 与 CLI (shipped — admin `/whitelist/userpass`、Web `AccessControlTab`、CLI `start --proxy-user` 与 `config access.userpass.*` 均已落地)

首期直接交付完整配置面，不拆到第二阶段。

- 管理端状态接口返回：
  - `userpass.enabled`
  - `userpass.loopback_requires_auth`
  - `userpass.accounts[]`
  - `username`
  - `enabled`
  - `has_password`
  - `last_connected_at`
- 管理端更新接口支持新增 / 删除 / 修改 / 启停账号
- Web 设置页在 Access Settings 中新增一块 “User/Password Auth”
- Web 展示字段：
  - enabled 开关
  - 账号列表
  - username 输入框
  - password 输入框
  - 账号级 enabled 开关
  - last connected 时间展示
- CLI `start` 参数补充：
  - `--proxy-user <USER:PASS>`，可重复传入
- CLI `config` 首期直接补充：
  - `access.userpass.enabled`
  - `access.userpass.accounts`
  - `access.userpass.loopback-requires-auth`
- CLI `config get/export/show` 返回：
  - `enabled`
  - `accounts[]`
  - `username`
  - `enabled`
  - `has_password`
  - `last_connected_at`

首期建议密码仍然只支持写入，不支持查询回显；CLI `show/export` 仅展示 `has_password`。

## 推荐落地顺序

### Phase 1：完整首期范围

- 扩展 `access` 配置模型 (shipped)
- 扩展 access 状态响应结构 (shipped)
- accept 阶段支持“带 userpass 的 deferred decision” (shipped)
- HTTP 代理支持 `Proxy-Authorization: Basic` (shipped)
- SOCKS5 复用同一套用户名密码配置 (shipped)
- 转发前移除下游 `proxy-authorization` (shipped)
- 多账号最近连接时间写入运行时状态 (shipped — `state.json`)
- admin API 返回与更新 `userpass` 状态 (shipped — `PUT /api/whitelist/userpass`)
- Web Access Settings 增加配置入口 (shipped — `web/src/pages/Settings/tabs/AccessControlTab.tsx`)
- CLI `start` 与 `config` 子命令补齐查看 / 写入 (shipped — `--proxy-user`、`bifrost config access.userpass.*`)
- 推送通道补充 access 状态变更 (shipped — `bifrost-admin/src/push.rs` 暴露 `userpass` 字段)
- README / docs 补充 HTTP 代理鉴权说明 (planned, not yet shipped as of 2026-06-16 — CLI docs 已写入 `docs/cli.md`，但 `README.md` / `README.en.md` 尚未提及 `user:password`)

## 风险与处理

### 1. 协议分流前放宽连接接入

风险：

- 开启 `userpass` 后，未授权连接会先进入协议解析阶段，不能像现在一样在 accept 时立即丢弃

控制方式：

- 仅在“IP 未通过且 `userpass` 已启用”时启用 deferred 路径
- 继续依赖现有连接并发上限、header buffer 限制、超时设置

### 2. 下游凭证泄露

风险：

- 若不移除 `proxy-authorization`，客户端给 Bifrost 的凭证可能被转发到真实目标站点

控制方式：

- 在所有真实上游转发前统一清洗该 header
- 只有命中显式上游代理规则时，才由上游代理发送逻辑重新构造该 header

### 3. 管理端回显密码

风险：

- Web / CLI / push 若直接返回 password，会造成凭证泄露

控制方式：

- 所有读取接口只返回 `has_password`
- 密码只允许写入与清空，不允许明文查询

### 4. 多账号下的用户名冲突与配置复杂度

风险：

- 多账号后，如果允许重名 username，会出现状态归属、最近连接时间归属和认证命中结果不确定

控制方式：

- 强制 username 唯一
- 配置写入时做去重校验
- 运行时状态以 username 为键，避免额外生成内部账号 id

### 5. 最近连接时间写入过于频繁

风险：

- 如果每次成功鉴权都同步落盘，可能带来额外 I/O 抖动

控制方式：

- 仅在“用户名密码鉴权成功”时写入
- 使用现有运行时状态文件，避免改动主配置文件
- 实现时可增加简单去抖策略，例如同一秒内重复成功只写一次

## 测试方案

### 单元测试

- `AccessMode + userpass` 组合语义测试
- 多账号命中顺序与唯一用户名校验测试
- HTTP Basic 解析与非法格式测试
- 转发前 header 清洗测试，确保 `proxy-authorization` 不泄露
- SOCKS5 在 IP 未命中时要求用户名密码的选择逻辑测试
- 最近连接时间仅在命中账号凭证成功时更新的状态测试
- 删除账号、修改 username 后清理旧时间戳的状态测试

### E2E 测试

新增 `bifrost-e2e` 覆盖以下场景：

- `whitelist` 未命中，但 HTTP `Proxy-Authorization` 正确，请求成功
- `local_only` 下远端客户端凭证正确，请求成功
- `interactive` 下未带凭证，请求进入 pending
- `interactive` 下带正确凭证，请求直接成功且不进入 pending
- HTTP 凭证错误时返回 407
- SOCKS5 在 IP 未命中时通过用户名密码成功建立连接
- 下游 `Proxy-Authorization` 不透传到目标站点
- 两个账号同时生效，任一账号都可成功通过
- Web 设置页可配置多个账号并看到各自最近连接时间
- CLI `config` 与 `start` 可配置多个账号
- `loopback_requires_auth=false` 时，本机 HTTP/HTTPS/SOCKS5 请求免密直连
- `loopback_requires_auth=true` 时，本机请求必须提供正确密码（返回 407）
- `loopback_requires_auth` 开关动态切换后行为立即生效

## 校验要求

- 先执行新增/相关 E2E
- 至少执行一次：
  - `cargo test --workspace --all-features`
- 开发完成后执行：
  - `rust-project-validate`

## 文档更新要求

- 更新 `README.md`，说明代理支持 `user:password` 客户端鉴权 (planned, not yet shipped as of 2026-06-16)
- 更新 CLI / 配置文档，补充 access.userpass.accounts 与重复 `--proxy-user` 参数 (partially shipped — `docs/cli.md` 已含 `--proxy-user`，`access.userpass.*` 详细字段说明仍待补)
- 更新管理端访问控制说明，补充最近连接时间字段语义 (planned, not yet shipped as of 2026-06-16)
- 若 Web 设置页新增入口，同步更新相关截图或操作说明 (planned, not yet shipped as of 2026-06-16)

## 推荐结论

推荐采用“IP 访问控制 + 可选 userpass 补充鉴权”的组合方案，而不是新增 mode。

原因：

- 最符合当前需求表述
- 对现有部署兼容性最好
- 能同时覆盖 HTTP 与 SOCKS5
- 支持未来逐步扩展更多账号而不重做模型
- 复用现有 access control 与 SOCKS5 用户密码能力，改动边界清晰

如果你认同这个更新后的方向，下一步可以直接按完整首期范围进入开发：后端核心能力、Web/CLI 配置入口、最近连接时间记录同一批落地。
