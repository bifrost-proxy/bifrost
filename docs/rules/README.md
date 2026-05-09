# Bifrost 规则协议手册

本目录用于按“协议”拆分说明规则的具体能力（如路由、请求/响应修改、脚本、WebSocket 等）。

在阅读本目录前，建议先了解整体语法与基础概念：

- 规则整体语法：[rule.md](../rule.md)
- URL 匹配（pattern）：[pattern.md](../pattern.md)
- 操作指令（operation/value/模板变量/Values 引用）：[operation.md](../operation.md)

---

## 文档导航

- 匹配模式详解：[patterns.md](./patterns.md)
- 规则优先级与执行顺序：[rule-priority.md](./rule-priority.md)
- 路由与转发规则：[routing.md](./routing.md)
- 请求修改规则：[request-modification.md](./request-modification.md)
- 响应修改规则：[response-modification.md](./response-modification.md)
- URL 操作规则：[url-manipulation.md](./url-manipulation.md)
- Body 操作规则：[body-manipulation.md](./body-manipulation.md)
- 状态码与重定向：[status-redirect.md](./status-redirect.md)
- 延迟与限速规则：[timing-throttle.md](./timing-throttle.md)
- 过滤器规则：[filters.md](./filters.md)
- 脚本规则：[scripts.md](./scripts.md)
- WebSocket 规则：[websocket.md](./websocket.md)

---

## 快速索引（按能力分类）

### 1) 路由与转发

- `host` / `xhost`
- `http` / `https`
- `ws` / `wss`
- `http3` / `h3`
- `proxy` / `pac`
- `redirect`
- `file` / `tpl` / `rawfile`
- `tunnel`

详见：[routing.md](./routing.md)、[status-redirect.md](./status-redirect.md)

### 2) 请求修改

- 头部/Cookie：`reqHeaders`、`reqCookies`、`auth`、`forwardedFor`
- 方法与常用字段：`method`、`ua`、`referer`、`reqType`、`reqCharset`
- CORS：`reqCors`
- Body：`reqBody`、`reqPrepend`、`reqAppend`、`reqReplace`、`params`（兼容别名 `reqMerge`）
- 延迟与限速：`reqDelay`、`reqSpeed`
- DNS / 上游协议：`dns`、`http3` / `h3`

详见：[request-modification.md](./request-modification.md)、[body-manipulation.md](./body-manipulation.md)、[timing-throttle.md](./timing-throttle.md)、[routing.md](./routing.md)

### 3) 响应修改

- 头部/Cookie/CORS：`resHeaders`、`resCookies`、`resCors`
- 常用响应字段：`resType`、`resCharset`、`cache`、`attachment`、`responseFor`、`trailers`
- Body：`resBody`、`resPrepend`、`resAppend`、`resReplace`、`resMerge`
- 状态码：`statusCode` / `replaceStatus`
- 延迟与限速：`resDelay`、`resSpeed`

详见：[response-modification.md](./response-modification.md)、[body-manipulation.md](./body-manipulation.md)、[status-redirect.md](./status-redirect.md)、[timing-throttle.md](./timing-throttle.md)

### 4) URL 操作

- `urlParams`
- `urlReplace`

详见：[url-manipulation.md](./url-manipulation.md)

### 5) 脚本与 decode

- `reqScript` / `resScript`
- `decode`
- `bp` + `decode://bp`

详见：[scripts.md](./scripts.md)（脚本规则）与 [../scripts.md](../scripts.md)（管理端 Scripts 使用与开发指南）

### 6) TLS / DevTools / 控制规则

- `tlsIntercept` / `tlsPassthrough`
- `tlsOptions` / `sniCallback`
- `passthrough` / `tunnel`
- `devtools`

详见：[../operation.md](../operation.md) 与 [filters.md](./filters.md)

### 7) WebSocket

- `ws` / `wss`

详见：[websocket.md](./websocket.md)
