# CLI 详细命令

本文档集中说明 `bifrost` CLI 的常用参数与命令。

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
| `--log-output <TARGETS>` | 日志输出目标：`console` / `file` / `console,file` | `console` |
| `--log-dir <DIR>` | 日志目录（默认：`<data_dir>/logs`） | 无 |
| `--log-retention-days <DAYS>` | 日志保留天数 | `7` |
| `-h, --help` | 显示帮助 | - |
| `-v, -V, --version` | 显示版本号 | - |

## `start` 命令

常见示例：

```bash
bifrost start
bifrost start --daemon
bifrost restart
bifrost -p 9000 start
bifrost -p 9900 --socks5-port 1080 start
bifrost start --skip-cert-check
bifrost start --no-intercept
bifrost start --intercept
bifrost start --intercept-exclude "*.example.com,internal.corp.com"
bifrost start --intercept-include "*.api.local"
bifrost start --app-intercept-include "*Chrome,*curl"
bifrost start --rules "example.com host://127.0.0.1:3000"
bifrost start --rules-file ./my-rules.txt
bifrost start --access-mode whitelist --whitelist "192.168.1.100,10.0.0.0/8"
bifrost start --allow-lan
bifrost start --proxy-user admin:password123
bifrost start --system-proxy
bifrost start --unsafe-ssl
bifrost start --disable-badge-injection
bifrost start --enable-badge-injection
```

当检测到已有 Bifrost 进程在运行时，`bifrost start` 会在终端提示是否重启：输入 `y/yes` 将停止旧进程并重新启动；输入 `n/no` 将取消本次启动。

如果需要在脚本/CI 中跳过交互，可以使用 `-y/--yes` 自动确认重启。

参数摘要：

| 参数 | 说明 |
| --- | --- |
| `-d, --daemon` | 守护进程模式 |
| `--skip-cert-check` | 跳过 CA 证书安装检查 |
| `--access-mode <MODE>` | `local_only` / `whitelist` / `interactive` / `allow_all` |
| `--whitelist <IPS>` | 客户端 IP 白名单，支持 CIDR |
| `--allow-lan` | 允许局域网访问 |
| `--proxy-user <USER:PASS>` | 代理认证账号（可重复指定） |
| `--intercept` | 启用 TLS 拦截 |
| `--no-intercept` | 禁用 TLS 拦截 |
| `--intercept-exclude <DOMAINS>` | TLS 拦截排除域名 |
| `--intercept-include <DOMAINS>` | TLS 拦截白名单（最高优先级，即使全局关闭也生效） |
| `--app-intercept-exclude <APPS>` | TLS 拦截排除应用（进程名通配） |
| `--app-intercept-include <APPS>` | TLS 拦截应用白名单（最高优先级） |
| `--unsafe-ssl` | 跳过上游证书校验，仅建议测试环境使用 |
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

`port` 命令需要主代理正在运行。临时端口与主端口共享同一个 `BIFROST_DATA_DIR` 中的规则、values、scripts、证书、流量记录等数据；端口绑定只保存“这个端口选择哪些规则集”。临时端口流量不受默认规则 enabled/disabled 状态影响：只有 `port bind` / `port update` 显式绑定的本地规则或 Group 规则会进入该临时端口的 resolver。

`--rule` 引用本地规则名；`--group-rule` 使用 `<group_id>/<rule_name>` 格式；`--rule-file` 直接绑定规则文件；`--rule-text` 直接绑定规则原文。销毁临时端口只关闭该端口监听，不删除共享规则数据，也不影响主端口。

临时端口绑定状态只在当前运行进程内存里生效，不写入持久配置。Bifrost 重启后临时端口会被重置，不会自动重新监听，也不会恢复之前的规则绑定；需要时请重新执行 `bifrost port bind ...`。

Traffic 记录会带监听端口信息，即使请求没有命中任何规则也会记录来源端口：`traffic list` 的表格包含 `PORT` 列，JSON compact 字段为 `lp`，`traffic get` 详情字段为 `listener_port`。这用于区分同一数据目录内主端口和临时端口产生的流量。

多端口推荐工作流：

1. 先启动主代理端口，例如 `bifrost start -p 8811 --no-system-proxy`。
2. 保持主端口继续承载默认启用规则。
3. 为临时调试场景按需再开多个端口：
   - `bifrost port bind --port 18888 --rule local-dev`
   - `bifrost port bind --port 18889 --group-rule 7152084678483132446/abc`
   - `bifrost port bind --port 0 --rule-file ./temp-debug.bifrost`
   - `bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"`
