# 路由与转发规则

本章介绍控制请求目标地址和转发方式的规则。

---

## host

将请求重定向到指定主机，是最常用的路由规则。

### 语法

```
pattern host://target[:port]
```

### 参数说明

| 参数     | 说明            | 示例                           |
| -------- | --------------- | ------------------------------ |
| `target` | 目标主机名或 IP | `127.0.0.1`, `api.backend.com` |
| `port`   | 可选，目标端口  | `8080`, `3000`                 |

### 基础示例

```bash
# 域名重定向到本地
www.example.com host://127.0.0.1

# 域名重定向到指定端口
www.example.com host://127.0.0.1:8080

# 域名重定向到另一域名
www.example.com host://api.backend.com

# 带端口的目标
www.example.com host://api.backend.com:3000
```

### HTTPS CONNECT 行为

`host://` 会改变请求的上游目标地址。对 HTTPS `CONNECT` 或 SOCKS5 建立的 TLS 隧道，命中具备明确域名/IP 作用域的 `host://` 规则时，即使 pattern 没有 `https://` 前缀，Bifrost 也会自动开启 TLS 解包，让内层 HTTPS 请求继续按 path、过滤器和优先级匹配。

如果希望按 HTTPS path、请求/响应头或 body 做处理，可以使用明确域名/IP pattern 的路径级路由或内容类规则；这类规则同样可自动触发 TLS 解包，不要求 matcher 前写 `https://`。例如 `www.example.com/api https://127.0.0.1:8443` 会先根据 `www.example.com` 识别需要解包，再在内层请求中按 `/api` 精确匹配。

### 通配符匹配

```bash
# 单级子域名通配（匹配 a.example.com, b.example.com）
*.example.com host://backend.local

# 多级子域名通配（匹配 a.b.example.com, x.y.z.example.com）
**.example.com host://backend.local
```

### 路径匹配

```bash
# 匹配特定路径
www.example.com/api host://api-server.local

# 路径通配
www.example.com/api/* host://api-server.local
```

### 测试用例

| 测试场景     | 规则                                    | 请求                            | 预期                   |
| ------------ | --------------------------------------- | ------------------------------- | ---------------------- |
| 基础重定向   | `test.com host://127.0.0.1:MOCK_PORT`   | `GET http://test.com/`          | 请求到达 Mock 服务器   |
| 带端口重定向 | `test.com host://127.0.0.1:8888`        | `GET http://test.com/`          | 请求转发到 8888 端口   |
| 路径保留     | `test.com host://127.0.0.1:MOCK_PORT`   | `GET http://test.com/api/users` | 路径 `/api/users` 保留 |
| 通配符匹配   | `*.test.com host://127.0.0.1:MOCK_PORT` | `GET http://api.test.com/`      | 匹配成功               |

---

## xhost

与 `host` 类似，但即使请求被其他规则处理，`xhost` 仍然会执行。

### 语法

```
pattern xhost://target[:port]
```

### 示例

```bash
www.example.com xhost://127.0.0.1:8080
```

---

## http / https

`http://` 和 `https://` 是显式上游协议转发规则。它们和 `host://` 一样会保留原请求路径与查询参数，但会把上游协议固定为 HTTP 或 HTTPS。

### 语法

```txt
pattern http://target[:port]
pattern https://target[:port]
```

### 示例

```bash
# 强制走 HTTP 上游
api.example.com http://127.0.0.1:3000

# 强制走 HTTPS 上游
api.example.com https://backend.example.com

# 路径级 HTTPS 转发：HTTPS CONNECT 阶段会因具体域名作用域自动解包，
# matcher 前不需要额外写 https://
api.example.com/v1 https://10.0.0.10:8443
```

若目标是 WebSocket，请优先使用 [WebSocket 规则](./websocket.md) 中的 `ws://` / `wss://`。

### HTTPS CONNECT 行为

带具体域名/IP 作用域的 `http://` / `https://` 路径级路由会自动开启 TLS 解包，即使 pattern 没有协议前缀。Bifrost 在 CONNECT 阶段只能看到 host；当 matcher 的 host 作用域覆盖当前目标时，会先解包，让后续内层 HTTPS 请求按 path、过滤器和优先级正常选择最具体规则。

