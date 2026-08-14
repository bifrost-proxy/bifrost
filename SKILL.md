---
name: "bifrost"
description: "使用 bifrost CLI 管理代理生命周期、规则、证书、脚本、系统代理、运行时配置、流量查询与远程调用；也用于通过 Bifrost IM Gateway 配置飞书/Lark、微信/Weixin 机器人，并向机器人 owner、已配置 target 或指定飞书群聊/用户发送文本、Markdown、图片、文件附件和飞书原生卡片。当用户提到启动/停止/检查 bifrost、TLS 拦截、规则或脚本、流量搜索（包括用 Referer/Origin 等请求头关联某站点页面发起的第三方请求）或少于 6 位的请求 ID、JWT/Cookie 诊断、重放/导出/等待请求、远程 shell/授权/文件操作，或提到‘用 bifrost/机器人发飞书或微信消息’‘给 owner 发消息’‘给指定群发 Markdown/图片/文件/卡片’‘配置 IM provider/target’时触发。"
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

该脚本默认会自动安装并信任 CA、安装所有 Bifrost skills，并启动后台服务。

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
git clone https://github.com/bifrost-proxy/bifrost.git bifrost
cd bifrost
cd web && pnpm install && pnpm build && cd ..
cargo build --release -p bifrost
./target/release/bifrost --version
```

## 3. 如果 bifrost 已存在，自动升级到最新版本

只要用户没有明确要求"固定当前版本"或"禁止升级"，都应在首次检查通过后继续执行：

```bash
bifrost upgrade
bifrost --version
bifrost install-skill -y 
```

- `bifrost upgrade` 默认无交互执行升级；如果检测到正在运行的代理，升级成功后会自动重启代理以加载新二进制
- 若升级失败，再回退到官方安装脚本重新安装最新版本
- 如果用户只允许最小改动，至少要告知"当前跳过升级，后续行为基于现有版本"

## 4. 完成自检后再进入正式任务

1. 先确认目标是"运行代理"还是"管理已有代理"。
2. 先用 `bifrost --help` 或 `bifrost <command> --help` 补充具体参数，再执行高影响命令。
3. 会改本机网络环境的命令必须谨慎：`system-proxy enable/disable`、`cli-proxy enable/disable`、`start --system-proxy`、`start --cli-proxy`。

## 关键约束

- `bifrost` 不带子命令时，等价于 `bifrost start`
- `config`、`traffic`、`search`、`group`、多数 `status` 相关能力依赖"已有运行中的代理"
- `rule`、`value`、`script`、`ca` 主要操作本地数据目录，不一定要求代理正在运行
- `im` 通过当前运行中的 Bifrost Admin API 管理 IM Gateway provider、target、route、schedule 和消息；配置飞书/微信通道时优先走 CLI 交互式授权/扫码流程，不要直接改 provider 配置文件
- `system-proxy` 会修改操作系统代理设置；除非用户明确要求，不要主动启用
- `cli-proxy enable/disable` 会写入或移除当前 shell profile 中的代理和 CA 环境块；执行前先确认用户希望影响的 shell，不要代替用户无授权修改 profile

## 命令能力映射

### 1. 生命周期

```bash
bifrost status                     # 启动前必须先检查是否已有服务在运行
bifrost start -p 9900              # 仅当 status 显示无运行中的服务时才启动
bifrost start -p 9900 --daemon
bifrost status --tui
bifrost stop
```

- **启动前必须先执行** **`bifrost status`** **检查**，如果已有服务在运行，直接复用，不要尝试启动新的
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
      --intercept-include <DOMAINS>   强制拦截域名（低于规则，高于应用/IP/全局）
      --app-intercept-exclude <APPS>  排除应用不拦截（逗号分隔，支持通配符）
      --app-intercept-include <APPS>  强制拦截应用（低于规则和域名，高于 IP/全局）
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
2. 域名：`--intercept-exclude`（Passthrough）优先于 `--intercept-include`（Force Intercept）
3. 应用：`--app-intercept-exclude`（Passthrough）优先于 `--app-intercept-include`（Force Intercept）
4. 客户端 IP：Passthrough 优先于 Force Intercept
5. `--intercept` / `--no-intercept`：全局开关（**默认关闭，不推荐全局开启**）

### 3. TLS / CA

```bash
bifrost ca generate
bifrost ca generate -f          # 强制重新生成
bifrost ca install
bifrost ca info
bifrost ca export -o ./bifrost-ca.pem
```

**⚠️ TLS 拦截默认关闭，不建议全局开启** **`--intercept`。推荐使用域名/应用白名单按需解包：**

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

- 总体顺序固定为 `Rules > Domain > App > Client IP > Global`；同一层内 Passthrough 优先于 Force Intercept
- 因此域名命中 `--intercept-exclude` 后，即使命中浏览器的 `--app-intercept-include`，也保持 CONNECT 隧道，不进行 TLS 解包
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

### 5A. 多端口临时规则绑定

当用户想“保留主代理不动，再开几个调试端口分别命中不同规则”时，优先使用 `bifrost port` 命令族，而不是反复启停主代理或切换默认 enabled 规则。

```bash
# 主端口继续跑默认规则
bifrost start -p 8811 --no-system-proxy

# 临时端口 18888 只绑定一个本地规则
bifrost port bind --port 18888 --rule local-dev

# 临时端口 18889 绑定 Group 规则
bifrost port bind --port 18889 --group-rule 7152084678483132446/abc

# 自动分配端口并直接绑定规则文件
bifrost port bind --port 0 --rule-file ./temp-debug.bifrost

# 直接用一条 inline 规则开一个独立调试端口
bifrost port bind --port 18890 --rule-text "debug.test status://218 resBody://(debug)"

# 查看所有临时端口与单个端口详情
bifrost port list
bifrost port show 18888
bifrost port active 18888

