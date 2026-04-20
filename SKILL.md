---
name: "bifrost"
description: "使用 bifrost 命令行工具管理代理生命周期、规则、Group 规则、证书、脚本、系统代理、运行时配置与流量查询。当用户提到以下任意场景时触发：1) 启动/停止/检查 bifrost 代理；2) 配置 TLS 拦截（域名白名单、应用白名单）；3) 调试或管理规则/Group 规则/脚本；4) 查看流量记录、搜索请求；5) 通过一个少于 6 位的数字 ID 获取请求详情（如「获取 57544 的详情」「获取 47544 请求的内容」「查看 12345」等）；6) 修改 values/config/CA 证书/系统代理。常见触发表述：'使用 bifrost 获取 xxxxx 的详情''获取 xxxxx 的请求内容''查看 xxxxx 的内容''bifrost traffic get xxxxx' 等。"
---

# Bifrost

该技能用于指导 Agent 直接使用 `bifrost` CLI 完成代理启动、配置修改、规则调试、Group 规则管理、脚本管理和流量排查，而不是绕过 CLI 直接改底层数据文件。

## 启动时必须执行的自检流程

每次技能被触发后，Agent 在正式执行任何 `bifrost` 命令前，都必须先完成下面的启动检查：

1. 检查 `bifrost` 是否存在
2. 如果不存在，自动安装最新版本
3. 如果存在，检查是否可执行
4. 如果可执行，优先升级到最新版本，再继续后续任务
5. 安装或升级完成后，再次验证 `bifrost --version`
6. 执行 bifrost install-skill -y 进行更新 skill 描述

除非用户明确禁止联网或禁止改动本机环境，否则不要跳过这个流程。

## 1. 检查 bifrost 是否存在

优先检查当前环境是否已经安装并可执行：

```bash
command -v bifrost
bifrost --version
```

- `command -v bifrost` 有输出路径，说明二进制已在 `PATH` 中
- `bifrost --version` 成功返回，说明 CLI 可以直接使用
- 如果仓库源码在本地但 `bifrost` 尚未加入 `PATH`，可退回源码方式检查：

```bash
cargo run -p bifrost -- --version
```

## 2. 如果 bifrost 不存在，自动安装最新版本

优先使用官方安装脚本直接安装最新版本：

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

安装完成后，必须重新执行：

```bash
command -v bifrost
bifrost --version
```

如果安装脚本失败，再按环境降级处理：

```bash
# macOS / Homebrew
brew tap bifrost-proxy/bifrost
brew install bifrost
```

如果用户不希望安装系统级二进制，才退回源码构建；注意这里应从官方仓库拉取源码，而不是假设当前工作区就是 Bifrost 仓库：

```bash
git clone https://github.com/bifrost-proxy/bifrost.git /tmp/bifrost
cd /tmp/bifrost
cd web && pnpm install && pnpm build && cd ..
cargo build --release -p bifrost
./target/release/bifrost --version
```

## 3. 如果 bifrost 已存在，自动升级到最新版本

只要用户没有明确要求"固定当前版本"或"禁止升级"，都应在首次检查通过后继续执行：

```bash
bifrost upgrade -y
bifrost --version
bifrost install-skill -y 
```

- `bifrost upgrade -y` 会跳过确认提示
- 若升级失败，再回退到官方安装脚本重新安装最新版本
- 如果用户只允许最小改动，至少要告知"当前跳过升级，后续行为基于现有版本"

## 4. 完成自检后再进入正式任务

1. 先确认目标是"运行代理"还是"管理已有代理"。
2. 先用 `bifrost --help` 或 `bifrost <command> --help` 补充具体参数，再执行高影响命令。
3. 会改本机网络环境的命令必须谨慎：`system-proxy enable/disable`、`start --system-proxy`、`start --cli-proxy`。

## 关键约束

- `bifrost` 不带子命令时，等价于 `bifrost start`
- `config`、`traffic`、`search`、`group`、多数 `status` 相关能力依赖"已有运行中的代理"
- `rule`、`value`、`script`、`ca` 主要操作本地数据目录，不一定要求代理正在运行
- `system-proxy` 会修改操作系统代理设置；除非用户明确要求，不要主动启用

## 命令能力映射

### 1. 生命周期

```bash
bifrost status                     # 启动前必须先检查是否已有服务在运行
bifrost start -p 9900              # 仅当 status 显示无运行中的服务时才启动
bifrost start -p 9900 --daemon
bifrost status --tui
bifrost stop
```

