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
| 状态码 | `s:CODE` / `s:200-299` / `s:200,404,500` | 应匹配响应状态码；⚠️ 当前 0.0.96 实现下响应阶段不评估 `s:`，恒不命中（见下方注） |
| 请求头存在 | `h:name` | 判断请求头是否存在 |
| 请求头匹配 | `h:name=value` / `reqH:name=/regex/` | `h:` 和 `reqH:` 都匹配请求头；`value` 可为普通文本或 `/regex/` |
| 响应头匹配 | `resH:name=value` / `resH:name=/regex/` | 应匹配响应头；⚠️ 当前 0.0.96 实现下响应阶段不评估 `resH:`，恒不命中（见下方注） |
| 客户端 IP | `i:ip` / `i:cidr` | 匹配客户端 IP 或 CIDR |
| 路径前缀 | `/path` | 以 `/` 开头但不以 `/` 结尾时，按普通前缀匹配：`/account` 匹配 `/account`、`/account/...`、`/account-center` 和带 query 的路径 |
| 路径正则 | `/regex/` | 以 `/` 开头并以 `/` 结尾时，按正则匹配路径 |
| URL host/path | `example.com` / `example.com/api` | 包含 `.` 的过滤值会按 host 与可选 path 匹配 |
| URL 通配符 | `*/api` / `*/alice/*` | 兼容 Whistle 风格的 URL wildcard filter；`*/api` 匹配 `/api` 及其子路径，`*/alice/*` 匹配 URL 中包含 `/alice/` 的请求 |

> 当前实现会解析 `b:` / `B:` body 过滤器，但运行时 resolver 尚未读取请求/响应 body 参与过滤，**实测行为随写法不同**：文档使用的 `b:/regex/` 形式（如 `b:/error/`）写在 `includeFilter://b:/.../`  里会 fail-closed，规则**永远不命中**（实测无论有无 body 都返回 502），写在 `excludeFilter://b:/.../`  里则永远不排除（规则照常生效）；不带斜杠的裸值形式（如 `b:foo`）则被直接忽略，规则照常命中。两种写法都无法真正按 body 过滤。不要把 body 过滤作为可用能力依赖；需要按内容筛选请使用 `bifrost search --req-body/--res-body` 查看流量证据。

> ⚠️ **当前限制（0.0.96，经实测）**：响应阶段过滤器 `s:`（状态码：`s:CODE` / `s:200-299` / `s:200,404,500` / `s:400-499`）与 `resH:`（响应头匹配）在当前实现下不参与运行时匹配，**恒不命中**。后果：
> - `includeFilter://s:...` / `includeFilter://resH:...` 把规则锁死为永不应用——即使响应确实是该状态码或带该响应头，被门控的操作也不会执行。
> - `excludeFilter://s:...` 是 no-op——不会按预期排除任何响应（例如 `excludeFilter://s:200` 不会在 200 响应上抑制规则）。
>
> 在该问题修复前，不要依赖 `s:` / `resH:` 过滤器；本页下文凡使用它们的示例均沿用此限制（已就地标注）。请求阶段的方法过滤 `m:` 与请求头过滤 `h:` / `reqH:` 不受影响，仍可正常使用。

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
>
> ⚠️ **当前不可用（0.0.96，经实测）**：以下 `s:` 状态码过滤器在当前实现下恒不命中（见上文响应阶段限制）。被门控的操作（`replaceStatus`、`resBody`、`resHeaders`）本身可正常工作，但加上 `includeFilter://s:...` 后规则永不应用。例如 `127.0.0.1:18181 replaceStatus://200 includeFilter://s:500` 对 500 响应不会改写状态码（实测响应仍为 500）；而去掉 `s:500` 过滤器时 `replaceStatus://200` 能把 500 正确改写为 200。下列示例仅展示**预期**语义，在修复前不要依赖。

