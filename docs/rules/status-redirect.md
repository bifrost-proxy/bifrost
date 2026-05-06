# 状态码与重定向规则

本章介绍控制 HTTP 状态码和重定向行为的规则。

---

## statusCode

直接返回指定的 HTTP 状态码，不向后端服务器发送请求。

### 语法

```
pattern statusCode://code
```

### 常用状态码

| 状态码 | 含义                  | 场景         |
| ------ | --------------------- | ------------ |
| `200`  | OK                    | 成功响应     |
| `201`  | Created               | 创建成功     |
| `204`  | No Content            | 成功但无内容 |
| `301`  | Moved Permanently     | 永久重定向   |
| `302`  | Found                 | 临时重定向   |
| `304`  | Not Modified          | 未修改       |
| `400`  | Bad Request           | 请求错误     |
| `401`  | Unauthorized          | 未授权       |
| `403`  | Forbidden             | 禁止访问     |
| `404`  | Not Found             | 未找到       |
| `500`  | Internal Server Error | 服务器错误   |
| `502`  | Bad Gateway           | 网关错误     |
| `503`  | Service Unavailable   | 服务不可用   |

### 基础示例

```bash
# 返回 404
www.example.com/old-page statusCode://404

# 返回 503
www.example.com/maintenance statusCode://503

# 模拟服务器错误
www.example.com/api statusCode://500
```

### 配合 Body

> ⚠️ **注意**：小括号内容会作为一个整体解析，可以包含空格；多行 Body 建议使用块变量。

```bash
# 返回 404 + 自定义内容（使用块变量）
www.example.com statusCode://404 resBody://{not-found-response}

# 返回 200 + JSON
www.example.com statusCode://200 resBody://({"ok": true})
```

块变量定义：

````
``` not-found-response
{"error": "Not Found"}
```
````

### 测试用例

| 测试场景 | 规则                                        | 预期                |
| -------- | ------------------------------------------- | ------------------- |
| 返回 404 | `test.com statusCode://404`                 | HTTP 状态码 404     |
| 返回 500 | `test.com statusCode://500`                 | HTTP 状态码 500     |
| 返回 200 | `test.com statusCode://200`                 | HTTP 状态码 200     |
| 带 Body  | `test.com statusCode://404 resBody://(err)` | 404 + Body 为 "err" |

---

## replaceStatus

替换后端服务器返回的状态码，请求仍然会发送到后端。

### 语法

```
pattern replaceStatus://new_code
```

### 与 statusCode 的区别

| 规则            | 请求后端  | 使用场景             |
| --------------- | --------- | -------------------- |
| `statusCode`    | ❌ 不请求 | 直接 Mock 响应       |
| `replaceStatus` | ✅ 请求   | 修改真实响应的状态码 |

### 示例

```bash
# 将 500 替换为 200
www.example.com replaceStatus://200

# 将任何状态码替换为 404
www.example.com replaceStatus://404

# 配合过滤器，只替换特定状态码
www.example.com replaceStatus://200 includeFilter://s:500
```

### 测试用例

| 测试场景   | 规则                                                 | 后端返回 | 预期            |
| ---------- | ---------------------------------------------------- | -------- | --------------- |
| 替换为 200 | `test.com replaceStatus://200`                       | 500      | HTTP 200        |
| 条件替换   | `test.com replaceStatus://200 includeFilter://s:404` | 404      | HTTP 200        |
| 条件替换   | `test.com replaceStatus://200 includeFilter://s:404` | 200      | HTTP 200 (不变) |

---

## redirect

返回 HTTP 重定向响应，让客户端跳转到新地址。

### 语法

```
pattern redirect://target_url
pattern redirect://status_code:target_url
```

### 重定向状态码

| 状态码 | 类型               | 说明                   |
| ------ | ------------------ | ---------------------- |
| `301`  | 永久重定向         | SEO 友好，浏览器会缓存 |
| `302`  | 临时重定向         | 默认值，不缓存         |
| `303`  | See Other          | 常用于 POST 后重定向   |
| `307`  | Temporary Redirect | 保持请求方法           |
| `308`  | Permanent Redirect | 永久重定向，保持方法   |

### 基础示例

```bash
# 默认 302 重定向
www.example.com/old redirect://https://www.example.com/new

# 301 永久重定向
www.example.com/legacy redirect://301:https://www.example.com/modern

# 307 临时重定向（保持 POST）
www.example.com/api/v1 redirect://307:https://www.example.com/api/v2
```

### 动态重定向

```bash
# 使用模板变量（需要反引号）
www.example.com/go redirect://`https://other.com${path}`

# 重定向到不同协议
http://www.example.com redirect://`https://www.example.com${path}`
```

### 测试用例

| 测试场景   | 规则                                      | 预期                            |
| ---------- | ----------------------------------------- | ------------------------------- |
| 302 重定向 | `test.com redirect://http://new.com/`     | 302 + Location: http://new.com/ |
| 301 重定向 | `test.com redirect://301:http://new.com/` | 301 + Location: http://new.com/ |
| 307 重定向 | `test.com redirect://307:http://new.com/` | 307 + Location: http://new.com/ |

---

## 规则组合

状态码规则可以与其他规则组合：

> ⚠️ **注意**：小括号内容会作为一个整体解析，可以包含空格；多行 Body 建议使用块变量。

```bash
# 状态码 + Body + 头部（使用块变量）
www.example.com statusCode://404 resBody://{not-found} resHeaders://(X-Error: true)

# 重定向 + CORS
www.example.com redirect://https://new.com/ resCors://*

# 替换状态码 + 修改头部
www.example.com replaceStatus://200 resHeaders://(X-Fixed: true)

# 条件状态码修改
www.example.com replaceStatus://200 includeFilter://s:500 includeFilter://s:502
```

块变量定义：

````
``` not-found
Not Found
```
````

---

## 注意事项

1. **statusCode 不请求后端**：使用 `statusCode` 时，请求不会发送到后端服务器
2. **replaceStatus 请求后端**：使用 `replaceStatus` 时，请求会正常发送，只修改返回的状态码
3. **redirect 优先级**：`redirect` 会立即返回重定向响应，后续规则不会执行
4. **状态码与 Body**：使用 `statusCode` 时，默认 Body 为空，需要配合 `resBody` 设置
5. **缓存影响**：301 重定向会被浏览器缓存，调试时建议使用 302