- **启动前必须先执行 `bifrost status` 检查**，如果已有服务在运行，直接复用，不要尝试启动新的
- 前台调试优先用普通 `start`
- 需要后台运行时才用 `--daemon`
- 若未指定端口，默认 `9900`

### 2. Start 完整参数

```bash
bifrost start [OPTIONS]
  -p, --port <PORT>                   代理端口（覆盖全局 -p）
  -H, --host <HOST>                   监听地址（覆盖全局 -H）
      --socks5-port <PORT>            独立 SOCKS5 端口（覆盖全局）
  -d, --daemon                        后台守护模式运行
      --skip-cert-check               跳过 CA 证书安装检查
      --access-mode <MODE>            访问模式：local_only|whitelist|interactive|allow_all
      --whitelist <IPS>               客户端 IP 白名单（逗号分隔，支持 CIDR）
      --allow-lan                     允许局域网（私有网络）客户端
      --proxy-user <USER:PASS>        代理认证凭据（USER:PASS 格式，可重复指定）
      --intercept                     启用 TLS/HTTPS 拦截
      --no-intercept                  禁用 TLS/HTTPS 拦截（默认）
      --intercept-exclude <DOMAINS>   排除域名不拦截（逗号分隔，支持通配符）
      --intercept-include <DOMAINS>   强制拦截域名（最高优先级，即使全局关闭也生效）
      --app-intercept-exclude <APPS>  排除应用不拦截（逗号分隔，支持通配符）
      --app-intercept-include <APPS>  强制拦截应用（最高优先级）
      --unsafe-ssl                    跳过上游 TLS 证书校验（危险，仅测试用）
      --enable-badge-injection        启用 HTML 页面注入 Bifrost 徽章
      --disable-badge-injection       禁用 HTML 页面注入 Bifrost 徽章
      --no-disconnect-on-config-change  TLS 配置变更时不自动断开受影响连接
      --rules <RULE>                  代理规则（可重复指定）
      --rules-file <PATH>             规则文件路径
      --system-proxy                  启用系统代理
      --proxy-bypass <LIST>           系统代理绕行列表（逗号分隔）
      --cli-proxy                     代理运行期间启用 CLI 代理环境变量
      --cli-proxy-no-proxy <LIST>     CLI 代理 no-proxy 列表（逗号分隔）
      -y, --yes                       自动回答 yes
```

TLS 拦截优先级（从高到低）：

1. 规则级别（`tlsIntercept://`、`tlsPassthrough://`）
2. `--intercept-include` / `--app-intercept-include`：**域名/应用白名单强制拦截（推荐方式）**
3. `--intercept-exclude` / `--app-intercept-exclude`：强制不拦截
4. `--intercept` / `--no-intercept`：全局开关（**默认关闭，不推荐全局开启**）

### 3. TLS / CA

```bash
bifrost ca generate
bifrost ca generate -f          # 强制重新生成
bifrost ca install
bifrost ca info
bifrost ca export -o ./bifrost-ca.pem
```

**⚠️ TLS 拦截默认关闭，不建议全局开启 `--intercept`。推荐使用域名/应用白名单按需解包：**

```bash
# ✅ 推荐：仅对指定域名启用 TLS 解包（无需全局 --intercept）
bifrost start --intercept-include 'api.example.com,*.target.local'

# ✅ 推荐：仅对指定应用启用 TLS 解包
bifrost start --app-intercept-include '*Chrome,*curl'

# ✅ 域名 + 应用组合白名单
bifrost start --intercept-include '*.api.local' --app-intercept-include '*Chrome'

# ⚠️ 不推荐：全局开启后排除（拦截范围过大，影响系统稳定性）
bifrost start --intercept --intercept-exclude '*.apple.com,*.microsoft.com'
```

- `--intercept-include` / `--app-intercept-include` 为最高优先级，即使全局 TLS 关闭也会对匹配的域名/应用生效
- 需要解密 HTTPS 时，先处理 `ca`（生成 + 安装），再配置白名单
- 若只是转发 HTTPS 而非查看明文，保持默认即可（`--no-intercept`）
- 应用级别白名单支持通配符匹配进程名

### 4. 规则管理

