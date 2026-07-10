# 代理 `user:password` 鉴权兼容方案

## 背景

Bifrost 原始访问控制是纯 IP 维度的 `ClientAccessControl`，只有 `allow_all` / `local_only` / `whitelist` / `interactive` 四种模式；SOCKS5 内部虽然实现了 RFC1929 用户名密码握手，但没有和 access control 联动，也不覆盖 HTTP 代理。多个真实场景下用户既想保留白名单/交互式审批，又想让远端可信客户端通过用户名密码继续接入，因此需要一套“IP 授权 + `user:password` 授权”的叠加式补充能力，同时不改变未配置该能力时的既有行为。

真实实现状态（截至 2026-07-10）：核心配置模型、HTTP 407 鉴权、SOCKS5 IP + userpass 组合、运行时 `last_connected_at`、admin API、Web 设置页、CLI `start --proxy-user`、低层 `config access.userpass.*` 与账号级 `account` 子命令均已落地；持久化配置中的账号密码会以本机设备指纹派生密钥加密落盘，运行时仍解密为现有 `UserPassAccountConfig.password` 供鉴权逻辑使用。

## 用户目标验证清单

### 必须实现

- 保留现有基于来源 IP 的访问控制；`user:password` 是可选补充，不是新的单选 mode。
- 当客户端 IP 未通过现有访问控制时，允许通过用户名密码继续完成授权。
- HTTP 代理与 SOCKS5 共用同一套 `access.userpass.accounts` 配置，不再分裂到 `server.socks5_auth`。
- 支持多账号并行生效，`username` 唯一且大小写敏感。
- Web 设置页与 CLI 首期同时支持配置、启停、查看账号。
- 记录每个账号最近一次成功鉴权时间 `last_connected_at`，管理端可见。
- 未配置该能力时，行为与旧版本完全一致。
- `loopback_requires_auth=false`（默认）时本机免密；`=true` 时本机也需要鉴权。
- CLI 提供账号级管理入口：`bifrost account list/add/update/remove/enable/disable/set-loopback-auth`，避免用户手写 `access.userpass.accounts` JSON。
- `config.toml` 不保存明文账号密码；读取配置时自动解密，保存配置时自动加密或迁移旧明文。

### 必须不破坏

- 转发到目标站点前必须清洗下游客户端带过来的 `proxy-authorization`，避免凭证泄露到业务目标。
- 命中 `proxy://user:pass@upstream` 上游代理规则时，仍由上游代理发送逻辑重新生成面向上游代理的 `Proxy-Authorization`。
- SOCKS5 在 IP 已通过时继续 `NoAuth`；只在 IP 未通过且 userpass 启用时选择 `UsernamePassword`。
- 管理端只回显 `has_password`，永远不回显明文 password。
- 更新配置后新请求立即按新配置校验；已有 CONNECT/SOCKS 通道不强制中断。
- Interactive 模式下带正确凭证直接放行，不进入 pending。
- 账号加密只用于本机静态文件防护，不承诺抵御同用户权限下可执行代码读取进程内存或调用本机 API。

### 必须真实验证

- Rust E2E `crates/bifrost-e2e/src/tests/userpass_auth.rs` 覆盖 HTTP/SOCKS5 正/反面。
- Shell E2E `e2e-tests/tests/test_userpass_loopback_e2e.sh` 覆盖 loopback 免密与强制鉴权切换。
- 单元测试 `crates/bifrost-storage/src/state.rs test_userpass_last_connected_at_crud` 覆盖时间戳 CRUD。
- CLI `bifrost start --proxy-user demo:secret` 与 `bifrost config access.userpass.*` 真实可用。

## 产品语义

### 叠加式授权

```text
Allow = IpAccessAllowed(unless loopback_requires_auth) OR AnyEnabledCredentialAuthenticated
Fallback = existing Interactive / Deny behavior
```

优先级：

1. loopback 默认直接放行（`loopback_requires_auth=false`）。
2. `loopback_requires_auth=true` 时 loopback 也需通过用户名密码。
3. 现有 IP 访问控制通过则直接放行，不消耗账号 `last_connected_at`。
4. 若配置了 userpass，则允许任一启用账号的用户名密码完成授权，成功时刷新该账号 `last_connected_at`。
5. 若仍未通过且 mode=`interactive`，进入 pending。
6. 其他情况拒绝（HTTP 返 407，SOCKS5 按协议拒绝）。

### 未配置零行为变化

`access.userpass.enabled=false` 或整段缺省时，`ClientAccessControl` 走旧路径，`server.socks5_auth` 保持原语义（不建议使用，未来标记 deprecated）。