如果目标地址与 pattern 指向同一站点，例如 `example.com/app https://example.com/app`，parser 会把它规范为 `passthrough://`。`passthrough://` 与其他路由目标遵循 first-win：更高优先级的具体 `https://` / `http://` / `host://` 决策已经选中时，后续更宽泛 passthrough 不会覆盖它。

---

## http3

为命中的请求启用上游 HTTP/3 尝试。

### 语法

```txt
pattern http3://
pattern h3://
```

### 使用场景

- 默认保持现有的 HTTP/1.1 / HTTP/2 上游代理行为
- 只对指定域名显式启用上游 H3 探测与协商
- 验证目标服务是否支持 QUIC/HTTP/3

### 示例

```bash
# 对指定站点启用上游 H3 尝试
chatgpt.com http3://

# 使用别名
api.example.com h3://
```

### 行为说明

- 该规则只控制代理到目标服务的上游连接
- 不会因此自动开启下游 UDP/QUIC 监听
- 仅对 HTTPS 上游请求生效
- 若目标不支持 H3，或 QUIC 建连失败，会自动回退到现有的 HTTP/1.1 / HTTP/2 转发链路
- 对普通绝对 URI 请求可直接生效；对浏览器常见的 HTTPS `CONNECT` 流量，通常需要 TLS interception 后，代理才能在解密后的转发阶段尝试 H3

### 测试用例

| 测试场景  | 规则                 | 预期                                  |
| --------- | -------------------- | ------------------------------------- |
| 默认关闭  | 无                    | 访问 H3-only HTTPS 目标时不会主动走 H3 |
| 显式启用  | `test.com http3://`  | 代理优先尝试上游 H3，失败后自动回退   |
| 别名启用  | `test.com h3://`     | 与 `http3://` 行为一致                |

---

## upstreamUnsafeSsl

为命中的单条规则允许不安全的上游 HTTPS 证书。适用于内网、自签名或测试环境上游，不需要通过启动参数全局开启 `--unsafe-ssl`。

### 语法

```
pattern https://host:port upstreamUnsafeSsl://true
```

`true` 可替换为 `1` / `yes` / `on`；裸 `upstreamUnsafeSsl://` 也视为启用。需要在组合规则中显式关闭时，可写 `upstreamUnsafeSsl://false`。

### 示例

```txt
qianchuan.jinritemai.com https://10.37.102.138:8080 upstreamUnsafeSsl://true
qianchuan.jinritemai.com https://10.37.102.138:8080 upstreamUnsafeSsl://true excludeFilter:///account excludeFilter:///api
```

### 行为说明

- 只影响 Bifrost 连接上游 HTTPS 服务时的证书校验。
- 只对命中该规则的请求生效；其他请求仍执行默认安全校验。
- 不改变客户端对 Bifrost CA 或目标站点证书的信任状态。
- 可以与 `https://`、`host://`、`tunnel://` 等上游路由规则组合；它不会单独改变请求目标。
- 对需要更精细匹配的路径应结合 `includeFilter`、`excludeFilter` 或正则过滤器使用。
- 如果未配置该协议且上游 TLS 证书不可信，Bifrost 返回的默认错误响应会提示在匹配规则上追加 `upstreamUnsafeSsl://true`。

### 测试用例

| 测试场景 | 规则 | 预期 |
| -------- | ---- | ---- |
| 单规则允许不安全证书 | `test.com https://127.0.0.1:8443 upstreamUnsafeSsl://true` | 自签名 HTTPS 上游可以成功转发 |
| 未命中规则仍保持安全校验 | `other.com https://127.0.0.1:8443` | 自签名 HTTPS 上游返回 TLS 校验失败 |
| 失败提示可操作 | 未配置 `upstreamUnsafeSsl` 且上游证书不可信 | 默认错误响应 body 包含 `upstreamUnsafeSsl://true` 建议 |

---

## proxy

通过 HTTP 代理转发请求。

### 语法

```
pattern proxy://proxy_host:proxy_port
```

### 参数说明

| 参数         | 说明           |
| ------------ | -------------- |
| `proxy_host` | 代理服务器地址 |
| `proxy_port` | 代理服务器端口 |

### 示例

```bash
# 通过代理转发所有请求
* proxy://proxy.company.com:8080

# 特定域名通过代理
*.internal.com proxy://proxy.internal:3128

# 带认证的代理（通过 URL）
example.com proxy://user:pass@proxy.com:8080
```

### HTTPS CONNECT 行为