```bash
bifrost rule list # 列规则基本信息，私有规则，非小组规则
bifrost rule active # 查看激活的规则
bifrost rule add demo -c "example.com host://127.0.0.1:3000"
bifrost rule add demo -f ./rules/demo.txt
bifrost rule update demo -c "example.com host://127.0.0.1:4000"
bifrost rule update demo -f ./rules/demo-v2.txt
bifrost rule show demo                     # 别名：get
bifrost rule enable demo
bifrost rule disable demo
bifrost rule delete demo
bifrost rule rename demo new-demo              # 重命名规则
bifrost rule reorder                           # 重新排序规则优先级
bifrost rule sync                          # 与远端服务器同步规则
```

- 新增/更新规则时，`--content` 和 `--file` 至少提供一个
- 单次验证可直接用 `start --rules "..."`
- 多条或长期规则优先放入规则文件，再用 `--rules-file`

### 5. Group 管理

```bash
# Group 查询
bifrost group list                            # 列出所有 groups
bifrost group list -k "team" -l 20            # 按关键词搜索，限制结果数
bifrost group show <group_id>                 # 查看 group 详情

# Group 规则查询
bifrost group rule list <group_id>            # 列出 group 下所有规则
bifrost group rule show <group_id> <name>     # 查看 group 规则详情

# Group 规则增删改
bifrost group rule add <group_id> <name> -c "example.com host://127.0.0.1:3000"
bifrost group rule add <group_id> <name> -f ./rules/demo.txt
bifrost group rule update <group_id> <name> -c "new content"
bifrost group rule update <group_id> <name> -f ./rules/demo-v2.txt
bifrost group rule delete <group_id> <name>

# Group 规则启用/禁用
bifrost group rule enable <group_id> <name>
bifrost group rule disable <group_id> <name>
```

- **需要代理运行中**：`group` 命令通过 admin API 通信，需先 `bifrost start`
- Group 规则新增/更新时，`--content` 和 `--file` 至少提供一个（add 可以不带，默认空内容）
- `group rule show` 别名：`get`
- `group list` 支持 `-k/--keyword` 模糊搜索、`-l/--limit` 限制最大结果数（默认 50）和 `-o/--offset` 分页偏移

### 6. 脚本管理
> 支持 QuickJS 引擎执行 JS 脚本
```bash
bifrost script list
bifrost script list -t request             # 按类型过滤：request, response, decode
bifrost script add request demo -c 'module.exports = ...'
bifrost script add response demo -f ./scripts/demo.js
bifrost script update request demo -c '...'
bifrost script update response demo -f ./scripts/demo-v2.js
bifrost script show demo                   # 模糊匹配，跨类型查找
bifrost script show request demo           # 精确指定类型
bifrost script run demo                    # 使用内置 mock 数据测试脚本
bifrost script run request demo            # 精确指定类型运行
bifrost script rename request demo new-name  # 重命名脚本
bifrost script delete request demo
```

- 脚本类型：`request`（请求修改）、`response`（响应修改）、`decode`（解码）
- 类型别名：`req`→request、`res`→response、`dec`→decode
- `show` 和 `run` 支持只传名称进行模糊匹配；如有歧义需指定类型
- `run` 会使用内置 mock 请求/响应数据执行脚本，输出修改结果和日志

### 7. 变量值

```bash
bifrost value list
bifrost value add LOCAL_SERVER 127.0.0.1:3000    # 别名：set
bifrost value show LOCAL_SERVER                  # 别名：get
bifrost value update LOCAL_SERVER 127.0.0.1:4000
bifrost value import ./values.json               # 支持 .txt/.kv/.json
bifrost value delete LOCAL_SERVER
```

- 规则中可使用 `${NAME}` 和 `${env.VAR_NAME}`
- 需要复用环境相关地址或 token 时，优先用 `value set` 而不是把值硬编码到规则里

### 8. 访问控制

```bash
bifrost whitelist status
bifrost whitelist list
bifrost whitelist add 192.168.1.0/24
bifrost whitelist remove 192.168.1.0/24
bifrost whitelist allow-lan true
bifrost whitelist mode                         # 查看当前访问模式
bifrost whitelist mode interactive             # 设置访问模式
bifrost whitelist pending                      # 查看待处理的访问请求
bifrost whitelist approve <ip>                 # 批准访问请求（按 IP）
bifrost whitelist reject <ip>                  # 拒绝访问请求（按 IP）
bifrost whitelist clear-pending                # 清空待处理请求
bifrost whitelist add-temporary <ip>           # 添加临时白名单
bifrost whitelist remove-temporary <IP>        # 移除临时白名单
```

