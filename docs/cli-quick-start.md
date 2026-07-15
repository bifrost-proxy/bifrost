# CLI 快速开始

本文档不是完整命令清单，而是按“我现在想完成什么事”来选择 Bifrost CLI 路径。完整参数、所有子命令和变量说明见 [CLI 详细命令](./cli.md)；规则语法见 [规则语法](./rule.md)、[匹配器](./pattern.md)、[操作符说明](./operation.md)。

## 先选场景

| 使用场景 | 首选命令 | 关键判断 |
| --- | --- | --- |
| 本机排障或联调，不污染默认规则 | `bifrost port bind ...` | 优先复用主服务，用多端口隔离规则和流量；不要默认用临时数据目录另起服务 |
| 让命令、浏览器或应用走代理 | `bifrost start -d` / `curl -x ...` | 后台启动会按默认配置启用系统代理，浏览器和桌面应用无需额外配置；只影响单个命令时再显式指定代理 |
| 把线上域名转到本地服务 | `bifrost rule add ... host://...` | HTTP 可直接转发；HTTPS 路径匹配通常需要 TLS 拦截 |
| 改请求头、响应头、状态码或返回体 | `reqHeaders://` / `resHeaders://` / `statusCode://` / `file://` | 默认直接写内联 value 或规则文件内嵌值；特别大的内容才放全局 Values |
| 查某个请求为什么没命中规则 | `bifrost traffic list/get/search` | 先看 `matched_rules`、入口端口、请求 URL、协议 |
| 同一个服务支持多个应用或开发任务 | `bifrost port bind ...` | 一个常驻 Bifrost 共享证书、配置和流量库；每个任务用独立端口绑定独立规则 |
| 局域网或多人访问 | `--access-mode` / `whitelist` | 不要直接用 `allow_all` 暴露未授权代理 |
| 操作另一台机器的 Bifrost | `bifrost remote ...` | `setting` 总是本机；远端配置要通过 `remote exec` 在远端执行 |
| 备份、迁移或同步规则 | `import` / `export` / `sync` | `.bifrost` 文件用于离线迁移，`sync` 用于远端同步服务 |
| 添加飞书或微信 IM 通道 | `im provider add` | 终端展示授权链接或二维码，扫码后自动创建并连接 provider |
| 和 Agent 协作开发业务 Skill | `install-skill` + `traffic list/get/search` | 先抓完整或应用专属流量，再让 Agent 基于真实请求和响应沉淀 skill |

## 场景 1：使用主服务和多端口安全调试

日常排障优先复用当前 Bifrost 主服务，再用临时端口隔离本次规则和流量。这样不会把调试规则混进默认 enabled 规则，也不需要为每次任务创建新的数据目录。

如果还没有启动主服务，直接启动代理即可。默认启动会启用系统代理，让浏览器和桌面应用自然进入 Bifrost；默认不要开启全局 TLS 抓包，也不要用 `--unsafe-ssl` 跳过上游证书校验：

```bash
bifrost start -d
```

为本次调试单独绑定端口：

```bash
bifrost port bind --port 18888 --rule-text "debug.test statusCode://218 resBody://(debug)"
curl -x http://127.0.0.1:18888 http://debug.test/
```

这组命令的含义：

- `bifrost start -d`：后台启动主服务；普通用户默认走系统代理，浏览器和桌面应用不用单独配置。
- `port bind --port 18888`：只给本次调试开一个独立入口端口，调试结束后可直接销毁。
- `--rule-text`：传入一次性规则，不写入默认规则库。

默认监听 `0.0.0.0:9900`，TLS/HTTPS 拦截默认关闭；访问控制默认是 `interactive`：本机 loopback 直接允许，非本机地址需要通过管理端审批、白名单或 `--allow-lan` / `--access-mode` 显式放行。

常用服务动作：

```bash
bifrost status
bifrost status --tui
bifrost stop
bifrost restart
bifrost port list
bifrost port destroy 18888
```

普通用户首次体验不要加测试隔离参数。`--no-system-proxy` 和临时 `BIFROST_DATA_DIR` 只用于 CI、自动化测试、并发实验或破坏性配置验证。

## 场景 2：让流量进入 Bifrost

