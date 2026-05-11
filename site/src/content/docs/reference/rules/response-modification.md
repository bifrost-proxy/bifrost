---
title: "响应改写"
description: "响应头、响应体与相关字段的改写说明。"
editUrl: false
sidebar:
  label: "响应改写"
  order: 230
---

> 此页面由 `docs/rules/response-modification.md` 自动同步生成。
# 响应修改规则

本章介绍修改返回给客户端的响应内容的规则。

---

## resHeaders

设置或修改响应头部。

### 语法

```
pattern resHeaders://key=value              # 内联格式（单个头）
pattern resHeaders://(key1: value1)         # 小括号格式（可包含空格）
pattern resHeaders://{varName}              # 引用内嵌值/Values（推荐）
```

> ⚠️ **重要**：
>
> 1. `{name}` 是引用内嵌值的语法，不是直接定义 JSON！
> 2. 小括号内容会作为一个整体解析，可以包含空格；多行或多个头部建议使用块变量

### 基础示例

```bash
# 内联格式设置单个头
www.example.com resHeaders://X-Custom-Header=custom-value

# 小括号格式
www.example.com resHeaders://(X-Version: 1.0)

# 引用内嵌值（推荐，支持空格和多个头）
www.example.com resHeaders://{my-res-headers}
```

内嵌值定义：

````
``` my-res-headers
X-Version: 1.0
X-Server: bifrost
```
````

### 常用场景

```bash
# 添加安全头部
www.example.com resHeaders://(X-Frame-Options: DENY)

# 设置缓存控制（使用内嵌值处理逗号和空格）
www.example.com resHeaders://{cache-headers}

# 添加调试信息
www.example.com resHeaders://X-Debug-Info=proxy-enabled
```

### 测试用例

| 测试场景     | 规则                                            | 预期                       |
| ------------ | ----------------------------------------------- | -------------------------- |
| 内联格式     | `test.com resHeaders://X-Custom=value`          | 响应包含 `X-Custom: value` |
| 小括号格式   | `test.com resHeaders://(X-A: 1)`                | 响应包含 X-A 头部          |
| 覆盖已有头部 | `test.com resHeaders://Content-Type=text/plain` | Content-Type 被覆盖        |

---

## resCookies

设置响应 Set-Cookie 头部。

### 语法

```
pattern resCookies://name=value              # 内联格式
pattern resCookies://(name: value)           # 小括号格式（可包含空格）
pattern resCookies://{varName}               # 引用内嵌值（推荐）
```

> ⚠️ **注意**：小括号内容会作为一个整体解析，可以包含空格；多个 Cookie 或带属性的复杂值建议使用块变量

### 基础示例

```bash
# 内联格式
www.example.com resCookies://session=abc123

# 小括号格式
www.example.com resCookies://(token: xyz789)

# 引用内嵌值（多个 Cookie，推荐）
www.example.com resCookies://{my-cookies}
```

### 高级选项

```bash
# 带属性的 Cookie（使用内嵌值处理复杂内容）
www.example.com resCookies://{auth-cookie}
```

内嵌值定义：

````
``` auth-cookie
auth: token123; path=/; httpOnly; secure
```
````

### 测试用例

| 测试场景   | 规则                                | 预期                        |
| ---------- | ----------------------------------- | --------------------------- |
| 内联格式   | `test.com resCookies://session=abc` | Set-Cookie 包含 session=abc |
| 小括号格式 | `test.com resCookies://(a: 1)`      | 响应包含 Set-Cookie         |

---

## resCors

快速设置 CORS（跨域资源共享）响应头。

### 语法

```

pattern resCors://\*
pattern resCors://https://app.example.com
pattern resCors://{options}

```

### 基础示例

```bash
# 允许所有来源
www.example.com resCors://*

# 允许特定来源
www.example.com resCors://https://app.example.com

# 允许特定来源
www.example.com resCors://https://app.example.com
```

### 高级选项

```bash
# 完整 CORS 配置（使用内嵌值）
www.example.com resCors://{cors-config}
```

内嵌值定义：

````
``` cors-config
origin: *
credentials: true
methods: GET,POST,PUT
headers: X-Custom
maxAge: 86400
expose: X-Trace-Id
```
````

说明：

- 支持 JSON 值，也支持上面的多行 `key: value` 格式
- `origin` 为空时默认回退为 `*`
- `credentials` 为 `true` 时会返回 `Access-Control-Allow-Credentials: true`

### CORS 头部映射

| 选项          | 对应头部                         | 说明               |
| ------------- | -------------------------------- | ------------------ |
| `origin`      | Access-Control-Allow-Origin      | 允许的来源         |
| `methods`     | Access-Control-Allow-Methods     | 允许的方法         |
| `headers`     | Access-Control-Allow-Headers     | 允许的请求头       |
| `credentials` | Access-Control-Allow-Credentials | 是否允许携带凭证   |
| `maxAge`      | Access-Control-Max-Age           | 预检请求缓存时间   |
| `expose`      | Access-Control-Expose-Headers    | 暴露给客户端的头部 |

### 测试用例

