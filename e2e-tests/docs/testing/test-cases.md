# 端到端测试用例清单

本文档列出了所有需要实现的 E2E 测试用例，按规则分类组织。

---

## 测试覆盖状态

| 状态 | 说明 |
|------|------|
| ✅ | 已实现 |
| 🔨 | 待实现 |
| ⏭️ | 跳过（UI 相关或暂不支持） |

---

## 1. 路由规则测试 (routing)

### host 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| R001 | 基础 host 重定向 | `test.com host://127.0.0.1:MOCK_PORT` | 请求到达 Mock 服务器 | ✅ |
| R002 | 带端口 host | `test.com host://127.0.0.1:8888` | 请求转发到指定端口 | ✅ |
| R003 | 路径保留 | `test.com host://127.0.0.1:MOCK_PORT` | 路径 `/api/users` 保留 | ✅ |
| R004 | 通配符匹配 | `*.test.com host://127.0.0.1:MOCK_PORT` | 子域名匹配成功 | ✅ |

### proxy 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| R010 | HTTP 代理转发 | `test.com proxy://127.0.0.1:PROXY_PORT` | 请求通过代理转发 | 🔨 |
| R011 | 代理链 | `test.com proxy://proxy1 xproxy://proxy2` | 多级代理 | 🔨 |

### socks 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| R020 | SOCKS5 代理 | `test.com socks://127.0.0.1:SOCKS_PORT` | 请求通过 SOCKS 转发 | 🔨 |

### tunnel 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| R030 | 隧道透传 | `test.com tunnel://127.0.0.1:PORT` | 请求直接透传 | 🔨 |

---

## 2. 请求修改测试 (request)

### reqHeaders 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q001 | 单个请求头 | `test.com reqHeaders://(X-Custom: value)` | 请求包含自定义头 | ✅ |
| Q002 | 多个请求头 | `test.com reqHeaders://{"X-A":"1","X-B":"2"}` | 请求包含两个头 | ✅ |
| Q002a | `&` 分隔多个请求头 | `test.com reqHeaders://(X-A=1&X-B=2)` | 请求包含两个独立头 | ✅ |
| Q003 | 覆盖已有头 | `test.com reqHeaders://(Accept: text/plain)` | Accept 被覆盖 | ✅ |
| Q004 | 模板变量 | `test.com reqHeaders://(X-UUID: ${randomUUID})` | 头部包含有效 UUID | ✅ |

### ua 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q010 | 自定义 UA | `test.com ua://CustomAgent/1.0` | User-Agent 被设置 | ✅ |

### referer 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q020 | 设置 Referer | `test.com referer://https://google.com/` | Referer 被设置 | ✅ |
| Q021 | 清除 Referer | `test.com referer://` | Referer 被删除 | 🔨 |

### method 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q030 | GET 转 POST | `test.com method://POST` | 后端收到 POST | ✅ |
| Q031 | POST 转 PUT | `test.com method://PUT` | 后端收到 PUT | 🔨 |

### reqCookies 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q040 | 设置 Cookie | `test.com reqCookies://(session: abc)` | Cookie 包含 session | ✅ |
| Q041 | 多个 Cookie | `test.com reqCookies://{"a":"1","b":"2"}` | Cookie 包含多个值 | 🔨 |

### reqType 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q060 | 设置 Content-Type | `test.com reqType://application/json` | Content-Type 被设置 | 🔨 |

### headerReplace 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| Q070 | 替换请求头内容 | `test.com headerReplace://req.accept:html=json` | 头部内容被替换 | 🔨 |

---

## 3. 响应修改测试 (response)

### resHeaders 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| S001 | 单个响应头 | `test.com resHeaders://(X-Custom: value)` | 响应包含自定义头 | ✅ |
| S002 | 多个响应头 | `test.com resHeaders://{"X-A":"1","X-B":"2"}` | 响应包含两个头 | ✅ |

### resCookies 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| S010 | 设置响应 Cookie | `test.com resCookies://session=abc` | Set-Cookie 被设置 | ✅ |
| S011 | 带属性 Cookie | `test.com resCookies://{"auth":{"value":"token","httpOnly":true}}` | Cookie 带属性 | 🔨 |

### resCors 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| S020 | 允许所有来源 | `test.com resCors://*` | ACAO: * | ✅ |
| S021 | 特定来源 | `test.com resCors://https://app.com` | ACAO: https://app.com | 🔨 |
| S022 | 带 Credentials | `test.com resCors://{credentials: true}` | ACAC: true | 🔨 |