# 把 18888 切换成另一整组规则
bifrost port update 18888 --rule login-mock --rule trace-api

# 调试结束后回收临时端口
bifrost port destroy 18888

# 用入口代理端口筛选流量，区分主端口和临时端口来源
bifrost traffic list --listener-port 18888
bifrost traffic search "debug" --proxy-port 18888
```

核心原则：

- `port` 命令要求主代理已经在运行。
- 主端口继续使用默认启用规则视图；临时端口只使用 `port bind` / `port update` 里显式绑定的规则引用。
- 所有端口共享同一个 `BIFROST_DATA_DIR`，因此 values、scripts、证书、流量数据库都是共用的。
- 临时端口绑定状态只在当前运行进程的内存里生效；Bifrost 重启后临时端口会被重置，不会自动恢复监听或绑定状态。
- 临时端口流量不受默认规则 enabled/disabled 状态影响；即使默认规则启用，也不会混入临时端口的规则视图。
- 销毁临时端口不会删除共享规则数据，也不会影响主端口监听。
- Traffic 会为来自主端口和临时端口的所有请求记录入口端口，即使没有命中任何规则也会记录。

四种规则来源的选择建议：

- `--rule`：复用已存在的本地规则名，适合长期维护的规则。
- `--group-rule`：复用已存在的 Group 规则，适合团队共享规则。
- `--rule-file`：直接绑定一个规则文件，适合本地临时排障，不污染共享 `rules/` 目录。
- `--rule-text`：直接写一条规则原文，适合快速构造一次性调试端口。

推荐工作流：

1. 先用主端口承载常规默认规则。
2. 按场景开多个临时端口，例如：
   - 18888 专门命中本地 mock 规则
   - 18889 专门命中某个 Group 规则
   - 自动分配端口用于一次性文件规则调试
3. 用 `port active <port>` 验证每个端口当前真正生效的规则集合。
4. 结合 `traffic list` / `traffic search` / `traffic get` 查看或筛选入口端口：
   - 表格列：`PORT`
   - compact JSON 字段：`lp`
   - 详情 JSON 字段：`listener_port`
   - 过滤参数：`--listener-port <PORT>`，别名 `--proxy-port <PORT>`
5. 调试结束后逐个 `port destroy` 回收。

当用户问“如何灵活绑定不同端口和规则”时，应该明确区分：

- 主端口：默认工作流、长期启用规则。
- 临时端口：隔离实验、单任务排障、局部 mock、对比不同规则集合。

如果用户只是想“切一下默认规则”，优先继续用 `rule enable/disable/reorder`；如果用户明确需要“同一时间并存多套规则入口”，则用 `port bind`。

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

### 10.1 CLI 代理与 CA 环境

```bash
bifrost cli-proxy enable
bifrost cli-proxy enable --shell zsh  # 可选：bash、zsh、fish、powershell
bifrost cli-proxy enable --host 127.0.0.1 --port 9900 --no-proxy 'localhost,127.0.0.1,::1,*.local'
bifrost cli-proxy enable --ca-file /path/to/custom.pem --ca-dir /path/to/certs
bifrost cli-proxy disable
bifrost cli-proxy disable --shell zsh
```

- 未指定 `--shell` 时优先识别当前 Bash、Zsh、Fish 或 PowerShell；识别失败时显式传 `--shell`
- Bash 复用已存在的首个登录 profile（`.bash_profile` → `.bash_login` → `.profile`），不得新建更高优先级文件遮蔽用户现有配置
- Enable 会写入大小写代理变量和 Node/Python/Go/cURL/Git/Cargo/Deno/AWS/gRPC 等 CA 变量；覆盖型 CA 变量指向系统 root + Bifrost CA 合并 bundle，不关闭 TLS 校验
- 成功后提醒用户新开 shell 或 reload CLI 列出的 profile；Disable 不能修改已启动的当前父 shell 环境
- 自动操作失败时，把 CLI 输出的完整复制块、profile 路径或 marker 删除指引原样交给用户，不要自行缩减环境变量集
- Bifrost 正常退出、崩溃后会自动清理所有受管 shell profile；`restart` 保留配置给新进程接管

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
bifrost traffic list --listener-port 18888 --format json
bifrost traffic get 57544 --request-body --response-body
bifrost traffic get --ids 57544,57545,57546 --max-body 32768 --format ndjson   # 批量获取（一次往返，最多 200 条）
bifrost traffic search openai --domain api.openai.com --method POST
bifrost traffic search openai --proxy-port 18888
bifrost traffic auth-status 57544                                              # JWT/Cookie 诊断（valid/expired/sub/exp/cookie 名）
bifrost traffic export 57544 --as curl                                         # 导出 curl 模板（原始捕获值）
bifrost traffic export 57544 --as fetch -o ./repro.js                          # 导出 fetch / HAR
bifrost traffic replay 57544 --patch /body/messages/0/content="hello"          # 重放并应用 JSON Patch
```

> 当用户提及一个少于 6 位的数字 ID 并希望查看详情时，直接执行 `bifrost traffic get <ID>`。
>
> **批量场景**：要在一次往返里取多条用 `--ids id1,id2,id3`（最多 200，默认 ndjson 输出，省 N-1 次 round-trip）。
>
> **敏感输出**：本期不做 Authorization、Cookie、JWT token 等敏感信息脱敏。`traffic get` / `traffic export` / `search --include` 按捕获原文输出；完整脱敏方案另开需求处理，当前不要把这些输出写入低信任日志、聊天或可复用 skill。

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
    --listener-port <PORT>    入口代理监听端口过滤（别名：--proxy-port）
    --has-rule-hit <BOOL>     是否命中规则
    --is-websocket <BOOL>     仅 WebSocket
    --is-sse <BOOL>           仅 SSE
    --is-tunnel <BOOL>        仅隧道
-f, --format <FMT>            输出格式：table|compact|json|json-pretty
    --no-color                禁用彩色输出