## 技术细节

### 配置模型（shipped）

```toml
[access]
mode = "interactive"
allow_lan = false
whitelist = ["10.0.0.0/8"]

[access.userpass]
enabled = true
loopback_requires_auth = false

[[access.userpass.accounts]]
username = "demo"
password = "bifrost-local-secret:{\"version\":1,\"nonce\":\"...\",\"ciphertext\":\"...\"}"
enabled = true

[[access.userpass.accounts]]
username = "ops"
password = "another-secret"
enabled = true
```

Rust 类型（`crates/bifrost-core/src/access_control.rs`）：

```rust
pub struct UserPassAccountConfig {
    pub username: String,
    pub password: Option<String>,
    pub enabled: bool,
}

pub struct UserPassAuthConfig {
    pub enabled: bool,
    pub accounts: Vec<UserPassAccountConfig>,
    pub loopback_requires_auth: bool,
}

pub struct AccessControlConfig {
    pub mode: AccessMode,
    pub whitelist: Vec<String>,
    pub allow_lan: bool,
    pub userpass: Option<UserPassAuthConfig>,
}
```

约束：`username` 唯一（大小写敏感）、`password` 只写不读、账号可禁用但保留、对外接口只返回 `enabled/username/has_password/last_connected_at`。旧版明文 `password = "secret"` 仍能被读取；下一次 `ConfigManager` 保存配置时会改写为 `bifrost-local-secret:` envelope。

### 本机加密落盘（shipped）

`crates/bifrost-storage/src/local_secrets.rs` 使用 AES-256-GCM envelope 加密本机配置 secret：

- key 派生材料：固定 domain separator、`BIFROST_DATA_DIR` 对应 data dir、hostname、用户环境变量、常见 machine-id 文件内容。
- 稳定性：同一用户、同一 data dir、同一设备指纹材料不变时可稳定解密；更换设备或迁移 data dir 后需要重新设置账号密码。
- 格式：`bifrost-local-secret:{"version":1,"nonce":"base64","ciphertext":"base64"}`。
- 边界：这是防止其他程序“只读配置文件即可拿走明文密码”的静态防护；不替代 OS Keychain，不防同权限进程主动调用 Bifrost API 或读取运行中进程内存。
- 接入点：`ConfigManager::save_config` 写文件前加密副本，`ConfigManager::new` 加载配置后解密到内存；Admin API、WebUI、`bifrost account`、`bifrost config access.userpass.*` 都共享这一边界。

### Accept 阶段 deferred decision（shipped）

`crates/bifrost-proxy/src/server.rs` 中：

- accept 阶段得到 `initial_access_decision`。
- Allow → 保持现状。
- Deny/Prompt 且未启用 userpass → 保持现状。
- Deny/Prompt 且启用 userpass → 允许进入协议处理链路，交给 HTTP / SOCKS5 再判定。

### HTTP `Proxy-Authorization` 链路（shipped）

在 `handle_request()` 中，真实代理流量进入 admin/proxy 路由前做 Basic 校验；admin path、public cert path、loopback 管理流量豁免。校验结果：

```rust
enum CredentialCheckResult {
    Passed,
    Missing,
    Invalid,
    NotConfigured,
}
```

策略：

- `Missing` / `Invalid`：返回 `407 Proxy Authentication Required`，`Proxy-Authenticate: Basic realm="Bifrost"`，body 提示可通过审批或提供代理用户名密码。
- Interactive 模式首次失败可补充 pending。
- 成功后记录命中的 `username`，刷新 `last_connected_at`。

（helper 枚举名以实际代码为准，未必叫 `CredentialCheckResult`）。

### 转发前 header 清洗（shipped）

`crates/bifrost-proxy/src/server.rs` 中对 `proxy-authorization` header 显式清洗，避免透传到目标站点。唯一例外：命中 `proxy://user:pass@upstream` 规则时，由上游代理发送逻辑（`build_upstream_proxy_auth_value`，`crates/bifrost-proxy/src/proxy/http/handler.rs:437`）重新生成。

### SOCKS5 链路（shipped）

`crates/bifrost-proxy/src/proxy/socks/tcp.rs` 根据 `requires_userpass_auth()` 选择 `UsernamePassword` 或 `NoAuth`：

- IP 通过 → `NoAuth`。
- IP 未通过 + userpass 启用 → `UsernamePassword`；失败按 mode 决定 pending / 拒绝；成功刷新 `last_connected_at`。

### 运行时状态（shipped）

落到 `state.json` 而不是配置：`crates/bifrost-storage/src/state.rs`

```rust
pub userpass_last_connected_at: HashMap<String, u64>,
```

