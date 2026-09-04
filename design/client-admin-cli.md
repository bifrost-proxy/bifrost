# Bifrost Client 直连远程 Admin

## 1. 目标与边界

`bifrost client` 让当前 CLI 作为 Admin API 客户端，通过目标设备的 IP、端口或域名直接管理另一台正在运行的 Bifrost。它使用与远程 WebUI 相同的 `/_bifrost/api/*` 接口和 Admin Bearer JWT。

它与 `bifrost remote` 完全独立：

| 模式 | 入口 | 网络路径 | 鉴权 | 用途 |
| --- | --- | --- | --- | --- |
| 本地 | `bifrost traffic list` | 本机存储或 loopback Admin API | loopback | 管理当前机器 |
| Client | `bifrost client traffic list` | 直连目标 Admin API | Admin 用户名、密码与 JWT | 像 WebUI 一样查询和管理目标 Bifrost |
| Remote Invoke | `bifrost remote ...` | Relay 到 target worker | pairing、grant、端到端加密 | 远端 shell、文件和受授权调用 |

Client 不使用 Relay、pair code、SSH key、grant、Remote Invoke worker、远端 shell或文件 API。Client 命令不支持时必须明确失败，不得读取调用端的业务数据，也不得自动降级为 `remote exec`。

## 2. CLI 契约

### 2.1 目标管理

```bash
bifrost client target add devbox --url http://10.0.0.8:9900 --allow-insecure-http
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
bifrost client target list
bifrost client target show devbox
bifrost client target rename devbox lab
bifrost client target logout lab
bifrost client target remove lab
```

- `target add` 只保存 endpoint 和用户名，不接收密码。
- 明文 HTTP 必须显式传 `--allow-insecure-http`；它会让同网段观察者看到密码和 JWT，应优先使用 HTTPS、VPN 或 SSH tunnel。
- 裸 IP/host 默认规范化为 `http://<host>:9900`；也支持 `host:port`、完整 HTTP(S) URL、IPv4 和带方括号的 IPv6。
- URL 不允许 userinfo、query、fragment 或任意业务路径；`/_bifrost` 与 `/_bifrost/api` 会被规范化为 origin。
- `target login` 在 TTY 隐藏读取密码；非交互模式必须用 `--password-stdin`。密码不保存。
- `target logout` 只删除调用端保存的 JWT；服务端全量撤销使用 `client ... admin revoke-all`。

### 2.2 选择规则

```bash
# 只有一个保存目标时自动选择
bifrost client status --format json

# 多目标或需要明确身份时
bifrost client --target devbox traffic list --format json
bifrost client --target 10.0.0.8:9900 status
```

选择顺序如下：

1. 外层 `--target`；
2. `BIFROST_CLIENT_TARGET`；
3. 唯一保存的目标；
4. 多目标且 stdin/stderr 均为 TTY 时交互选择；
5. 多目标非 TTY 时拒绝并列出别名。

selector 先按别名匹配，再按 URL/authority 解析。未保存的临时地址必须显式使用 `--target`，并通过本次进程的 `BIFROST_ADMIN_TOKEN` 提供 token。环境 token 不会在省略 `--target` 时隐式绑定到某个 profile。

原业务命令的参数和 stdout schema 保持不变。TTY 下的 `Target: <name> (<origin>)` 提示只写 stderr；非 TTY 不输出目标提示。

### 2.3 Envelope 实现

`ClientInvocation` 保存 `--target` 和尾部 `Vec<OsString>`，再用根 `Cli` 的同一套 Clap 定义解析原业务命令，避免复制命令树。解析完成后先经过显式 capability allowlist，再派生当前可执行文件并注入仅供内部使用的 Client endpoint、token 和目标名环境变量。子进程继续走原命令 dispatch，因此参数和格式化行为与本地命令一致。

Client 子进程不得读取本机 runtime port 来决定目标；相关 handler 必须先检查 Client 上下文并只调用目标 Admin API。Client envelope 和子进程不初始化本机文件日志，避免把凭据或远端操作写入调用端实例日志。

## 3. 配置与凭据

调用端状态位于当前 `BIFROST_DATA_DIR`：

- `cli/admin-targets.toml`：目标 ID、别名、规范化 origin、用户名和 HTTP 风险确认；
- `cli/admin-credentials.toml`：按目标 ID 保存 Admin JWT 与服务端返回的到期时间。

两份文件均采用同目录临时文件加 rename 的方式原子写入。在 Unix 上目录权限为 `0700`，文件权限为 `0600`。密码、Authorization header 和 token 不进入普通 target profile、argv 或输出。

V1 暂未接入系统 Keychain，JWT 仍是磁盘上的敏感信息。文件权限降低了其他本机用户读取的风险，但不能抵御同一用户权限下的恶意进程。后续可迁移到系统凭据库，profile 的稳定 target ID 保证迁移和 rename 不丢失关联。

登录前先请求 `/auth/status`。目标未执行本地 `bifrost admin remote enable` 时，Client 明确提示必须在目标机本地开启，不能通过未认证通道自举。登录成功后保存服务端返回的 token 和 `expires_at`。本地发现 token 缺失或过期时要求重新登录；服务端返回 401 时给出 `bifrost client target login <target>` 提示。V1 不缓存密码，也不自动交互重登或自动重放请求。

## 4. 网络安全

