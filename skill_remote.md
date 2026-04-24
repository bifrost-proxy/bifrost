---
name: "bifrost-remote"
description: "通过 Bifrost Remote Invoke 实现远程设备控制能力：连接另一台电脑上的 Bifrost，指导目标终端安装/启动并开启系统代理，使用 SSH key 或配对码授权，查询远端状态/流量，并通过受 Shell Access policy 控制的 remote command exec 操作目标设备。"
---

# Bifrost Remote

该技能用于指导用户和 Agent 通过 `bifrost remote` 与目标终端上的 Bifrost 建立 Remote Invoke 连接。Remote command 的目标是在用户明确授权后，让 Agent 可以操作另一台设备；能力边界来自目标终端的授权、Shell Access policy 和当前 CLI 已实现的 relay-backed 子命令。

## 适用场景

当用户有以下需求时，应优先使用本技能：

- 让 Agent 连接另一台机器上的 Bifrost。
- 指导用户在目标终端安装 Bifrost 并准备 Remote Invoke。
- 使用 SSH key 建立长期可复用连接。
- 首次通过 6 位配对码建立连接。
- 查询远端 Bifrost 状态、流量列表、流量详情或搜索结果。
- 在远端执行受控 `shell.exec` 命令。
- 解释授权、grant scope、Shell Access policy、SSH key revoke 等语义。

以下场景需要先确认授权和执行路径：

- 需要操作目标设备文件、配置、规则、脚本、证书或系统代理时，优先通过已授权的 `bifrost remote command exec ...` 在目标终端执行对应命令。
- 目标终端未配置匹配的 Shell Access policy 时，引导用户在目标终端本机或 Web UI 中放行对应 policy。
- 不要绕过目标终端 Web UI 或 SSH key 授权。

## 一、目标终端如何安装 Bifrost

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

## 二、目标终端如何启动 Bifrost

启动前必须先检查是否已有服务在运行：

```bash
bifrost status
```

如果已有 Bifrost 正在运行，直接复用当前实例，不要启动第二个代理。

如果目标终端尚未运行 Bifrost，面向用户正式使用场景应使用默认数据目录、默认端口和系统代理启动。这样 Remote Invoke、Web UI 和设备流量采集会落在同一个用户预期的本机实例上：

```bash
bifrost start
```

该命令默认以前台方式运行。启动成功后，继续执行 `bifrost status`、打开 Web UI、或执行后续 `bifrost remote ...` 命令时，请在另一个终端窗口中操作。

启动后先检查状态：

```bash
bifrost status
```

只有测试或临时验证场景才应使用临时数据目录、非 9900 端口和 `--no-system-proxy`，避免污染用户正式实例或修改测试机系统代理。

## 三、两类操作的前置准备工作

Remote Invoke 有两类常见操作，准备工作不同：

- 只读查询类：`remote status`、`remote search`、`remote traffic list/get/search`。
- 远程设备控制类：`remote command exec`，用于在目标设备上执行受 Shell Access policy 控制的命令。

### 3.1 只读查询类：启用 Remote Invoke 授权

在目标终端打开本地 Web UI。端口应以 `bifrost status` 输出为准，默认是：

```text
http://127.0.0.1:9900/_bifrost/settings?tab=remote-invoke
```

推荐优先使用 SSH key；如果暂时没有 SSH key，再使用配对码。

#### 方式 A：SSH key（推荐，适合长期绑定）

1. 在 `Remote Invoke` 页面创建或导出 Bifrost SSH key。
2. 将 key 文件安全地交给 Agent 所在终端，建议保存为 `~/.bifrost/remote-device.key`。
3. Agent 使用该 key 建立一次连接后，后续可长期复用。
4. 如需吊销该设备，回到 Web UI reset 或 revoke SSH key；SSH 授权预期永久有效直到 key 被撤销或重置。

#### 方式 B：配对码

1. 在 `Remote Invoke` 页面点击 `Enter Discovery Mode`。
2. 记录页面显示的 6 位配对码。
3. 保持页面在线，等待 Agent 发起连接。
4. 当页面弹出授权请求时，由用户选择 `query` 访问模式和授权时长。

说明：

- 配对码只是首次发现目标终端的入口，不是长期凭证。
- 真正的执行许可来自用户在目标终端上的人工授权，或来自已授权的 SSH key。
- Pair code 成功消费后不要重复使用；后续应优先复用保存的连接。

完成只读查询类准备后，Agent 在 caller 终端执行连接并验证：

```bash
bifrost remote connect --ssh-key ~/.bifrost/remote-device.key
# 或：
bifrost remote connect <pair-code>

bifrost remote status
```

`bifrost remote status` 成功后，才继续执行 `remote search`、`remote traffic list/get/search`。

### 3.2 远程设备控制类：启用 Shell Access policy

远程设备控制类操作需要先完成 3.1 的 Remote Invoke 授权；在此基础上，目标终端还必须启用匹配的 Shell Access policy/profile，并在授权请求中选择 `selected` 或 `all` 访问模式。

目标终端上的准备流程：