`proxy://` 只选择下游代理出口。对 HTTPS `CONNECT` 或 SOCKS5 建立的 TLS 隧道，命中纯 `proxy://` 规则时，Bifrost 会向下游 HTTP 代理发送 `CONNECT original_host:original_port` 并透传原始 TLS 字节，不会因为这条规则自动解包。

这是严格边界：即使 proxy 规则 matcher 带具体域名或路径，例如 `example.com/app proxy://127.0.0.1:8080`，只要目标协议只有下游代理转发这一类，就不能因为这条 proxy 规则自动 TLS 解包。

仍会触发解包的情况：

- 全局 TLS 拦截已开启，或目标命中 TLS include / 应用 include。
- 规则显式配置了 `tlsIntercept://`。
- 同一目标命中需要读取/修改 HTTPS 内层 HTTP 的规则，并且该规则 pattern 有明确 host 作用域，例如 `api.example.com resHeaders://X-Debug=1`、`*.example.com reqHeaders://X-Env=test`、`192.168.1.10 resBody://...`。
- 同一目标命中路径级 `http://` / `https://` / `host://` / `xhost://` / `ws://` / `wss://` 路由，并且 matcher 有明确 host 作用域，例如 `api.example.com/v1 https://10.0.0.10:8443`。

不会单独触发自动解包的情况：

- 纯下游代理：`example.com proxy://127.0.0.1:8080`。
- 纯 wildcard / regex 内容规则：`* resHeaders://X-Debug=1`、`*/api/* resHeaders://X-Debug=1`、`/api\/v\d+/ resHeaders://X-Debug=1`。

### 测试用例

| 测试场景      | 规则                                              | 预期                   |
| ------------- | ------------------------------------------------- | ---------------------- |
| HTTP 代理转发 | `test.com proxy://127.0.0.1:PROXY_PORT`           | 请求通过代理服务器转发 |
| 代理认证      | `test.com proxy://user:pass@127.0.0.1:PROXY_PORT` | 代理收到认证信息       |

---

## pac

> ⚠️ **未实现**：完整的 PAC (Proxy Auto-Config) 支持尚未实现。代理**不会**拉取远程 `.pac` 文件，也**不会**执行任何 JavaScript PAC 脚本。

当前 `pac://` 仅识别规则值中字面量的 `PROXY host:port`（或 `DIRECT`）字符串，并从中提取 `host:port` 作为上游；远程 `.pac` URL 和内联 JS PAC 脚本不会被拉取或执行。

### 语法

```
pattern pac://PROXY host:port
```

### 示例

```bash
# 仅支持字面量 PROXY host:port，从中提取上游
* pac://PROXY 127.0.0.1:8080
```

---

## tunnel

`tunnel://` 用于重定向 CONNECT 隧道目标，适合只想改隧道上游地址、不解密 HTTPS 内容的场景。CONNECT 隧道只是其主要用途：`tunnel://` 规则同样会为匹配的明文（非 CONNECT）HTTP 请求改写上游地址，对这类请求的行为与 `host://` 相同。若需要按 HTTPS path 匹配或改写明文内容，应使用 `tlsIntercept://` 让代理看到解密后的 HTTP 请求。

### 语法

```txt
pattern tunnel://target[:port]
```

### 示例

```bash
# 把 CONNECT 隧道转到指定主机
api.example.com tunnel://127.0.0.1:8443
```

---

## 规则组合

路由规则可以与其他规则组合使用：

```bash
# 路由 + 请求头修改
www.example.com host://backend.local reqHeaders://(X-Forwarded-Host:www.example.com)

# 路由 + 过滤器
www.example.com host://backend.local includeFilter://m:GET

# 路由 + 响应修改
www.example.com host://backend.local resCors://*
```

---

## 注意事项

1. **路径保留**：使用 `host` 时，原始请求的路径和查询参数会保留
2. **上游协议**：裸 `host:port` 会按 `host://` 路由，非 443/8443 端口默认使用明文 HTTP；如果目标服务在非标准端口上提供 HTTPS，必须显式写 `https://host:port`
3. **Host 头部**：默认情况下，`Host` 头部会更新为目标主机
4. **HTTPS 处理**：对于 HTTPS 请求，需要安装/信任 Bifrost CA 证书才能进行内容修改
5. **优先级**：当前文档仅覆盖仓库内已实现并稳定支持的路由协议；如需查看历史设计，请以代码支持集为准