- 默认应偏向最小暴露面
- 只有明确需要局域网访问时，再配合 `allow-lan` 或白名单

### 9. 代理认证

```bash
bifrost start --proxy-user admin:password123
bifrost start --proxy-user user1:pass1 --proxy-user user2:pass2
```

- 通过运行时配置管理：

```bash
bifrost config set access.userpass.enabled true
bifrost config add access.userpass.accounts 'user:pass'
bifrost config set access.userpass.loopback-requires-auth false
```

### 10. 系统代理

```bash
bifrost system-proxy status
bifrost system-proxy enable --host 127.0.0.1 --port 9900 --bypass 'localhost,127.0.0.1,*.local'
bifrost system-proxy disable
```

- 这是高影响命令，可能触发管理员权限
- 没有用户明确授权时，不要主动修改系统代理

### 11. 运行时配置

```bash
bifrost config show
bifrost config show --json
bifrost config show --section tls            # 按 section 过滤：tls, traffic, access
bifrost config get tls.enabled
bifrost config get tls.enabled --json
bifrost config set tls.enabled true
bifrost config add tls.exclude '*.example.com'
bifrost config remove tls.exclude '*.example.com'
bifrost config reset tls.enabled -y
bifrost config reset all -y                  # 重置所有配置
bifrost config clear-cache -y
bifrost config disconnect example.com
bifrost config disconnect-by-app Chrome       # 按应用断开连接
bifrost config performance                    # 查看性能概览
bifrost config websocket                      # 查看活跃 WebSocket 连接
bifrost config connections                    # 查看活跃代理连接
bifrost config memory                         # 查看内存诊断信息
bifrost config export -o ./config.toml --format toml
bifrost config export --format json
```

- `config` 走的是运行中代理的管理接口，不是直接改静态文件
- 修改后若涉及 TLS 或连接行为，必要时执行 `config disconnect <domain>` 触发重连验证
- 查询前先确认目标实例端口；如有显式端口，使用同一套 `-p`

可用的配置键：

| Section | Key                                          | 类型 | 说明                                                 |
| ------- | -------------------------------------------- | -- | -------------------------------------------------- |
| server  | `server.timeout-secs`                        | 数值 | 服务器超时秒数                                            |
| server  | `server.http1-max-header-size`               | 大小 | HTTP/1.1 最大请求头大小                                   |
| server  | `server.http2-max-header-list-size`          | 大小 | HTTP/2 最大头列表大小                                     |
| server  | `server.websocket-handshake-max-header-size` | 大小 | WebSocket 握手最大头大小                                  |
| tls     | `tls.enabled`                                | 布尔 | TLS 拦截开关                                           |
| tls     | `tls.unsafe-ssl`                             | 布尔 | 跳过上游证书校验                                           |
| tls     | `tls.disconnect-on-change`                   | 布尔 | 配置变更时自动断开连接                                        |
| tls     | `tls.exclude`                                | 列表 | TLS 拦截排除域名                                         |
| tls     | `tls.include`                                | 列表 | TLS 拦截包含域名                                         |
| tls     | `tls.app-exclude`                            | 列表 | TLS 拦截排除应用                                         |
| tls     | `tls.app-include`                            | 列表 | TLS 拦截包含应用                                         |
| traffic | `traffic.max-records`                        | 数值 | 最大记录数                                              |
| traffic | `traffic.max-db-size`                        | 大小 | 最大数据库大小                                            |
| traffic | `traffic.max-body-size`                      | 大小 | 最大 body 大小                                         |
| traffic | `traffic.max-buffer-size`                    | 大小 | 最大缓冲区大小                                            |
| traffic | `traffic.retention-days`                     | 数值 | 记录保留天数                                             |
| traffic | `traffic.sse-stream-flush-bytes`             | 大小 | SSE 流刷新字节数                                         |
| traffic | `traffic.sse-stream-flush-interval-ms`       | 数值 | SSE 流刷新间隔（毫秒）                                      |
| traffic | `traffic.ws-payload-flush-bytes`             | 大小 | WebSocket 载荷刷新字节数                                  |
| traffic | `traffic.ws-payload-flush-interval-ms`       | 数值 | WebSocket 载荷刷新间隔（毫秒）                               |
| traffic | `traffic.ws-payload-max-open-files`          | 数值 | WebSocket 载荷最大打开文件数                                |
| access  | `access.mode`                                | 枚举 | 访问模式（local\_only/whitelist/interactive/allow\_all） |
| access  | `access.allow-lan`                           | 布尔 | 允许局域网访问                                            |
| access  | `access.userpass.enabled`                    | 布尔 | 代理认证开关                                             |
| access  | `access.userpass.accounts`                   | 列表 | 代理认证账户列表                                           |
| access  | `access.userpass.loopback-requires-auth`     | 布尔 | 回环地址是否需要认证                                         |

