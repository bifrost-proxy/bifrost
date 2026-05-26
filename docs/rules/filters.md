# 过滤器规则

本章介绍控制规则生效条件的过滤器。

---

## includeFilter

包含过滤器，只有满足条件的请求才会应用规则。

### 语法

```
pattern rules... includeFilter://condition
```

### 过滤条件

| 条件类型 | 语法 | 说明 |
| --- | --- | --- |
| 方法 | `m:METHOD` / `m:GET,POST` | 匹配请求方法 |
| 状态码 | `s:CODE` / `s:200-299` / `s:200,404,500` | 匹配响应状态码；请求阶段没有状态码时不会命中 |
| 请求头存在 | `h:name` | 判断请求头是否存在 |
| 请求头匹配 | `h:name=value` / `reqH:name=/regex/` | `h:` 和 `reqH:` 都匹配请求头；`value` 可为普通文本或 `/regex/` |
| 响应头匹配 | `resH:name=value` / `resH:name=/regex/` | 匹配响应头；仅响应阶段有意义 |
| 客户端 IP | `i:ip` / `i:cidr` | 匹配客户端 IP 或 CIDR |
| 路径前缀 | `/path` | 以 `/` 开头但不以 `/` 结尾时，按普通前缀匹配：`/account` 匹配 `/account`、`/account/...`、`/account-center` 和带 query 的路径 |
| 路径正则 | `/regex/` | 以 `/` 开头并以 `/` 结尾时，按正则匹配路径 |
| URL host/path | `example.com` / `example.com/api` | 包含 `.` 的过滤值会按 host 与可选 path 匹配 |
| URL 通配符 | `*/api` / `*/alice/*` | 兼容 Whistle 风格的 URL wildcard filter；`*/api` 匹配 `/api` 及其子路径，`*/alice/*` 匹配 URL 中包含 `/alice/` 的请求 |

> 当前实现会解析 `b:` / `B:` body 过滤器，但运行时 resolver 尚未读取请求/响应 body 参与过滤，匹配结果恒为不命中。不要把 body 过滤作为可用能力依赖；需要按内容筛选请使用 `bifrost search --req-body/--res-body` 查看流量证据。

### 方法过滤

```bash
# 只对 POST 请求生效
www.example.com resHeaders://(X-Method:POST) includeFilter://m:POST

# 只对 GET 请求生效
www.example.com resHeaders://(X-Method:GET) includeFilter://m:GET

# 只对 PUT 请求生效
www.example.com resHeaders://(X-Method:PUT) includeFilter://m:PUT
```

### 状态码过滤

> ⚠️ **注意**：小括号内容会作为一个整体解析，可以包含空格；多行响应内容建议使用块变量。

```bash
# 只对 500 响应生效
www.example.com replaceStatus://200 includeFilter://s:500

# 只对 404 响应生效（使用块变量）
www.example.com resBody://{not-found} includeFilter://s:404

# 对 4xx 响应生效
www.example.com resHeaders://(X-Error: true) includeFilter://s:400-499
```

块变量定义：

````
``` not-found
Not Found
```
````

### 请求头过滤

```bash
# 匹配带有特定头的请求
www.example.com host://debug.local includeFilter://h:X-Debug

# 匹配头部值
www.example.com host://admin.local includeFilter://h:X-Role=admin

# 匹配 Content-Type
www.example.com resHeaders://(X-Json: true) includeFilter://h:content-type=application/json
```

### 响应头过滤

```bash
# 匹配响应头
www.example.com resBody://(cached) includeFilter://resH:X-Cache=HIT
```

> 注：`(cached)` 无空格，可使用行内值

### 路径过滤

```bash
# 匹配特定路径
www.example.com resHeaders://(X-Api: true) includeFilter:///api

# 匹配路径前缀，/account 会匹配 /account-center；需要边界时使用路径正则
www.example.com host://account.local excludeFilter:///account

# 匹配路径模式（正则）
www.example.com resDelay://1000 includeFilter:///slow

# 兼容 Whistle 风格 URL 通配符过滤，排除 /static、/api 及包含 /alice/ 的 URL
www.example.com http://localhost:5173 excludeFilter://*/static excludeFilter://*/api excludeFilter://*/alice/*
```

### 多条件组合

```bash
# AND 条件（同时满足）
www.example.com replaceStatus://200 includeFilter://m:POST includeFilter://s:500

# 方法 + 头部
www.example.com host://special.local includeFilter://m:POST includeFilter://h:X-Special
```

### 测试用例

| 测试场景   | 规则                                                   | 请求     | 预期         |
| ---------- | ------------------------------------------------------ | -------- | ------------ |
| POST 方法  | `test.com resHeaders://(X: 1) includeFilter://m:POST`  | POST     | 应用规则     |
| POST 方法  | `test.com resHeaders://(X: 1) includeFilter://m:POST`  | GET      | 不应用       |
| 状态码 500 | `test.com replaceStatus://200 includeFilter://s:500`   | 返回 500 | 状态码变 200 |
| 状态码 500 | `test.com replaceStatus://200 includeFilter://s:500`   | 返回 200 | 不变         |
| 头部匹配   | `test.com host://debug includeFilter://h:X-Debug=true` | 有头部   | 应用规则     |

---

## excludeFilter

排除过滤器，满足条件的请求不会应用规则。

### 语法