| 测试场景     | 规则                                 | 预期                                         |
| ------------ | ------------------------------------ | -------------------------------------------- |
| 允许所有来源 | `test.com resCors://*`               | Access-Control-Allow-Origin: \*              |
| 特定来源     | `test.com resCors://https://app.com` | Access-Control-Allow-Origin: https://app.com |
| 详细配置     | `test.com resCors://{cors-config}`   | 返回 methods / headers / max-age 等完整 CORS 头 |

---

## resType

设置响应的 Content-Type 头部。

### 语法

```
pattern resType://content_type
```

### 常用类型

| 类型       | 值                        |
| ---------- | ------------------------- |
| JSON       | `application/json`        |
| HTML       | `text/html`               |
| JavaScript | `application/javascript`  |
| CSS        | `text/css`                |
| 纯文本     | `text/plain`              |
| XML        | `application/xml`         |
| 图片       | `image/png`, `image/jpeg` |

### 示例

```bash
# 设置为 JSON
www.example.com resType://application/json

# 设置为 HTML
www.example.com resType://text/html

# 强制下载
www.example.com resType://application/octet-stream
```

### 测试用例

| 测试场景  | 规则                                  | 预期                           |
| --------- | ------------------------------------- | ------------------------------ |
| 设置 JSON | `test.com resType://application/json` | Content-Type: application/json |
| 设置 HTML | `test.com resType://text/html`        | Content-Type: text/html        |

---

## resCharset

设置响应的字符编码。

### 语法

```
pattern resCharset://charset
```

### 示例

```bash
# 设置 UTF-8
www.example.com resCharset://utf-8

# 设置 GBK
www.example.com resCharset://gbk

# 设置 ISO-8859-1
www.example.com resCharset://iso-8859-1
```

### 测试用例

| 测试场景   | 规则                          | 预期                            |
| ---------- | ----------------------------- | ------------------------------- |
| UTF-8 编码 | `test.com resCharset://utf-8` | Content-Type 包含 charset=utf-8 |

---

## responseFor / trailers

这些协议用于设置额外响应元数据。`cache` 与 `attachment` 在下方有独立章节。

### 语法

```txt
pattern responseFor://value
pattern trailers://{trailer-headers}
```

### 示例

```bash
# 添加 x-bifrost-response-for 响应头
api.example.com responseFor://mock

# 设置响应 trailers（使用内嵌值）
api.example.com trailers://{debug-trailers}
```

内嵌值定义：

````
``` debug-trailers
X-Trace-Id: abc123
X-Debug: true
```
````

---

## headerReplace (响应头)

局部替换响应头内容。

### 语法

```
pattern headerReplace://res.header_name:old_value=new_value
pattern headerReplace://res.header_name:/regex/=replacement
```

### 示例

```bash
# 替换 Content-Type
www.example.com headerReplace://res.content-type:text/plain=application/json

# 使用正则替换
www.example.com headerReplace://res.server:/nginx\/\d+/=nginx/custom

# 修改 Cache-Control
www.example.com headerReplace://res.cache-control:max-age=\d+=max-age=0
```

### 测试用例

| 测试场景 | 规则                                                      | 预期                   |
| -------- | --------------------------------------------------------- | ---------------------- |
| 简单替换 | `test.com headerReplace://res.server:nginx=custom`        | Server 中 nginx 被替换 |
| 正则替换 | `test.com headerReplace://res.content-type:/\/\w+/=/json` | MIME 类型被修改        |

---

## cache

控制响应缓存行为。

### 语法

```
pattern cache://seconds
pattern cache://{options}
```

### 示例

```bash
# 缓存 1 小时
www.example.com cache://3600

# 禁用缓存
www.example.com cache://0

# 缓存一天
www.example.com cache://86400
```

### 测试用例

| 测试场景 | 规则                    | 预期                            |
| -------- | ----------------------- | ------------------------------- |
| 设置缓存 | `test.com cache://3600` | Cache-Control 设置 max-age=3600 |
| 禁用缓存 | `test.com cache://0`    | 响应头包含 no-cache 相关指令    |

---

## attachment

设置响应为附件下载。

### 语法

```
pattern attachment://filename
```

### 示例

```bash
# 设置下载文件名
www.example.com/api/export attachment://data.csv

# 动态文件名（使用模板变量需要反引号）
www.example.com/report attachment://`report_${now}.pdf`
```

### 测试用例

| 测试场景   | 规则                             | 预期                                                 |
| ---------- | -------------------------------- | ---------------------------------------------------- |
| 设置附件名 | `test.com attachment://file.txt` | Content-Disposition: attachment; filename="file.txt" |

---

## 规则组合

响应修改规则可以组合使用：

```bash
# 多个响应头 + CORS（使用内嵌值）
www.example.com resHeaders://{my-headers} resCors://*

# 类型 + 编码
www.example.com resType://text/html resCharset://utf-8

# 配合路由规则
www.example.com host://backend.local resHeaders://{proxy-headers} resCors://*

# 配合过滤器
www.example.com resCors://* includeFilter://m:OPTIONS
```

---

## 注意事项

1. **头部覆盖**：设置已存在的头部会覆盖原值
2. **CORS 预检**：`resCors` 会自动处理 OPTIONS 预检请求
3. **Cookie 安全**：生产环境建议使用 `httpOnly` 和 `secure` 属性
4. **缓存控制**：`cache://0` 会同时设置 `no-cache`, `no-store`, `must-revalidate`