普通用户启动 `bifrost start -d` 后，系统代理会自动接管浏览器和桌面应用流量。单个命令也可以显式指定代理，这样最可控：

```bash
curl -x http://127.0.0.1:9900 http://httpbin.org/headers
curl -x http://127.0.0.1:9900 https://httpbin.org/headers
```

需要 TLS 抓包时，启动流程会自动准备 Bifrost CA。普通浏览器和桌面应用优先使用系统信任链；如果某个 CLI 工具不读取系统信任链，再用 `bifrost ca export -o ./bifrost-ca.pem` 导出证书并传给该工具。`-k` / `--unsafe-ssl` 不作为默认建议，只适合你明确要验证证书异常或临时诊断上游证书问题时使用。

如果系统代理被手动关闭过，可以这样重新启用或查看状态：

```bash
bifrost system-proxy status
bifrost system-proxy enable --host 127.0.0.1 --port 9900 --bypass "localhost,127.0.0.1,*.local"
bifrost system-proxy disable
```

`--no-system-proxy` 是测试/诊断选项，例如 CI、沙箱、只想用 `curl -x` 或应用内代理配置时才使用，不作为默认上手命令。

`--cli-proxy` 是另一条路径：它会写入 shell rc 文件，让终端命令继承 `http_proxy` / `https_proxy` 等环境变量。它不是系统代理，适合只影响命令行工具：

```bash
bifrost start --cli-proxy --cli-proxy-no-proxy "localhost,127.0.0.1,*.local"
```

## 场景 3：把某个域名转发到本地服务

HTTP 或不需要路径匹配的 HTTPS 透传场景，可以从最小 host 规则开始：

```bash
bifrost rule add local-api -c "api.example.com host://127.0.0.1:3000"
bifrost rule enable local-api
bifrost rule active
```

也可以启动时临时传入规则，不写入规则库：

```bash
bifrost start --rules "api.example.com host://127.0.0.1:3000"
bifrost start --rules-file ./rules.txt
```

简单值优先直接写在规则里，不要默认拆成全局 Values：

```bash
bifrost rule add local-api -c "api.example.com host://127.0.0.1:3000"
```

如果内容较长但仍属于这条规则，优先使用规则文件内嵌值，让规则和它依赖的内容一起迁移、一起审查：

````txt
``` local-api-target.txt
127.0.0.1:3000
```
api.example.com host://{local-api-target.txt}
````

只有特别大的 mock body、PAC 脚本、大段 header 块，或确实需要被很多规则长期共享的内容，才建议使用 `bifrost value set <NAME> <VALUE>` 创建全局 Values。`${env.API_TOKEN}`、`${path}`、`${reqHeaders.authorization}` 等是运行时模板变量，展开时依赖当前请求上下文。

## 场景 4：按需调试 HTTPS 和路径规则

如果你要按 HTTPS URL 的 path 匹配，例如 `api.example.com/v1/users https://10.0.0.10:8443`，只要规则 matcher 有明确域名/IP 作用域，Bifrost 会在全局 TLS 抓包关闭时自动对该域名解包，以便读取内部路径；matcher 前不需要强制写 `https://`。不要默认开启全局 TLS 抓包，优先用域名白名单、应用白名单或规则级 `tlsIntercept://` 精确控制范围。对范围很宽的 wildcard/regex，或没有明确 host 作用域的规则，请显式收窄范围，避免影响带 SSL pinning 的应用。

```bash
bifrost rule add https-path -c "api.example.com/v1/users tlsIntercept:// host://127.0.0.1:3000"
```

如果你希望启动配置层面只抓某个域名或某类应用：

```bash
bifrost start --intercept-include "api.example.com"
bifrost start --app-intercept-include "*Chrome,*curl"
```

遇到 SSL pinning 应用时，优先排除应用或域名，让它继续走 CONNECT 隧道：

```bash
bifrost start --app-intercept-exclude "*BankApp,*PinnedApp"
bifrost start --intercept-exclude "*.apple.com,*.microsoft.com"
```

TLS 抓包优先级从高到低：

1. 规则级 `tlsIntercept://` / `tlsPassthrough://`
2. `--intercept-include` / `--app-intercept-include`
3. `--intercept-exclude` / `--app-intercept-exclude`
4. `--intercept` / `--no-intercept`