```

`traffic get` 批量参数：

```
[ID]                          单条 ID（与 --ids 互斥；省略时进入交互式选择）
    --ids ID,ID,...           批量获取，单次往返，最多 200 条
    --request-body            包含请求 body（best effort）
    --response-body           包含响应 body（best effort）
    --max-body <BYTES>        批量模式每条 body 上限（默认 65536）
-f, --format <FMT>            输出格式：table|compact|json|json-pretty|ndjson（--ids 默认 ndjson）
```

`traffic auth-status` / `traffic export` / `traffic replay`（新）：

```
# auth-status：诊断单条请求的 JWT/Cookie 状态
bifrost traffic auth-status <ID> [--format human|json]

# export：导出 curl / fetch / HAR 模板，便于在外部 shell / 浏览器 / Postman 复现
bifrost traffic export <ID> --as curl|fetch|har [-o <file>]

# replay：从 admin 端直接重放请求；可叠加 RFC6902 JSON Patch
bifrost traffic replay <ID> [--patch /a/b=value]... [--patch-json <RAW_OPS>] \
    [--refresh-auth] [--timeout 30s] [--format human|json|json-pretty]
```

- `--patch` 是 shorthand：`/headers/Authorization=Bearer xxx` 或 `/body/messages/0/content="hi"`；value 自动识别数字/字符串。
- `--patch-json` 是 RFC6902 原生数组：`'[{"op":"replace","path":"/headers/X-A","value":"v"}]'`。
- `--refresh-auth` 会从最近一次同 host 的成功请求里拉 `Authorization` / `Cookie` / `X-Tt-*` 注入，适合 token 过期场景。
- export / replay 使用捕获原文或重放结果原文；复制到外部前必须手动移除 Authorization、Cookie、JWT token 等敏感值。

### 13. 全文搜索

```bash
bifrost search openai --domain api.openai.com --method POST
bifrost search '{"error"' --res-body --content-type json
bifrost search --interactive                    # 交互式 TUI 模式
bifrost search --req-json '$.user.name=alice' --res-json '$.data.errno=0'   # JSONPath 等值过滤（可重复）
bifrost search --req-header-eq 'content-type=application/json' --res-header-eq 'x-tt-logid=*'  # header 等值过滤
bifrost search foo --since 30m --until now                                          # 时间窗（绝对 RFC3339 或相对 30s/5m/2h/1d）
bifrost search foo --latest 10m                                                     # --latest 10m == --since now-10m
bifrost search foo --include bodies,headers --max-body 32768                        # 在结果里直接带上 body/headers
bifrost search foo --include req-body,res-headers                                   # 也可单独挑：req-body|res-body|req-headers|res-headers
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
    --listener-port <PORT>    入口代理监听端口过滤（别名：--proxy-port）
    --protocol <PROTO>        协议过滤：HTTP|HTTPS|WS|WSS
    --content-type <TYPE>     Content-Type 过滤（json/xml/html/form 等）
    --domain <PATTERN>        域名 pattern 过滤
    --max-scan <N>            最大扫描记录数（默认 10000，增大可扩大搜索范围）
    --max-results <N>         最大返回匹配结果数（默认 100）
    --req-json PATH=VALUE     JSONPath 等值过滤请求 body，如 $.user.name=alice（可重复，AND 关系）
    --res-json PATH=VALUE     JSONPath 等值过滤响应 body，如 $.data.errno=0（可重复）
    --req-header-eq NAME=VAL  请求 header 等值过滤，大小写不敏感（可重复）
    --res-header-eq NAME=VAL  响应 header 等值过滤（可重复）
    --since TIME              起始时间（RFC3339 或相对 30s/5m/2h/1d）
    --until TIME              结束时间（RFC3339 或相对）
    --latest DURATION         快捷写法，等价于 --since now-DURATION
    --include PARTS           随结果返回 request-body|response-body|request-headers|response-headers；别名 req-body/res-body/req-headers/res-headers；快捷 bodies / headers（逗号分隔）
    --max-body BYTES          配合 --include 限制每条 body 上限（默认 65536）
    --no-color                禁用彩色输出
```

> JSONPath 子集支持 `$`、`.member`、`[N]`、`[*]`（不依赖外部 crate）。`--include` / `--max-body` 与 `bifrost traffic get` 行为对齐：bodies 走每条记录 LRU cache，重复查询不重复读数据库。
>
> **时间预过滤**：`--since/--until/--latest` 在 SQL 层（searched_range 索引）就裁掉超窗记录，不会被 `--max-scan` 浪费扫描预算。

#### 按发起站点关联第三方流量（请求头溯源）

当用户要找的不是“发往 `example.com` 的请求”，而是“由 `example.com` 页面触发的请求，包括发往其他域名的第三方服务”时，不要把 `--host example.com` 或 `--domain example.com` 作为主搜索条件。它们过滤的是**请求目的地**，会漏掉分析、埋点、CDN、图片、鉴权等第三方 host。

推荐分两步查询：

```bash
# 第一步：在请求 header 中搜索发起站点域名，先找候选请求并按结果 host 判断去向
bifrost search 'example.com' --req-header --latest 30m --max-scan 20000 --format json

# 第二步：需要确认关联依据时，只筛出 Referer / Origin / Sec-Fetch-Site
# --include req-headers 会读取完整请求头，用 jq 避免把 Cookie/Authorization 打到终端
bifrost search 'example.com' --req-header --latest 30m --max-scan 20000 \
  --include req-headers --format json | jq '[.results[] | {
    seq, host, path,
    linkage_headers: [.headers.request[] | select(
      (.[0] | ascii_downcase) as $name
      | ["referer", "origin", "sec-fetch-site"] | index($name)
    )]
  } | select(.linkage_headers | length > 0)]'

