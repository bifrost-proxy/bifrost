---
title: "CLI 详细命令"
description: "Bifrost CLI 命令、参数与环境变量的完整参考。"
editLink: false
---

> 此页面由 `docs/cli.md` 自动同步生成。
# CLI 详细命令

本文档集中说明 `bifrost` CLI 的所有顶层命令、主要子命令、环境变量和规则模板变量。若只想按使用场景快速上手，请先看 [CLI 快速开始](../getting-started/cli-quick-start)。

## 帮助与文档入口

```bash
bifrost --help
bifrost <command> --help
bifrost <command> <subcommand> --help
```

顶层 `bifrost --help` 保持短输出：只展示常用示例、默认行为和关键文档链接，避免手写长参考与实际子命令参数漂移。详细参数以对应子命令 help 和本文档为准。

阅读原则：

- 本文档解释“这个命令做什么、何时使用、如何组合使用”；每个参数的最终解析以 `bifrost <command> --help` 为准。
- 会产生外部副作用的命令会明确标注，例如系统代理、shell rc 写入、远程 shell 执行、清空缓存、清除流量记录。
- `setting` 始终管理本机数据目录；`remote` 才会操作已连接的远端 Bifrost。不要把这两类命令混用。
- `--port` 在顶层和 `start` 中表示代理监听端口；在 `traffic list` 中 `--port` 表示 Admin API 端口，按流量入口端口过滤请用 `--listener-port` 或 `--proxy-port`。

关键文档：

- [CLI 快速开始](../getting-started/cli-quick-start)：按使用场景选择命令路径
- [安装与启动](../getting-started/installation)：安装方式与首次启动
- [规则语法](./rule-engine)：规则整体结构、过滤器、优先级
- [匹配器](./patterns)：domain / wildcard / regex / path wildcard 等匹配逻辑
- [操作符说明](./operations)：`host://`、`reqHeaders://`、`file://` 等 operation 的值格式
- [规则协议手册](./rules/)：按协议分类的完整能力说明

## 命令覆盖索引

