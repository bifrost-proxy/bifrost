---
title: "Breakpoint 使用手册"
description: "从 docs/breakpoint.md 自动同步生成的 Breakpoint 使用手册 文档。"
editLink: false
---

> 此页面由 `docs/breakpoint.md` 自动同步生成。
# Breakpoint 使用手册

Breakpoint 用于在 Bifrost 转发 HTTP 流量时暂停指定 request 或 response。暂停后可以查看并修改 headers/body，再继续放行。它适合临时调试接口、验证上游入参、模拟下游响应、排查鉴权/header 问题，或在不改业务代码的情况下验证请求和响应改写效果。

## 适用场景

- 查看某个接口真正发往上游的 request headers/body。
- 临时修改 request body、query、headers 后再发送给上游。
- 在 response 返回客户端前修改 headers/body，验证客户端兼容性。
- 模拟异常响应、特殊 header 或边界数据。
- 只针对某个域名、端口或路径调试，避免影响其他网络请求。

不建议把 Breakpoint 长期开在宽泛规则上。Breakpoint 是交互式调试能力，命中的流量会等待用户 Resume 或等待超时自动放行；规则越宽，越容易造成请求等待、连接堆积和额外 CPU/内存开销。

## 工作方式

Breakpoint 有双重门禁：

1. Traffic Toolbar 中的 Breakpoint 开关必须打开。
2. 请求必须命中包含 `breakpoint://request` 或 `breakpoint://response` 的规则。

只打开 Toolbar 开关不会暂停任何流量。只有全局开关打开，并且规则明确授权了 request 或 response 阶段，流量才会暂停。

## 在 WebUI 中使用

1. 打开 WebUI，进入 Traffic 页面。
2. 在 Toolbar 中打开 `Breakpoint` 开关。
3. 进入 Rules 页面，为目标流量增加一条尽量精确的规则。
4. 发送命中该规则的请求。
5. 回到 Traffic 页面，打开对应请求的 TrafficDetail。
6. Network 行会显示 request/response 暂停标识；选中后详情自动打开对应阶段。
7. Request 可编辑 method、URL/query、headers 和可编辑 body；Response 可编辑 status、headers 和可编辑 body。
8. 点击 `Resume unchanged` 原样放行，或点击 `Apply & Resume` 应用编辑后放行。

pending 的 Network 行整行使用主题自适应的淡黄色警示背景；恢复、关闭 Breakpoint 或 timeout 自动放行后，背景立即消失。浅色和深色主题分别使用各自的 warning token，不使用固定浅色值。

Breakpoint 关闭后，新流量不会再暂停；已经 pending 的 breakpoint 会被释放。

## 规则配置

使用 `breakpoint` 协议选择暂停阶段：

```text
api.example.com/v1/users breakpoint://request
api.example.com/v1/users breakpoint://response
api.example.com/v1/users breakpoint://request,response
```

支持的 value：

- `request` 或 `req`：在 request 发往上游前暂停。
- `response` 或 `res`：在 response 返回客户端前暂停。
- `request,response`、`req,res`、`both` 或 `all`：request 和 response 两个阶段都会暂停。

建议把规则 pattern 收窄到明确域名、端口、路径或方法。例如只调试登录接口时，优先写登录路径，不要写整个域名。空 value 或无法识别的 value 不会触发 Breakpoint。

## Request Breakpoint

`breakpoint://request` 会在请求发往上游前暂停。适合：

- 检查真实 request headers/body。
- 临时添加、删除或修改请求 header。
- 修改 body 后再发送给上游。
- 验证客户端请求是否符合预期。

Resume 后，上游会收到修改后的 method、URL/query、headers/body。若 body 因过大、非文本或无法在安全上限内完整读取，页面会提示 body 被省略，此时仍可修改 method、URL/query 和 headers，Resume 时不会替换原始 body。

## Response Breakpoint

`breakpoint://response` 会在上游返回后、客户端收到前暂停。适合：