```bash
# 预期：只对 500 响应生效（当前 s: 恒不命中，规则不会应用）
www.example.com replaceStatus://200 includeFilter://s:500

# 预期：只对 404 响应生效，使用块变量（当前 s: 恒不命中，规则不会应用）
www.example.com resBody://{not-found} includeFilter://s:404

# 预期：对 4xx 响应生效（当前 s: 恒不命中，规则不会应用）
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

> ⚠️ **当前不可用（0.0.96，经实测）**：`resH:` 响应头过滤器在当前实现下恒不命中（见上文响应阶段限制）。实测即使响应确实带有目标响应头（例如响应携带 `X-Upstream: mock-18181`，规则 `includeFilter://resH:X-Upstream=mock-18181` 仍不应用，`/regex/` 形式同样不命中），被门控的操作也不会执行。下列示例仅展示**预期**语义，在修复前不要依赖。

```bash
# 预期：匹配响应头（当前 resH: 恒不命中，规则不会应用）
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
| 状态码 500 | `test.com replaceStatus://200 includeFilter://s:500`   | 返回 500 | ⚠️ 0.0.96 实测仍为 500（`s:` 恒不命中，规则不应用；预期为变 200） |
| 状态码 500 | `test.com replaceStatus://200 includeFilter://s:500`   | 返回 200 | 不变（`s:` 恒不命中，规则本就不应用） |
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

# 预期：排除成功响应（⚠️ 0.0.96 实测 excludeFilter://s:200 为 no-op：200 响应上仍会注入 X-Error: true，排除不生效；见上文响应阶段限制）
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

`passthrough://` 用于在当前优先级位置选择直接透传请求。旧的 `ignore://` 写法会在导入、同步或保存时自动转换为 `passthrough://`。

`passthrough://` 与 `http://`、`https://`、`host://` 等路由目标遵循同一套 first-win 语义：如果更高优先级的具体路由已经选中，后续更宽泛的 passthrough 不会覆盖它；如果 passthrough 优先级更高，则它会阻止后续较低优先级的路由目标生效。

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

| 目标     | 语法              | 说明          |
| -------- | ----------------- | ------------- |
| 请求头   | `reqHeaders.name` | 删除请求头    |
| 响应头   | `resHeaders.name` | 删除响应头    |
| URL 参数 | `urlParams.name`  | 删除 URL 参数 |

### 示例

```bash
# 删除请求头
www.example.com delete://reqHeaders.X-Custom

# 删除响应头
www.example.com delete://resHeaders.X-Powered-By

# 删除 URL 参数
www.example.com delete://urlParams.debug
```

### 多个删除

```bash
# 删除多个头部
www.example.com delete://reqHeaders.X-A delete://reqHeaders.X-B

# 删除多个 URL 参数
www.example.com delete://urlParams.a delete://urlParams.b
```

### 测试用例

| 测试场景   | 规则                                    | 预期                |
| ---------- | --------------------------------------- | ------------------- |
| 删除请求头 | `test.com delete://reqHeaders.X-Custom` | 请求不含 X-Custom   |
| 删除响应头 | `test.com delete://resHeaders.Server`   | 响应不含 Server     |
| 删除参数   | `test.com delete://urlParams.debug`     | URL 不含 debug 参数 |

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
3. **状态码 / 响应头过滤当前不可用**：`s:` 与 `resH:` 本应用于响应阶段，但 0.0.96 实测响应阶段不评估它们，恒不命中——`includeFilter://s:` / `includeFilter://resH:` 会让规则永不应用，`excludeFilter://s:` / `excludeFilter://resH:` 是 no-op。修复前请勿依赖；请求阶段的 `m:` / `h:` / `reqH:` 不受影响。
4. **头部大小写**：头部名称匹配不区分大小写
5. **Body 过滤边界**：`b:`/`B:` 当前只被解析，不参与运行时匹配，行为随写法不同：文档的 `b:/regex/` 形式在 `includeFilter` 里 fail-closed（规则永远不命中、实测 502），在 `excludeFilter` 里永远不排除；不带斜杠的裸值（`b:foo`）则被忽略、规则照常命中。两种写法都不可用，不要在生产规则里依赖它
