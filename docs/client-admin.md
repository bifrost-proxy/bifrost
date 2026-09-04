# Client 远程管理使用手册

> Language: **中文** | [English](../docs-en/client-admin.md)

`bifrost client` 可以通过目标设备的 IP、端口或域名直连另一台正在运行的 Bifrost，并使用与远程 WebUI 相同的 Admin API 查询流量、管理规则和修改配置。它适合已知目标地址、希望像操作本机 CLI 一样管理远端 Bifrost 的场景。

## 适用场景

| 需求 | 推荐方式 |
| --- | --- |
| 已知目标 IP 或域名，查询流量、规则、配置、端口和运行状态 | `bifrost client` |
| 在浏览器中管理目标 Bifrost | 远程 WebUI |
| 读写目标机任意文件、修改仓库、执行 shell 或管理进程 | `bifrost remote` |
| 管理当前机器上的 Bifrost | 普通 `bifrost status`、`bifrost traffic ...` 等本机命令 |

Client 不使用 Relay、pair code、SSH key 或 Remote Invoke grant。Client 命令失败或不受支持时会直接报错，不会读取本机业务数据，也不会自动降级到 `bifrost remote`。

## 快速开始

### 1. 在目标机启用远程 Admin

以下命令必须在目标机本地执行。先设置 Admin 密码，再启用远程访问：

```bash
bifrost admin passwd
bifrost admin remote enable
bifrost admin remote status
```

`remote enable` 只开放远程 WebUI 和 Admin API，不等同于启用 Remote Invoke。为了避免未认证客户端自行扩大权限，不能通过 `bifrost client` 远程执行首次启用。

确认目标 Bifrost 的监听地址能被调用端访问。默认端口是 `9900`；如果目标使用其他端口，请在后续 URL 中填写实际端口。

### 2. 在调用端保存目标

优先使用证书有效的 HTTPS 地址：

```bash
bifrost client target add devbox --url https://bifrost.example.com
```

在可信局域网内使用明文 HTTP 时，必须显式确认风险：

```bash
bifrost client target add devbox \
  --url http://10.0.0.8:9900 \
  --allow-insecure-http
```

明文 HTTP 会暴露 Admin 密码和 JWT 给能够观察网络流量的人。跨不可信网络时应使用 HTTPS、VPN 或 SSH tunnel。

### 3. 登录目标

交互式终端会隐藏密码输入：

```bash
bifrost client target login devbox --username admin
```

自动化场景应从标准输入读取密码，避免把密码放进命令行参数或 shell 历史：

```bash
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
```

Client 只保存登录成功后返回的 Admin JWT 和过期时间，不保存密码。

### 4. 执行远程管理命令

只有一个已保存目标时，可以省略 `--target`：

```bash
bifrost client status --format json
bifrost client traffic list --limit 20
```

有多个目标或希望明确操作对象时，显式选择目标：

```bash
bifrost client --target devbox status --format json
bifrost client --target devbox traffic list --format json
bifrost client --target devbox rule list
```

`--target` 必须放在业务命令之前。业务命令的参数和 stdout 格式与本机命令保持一致，因此现有 JSON/NDJSON 脚本通常只需增加 `client --target <name>` 前缀。

## 管理目标

```bash
bifrost client target list
bifrost client target show devbox
bifrost client target rename devbox lab
bifrost client target logout lab
bifrost client target remove lab
```

- `target add` 只保存地址和默认用户名，不接收密码。
- `target logout` 只删除调用端保存的 JWT，不会撤销其他浏览器或 CLI 的会话。
- `target remove` 会同时删除该目标及其本地会话。
- 如需让目标上的全部 Admin 会话失效，执行 `bifrost client --target lab admin revoke-all`。

目标地址可以是 IP、`host:port` 或完整 HTTP(S) URL。裸 host 默认使用 `http://<host>:9900`。URL 不能包含用户名密码、query、fragment 或业务路径；末尾的 `/_bifrost`、`/_bifrost/api` 会自动规范化为服务 origin。

## 目标选择

Client 按以下顺序选择目标：

1. 命令行 `--target <name-or-address>`；
2. 环境变量 `BIFROST_CLIENT_TARGET`；
3. 唯一保存的目标；
4. 多目标交互终端中的选择列表；
5. 多目标非交互环境中拒绝执行，并列出可用目标。