### resType 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| S030 | 设置 Content-Type | `test.com resType://application/json` | Content-Type 被设置 | 🔨 |

### cache 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| S040 | 设置缓存 | `test.com cache://3600` | Cache-Control 设置 | 🔨 |
| S041 | 禁用缓存 | `test.com cache://0` | no-cache 设置 | 🔨 |

---

## 4. URL 操作测试 (url_modification)

### urlParams 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| U001 | 添加单个参数 | `test.com urlParams://debug=true` | URL 包含 debug=true | 🔨 |
| U002 | 添加多个参数 | `test.com urlParams://{x: 1, y: 2}` | URL 包含两个参数 | 🔨 |
| U003 | 覆盖已有参数 | `test.com urlParams://a=new` | 参数 a 被覆盖 | 🔨 |

### pathReplace 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| U010 | 简单路径替换 | `test.com pathReplace://old=new` | 路径中 old 被替换 | 🔨 |
| U011 | 正则路径替换 | `test.com pathReplace://(/v\d+/=v99)` | 版本号被替换 | 🔨 |

---

## 5. Body 操作测试 (body_modification)

### file 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B001 | 内联 JSON 响应 | `test.com file://({"ok":true})` | 响应 Body 为 JSON | 🔨 |
| B002 | 内联文本响应 | `test.com file://(hello world)` | 响应 Body 为文本 | 🔨 |

### tpl 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B010 | 时间戳变量 | `test.com tpl://({"t":${now}})` | 响应包含时间戳 | ✅ |
| B011 | UUID 变量 | `test.com tpl://({"id":"${randomUUID}"})` | 响应包含 UUID | ✅ |
| B012 | 请求信息变量 | `test.com tpl://({"path":"${path}"})` | 响应包含路径 | ✅ |
| B013 | JSONP 回调 | `test.com tpl://(${query.cb}({}))` | 响应使用回调包装 | 🔨 |

### reqBody 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B020 | 设置请求 Body | `test.com reqBody://({"a":1})` | 请求 Body 被设置 | 🔨 |
| B021 | 清空请求 Body | `test.com reqBody://()` | 请求 Body 为空 | 🔨 |

### resBody 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B030 | 设置响应 Body | `test.com resBody://({"ok":true})` | 响应 Body 被设置 | 🔨 |

### resReplace 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B040 | 简单替换 | `test.com resReplace://old=new` | Body 中内容被替换 | 🔨 |
| B041 | 全局替换 | `test.com resReplace://(/a/g=b)` | 所有匹配被替换 | 🔨 |

### resMerge 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| B050 | JSON 合并 | `test.com resMerge://{extra: 1}` | JSON 字段被添加 | 🔨 |

---

## 6. 状态码测试 (status)

### statusCode 规则 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| C001 | 返回 404 | `test.com statusCode://404` | HTTP 404 | ✅ |
| C002 | 返回 500 | `test.com statusCode://500` | HTTP 500 | ✅ |
| C003 | 返回 200 | `test.com statusCode://200` | HTTP 200 | ✅ |
| C004 | 带 Body 返回 | `test.com statusCode://404 resBody://(err)` | 404 + Body | 🔨 |

### replaceStatus 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| C010 | 替换为 200 | `test.com replaceStatus://200` | 状态码被替换 | 🔨 |
| C011 | 条件替换 | `test.com replaceStatus://200 includeFilter://s:500` | 仅 500 被替换 | 🔨 |

### redirect 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| C020 | 302 重定向 | `test.com redirect://http://new.com/` | 302 + Location | 🔨 |
| C021 | 301 重定向 | `test.com redirect://301:http://new.com/` | 301 + Location | 🔨 |

---

## 7. 延迟限速测试 (timing)

### reqDelay 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| T001 | 请求延迟 1s | `test.com reqDelay://1000` | 延迟 ~1000ms | 🔨 |

### resDelay 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| T010 | 响应延迟 1s | `test.com resDelay://1000` | 延迟 ~1000ms | 🔨 |

### resSpeed 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| T020 | 限速 10KB/s | `test.com resSpeed://10` | 速度 ~10KB/s | 🔨 |

---

## 8. 过滤器测试 (filters)