4. 用 `bifrost port list` 查看当前所有临时端口；用 `bifrost port show <port>` 看绑定元信息；用 `bifrost port active <port>` 看这个端口当前真正生效的规则视图。
5. 当一个临时端口需要切换到另一组规则时，使用 `bifrost port update <port> ...` 传入新的完整规则引用集合。
6. 调试结束后执行 `bifrost port destroy <port>` 回收对应监听端口。

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
bifrost traffic get <id> --request-body --response-body
bifrost traffic search "keyword"
bifrost traffic search "keyword" --listener-port 50831
bifrost traffic search "keyword" --proxy-port 50831
bifrost search "keyword"
bifrost search "keyword" --method POST --host api.openai.com --path /v1/responses
bifrost search "keyword" --req-header
bifrost search "keyword" --res-body
```

`bifrost search` 与 `bifrost traffic search` 等价，支持关键词搜索、基础过滤器与搜索范围控制。

基础过滤器：

| 参数 | 说明 |
| --- | --- |
| `--method <METHOD>` | 按 HTTP 方法过滤，如 `GET`、`POST` |
| `--host <TEXT>` | 按 Host 包含匹配过滤 |
| `--path <TEXT>` | 按 Path 包含匹配过滤 |
| `--status <FILTER>` | 按状态段过滤，如 `2xx`、`4xx`、`5xx`、`error` |
| `--protocol <PROTO>` | 按协议过滤，如 `HTTP`、`HTTPS`、`WS`、`WSS` |
| `--domain <PATTERN>` | 按域名模式过滤 |
| `--content-type <TYPE>` | 按内容类型过滤，如 `json`、`html`、`form` |
| `--listener-port <PORT>` / `--proxy-port <PORT>` | 按流量入口代理端口过滤；`traffic list` 中的 `--port` 仍表示 Admin API 端口 |

入口端口过滤用于区分主代理端口、临时代理端口、远端代理端口产生的流量。例如临时端口 `50831` 的请求可以用 `traffic list --listener-port 50831` 或 `traffic search "keyword" --proxy-port 50831` 查询；顶层 `bifrost search` 与 `bifrost traffic search` 的过滤语义一致。

搜索范围：

| 参数 | 说明 |
| --- | --- |
| `--url` | 仅搜索 URL / Path |
| `--req-header` | 仅搜索请求头 |
| `--res-header` | 仅搜索响应头 |
| `--req-body` | 仅搜索请求体 |
| `--res-body` | 仅搜索响应体 |
| `--headers` | 同时搜索请求头与响应头 |
| `--body` | 同时搜索请求体与响应体 |

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
```

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
bifrost rule update <name> --content "new rule"
bifrost rule update <name> --file rules.txt
bifrost rule enable <name>
bifrost rule disable <name>
bifrost rule delete <name>
bifrost rule show <name>
bifrost rule get <name>
bifrost rule sync
bifrost rule rename <name> <new_name>
bifrost rule reorder <name1> <name2> ...
```

- `rule active` 需要代理服务运行中（通过管理接口获取运行时已启用规则摘要）

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

# 更新 group 规则
bifrost group rule update <group_id> <name> --content "new rule"
bifrost group rule update <group_id> <name> --file rules.txt

# 启用/禁用 group 规则
bifrost group rule enable <group_id> <name>
bifrost group rule disable <group_id> <name>

# 删除 group 规则
bifrost group rule delete <group_id> <name>
```

- `group` 命令需要代理服务运行中（通过 admin API 通信）
- `group list` 支持 `--keyword` 模糊搜索和 `--limit` 限制结果数
- `group rule add/update` 通过 `--content` 或 `--file` 提供规则内容

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
bifrost upgrade -y
bifrost upgrade -y --restart
```

### 同步（sync）

```bash
bifrost sync status
bifrost sync login
bifrost sync login --token "$BIFROST_SYNC_TOKEN" --url https://bifrost.bytedance.net
bifrost sync logout
bifrost sync run
bifrost sync config --enabled true --auto-sync true --remote-url https://example.com
```

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
bifrost setting grant update --grant-id <grant-id> --scope shell --file-access read
bifrost setting grant revoke --grant-id <grant-id>
```

### 远程调用（remote）

`remote` 通过 relay 对另一台已授权的 Bifrost 实例执行操作。全局参数 `--relay-url` 的优先级为：命令行显式值 > 当前运行服务的 sync 配置 > 本地配置文件 > 内置默认值；`--client-id` 用于在多个已保存连接中选择目标前缀。

```bash
bifrost remote conn up <pair-code>
bifrost remote conn up --ssh-key ./bifrost-device.key --label "dev-mac"
bifrost remote conn status
bifrost remote conn down
bifrost remote conn down --grant-id <grant-id>
bifrost remote conn down --all

bifrost remote exec --shell-text "pwd && ls"
bifrost remote exec -- /bin/zsh -lc 'bifrost status'

bifrost remote file read README.md --cwd /path/to/repo
bifrost remote file list src --depth 2 --cwd /path/to/repo
bifrost remote file find "TODO" --path src --cwd /path/to/repo
bifrost remote file write notes.txt --content "hello" --cwd /tmp
bifrost remote file patch --patch-file ./change.diff --cwd /path/to/repo

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

远程文件操作受远端 grant 的 file access policy 约束；`remote exec` 是最高权限路径，能运行任意 shell 命令，实际允许范围由远端 Shell Access policy 决定。

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
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --enabled true
bifrost im target add oncall --receive-id-type chat_id --receive-id oc_xxx
bifrost im send --text "hello owner"
bifrost im send --target oncall --text "hello group"
bifrost im route add deploy --event message.receive --regex '^/deploy' --script-file ./deploy.sh
bifrost im schedule add health --target oncall --cron '*/5 * * * *' --script-file ./check.sh
bifrost im messages list --direction inbound
```

需要 provider 的 IM 命令都支持 `--provider <id>` 显式指定。未提供 `--provider` 时，CLI 会复用统一选择逻辑：只有一个 enabled provider 时自动选择；多个 enabled provider 且处于交互式终端时展示列表让用户选择；多个 provider 且 stdin 非交互时会要求显式传 `--provider`。`bifrost im send` 未传 `--target` 时默认发送给所选 provider 的 owner，因此 provider 需要配置 `owner_open_id`（可在创建时用 `--owner-open-id`，或由后端连接飞书后自动检测）。

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
```

### 安装 Skill（install-skill）

```bash
bifrost install-skill -y
bifrost install-skill -t trae -y
bifrost install-skill --cwd -y
```
