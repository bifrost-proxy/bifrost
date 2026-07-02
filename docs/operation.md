# 操作符/operation

在 Bifrost 中，每条规则由 **匹配模式（`pattern`）** 和 **操作（`operation`）** 两部分组成，操作的通用语法为：

```txt
protocol://[value]
```

- **protocol**：指定操作类型（如 `file`、`proxy`、`resHeaders` 等）
- **value**：操作内容（支持多种格式，见下文）

## Value 类型

Bifrost 会根据 value 的格式自动识别其类型，支持以下 6 种：

| 类型        | 格式            | 示例                            | 说明                     |
| ----------- | --------------- | ------------------------------- | ------------------------ |
| 内联值      | 普通字符串      | `127.0.0.1:8080`                | 直接作为操作内容         |
| 内联参数    | `key=value&...` | `q=keyword&page=1`              | 解析为键值对（仅 `urlParams`/`params` 按 `&` 拆分） |
| 小括号内容  | `(content)`     | `({"ec": 0})`                   | 括号内容直接作为操作内容 |
| Values 引用 | `{key}`         | `{config.json}`                 | 引用 Values 中的内容     |
| 本地文件    | `/path/to/file` | `/Users/you/mock.json`          | 从本地文件加载内容（必须以 `/` 开头的绝对路径） |
| 远程资源    | `http(s)://url` | `https://example.com/data.json` | 从远程 URL 加载内容      |

> ⚠️ **重要**：普通内联值和内联参数仍然不能直接包含空格，因为规则解析器使用空格分隔多个操作符。小括号内容会作为一个整体解析，可以包含空格。多行内容优先使用规则文件内嵌值；只有特别大的 JSON/HTML/JS/CSS、PAC 脚本，或需要被很多规则长期共享的内容，才建议使用全局 **Values 引用**、**本地文件** 或 **远程资源**。

### 识别规则

Value 按以下优先级进行识别：

1. 以 `http://` 或 `https://` 开头 → **远程资源**
2. 以 `/` 开头（非 `//`）→ **本地文件**
3. 以 `(` 开头且以 `)` 结尾 → **小括号内容**
4. 以 `{` 开头且以 `}` 结尾 → **Values 引用**
5. 包含 `=` 且不含 `/` 和 `{` → **内联参数**
6. 其他情况 → **内联值**

## 内联值

直接将 value 作为操作内容：

```txt
pattern reqHeaders://x-proxy=Bifrost       # 设置单个请求头
pattern statusCode://404                   # 修改状态码
pattern host://127.0.0.1:8080              # 转发到指定地址
```

> ⚠️ **值不能包含空格**，空格会被解析器识别为操作符分隔符，导致规则解析错误。

## 内联参数

当 value 符合 `key=value&key2=value2` 格式时，会解析为键值对：

```txt
pattern urlParams://q=keyword&page=1
pattern params://foo=1&bar=2
```

解析规则：

- 以 `&` 分隔多个键值对
- 以 `=` 分隔键和值
- 键为空时忽略该对
- 值可以为空（如 `flag=`）

> ⚠️ **重要**：`&` 拆分仅对 `urlParams`、`params` 这类查询参数协议生效。对 `reqHeaders`、`reqCookies`、`resCookies` 而言，`&` **不是分隔符**：`reqHeaders://X-Custom=test&X-Another=value` 会设置单个请求头 `x-custom: test&X-Another=value`，`reqCookies://session=abc123&user=test` 会设置单个 cookie 值 `session=abc123&user=test`。要设置多个 header/cookie，请使用行格式（每行一个 `Header: value`，例如通过内嵌值块引用）或合法 JSON 对象：

````txt
``` headers.txt
X-Custom: test
X-Another: value
```
pattern reqHeaders://{headers.txt}
````

或使用不含未包裹空格的 JSON 对象：

```txt
pattern reqHeaders://{"X-Custom":"test","X-Another":"value"}
```

> ⚠️ **值不能包含空格**，空格会被解析器识别为操作符分隔符，导致规则解析错误。

## 小括号内容

当操作内容需要包含特殊字符（如 `/`、`{`）时，使用小括号包裹可避免被误识别：

```txt
pattern file://({"ec": 0, "data": null})    # JSON 作为响应内容
pattern resBody://(/User/xxx/yyy.txt)      # 将路径字符串作为响应内容
```