- 查看上游真实响应。
- 临时修改响应 header。
- 修改响应 body，验证客户端展示或错误处理。
- 模拟特殊状态或特殊响应内容。

Resume 后，客户端会收到修改后的 status、headers/body。普通未知长度正文会在安全上限内有界读取后允许编辑；超限或持续流式响应保留原始 streaming。受支持的压缩正文会以解压文本编辑，并在放行前按最终 `Content-Encoding` 重新编码。

## Timeout 配置

Breakpoint Auto-Resume Timeout 位于 `Settings -> Performance`。

- 默认值：30 秒。
- 可配置范围：5 秒到 5 分钟。
- 超时后：Bifrost 自动继续原始 request/response，不应用未提交的编辑。

这个配置用于避免用户忘记点击 Resume 时请求无限等待。较低值能降低网络延迟和连接堆积风险；较高值适合需要较长时间手动检查 headers/body 的场景。超过 5 分钟会带来明显的连接占用和性能风险，因此不允许配置。

## 性能与安全建议

- 默认保持 Breakpoint 关闭，只在需要调试时打开。
- 使用精确规则，避免把整个域名或所有流量都纳入 breakpoint。
- 优先只暂停需要的阶段：只看请求就用 `breakpoint://request`，只看响应就用 `breakpoint://response`。
- 调试完成后关闭 Breakpoint 或禁用对应规则。
- 对大 body、二进制 body、未知长度 streaming body，Bifrost 会限制编辑能力，避免额外复制和缓存造成明显性能开销。
- 如果发现请求等待变长，降低 Auto-Resume Timeout 或收窄规则范围。

## API 自动化

日常使用建议通过 WebUI 操作。需要和外部工具、脚本或团队自动化集成时，可以使用 Admin API。

启用或关闭全局 Breakpoint 门禁：

```bash
curl -sS -X POST http://127.0.0.1:8800/_bifrost/api/breakpoint/settings \
  -H 'Content-Type: application/json' \
  --data '{"enabled":true,"max_body_bytes":1048576}'
```

修改 Auto-Resume Timeout：

```bash
curl -sS -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H 'Content-Type: application/json' \
  --data '{"breakpoint_timeout_ms":30000}'
```

恢复一个暂停项：

```bash
curl -sS -X POST http://127.0.0.1:8800/_bifrost/api/breakpoint/resume \
  -H 'Content-Type: application/json' \
  --data '{
    "request_id": "REQ-xxxx",
    "phase": "request",
    "headers": [["X-Debug", "1"]],
    "body": "edited body"
  }'
```

完整 API 描述可在 WebUI 的 Swagger 页面查看，或通过 `/_bifrost/api/openapi.json` 获取。

页面刷新或 push 重连后可查询当前暂停项：

```bash
curl -sS http://127.0.0.1:8800/_bifrost/api/breakpoint/pending
```

## 常见问题

**打开 Breakpoint 后为什么没有暂停？**

确认目标流量命中了包含 `breakpoint://request` 或 `breakpoint://response` 的规则。Toolbar 开关只是全局门禁，不会单独暂停流量。

标准 TLS 端口上的 HTTPS 请求命中 Breakpoint 规则且全局开关开启时，会自动触发该连接的 scoped TLS interception；显式 `tlsIntercept://false` 仍优先。客户端必须信任 Bifrost CA，Toolbar 在全局 TLS interception 关闭时会提示这一点。

**为什么 body 不能编辑？**

通常是 body 超过上限、不是 UTF-8 文本、属于二进制内容，或是未知长度 streaming body。此时 Bifrost 会保护性能，只允许 header-only pause 或直接保持 streaming。

**请求一直等待怎么办？**

点击 Resume，或等待 Auto-Resume Timeout 自动放行。也可以在 Settings -> Performance 降低 timeout，并收窄 breakpoint 规则范围。

**如何避免影响其他应用？**

使用更精确的规则，例如域名 + 路径 + 方法；调试结束后关闭 Breakpoint 或禁用规则。