大小类型支持单位：`B`、`KB`、`MB`、`GB`（如 `10MB`、`512KB`）。

### 12. 流量查询

```bash
bifrost traffic list --limit 20
bifrost traffic list --host example.com --method POST --format json-pretty
bifrost traffic get 57544 --request-body --response-body
bifrost traffic search openai --domain api.openai.com --method POST
```

> 当用户提及一个少于 6 位的数字 ID 并希望查看详情时，直接执行 `bifrost traffic get <ID>`。

`traffic list` 完整过滤参数：

```
-l, --limit <N>               最大返回数（默认 50）
    --cursor <SEQ>            分页游标（来自 next_cursor/prev_cursor）
    --direction <DIR>         分页方向：backward（默认）或 forward
    --method <METHOD>         HTTP 方法过滤
    --status <CODE>           精确状态码过滤
    --status-min <CODE>       状态码下限
    --status-max <CODE>       状态码上限
    --protocol <PROTO>        协议过滤（http/https/ws/wss/h3）
    --host <TEXT>             Host 包含过滤
    --url <TEXT>              URL 包含过滤
    --path <TEXT>             Path 包含过滤
    --content-type <TYPE>     Content-Type 过滤
    --client-ip <IP>          客户端 IP 过滤
    --client-app <APP>        客户端应用过滤
    --has-rule-hit <BOOL>     是否命中规则
    --is-websocket <BOOL>     仅 WebSocket
    --is-sse <BOOL>           仅 SSE
    --is-tunnel <BOOL>        仅隧道
-f, --format <FMT>            输出格式：table|compact|json|json-pretty
    --no-color                禁用彩色输出
```

### 13. 全文搜索

```bash
bifrost search openai --domain api.openai.com --method POST
bifrost search '{"error"' --res-body --content-type json
bifrost search --interactive                    # 交互式 TUI 模式
```

`search` 完整参数：

```
[keyword]                     搜索关键词（URL/headers/body 全文搜索）
-i, --interactive             交互式 TUI 模式（无关键词时默认进入）
-l, --limit <N>               最大结果数（默认 50）
-f, --format <FMT>            输出格式：table|compact|json|json-pretty
    --url                     仅搜索 URL/path
    --headers                 仅搜索 headers（请求+响应）
    --body                    仅搜索 body（请求+响应）
    --req-header              仅搜索请求 headers
    --res-header              仅搜索响应 headers
    --req-body                仅搜索请求 body
    --res-body                仅搜索响应 body
    --status <FILTER>         状态过滤：2xx|3xx|4xx|5xx|error
    --method <METHOD>         HTTP 方法过滤
    --host <TEXT>             Host 包含过滤
    --path <TEXT>             Path 包含过滤
    --protocol <PROTO>        协议过滤：HTTP|HTTPS|WS|WSS
    --content-type <TYPE>     Content-Type 过滤（json/xml/html/form 等）
    --domain <PATTERN>        域名 pattern 过滤
    --max-scan <N>            最大扫描记录数（默认 10000，增大可扩大搜索范围）
    --max-results <N>         最大返回匹配结果数（默认 100）
    --no-color                禁用彩色输出
```

### 14. 升级

```bash
bifrost upgrade
bifrost upgrade -y            # 跳过确认
bifrost version-check         # 仅检查新版本，不升级
```

### 15. 导入 / 导出

```bash
bifrost import ./backup.bifrost                # 从 .bifrost 文件导入（规则、脚本、变量）
bifrost import --detect-only ./backup.bifrost  # 仅检测文件类型不导入

bifrost export rules -o ./rules.bifrost        # 导出规则
bifrost export values -o ./values.bifrost      # 导出变量
bifrost export scripts -o ./scripts.bifrost    # 导出脚本
```

### 16. 远程同步

```bash
bifrost sync status                            # 查看同步状态
bifrost sync login                             # 登录同步服务
bifrost sync logout                            # 登出同步服务
bifrost sync run                               # 手动触发同步
bifrost sync config                            # 查看/更新同步配置
```