域名或应用白名单示例：

```bash
bifrost start --intercept-include "*.api.local"
bifrost start --intercept-exclude "*.apple.com,*.microsoft.com"
bifrost start --app-intercept-include "*Chrome,*curl"
bifrost start --app-intercept-exclude "*Safari*"
```

## 场景 5：改请求、响应或模拟异常

规则快速入门：

```txt
pattern operation [operations...] [filters...] [lineProps://...]
```

常见操作：

```txt
api.example.com reqHeaders://X-Debug=1
api.example.com resHeaders://X-Proxy=Bifrost
api.example.com statusCode://503
api.example.com file://({"error":"mocked"})
api.example.com includeFilter://m:POST reqHeaders://X-Method=post
```

常见 pattern：

```txt
example.com
example.com/api
192.168.1.1
192.168.0.0/16
/pattern/i
*.example.com
!*.internal.example.com
```

过滤器和规则属性：

```txt
example.com host://127.0.0.1:3000 includeFilter://m:GET
example.com host://127.0.0.1:3000 excludeFilter:///admin/
example.com host://127.0.0.1:3000 lineProps://important
example.com host://127.0.0.1:3000 lineProps://disabled
```

更多 operation 的值格式和模板变量见 [操作符说明](./operation.md)。

## 场景 6：从流量记录定位问题

先看最近流量，再看单条或批量详情。本期不做 Authorization、Cookie、JWT token 等敏感信息脱敏，以下输出按捕获原文返回：

```bash
bifrost traffic list
bifrost traffic list --method GET --status-min 400 --limit 100
bifrost traffic get <id> --request-body --response-body
bifrost traffic get --ids 12,13,14 --request-body --response-body --format ndjson
bifrost traffic auth-status <id>
```

如果你要先启动等待，再在浏览器或桌面应用里触发一次操作，用 `capture wait` 等下一条匹配记录。超时退出码为 `124`，适合脚本判断没有抓到目标请求：

```bash
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s
bifrost capture wait --host api.example.com --timeout 10s --format json
```

按关键词、结构字段和范围搜索：

```bash
bifrost search "Bearer " --req-header
bifrost search "cache-control" --res-header
bifrost search "invalid_request_error" --res-body
bifrost search "keyword" --host api.example.com --path /v1/users
bifrost search "" --host api.example.com --req-json '$.user.id=42' --include request-body,response-body
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m --include response-body
```

需要把捕获请求交给同事、脚本或 Agent 复现时，可以导出模板或基于原请求重放；导出内容包含捕获原文，传播前必须手动移除敏感信息：

```bash
bifrost traffic export <id> --as curl
bifrost traffic export <id> --as har -o ./request.har
bifrost traffic replay <id>
bifrost traffic replay <id> --patch '/json/debug=true'
```

如果你在调试临时端口，记得带入口端口过滤：

```bash
bifrost traffic list --listener-port 18888
bifrost search "keyword" --proxy-port 18888
```

排查规则不生效时，优先检查：

- 请求是否真的经过 Bifrost。
- `traffic get <id>` 中的 URL、host、path、protocol 是否符合 pattern。
- HTTPS 请求是否需要 `tlsIntercept://` 才能看到 path。
- 入口端口是否是你绑定规则的临时端口。

## 场景 7：用临时端口隔离多组规则

临时端口适合同时调试多套规则，互不影响主端口：

```bash
bifrost start -d
bifrost port bind --port 18888 --rule local-dev
bifrost port bind --port 18889 --rule-text "debug.test statusCode://218 resBody://(debug)"
bifrost port list
bifrost port active 18888
bifrost port destroy 18888
```

临时端口共享同一个 `BIFROST_DATA_DIR` 中的 values、scripts、证书和流量库，但只使用 `port bind` / `port update` 显式绑定的规则。

## 场景 8：同一个服务服务多个应用或开发任务

多人协作、多个 App 联调、多个 Agent 任务并行时，不需要为每个任务启动一个 Bifrost。推荐保持一个主服务常驻，用不同临时端口承载不同任务的规则。这样证书、系统代理、Web UI、traffic 数据库都共享，但每个任务的规则入口相互隔离。

