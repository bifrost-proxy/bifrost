---
name: "bifrost-remote"
description: "当用户需要让 agent 连接另一台电脑终端上的 bifrost、指导目标终端安装并启动 bifrost、优先使用 SSH key 建立长期远程连接，或在必要时使用配对码连接，并查询远端状态或流量时触发。"
---

# Bifrost Remote

该技能用于指导用户和 Agent 通过 `bifrost remote` 与目标终端上的 Bifrost 建立连接。它分成两部分：

- 用户侧：在目标电脑终端上安装并启动 `bifrost`，并在 Web UI 中准备远程授权
- Agent 侧：优先通过 SSH key 连到目标终端建立长期连接，必要时再通过配对码连接，并执行当前允许的只读远程命令

## 适用场景

当用户有以下需求时，应优先使用本技能：

- 让 Agent 连接另一台机器上的 `bifrost`
- 指导用户在目标终端安装 `bifrost`
- 启动目标终端上的 Bifrost 并开启 Remote Invoke
- 使用 SSH key 建立长期可复用连接
- 首次通过配对码建立连接
- 查询远端 Bifrost 状态、流量详情和搜索结果

以下场景不属于本技能范围：

- 直接在远端执行任意 shell / TTY
- 远端修改规则、脚本、证书、配置
- 远端上传/下载文件
- 绕过 Web UI 授权直接接管目标终端

## 一、目标终端如何安装 bifrost

优先在目标终端检查 `bifrost` 是否已存在：

```bash
command -v bifrost
bifrost --version
```

如果命令不存在，优先使用官方安装脚本：

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

安装完成后再次确认：

```bash
command -v bifrost
bifrost --version
```

## 二、目标终端如何启动 bifrost

启动前遵守以下约束：

- 使用默认数据目录启动，避免用户额外配置目录
- 默认启用系统代理，这样正式使用时才能直接抓取设备流量
- 不额外指定端口、数据目录等配置，保持最小启动参数
- 需要 Web UI 时，确保管理端页面可通过浏览器访问

推荐启动方式：

```bash
bifrost start
```

该命令默认以前台方式运行。启动成功后，继续执行 `bifrost status`、打开 Web UI、或执行后续 `bifrost remote ...` 命令时，请在另一个终端窗口中操作。

启动后先检查状态：

```bash
bifrost status
```

## 三、目标终端如何准备远程连接

在目标终端启动好 Bifrost 后，让用户打开本地 Web UI：

```text
http://127.0.0.1:9900/_bifrost/settings?tab=remote-invoke
```

然后优先选择 SSH key 方式；如暂时没有 SSH key，再使用配对码。

### 方式 A：SSH key（推荐，适合长期绑定）

1. 在 `Remote Invoke` 页面导出 Bifrost SSH key
2. 将 key 文件安全地交给 Agent 所在终端，建议保存为 `~/.bifrost/remote-device.key`
3. Agent 使用该 key 建立一次连接后，后续可长期复用，不用每次单独重新建立连接
4. 如需吊销该设备，回到 Web UI 撤销或重置 SSH key

### 方式 B：配对码

1. 在 `Remote Invoke` 页面点击进入发现模式
2. 记录页面显示的 6 位配对码
3. 保持该页面在线，等待 Agent 发起连接
4. 当页面弹出授权请求时，由用户选择：
   - 仅本次
   - 30 分钟
   - 1 小时
   - 1 天
   - 永久

说明：

- SSH key 是推荐方式，适合长期连接和重复使用
- 配对码只是首次发现目标终端的入口，不是长期凭证
- 真正的执行许可来自用户在目标终端上的人工授权，或来自已授权的 SSH key

## 四、Agent 如何与目标终端建立连接

Agent 在正式执行远程查询前，先确认本地是否已有已保存连接。如果没有，优先使用 SSH key 建立长期连接；没有 SSH key 时，再使用配对码连接。

如果需要先查看完整命令结构和参数说明，可以先执行：

```bash
bifrost remote -h
```

### 1. 使用 SSH key 建立长期连接（推荐）

```bash
bifrost remote connect --ssh-key <path>
```

示例：