API：

- `set_userpass_last_connected_at(username, ts)`：只有鉴权成功且账号存在时才写。
- `replace_userpass_last_connected_at(map)`：账号列表变更后清理孤儿。
- `remove_userpass_last_connected_at(username)`：删除或改名时清理。

单测：`test_userpass_last_connected_at_crud`（`crates/bifrost-storage/src/state.rs:278`）。

### 热更新（shipped）

`ClientAccessControl` 已有 generation。userpass 配置纳入同一代际：账号列表 / username / password / 启停变更后后续请求立即按新配置校验；`last_connected_at` 更新不参与 generation 递增。已建立 tunnel 不断连，keep-alive 上后续请求按最新配置。

## CLI+Web+Admin API

### Admin API（shipped）

- `GET /api/whitelist`：响应扩展 `userpass` 字段：

```json
{
  "mode": "interactive",
  "allow_lan": false,
  "whitelist": ["10.0.0.0/8"],
  "temporary_whitelist": [],
  "userpass": {
    "enabled": true,
    "loopback_requires_auth": false,
    "accounts": [
      { "username": "demo", "enabled": true, "has_password": true, "last_connected_at": "2026-04-02T12:34:56Z" },
      { "username": "ops",  "enabled": false, "has_password": true, "last_connected_at": null }
    ]
  }
}
```

- `PUT /api/whitelist/userpass`：新增/更新/删除账号、切换 loopback；handler 位于 `crates/bifrost-admin/src/handlers/whitelist.rs:398 handle_set_userpass`。
- push scope 复用 `whitelist_status`，`crates/bifrost-admin/src/push.rs` 已暴露 `userpass` 字段。

### Web（shipped）

- `web/src/pages/Settings/tabs/AccessControlTab.tsx` 新增 “User/Password Auth” 区域。
- Store：`web/src/stores/useWhitelistStore.ts`。
- 展示：enabled 开关、loopback toggle、账号列表（username/password/enabled/last_connected_at）。
- 密码只写不显；`has_password=true` 显示为 `••••••`。

### CLI（shipped）

- `bifrost start --proxy-user USER:PASS`（可重复）：临时启用一批账号。
- `bifrost account list [--json]`
- `bifrost account add USER --password-stdin [--enable-auth]`
- `bifrost account update USER [--password-stdin] [--enable|--disable]`
- `bifrost account remove USER`
- `bifrost account enable` / `bifrost account disable`
- `bifrost account set-loopback-auth true|false`
- `bifrost config access.userpass.enabled true|false`
- `bifrost config access.userpass.loopback-requires-auth true|false`
- `bifrost config set access.userpass.accounts '[{"username":"demo","password":"secret","enabled":true}]'`（低层 JSON 入口，推荐脚本外使用 `bifrost account`）
- `bifrost config get/export/show`：仅回显 `enabled/accounts[]/username/enabled/has_password/last_connected_at`。

## Sync 边界

- 账号明文密码不参与云端 sync（`crates/bifrost-storage/src/config_manager.rs` 中的 sync-exclusion 处理）。
- `last_connected_at` 属于本机运行时状态，不同步。
- 若未来接入配置 sync，`access.userpass.accounts` 需要单独设计密码加密与冲突合并策略，本期不做。

## Phase 1 —— 首期完整落地（已 shipped）

- 扩展 `access` 配置模型与 access 状态响应结构。
- accept 阶段支持 deferred decision。
- HTTP 407 + `Proxy-Authorization: Basic` 校验。
- SOCKS5 IP + userpass 组合。
- 转发前清洗 `proxy-authorization`。
- 运行时 `last_connected_at` 写 `state.json`。
- Admin API `GET /api/whitelist` / `PUT /api/whitelist/userpass`。
- Web Access Settings 新增入口。
- CLI `start --proxy-user` / `config access.userpass.*`。
- push 通道补充 userpass 字段。

## Phase 2 —— 文档与 README（planned）

- `README.md` / `README.en.md` 补充 HTTP 代理用户名密码鉴权说明。
- `docs/cli.md` 已含 `--proxy-user`，需补 `access.userpass.*` 详细字段。
- 管理端访问控制说明补 `last_connected_at` 字段语义。
- Web 设置页截图或操作说明同步更新。

## Phase 3 —— 后续演进（out of scope）

- 密码加密存储（当前为明文写配置，仰赖 fs 权限）。
- 账号导入/导出、SSO/OAuth/Token/2FA。
- 账号维度的速率限制与审计日志分级。

## Phase 4 —— 兼容性维护

