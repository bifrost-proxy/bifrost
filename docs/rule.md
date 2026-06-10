# 规则语法

Bifrost 通过简洁的规则配置来修改请求和响应。

## 语法结构

```txt
pattern operation [operations...] [filters...] [lineProps://...]
```

| 组成部分      | 是否必填 | 描述                                                          |
| :------------ | :------- | :------------------------------------------------------------ |
| **pattern**   | 是       | 匹配请求 URL 的表达式，详见 [pattern](./pattern.md)           |
| **operation** | 是       | 操作指令 `protocol://value`，详见 [operation](./operation.md) |
| **filters**   | 否       | 过滤条件，详见下文                                            |
| **lineProps** | 否       | 规则属性，详见下文                                            |

## Pattern 类型

Pattern 根据格式自动识别类型，优先级影响匹配顺序：

| 类型         | 格式示例                                    | 优先级 |
| :----------- | :------------------------------------------ | :----- |
| Domain       | `example.com` `example.com/api`             | 100+   |
| IP（精确）   | `192.168.1.1`                               | 95     |
| CIDR         | `192.168.0.0/16`                            | 70-78  |
| Regex        | `/pattern/` `/pattern/i`                    | 80     |
| PathWildcard | `^example.com/api/*` `^example.com/api/**`  | 60-70  |
| Wildcard     | `*.example.com` `$example.com` `example?.com` | 40-60  |

Domain 优先级以 100 为基准，显式协议（`http(s)://`）+5、显式端口 +10，因此带协议带端口的 Domain pattern 可达 100-115。CIDR 优先级为 `70 + prefix_len/4`（约 70-78），低于 Regex（80），因此一条宽泛的 CIDR（如 `/16`）可能排在 Regex 之后；只有精确 IP 才是 95。

取反匹配：所有类型均支持 `!` 前缀，如 `!*.example.com`。完整的类型检测顺序、优先级与协议前缀注意事项见 [pattern.md](./pattern.md)。

## 高级配置

### 1. 组合配置

单条规则支持多个操作指令：

```txt
www.example.com file:///static-files cache://3600 resCors://*
chatgpt.com http3://
```

### 2. 位置调换

operation 和 pattern 可调换位置，便于批量配置：

```txt
proxy://127.0.0.1:8080 www.example.com api.example.com
```

### 3. 简写支持

`host:port[/path]` 格式自动识别为 `host://` 协议：

```txt
example.com 127.0.0.1:3000/api
# 等价于
example.com host://127.0.0.1:3000/api
```

当右侧已经出现 pattern，且目标值为不带协议的 `domain[/path]`、`domain:port[/path]`、`localhost[/path]`、IP/IPv6 带路径形式时，也会自动识别为 `host://`，下游协议按实际发起请求自动补偿：

```txt
gamingpop-boe.bifrost.local/manager gamingpop-boe.bifrost.local/manager
# 等价于
gamingpop-boe.bifrost.local/manager host://gamingpop-boe.bifrost.local/manager
```

### 4. 多行配置

**反斜杠续行**：行末 `\` 将下一行合并

```txt
example.com \
host://127.0.0.1 \
reqHeaders://{test=1}
```

**line 块语法**：块内换行自动转空格

```txt
line`
proxy://127.0.0.1:8080
www.example.com
api.example.com
includeFilter://m:GET
excludeFilter:///admin/
`
```

### 5. 过滤器

通过 `includeFilter://` 和 `excludeFilter://` 添加过滤条件：

```txt
example.com host://127.0.0.1 includeFilter://m:GET excludeFilter:///admin/
```

**过滤条件类型**：

| 前缀      | 说明       | 示例                                |
| :-------- | :--------- | :---------------------------------- |
| `m:`      | HTTP 方法  | `m:GET` `m:GET,POST,PUT`            |
| `s:`      | 状态码     | `s:200` `s:200-299` `s:200,404,500` |
| `h:`      | 请求头存在或匹配 | `h:X-Custom-Header` `h:Content-Type=json` |
| `reqH:`   | 请求头匹配 | `reqH:Content-Type=/json/`          |
| `resH:`   | 响应头匹配 | `resH:Content-Type=/json/`          |
| `i:`      | 客户端 IP  | `i:192.168.1.1` `i:192.168.0.0/16`  |
| `/path`   | 路径包含   | `/api`                              |
| `/regex/` | 路径正则   | `/^\/api\/v\d+/`                    |
| `domain.com/path` | URL host/path | `api.example.com/v1` |

> `b:` / `B:` body 过滤器当前只被 parser 接受，运行时 resolver 尚未读取 body 做过滤，其匹配结果恒为「不命中」：写在 `includeFilter://b:...` 里会让规则**永远不命中**，写在 `excludeFilter://b:...` 里则**永远不排除**（即等于无效）。请用 `bifrost search --req-body/--res-body` 做内容筛选，不要把 body 过滤写成生产规则依赖。