```bash
bifrost remote connect --ssh-key ~/.bifrost/remote-device.key
```

还支持以下输入形式：

- `--ssh-key env:KEY_NAME`
- `--ssh-key -` 从标准输入读取

SSH 连接成功后，会把连接信息保存到本地，后续可以长期复用；只要授权未被撤销、SSH key 未被重置，就不需要每次单独重新建立连接。

### 2. 使用配对码连接

```bash
bifrost remote connect <pair-code>
```

示例：

```bash
bifrost remote connect 123456
```

连接流程如下：

1. Agent 输入配对码并发起连接
2. 目标终端 Web UI 出现待授权请求
3. 用户在目标终端批准授权
4. 本地终端保存连接信息到：
   ```text
   ~/.bifrost/remote-connections.json
   ```
5. 后续应优先使用已保存连接执行 `remote status`、`remote traffic ...`、`remote search ...`

如果授权过期、被撤销或本地没有连接缓存，重新执行 `remote connect`。

### 3. 多连接场景下选择目标客户端

如果本地保存了多个远程连接，优先显式指定：

```bash
bifrost remote status --client-id <client-prefix>
```

如果不指定且当前环境是交互终端，CLI 可能会提示用户选择连接；非交互环境下应显式传 `--client-id`。
如果需要覆盖默认 relay，再额外显式传入 `--relay-url <url>`。

## 五、建立连接后可以执行哪些命令

当前远程能力以只读查询为主。Agent 建立连接后，应优先使用以下命令：

### 1. 查看远端状态

```bash
bifrost remote status
```

### 2. 搜索远端流量

```bash
bifrost remote search <keyword> --max-results 50 --max-scan 200
```

其中：

- `--max-results` 控制最多返回多少条命中结果
- `--max-scan` 控制远端执行端最多扫描多少条记录
- `--limit` 仍可作为 `--max-results` 的兼容别名使用

### 3. 列出远端流量记录

```bash
bifrost remote traffic list --limit 50
```

支持附加过滤：

```bash
bifrost remote traffic list --limit 20 --method GET --status 200
```

### 4. 查看远端流量详情

```bash
bifrost remote traffic get <id>
```

如需附带 body：

```bash
bifrost remote traffic get <id> --request-body --response-body
```

### 5. 搜索远端流量详情

```bash
bifrost remote traffic search <keyword> --max-results 50 --max-scan 200
```

### 6. 撤销远端授权

```bash
bifrost remote disconnect
```

如需撤销全部授权或特定 grant：

```bash
bifrost remote disconnect --all
bifrost remote disconnect --grant-id <grant-id>
```

## 六、当前支持的远程执行白名单

当前实现只允许以下远程执行命令被 relay / client 接收：

- `status`
- `search.get`
- `traffic.list`
- `traffic.get`
- `traffic.search`

这意味着 Agent 应将远程能力理解为“查询远端运行状态与流量记录”，而不是“获得一台通用远程 shell 主机”。

## 七、明确不支持的操作

当前不要尝试通过 `bifrost remote` 做以下事情：

- 任意 shell 命令执行
- 规则管理：`rule add`、`rule update`、`rule delete`
- 配置管理：`config set`、`config remove`
- 脚本管理：`script run`、`script update`
- 证书管理：`ca generate`、`ca install`
- 文件上传、文件下载、目录浏览

如果用户需要这些写操作，应改为在目标终端本地执行，或等待未来能力扩展。

## 八、Agent 执行约束

Agent 使用本技能时，遵循以下原则：

1. 先判断是否已有本地保存连接，避免重复 `remote connect`
2. 有 SSH key 时优先用 SSH key 建立长期连接，不要默认每次都走配对码
3. 连接失败时，优先检查 SSH key 是否有效，或检查配对码是否过期、目标终端是否在线、Web UI 是否已授权
4. 仅执行当前白名单内的只读命令
5. 在多客户端场景下，明确确认目标客户端，避免误操作到错误设备
6. 如果 grant 失效，重新走配对或 SSH connect，不要伪造本地连接文件
7. 若用户只需要本机本地操作，优先使用普通 `bifrost` CLI，不必绕到 `remote`