先启动一次主服务：

```bash
bifrost start -d
```

再给不同任务绑定不同端口：

```bash
bifrost port bind --port 18881 --rule-text "app-a.example.com tlsIntercept:// host://127.0.0.1:3001"
bifrost port bind --port 18882 --rule-text "app-b.example.com tlsIntercept:// host://127.0.0.1:3002"
bifrost port bind --port 18883 --rule team-shared-auth
```

让不同应用或任务走各自端口：

```bash
curl -x http://127.0.0.1:18881 https://app-a.example.com/api/me
curl -x http://127.0.0.1:18882 https://app-b.example.com/api/me
```

排查时按入口端口过滤流量，不要只按 host 猜：

```bash
bifrost traffic list --listener-port 18881 --limit 50
bifrost traffic list --listener-port 18882 --limit 50
bifrost port active 18881
bifrost port active 18882
```

这个模型的边界：

- 主服务负责系统代理、CA、Web UI、traffic 存储和默认 enabled 规则。
- 临时端口只使用 `port bind` / `port update` 显式绑定的规则；不会继承默认 enabled 规则。
- 不同任务需要共享同一套规则时，用 `--rule` 复用本地规则；只想快速试验时，用 `--rule-text`；规则较长时，用 `--rule-file`。
- Agent 协作时，把端口号写进指令，例如“只分析 `listener_port=18882` 的流量”，避免把其他应用流量混进结论。
- 任务结束后只销毁对应端口：`bifrost port destroy 18882`，不要停止整个主服务。

## 场景 9：管理访问范围

默认访问限制为本机。需要局域网访问时，优先使用白名单或交互审批：

```bash
bifrost start --access-mode whitelist --whitelist "192.168.1.100,10.0.0.0/8"
bifrost whitelist status
bifrost whitelist add 192.168.1.101
bifrost whitelist pending
bifrost whitelist approve <ip>
```

`allow_all` 会暴露代理能力，除非你明确知道网络边界和认证策略，否则不要用于共享网络。

## 场景 10：远程操作另一台 Bifrost

首次连接或重新授权：

```bash
bifrost setting ssh-key create --label "dev-mac" --output ./bifrost-device.key
bifrost remote conn up <pair-code>
bifrost remote conn up --ssh-key ./bifrost-device.key --label "dev-mac"
export BIFROST_REMOTE_SSH_KEY="$(cat ./bifrost-device.key)"
bifrost remote conn up --ssh-key --label "ci-agent"
bifrost remote conn status
```

`--ssh-key` 带路径时读取指定 key 文件；不带值时读取固定环境变量 `BIFROST_REMOTE_SSH_KEY`，适合 CI secret 和自动化环境。

远程查看状态、流量或文件：

```bash
bifrost remote exec -- bifrost status
bifrost remote traffic list --limit 20
bifrost remote file read README.md --cwd /path/to/repo
bifrost remote file find "TODO" --path src --cwd /path/to/repo
bifrost remote run --script-file ./smoke.py --interpreter python3 --cwd /path/to/repo -- --json
```

`remote exec` 是最高权限路径，能在远端运行 shell 命令；实际允许范围由远端 Shell Access policy 和 grant 决定。结构化文件访问优先使用 `bifrost remote file ...`。

多台远端同时可用时，把 `--client-id` 放在 `remote` 后、子命令前；也可以用 `BIFROST_REMOTE_CLIENT_ID` 设置默认目标：

```bash
export BIFROST_REMOTE_CLIENT_ID=devbox
bifrost remote file read Cargo.toml --cwd /repo
bifrost remote --client-id macbook run --script-file ./query.py --interpreter python3 --cwd /repo
```

`remote run` 会把本地脚本上传到远端 FileAccessPolicy 允许的 scratch 目录再执行，适合替代 `exec + heredoc`。长时间静默的 `--wait` / 轮询型命令使用 `--detach` 后接 `remote job watch <call_id> --output-file <log>`，避免 300 秒无事件 idle timeout 或 caller shell 超时。

注意：`bifrost setting ...` 总是管理当前机器。要改远端设置，需要在远端执行：

```bash
bifrost remote exec -- bifrost setting shell list
```

## 场景 11：备份、迁移、同步和补全