# 已知 Origin 的完整值时可以叠加严格等值过滤，进一步收窄 XHR/fetch/CORS 请求
bifrost search 'example.com' --req-header \
  --req-header-eq 'origin=https://example.com' --latest 30m --format json
```

判断规则：

- `Referer`：通常是最直接的页面来源证据，常包含完整页面 URL 和 path；但会受 `Referrer-Policy`、隐私设置、跳转和客户端实现影响，可能只保留 origin 或完全缺失。
- `Origin`：对 XHR、fetch、CORS、WebSocket 等请求很有价值，通常只有 `scheme://host[:port]`；普通静态资源 GET 经常不带，因此不能只搜 `Origin`。
- `Sec-Fetch-Site`：只能说明 `same-origin` / `same-site` / `cross-site` 等相对关系，不能单独识别具体是哪个页面发起；应与 `Referer` 或 `Origin` 一起看。
- `--req-header` 会搜索所有请求头，不只 `Referer` / `Origin`。如果 Cookie、自定义 header 或其他字段也含目标域名，第一步会产生假候选；必须像第二步那样确认关联字段，空 `linkage_headers` 的结果不能据此认定为页面触发。
- 搜索结果中的 `host` 是请求实际发往的目的地。请求头搜索会同时命中目标站自身请求（例如 `Host`/`Referer` 中含目标域名）和第三方请求；应按结果 `host` 分组，重点检查不同于目标站的 host。
- `--req-header-eq NAME=VALUE` 是大小写不敏感的**严格等值**过滤，不是包含或 glob 匹配。`referer=*example.com*` 不会匹配带 path 的 Referer；Referer 不知道完整值时，应使用 `bifrost search 'example.com' --req-header` 做包含搜索。
- 浏览器后台任务、Service Worker、原生客户端或严格隐私策略可能不发送 `Referer` / `Origin`。请求头关联是高价值线索，不是完整因果链；必要时结合 `--latest` 缩小到用户刚执行操作的时间窗，并对比请求时间、目标 host、path 和客户端应用。
- HTTPS 请求只有在 Bifrost 已解包明文时才能搜索这些 header；如果记录只是 CONNECT 隧道，应按目标域名或浏览器应用小范围启用 TLS 拦截，不要为了关联流量全局开启 `--intercept`。
- `--include req-headers` 会读取完整请求头。优先像上例一样立即用 `jq` 只保留关联字段；不要把 Cookie、Authorization、JWT、账号标识或带敏感 query 的完整 Referer 写入聊天、日志或技能文档。

### 13.1 `bifrost capture wait`（新）

等待下一条匹配过滤条件的流量并立即打印，常用于「点一下网页 → 抓那次请求」的脚手架场景：

```bash
bifrost capture wait --host api.openai.com --method POST --timeout 60s
bifrost capture wait --path /v1/messages --open https://app.example.com/dashboard
bifrost capture wait --host bot.example.com --format json --timeout 2m
```

`capture wait` 参数：

```
--host TEXT                   host 包含过滤（大小写不敏感）
--method METHOD               HTTP method 精确过滤
--path TEXT                   path 包含过滤
--open URL                    在开始等待前用系统默认浏览器打开 URL（触发请求）
--timeout DURATION            超时（30s/2m/1h/500ms；上限 600s；默认 60s）
--format human|json           输出格式（默认 human）
```

退出码契约：匹配到记录退出码 0；超时退出码 124（无 stderr 噪声）；过滤条件非法或服务端不可用退出码 1。底层使用 admin 推送（subscribe_once），不轮询数据库。

### 13.2 `bifrost status --format json`（新）

```bash
bifrost status                              # 默认 text（与历史输出兼容）
bifrost status --format json                # 单行 JSON，适合脚本/CI
bifrost status --format json-pretty         # 缩进 JSON，便于人看
bifrost status --tui                        # 交互式 TUI dashboard
```

JSON 字段包含 `pid / version / proxy_port / admin_port / uptime_ms / system_proxy / cli_proxy / tls_intercept / rules / scripts / values / connections / traffic_count` 等，字段稳定后只增不删；脚本要做能力探测时请基于 key 存在性判断，不要按数组下标解析。

### 14. 升级

```bash
bifrost upgrade
bifrost version-check         # 仅检查新版本，不升级
```

GitHub 更新检查、升级下载与 `install-skill` 拉取 GitHub raw 在企业 MITM / Linux 沙箱 / CI 私有根证书环境中，优先信任系统 CA；系统 trust store 不可控时可设置 `BIFROST_GITHUB_CA_BUNDLE=/path/to/ca.pem` 或 `BIFROST_UPGRADE_CA_BUNDLE=/path/to/ca.pem`。Sync/登录/云端接口、AI/Agent provider、MCP OAuth、ASR/语音模型下载、脚本 `net.fetch`、规则远程 URL 等通用外部 HTTPS 链路可设置 `BIFROST_CA_BUNDLE=/path/to/ca.pem` 或 `BIFROST_CA_DIR=/path/to/certs`。同时兼容 `SSL_CERT_FILE`、`SSL_CERT_DIR`、`REQUESTS_CA_BUNDLE`、`CURL_CA_BUNDLE`、`NODE_EXTRA_CA_CERTS`、`GIT_SSL_CAINFO`、`AWS_CA_BUNDLE`、`PIP_CERT`、`NPM_CONFIG_CAFILE`、`GRPC_DEFAULT_SSL_ROOTS_FILE_PATH` 等常见 CA 来源。只有确认 CA 无法注入且处于受控环境时，才用 `BIFROST_UPGRADE_UNSAFE_SSL=1`、`BIFROST_GITHUB_UNSAFE_SSL=1` 或 `BIFROST_UNSAFE_SSL=1` 作为最终兜底；它们不等同于代理服务 `--unsafe-ssl`，`bifrost remote` 仍优先使用 `BIFROST_REMOTE_UNSAFE_SSL` 作为 scoped 兜底。

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