### 6. 规则属性

通过 `lineProps://` 设置规则属性：

| 属性        | 说明                 |
| :---------- | :------------------- |
| `important` | 提升优先级（+10000） |
| `disabled`  | 禁用规则             |

```txt
example.com host://127.0.0.1 lineProps://important
example.com host://127.0.0.1 lineProps://important,disabled
```

### 7. 变量替换

使用 `{varName}` 引用预定义变量，支持嵌套展开（最多 10 次迭代）：

```txt
example.com host://{myHost}
example.com resBody://{mockBody}
```

`${varName}` 格式为模板变量，不会被预处理展开。

## 注意事项

### 规则优先级

1. `lineProps://important` 规则优先匹配
2. 规则按数值优先级 `priority()` 排序：Domain（100+）> 精确 IP（95）> Regex（80）> PathWildcard（60-70）> Wildcard（40-60）；CIDR 为 70-78，因此宽泛 CIDR 可能排在 Regex 之后。不同 Pattern 类型的优先级互不相同，不存在「相同优先级按类型」的二级比较。
3. 仅当优先级相同（即同类型）时，规则才按从上到下的文件顺序匹配

### 调试技巧

1. **逐步验证**：从简单规则开始，逐步添加复杂条件
2. **日志查看**：使用 Bifrost Network 界面的 Overview 面板查看规则匹配情况
3. **临时禁用**：使用 `#` 注释或 `lineProps://disabled` 暂时禁用规则

### HTTPS 自动 TLS 解包边界

当全局 TLS 拦截关闭时，Bifrost 仍会为了执行必须读取或修改 HTTPS 内层 HTTP 内容的规则而自动开启 TLS 解包，例如 `reqHeaders`、`resHeaders`、body 修改、脚本、mock 或状态码类规则。但这个自动解包有明确边界：

- 仅用于 `host://` 改目标地址的规则不会自动开启 TLS 解包。
- 仅用于 `proxy://` 选择下游代理的规则不会自动开启 TLS 解包；HTTPS `CONNECT` 会保持隧道透传并转发给下游代理。
- 规则驱动的自动解包必须有明确 host 作用域：Domain、IP/CIDR、带具体域名或 IP 片段的 Wildcard/PathWildcard 可以触发。
- 纯 regex 或纯 wildcard 范围过大，不能单独触发自动 TLS 解包，例如 `* resHeaders://...`、`*/api/* resHeaders://...`、`/api\/v\d+/ resHeaders://...`。
- 如果确实需要让宽泛匹配规则处理 HTTPS 明文，请先把 pattern 收窄到明确域名/IP，或显式配置 `tlsIntercept://` / 全局 TLS include。

示例：

```txt
# 不会自动解包，只改 CONNECT/SOCKS5 上游目标
example.com host://127.0.0.1:3000

# 不会自动解包，只把隧道交给下游代理
example.com proxy://127.0.0.1:8080

# 会自动解包，因为响应修改规则绑定到明确域名
api.example.com resHeaders://X-Debug=1

# 不会自动解包，因为 pattern 没有明确 host 作用域
* resHeaders://X-Debug=1
/api\/v\d+/ resHeaders://X-Debug=1
```

### 上游 HTTP/3 规则

`http3://` 用于为命中的请求启用“代理到目标服务”的上游 HTTP/3 尝试，默认关闭。

```txt
chatgpt.com http3://
api.example.com h3://
```

- `h3://` 是 `http3://` 的别名
- 仅在代理自己能够读取 HTTP 请求时生效
- 对普通绝对 URI 代理请求可直接生效
- 对浏览器常见的 HTTPS `CONNECT` 流量，通常需要启用 TLS interception 后，代理才能在解密后的上游转发阶段尝试 H3
- 纯 `CONNECT` 透传隧道不会把上游 TCP 连接自动切换成 QUIC/H3

### 上游不安全 HTTPS 证书规则

`upstreamUnsafeSsl://true` 仅对命中的规则允许 Bifrost 到上游 HTTPS 服务时跳过证书校验。它用于某个测试环境、内网服务或自签名上游，不需要在启动整个代理时使用全局 `--unsafe-ssl`。

```txt
internal-api.example.test https://10.37.102.138:8080 upstreamUnsafeSsl://true
```

- 该规则只影响代理到上游的 HTTPS 连接，不会改变客户端到 Bifrost 的 TLS 信任关系。
- 没有命中该规则的请求仍按默认安全证书校验执行。
- 如果上游证书不可信且没有配置该规则，默认错误响应 body 会提示在匹配规则中追加 `upstreamUnsafeSsl://true`。
- 如果目标上游证书可信，应不要使用该规则；它是针对单个连接/规则的显式例外。

## 扩展阅读

- [规则协议手册](./rules/README.md)：按协议查看各能力说明与示例
