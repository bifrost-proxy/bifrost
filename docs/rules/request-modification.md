# 请求修改规则

本章介绍修改发送到服务器的请求内容的规则。

---

## reqHeaders

设置或修改请求头部。

### 语法

```
pattern reqHeaders://key=value                    # 内联格式（单个头）
pattern reqHeaders://(key1: value1)               # 小括号格式（可包含空格）
pattern reqHeaders://{varName}                    # 引用内嵌值/Values（推荐）
```

> ⚠️ **重要**：
> 1. `{name}` 是引用内嵌值的语法，不是直接定义 JSON！
> 2. 小括号内容会作为一个整体解析，可以包含空格；多行或多个头部建议使用块变量

### 基础示例

```bash
# 方式1：内联格式设置单个头
www.example.com reqHeaders://X-Custom-Header=custom-value

# 方式2：小括号格式（可包含空格）
www.example.com reqHeaders://(X-Token: abc123)

# 方式3：引用内嵌值（推荐，支持空格和多个头）
www.example.com reqHeaders://{my-headers}
```

内嵌值定义：
````
``` my-headers
X-Token: abc123
X-Version: 1.0
Host: backend.example.com
```
pattern reqHeaders://{my-headers}
````

### 特殊头部

```bash
# 设置 Host 头部
www.example.com reqHeaders://(Host: backend.example.com)

# 设置 Content-Type
www.example.com reqHeaders://Content-Type=application/json

# 设置 Authorization（使用内嵌值避免特殊字符问题）
www.example.com reqHeaders://{auth-header}
```

### 模板变量

模板字符串必须用反引号包裹：

````bash
# 方式1：内联模板
www.example.com reqHeaders://`X-Request-Id=${randomUUID}`

# 方式2：引用内嵌值 + 模板（推荐）
www.example.com reqHeaders://`{req-headers-tpl}`

``` req-headers-tpl
X-Request-Id: ${randomUUID}
X-Timestamp: ${now}
```
````

### 测试用例

| 测试场景 | 规则 | 预期 |
| -------- | --------------------------------------- | ---- |
| 内联格式 | `test.com reqHeaders://X-Custom=value` | 请求包含 `X-Custom: value` |
| 小括号格式 | `test.com reqHeaders://(X-A: 1)` | 请求包含 X-A 头部 |
| 覆盖已有头部 | `test.com reqHeaders://Accept=text/plain` | Accept 被覆盖 |
| 模板变量 | ``test.com reqHeaders://`X-Time=${now}` `` | X-Time 包含时间戳 |

---

## ua

设置 User-Agent 请求头。

### 语法

```
pattern ua://user_agent_string
```

### 示例

> ⚠️ **注意**：操作符后面不支持空格，包含空格的值需要使用块变量或 URI 编码

```bash
# 设置简单标识（无空格）
www.example.com ua://MyApp/1.0

# 使用块变量处理含空格的 UA
www.example.com ua://{chrome-ua}

# 使用 URI 编码处理含空格的 UA
www.example.com ua://Mozilla/5.0%20(Windows%20NT%2010.0)%20Chrome/120.0.0.0
```

块变量定义示例：
```
``` chrome-ua
Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120.0.0.0
```
```

### 预定义 UA

```bash
# iOS Safari
www.example.com ua://iphone

# Android Chrome
www.example.com ua://android

# 桌面 Chrome
www.example.com ua://chrome
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 自定义 UA | `test.com ua://CustomAgent/1.0` | User-Agent 为 `CustomAgent/1.0` |
| 预定义 UA | `test.com ua://iphone` | User-Agent 包含 iPhone 标识 |

---

## referer

设置 Referer 请求头。

### 语法

```
pattern referer://referer_url
```

### 示例

```bash
# 设置来源页面
www.example.com referer://https://www.google.com/

# 清除 Referer
www.example.com referer://

# 设置为同域名
www.example.com/api referer://https://www.example.com/
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 设置 Referer | `test.com referer://https://google.com/` | Referer 为指定 URL |
| 清除 Referer | `test.com referer://` | 无 Referer 头部 |

---

## auth

设置 Basic Auth 的 `Authorization` 请求头。当前实现会把规则值整体当作 `user:password`，并写入 `Authorization: Basic <base64(user:password)>`。

### 语法

```txt
pattern auth://user:password
pattern auth://{basic-auth}
```

### 示例

```bash
# 生成 Authorization: Basic dXNlcjpwYXNz
www.example.com auth://user:pass

# 推荐把账号密码放入内嵌值，避免在规则行暴露
www.example.com auth://{basic-auth}
```

内嵌值定义：

````
``` basic-auth
user:pass
```
````

> 如果需要设置 Bearer token 或任意自定义 `Authorization` 值，请使用 `reqHeaders://Authorization=Bearer...` 或 `reqHeaders://{headers}`，不要用 `auth://`。

---

## forwardedFor

设置 `X-Forwarded-For` 请求头，用于模拟客户端来源 IP。

### 语法

```txt
pattern forwardedFor://ip
```

### 示例

```bash
www.example.com forwardedFor://203.0.113.10
```

---

## method

修改请求方法。

### 语法

```
pattern method://HTTP_METHOD
```

### 支持的方法

- `GET`
- `POST`
- `PUT`
- `DELETE`
- `PATCH`
- `HEAD`
- `OPTIONS`

### 示例

