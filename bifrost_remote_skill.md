---
name: "bifrost-remote"
description: "通过 `bifrost remote` 子命令远程查询 Bifrost 客户端实例。当用户提到远程查询流量、远程查看状态、远程搜索请求、使用 `bifrost remote` 子命令等场景时触发。适用于目标 Bifrost 实例不在本机，需要通过 Relay 中转进行只读查询的情况。"
---

# Bifrost Remote — 远程查询技能

本技能用于指导 Agent 使用 `bifrost remote` 子命令，通过云端 Relay 中转，远程查询目标 Bifrost 客户端实例的状态和流量信息。

## 适用场景

- 目标 Bifrost 实例不在本机，需要通过 Relay 远程查询
- 用户提到"远程查看流量"、"远程查询状态"、"remote traffic"、"remote status"等
- 用户使用 `bifrost remote ...` 命令

## 前置条件

### 1. 确认 bifrost CLI 可用

```bash
bifrost --version
```

### 2. 确认 Relay 连接

bifrost remote 依赖 Relay 中转服务。Bifrost 客户端需要已经连接到 Relay 并处于在线状态。

### 3. 确认授权状态

远程调用需要授权。首次调用需要走配对授权流程：
1. 目标 Bifrost 客户端在 WebUI Settings -> Remote Invoke 中开启"发现模式"
2. 获取 6 位一次性授权码
3. 调用方使用授权码完成配对
4. 目标客户端用户在 WebUI 中人工批准

已授权的调用方可以在授权有效期内直接复用，无需重新配对。

## 命令能力映射

### 1. 查看远端状态

```bash
bifrost remote status
```

返回目标 Bifrost 客户端的运行状态概览。

### 2. 远程搜索流量

```bash
bifrost remote search <query>
```

按关键词全文搜索远端流量记录（覆盖 URL、headers、body）。

参数说明:
- `<query>`: 搜索关键词，支持中文等 Unicode 字符，禁止 ASCII 控制字符，最大长度 500

### 3. 远程流量列表

```bash
bifrost remote traffic list [--limit N] [--cursor C] [--method GET] [--host example.com] [--status 200] [--protocol https] ...
```

支持的过滤参数：
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

### 4. 远程获取流量详情

```bash
bifrost remote traffic get <id> [--request-body] [--response-body]
```

参数说明：
- `<id>`: 流量记录 ID 或 sequence 编号（纯数字）
- `--request-body`: 包含请求体
- `--response-body`: 包含响应体

当用户提及一个少于 6 位的数字 ID 并希望远程查看详情时，直接执行此命令。

### 5. 远程搜索流量（traffic search）

```bash
bifrost remote traffic search <query>
```

功能等价于 `bifrost remote search`。

## 命令协议说明

`bifrost remote` 采用"受控查询命令白名单"模型，不支持任意 shell 透传。

支持的命令白名单：
- `status` — 查询客户端运行状态
- `search.get` / `traffic.search` — 关键词搜索流量
- `traffic.list` — 分页查询流量列表
- `traffic.get` — 获取单条流量详情

不支持的操作（会返回 `unsupported_command` 错误）：
- 任何配置修改操作
- 规则新增/编辑/删除
- `traffic.clear`（写操作）
- values/config/cert/系统代理管理
- 任意文件访问或脚本执行

## 授权模式

远程调用支持以下授权策略：
| 模式 | 说明 |
|------|------|
| 仅本次 | 单次调用后授权自动失效 |
| 30 分钟 | 30 分钟内可复用 |
| 1 小时 | 1 小时内可复用 |
| 1 天 | 24 小时内可复用 |
| 永久 | 永久有效，需要二次确认 |

已授权的调用在有效期内会自动复用，无需重新配对。

## 常见工作流

### 远程排查某个域名的请求

```bash
bifrost remote traffic list --host example.com --limit 20
bifrost remote traffic get <id> --request-body --response-body
```

### 远程搜索关键词

```bash
bifrost remote search "error"
bifrost remote search "api/v1/users"
```

### 查看远端状态

```bash
bifrost remote status
```

## 安全说明

- 所有命令内容通过端到端加密传输（X25519 + ChaCha20-Poly1305），Relay 无法查看明文
- 每次调用使用独立的临时密钥对，调用结束后立即销毁
- Relay 仅存储命令摘要和审计信息，不存储原始命令或结果明文
- 授权绑定调用方设备指纹（`caller_fingerprint`），token 被窃取也无法冒充

## Agent 行为建议

- 优先检查是否有可复用授权，避免每次都走配对流程
- 远程命令仅支持只读查询，不要尝试远程修改配置或规则
- 当用户需要修改远端 Bifrost 配置时，提示用户直接在目标机器上操作
- 如果远程命令返回 `unsupported_command`，说明该操作不在白名单内
- 如果返回授权相关错误，引导用户在目标 Bifrost WebUI 中进行授权
- 遇到网络超时或 Relay 不可达，提示检查 Relay 服务状态和网络连接
- 当用户提供一个少于 6 位的纯数字并希望远程查看详情时，使用 `bifrost remote traffic get <ID> --request-body --response-body`

## 特别说明

完整参数和用法以 CLI 内置帮助为准：

```bash
bifrost remote -h
bifrost remote traffic -h
bifrost remote traffic list -h
bifrost remote traffic get -h
bifrost remote search -h
```