### 21. IM Gateway provider 与 Agent 通道

当用户要求配置飞书、Lark、微信、Weixin、IM Gateway、IM 通道、通过聊天工具运行 Agent、扫码添加通道、向 owner/群聊/用户发消息，或发送 Markdown、图片、文件、飞书卡片时，使用本地 `bifrost im` 命令。先确认 Bifrost 服务已启动；未启动时使用 `bifrost start -d`。

```bash
bifrost im provider list
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --runner "Claude Code"
bifrost im provider capabilities feishu-main --format json-pretty
bifrost im target add oncall --provider feishu-main --receive-id-type chat_id --receive-id oc_xxx
bifrost im send weixin-main --text "hello owner"
bifrost im send feishu-main --markdown-file ./report.md
bifrost im send feishu-main --target oncall --card-file ./card.json
bifrost im send feishu-main --chat-id oc_xxx --markdown "**发布完成**" --image ./chart.png --file ./report.pdf
bifrost im send --bot-id cli_xxx --chat-id oc_xxx --markdown-file ./report.md
bifrost im send --bot-name "Release Bot" --chat-id oc_xxx --text "发布完成"
bifrost im messages list --provider feishu-main --direction inbound
```

使用规则：

- Feishu / Weixin 首次配置优先交互式 setup：只传 provider ID、`--type` 和 `--runner`。Feishu 会打印授权 URL 和终端二维码；Weixin 会打印扫码二维码；CLI 会保持等待，直到用户授权/扫码后自动创建并连接 provider。
- Runner 不能静默默认。交互式终端未传 `--runner` 时允许让用户用上下键选择；stdin 非交互时必须显式传 `--runner`。Runner 不存在或未启用时，错误会列出当前可用 Runner 和默认内置 Runner 类型。
- 支持常见 Runner 别名：`codex`、`traex` / `trae`、`Claude Code`。如果用户给了具体 runner，先让 CLI 校验，不要自己猜配置文件。
- Feishu / Weixin / WeChat provider 的 `base_url` 由 provider 类型固定管理。不要建议传 `--base-url`；CLI 会拒绝该参数。
- 已有 App ID / Secret 时可以手动配置，但 secret 应使用 `env:NAME`，避免写入 shell history 或最终报告。
- `bifrost im send` 的第一个位置参数是 provider 名称；兼容形式是 `--provider <ID>`。群内 Agent 若只拿到飞书机器人 App ID/名称和群 ID，可省略 provider，使用 `--bot-id <APP_ID> --chat-id <chat_id>`，或名称唯一时使用 `--bot-name <精确名称> --chat-id <chat_id>`；机器人名称重名会明确失败，应改用 bot ID。直接 provider 选择与 bot 选择器互斥。未指定目的地时发送给 owner；`--target <别名>` 使用已配置 target；其他直达地址使用成对的 `--receive-id-type` 与 `--receive-id`。不要猜 provider、机器人、群 ID 或用户 ID。
- 内容参数可以重复并按命令行顺序发送：`--text`、`--markdown` / `--markdown-file`、`--image` / `--image-file` / `--image-key`、`--file` / `--file-key`、`--card-file` / `--card-json`，以及快速卡片参数 `--card-title`、`--card-text`、`--card-image-file` / `--card-image-key`、`--card-image-alt`。视频能力没有独立 `--video` 参数，应使用 `--file` 交给 provider 按媒体类型识别。图片和文件通过本地 Admin API 以二进制上传，不把内容写成 base64 JSON。
- 遇到参数、能力或发送错误时，依次执行 `bifrost im --help`、`bifrost im send --help`、`bifrost im provider capabilities <ID> --format json-pretty`、`bifrost im provider list`。CLI help 与 provider 运行时 capabilities 是权威依据，不要猜测或改走其他 IM connector。当前飞书原生支持文本、Markdown 卡片、图片、文件和原生卡片；当前微信原生支持文本、图片、文件和视频，Markdown 会降级为可读纯文本、飞书原生卡片不支持，而且机器人必须先收到该用户的一条消息以建立发送上下文；后续能力变化一律以运行时输出为准。
- IM 外发会影响真实人员和群聊。只有用户已明确要求发送且 provider 与目的地都能从上下文唯一确定时才执行；否则先确认。用户只要求设计、开发或测试时，只能使用本地假服务，不能向真实 IM 通道试发。
- 每次发送都检查 bundle 级 `status` 与逐项 receipt。`partial_success` 代表只有部分内容成功，必须向用户明确列出失败项和 provider 返回原因，不能笼统声称消息已发出。需要安全重试时传稳定的 `--idempotency-key`。
- IM 通道上线通知会自动附带 runner-aware 帮助。所有外部 Runner 支持 `/help`、`/status`、`/cwd`、`/runner`、`/q`、`/rq`、`/stop`；Codex / Traex / Claude Code 等 Runner 只在适配器支持时展示 `/models`、`/model`、`/efforts`、`/effort`。

### 22. 安装 Skill 到 AI 工具

bifrost 支持将自身的 `SKILL.md` 文档安装到各种 AI 编码辅助工具中（如 Claude Code、Codex、Trae、Cursor、GitHub Copilot 等），也兼容更多遵循通用 Agent Skills 目录规范的运行时。

```bash
bifrost install-skill --cwd                    # 安装到当前项目目录（如 .claude/.codex/.agents/.github/.trae/.cursor）
bifrost install-skill -t trae                  # 仅安装到 Trae
bifrost install-skill -t github-copilot        # 仅安装到 GitHub Copilot
bifrost install-skill -t universal             # 仅安装到通用 .agents/skills 目录
bifrost install-skill -t all -y                # 自动安装到所有支持的工具
```

通用 `bifrost` skill 覆盖本机代理、规则、流量、IM Gateway CLI 配置和 Agent 采证工作流；专用 `bifrost-remote` skill 覆盖连接另一台机器、远程查流量、远程文件、远程脚本和授权 shell 边界。