1. 用户在目标终端本机或 Web UI 中配置 Shell Access profile，定义允许的 cwd、env、timeout、stdin/interactive 等执行环境。
2. 用户在目标终端本机或 Web UI 中配置 Shell Access policy，定义允许的 argv 程序或 shell_text 正则。
3. 用户在目标终端 `Remote Invoke` 授权请求中选择：
   - `selected`：只绑定指定 Shell Access policy。
   - `all`：允许当前已启用 Shell Access policy 覆盖的命令。
4. 如果需要 stdin 或 interactive 能力，授权时还要开启对应选项；底层 grant scope 会对应 `remote_shell_interactive`。

目标终端本机 CLI 示例：

```bash
bifrost remote shell profile add \
  --id default \
  --name "Default" \
  --cwd "$HOME" \
  --env PATH \
  --env HOME \
  --default-cwd "$HOME" \
  --timeout-ms 30000 \
  --inherit-env

bifrost remote shell policy add \
  --id allow-bifrost-cli \
  --name "Allow Bifrost CLI" \
  --mode shell_text \
  --pattern '^bifrost\s+' \
  --shell /bin/bash \
  --profile default
```

如果用户希望 Agent 完整操作目标设备，可以在目标终端明确创建更宽的 Shell Access policy，并在授权请求中选择 `all` 或绑定对应 policy。能力开放程度由目标终端用户决定。

caller 终端验证远程设备控制能力：

```bash
bifrost remote command exec --shell-text "bifrost status"
```

如果 caller 当前只有 `query` 授权，目标终端用户需要重新授权或在目标终端本机更新 grant，使该 caller 获得 `selected` 或 `all` 访问模式，以及对应 Shell Access policy binding。

## 四、Agent 如何与目标终端建立连接

Agent 在正式执行远程查询前，先确认本地是否已有已保存连接。如果没有，优先使用 SSH key 建立长期连接；没有 SSH key 时，再使用配对码连接。

如果需要先查看完整命令结构和参数说明，可以执行：

```bash
bifrost remote -h
```

### 1. 使用 SSH key 建立长期连接

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

SSH connect 成功后会把连接信息保存到本地。只要目标端 SSH key 未被 reset/revoke，后续不需要重复 connect。

### 2. 使用配对码连接

```bash
bifrost remote connect <pair-code>
```

示例：

```bash
bifrost remote connect 123456
```

连接流程如下：

1. Agent 输入配对码并发起连接。
2. 目标终端 Web UI 出现待授权请求。
3. 用户在目标终端批准授权。
4. 本地终端保存连接信息到 Bifrost 数据目录下的 `remote-connections.json`。
5. 后续优先使用已保存连接执行 `remote status`、`remote traffic ...`、`remote search ...`。

如果授权过期、被撤销或本地没有连接缓存，重新执行 `remote connect`。

### 3. 多连接场景下选择目标客户端

如果本地保存了多个远程连接，优先显式指定：

```bash
bifrost remote status --client-id <client-prefix>
```

如果不指定且当前环境是交互终端，CLI 可能会提示用户选择连接；非交互环境下应显式传 `--client-id`。如果需要覆盖默认 relay，再额外显式传入 `--relay-url <url>`。

## 五、建立连接后可以执行哪些命令

远程能力分为只读查询和受控 `shell.exec` 两层，具体能力取决于用户批准的访问模式和底层 grant scope。

### 授权模型

| UI 访问模式 | 底层 grant scope | 允许的操作 |
| --- | --- | --- |
| `query` | `remote_query` | 只读查询：status、search、traffic list/get/search |
| `selected` | `remote_shell_exec` 或 `remote_shell_interactive` | 查询 + 绑定指定 Shell Access policy 的 `shell.exec` |
| `all` | `remote_shell_exec` 或 `remote_shell_interactive` | 查询 + 所有已启用 Shell Access policy 覆盖的 `shell.exec` |

`remote_shell_interactive` 表示 grant 允许 stdin/interactive 相关能力；当前是否能形成完整交互体验取决于 CLI/PTY 能力和目标端 policy。

### 1. 查看远端状态

```bash
bifrost remote status
```

### 2. 搜索远端流量

```bash
bifrost remote search <keyword> --max-results 50 --max-scan 200
```

其中：

- `--max-results` 控制最多返回多少条命中结果。
- `--max-scan` 控制远端执行端最多扫描多少条记录。
- `--limit` 仍可作为 `--max-results` 的兼容别名使用。

支持过滤选项：

```bash
bifrost remote search <keyword> --url
bifrost remote search <keyword> --headers
bifrost remote search <keyword> --body
bifrost remote search <keyword> --req-header
bifrost remote search <keyword> --res-body

bifrost remote search <keyword> --method GET --status 2xx --host example.com
bifrost remote search <keyword> --protocol HTTPS --content-type json --domain "*.api.com"
```

### 3. 列出远端流量记录

```bash
bifrost remote traffic list --limit 50
```

支持过滤和分页：