> 注意：`resBody:///User/xxx/yyy.txt` 会从文件加载内容，而 `resBody://(/User/xxx/yyy.txt)` 会将 `/User/xxx/yyy.txt` 字符串直接作为响应内容

> 小括号内部可以包含空格，括号外的空格仍用于分隔多个 operation。需要换行或大段内容时，优先使用 **Values 引用**。

## Values 引用

通过 `{key}` 格式引用值内容。`{key}` 实际解析的是**当前规则文件中定义的内嵌值块**（见下文“内嵌值”），并且要用产生响应内容的协议（如 `resBody`）承载：

````txt
``` mockResponse.json
{"code":0}
```
pattern resBody://{mockResponse.json}      # 引用本规则文件内嵌的 mockResponse.json 值块
````

> ℹ️ 经实测（bifrost 0.0.96，真实 `bifrost start` 路径）：通过 `bifrost value add` 存储的全局 Values 会被规则里的 `{key}` 正常解析（例如 `bifrost value add myval ...` 后，`resBody://{myval}` 输出该 value 内容）；规则文件中定义的内嵌值块同样用 `{key}` 引用。注意 `file://`/`tpl://` 的 value 会被当作文件路径处理，所以要把某个 value 作为响应内容回放时，用 `resBody://{key}` 这类内容承载协议，而不是 `file://{key}`。

### Values 存储机制

Values 以文件形式存储在 Bifrost 数据目录的 `values/` 子目录中，每个 key 对应一个 `.txt` 文件。

日常规则不要默认创建全局 Values。短值直接写内联 value；较长但只属于当前规则文件的内容，优先使用下面的“内嵌值”。全局 Values 主要适合特别大的内容，或需要被很多规则长期共享的内容。

**创建/编辑 Values：**

1. 在 Bifrost 界面的 Values 模块中创建或编辑
2. 在规则中通过 `{key}` 引用

### 内嵌值

除了在 Values 模块中预先创建，也可以在规则文件中直接定义内嵌值：

````txt
``` ua.txt
Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X)
```
pattern ua://{ua.txt}
````

内嵌值的 key 需要包含文件扩展名（如 `.txt`、`.json`），以便与普通变量区分。

## 文件/远程资源

从本地文件或远程 URL 加载操作内容：

```txt
pattern reqHeaders:///Users/you/headers.txt           # 从本地文件加载（绝对路径，以 / 开头）
pattern resHeaders://https://example.com/config.json  # 从远程 URL 加载
```

> ⚠️ 注意：部分协议（如 `http`、`https`、`ws`、`wss`、`host`、`cache` 等）禁止通过文件路径或远程 URL 获取内容，详见各协议文档。

## 模板字符串

Bifrost 支持类似 ES6 的模板字符串功能，在 value 中动态引用请求信息。`${var}` 会在任意 value 中无条件展开，不需要、也不依赖反引号 `` ` `` 触发；反引号不是特殊语法，会原样保留在输出中（如 `resReplace://echo=`${hostname}`` 的输出仍带反引号 `` `tp2.test` ``）。下文示例中的反引号仅用于视觉包裹，可省略。

### 基本语法

```txt
pattern protocol://`...${variable}...`
```

### 支持的变量

#### 基础信息

| 变量                  | 说明                    |
| --------------------- | ----------------------- |
| `${now}`              | 当前时间戳（毫秒）      |
| `${random}`           | 0-1 之间的随机小数      |
| `${randomUUID}`       | 随机 UUID               |
| `${randomInt(n)}`     | 0 到 n 之间的随机整数   |
| `${randomInt(n1-n2)}` | n1 到 n2 之间的随机整数 |
| `${version}`          | Bifrost 版本号          |
| `${id}` / `${reqId}`  | 请求唯一标识符          |

#### URL 相关

| 变量                                 | 说明                     |
| ------------------------------------ | ------------------------ |
| `${url}`                             | 完整请求 URL             |
| `${host}`                            | 主机名（可能含端口）     |
| `${hostname}`                        | 主机名（不含端口）       |
| `${port}`                            | 端口号                   |
| `${path}`                            | 路径（含查询字符串）     |
| `${pathname}`                        | 路径（不含查询字符串）   |
| `${search}`                          | 查询字符串（含 `?`）     |
| `${query}`                           | 查询字符串（不含 `?`）   |
| `${query.xxx}`                       | 查询参数 xxx 的值        |
| `${queryString}` / `${searchString}` | 查询字符串，空时返回 `?` |