### 23. 远程调用 (Remote)

`bifrost install-skill` 会同时安装通用 `bifrost` skill 和专用 `bifrost-remote` skill。用户明确要连接另一台机器、使用 SSH key / pair code、远程查询流量或通过 `shell.exec` 操作目标设备时，应优先使用 `bifrost-remote` skill 中的完整流程。

本节只保留快速索引（完整流程、错误码、典型 workflow 见 `skill_remote.md`）：

```bash
bifrost remote conn up --ssh-key <path>         # 使用导出的 SSH key 建立长期授权
BIFROST_REMOTE_SSH_KEY="$(cat <path>)" bifrost remote conn up --ssh-key  # 固定 env
bifrost remote conn up <code>                   # 使用一次性配对码建立授权
bifrost remote conn status                      # 查看远端状态
bifrost remote conn down [--all|--grant-id <g>] # 回收 grant
bifrost remote traffic search <query> --listener-port 18888  # 按远端入口代理端口搜索流量
bifrost remote traffic list --proxy-port 18888               # 按远端入口代理端口列流量
bifrost remote traffic get    <id> [OPTIONS]    # 远程获取流量详情
bifrost remote file read-many --path a --path b --cwd <repo>  # 一次读多个文件
bifrost remote file scratch-dir --cwd <repo>      # 获取 policy 允许的临时目录
bifrost remote file outline src/lib.rs --cwd <repo>  # 获取符号地图
bifrost remote file upload ./large.bin assets/large.bin --create-parents --resume  # 分块上传大文件
bifrost remote file download assets/large.bin ./large.bin --resume                 # 分块下载大文件
bifrost remote run --script-file ./smoke.py --interpreter python3 --cwd <repo> -- --json  # 上传本地脚本到远端执行
bifrost remote exec --shell-text "pwd"          # 受 Shell Access policy 限制的 shell.exec
bifrost remote exec --detach -- cargo test       # 长时间静默任务先 detach
bifrost remote job watch <call_id> --output-file ./x.log  # 续接 detached job
```

Relay HTTPS 私有 CA / 企业 MITM 优先通过系统 trust store 或 `BIFROST_REMOTE_RELAY_CA_BUNDLE=/path/to/ca.pem` 解决；只有 CA 无法注入的受控环境，才用 `BIFROST_REMOTE_UNSAFE_SSL=1 bifrost remote ...` 作为最终兜底跳过 remote relay 证书信任校验。该环境变量只作用于 remote relay HTTP/SSE client，不等同于代理服务 `--unsafe-ssl`。

> **4-tier 命名（2026 Q2）**：旧的 `remote connect / disconnect / status（顶层）/ search（顶层）` 仍有过渡别名但运行时会打印 deprecation warning，下个 minor release 会移除；`remote command exec` 已硬切为 `remote exec`，没有别名。`remote file {mv, rm, search, apply-patch}` 也已硬切为 `{move, delete, find, patch}`。CI 有 `scripts/ci/check-remote-cli-legacy.sh` 守卫，引用旧名会让流水线红。

边界说明：

- 只读查询类操作需要目标设备先启动 Bifrost、在 Remote Invoke 页面启用 SSH key 或配对码授权，并由 caller 用 `bifrost remote conn status` 验证连接。
- 远程设备控制类操作还需要目标设备启用 Shell Access profile/policy，并在授权请求中选择 `selected` 或 `all` 访问模式。
- `remote traffic clear` 是写操作，不提供 remote CLI 子命令；如确需清理目标设备流量记录，应在明确 shell 授权后通过 `remote exec` 执行目标机本地命令或 API。
- `remote shell ...` 与 `remote grant ...` 是当前机器本地管理命令（已 deprecated，请改用 `bifrost setting shell` / `bifrost setting grant`）；caller 要管理目标设备时，应通过 `remote exec` 执行目标机命令。
- `shell.exec` 受目标终端 Shell Access policy 约束；当前不能承诺 OS 级 sandbox 隔离。
- rule/config/script/value/CA/系统代理等没有专门的 `bifrost remote <module>` 子命令时，不代表不能远程操作；应走已授权的 `remote exec`。
- 修改远端文件优先使用 `remote file read/edit/patch/write/move/delete`；临时脚本优先使用 `remote run`，不要用 `remote exec + heredoc/base64/echo` 拼接文件内容。
- 长时间静默的构建、测试、轮询命令优先 `remote exec --detach` 或 `remote run --detach`，再用 `remote job list/status/logs/watch` 续接，避免 300 秒无事件 idle timeout。

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

### 排查发往某个域名的请求

```bash
bifrost search example --domain example.com --format json-pretty
bifrost traffic list --host example.com --limit 20
bifrost traffic get <id> --request-body --response-body  # <id> 为少于 6 位的数字序号
```

### 排查某个站点页面触发的第三方请求

```bash
# --host/--domain 查的是请求目的地；页面来源关联要搜索请求 header
bifrost search 'example.com' --req-header --latest 30m --max-scan 20000 --format json

# Origin 完整值已知时可严格收窄；Referer 通常带 path，未知完整值时不要用 --req-header-eq
bifrost search 'example.com' --req-header \
  --req-header-eq 'origin=https://example.com' --latest 30m --format json
```

先按结果 `host` 找出不同于页面站点的第三方目的地，再按需用 `--include req-headers` 在本地核对 `Referer`、`Origin` 和 `Sec-Fetch-Site`；完整请求头含敏感信息，不要原样分享。

### 创建规则工作流

#### 第一步：阅读必要文档