| 命令 | 用途 | 本文档位置 |
| --- | --- | --- |
| `start` | 启动 HTTP/HTTPS/SOCKS5 代理，可配置 TLS 拦截、系统代理、规则、访问控制 | [`start` 命令](#start-命令) |
| `stop` / `restart` / `status` | 停止、重启、查看代理状态 | [服务管理](#服务管理) |
| `rule` | 管理本地规则：新增、更新、启停、排序、查看运行时 active 视图 | [规则管理](#规则管理) |
| `group` | 管理远端/共享 Group 与 Group 规则 | [Group 管理](#group-管理) |
| `port` | 绑定临时代理端口到显式规则集 | [临时端口规则绑定](#临时端口规则绑定) |
| `ca` | 生成、安装、导出、查看 Bifrost CA | [CA 证书管理](#ca-证书管理) |
| `whitelist` | 管理本机代理访问控制、待审批请求和临时放行 | [白名单管理](#白名单管理) |
| `system-proxy` | 启用、禁用、查看操作系统代理 | [系统代理管理](#系统代理管理) |
| `value` | 管理 `{VALUE_NAME}` 规则变量 | [Values 管理](#values-管理) |
| `script` | 管理请求、响应、decode 脚本 | [Scripts 管理](#scripts-管理) |
| `upgrade` / `version-check` | 检查新版本、升级二进制 | [升级与版本检查](#升级与版本检查upgrade--version-check) |
| `config` | 查看和修改运行时配置、连接、缓存、性能状态 | [配置项管理](#配置项管理) |
| `admin` | 管理 Admin 远程访问、密码、会话、审计日志 | [管理端远程访问与鉴权](#管理端远程访问与鉴权admin) |
| `capture` | 等待下一条匹配条件的流量记录，适合浏览器/桌面应用联调和 Agent 采证 | [等待捕获](#等待捕获capture-wait) |
| `traffic` / `search` | 查看、获取、搜索、导出、重放、诊断和清除流量记录 | [流量查看与搜索](#流量查看与搜索) |
| `install-skill` | 安装 Bifrost Agent Skill 文档到 AI coding tools | [安装 Skill](#安装-skillinstall-skill) |
| `ai voice` | 本地语音输入 runtime：来源探测、监听、词汇管理 | [本地语音输入](#本地语音输入ai-voice) |
| `completions` | 生成 shell 补全脚本 | [Shell 补全](#shell-补全completions) |
| `metrics` | 查看实时指标和历史指标 | [指标](#指标metrics) |
| `sync` | 登录、退出、触发、配置远端同步服务 | [同步](#同步sync) |
| `import` / `export` | 导入导出 `.bifrost` 规则、values、scripts 包 | [导入/导出](#导入导出import--export) |
| `setting` | 管理本机 Shell Access policy/profile 与本机 remote-invoke grant | [本机 remote-invoke 设置](#本机-remote-invoke-设置setting) |
| `remote` | 通过 relay 操作远端 Bifrost：连接、流量、文件、shell、keep-awake | [远程调用](#远程调用remote) |
| `keep-awake` | 管理本机 macOS 防睡眠 | [macOS 防睡眠](#macos-防睡眠keep-awake) |
| `im` | 管理 IM Gateway provider、target、route、schedule、消息历史 | [IM Gateway](#im-gatewayim) |

## 全局参数

```txt
bifrost [OPTIONS] [COMMAND]
```

| 参数 | 说明 | 默认值 |
| --- | --- | --- |
| `-p, --port <PORT>` | HTTP 代理端口 | `9900` |
| `-H, --host <HOST>` | 监听地址 | `0.0.0.0` |
| `--socks5-port <PORT>` | SOCKS5 端口 | 无 |
| `-l, --log-level <LEVEL>` | 日志级别 | `info` |
| `--log-output <TARGETS>` | 日志输出目标：`file` / `console` / `console,file`；默认写文件，显式 `console` 会额外输出到终端且仍保留文件日志 | `file` |
| `--log-dir <DIR>` | 日志目录（默认：`<data_dir>/logs`） | 无 |
| `--log-retention-days <DAYS>` | 日志保留天数 | `7` |
| `-h, --help` | 显示帮助 | - |
| `-v, -V, --version` | 显示版本号 | - |

## 环境变量

这些变量会影响 CLI 或运行时行为。仅用于测试、构建或内部诊断的变量不应作为日常用户接口依赖。

| 变量 | 作用范围 | 说明 |
| --- | --- | --- |
| `BIFROST_DATA_DIR` | CLI 与服务运行时 | 覆盖数据目录。规则、values、scripts、证书、配置、日志和流量数据库都会写入该目录。日常使用优先复用默认数据目录和多端口隔离；自动化测试、并发实验或破坏性配置验证才建议设置临时目录。 |
| `RUST_LOG` | 日志 | 覆盖 `-l/--log-level`，支持模块过滤，例如 `RUST_LOG=bifrost_proxy=debug,info bifrost start`。 |
| `NO_COLOR` | CLI 输出 | 存在时禁用彩色输出，适合日志采集、CI、Agent 解析。 |
| `FORCE_COLOR` | CLI 输出 | 存在时强制彩色输出，优先级高于终端自动检测。 |
| `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` | CLI 发起的外部 HTTP 请求 | 影响升级检查、同步、远程 relay 等需要访问网络的 CLI 请求；也可能由 `--cli-proxy` 写入 shell rc 文件。 |
| `NO_PROXY` | CLI 发起的外部 HTTP 请求 | 配合代理环境变量使用，声明不走代理的 host 或域名后缀。 |
| `BIFROST_REMOTE_SSH_KEY` | `remote conn up --ssh-key` | 固定的 Remote Invoke SSH key 环境变量；当 `--ssh-key` 不带路径时读取该变量内容。 |
| `BIFROST_FORCE_UPDATE_CHECK` | 版本检查 | 强制执行更新检查；主要用于调试或测试更新提示。 |
| `BIFROST_INSTALL_SKILL_SOURCE` | `install-skill` | 选择 skill 安装源；正常用户通常无需设置。 |
| `BIFROST_INSTALL_SKILL_DIR` | `install-skill` | 未显式传 `--dir` / `--cwd` 时覆盖全局 skill 安装目录；主要用于升级/E2E 测试隔离，避免写入真实 AI tool 目录。 |
| `BIFROST_AGENT_HOME` | Agent 运行时 | Agent 配置和记忆目录的兼容覆盖项；默认优先使用 `$BIFROST_DATA_DIR/agent/`。 |
| `BIFROST_AGENT_MODEL` / `BIFROST_AGENT_PROVIDER` | Agent 运行时 | 覆盖 Agent 默认模型和 provider。只影响 Agent 能力，不影响代理核心规则。 |
| `BIFROST_AGENT_WORK_DIR` | Agent 运行时 | 覆盖 Agent 默认工作目录。 |

容易混淆的边界：

- `--system-proxy` 修改操作系统代理配置；`--cli-proxy` 写入 shell rc 文件中的代理环境变量；`HTTP_PROXY` / `HTTPS_PROXY` 是当前进程继承到的环境变量。
- `BIFROST_DATA_DIR` 不只是配置目录，也决定当前 CLI 连接的 rules、values、scripts、certs、traffic DB 等状态来源。
- `RUST_LOG` 优先级高于命令行 `-l/--log-level`。如果你设置了 `RUST_LOG`，再改 `--log-level` 可能看起来“不生效”。

## 规则变量速查

Bifrost 规则里有两类常见变量，含义不同：

| 写法 | 来源 | 用途 |
| --- | --- | --- |
| `{NAME}` | 规则文件内嵌值，或少量确实需要全局共享的大型 Values | 在规则解析前展开。默认优先写内联 value 或规则文件内嵌值；特别大的 body/header/PAC 或跨很多规则复用时才放全局 Values |
| `${name}` / `${name.property}` | 当前请求/响应上下文或环境变量 | 在请求处理时展开，适合引用 URL、headers、cookies、query、状态码 |
| `$1` / `$2` | regex 或 wildcard 捕获组 | 在 pattern 命中后引用捕获结果 |
| `$${name}` | 转义 | 输出字面量 `${name}`，不展开 |

常用模板变量：

| 变量 | 含义 |
| --- | --- |
| `${url}` / `${host}` / `${hostname}` / `${port}` | 当前请求 URL、host、hostname、端口 |
| `${path}` / `${pathname}` / `${search}` / `${query}` / `${query.name}` | 路径、查询字符串、指定 query 参数 |
| `${method}` / `${statusCode}` / `${status}` | 请求方法、响应状态码 |
| `${reqHeaders.name}` / `${reqH.name}` | 请求头；不带 `.name` 时返回全部请求头文本 |
| `${resHeaders.name}` / `${resH.name}` | 响应头；仅在响应阶段有意义 |
| `${reqCookies.name}` / `${resCookies.name}` | 请求或响应 cookie |
| `${clientIp}` / `${clientPort}` / `${remoteAddress}` / `${remotePort}` | 客户端与远端连接信息 |
| `${realUrl}` / `${realHost}` / `${realPort}` | Bifrost 解析到的真实请求信息 |
| `${now}` / `${random}` / `${randomUUID}` / `${randomInt(n)}` / `${randomInt(a-b)}` | 时间和随机值 |
| `${version}` / `${id}` / `${reqId}` | Bifrost 版本与请求 ID |
| `${env.NAME}` | 读取当前 Bifrost 进程环境变量 `NAME` |

高级模板写法：

```txt
api.example.com reqHeaders://X-Path=${path}
api.example.com reqHeaders://X-Encoded=${{url}}
api.example.com reqHeaders://X-Host=${hostname.replace(example,test)}
api.example.com file://({"literal":"$${host}"})
```

更多变量、URL 编码、replace 语法和数据对象格式见 [操作符说明](./operations)。

## `start` 命令

常见示例：

```bash
bifrost start -d
bifrost start
bifrost restart
bifrost -p 9000 start
bifrost -p 9000 --socks5-port 1080 start
bifrost start --no-intercept
bifrost start --intercept-exclude "*.example.com,internal.corp.com"
bifrost start --intercept-include "*.api.local"
bifrost start --app-intercept-include "*Chrome,*curl"
bifrost start --app-intercept-exclude "*PinnedApp"
bifrost start --rules "example.com host://127.0.0.1:3000"
bifrost start --rules-file ./my-rules.txt
bifrost start --access-mode whitelist --whitelist "192.168.1.100,10.0.0.0/8"
bifrost start --allow-lan
bifrost start --proxy-user admin:password123
bifrost start --system-proxy
bifrost start --disable-badge-injection
bifrost start --enable-badge-injection
```

`bifrost start -d` 是普通用户推荐的后台启动方式；`bifrost start` 适合需要查看实时日志的前台模式。`bifrost start` 默认会启用系统代理，最快让浏览器和桌面应用流量进入 Bifrost。TLS 抓包不是默认建议，应按需通过规则级 `tlsIntercept://`、`--intercept-include` 或 `--app-intercept-include` 收窄到目标域名/应用；遇到 SSL pinning 应用时，用 `tlsPassthrough://`、`--intercept-exclude` 或 `--app-intercept-exclude` 排除。需要 TLS 抓包时，服务启动流程会自动生成并安装 Bifrost CA；`ca generate` / `ca install` 主要用于手动修复或诊断证书状态。默认访问控制模式为 `interactive`：本机 loopback 直接允许，非本机地址需要管理端审批、白名单或显式 `--allow-lan` / `--access-mode` 放行。`--no-system-proxy`、`--unsafe-ssl` 和 `--skip-cert-check` 是测试/诊断选项，不应作为普通启动路径。

当检测到已有 Bifrost 进程在运行时，`bifrost start` 会在终端提示是否重启：输入 `y/yes` 将停止旧进程并重新启动；输入 `n/no` 将取消本次启动。

如果需要在脚本/CI 中跳过交互，可以使用 `-y/--yes` 自动确认重启。

参数摘要：

| 参数 | 说明 |
| --- | --- |
| `-d, --daemon` | 守护进程模式 |
| `--skip-cert-check` | 跳过 CA 证书安装检查，仅用于你明确知道证书状态且需要继续测试的场景；普通启动会自动处理 CA |
| `--access-mode <MODE>` | `local_only` / `whitelist` / `interactive` / `allow_all` |
| `--whitelist <IPS>` | 客户端 IP 白名单，支持 CIDR |
| `--allow-lan` | 允许局域网访问 |
| `--proxy-user <USER:PASS>` | 代理认证账号（可重复指定） |
| `--intercept` | 启用全局 TLS 抓包；不要作为默认启动参数，优先使用域名/应用白名单或规则级 `tlsIntercept://` |
| `--no-intercept` | 禁用 TLS 拦截 |
| `--intercept-exclude <DOMAINS>` | TLS 拦截排除域名 |
| `--intercept-include <DOMAINS>` | TLS 拦截白名单（最高优先级，即使全局关闭也生效） |
| `--app-intercept-exclude <APPS>` | TLS 拦截排除应用（进程名通配） |
| `--app-intercept-include <APPS>` | TLS 拦截应用白名单（最高优先级） |
| `--unsafe-ssl` | 跳过上游证书校验，仅用于证书异常诊断或隔离测试；普通 HTTPS 调试应让启动流程自动处理 CA 并保持证书校验 |
| `--enable-badge-injection` | 强制启用 HTML 页面注入 Bifrost 小圆点（会持久化到配置） |
| `--disable-badge-injection` | 禁用 HTML 页面注入 Bifrost 小圆点（会持久化到配置） |
| `--no-disconnect-on-config-change` | TLS 配置变更时不自动断开受影响连接 |
| `--rules <RULE>` | 直接传入规则，可多次指定 |
| `--rules-file <PATH>` | 从文件加载规则 |
| `--system-proxy` | 启动后自动设置系统代理 |
| `--proxy-bypass <LIST>` | 系统代理绕过列表 |
| `--cli-proxy` | 运行期间写入命令行代理环境变量 |
| `--cli-proxy-no-proxy <LIST>` | 命令行代理 no-proxy 列表 |
| `-y, --yes` | 自动确认交互提示（如已运行进程的重启确认） |

## 常用命令

### 服务管理

```bash
bifrost status
bifrost status --tui
bifrost stop
bifrost restart
bifrost restart --port 9900 --host 127.0.0.1 --log-level debug
bifrost restart --force
```

`restart` 会停止当前代理并启动一个新的后台 daemon，常用于 `bifrost upgrade` 后让运行中的服务切到新二进制。该命令会把新进程与当前终端管道解耦，因此也适合通过 `bifrost remote exec` 远程触发。

### 临时端口规则绑定

```bash
bifrost port bind --port 18888 --rule local-dev
bifrost port bind --port 18889 --rule local-dev --group-rule 7152084678483132446/abc
bifrost port bind --port 0 --rule-file ./temp-rule.bifrost
bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"
bifrost port list
bifrost port show 18888
bifrost port active 18888
bifrost port update 18888 --rule another-local-rule
bifrost port update 18888 --rule-file ./updated-temp-rule.bifrost
bifrost port destroy 18888
```

`port` 命令需要主代理正在运行。临时端口与主端口共享同一个 `BIFROST_DATA_DIR` 中的规则、values、scripts、证书、流量记录等数据；端口绑定只保存“这个端口选择哪些规则集”。临时端口 resolver 会先加载全局默认规则 `Default`，再加载 `port bind` / `port update` 显式绑定的本地规则或 Group 规则；它不继承主端口的其它 enabled 普通规则。

`--rule` 引用本地规则名；`--group-rule` 使用 `<group_id>/<rule_name>` 格式；`--rule-file` 直接绑定规则文件；`--rule-text` 直接绑定规则原文。不要把 `Default` 作为 `--rule` 显式绑定，它会自动应用到所有临时端口，`default`/`DEFAULT` 等大小写变体也会被拒绝。销毁临时端口只关闭该端口监听，不删除共享规则数据，也不影响主端口。

临时端口绑定状态只在当前运行进程内存里生效，不写入持久配置。Bifrost 重启后临时端口会被重置，不会自动重新监听，也不会恢复之前的规则绑定；需要时请重新执行 `bifrost port bind ...`。

Traffic 记录会带监听端口信息，即使请求没有命中任何规则也会记录来源端口：`traffic list` 的表格包含 `PORT` 列，JSON compact 字段为 `lp`，`traffic get` 详情字段为 `listener_port`。这用于区分同一数据目录内主端口和临时端口产生的流量。

多端口推荐工作流：

1. 先确认主代理已运行；默认用 `bifrost start`。如需 HTTPS 明文调试，优先在任务规则里使用 `tlsIntercept://`，或用 `--intercept-include` / `--app-intercept-include` 只放行目标域名/应用。
2. 保持主端口继续承载默认启用规则。
3. 如需所有端口共享兜底规则，编辑全局 `Default`；它会自动排在每个端口 active rules 的最前面。
4. 为临时调试场景按需再开多个端口：
   - `bifrost port bind --port 18888 --rule local-dev`
   - `bifrost port bind --port 18889 --group-rule 7152084678483132446/abc`
   - `bifrost port bind --port 0 --rule-file ./temp-debug.bifrost`
   - `bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"`
5. 用 `bifrost port list` 查看当前所有临时端口；用 `bifrost port show <port>` 看绑定元信息；用 `bifrost port active <port>` 看这个端口当前真正生效的规则视图。
6. 当一个临时端口需要切换到另一组规则时，使用 `bifrost port update <port> ...` 传入新的完整规则引用集合。
7. 调试结束后执行 `bifrost port destroy <port>` 回收对应监听端口。

同一个 Bifrost 服务可以同时服务多个应用或开发任务。推荐做法是共享一个常驻主服务，让系统代理、CA、Web UI 和 traffic 存储保持统一；每个应用、任务或 Agent 分配一个独立入口端口，并只在该端口绑定对应规则：

```bash
bifrost port bind --port 18881 --rule-text "app-a.example.com tlsIntercept:// host://127.0.0.1:3001"
bifrost port bind --port 18882 --rule-text "app-b.example.com tlsIntercept:// host://127.0.0.1:3002"
bifrost traffic list --listener-port 18881 --limit 50
bifrost traffic list --listener-port 18882 --limit 50
```

给 Agent 排查或生成 skill 时，应在指令中明确入口端口，例如“只分析 `listener_port=18882` 的流量”。这能避免多个应用共享同一个 traffic 数据库时，把其他任务的请求混入协议理解。

规则来源选择建议：

| 方式 | 适用场景 | 示例 |
| --- | --- | --- |
| `--rule` | 复用已有本地规则 | `bifrost port bind --port 18888 --rule local-dev` |
| `--group-rule` | 复用已有 Group 规则 | `bifrost port bind --port 18889 --group-rule 7152084678483132446/abc` |
| `--rule-file` | 一次性加载本地规则文件，不写入共享规则目录 | `bifrost port bind --port 0 --rule-file ./temp-debug.bifrost` |
| `--rule-text` | 临时写一条短规则快速排障 | `bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"` |

帮助文档检查点：

- `bifrost --help` 应能看到 `port` 顶层命令。
- `bifrost port --help` 应解释主端口与临时端口的职责差异。
- `bifrost port bind --help` / `bifrost port update --help` 应解释四类规则来源和 `--port 0` 自动分配行为。

### 流量查看与搜索

```bash
bifrost traffic list
bifrost traffic list --method GET --status-min 400 --limit 100
bifrost traffic list --listener-port 50831 --format json
bifrost traffic list --proxy-port 50831 --format json
bifrost traffic list --client-app Chrome --limit 50
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s
bifrost capture wait --host api.example.com --timeout 10s --format json
bifrost traffic get <id> --request-body --response-body
bifrost traffic get --ids 1,2,3 --request-body --response-body --format ndjson
bifrost traffic get --ids 1,2,3 --request-body --response-body --max-body 32768
bifrost traffic auth-status <id>
bifrost traffic auth-status <id> --format json
bifrost traffic export <id> --as curl
bifrost traffic export <id> --as fetch
bifrost traffic export <id> --as har -o ./request.har
bifrost traffic replay <id>
bifrost traffic replay <id> --patch '/json/user/name="debug"'
bifrost traffic replay <id> --patch-json '[{"op":"replace","path":"/json/user/name","value":"debug"}]'
bifrost traffic search "keyword"
bifrost traffic search "keyword" --listener-port 50831
bifrost traffic search "keyword" --proxy-port 50831
bifrost search "keyword"
bifrost search "keyword" --method POST --host api.openai.com --path /v1/responses
bifrost search "keyword" --req-header
bifrost search "keyword" --res-body
bifrost search "" --host api.example.com --req-json '$.user.id=42' --include request-body,response-body
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m
bifrost search "token" --req-header-eq authorization="Bearer ..." --since 2026-01-01T00:00:00Z --until 2026-01-01T01:00:00Z
```

`bifrost search` 与 `bifrost traffic search` 等价，支持关键词搜索、基础过滤器、结构化 JSONPath / Header 精确匹配、时间范围、返回内容 include 与搜索范围控制。`traffic get` 默认查询单条详情；需要一次交给 Agent 或脚本分析多条记录时，用 `--ids` 批量读取，批量输出推荐 `--format ndjson`，避免把多条大响应合成一个巨大 JSON。

本期不做 Authorization、Cookie、JWT token 等敏感信息脱敏。`traffic get`、`traffic export`、`search --include` 输出均按捕获原文返回；完整脱敏方案会另开需求处理，当前不要把这些输出粘贴到低信任渠道或可复用文档。

基础过滤器：

| 参数 | 说明 |
| --- | --- |
| `--method <METHOD>` | 按 HTTP 方法过滤，如 `GET`、`POST` |
| `--host <TEXT>` | 按 Host 包含匹配过滤 |
| `--url <TEXT>` | 按完整 URL 包含匹配过滤，仅 `traffic list` 支持 |
| `--path <TEXT>` | 按 Path 包含匹配过滤 |
| `--status <FILTER>` | `traffic list` 中为精确状态码，如 `404`；`search` 中为状态段，如 `2xx`、`4xx`、`5xx`、`error` |
| `--status-min <CODE>` / `--status-max <CODE>` | 按状态码上下界过滤，仅 `traffic list` / `remote traffic list` 支持 |
| `--protocol <PROTO>` | `traffic list` 使用小写 `http` / `https` / `ws` / `wss` / `h3`；`search` 使用大写 `HTTP` / `HTTPS` / `WS` / `WSS` |
| `--domain <PATTERN>` | 按域名模式过滤 |
| `--content-type <TYPE>` | 按内容类型过滤，如 `json`、`html`、`form` |
| `--client-ip <IP>` | 按客户端 IP 过滤，仅 `traffic list` 支持 |
| `--client-app <APP>` | 按客户端应用或进程名过滤，适合只分析某个浏览器、桌面应用或 CLI 工具产生的流量 |
| `--listener-port <PORT>` / `--proxy-port <PORT>` | 按流量入口代理端口过滤；`traffic list` 中的 `--port` 仍表示 Admin API 端口 |
| `--req-json <PATH=VALUE>` / `--res-json <PATH=VALUE>` | 按请求体或响应体 JSONPath 值过滤；适合不用关键词、只按结构字段定位请求 |
| `--req-header-eq <NAME=VALUE>` / `--res-header-eq <NAME=VALUE>` | 按请求头或响应头精确值过滤 |
| `--since <TIME>` / `--until <TIME>` | 限定搜索时间窗口，支持 RFC3339 时间或实现支持的相对时间格式 |
| `--latest <DURATION>` | 只搜索最近一段时间，例如 `15m`、`1h` |
| `--has-rule-hit <true|false>` | 按是否命中规则过滤，仅 `traffic list` 支持 |
| `--is-websocket <true|false>` / `--is-sse <true|false>` / `--is-tunnel <true|false>` | 按 WebSocket、SSE 或 CONNECT 隧道流量过滤，仅 `traffic list` 支持 |

入口端口过滤用于区分主代理端口、临时代理端口、远端代理端口产生的流量。例如临时端口 `50831` 的请求可以用 `traffic list --listener-port 50831` 或 `traffic search "keyword" --proxy-port 50831` 查询；顶层 `bifrost search` 与 `bifrost traffic search` 的过滤语义一致。

按应用过滤依赖 Bifrost 记录到的客户端进程信息。和 `start --app-intercept-include` 配合使用时，可以把某个浏览器或桌面应用的 HTTPS 明文请求收窄成可交给 Agent 分析的证据集；若记录里没有应用名，请改用 `--host`、`--path`、`--listener-port` 等过滤器。

搜索范围与返回内容：

| 参数 | 说明 |
| --- | --- |
| `--url` | 仅搜索 URL / Path |
| `--req-header` | 仅搜索请求头 |
| `--res-header` | 仅搜索响应头 |
| `--req-body` | 仅搜索请求体 |
| `--res-body` | 仅搜索响应体 |
| `--headers` | 同时搜索请求头与响应头 |
| `--body` | 同时搜索请求体与响应体 |
| `--include <FIELDS>` | 在搜索结果里附加指定内容，支持 `request-body`、`response-body`、`request-headers`、`response-headers`，也可用 `req-body`、`res-body`、`req-headers`、`res-headers`、`bodies`、`headers` 等别名 |
| `--max-body <BYTES>` | 限制返回 body 的最大字节数，避免一次输出过大 |

常见组合示例：

```bash
# 在 OpenAI 请求里搜索 Authorization 请求头
bifrost search "Bearer " --method POST --host api.openai.com --req-header

# 搜索某个接口的请求体
bifrost search "user_123" --host api.example.com --path /v1/users --req-body

# 搜索响应头中的缓存标记
bifrost search "cache-control" --res-header

# 搜索响应体中的错误信息
bifrost search "invalid_request_error" --res-body

# 等待浏览器或桌面应用产生下一条目标请求；超时退出码为 124
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s

# 一次读取多条详情给 Agent 分析；输出为捕获原文，注意不要扩散敏感信息
bifrost traffic get --ids 12,13,14 --request-body --response-body --format ndjson

# 按 JSON 字段和最近时间窗口定位失败响应，并附带响应体
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m --include response-body

# 诊断一条请求里的 JWT / Cookie 登录态是否过期
bifrost traffic auth-status 12

# 导出一条请求为可复现模板，或基于原始请求重放并局部改 body
bifrost traffic export 12 --as curl
bifrost traffic replay 12 --patch '/json/debug=true'
```

### 等待捕获（capture wait）

```bash
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s
bifrost capture wait --host api.example.com --timeout 10s --format json
bifrost capture wait --path /healthz --timeout 5s --open "https://api.example.com/healthz"
```

`capture wait` 用于“先启动等待，再让浏览器、桌面应用或脚本发起请求”的场景。它会长轮询 Admin API，直到出现符合 `--host`、`--method`、`--path` 的下一条 traffic 记录，或等待超时。`--timeout` 默认 `60s`，最大 `600s`；超时时命令退出码为 `124`，适合脚本判断“没有抓到目标请求”。`--format json` 会输出服务端返回的结构化结果；`--open` 会在等待前尝试打开 URL，打开失败只打印 warning，不影响继续等待。

### CA 证书管理

```bash
bifrost ca generate
bifrost ca generate --force
bifrost ca install
bifrost ca export
bifrost ca export -o ca.crt
bifrost ca info
```

### 规则管理

```bash
bifrost rule list
bifrost rule active
bifrost rule add <name> --content "rule"
bifrost rule add <name> --file rules.txt
bifrost rule add <name> --json --content "rule"
bifrost rule add <name> --allow-invalid --content "draft rule"
bifrost rule update <name> --content "new rule"
bifrost rule update <name> --file rules.txt
bifrost rule update <name> --json --content "new rule"
bifrost rule update <name> --allow-invalid --content "draft rule"
bifrost rule enable <name>
bifrost rule disable <name>
bifrost rule delete <name>
bifrost rule show <name>
bifrost rule get <name>
bifrost rule share <name> https://example.com/path
bifrost rule share <name> https://example.com/path --content "example.com bp://127.0.0.1:3000"
bifrost rule share <name> https://example.com/path --file rules.txt
bifrost rule sync
bifrost rule rename <name> <new_name>
bifrost rule reorder <name1> <name2> ...
```

- `rule active` 需要代理服务运行中（通过管理接口获取运行时已启用规则摘要）
- `rule share` 会生成带 `__bifrost_rule` query 的分享链接。目标 URL 支持完整 `http://` / `https://` 地址，也支持 `a.com`、`example.com/path`、`localhost:3000` 这类裸域名输入；裸域名会默认规范成 `http://...`，确保普通 HTTP 代理请求能在不依赖 TLS 拦截的情况下看到分享 query。显式输入 `https://...` 时会保持 HTTPS。未传 `--content` 或 `--file` 时读取同名本地规则；传入 `--content` 或 `--file` 时只生成链接，不把规则写入本地规则目录。
- 分享链接被 Bifrost 代理劫持后会先跳到本机确认页面，展示规则名、内容 hash、独占范围、返回目标和完整规则内容；content hash 仅用于人工核对，用户不需要手工输入。用户确认后才会导入到 `share/<规则名>` 命名空间并启用它，同时禁用其他个人规则；第一版固定 `exclusive_scope=my_rules`，不会修改 Group 规则。对已导入的 `share/...` 规则再次执行 `rule share` 时，协议 payload 会自动剥掉 `share/` 前缀，继续使用原始分享名。
- `rule add` / `rule update` 默认会在保存前执行语法检查。规则无效时命令退出码为 2，不写入规则文件；加 `--json` 可获得 `saved=false`、`syntax.errors[]` 和 `syntax.guidance` 结构化反馈，便于 Agent 按建议修复后重试。
- `--allow-invalid` 只用于显式保存临时草稿；命令仍会返回语法报告，且无效规则的 `syntax.valid` 保持为 `false`。

### Group 管理

```bash
# 列出/搜索 groups
bifrost group list
bifrost group list --keyword "team" --limit 20

# 查看 group 详情
bifrost group show <group_id>

# 列出 group 下所有规则
bifrost group rule list <group_id>

# 查看 group 规则详情
bifrost group rule show <group_id> <rule_name>

# 添加 group 规则
bifrost group rule add <group_id> <name> --content "example.com host://127.0.0.1:3000"
bifrost group rule add <group_id> <name> --file rules.txt
bifrost group rule add <group_id> <name> --allow-invalid --content "@draft-shared"

# 更新 group 规则
bifrost group rule update <group_id> <name> --content "new rule"
bifrost group rule update <group_id> <name> --file rules.txt
bifrost group rule update <group_id> <name> --allow-invalid --content "@draft-shared"

# 启用/禁用 group 规则
bifrost group rule enable <group_id> <name>
bifrost group rule disable <group_id> <name>

# 删除 group 规则
bifrost group rule delete <group_id> <name>
```

- `group` 命令需要代理服务运行中（通过 admin API 通信）
- `group list` 支持 `--keyword` 模糊搜索和 `--limit` 限制结果数
- `group rule add/update` 通过 `--content` 或 `--file` 提供规则内容
- `group rule add/update` 调用管理接口保存，因此同样会触发保存前语法检查；后端拒绝保存时 CLI 会显示 HTTP 422、第一条语法错误和修复建议。`--allow-invalid` 会显式透传给后端，允许保存临时无效草稿。

### 白名单管理

```bash
bifrost whitelist list
bifrost whitelist add 192.168.1.100
bifrost whitelist add 10.0.0.0/8
bifrost whitelist remove 192.168.1.100
bifrost whitelist allow-lan true
bifrost whitelist allow-lan false
bifrost whitelist status
bifrost whitelist mode                         # 查看当前访问模式
bifrost whitelist mode whitelist               # 设置访问模式（local_only/whitelist/interactive/allow_all）
bifrost whitelist pending                      # 查看待处理的访问请求
bifrost whitelist approve <ip>                 # 批准待处理请求（按 IP）
bifrost whitelist reject <ip>                  # 拒绝待处理请求（按 IP）
bifrost whitelist clear-pending                # 清空待处理请求
bifrost whitelist add-temporary <ip>           # 临时放行（按 IP）
bifrost whitelist remove-temporary <ip>        # 移除临时放行（按 IP）
```

- `mode/pending/approve/reject/clear-pending/add-temporary/remove-temporary` 需要代理服务运行中（走管理接口）

### Values 管理

```bash
bifrost value list
bifrost value show <name>
bifrost value get <name>
bifrost value add <name> <value>
bifrost value set <name> <value>
bifrost value update <name> <value>
bifrost value delete <name>
bifrost value import <file>
```

`value` 管理的是规则里的 `{NAME}` 引用，不是 shell 环境变量。默认不要把普通 host、token 片段、短 header 或小 mock body 拆到全局 Values；优先直接写内联 value，或在规则文件里用内嵌值定义，让规则和依赖内容一起审查、导入、导出。只有特别大的 body/header/PAC，或确实需要被很多规则长期共享的内容，才建议使用 `bifrost value set`。`add` 和 `set` 等价；`show` 和 `get` 等价。

### Scripts 管理

```bash
bifrost script list
bifrost script list -t request
bifrost script add request demo --content 'log.info("hello")'
bifrost script update request demo --content 'log.info("updated")'
bifrost script show request demo
bifrost script show demo
bifrost script get demo
bifrost script run demo
bifrost script run request demo
bifrost script rename request demo demo-v2
bifrost script delete request demo
```

当前脚本运行时和管理端 API 支持 `request` / `response` / `decode` / `parser` 四类脚本；其中 `parser` 用于 `bp://... decode://bp` 的二进制协议解析。CLI 的 `script list -t`、`add`、`update`、`delete`、`rename` 参数校验仍只暴露 `request` / `response` / `decode`，不适合用来新建 parser 脚本；parser 脚本请通过 WebUI Scripts 页面、Admin API `/_bifrost/api/scripts/parser/<name>`，或直接按数据目录结构写入 `scripts/parser/<name>.js`。不带 `-t` 的 `script list` 与 `script show/run <name>` 会扫描 parser 脚本。

### 系统代理管理

```bash
bifrost system-proxy status
bifrost system-proxy enable
bifrost system-proxy enable --host 127.0.0.1 --port 9900
bifrost system-proxy enable --bypass "localhost,127.0.0.1,*.local"
bifrost system-proxy disable
```

### 配置项管理

```bash
bifrost config show --section traffic
bifrost config show --json
bifrost config get tls.enabled
bifrost config get tls.enabled --json
bifrost config set traffic.max-records 10000
bifrost config add tls.exclude '*.example.com'
bifrost config remove tls.exclude '*.example.com'
bifrost config reset tls.enabled -y
bifrost config clear-cache -y
bifrost config disconnect example.com
bifrost config disconnect-by-app Chrome
bifrost config export -o ./config.toml --format toml
bifrost config export --format json
bifrost config performance
bifrost config websocket
bifrost config set traffic.max-db-size 2GB
bifrost config set traffic.max-body-size 1MB
bifrost config set traffic.max-buffer-size 20MB
bifrost config set traffic.retention-days 3
bifrost config set traffic.sse-stream-flush-bytes 64KB
bifrost config set traffic.sse-stream-flush-interval-ms 200
bifrost config set traffic.ws-payload-flush-bytes 256KB
bifrost config set traffic.ws-payload-flush-interval-ms 200
bifrost config set traffic.ws-payload-max-open-files 128
bifrost config connections
bifrost config memory
```

配置命令的使用边界：

| 命令 | 做什么 | 常见用途 |
| --- | --- | --- |
| `show` / `get` | 读取配置 | 排查当前 TLS、traffic、access、sync 等配置 |
| `set` | 设置标量配置 | 修改 `traffic.max-records`、`tls.enabled` 等单值 |
| `add` / `remove` | 修改列表配置 | 增减 TLS include/exclude、访问控制列表等 |
| `reset` | 恢复默认值 | 回滚某个 key 或 `all` |
| `clear-cache` | 清理 body、traffic、frame 缓存 | 释放磁盘或清除调试残留；会影响后续流量查询 |
| `disconnect` / `disconnect-by-app` | 主动断开匹配连接 | TLS 或规则配置变更后让连接重新建立 |
| `performance` / `websocket` / `connections` / `memory` | 查看运行时状态 | 诊断性能、连接和资源占用 |

配置 key 的完整可用范围以 `bifrost config show --json` 的结构为准。写自动化脚本时建议先 `get` 再 `set`，避免把布尔、数字、大小单位字符串写错。

## 其他命令（与当前 CLI 对齐）

### 管理端远程访问与鉴权（admin）

```bash
bifrost admin remote status
bifrost admin remote enable
bifrost admin remote disable

bifrost admin passwd
bifrost admin passwd --username admin
printf '%s\n' 'new_password' | bifrost admin passwd --password-stdin

bifrost admin revoke-all

bifrost admin audit
bifrost admin audit --limit 100 --offset 0
bifrost admin audit --json
```

| 子命令 | 说明 | 注意事项 |
| --- | --- | --- |
| `remote enable/disable/status` | 控制管理端远程访问开关 | 只影响 Admin UI/API 远程访问，不等同于 `remote` relay 调用 |
| `passwd` | 设置或修改 admin 密码 | 交互式隐藏输入；脚本可用 `--password-stdin` |
| `revoke-all` | 注销所有 admin session | 会让已登录管理端重新登录 |
| `audit` | 查看登录审计日志 | `--json` 适合 Agent 或脚本读取 |

### traffic 清理（clear）

```bash
bifrost traffic clear
bifrost traffic clear --ids 1,2,3 -y
```

### 全文搜索（search）

```bash
bifrost search "keyword" --host example.com --req-header
bifrost search --interactive
```

### 升级与版本检查（upgrade / version-check）

```bash
bifrost version-check
bifrost upgrade
```

`upgrade` 默认无交互执行升级；如果检测到当前有代理在运行，会在升级成功后停止并重新拉起代理，让运行中的服务切换到新二进制。Windows 手动安装路径下，CLI 会先 stage 新的 `bifrost.exe`，等待当前 upgrade 进程退出后再替换自身，并用新的 exe 启动 daemon。

### 同步（sync）

```bash
bifrost sync status
bifrost login
bifrost login --token "$BIFROST_SYNC_TOKEN"
bifrost login --token "$BIFROST_SYNC_TOKEN" --url https://bifrost.bytedance.net
bifrost sync login
bifrost sync login --token "$BIFROST_SYNC_TOKEN"
bifrost sync login --token "$BIFROST_SYNC_TOKEN" --url https://bifrost.bytedance.net
bifrost sync logout
bifrost sync run
bifrost sync config --enabled true --auto-sync true --remote-url https://example.com
```

`bifrost login` 与 `bifrost sync login` 等价，用于更直接地表达登录动作。Headless/CI 登录 token 可从 `https://bifrost.bytedance.net/v4/sso/token-login` 获取；省略 `--url` 时使用当前同步配置的远端 URL，默认是内置 Bifrost Provider。

### 本机 remote-invoke 设置（setting）

`setting` 总是管理当前机器的数据目录，不会直接操作远端设备。若要配置远端机器，需要通过 `bifrost remote exec -- bifrost setting ...` 在远端执行。

```bash
bifrost setting shell list
bifrost setting shell show --json
bifrost setting shell profile add --id default --name Default --cwd "$HOME" --env PATH --env HOME --timeout-ms 30000
bifrost setting shell policy add --id allow-bifrost-cli --name "Allow Bifrost CLI" --mode shell_text --pattern '^bifrost\\s+' --shell /bin/zsh --profile default
bifrost setting shell policy enable allow-bifrost-cli

bifrost setting grant list
bifrost setting grant list --json
bifrost setting grant update <grant-id> --scope remote_shell_exec --file-access read
bifrost setting grant revoke --grant-id <grant-id>

bifrost setting ssh-key create --label "dev-mac" --output ./bifrost-device.key
bifrost setting ssh-key export --output ./bifrost-device.key --force
bifrost setting ssh-key status
bifrost setting ssh-key revoke
```

| 子命令 | 说明 | 适用场景 |
| --- | --- | --- |
| `setting shell list/show/apply` | 查看或整体应用 Shell Access 配置 | 审计当前本机允许哪些远程 shell 能力 |
| `setting shell profile add/delete/enable/disable` | 管理 shell 执行 profile | 约束 cwd、env、timeout 等执行环境 |
| `setting shell policy add/update/delete/enable/disable` | 管理 shell 命令匹配策略 | 控制哪些 shell text 或 argv 可以被远程 grant 使用 |
| `setting grant list/update/revoke` | 管理本机 remote-invoke grant | 调整或撤销远端调用本机的授权 |
| `setting ssh-key create/export/status/revoke` | 管理本机 reusable remote-invoke SSH key | 在 target 机器上快速生成 key 文件并交给 caller 使用 |

Shell Access policy 是远程执行的最后一道本机策略，不是普通 CLI alias。放宽 policy 前要确认 grant 的来源和 file access 范围。

### 远程调用（remote）

`remote` 通过 relay 对另一台已授权的 Bifrost 实例执行操作。全局参数 `--relay-url` 的优先级为：命令行显式值 > 当前运行服务的 sync 配置 > 本地配置文件 > 内置默认值；`--client-id` 用于在多个已保存连接中选择目标 client id 或 label 前缀，必须放在 `remote` 后、具体子命令前。也可以用 `BIFROST_REMOTE_CLIENT_ID` 固定默认目标。

```bash
bifrost setting ssh-key create --label "dev-mac" --output ./bifrost-device.key
bifrost remote conn up <pair-code>
bifrost remote conn up --ssh-key ./bifrost-device.key --label "dev-mac"
export BIFROST_REMOTE_SSH_KEY="$(cat ./bifrost-device.key)"
bifrost remote conn up --ssh-key --label "ci-agent"
bifrost remote conn status
bifrost remote conn down
bifrost remote conn down --grant-id <grant-id>
bifrost remote conn down --all

bifrost remote exec --shell-text "pwd && ls"
bifrost remote exec -- /bin/zsh -lc 'bifrost status'
bifrost remote run --script-file ./q.py --interpreter python3 --cwd /path/to/repo -- --limit 5
bifrost remote run --script-file ./check-status.js --interpreter node --detach --cwd /path/to/repo -- --wait

bifrost remote file read README.md --cwd /path/to/repo
bifrost remote file list src --depth 2 --cwd /path/to/repo
bifrost remote file stat Cargo.toml --cwd /path/to/repo
bifrost remote file glob "src/**/*.rs" --cwd /path/to/repo
bifrost remote file find "TODO" --path src --cwd /path/to/repo
bifrost remote file hash Cargo.toml --cwd /path/to/repo
printf 'hello\n' | bifrost remote file write notes.txt --content-file - --cwd /tmp
bifrost remote file write notes.txt --from-local ./notes.txt --cwd /tmp
bifrost remote file edit README.md --edits '[{"start_line":1,"end_line":1,"replacement":"# Title\n"}]' --cwd /path/to/repo
bifrost remote file edit README.md --from-local ./edits.json --cwd /path/to/repo
bifrost remote file mkdir notes --parents --cwd /tmp
bifrost remote file move old.txt new.txt --cwd /tmp
bifrost remote file delete stale.txt --cwd /tmp
bifrost remote file patch --from-local ./change.diff --cwd /path/to/repo

bifrost remote traffic list --limit 20
bifrost remote traffic list --listener-port 50831
bifrost remote traffic get <id> --request-body --response-body
bifrost remote traffic search "keyword" --listener-port 50831 --req-body

bifrost remote keep-awake status
bifrost remote keep-awake on
bifrost remote keep-awake off
bifrost remote keep-awake mode set force_on
bifrost remote keep-awake mode get
```

`--ssh-key` 带路径时读取指定 key 文件；不带值时读取固定环境变量 `BIFROST_REMOTE_SSH_KEY`。环境变量名称固定，不支持自定义名称。

多远端编排时，固定把目标选择放在父命令位置：

```bash
export BIFROST_REMOTE_CLIENT_ID=devbox
bifrost remote file read Cargo.toml --cwd /repo
bifrost remote --client-id macbook exec --detach --shell-text "python3 check.py"
bifrost remote --client-id macbook job watch <call_id> --output-file ./check.log
```

`remote run` 是“远端跑本地脚本”的一等公民入口：CLI 会先用 `remote file scratch-dir` 与 `remote file write` 把本地脚本上传到授权目录，再通过 `remote exec` 以 argv 方式执行，避免 heredoc、反引号、管道符和 `/tmp` symlink escape 等问题。前台执行默认在结束后删除上传脚本；`--detach` 会保留脚本以便远端 job 后续读取。

普通 `remote exec` / streaming exec 的调用事件订阅有 300 秒无事件 idle timeout。长时间静默的构建、轮询、迁移或 `--wait` 命令应使用 `remote exec --detach` 或 `remote run --detach`，再通过 `remote job watch <call_id> --output-file <log>` 获取真实远端退出码，不要在 caller 侧用 `sleep` 轮询。

远程文件操作受远端 grant 的 file access policy 约束；`remote exec` 是最高权限路径，能运行任意 shell 命令，实际允许范围由远端 Shell Access policy 决定。

| 子命令 | 做什么 | 何时使用 |
| --- | --- | --- |
| `remote conn up/down/status` | 建立、撤销、检查远端连接授权 | 首次 pairing、授权过期、切换目标设备 |
| `remote traffic list/get/search` | 查询远端 Bifrost 的流量记录 | 远端机器才有真实流量证据时 |
| `remote file read/list/stat/glob/find/hash` | 远端只读文件操作 | 读取日志、配置、源码片段，受 file policy 限制 |
| `remote file write/edit/mkdir/move/delete/patch` | 远端写文件操作 | 需要 policy 允许写入；优先于 `remote exec` 做结构化文件修改 |
| `remote run` | 上传本地脚本到远端 scratch-dir 并执行 | 需要先传一段本地 py/js/sh 等诊断脚本到远端运行时 |
| `remote exec` | 在远端执行 shell | 最高权限路径，适合执行已有 CLI、测试、诊断命令 |
| `remote keep-awake` | 管理远端 macOS 防睡眠 | 需要远端 grant 包含 `remote_power_mgmt` |

`--relay-url` 的优先级为：命令行显式值 > 当前运行服务的 sync 配置 > 本地配置文件 > 内置默认值。`--client-id` 用于从多个已保存连接中选择目标设备前缀；显式参数优先于 `BIFROST_REMOTE_CLIENT_ID`。

### macOS 防睡眠（keep-awake）

```bash
bifrost keep-awake status
bifrost keep-awake on
bifrost keep-awake off
bifrost keep-awake mode set force_on
bifrost keep-awake mode set auto
bifrost keep-awake mode get
```

该命令通过本机 Admin API 管理 macOS IOKit power assertion；非 macOS 平台会返回不支持。

### IM Gateway（im）

```bash
bifrost im
bifrost im provider list
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --enabled true
bifrost im target add oncall --receive-id-type chat_id --receive-id oc_xxx
bifrost im send --text "hello owner"
bifrost im send --target oncall --text "hello group"
bifrost im send --image-file ./alert.png
bifrost im send --card-title "Deploy report" --card-text "**Done**" --card-image-file ./chart.png
bifrost im route add deploy --event message.receive --regex '^/deploy' --script-file ./deploy.sh
bifrost im schedule add health --target oncall --cron '*/5 * * * *' --script-file ./check.sh
bifrost im schedule add agent-daily --target oncall --cron '0 9 * * *' --agent-prompt 'Summarize traffic' --agent-runner-id codex --agent-model gpt-5 --agent-reasoning-effort high --agent-enable web_search
bifrost im messages list --direction inbound
```

`provider add` 支持两种配置方式：

- 交互式配置：Feishu / Weixin provider 只传 provider ID、`--type` 和 Runner 即可，例如 `bifrost im provider add feishu-main --type feishu --runner traex` 或 `bifrost im provider add weixin-main --type weixin --runner codex`。Feishu 会在终端打印授权 URL 和二维码，CLI 保持等待，用户完成授权后自动创建并连接 provider；Weixin 会在终端打印登录二维码，扫码确认后自动完成创建和连接。
- 手动凭据配置：已持有 App ID / Secret 时继续传 `--app-id`、`--secret env:NAME`、`--owner-open-id` 等字段。Feishu / Weixin / WeChat provider 仍必须绑定 Runner。

Runner 选择规则：交互式终端中未传 `--runner` 时会展示启用 Runner 列表，使用上下键和回车选择；stdin 非交互时必须显式传 `--runner`，否则命令会报错并列出可用 Runner。`--runner` 会校验是否存在且启用，支持常见别名如 `codex`、`traex` / `trae`、`Claude Code`；错误信息会同时提示当前可用 Runner 和内置默认 Runner 类型。Feishu / Weixin provider 的 `base_url` 由 provider 类型固定管理，CLI 不接受 `--base-url`，避免把不同平台的 OpenAPI 入口写错到 provider 配置中。

需要 provider 的 IM 命令都支持 `--provider <id>` 显式指定。未提供 `--provider` 时，CLI 会复用统一选择逻辑：只有一个 enabled provider 时自动选择；多个 enabled provider 且处于交互式终端时展示列表让用户选择；多个 provider 且 stdin 非交互时会要求显式传 `--provider`。`bifrost im send` 未传 `--target` 时默认发送给所选 provider 的 owner，因此 provider 需要配置 `owner_open_id`（可在创建时用 `--owner-open-id`，或由后端连接飞书后自动检测）。

IM 通道建立后，Bifrost 会先推送上线通知和可用命令帮助。帮助内容会按当前绑定 Runner 过滤：所有 Runner 都显示 `/help`、`/status`、`/cwd`、`/runner`、`/q`、`/rq`、`/stop` 等 IM 通道命令；内置 Bifrost Agent 才显示 `/remember`、`/memories`、`/forget`、`/compact`、`/goal`、`/g` 等内置 Agent 命令；Codex / Traex / Claude Code 等外部 Runner 只在适配器支持时显示 `/models`、`/model`、`/efforts`、`/effort`。

Agent 配置支持 `default_message_channel`，用于给 turn 级 `send_msg` 工具和 Agent 创建的 schedule 提供默认 IM 发送目标。手动创建 schedule 时仍应显式绑定目标（例如 `--target oncall`，或 API 的 `message_channel`），避免任务执行时把通知发到最近一次对话；通过 IM 消息触发的 Agent 创建 schedule 时会自动继承当前来源通道，通过 `/agent/chat` 创建时会回退到 `default_message_channel`。

`bifrost im schedule add/update` 创建 Agent schedule 时可用 `--agent-runner-id` 选择 Runner，并通过 `--agent-model`、`--agent-profile`、`--agent-profile-v2`、`--agent-sandbox`、`--agent-reasoning-effort`、`--agent-reasoning-summary`、`--agent-approval-policy`、`--agent-danger-full-access`、`--agent-bypass-hook-trust`、`--agent-skip-git-repo-check`、`--agent-ignore-user-config`、`--agent-ignore-rules`、`--agent-add-dir`、`--agent-config`、`--agent-enable`、`--agent-disable` 等参数写入 `agent.adapter_config`。这些 schedule 级参数会在运行时覆盖 Runner 默认 Codex adapter 配置；历史 `--agent-search` 仅作为兼容入口映射为 `--enable web_search`，不再生成当前 Codex CLI 不支持的 `--search`。

### 本地语音输入（ai voice）

`bifrost ai voice` 管理本机 Voice Input Runtime。它只使用本地 ASR 能力；原始音频默认不上传云端、不落盘。系统音频和单应用音频会先返回能力状态，权限或平台不满足时返回 `needs_permission` / `unsupported` / `source_unavailable`，不会静默录制。

```bash
bifrost ai voice sources --json
bifrost ai voice listen --source mic --duration 15 --model Qwen3-ASR-0.6B --chunk-ms 1000 --format jsonl
bifrost ai voice listen --source file --input-file ./sample.wav --duration 7 --model Qwen3-ASR-0.6B --chunk-ms 1000 --format jsonl
bifrost ai voice listen --source file --input-file ./sample.wav --duration 7 --model Qwen3-ASR-0.6B --provider qwen3_stateful_streaming --format jsonl
bifrost ai voice listen --source file --input-file ./sample.wav --duration 7 --model Qwen3-ASR-1.7B --provider qwen3_stateful_streaming --allow-stateful-large-model --format jsonl
bifrost -p 18887 ai asr start --model Qwen3-ASR-0.6B --language chinese
bifrost ai voice listen --source mic --dry-run --text "请打开宽增"
bifrost ai voice vocabulary import ./terms.txt
bifrost ai voice vocabulary list --json
```

词汇文件格式为每行一个 `canonical=alias1,alias2`，例如：

```txt
Bifrost=宽增,白 Frost
```

V1 中 `listen --source mic` 在 macOS 上通过本机 `ffmpeg` 持续输出 16kHz mono PCM。CLI 会连接当前 Bifrost 管理端的 `/_bifrost/api/voice/listen-ws`，等本机 Voice service ready 后再开始推送音频，并持续把服务端返回的 `asr_partial/asr_stable_delta/asr_final_utterance/done` 事件打印到 stdout。Voice 实时输入只支持 `qwen3_stateful_streaming` provider：每个 Voice WebSocket session 都由 Bifrost daemon 拉起独立 `bifrost ai voice worker` 子进程，worker 内部创建 Rust `qwen3-asr` `StreamingState`，daemon 通过 stdio 持续喂 PCM chunk 并流式返回 partial/stable/final，避免模型加载和推理占用代理主进程资源；旧的 ASR server 窗口式 realtime provider 不再作为 Web/CLI 实时输入方案。`asr_partial` 是可变的实时假设，UI 可以局部替换展示；只有静音边界、Finish/Stop 或最长连续 utterance 边界触发的 `asr_stable_delta` / `asr_final_utterance` 才会追加到 committed transcript。Voice 输入默认使用 `Qwen3-ASR-0.6B` 降低本机资源压力；`Qwen3-ASR-1.7B` 必须在高性能机器上显式传 `--allow-stateful-large-model`，Web/WS 调用可传 `allow_stateful_17b=1`，自动化实验也可继续使用 `BIFROST_VOICE_ALLOW_STATEFUL_17B=1`。`--chunk-ms` 作为 stateful streaming chunk size 传给本地模型，不会触发 HTTP whole-file 窗口转写。离线文件转写继续使用 `bifrost ai asr ...` 管理的本机 ASR server；`--source file --input-file` 属于实时链路的文件回放，会使用 `ffmpeg -re` 以实时速度推流验证 Web/CLI realtime 行为。`--source system` 和 `--source app` 先提供 capability 状态与明确错误，真实捕获将在 source discovery、权限和 Core Audio / ScreenCaptureKit 集成就绪后启用。

资源边界：实时 session 收到 1 秒左右静音会提交当前 partial 并关闭当前 worker；连续说话约 30 秒会强制形成一次 stable boundary，避免 transcript 和 `StreamingState` 无界增长；如果 Start 后持续静音或 WebSocket 长时间没有音频，daemon 会先提交已有 partial，再停止/卸载 worker，并可发送 `worker_idle_unloaded` 事件。

IM Gateway 子命令按对象划分：

| 对象 | 说明 | 常见动作 |
| --- | --- | --- |
| `provider` | IM 平台连接配置，例如 Feishu、WeChat、Webhook | `list/add/update/delete/test` |
| `target` | 消息接收目标，例如群、用户、owner | `list/add/update/delete` |
| `send` | 主动发送消息 | `--text`、`--image-file`、`--image-key`、`--card-file`、`--card-json`、`--card-title`、`--card-text`、`--card-image-file`、`--card-image-key`、`--target`、`--provider` |
| `route` | 把收到的事件路由到脚本 | 按 event、regex、script 触发 |
| `schedule` | 定时执行脚本并发送结果 | cron 表达式、目标、脚本文件 |
| `history` / `messages` | 查看事件、任务、消息历史 | 排查路由和发送结果 |

涉及密钥的参数支持 `env:NAME` 形式，例如 `--secret env:FEISHU_APP_SECRET`，避免把 secret 写进 shell history 或文档。

### 导入/导出（import / export）

```bash
bifrost import ./backup.bifrost
bifrost import --detect-only ./backup.bifrost

bifrost export rules demo -o ./rules.bifrost
bifrost export values -o ./values.bifrost
bifrost export scripts request/demo -o ./scripts.bifrost
```

### 指标（metrics）

```bash
bifrost metrics summary
bifrost metrics apps
bifrost metrics hosts
bifrost metrics history --limit 200
```

### Shell 补全（completions）

```bash
bifrost completions bash
bifrost completions zsh
bifrost completions fish
bifrost completions elvish
bifrost completions powershell
```

### 安装 Skill（install-skill）

```bash
bifrost install-skill -y
bifrost install-skill -t codex -y
bifrost install-skill -t trae -y
bifrost install-skill -t github-copilot -y
bifrost install-skill -t universal -y
bifrost install-skill -t all -y
bifrost install-skill --cwd -y
```

`install-skill` 只安装 Bifrost Agent Skill 文档，让 Agent 知道如何调用本机或远端 Bifrost CLI。它不会启动代理、不会开启系统代理、不会导入规则，也不会授予远端 shell 权限。默认目标为 `all`，会覆盖安装到所有支持的 AI 编程工具；`--cwd` 安装到当前项目目录，`--dir` 可指定自定义目录，二者互斥。`BIFROST_INSTALL_SKILL_SOURCE` 可覆盖安装源，`BIFROST_INSTALL_SKILL_DIR` 可在测试中覆盖默认安装目录，普通用户通常不需要设置。

安装内容包含通用 `bifrost` skill 和专用 `bifrost-remote` skill。用户只是在本机启动代理、写规则、查本机流量时使用通用 skill；如果要连接另一台机器、使用 pair code / SSH key、远程查询流量、上传脚本执行或通过授权 shell 操作目标设备，应切换到 `bifrost-remote` skill 的完整流程。

和 Agent 协作沉淀业务 skill 时，推荐先执行 `bifrost install-skill -t codex -y` 或面向当前工具的目标安装，再用 `capture wait`、`traffic list/get/search` 给 Agent 提供真实流量证据；流量详情当前按捕获原文输出，沉淀可复用 skill 前必须手动移除 token、cookie、手机号、邮箱等敏感信息。