- `server.socks5_auth` 保留至少一个版本以兼容旧配置，读取时优先 `access.userpass`。
- 未来若引入 `access.userpass.accounts` 密码 hash，需要写迁移代码并兼容旧明文。

## 测试方案

### 单元测试

- `crates/bifrost-core/src/access_control.rs`：`AccessMode + userpass` 组合语义、多账号命中顺序、username 唯一性、HTTP Basic 解析（含非法格式）。
- `crates/bifrost-proxy/src/server.rs`：转发前 header 清洗（`proxy-authorization` 不泄露）。
- `crates/bifrost-proxy/src/proxy/socks/tcp.rs`：IP 未命中时选择 UsernamePassword、IP 命中时选择 NoAuth。
- `crates/bifrost-storage/src/state.rs`：`test_userpass_last_connected_at_crud`。
- `crates/bifrost-admin/src/handlers/whitelist.rs`：`validate_userpass_request` 保留旧密码、拒绝空 username、拒绝重复 username。

### E2E

Rust E2E `crates/bifrost-e2e/src/tests/userpass_auth.rs`：

- `test_http_correct_credentials`（`:74`）
- `test_http_wrong_credentials`（`:104`）
- `test_http_no_credentials`（`:133`）
- `test_http_multi_accounts`（`:161`）
- `test_http_disabled_account`（`:212`）
- `test_socks5_correct_credentials`（`:241`）
- `test_socks5_wrong_credentials`（`:271`）

Shell E2E `e2e-tests/tests/test_userpass_loopback_e2e.sh`：

- `test_userpass_config_api`
- `test_loopback_no_auth_default`
- `test_loopback_with_auth_also_works`
- `test_loopback_requires_auth_on_returns_407_without_creds`

真人回归 `human_tests/proxy-auth-brute-force.md` 覆盖失败限流场景；`human_tests/api-whitelist.md` 覆盖 admin API + Web 配置。

## Review/Fix/Test 闭环

### 第 1 轮

- 确认 accept-阶段 deferred decision 只在 userpass 启用时生效，未启用路径与旧实现一致。
- 确认 `proxy-authorization` 清洗覆盖 HTTP、HTTPS CONNECT、WebSocket 升级三条链路。
- 复测：`userpass_auth.rs` Rust E2E + shell E2E + state.rs 单测。

### 第 2 轮

- 复核 Web 设置页密码明文不出网络回包，push 通道 `userpass` 字段无 password。
- 复核 CLI `config get/export/show` 不回显明文密码。
- 复测：`cargo test --workspace --all-features` + `rust-project-validate`。

## 风险与决策

- **协议分流前放宽**：仅在 IP 未通过且 userpass 启用时启用 deferred 路径，其他情况保持旧的 accept-time 立即拒绝，避免整体连接接入面被扩大。
- **下游凭证泄露**：统一转发前清洗；单元测试锁死 `proxy-authorization` 不在 upstream 出现。
- **明文密码回显**：所有读接口只返回 `has_password`；push、CLI show/export、Web 都不返回密码。
- **多账号重名**：写入强制 username 唯一；运行时状态以 username 为键，避免额外内部 id。
- **`last_connected_at` 写盘频率**：仅在鉴权成功时写，去抖同一秒重复成功只写一次；使用 `state.json` 而非 `config.json`，避免主配置被高频改写。
- **`server.socks5_auth` 冲突**：优先 `access.userpass`；若两者同时存在，日志 warn 并以 `access.userpass` 为准。

## 校验要求

- `cargo test -p bifrost-storage state -- --nocapture`
- `cargo test -p bifrost-core access_control -- --nocapture`
- `cargo test -p bifrost-admin whitelist -- --nocapture`
- Rust E2E：`cargo run -p bifrost-e2e -- --test userpass_auth::test_http_correct_credentials`（其他用例同名替换）
- Shell E2E：`bash e2e-tests/tests/test_userpass_loopback_e2e.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 文档更新要求

- `README.md` / `README.en.md` 补充 HTTP 代理支持 `user:password`（planned）。
- `docs/cli.md` 已含 `--proxy-user`，补 `access.userpass.*` 完整字段（partially shipped）。
- 管理端访问控制说明补 `last_connected_at` 字段语义（planned）。
- Web 设置页新增入口截图或操作说明（planned）。
- `human_tests/api-whitelist.md`、`human_tests/proxy-auth-brute-force.md` 与本方案保持同步。

## 推荐结论

推荐采用“IP 访问控制 + 可选 userpass 补充鉴权”的叠加式方案，而非新增 mode。已在生产分支落地：后端核心能力、Web/CLI 配置入口、账号最近连接时间同一批交付；剩余只是 README 与操作文档补齐。