#### 网络信息

| 变量                          | 说明       |
| ----------------------------- | ---------- |
| `${method}`                   | 请求方法   |
| `${clientIp}` / `${ip}`       | 客户端 IP  |
| `${clientPort}`               | 客户端端口 |
| `${serverIp}`                 | 服务端 IP  |
| `${serverPort}`               | 服务端端口 |
| `${remoteAddress}`            | 远程地址   |
| `${remotePort}`               | 远程端口   |
| `${statusCode}` / `${status}` | 响应状态码（见下方限制说明） |

> ⚠️ **实测限制（bifrost 0.0.96）**：`${statusCode}` / `${status}` 在所有响应阶段协议中都展开为**空字符串**——`resBody://PRE_${statusCode}_POST` 输出 `PRE__POST`，`resReplace`、`resHeaders://x-probe=${statusCode}` 注入也都为空。该变量当前在规则模板上下文中拿不到响应状态码，请勿在规则模板里依赖它。

#### Headers 和 Cookies

| 变量                                                         | 说明                 |
| ------------------------------------------------------------ | -------------------- |
| `${reqHeaders.xxx}` / `${reqHeader.xxx}` / `${reqH.xxx}`     | 请求头字段 xxx 的值  |
| `${resHeaders.xxx}` / `${resHeader.xxx}` / `${resH.xxx}`     | 响应头字段 xxx 的值  |
| `${reqCookies.xxx}` / `${reqCookie.xxx}`                     | 请求 cookie xxx 的值 |
| `${resCookies.xxx}` / `${resCookie.xxx}`                     | 响应 cookie xxx 的值 |

> 不带 `.xxx` 属性时返回所有 headers/cookies 的完整字符串。

> ⚠️ **实测限制（bifrost 0.0.96）**：只有**请求阶段**的 `${reqHeaders.xxx}` / `${reqCookies.xxx}` 能在规则模板里取到值（如 `resReplace://echo=UA[${reqHeaders.user-agent}]` 输出 `UA[curl/8.7.1]`）。**响应阶段**的 `${resHeaders.xxx}` / `${resCookies.xxx}` 当前展开为**空字符串**——即使响应头/Cookie 确实在链路上存在：上游返回 `X-Upstream: mock-18181` 时 `${resHeaders.x-upstream}` 仍为空（`RH[]`）；规则自己 `resCookies://mycookie=COOKVAL` 设置了 `Set-Cookie` 后 `${resCookies.mycookie}` 仍为空（`RC[]`）。请勿在规则模板里依赖响应头/响应 Cookie 变量。

#### 客户端标识

| 变量              | 说明                                |
| ----------------- | ----------------------------------- |
| `${clientId}`     | 客户端标识符                        |
| `${localClientId}`| 本地客户端标识符                    |

#### 其他

| 变量          | 说明                  |
| ------------- | --------------------- |
| `${env.xxx}`  | 环境变量 xxx 的值     |
| `${realHost}` | Bifrost 监听的网卡 IP（见下方限制说明） |
| `${realPort}` | Bifrost 端口号（见下方限制说明）        |
| `${realUrl}`  | 实际请求 URL（见下方限制说明）          |

> ⚠️ **实测限制（bifrost 0.0.96）**：`${realHost}` / `${realPort}` / `${realUrl}` 在规则模板上下文中都展开为**空字符串**——`resBody://PRE_${realHost}_POST` / `…${realPort}…` / `…${realUrl}…` 均输出 `PRE__POST`，`resHeaders` 注入同样为空。作为对照，同上下文的 `${host}` 正常输出（`PRE_vars.test_POST`）。这三个变量当前拿不到 Bifrost 监听信息，请勿在规则模板里依赖它们。

### 高级用法

#### URL 编码

使用双花括号语法对变量值进行 URL 编码：

```txt
pattern redirect://`https://example.com?url=${{url}}`
```

#### 转义

使用 `$${}` 阻止变量展开（注意要用产生响应内容的协议演示，`file://` 会把 value 当作文件路径加载，得到 404 而非字面量响应体）：