```
pattern rules... excludeFilter://condition
```

### 过滤条件

与 `includeFilter` 相同的条件语法。

### 示例

```bash
# 排除 GET 请求
www.example.com resDelay://1000 excludeFilter://m:GET

# 排除静态资源
www.example.com resHeaders://(X-Dynamic: true) excludeFilter:///\.js$/ excludeFilter:///\.css$/

# 排除成功响应
www.example.com resHeaders://(X-Error: true) excludeFilter://s:200

# 排除特定头部
www.example.com host://default.local excludeFilter://h:X-Special
```

### 测试用例

| 测试场景 | 规则                                                | 请求 | 预期     |
| -------- | --------------------------------------------------- | ---- | -------- |
| 排除 GET | `test.com resHeaders://(X: 1) excludeFilter://m:GET` | GET  | 不应用   |
| 排除 GET | `test.com resHeaders://(X: 1) excludeFilter://m:GET` | POST | 应用规则 |

---

## passthrough

`passthrough://` 用于忽略后续规则并直接透传请求。旧的 `ignore://` 写法会在导入、同步或保存时自动转换为 `passthrough://`。

### 语法

```txt
pattern passthrough://
```

### 示例

```bash
# 透传特定域名
internal.example.com passthrough://

# 透传健康检查路径
*.local/health passthrough://
*.local/metrics passthrough://
```

### 测试用例

| 测试场景 | 规则 | 请求 | 预期 |
| --- | --- | --- | --- |
| 透传域名 | `ignore-this.local passthrough://` | `ignore-this.local` | 请求直接透传 |
| 透传路径 | `*.local/health passthrough://` | `/health` | 请求直接透传 |

---

## delete

删除请求/响应中的头部、Cookie 或 URL 参数。

### 语法

```
pattern delete://target.name
```

### 目标类型

| 目标        | 语法              | 说明                |
| ----------- | ----------------- | ------------------- |
| 请求头      | `reqHeaders.name` | 删除请求头          |
| 响应头      | `resHeaders.name` | 删除响应头          |
| 请求 Cookie | `reqCookies.name` | 删除请求 Cookie     |
| 响应 Cookie | `resCookies.name` | 删除响应 Set-Cookie |
| URL 参数    | `urlParams.name`  | 删除 URL 参数       |

### 示例

```bash
# 删除请求头
www.example.com delete://reqHeaders.X-Custom

# 删除响应头
www.example.com delete://resHeaders.X-Powered-By

# 删除请求 Cookie
www.example.com delete://reqCookies.session

# 删除响应 Cookie
www.example.com delete://resCookies.tracking

# 删除 URL 参数
www.example.com delete://urlParams.debug
```

### 多个删除

```bash
# 删除多个头部
www.example.com delete://reqHeaders.X-A delete://reqHeaders.X-B

# 删除多个 Cookie
www.example.com delete://reqCookies.a delete://reqCookies.b
```

### 测试用例

| 测试场景    | 规则                                    | 预期                |
| ----------- | --------------------------------------- | ------------------- |
| 删除请求头  | `test.com delete://reqHeaders.X-Custom` | 请求不含 X-Custom   |
| 删除响应头  | `test.com delete://resHeaders.Server`   | 响应不含 Server     |
| 删除 Cookie | `test.com delete://reqCookies.session`  | Cookie 不含 session |
| 删除参数    | `test.com delete://urlParams.debug`     | URL 不含 debug 参数 |

---

## skip

跳过指定的已命中规则，并继续尝试匹配剩余规则。

### 语法

```txt
pattern skip://pattern=patternString
pattern skip://operation=protocol://value
```

### 示例

```bash
# 跳过更具体的 pattern，回落到父级规则
www.example.com/api/blocked skip://pattern=www.example.com/api/blocked

# 跳过某条已经命中的操作
www.example.com/api skip://operation=resHeaders://X-Debug:first
```

### 行为说明

- `pattern=...`：按规则左侧的 pattern 跳过
- `operation=...`：按 `protocol://value` 跳过
- 跳过后不会终止匹配；后续规则仍会继续尝试

### 测试用例

| 测试场景 | 规则 | 预期 |
| --- | --- | --- |
| 跳过 operation | `test.com skip://operation=resHeaders://X-A:first` | 后续同类规则仍可继续生效 |
| 跳过 pattern | `test.com/api/blocked skip://pattern=test.com/api/blocked` | 请求回落到更通用的规则 |

---

## 规则组合

过滤器可以与其他规则组合使用：

```bash
# 多个过滤器
www.example.com host://backend.local includeFilter://m:POST excludeFilter:///health

# 过滤器 + 修改规则
www.example.com resHeaders://(X-Debug: true) includeFilter://h:X-Debug

# 删除 + 过滤器
www.example.com delete://reqHeaders.X-Internal includeFilter://m:GET

# 条件透传
www.example.com passthrough:// includeFilter:///static
```

---

## 注意事项

1. **条件顺序**：多个 `includeFilter` 之间是 AND 关系
2. **优先级**：`excludeFilter` 优先于 `includeFilter`
3. **状态码过滤**：`s:` 过滤器用于响应阶段的规则
4. **头部大小写**：头部名称匹配不区分大小写
5. **Body 过滤边界**：`b:`/`B:` 当前只被解析，不参与运行时匹配；不要在生产规则里依赖它