| 优先级    | 文档                                                                                                                            | 内容                                                       |
| ------ | ----------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| **必读** | [docs/rule.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rule.md)                                               | 规则整体语法（pattern + operation + filter + lineProps）         |
| **必读** | [docs/pattern.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/pattern.md)                                         | 匹配模式：Domain / IP / Wildcard / PathWildcard / Regex、否定匹配  |
| **必读** | [docs/operation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/operation.md)                                     | 操作指令语法、Value 类型、模板变量、协议列表                                |
| 按需     | [docs/rules/routing.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/routing.md)                             | host / xhost / proxy / pac 等路由转发                         |
| 按需     | [docs/rules/request-modification.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/request-modification.md)   | reqHeaders / reqBody / reqCookies / method / ua 等        |
| 按需     | [docs/rules/response-modification.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/response-modification.md) | resHeaders / resBody / resCookies / statusCode / cache 等 |
| 按需     | [docs/rules/body-manipulation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/body-manipulation.md)         | reqReplace / resReplace / resMerge 等 Body 操作             |
| 按需     | [docs/rules/url-manipulation.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/url-manipulation.md)           | urlParams / pathReplace 等 URL 操作                         |
| 按需     | [docs/rules/status-redirect.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/status-redirect.md)             | statusCode / redirect                                    |
| 按需     | [docs/rules/filters.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/filters.md)                             | includeFilter / excludeFilter                            |
| 按需     | [docs/rules/scripts.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/scripts.md)                             | reqScript / resScript / decode                           |
| 按需     | [docs/values.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/values.md)                                           | Values 变量管理                                              |
| 按需     | [docs/scripts.md](https://github.com/bifrost-proxy/bifrost/blob/main/docs/scripts.md)                                         | 脚本开发完整指南                                                 |

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
- 如果用户没有要求修改系统或 shell 环境，不要开启 `--system-proxy`、`--cli-proxy`，也不要执行 `system-proxy enable`、`cli-proxy enable/disable`
- **TLS 拦截默认关闭**，不要主动全局开启 `--intercept`（详见 §3 TLS/CA）
- 如果用户只想验证规则，不必启用 TLS 拦截
- 当用户提供一个少于 6 位的纯数字（如 57544、12345），且上下文含有「详情」「内容」「请求」「查看」等关键词时，应识别为 `bifrost traffic get <ID> --request-body --response-body` 操作；如果用户给了 ≥2 个 ID 应直接走 `bifrost traffic get --ids ID1,ID2,...` 批量
- **敏感输出**：`traffic get` / `traffic export` / `search --include` 当前按捕获原文输出，不要把 Authorization/Cookie/JWT/邮箱/手机号写到日志、聊天或可复用 skill 里
- 想知道 token/JWT 是否过期、属于谁，优先用 `bifrost traffic auth-status <ID>`，不要让 user 自己 decode JWT
- 想在外部 shell / 浏览器复现请求，用 `bifrost traffic export <ID> --as curl|fetch|har`；想直接重放并修改 body/headers 用 `bifrost traffic replay <ID> --patch ...`，不要自己手动拼 curl
- 需要捕获一次后续请求（比如「等我点击那个按钮」），用 `bifrost capture wait` 而不是 `traffic list` 轮询
- 用户要找“某站点页面发起的第三方请求”时，先用站点域名配合 `--req-header` 搜索 `Referer` / `Origin` 线索，并用 `--latest` 限定操作时间窗；不要误用只过滤目的地的 `--host` / `--domain`，也不要把严格等值的 `--req-header-eq` 当成 glob 搜索
- 写脚本/CI 集成时 `bifrost status` 必须加 `--format json`；不要去 grep text 输出
- 遇到不确定的参数或用法，**先执行** **`bifrost <command> -h`** **获取完整手册**，不要猜测


## 9. 远程文件 API（coding agent 友好）

受 `FileAccessPolicy` 约束的远程文件访问能力，面向 Claude Code / Cursor / Codex 等 coding agent 打磨。默认策略：`roots=[cwd]`，`denies=[**/.git/**, **/target/**, **/*.key, **/*.pem]`，默认遵守最近 `.gitignore`（可用 `--no-ignore` 关闭）。

### 授权模型（grant scope）

文件能力使用独立 grant scope，**不**由 `remote_query` / `remote_shell_*` 自动授予：

| 能力         | 所需 scope           | 覆盖子命令                                  |
| ---------- | ------------------ | -------------------------------------- |
| 只读         | `remote_file_read` | `read` / `read-many` / `list` / `stat` / `glob` / `find` / `hash` / `outline` / `download` |
| 读写 / patch | `remote_file_write` | `write` / `edit` / `mkdir` / `move` / `delete` / `patch` / `upload` |

### 子命令一览

```bash
# 只读
bifrost remote file read   <path> [--max-bytes <N>] [--allow-binary] \
                                   [--offset <line>] [--limit <lines>]
bifrost remote file read-many --path <p1> --path <p2> ...
bifrost remote file scratch-dir [--cwd <repo>] [--name .bifrost-tmp]
bifrost remote file list   [path] [--depth <N>] [--no-ignore] \
                                   [--exclude <name>]...
bifrost remote file stat   <path>
bifrost remote file glob   '<glob>' [--max-matches <N>] [--no-ignore] \
                                     [--exclude <name>]...
bifrost remote file find   '<regex>' [--path <sub>] [--max-matches <N>] \
                                      [--max-scan <bytes>] \
                                      [-B <n>] [-A <n>] \
                                      [-i|--case-insensitive] \
                                      [--glob '<pat>'] [--no-ignore] \
                                      [--exclude <name>]...
bifrost remote file hash   <path> [--algo sha256]
bifrost remote file outline <path> [--max-symbols <N>] [--max-bytes <N>]
bifrost remote file download <remote> <local> [--chunk-size <bytes>] \
                                   [--overwrite] [--resume] [--no-progress]

# 读写
bifrost remote file write  <path> [--content-file/--from-local <local|->] \
                                   [--content-b64 <base64>] \
                                   [--base-sha256 <sha>] \
                                   [--allow-overwrite true|false] \
                                   [--create-parents]
bifrost remote file edit   <path> (--edits '<json>' | --from-local <local-json|->) [--base-sha256 <sha>]
bifrost remote file mkdir  <path> [--parents]
bifrost remote file move   <from> <to>
bifrost remote file delete <path> [--recursive]
bifrost remote file patch  (--patch-file/--from-local <local|->) | (--patch-b64 <base64>)
bifrost remote file upload <local> <remote> [--chunk-size <bytes>] \
                                   [--overwrite] [--create-parents] \
                                   [--resume] [--no-progress]
```

所有子命令共享 `--cwd <path>`（工作目录覆盖）、`--output human|json`、`--relay-url`、`--client-id`。

### coding-agent 关键语义

- **gitignore 感知**：`list` / `glob` / `search` 默认跳过 `.gitignore` 命中的路径；agent 需要扫隐藏/忽略文件时加 `--no-ignore`。
- **truncated 标志**：`list` / `glob` / `search` / `read` 超过上限时在 JSON 响应中返回 `"truncated": true`；agent 应据此分片重试或收窄范围。
- **完整文件 sha256**：`read` 在 `truncated=true` 时响应体额外带 `file_sha256`（整文件哈希，不是截断片段的），用于 agent 做 resume / 一致性校验。
- **分块大文件传输**：`upload` / `download` 走 `file.upload_*` / `file.download_*` 分块协议，支持 `--chunk-size`、`--resume`、逐块 sha256 和整文件 sha256 校验。上传需要 `remote_file_write`，下载只需要 `remote_file_read`。
- **Symlink 语义**：`stat` 和 `list` 对符号链接采用 **lstat** 语义，不跟随链接，额外返回 `symlink_target`；Windows 上自动去除 `\\?\` / `\\?\UNC\` NT verbatim 前缀，跨平台一致。
- **原子写 + 乐观锁**：`write` / `edit` 采用 tmp+rename 两阶段提交，失败自动快照回滚；传 `--base-sha256` 时若与当前文件不一致，返回错误码 `[file.sha_mismatch]`，agent 应重新 `read` 再重试。
- **EOL 保留**：`edit` 自动识别并保留原文件的 LF / CRLF 行尾风格。
- **`--from-local`**：caller 侧本地 payload 入口。`write --from-local ./file` 读取本地文件内容，`edit --from-local ./edits.json` 读取 edits JSON，`patch --from-local ./change.diff` 读取 unified diff；`mkdir/move/delete` 没有本地 payload，不使用该参数。
- **`--content-b64` / `--patch-b64`**：用于传输二进制或含特殊字符的文本，caller 在本地 base64，admin 侧解码后写入；比 `--content-file -` + stdin 管道更适合非交互 / Windows agent。
- **`--create-parents`**：`write` 自带 `mkdir -p` 语义，避免 agent 先 `mkdir` 再 `write` 的两次往返。
- **搜索过滤**：`find` 的 `-i` 等价于 `(?i)` 前缀；`--glob '*.rs'` 用于在同一次 regex 扫描里按文件名进一步收窄。

### 错误码契约

agent 应按以下错误码做分支逻辑：

| 错误码                          | 含义                              | 典型应对                                |
| ----------------------------- | ------------------------------- | ----------------------------------- |
| `file.out_of_scope`           | 路径落在 `roots` 之外                 | 不要自动改 `--cwd`；请用户确认                 |
| `file.permission_denied`      | 命中 denies 或 policy 不允许写         | 放弃写入；可降级为只读流程                       |
| `file.op_not_permitted`       | 当前 FileAccessPolicy 未开放对应 op     | 调整授权或 policy；`upload` 需 `upload` op，`download` 需 `download` op |
| `file.size_too_large`         | 文件超过策略 `max_transfer_bytes`       | 拆分文件或让用户明确提高策略上限                  |
| `file.precondition_failed`    | 目标存在且未 overwrite、offset/patch 不匹配 | 重新读取状态；需要覆盖时显式加 overwrite          |
| `file.binary_not_allowed`     | 二进制且未传 `--allow-binary`         | 加 `--allow-binary` 或改 `hash` + 下载分片 |
| `file.sha_mismatch`           | 乐观锁失败，文件已被他人修改                  | 重新 `read` + 重算 sha + 重试             |
| `file.not_found`              | 路径不存在                           | 视任务决定是否 `mkdir` / `write`            |
| `file.is_a_directory` / `file.not_a_directory` | 类型不匹配        | 改换子命令                               |

### 典型调用链

```bash
# 1. 侦察
bifrost remote file list src --depth 2 --output json
bifrost remote file glob 'src/**/*.rs' --max-matches 200 --output json

# 2. 精准定位
bifrost remote file find 'fn handle_file_\w+' --path src --glob '*.rs' \
  -B 2 -A 2 --output json

# 3. 读取 + 记住 sha
bifrost remote file read src/lib.rs --output json   # -> 拿到 sha256

# 4. 原子编辑（带乐观锁）
bifrost remote file edit src/lib.rs \
  --base-sha256 <sha> \
  --edits '[{"start_line":10,"end_line":12,"replacement":"// new\n"}]' \
  --output json

# 5. 大文件 / 二进制传输
bifrost remote file upload ./local.bin assets/local.bin \
  --create-parents --overwrite --resume
bifrost remote file download assets/local.bin ./roundtrip.bin \
  --overwrite --resume

# 5b. 短文件直写（base64，适合含换行/特殊字符内容）
bifrost remote file write docs/notes.md \
  --content-b64 "$(base64 < ./local-notes.md)" \
  --create-parents --output json

# 6. 多文件同步
bifrost remote file patch --patch-file ./refactor.diff --output json
```

所有子命令支持 `--output json` 做机器可读输出，统一用上述错误码前缀做 failure 分支。