```txt
pattern resBody://`$${host}`   # 响应体为字面量 ${host}
```

#### 字符串替换

支持对变量值进行替换操作：

```txt
# 简单替换
${hostname.replace(example,test)}     # example.com → test.com

# 正则替换（首次匹配）
${hostname.replace(/\./,-)}           # example.com → example-com

# 正则全局替换
${hostname.replace(/\./g,-)}          # a.b.c.d → a-b-c-d

# 忽略大小写
${hostname.replace(/ABC/i,xyz)}
```

### 模板与 Values 结合

模板字符串可以与 Values 引用结合使用：

````txt
``` response.json
{"host":"${hostname}","path":"${path}","time":${now}}
```
www.test.com resBody://{response.json}
````

访问 `http://www.test.com/api/users` 时返回（已实测渲染，bifrost 0.0.96）：

```json
{"host":"www.test.com","path":"/api/users","time":1781093526662}
```

> ⚠️ 注意：此处必须用 `resBody://{response.json}` 承载内嵌值块；`file://`{response.json}`` 与 `tpl://{response.json}` 都会把 `{response.json}` 当作磁盘文件名查找，分别报 `File not found` / `Template file not found`，无法渲染内嵌值块。

### 捕获变量

在正则匹配模式或通配符模式中，可以使用 `$1`、`$2` 等引用捕获组：

```txt
# 正则捕获
/api\/v(\d+)\/(.*)/ redirect://`https://api-v$1.example.com/$2`

# 通配符捕获
*.example.com host://$1.backend.local