```bash
# GET 转 POST
www.example.com/api method://POST

# POST 转 PUT
www.example.com/api/update method://PUT

# 任意请求转 DELETE
www.example.com/api/resource method://DELETE
```

### 测试用例

| 测试场景 | 规则 | 原始请求 | 预期 |
|---------|------|---------|------|
| GET 转 POST | `test.com method://POST` | GET | 后端收到 POST |
| POST 转 PUT | `test.com method://PUT` | POST | 后端收到 PUT |

---

## reqCookies

设置请求 Cookie。

### 语法

```
pattern reqCookies://name=value              # 内联格式（单个）
pattern reqCookies://(name: value)           # 小括号格式（可包含空格）
pattern reqCookies://{varName}               # 引用内嵌值（推荐）
```

> ⚠️ **注意**：小括号内容会作为一个整体解析，可以包含空格；多个 Cookie 或带属性的复杂值建议使用块变量

### 示例

```bash
# 设置单个 Cookie（内联格式）
www.example.com reqCookies://session=abc123

# 小括号格式
www.example.com reqCookies://(token: xyz789)

# 引用内嵌值（多个 Cookie，推荐）
www.example.com reqCookies://{my-cookies}
```

内嵌值定义：
````
``` my-cookies
token: xyz789
user_id: 12345
```
````

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 内联格式 | `test.com reqCookies://session=abc` | Cookie 包含 session=abc |
| 小括号格式 | `test.com reqCookies://(a: 1)` | Cookie 包含 a=1 |

---

## reqCors

给上游请求补充 CORS 预检相关请求头。

### 语法

```txt
pattern reqCors://*
pattern reqCors://https://frontend.example.com
pattern reqCors://{cors-config}
```

### 行为说明

- `reqCors://*`：设置 `Origin: *`
- `reqCors://https://frontend.example.com`：仅设置 `Origin`
- `reqCors://{cors-config}`：支持完整预检头配置

内嵌值定义：

````
``` cors-config
origin: https://frontend.example.com
method: POST
headers: x-trace-id,x-auth-token
```
````

对应请求头：

| 配置项 | 请求头 |
| --- | --- |
| `origin` | `Origin` |
| `method` / `methods` | `Access-Control-Request-Method` |
| `headers` | `Access-Control-Request-Headers` |

### 示例

```bash
# 只设置 Origin
www.example.com reqCors://*

# 设置固定 Origin
www.example.com reqCors://https://frontend.example.com

# 设置完整预检头
www.example.com reqCors://{cors-config}
```

### 测试用例

| 测试场景 | 规则 | 预期 |
| --- | --- | --- |
| 快捷模式 | `test.com reqCors://*` | 上游收到 `Origin: *` |
| 固定来源 | `test.com reqCors://https://frontend.example.com` | 上游收到指定 `Origin` |
| 详细模式 | `test.com reqCors://{cors-config}` | 上游同时收到 `Origin` / `Access-Control-Request-Method` / `Access-Control-Request-Headers` |

---

## reqType

设置请求的 Content-Type 头部。

### 语法

```
pattern reqType://content_type
```

### 常用类型

| 类型 | 值 |
|------|-----|
| JSON | `application/json` |
| 表单 | `application/x-www-form-urlencoded` |
| 文件上传 | `multipart/form-data` |
| 纯文本 | `text/plain` |
| XML | `application/xml` |

### 示例

```bash
# 设置为 JSON
www.example.com reqType://application/json

# 设置为表单
www.example.com reqType://application/x-www-form-urlencoded
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 设置 JSON 类型 | `test.com reqType://application/json` | Content-Type: application/json |

---

## reqCharset

设置请求的字符编码。

### 语法

```
pattern reqCharset://charset
```

### 示例

```bash
# 设置 UTF-8
www.example.com reqCharset://utf-8

# 设置 GBK
www.example.com reqCharset://gbk
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 设置 UTF-8 | `test.com reqCharset://utf-8` | Content-Type 包含 charset=utf-8 |

---

## headerReplace

局部替换请求头内容。

### 语法

```
pattern headerReplace://req.header_name:old_value=new_value
pattern headerReplace://req.header_name:/regex/=replacement
```

### 示例

```bash
# 替换 Accept 中的内容
www.example.com headerReplace://req.accept:text/html=application/json

# 使用正则替换
www.example.com headerReplace://req.user-agent:/Chrome\/\d+/=Chrome/999

# 删除部分内容
www.example.com headerReplace://req.cookie:session=[^;]+=
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 简单替换 | `test.com headerReplace://req.accept:html=json` | Accept 中 html 被替换为 json |
| 正则替换 | `test.com headerReplace://req.ua:/v\d+/=v999` | 版本号被替换 |

---

## 规则组合

请求修改规则可以组合使用：

```bash
# 同时修改多个属性
www.example.com reqHeaders://X-Token=abc ua://MyApp/1.0 referer://https://google.com/

# 配合路由规则（使用内嵌值处理含空格头部值）
www.example.com host://backend.local reqHeaders://{forwarded-headers}

# 配合过滤器
www.example.com reqHeaders://X-Debug=true includeFilter://m:POST
```

---

## 注意事项

1. **头部覆盖**：设置已存在的头部会覆盖原值
2. **大小写**：HTTP 头部名称不区分大小写
3. **Cookie 合并**：`reqCookies` 会与现有 Cookie 合并
4. **模板变量**：支持在头部值中使用 `${variable}` 形式的模板变量