```bash
bifrost remote traffic list --limit 20 --method GET --status 200
bifrost remote traffic list --method POST --status-min 400 --status-max 499
bifrost remote traffic list --protocol https --host example.com --path "/api/"
bifrost remote traffic list --content-type json --client-ip 192.168.1.1
bifrost remote traffic list --has-rule-hit true --is-websocket false
bifrost remote traffic list --limit 20 --cursor <cursor> --direction forward
bifrost remote traffic list --format json-pretty
bifrost remote traffic list --format compact --no-color
```

### 4. 查看远端流量详情

```bash
bifrost remote traffic get <id>
```

如需附带 body：

```bash
bifrost remote traffic get <id> --request-body --response-body
```

支持格式选项：`--format table|compact|json|json-pretty`。

### 5. 搜索远端流量详情

```bash
bifrost remote traffic search <keyword> --max-results 50 --max-scan 200
```

`traffic search` 支持与顶层 `search` 相同的过滤选项。

### 6. 在远端执行受控 shell 命令

需要用户授权 `selected` 或 `all` 访问模式，且目标终端已配置匹配的 Shell Access policy。

```bash
bifrost remote command exec --shell-text "ls -la /tmp"
bifrost remote command exec -- ls -la /tmp
bifrost remote command exec --cwd /home/user --env MY_VAR=hello --shell-text "echo $MY_VAR"
bifrost remote command exec --timeout-ms 10000 --shell-text "sleep 5 && echo done"
```

说明：

- `shell.exec` 通过目标终端的 Shell Access policy 做命令、cwd、env、stdin、timeout 等限制。
- 当前实现会在目标终端本机按 policy 启动进程；Agent 可以在授权允许范围内操作目标设备。
- 如果目标策略是未实现的 sandbox policy，执行端会拒绝执行并提示 sandbox execution is not implemented。
- 命令内容和输出通过 encrypted remote invoke 通道传输，但 relay 仍会保存必要的路由、审计和摘要元数据。

### 7. 撤销本地保存的远端授权

```bash
bifrost remote disconnect
```

如需撤销全部授权或特定 grant：

```bash
bifrost remote disconnect --all
bifrost remote disconnect --grant-id <grant-id>
```

## 六、当前支持的 relay-backed 远程命令

### 查询类命令（`query.readonly`）

以下命令在 `remote_query` 及以上 scope 下可用：

- `status` — 查询远端状态。
- `search.stream` — 搜索远端流量。
- `traffic.list` — 列出远端流量记录。
- `traffic.get` — 查看远端流量详情。

### Shell 执行命令（`shell.exec`）

以下命令在 `remote_shell_exec` 或 `remote_shell_interactive` scope 下可用：

- `shell.exec` — 在目标终端按 Shell Access policy 执行受控命令。

Agent 应将远程能力理解为“查询远端运行状态与流量记录”加上“在授权和策略控制下操作目标设备的 shell.exec”。

## 七、本地管理命令与远端操作路径

`bifrost remote shell ...` 和 `bifrost remote grant ...` 是目标终端本机管理命令：

- `bifrost remote shell ...` 管理当前机器数据目录中的 Shell Access policy/profile。
- `bifrost remote grant ...` 管理当前机器 admin API 中的 local grants。

caller 侧不应把这两个子命令当成 relay-backed 管理 API 直接调用；如果用户希望远程管理目标设备，可以通过 `bifrost remote command exec ...` 在目标终端执行等价的本机命令，前提是目标端 policy 已授权。

如果需要让远端允许某个 `shell.exec`，正确流程是：

1. 用户或 Agent 在目标终端本机配置 Shell Access policy。
2. 用户在目标终端 Web UI 授权 caller 的访问模式和 policy binding。
3. caller 再执行 `bifrost remote command exec ...`。

## 八、当前 relay-backed 子命令边界

`traffic clear` 当前不是已启用的 relay-backed query 子命令；如果用户要清理目标端流量记录，应通过已授权的 `remote command exec` 在目标端执行本机 CLI/API 操作。

类似地，rule/config/script/value/CA/系统代理等没有专门的 `bifrost remote <module>` 子命令时，不代表 Agent 不能操作目标设备；应切换到 `remote command exec`，在用户授权和 Shell Access policy 允许范围内执行目标机命令。

## 九、Agent 执行约束

Agent 使用本技能时，遵循以下原则：

1. 先判断是否已有本地保存连接，避免重复 `remote connect`。
2. 有 SSH key 时优先用 SSH key 建立长期连接，不要默认每次都走配对码。
3. 连接失败时，优先检查 SSH key 是否有效，或检查配对码是否过期、目标终端是否在线、Web UI 是否已授权。
4. 查询操作可直接执行；`shell.exec` 需确认用户已授权 shell 访问且目标终端已配置匹配 policy。
5. 在多客户端场景下，明确确认目标客户端，避免误操作到错误设备。
6. 如果 grant 失效，重新走配对或 SSH connect，不要伪造本地连接文件。
7. 若用户只需要本机本地操作，优先使用普通 `bifrost` CLI，不必绕到 `remote`。
8. caller 侧需要管理目标设备时，优先使用 `remote command exec` 执行目标机命令，而不是误把本地 `remote shell` / `remote grant` 当成 relay-backed API。
9. 不要承诺 OS 级 sandbox；描述为当前 Shell Access policy 的授权和限制能力。