离线迁移用 `.bifrost` 文件：

```bash
bifrost export rules local-api -o ./rules.bifrost
bifrost export values -o ./values.bifrost
bifrost import ./rules.bifrost
```

远端同步服务：

```bash
bifrost sync status
bifrost login --token "$BIFROST_SYNC_TOKEN"
bifrost login --token "$BIFROST_SYNC_TOKEN" --url https://example.com
bifrost sync run
```

`bifrost login` 与 `bifrost sync login` 等价。Headless/CI 登录 token 可从 `https://bifrost.example.com/v4/sso/token-login` 获取；省略 `--url` 时使用当前同步配置的远端 URL，默认是内置 Bifrost Provider。

Shell 补全：

```bash
bifrost completions zsh
```

## 场景 12：添加飞书或微信 IM 通道

IM Gateway 适合把飞书或微信变成 Bifrost 的 Agent 会话入口。首次添加 provider 时先确认 Bifrost 服务已启动，再用 `im provider add` 进入交互式配置：

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu 会在终端显示授权 URL 和二维码；Weixin 会显示登录二维码。CLI 会保持等待，直到用户完成授权或扫码确认，然后自动创建并连接 provider。`--runner` 用于绑定这个 IM 通道默认使用的 Agent Runner；交互式终端里可以省略后通过上下键选择，脚本或管道环境必须显式传入。Runner 写错时错误信息会列出当前可用 Runner。Feishu / Weixin 的 `base_url` 由 provider 类型固定，CLI 不接受 `--base-url`。

如果你已经有 App ID 和 Secret，可以继续走手动凭据方式，并用 `env:NAME` 避免把 secret 写进 shell history：

```bash
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --runner "Claude Code"
```

IM 通道连接成功后会收到上线通知和可用命令帮助。所有外部 Runner 都有 `/help`、`/status`、`/cwd`、`/runner`、`/q`、`/rq`、`/stop`；Codex、Traex、Claude Code 等 Runner 还会按适配器能力显示模型和 reasoning effort 命令。

Agent skill 的完整协作流程见下一节。

## 场景 13：和 Agent 协作开发业务 Skill

这个场景适合把一个真实系统、内部平台或桌面应用的网络协议沉淀成专属 Agent skill。核心思路是：先让 Bifrost 记录可复现的真实请求和响应，再让 Agent 读取这些证据，总结认证方式、接口顺序、字段含义和错误格式，最后写成可复用的 skill。

第一步，给 Agent 安装 Bifrost 能力说明。这个命令只安装 skill 文档，不启动代理、不抓流量、不修改规则：

```bash
bifrost install-skill -t codex -y
```

如果你同时使用多个支持的 Agent，可以安装到默认可发现的位置：

```bash
bifrost install-skill -y
```

第二步，确认主服务已启动，并为本次采集绑定一个独立端口。需要抓 HTTPS 明文内容时，优先在规则里只对目标域名写 `tlsIntercept://`；启动流程会自动准备 CA，不要默认跳过 SSL 验证：

```bash
bifrost start -d
bifrost port bind --port 18882 --rule-text "api.example.com tlsIntercept://"
```

如果只想在启动配置层面允许某个域名或某类应用被 TLS 抓包，可以用白名单收窄范围；应用流量仍然从独立端口进入，便于后续按入口端口和应用过滤：

```bash
bifrost start --intercept-include "api.example.com" --app-intercept-include "*Chrome,*curl"
```

如果目标 App 有 SSL pinning，不要强行抓包它的 TLS 明文；用 `--app-intercept-exclude` 或规则级 `tlsPassthrough://` 排除后，只观察 CONNECT/host/端口等可见信息。

第三步，让目标流量进入 Bifrost。命令行工具最推荐显式代理：

```bash
curl -x http://127.0.0.1:18882 https://api.example.com/v1/me
```

浏览器或桌面应用默认会通过系统代理进入 Bifrost；沙箱或 CI 的隔离启动方式请按测试文档执行，不要把测试参数作为普通用户采集流量的默认路径。为了让 Agent 能写出可靠 skill，不要只抓一个成功请求，至少覆盖这些操作：