- 所有 Client HTTP agent 忽略代理环境，避免请求回流当前 Bifrost。
- 重定向关闭，避免 Bearer token 被转发到其他 origin。
- Bearer token 只通过 `Authorization` header 发送，不使用 query、Cookie 或 argv。
- HTTPS 使用运行环境的正常证书校验；V1 不提供 Client 专用的跳过校验或自定义 CA profile。
- mutation 在超时、5xx 或连接中断后不自动重试，避免重复副作用。
- 401 只报告并要求显式重登；403 与业务错误不会改走本机或 Remote Invoke。

## 5. V1 能力矩阵

### 5.1 可进入 Client 模式

| 命令族 | V1 行为 |
| --- | --- |
| `status` | 通过 `/system/overview` 查询；`--tui` 暂不支持 |
| `traffic` | list/get/batch/body/sequence/auth-status/export/replay 走目标 API |
| `search` | metrics probe、SSE 搜索和详情/body 请求走目标 API |
| `capture` | authenticated long-poll/wait 走目标 API |
| `metrics` | 复用目标 metrics API |
| `rule` | list/show/add/update/delete/enable/disable/active/rename/reorder/share 走目标 API；`rule sync` 拒绝 |
| `group`、`port` | 查询和 mutation 均走目标 API |
| `value` | list/show/add/update/delete/import 走目标 API |
| `script` | list/show/add/update/delete/enable/disable/rename 走目标 API；show 需显式 type，`script run` 拒绝 |
| `config`、`whitelist`、`account` | 查询和 mutation 均走目标 API |
| `admin` | remote status/disable、passwd、revoke-all、audit 走目标 API；remote enable 必须目标机本地执行 |
| `login`、`sync` | 操作目标实例自身的 Sync 登录与状态，不等于 Client Admin 登录 |
| `import`、`export` | 调用端读写文件，payload 通过目标 Admin API |
| `version-check` | 只查询目标 API；失败不回退调用端 GitHub |

### 5.2 在发请求前拒绝

- nested `client` 与全部 `remote` 命令；
- `start`、`stop`、`restart`；
- `status --tui`、`rule sync`、`script run`；
- `cli-proxy`、`system-proxy`、`keep-awake`、`upgrade`、`completions`、`install-skill`、`app`、`ca`；
- `setting`、`ai`、`im`、`agent`；
- self-update handoff 和 hidden worker 命令。

拒绝是 V1 的明确安全边界，不表示改用本机执行。未来命令只有在 Admin API、鉴权、幂等性和真实链路测试齐备后才能加入 allowlist。

## 6. Agent Skill 路由

通用 `SKILL.md` 持有 Client 工作流；`skill_remote.md` 只负责 Remote Invoke。Agent 必须先按意图选模式：

| 用户意图 | 选择 |
| --- | --- |
| 已知 IP/端口/域名，像 WebUI 一样查流量、改规则或配置 | 通用 `bifrost` skill + `bifrost client` |
| 读写目标机任意文件、改远端仓库、执行 shell 或构建 | `bifrost-remote` skill + `bifrost remote` |
| 服务未运行，需要启动进程或做 OS/VCS 操作 | 明确授权的 Remote Invoke 或目标机 service manager |

Client 失败后禁止自动降级为 Remote Invoke；Remote Invoke 也不读取 Client target 或 Admin JWT。

## 7. 验证方案

### 自动测试

- 单元测试覆盖 URL 规范化与非法组件、Client/Remote 嵌套拒绝、公开命令 allowlist，以及通用 Admin client 的 Bearer 注入和跨 origin redirect 禁止。
- `e2e-tests/tests/test_client_admin_cli.sh` 使用临时数据目录、动态非 `9900` 端口、tray/Sync 弹窗护栏和 `--no-system-proxy` 启动目标。
- E2E 必须从非 loopback 局域网 IP 登录，并验证：单目标自动选择、多目标非 TTY 拒绝、显式 target、凭据文件权限、rule/value/script/whitelist/account/config/metrics/sync/port、traffic、SSE search、capture、local-only/Remote Invoke 拒绝、revoke/login/logout。
- install-skill 测试断言安装后的通用 skill 包含 Client target/login/管理流程，remote skill 包含两种远程模式的选择边界。

### 真实场景

`human_tests/client-admin-cli.md` 记录并驱动真实局域网链路，至少验证 Admin bootstrap、目标保存与登录、代表性查询和写入、SSE/long-poll、目标选择、401 恢复提示，以及 Client/Remote Invoke 隔离。

### Rust 门禁与交付

生产 Rust 变更执行相关单元测试、Client E2E、`cargo fmt --all -- --check`、workspace clippy、workspace tests 和 `make coverage-changed`。本地不运行无差别全量 CI；完整平台矩阵和覆盖率总门禁交给远端 CI。完成两轮 Review/Fix/Test 后提交、推送、更新 PR，并用 fail-fast 流程看护 CI 到全绿。

## 8. 已知限制与后续方向

- V1 凭据是权限受限文件，不是系统 Keychain。
- V1 不自动重登，401 后需要显式执行 target login。
- V1 覆盖 REST、SSE、search 交互界面和 capture long-poll；未迁移 WebSocket、status TUI、IM、Agent、AI。
- V1 通过显式 allowlist 管理 capability，未实现服务端 capability negotiation 或统一 `ExecutionContext` trait。
- V1 沿用现有 CLI 的通用失败退出码，尚未提供按 transport/auth/permission/capability 分类的稳定退出码。
- 高风险命令沿用各原命令当前确认语义，尚未形成 Client 专用的统一确认框架。