### includeFilter 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| F001 | 方法过滤 POST | `test.com resHeaders://X=1 includeFilter://m:POST` | 仅 POST 生效 | 🔨 |
| F002 | 方法过滤 GET | `test.com resHeaders://X=1 includeFilter://m:GET` | 仅 GET 生效 | 🔨 |
| F003 | 头部过滤 | `test.com host://debug includeFilter://h:X-Debug=true` | 有头部时生效 | 🔨 |
| F004 | 状态码过滤 | `test.com replaceStatus://200 includeFilter://s:500` | 仅 500 生效 | 🔨 |

### excludeFilter 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| F010 | 排除 GET | `test.com resHeaders://X=1 excludeFilter://m:GET` | GET 不生效 | 🔨 |

### delete 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| F020 | 删除请求头 | `test.com delete://reqHeaders.X-Custom` | 头部被删除 | 🔨 |
| F021 | 删除响应头 | `test.com delete://resHeaders.Server` | 头部被删除 | 🔨 |
| F022 | 删除 Cookie | `test.com delete://reqCookies.session` | Cookie 被删除 | 🔨 |
| F023 | 删除 URL 参数 | `test.com delete://urlParams.debug` | 参数被删除 | 🔨 |

### enable/disable 规则 🔨

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| F030 | 中断请求 | `test.com enable://abort` | 请求被中断 | 🔨 |

---

## 9. 匹配模式测试 (matchers)

### 精确匹配 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| M001 | 域名精确匹配 | `test.com host://...` | 精确匹配 test.com | ✅ |
| M002 | 路径前缀匹配 | `test.com/api host://...` | 匹配 /api 开头路径 | ✅ |

### 通配符匹配 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| M010 | 单级通配 | `*.test.com host://...` | 匹配一级子域名 | ✅ |
| M011 | 多级通配 | `**.test.com host://...` | 匹配多级子域名 | 🔨 |

### 正则匹配 ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| M020 | 正则匹配 | `/\/api\/v\d+/ host://...` | 匹配 /api/v1, /api/v2 | ✅ |
| M021 | 大小写不敏感 | `/\/api/i host://...` | 匹配 /api, /API | ✅ |

---

## 10. 模板变量测试 (template) ✅

| 测试 ID | 测试名称 | 规则 | 预期结果 | 状态 |
|---------|---------|------|---------|------|
| V001 | ${now} | `tpl://(${now})` | 返回时间戳 | ✅ |
| V002 | ${random} | `tpl://(${random})` | 返回随机数 | ✅ |
| V003 | ${randomUUID} | `tpl://(${randomUUID})` | 返回 UUID | ✅ |
| V004 | ${url} | `tpl://(${url})` | 返回完整 URL | ✅ |
| V005 | ${host} | `tpl://(${host})` | 返回主机名 | ✅ |
| V006 | ${path} | `tpl://(${path})` | 返回路径 | ✅ |
| V007 | ${method} | `tpl://(${method})` | 返回请求方法 | ✅ |

---

## 测试统计

| 类别 | 总用例 | 已实现 | 待实现 |
|------|--------|--------|--------|
| 路由规则 | 8 | 4 | 4 |
| 请求修改 | 12 | 7 | 5 |
| 响应修改 | 10 | 5 | 5 |
| URL 操作 | 5 | 0 | 5 |
| Body 操作 | 13 | 3 | 10 |
| 状态码 | 7 | 3 | 4 |
| 延迟限速 | 3 | 0 | 3 |
| 过滤器 | 9 | 0 | 9 |
| 匹配模式 | 5 | 4 | 1 |
| 模板变量 | 7 | 7 | 0 |
| **总计** | **79** | **33** | **46** |

---

## 实现优先级

### P0 - 核心功能

1. `urlParams` - URL 参数操作
2. `pathReplace` - 路径替换
3. `file` - Mock 文件响应
4. `resBody` - 响应 Body 设置
5. `redirect` - 重定向

### P1 - 重要功能

1. `reqBody` - 请求 Body 设置
2. `resReplace` - 响应替换
3. `replaceStatus` - 状态码替换
4. `includeFilter` - 过滤器
5. `delete` - 删除操作

### P2 - 扩展功能

1. `reqDelay`/`resDelay` - 延迟
2. `resSpeed` - 限速
3. `headerReplace` - 头部替换
4. `enable`/`disable` - 特性控制