可以为脚本固定默认别名：

```bash
export BIFROST_CLIENT_TARGET=devbox
bifrost client status --format json
```

也可以临时直连未保存的地址。此时必须显式传入地址，并通过当前进程的 `BIFROST_ADMIN_TOKEN` 提供 JWT：

```bash
BIFROST_ADMIN_TOKEN="$TOKEN" \
  bifrost client --target https://bifrost.example.com status --format json
```

环境 token 不会在省略 `--target` 时自动绑定到某个已保存目标。

## 常用工作流

### 查询流量和等待请求

```bash
bifrost client --target devbox traffic list --limit 50
bifrost client --target devbox traffic get <id> --request-body --response-body
bifrost client --target devbox search "invalid_request" --res-body
bifrost client --target devbox capture wait \
  --host api.example.com \
  --method POST \
  --timeout 30s
```

流量详情可能包含 Authorization、Cookie、JWT 和业务数据。共享输出前应先删除敏感字段。

### 管理规则、Values 和 Scripts

```bash
bifrost client --target devbox rule list
bifrost client --target devbox rule add debug-api \
  -c "api.example.com reqHeaders://X-Debug=1"
bifrost client --target devbox rule enable debug-api

bifrost client --target devbox value list
bifrost client --target devbox script list
```

这些 mutation 直接作用于目标实例。连接中断、超时或服务端 5xx 后 Client 不会自动重试，以免重复执行有副作用的操作；再次执行前请先查询目标状态。

### 查询配置和运行状态

```bash
bifrost client --target devbox status --format json
bifrost client --target devbox config show --json
bifrost client --target devbox metrics summary
bifrost client --target devbox port list
bifrost client --target devbox whitelist list
```

Client V1 还支持目标上的 `group`、`account`、`admin`、`login`、`sync`、`import`、`export` 和 `version-check` 等 Admin API 能力。运行 `bifrost client --target devbox <command> --help` 可查看业务命令参数。

## 安全说明

- HTTPS 使用系统正常证书校验；Client 不提供跳过证书校验的选项。
- Client 请求忽略 `HTTP_PROXY`、`HTTPS_PROXY` 和 `ALL_PROXY`，避免请求回流当前 Bifrost。
- HTTP 重定向被禁用，Bearer token 不会被转发到其他 origin。
- JWT 保存在当前 `BIFROST_DATA_DIR` 下的 `cli/admin-credentials.toml`；目标信息保存在 `cli/admin-targets.toml`。
- Unix 上凭据目录权限为 `0700`、文件权限为 `0600`。V1 尚未接入系统 Keychain，同一操作系统用户权限下的恶意进程仍可能读取凭据。
- 高风险写操作沿用对应本机命令的确认语义。执行前始终确认当前 target。

## 不支持的命令

Client V1 会在发请求前拒绝下列本机或不同 transport 的能力：

- `start`、`stop`、`restart` 和 `status --tui`；
- 嵌套 `client`、全部 `remote` 命令；
- `rule sync`、`script run`；
- `cli-proxy`、`system-proxy`、`keep-awake`、`upgrade`、`completions`、`install-skill`、`app`、`ca`；
- `setting`、`ai`、`im`、`agent`。

这些命令被拒绝后不会在调用端本地执行。如果需要操作目标机文件、shell、进程或仓库，请明确切换到 [Remote Invoke](./cli.md#远程调用remote)。

## 故障排查

### 提示远程 Admin 未启用

在目标机本地执行：

```bash
bifrost admin remote enable
bifrost admin remote status
```

同时确认目标服务监听地址、防火墙和网络路由允许调用端访问。

### 提示未登录、token 过期或返回 401

Client 不缓存密码，也不会自动重登或自动重放原请求。请显式重新登录，再重新执行原命令：

```bash
bifrost client target login devbox --username admin
bifrost client --target devbox status
```

### 多目标环境拒绝执行

交互终端可从列表选择；脚本和管道中必须使用 `--target` 或 `BIFROST_CLIENT_TARGET`，避免误操作另一台设备。

### 命令显示不支持 Client 模式

该能力尚未迁移到 Admin API 或属于本机/Remote Invoke 边界。Client 不会自动 fallback。根据错误提示改用目标机本地命令或显式使用 `bifrost remote`。