- 登录或会话初始化：确认 token、cookie、CSRF、租户 ID 等从哪里来。
- 列表和详情：确认分页、过滤、排序、资源 ID 字段。
- 创建、更新、删除或提交动作：确认 method、body、幂等键和状态码。
- 失败路径：确认鉴权失败、参数错误、权限不足、限流等错误格式。

第四步，用 traffic 命令把“完整流量”变成 Agent 可读的证据：

```bash
bifrost traffic list --host api.example.com --limit 50
bifrost traffic list --listener-port 18882 --limit 50
bifrost traffic list --client-app Chrome --limit 50
bifrost capture wait --host api.example.com --method POST --path /login --timeout 30s
bifrost search "keyword" --host api.example.com --req-body
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m --include response-body
bifrost traffic get <id> --request-body --response-body
bifrost traffic get --ids 12,13,14 --request-body --response-body --format ndjson
bifrost traffic auth-status <id>
```

这里的“完整”不是把所有隐私数据复制给 Agent，而是让 Agent 能看到完成任务所需的请求链路：URL、method、关键 headers/cookies、请求体、响应体、状态码、错误体和先后顺序。`traffic get`、`search --include`、`traffic export` 等输出当前按捕获原文返回；敏感 token、cookie、手机号、邮箱、身份证号等不应写入最终 skill，skill 里用环境变量、上下文或用户登录态描述来源。

第五步，给 Agent 下达任务指令。可以直接使用下面的模板：

```txt
请使用 bifrost capture wait 和 bifrost traffic list/search/get 读取 api.example.com 的真实流量。
先按 host/client_app/listener_port 过滤，挑出登录、列表、详情、创建、更新、失败响应各 1-2 条。
对每条请求总结 URL、method、headers/cookies/token 来源、请求体、响应体字段、分页/错误格式。
不要 mock，不要猜；证据不足时继续用 traffic get <id> --request-body --response-body 或 traffic get --ids ... --format ndjson 查看完整 body。
流量输出是捕获原文；发布或沉淀 skill 前必须手动移除 Authorization、Cookie、JWT token 等敏感值。
最后把协议理解整理成一个专属 skill：触发场景、所需参数、认证来源、API 调用步骤、错误处理、验证命令和隐私处理规则。
```

Agent 产出的 skill 至少应包含：

- 触发场景：用户说什么时应该使用这个 skill。
- 前置条件：需要登录态、环境变量、工作目录、Bifrost 连接或网络权限。
- 认证来源：token、cookie、CSRF、租户或用户 ID 从哪里读取，禁止把真实密钥写进 skill。
- 操作步骤：按真实流量总结出的 API 调用顺序、参数、body 和响应判断。
- 错误处理：常见错误码、错误字段、重试或重新登录条件。
- 验证方式：如何用 Bifrost traffic、CLI 命令或 API 响应确认 skill 真的完成任务。

第六步，清理本次采集环境：

```bash
bifrost port destroy 18882
```

## 命令快捷别名

| 快捷别名 | 等价命令 |
| --- | --- |
| `st` | `status` |
| `cfg` | `config` |
| `sp` | `system-proxy` |
| `val` | `value` |
| `wl` | `whitelist` |
| `comp` | `completions` |

## 环境变量

| 变量 | 说明 |
| --- | --- |
| `BIFROST_DATA_DIR` | 自定义数据目录。默认使用平台相关的 `~/.bifrost`，包含 config、rules、values、certs、logs、traffic 等数据。 |
| `RUST_LOG` | 控制日志级别和模块过滤，优先级高于 `-l/--log-level`。示例：`RUST_LOG=bifrost_proxy=debug,info bifrost`。 |
| `NO_COLOR` | 禁用 CLI 彩色输出。 |
| `FORCE_COLOR` | 强制启用 CLI 彩色输出。 |
| `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` | CLI 访问外部 HTTP 服务时会被底层 HTTP 客户端读取；和 `--cli-proxy` 写入的 shell 代理变量是同一类环境变量。 |

## 继续阅读

- [CLI 详细命令](./cli.md)
- [安装与启动](./getting-started.md)
- [规则语法](./rule.md)
- [匹配器](./pattern.md)
- [操作符说明](./operation.md)
- [规则协议手册](./rules/README.md)