# 路径通配符捕获
^example.com/*/action/* redirect://`https://new.example.com/$1/do/$2`
```

## 数据对象格式

部分协议的 value 需要是键值对数据，Bifrost 支持以下格式：

> ⚠️ **重要**：JSON 对象格式可用于 body 类协议（如 `resMerge`）以及扁平 header/cookie 类协议（如 `reqHeaders`、`resHeaders`、`reqCookies`）。header/cookie JSON 只接受 string/number/bool/null 这类标量值；数组或嵌套对象不会作为 header/cookie 值。`resCookies` 额外支持对象值表示 Cookie 属性。

### JSON 格式

```json
{
  "key1": "value1",
  "key2": "value2"
}
```

规则内联写法示例：

```txt
api.example.com reqHeaders://{"X-Env":"ppe","X-Flag":"1"}
api.example.com resHeaders://({"Cache-Control":"max-age=3600, public"})
```

### 行格式

```txt
key1: value1
key2: value2
key3:value3
```

解析规则：

- 优先以 `: `（冒号+空格）分隔
- 没有冒号+空格时，以第一个冒号分隔
- 不包含分隔符的行会被跳过（不产生键值对）

### 内联参数格式

```txt
key1=value1&key2=value2&keyN=valueN
```

> 建议对 key 和 value 进行 `encodeURIComponent` 编码

## 协议别名

部分协议支持别名，以下是完整的别名映射：

| 别名          | 实际协议      |
| ------------- | ------------- |
| `hosts`       | `host`        |
| `ignore`      | `passthrough` |
| `status`      | `statusCode`  |
| `download`    | `attachment`  |
| `html`        | `htmlAppend`  |
| `js`          | `jsAppend`    |
| `css`         | `cssAppend`   |
| `http-proxy`  | `proxy`       |
| `h3`          | `http3`       |
| `pathReplace` | `urlReplace`  |
| `reqMerge`    | `params`      |

## 操作协议

每个协议对应一种特定的操作类型，用于对匹配的请求进行相应处理。协议分为以下几类：

### 控制类

| 协议 | 说明 |
| ---- | ---- |
| `tlsIntercept` | 启用 TLS 拦截 |
| `tlsPassthrough` | 禁用 TLS 拦截（直接透传） |
| `tlsOptions` | 配置 CONNECT 上游 TLS 选项 |
| `upstreamUnsafeSsl` | 仅对命中规则的上游 HTTPS 连接跳过证书校验 |
| `sniCallback` | 配置 SNI 回调元数据（CONNECT 请求） |
| `devtools` | 为命中的代理页面开启显式 DevTools 控制授权 |
| `breakpoint` | 为命中流量授权 Breakpoint request/response 暂停阶段 |
| `passthrough` | 直连透传，不做任何修改 |
| `tunnel` | 重定向 CONNECT 隧道目标 |
| `delete` | 删除/阻断请求（见下方实测限制） |
| `skip` | 跳过已匹配的规则，继续后续匹配 |

> ⚠️ **实测限制（bifrost 0.0.96）**：未能复现 `delete://` 真正阻断/删除请求的行为。`example.com delete://`、`example.com delete://true` 都返回 **HTTP 200** 并带完整上游响应（与对照的 `example.com passthrough://` 一致，均为 200），请求并未被阻断；`m.test delete://`（host 不可解析）返回 502，但与同样 502 的 `m.test passthrough://` 无法区分，仅是 DNS 失败而非阻断。当前无法给出可工作的阻断示例。如需稳定返回固定状态码/直接短路响应，请改用 `statusCode://`（实测 `example.com statusCode://403` 返回 403）。

### 请求修改类

| 协议 | 说明 |
| ---- | ---- |
| `reqHeaders` | 修改请求头 |
| `reqBody` | 设置请求体 |
| `reqPrepend` | 在请求体前追加内容 |
| `reqAppend` | 在请求体后追加内容 |
| `reqCookies` | 设置请求 Cookie |
| `reqCors` | 添加 CORS 请求头 |
| `reqDelay` | 延迟请求（毫秒） |
| `reqSpeed` | 限制请求速率（KB/s，输入值 × 1024 为实际字节限速） |
| `reqType` | 设置请求 Content-Type |
| `reqCharset` | 设置请求字符集 |
| `reqReplace` | 替换请求体内容 |
| `forwardedFor` | 设置 X-Forwarded-For 请求头 |
| `method` | 修改请求方法 |
| `auth` | 设置 Authorization 请求头 |
| `ua` | 设置 User-Agent 请求头 |
| `referer` | 设置 Referer 请求头 |
| `urlParams` | 添加/修改 URL 查询参数 |
| `params` | 合并参数 |
| `dns` | 自定义 DNS 解析 |
| `http3` | 启用上游 HTTP/3 尝试 |
| `reqScript` | 执行请求脚本 |

### 响应修改类

| 协议 | 说明 |
| ---- | ---- |
| `resHeaders` | 修改响应头 |
| `resBody` | 设置响应体 |
| `resPrepend` | 在响应体前追加内容 |
| `resAppend` | 在响应体后追加内容 |
| `resCookies` | 设置响应 Cookie |
| `resCors` | 添加 CORS 响应头 |
| `resDelay` | 延迟响应（毫秒） |
| `resSpeed` | 限制响应速率（KB/s，输入值 × 1024 为实际字节限速） |
| `resType` | 设置响应 Content-Type |
| `resCharset` | 设置响应字符集 |
| `resReplace` | 替换响应体内容 |
| `replaceStatus` | 替换响应状态码（请求完成后替换） |
| `statusCode` | 直接返回指定状态码 |
| `cache` | 设置缓存控制（秒） |
| `attachment` | 设置 Content-Disposition 为下载 |
| `responseFor` | 设置 x-bifrost-response-for 响应头 |
| `trailers` | 设置响应 trailers |
| `resMerge` | 合并 JSON 到响应体 |
| `headerReplace` | 替换 header 内容（请求/响应均可，用 `req.`/`res.` 前缀选边，如 `headerReplace://req.header-name:old=new`） |
| `resScript` | 执行响应脚本 |

### 路由类

| 协议 | 说明 |
| ---- | ---- |
| `host` | 转发请求到指定 host |
| `xhost` | 扩展 host 转发（支持路径重写） |
| `http` | HTTP 协议转发 |
| `https` | HTTPS 协议转发 |
| `ws` | WebSocket 转发 |
| `wss` | 安全 WebSocket 转发 |
| `proxy` | HTTP 代理转发 |
| `pac` | PAC 脚本路由（支持内嵌/Values/本地文件/远程 URL） |
| `redirect` | URL 重定向（301/302） |
| `file` | 返回文件内容作为响应 |
| `tpl` | 模板响应（支持变量替换） |
| `rawfile` | 返回原始文件内容 |

### URL 处理类

| 协议 | 说明 |
| ---- | ---- |
| `urlReplace` | 替换 URL 路径 |

### 内容注入类

| 协议 | 说明 |
| ---- | ---- |
| `htmlAppend` | 在 HTML 元素内部后部追加内容（优先插入 `</html>` 前） |
| `htmlPrepend` | 在 HTML 元素内部前部追加内容（优先插入 `<html>` 后） |
| `htmlBody` | 替换 HTML body |
| `jsAppend` | 在 JavaScript 尾部追加内容 |
| `jsPrepend` | 在 JavaScript 头部追加内容 |
| `jsBody` | 替换 JavaScript body |
| `cssAppend` | 在 CSS 尾部追加内容 |
| `cssPrepend` | 在 CSS 头部追加内容 |
| `cssBody` | 替换 CSS body |

### 脚本/解码类

| 协议 | 说明 |
| ---- | ---- |
| `reqScript` | 执行请求阶段脚本（同时列于请求修改类） |
| `resScript` | 执行响应阶段脚本（同时列于响应修改类） |
| `decode` | 执行解码脚本（请求/响应解码） |
| `bp` | 绑定二进制协议 parser 引用，通常与 `decode://bp` 配合；只影响 Traffic 落库、详情展示与搜索，不改写真实转发流量 |

### `devtools`

`devtools://` 是显式授权控制规则，用于让命中的代理页面进入 DevTools 控制面。它不是页面发现机制；页面仍需要先经过 Bifrost 代理，再由规则授权控制资格。

```txt
https://example.com/ devtools://
```

该能力应只在明确需要远程控制或调试代理页面时开启。

### `breakpoint`

`breakpoint://request` / `breakpoint://response` 是 Breakpoint 的规则级授权。Toolbar 中的 Breakpoint Switch 只是全局门禁；只有全局开关开启且请求命中了包含 `breakpoint` 协议的规则，对应阶段才会暂停。

```txt
127.0.0.1:18080 breakpoint://request
127.0.0.1:18080 breakpoint://response
127.0.0.1:18080 breakpoint://request,response
```

说明：

- `request` / `req`：在请求发往 upstream 前暂停。
- `response` / `res`：在响应返回 client 前暂停。
- `request,response`、`req,res`、`both` 或 `all`：同一条命中流量先暂停 request，resume 后再暂停 response。
- 空 value 或未知 value 不触发暂停。
- 仅打开 Toolbar 开关不会暂停任何流量；关闭 Toolbar 开关会阻止新暂停并释放 pending breakpoint。

### `http3`

启用命中请求的上游 HTTP/3 尝试。该协议没有 value，写法如下：

```txt
chatgpt.com http3://
api.example.com h3://
```

说明：

- 默认不会主动尝试上游 H3，只有命中 `http3://` 或 `h3://` 规则才会启用
- 该能力控制的是"代理到目标服务"的上游协议选择，不是开启下游 UDP/QUIC 监听
- 仅对 HTTPS 上游请求生效
- 如果目标不支持 H3 或协商失败，会回退到现有的 HTTP/1.1 或 HTTP/2 转发链路

### `upstreamUnsafeSsl`

允许命中规则的上游 HTTPS 连接跳过证书校验。该协议用于内网、自签名或测试环境 HTTPS 上游，避免为了单个目标在启动整个 Bifrost 时使用全局 `--unsafe-ssl`。

```txt
internal-api.example.test https://10.37.102.138:8080 upstreamUnsafeSsl://true
```

说明：

- `upstreamUnsafeSsl://true` 只影响 Bifrost 连接上游 HTTPS 服务时的证书校验，不改变客户端到 Bifrost 的 TLS 信任关系
- 该能力按规则生效；未命中的其他请求仍执行默认安全证书校验
- value 可写为 `true` / `1` / `yes` / `on` 启用，也可写为 `false` / `0` / `no` / `off` 显式关闭；裸 `upstreamUnsafeSsl://` 等价于启用
- 该协议可以与 `https://`、`host://`、`tunnel://`、`includeFilter` / `excludeFilter` 等规则组合使用
- 如果上游 TLS 握手因自签名或不可信证书失败，默认错误响应会提示给匹配规则追加 `upstreamUnsafeSsl://true`