### 17. 管理端远程访问 (Admin)

用于启用/禁用管理端（Web UI）的远程访问权限，并管理认证密码和审计日志。

```bash
# 远程访问状态与开关
bifrost admin remote status                    # 查看当前远程访问状态
bifrost admin remote enable                    # 开启管理端远程访问
bifrost admin remote disable                   # 关闭管理端远程访问

# 认证管理
bifrost admin passwd                           # 修改 admin 账户密码（交互式）
bifrost admin passwd --username admin
printf '%s\n' 'new_password' | bifrost admin passwd --password-stdin
bifrost admin revoke-all                       # 吊销所有现有的管理端登录会话（JWT）

# 审计日志
bifrost admin audit                            # 查看管理端登录审计日志
bifrost admin audit --limit 100 --offset 0
bifrost admin audit --limit 100 --json         # 以 JSON 格式输出最近 100 条审计记录
```

- `admin remote enable/disable` 修改远程访问开关（管理端会在请求时读取该值）
- `admin passwd` 会更新本地认证凭据
- `admin revoke-all` 会立即让所有已登录的管理端会话失效

### 18. 流量清理

```bash
bifrost traffic clear                          # 清除流量记录
bifrost traffic clear --ids 1,2,3 -y           # 按 ID 清除，并跳过确认
```

### 19. 实时指标

```bash
bifrost metrics summary                        # 查看指标摘要（默认）
bifrost metrics apps                           # 按应用查看流量指标
bifrost metrics hosts                          # 按域名查看流量指标
bifrost metrics history                        # 查看指标历史
```

### 20. Shell 补全

```bash
bifrost completions bash                       # 生成 bash 补全脚本
bifrost completions zsh                        # 生成 zsh 补全脚本
bifrost completions fish                       # 生成 fish 补全脚本
```

### 21. 安装 Skill 到 AI 工具

bifrost 支持将自身的 `SKILL.md` 文档安装到各种 AI 编码辅助工具中（如 Claude Code、Codex、Trae、Cursor、GitHub Copilot 等），也兼容更多遵循通用 Agent Skills 目录规范的运行时。

```bash
bifrost install-skill --cwd                    # 安装到当前项目目录（如 .claude/.codex/.agents/.github/.trae/.cursor）
bifrost install-skill -t trae                  # 仅安装到 Trae
bifrost install-skill -t github-copilot        # 仅安装到 GitHub Copilot
bifrost install-skill -t universal             # 仅安装到通用 .agents/skills 目录
bifrost install-skill -t all -y                # 自动安装到所有支持的工具
```

### 22. 远程调用 (Remote)

通过云端 Relay 中转，远程查询目标 Bifrost 客户端实例的状态和流量信息。适用于目标 Bifrost 实例不在本机的场景。

#### 前置条件

- bifrost remote 依赖 Relay 中转服务，Bifrost 客户端需已连接到 Relay 并处于在线状态
- 首次调用需走配对授权流程：
  1. 目标 Bifrost 客户端在 WebUI Settings -> Remote Invoke 中开启"发现模式"
  2. 获取 6 位一次性授权码
  3. 调用方使用授权码完成配对
  4. 目标客户端用户在 WebUI 中人工批准
- 已授权的调用方可在授权有效期内直接复用，无需重新配对

#### 命令

```bash
bifrost remote status                          # 查看远端状态
bifrost remote search <query>                  # 远程搜索流量（支持中文等 Unicode，禁止 ASCII 控制字符，最大 500 字符）
bifrost remote traffic search <query>          # 等价于 bifrost remote search
bifrost remote traffic list [OPTIONS]          # 远程流量列表
bifrost remote traffic get <id> [OPTIONS]      # 远程获取流量详情
```

`remote traffic list` 支持的过滤参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `--limit` | number | 每页记录数，默认 50，上限 100 |
| `--cursor` | number | 分页游标 |
| `--direction` | string | 翻页方向：backward/forward |
| `--method` | string | HTTP 方法过滤 |
| `--status` | number | 精确状态码 |
| `--status-min` | number | 状态码下限 |
| `--status-max` | number | 状态码上限 |
| `--protocol` | string | 协议：http/https/ws/wss/h3 |
| `--host` | string | 域名包含匹配 |
| `--url` | string | URL 包含匹配 |
| `--path` | string | 路径包含匹配 |
| `--content-type` | string | Content-Type 过滤 |
| `--client-ip` | string | 客户端 IP |
| `--client-app` | string | 客户端应用 |
| `--has-rule-hit` | bool | 是否命中规则 |
| `--is-websocket` | bool | 仅 WebSocket |
| `--is-sse` | bool | 仅 SSE |
| `--is-tunnel` | bool | 仅隧道 |

`remote traffic get` 参数：
- `<id>`: 流量记录 ID 或 sequence 编号（纯数字）
- `--request-body`: 包含请求体
- `--response-body`: 包含响应体

> 当用户提及一个少于 6 位的数字 ID 并希望远程查看详情时，直接执行 `bifrost remote traffic get <ID> --request-body --response-body`。

#### 命令协议说明

`bifrost remote` 采用"受控查询命令白名单"模型，不支持任意 shell 透传。

支持的命令白名单：
- `status` — 查询客户端运行状态
- `search.get` / `traffic.search` — 关键词搜索流量
- `traffic.list` — 分页查询流量列表
- `traffic.get` — 获取单条流量详情

不支持的操作（会返回 `unsupported_command` 错误）：
- 任何配置修改操作、规则新增/编辑/删除
- `traffic.clear`（写操作）
- values/config/cert/系统代理管理
- 任意文件访问或脚本执行

#### 授权模式

| 模式 | 说明 |
|------|------|
| 仅本次 | 单次调用后授权自动失效 |
| 30 分钟 | 30 分钟内可复用 |
| 1 小时 | 1 小时内可复用 |
| 1 天 | 24 小时内可复用 |
| 永久 | 永久有效，需要二次确认 |

#### 安全说明

- 所有命令内容通过端到端加密传输（X25519 + ChaCha20-Poly1305），Relay 无法查看明文
- 每次调用使用独立的临时密钥对，调用结束后立即销毁
- Relay 仅存储命令摘要和审计信息，不存储原始命令或结果明文
- 授权绑定调用方设备指纹（`caller_fingerprint`），token 被窃取也无法冒充

#### 远程调用常见工作流

```bash
# 远程排查某个域名的请求
bifrost remote traffic list --host example.com --limit 20
bifrost remote traffic get <id> --request-body --response-body

# 远程搜索关键词
bifrost remote search "error"
bifrost remote search "api/v1/users"

# 查看远端状态
bifrost remote status
```

#### 远程调用 Agent 行为建议

- 优先检查是否有可复用授权，避免每次都走配对流程
- 远程命令仅支持只读查询，不要尝试远程修改配置或规则
- 当用户需要修改远端 Bifrost 配置时，提示用户直接在目标机器上操作
- 如果远程命令返回 `unsupported_command`，说明该操作不在白名单内
- 如果返回授权相关错误，引导用户在目标 Bifrost WebUI 中进行授权
- 遇到网络超时或 Relay 不可达，提示检查 Relay 服务状态和网络连接

## 推荐工作流

### 调试 HTTPS 明文请求

```bash
bifrost ca generate
bifrost ca install
# 推荐：仅对目标域名启用 TLS 解包
bifrost start -p 9900 --intercept-include '*.target.local'
```

若需要对特定应用启用：

```bash
bifrost start -p 9900 --app-intercept-include '*Chrome'
```

仅在确实需要全局解包时才用 `--intercept`（不推荐）。

### 脚本开发调试

```bash
# 添加请求修改脚本
bifrost script add request add-header -f ./scripts/add-header.js

# 测试脚本（使用内置 mock 数据）
bifrost script run add-header

# 查看脚本内容
bifrost script show add-header

# 更新脚本
bifrost script update request add-header -f ./scripts/add-header-v2.js
```

### 排查某个域名请求

```bash
bifrost search example --domain example.com --format json-pretty
bifrost traffic list --host example.com --limit 20
bifrost traffic get <id> --request-body --response-body  # <id> 为少于 6 位的数字序号
```

### 创建规则工作流

#### 第一步：阅读必要文档

| 优先级 | 文档 | 内容 |
| --- | --- | --- |
| **必读** | [docs/rule.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rule.md) | 规则整体语法（pattern + operation + filter + lineProps） |
| **必读** | [docs/pattern.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/pattern.md) | 匹配模式：Domain / IP / Wildcard / PathWildcard / Regex、否定匹配 |
| **必读** | [docs/operation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/operation.md) | 操作指令语法、Value 类型、模板变量、协议列表 |
| 按需 | [docs/rules/routing.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/routing.md) | host / xhost / proxy / pac 等路由转发 |
| 按需 | [docs/rules/request-modification.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/request-modification.md) | reqHeaders / reqBody / reqCookies / method / ua 等 |
| 按需 | [docs/rules/response-modification.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/response-modification.md) | resHeaders / resBody / resCookies / statusCode / cache 等 |
| 按需 | [docs/rules/body-manipulation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/body-manipulation.md) | reqReplace / resReplace / resMerge 等 Body 操作 |
| 按需 | [docs/rules/url-manipulation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/url-manipulation.md) | urlParams / pathReplace 等 URL 操作 |
| 按需 | [docs/rules/status-redirect.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/status-redirect.md) | statusCode / redirect |
| 按需 | [docs/rules/filters.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/filters.md) | includeFilter / excludeFilter |
| 按需 | [docs/rules/scripts.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/scripts.md) | reqScript / resScript / decode |
| 按需 | [docs/values.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/values.md) | Values 变量管理 |
| 按需 | [docs/scripts.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/scripts.md) | 脚本开发完整指南 |

#### 第二步：添加规则

```bash
# 内联规则
bifrost rule add my-rule -c "example.com host://127.0.0.1:3000"
bifrost rule add my-rule -c "example.com host://127.0.0.1:3000 reqHeaders://X-Debug=1 resCors://*"

# 从文件添加（适合多条/复杂规则）
bifrost rule add my-rule -f ./rules/my-rule.txt
bifrost rule enable my-rule

# 使用 Values 引用
bifrost value add mock-response '{"code":0,"data":{"name":"test"}}'
bifrost rule add api-mock -c "api.example.com/user resBody://{mock-response}"

# 临时规则（不持久化，适合一次性调试）
bifrost start -p 9900 --rules "example.com host://127.0.0.1:3000"
```

#### 第三步：验证规则

```bash
bifrost rule show my-rule                                          # 确认规则内容
curl -x http://127.0.0.1:9900 http://example.com/api/test         # 发送测试请求
bifrost traffic list --host example.com --has-rule-hit true --limit 5  # 确认规则命中
bifrost traffic get <id> --request-body --response-body            # 检查请求/响应详情
```

### 修改规则工作流

```bash
# 查看现有规则
bifrost rule list # 私有规则，非小组规则
bifrost rule show <rule-name>

# 更新规则内容
bifrost rule update my-rule -c "example.com host://127.0.0.1:4000 reqHeaders://X-Version=2"
bifrost rule update my-rule -f ./rules/my-rule-v2.txt

# 启用 / 禁用 / 删除
bifrost rule disable my-rule
bifrost rule enable my-rule
bifrost rule delete my-rule

# 重命名 / 调整优先级
bifrost rule rename old-name new-name
bifrost rule reorder

# 验证（同创建规则第三步）
bifrost rule show my-rule
curl -x http://127.0.0.1:9900 http://example.com/api/test
bifrost traffic list --host example.com --has-rule-hit true --limit 5
```

## 特别说明

本文档仅列出常用命令和参数摘要。**CLI 的完整参数、用法说明和示例均内置于** **`--help`** **输出中**，包括协议列表、规则语法快速参考、变量展开说明等。遇到本文档未覆盖的参数或用法时，**必须**先执行以下命令获取权威信息：

```bash
bifrost -h                    # 完整帮助（含协议、规则语法、环境变量等）
bifrost <command> -h          # 子命令帮助（如 bifrost start -h、bifrost script -h）
bifrost <command> <action> -h # 子动作帮助（如 bifrost rule add -h、bifrost config set -h）
```

`-h` 输出的信息始终与当前安装版本一致，是最准确的参数参考。本文档可能因版本迭代而滞后，**以** **`--help`** **输出为准**。

## Agent 行为建议

- 优先通过 CLI 完成任务，不要直接手改底层数据文件
- 如果用户没有要求修改系统环境，不要开启 `--system-proxy`、`--cli-proxy`
- **TLS 拦截默认关闭**，不要主动全局开启 `--intercept`（详见 §3 TLS/CA）
- 如果用户只想验证规则，不必启用 TLS 拦截
- 当用户提供一个少于 6 位的纯数字（如 57544、12345），且上下文含有「详情」「内容」「请求」「查看」等关键词时，应识别为 `bifrost traffic get <ID> --request-body --response-body` 操作
- 遇到不确定的参数或用法，**先执行** **`bifrost <command> -h`** **获取完整手册**，不要猜测
