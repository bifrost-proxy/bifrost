# 代理规则高级协议测试用例

## 功能模块说明

验证 Bifrost 代理规则中除基础操作（host、file、redirect、statusCode、resHeaders、reqHeaders、resBody、reqBody、delay、cache、resCors）之外的所有高级协议，包括请求修改、响应修改、内容注入、控制协议、路由协议、脚本协议及高级特性（Values 引用、模板字符串、捕获组、多行规则、规则优先级、内联值）。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保端口 8800 未被占用
3. 部分测试需要通过 API 创建规则，API 地址为 `http://127.0.0.1:8800/_bifrost/api/rules`
4. 部分测试需要通过 API 创建 Values，API 地址为 `http://127.0.0.1:8800/_bifrost/api/values`
5. 部分测试需要通过 API 创建 Scripts，API 地址为 `http://127.0.0.1:8800/_bifrost/api/scripts`

---

## 测试用例

### 一、请求修改协议

### TC-PRA-01：reqCookies 协议（修改请求 Cookie）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqcookies", "content": "httpbin.org/cookies reqCookies://(session=abc123;user=test)", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/cookies
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqcookies
   ```

**预期结果**：
- 响应体 JSON 中 `cookies` 对象包含 `"session": "abc123"` 和 `"user": "test"`
- 代理在转发前注入了请求 Cookie

---

### TC-PRA-02：reqCors 协议（添加请求 CORS 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqcors", "content": "httpbin.org/headers reqCors://", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqcors
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 CORS 相关请求头（如 `Origin` 或 `Access-Control-Request-Method`）
- 代理在转发前添加了 CORS 请求头

---

### TC-PRA-03：method 协议（修改 HTTP 方法）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-method", "content": "httpbin.org/post method://POST", "enabled": true}'
   ```
2. 执行命令（使用 GET 请求，但会被代理改为 POST）：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-method
   ```

**预期结果**：
- 返回 HTTP 200 状态码（httpbin.org/post 仅接受 POST 请求）
- 响应体 JSON 中包含 `"url": "http://httpbin.org/post"`
- 代理将原始 GET 方法改为 POST 转发

---

### TC-PRA-04：ua 协议（设置 User-Agent）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-ua", "content": "httpbin.org/user-agent ua://BifrostTestAgent/2.0", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/user-agent
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-ua
   ```

**预期结果**：
- 响应体 JSON 中 `user-agent` 值为 `"BifrostTestAgent/2.0"`
- 代理覆盖了原始的 User-Agent 头

---

### TC-PRA-05：referer 协议（设置 Referer 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-referer", "content": "httpbin.org/headers referer://https://www.google.com/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-referer
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 `"Referer": "https://www.google.com/"`
- 代理注入了 Referer 请求头

---

### TC-PRA-06：urlParams 协议（添加/修改 URL 查询参数）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-urlparams", "content": "httpbin.org/get urlParams://(source=bifrost&version=1.0)", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-urlparams
   ```

**预期结果**：
- 响应体 JSON 中 `args` 对象包含 `"source": "bifrost"` 和 `"version": "1.0"`
- 响应体 JSON 中 `url` 包含 `?source=bifrost&version=1.0`（或参数顺序相反）

---

### TC-PRA-07：params 协议（合并 JSON 到请求体，reqMerge 别名）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-params", "content": "httpbin.org/post params://(injected=true)", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -H "Content-Type: application/json" -d '{"original":"data"}' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-params
   ```

**预期结果**：
- 响应体 JSON 中 `form` 或 `data` 字段包含 `injected` 参数
- 代理在转发前将参数合并到了请求体中

---

### TC-PRA-08：reqPrepend 协议（请求体前置内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqprepend", "content": "httpbin.org/post reqPrepend://PREFIX_", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -d 'original_body' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqprepend
   ```

**预期结果**：
- 响应体 JSON 中 `data` 字段以 `PREFIX_` 开头
- 代理在转发前在请求体前面添加了内容

---

### TC-PRA-09：reqAppend 协议（请求体追加内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqappend", "content": "httpbin.org/post reqAppend://_SUFFIX", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -d 'original_body' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqappend
   ```

**预期结果**：
- 响应体 JSON 中 `data` 字段以 `_SUFFIX` 结尾
- 代理在转发前在请求体末尾追加了内容

---

### TC-PRA-10：reqReplace 协议（请求体搜索替换）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqreplace", "content": "httpbin.org/post reqReplace://hello/world/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -d 'say hello to bifrost' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqreplace
   ```

**预期结果**：
- 响应体 JSON 中 `data` 字段为 `say world to bifrost`
- 请求体中的 `hello` 被替换为 `world`

---

### TC-PRA-11：reqType 协议（设置请求 Content-Type）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqtype", "content": "httpbin.org/post reqType://application/xml", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -d '<data>test</data>' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqtype
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象的 `Content-Type` 值包含 `application/xml`
- 代理修改了请求的 Content-Type 头

---

### TC-PRA-12：reqCharset 协议（设置请求字符集）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqcharset", "content": "httpbin.org/post reqCharset://gbk", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST -H "Content-Type: text/plain" -d 'test' http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqcharset
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象的 `Content-Type` 值包含 `charset=gbk`
- 代理修改了请求的字符集声明

---

### TC-PRA-13：reqDelay 协议（请求发送延迟）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqdelay", "content": "httpbin.org/get reqDelay://2000", "enabled": true}'
   ```
2. 执行命令并计时：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{time_total}" http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqdelay
   ```

**预期结果**：
- 总耗时至少 2 秒（`time_total >= 2.0`）
- 响应最终返回 HTTP 200
- 延迟发生在请求发送阶段

---

### TC-PRA-14：reqSpeed 协议（限制请求上传速度）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqspeed", "content": "httpbin.org/post reqSpeed://1", "enabled": true}'
   ```
2. 生成测试数据并执行命令计时：
   ```bash
   dd if=/dev/zero bs=1024 count=10 2>/dev/null | curl -x http://127.0.0.1:8800 -X POST -s -o /dev/null -w "%{time_total}" --data-binary @- http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqspeed
   ```

**预期结果**：
- 上传 10KB 数据时，总耗时明显增加（限速 1 kb/s，理论约 10 秒以上）
- 响应最终返回 HTTP 200

---

### TC-PRA-15：auth 协议（设置 HTTP Basic Auth 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-auth", "content": "httpbin.org/basic-auth/testuser/testpass auth://testuser:testpass", "enabled": true}'
   ```
2. 执行命令（不带认证信息）：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/basic-auth/testuser/testpass
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-auth
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体 JSON 包含 `"authenticated": true` 和 `"user": "testuser"`
- 代理自动注入了 Authorization Basic 头，无需客户端提供认证

---

### 二、响应修改协议

### TC-PRA-16：resCookies 协议（设置响应 Cookie）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rescookies", "content": "httpbin.org/get resCookies://(token=xyz789;path=/)", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Set-Cookie"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-rescookies
   ```

**预期结果**：
- 响应头中包含 `Set-Cookie`，值包含 `token=xyz789`
- 代理在响应中注入了 Cookie

---

### TC-PRA-17：resPrepend 协议（响应体前置内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resprepend", "content": "httpbin.org/html resPrepend://<!--INJECTED-->", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html | head -5
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resprepend
   ```

**预期结果**：
- 响应体以 `<!--INJECTED-->` 开头
- 原始 HTML 内容紧跟其后

---

### TC-PRA-18：resAppend 协议（响应体追加内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resappend", "content": "httpbin.org/html resAppend://<!--APPENDED-->", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html | tail -5
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resappend
   ```

**预期结果**：
- 响应体末尾包含 `<!--APPENDED-->`
- 原始 HTML 内容在前

---

### TC-PRA-19：resReplace 协议（响应体搜索替换）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resreplace", "content": "httpbin.org/html resReplace://Herman Melville/Bifrost Proxy/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resreplace
   ```

**预期结果**：
- 响应体中原本的 `Herman Melville` 被替换为 `Bifrost Proxy`
- HTML 结构保持完整

---

### TC-PRA-20：resType 协议（设置响应 Content-Type）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-restype", "content": "httpbin.org/get resType://text/plain", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Content-Type"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-restype
   ```

**预期结果**：
- 响应头中 `Content-Type` 值为 `text/plain`（而非原始的 `application/json`）
- 代理修改了响应的 Content-Type

---

### TC-PRA-21：resCharset 协议（设置响应字符集）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rescharset", "content": "httpbin.org/get resCharset://gbk", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Content-Type"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-rescharset
   ```

**预期结果**：
- 响应头中 `Content-Type` 值包含 `charset=gbk`
- 代理修改了响应的字符集声明

---

### TC-PRA-22：resSpeed 协议（限制响应下载速度）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resspeed", "content": "httpbin.org/bytes/10240 resSpeed://1", "enabled": true}'
   ```
2. 执行命令并计时：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{time_total}" http://httpbin.org/bytes/10240
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resspeed
   ```

**预期结果**：
- 下载 10KB 数据时，总耗时明显增加（限速 1 kb/s，理论约 10 秒以上）
- 响应最终返回 HTTP 200

---

### TC-PRA-23：attachment 协议（强制下载并指定文件名）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-attachment", "content": "httpbin.org/get attachment://response-data.json", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Content-Disposition"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-attachment
   ```

**预期结果**：
- 响应头中包含 `Content-Disposition: attachment; filename="response-data.json"`
- 浏览器访问时会触发下载而非内联显示

---

### TC-PRA-24：resMerge 协议（合并 JSON 到响应体）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resmerge", "content": "httpbin.org/get resMerge://{\"injected\":true,\"proxy\":\"bifrost\"}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resmerge
   ```

**预期结果**：
- 响应体 JSON 中包含原始字段（如 `url`、`headers`）
- 同时包含新增字段 `"injected": true` 和 `"proxy": "bifrost"`
- JSON 合并操作不破坏原始数据结构

---

### 三、内容注入协议

### TC-PRA-25：htmlAppend 协议（在 HTML </html> 前注入内容）

**操作步骤**：
1. 在单独终端启动本地 HTML 上游服务，并保持该终端运行：
   ```bash
   TMP_DIR="$(mktemp -d)"
   echo "$TMP_DIR" >/tmp/bifrost-htmlappend-upstream-dir
   cat > "$TMP_DIR/index.html" <<'HTML'
   <!doctype html><html><head><title>htmlAppend</title></head><body><main id="app">ORIGINAL_BODY</main></body></html>
   HTML
   cd "$TMP_DIR" && python3 -m http.server 18081
   ```
2. 通过 API 创建规则（使用 vConsole 风格的双 script 片段回归真实规则）：
   ```bash
   python3 - <<'PY'
   import json
   import textwrap
   import urllib.request

   content = textwrap.dedent("""\
   ``` vconsole-inject
   <script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>
   <script>new VConsole();</script>
   ```

   test-htmlappend.local host://127.0.0.1:18081
   test-htmlappend.local htmlAppend://{vconsole-inject}
   """)
   payload = json.dumps({
       "name": "test-htmlappend-body",
       "content": content,
       "enabled": True,
   }).encode()
   req = urllib.request.Request(
       "http://127.0.0.1:8800/_bifrost/api/rules",
       data=payload,
       headers={"Content-Type": "application/json"},
       method="POST",
   )
   print(urllib.request.urlopen(req).read().decode())
   PY
   ```
3. 执行命令并保存响应：
   ```bash
   curl -sS -x http://127.0.0.1:8800 http://test-htmlappend.local/index.html -o /tmp/bifrost-htmlappend-body.html
   python3 - <<'PY'
   from pathlib import Path
   html = Path("/tmp/bifrost-htmlappend-body.html").read_text()
   inject_start = '<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>'
   inject_end = '<script>new VConsole();</script>'
   assert "ORIGINAL_BODY" in html, html
   assert inject_start in html, html
   assert inject_end in html, html
   assert html.rindex(inject_start) < html.rindex(inject_end) < html.lower().rindex("</html>"), html
   assert not html.rstrip().endswith("</html>" + inject_start), html
   assert not html.rstrip().endswith("</html>" + inject_end), html
   PY
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-htmlappend-body
   # 在运行 http.server 的终端按 Ctrl+C 停止上游服务
   rm -f /tmp/bifrost-htmlappend-body.html
   rm -rf "$(cat /tmp/bifrost-htmlappend-upstream-dir)"
   rm -f /tmp/bifrost-htmlappend-upstream-dir
   ```

**预期结果**：
- 响应体 HTML 中 `</html>` 标签之前包含 vConsole 注入片段
- vConsole 注入片段不会出现在 `</html>` 之后
- 原始 HTML 内容保持不变

---

### TC-PRA-26：htmlPrepend 协议（在 HTML <html> 开始标签后注入内容）

**操作步骤**：
1. 在单独终端启动本地 HTML 上游服务，并保持该终端运行：
   ```bash
   TMP_DIR="$(mktemp -d)"
   echo "$TMP_DIR" >/tmp/bifrost-htmlprepend-upstream-dir
   cat > "$TMP_DIR/index.html" <<'HTML'
   <!doctype html><html><head><title>htmlPrepend</title></head><body><main id="app">ORIGINAL_BODY</main></body></html>
   HTML
   cd "$TMP_DIR" && python3 -m http.server 18083
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-htmlprepend", "content": "test-htmlprepend.local host://127.0.0.1:18083\ntest-htmlprepend.local htmlPrepend://(<bifrost-prepend-marker>)", "enabled": true}'
   ```
3. 执行命令并断言插入位置：
   ```bash
   curl -sS -x http://127.0.0.1:8800 http://test-htmlprepend.local/index.html -o /tmp/bifrost-htmlprepend.html
   python3 - <<'PY'
   from pathlib import Path
   html = Path("/tmp/bifrost-htmlprepend.html").read_text()
   marker = "<bifrost-prepend-marker>"
   lower = html.lower()
   assert marker in html, html
   assert lower.index("<html") < html.index(marker) < lower.rindex("</html>"), html
   assert not html.lstrip().startswith(marker), html
   assert "ORIGINAL_BODY" in html, html
   PY
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-htmlprepend
   rm -f /tmp/bifrost-htmlprepend.html
   rm -rf "$(cat /tmp/bifrost-htmlprepend-upstream-dir)"
   rm -f /tmp/bifrost-htmlprepend-upstream-dir
   ```

**预期结果**：
- 响应体 HTML 的 `<html>` 开始标签之后、`</html>` 之前包含 `<bifrost-prepend-marker>`
- 注入内容不会出现在 `<!doctype>` 或 `<html>` 之前
- 原始 HTML 内容保持不变

---

### TC-PRA-27：htmlBody 协议（替换 `<body>` 标签内部内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-htmlbody", "content": "httpbin.org/html htmlBody://<h1>Replaced by Bifrost</h1>", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/html
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-htmlbody
   ```

**预期结果**：
- 响应体保留原始 HTML 文档结构和 `<body>` 标签
- `<body>` 与 `</body>` 之间的原始内容被替换为 `<h1>Replaced by Bifrost</h1>`
- 原始 body 内部内容不再出现

---

### TC-PRA-28：jsAppend 协议（在 JS 内容末尾注入）

**操作步骤**：
1. 创建本地测试 JS 文件用的规则，匹配返回 JS 内容的 URL：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-jsappend", "content": "httpbin.org/get resType://application/javascript\nhttpbin.org/get resBody://var original=1;\nhttpbin.org/get jsAppend://var injected=2;", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-jsappend
   ```

**预期结果**：
- 响应体包含 `var original=1;`
- 响应体末尾包含 `var injected=2;`

---

### TC-PRA-29：cssAppend 协议（在 CSS 内容末尾注入）

**操作步骤**：
1. 创建规则，匹配返回 CSS 内容的 URL：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-cssappend", "content": "httpbin.org/get resType://text/css\nhttpbin.org/get resBody://body{color:black;}\nhttpbin.org/get cssAppend://body{background:red;}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-cssappend
   ```

**预期结果**：
- 响应体包含 `body{color:black;}`
- 响应体末尾包含 `body{background:red;}`

---

### TC-PRA-29A：html/js/css 内容注入协议按响应类型执行追加、前置、替换

**操作步骤**：
1. 在单独终端启动本地内容类型上游服务，并保持该终端运行：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           if self.path.endswith(".html"):
               content_type = "text/html"
               body = "<!doctype html><html><head><title>original</title></head><body>HTML_ORIGINAL</body></html>"
           elif self.path.endswith(".js"):
               content_type = "application/javascript"
               body = "window.original=1;"
           elif self.path.endswith(".css"):
               content_type = "text/css"
               body = ".original{color:black;}"
           else:
               self.send_response(404)
               self.end_headers()
               return
           data = body.encode()
           self.send_response(200)
           self.send_header("Content-Type", content_type)
           self.send_header("Content-Length", str(len(data)))
           self.end_headers()
           self.wfile.write(data)

   ThreadingHTTPServer(("127.0.0.1", 18082), Handler).serve_forever()
   PY
   ```
2. 通过 API 创建一组矩阵规则：
   ```bash
   python3 - <<'PY'
   import json
   import textwrap
   import urllib.request

   content = textwrap.dedent("""\
   matrix-html-append.local host://127.0.0.1:18082
   matrix-html-append.local htmlAppend://(<script>htmlAppend()</script>)
   matrix-html-prepend.local host://127.0.0.1:18082
   matrix-html-prepend.local htmlPrepend://(<!--HTML_PREPEND-->)
   matrix-html-body.local host://127.0.0.1:18082
   matrix-html-body.local htmlBody://(<main>HTML_REPLACED</main>)
   matrix-js-append.local host://127.0.0.1:18082
   matrix-js-append.local jsAppend://(window.appended=1;)
   matrix-js-prepend.local host://127.0.0.1:18082
   matrix-js-prepend.local jsPrepend://(window.prepended=1;)
   matrix-js-body.local host://127.0.0.1:18082
   matrix-js-body.local jsBody://(window.replaced=1;)
   matrix-css-append.local host://127.0.0.1:18082
   matrix-css-append.local cssAppend://(.appended{color:red;})
   matrix-css-prepend.local host://127.0.0.1:18082
   matrix-css-prepend.local cssPrepend://(:root{--prepended:1;})
   matrix-css-body.local host://127.0.0.1:18082
   matrix-css-body.local cssBody://(.replaced{color:red;})
   """)
   payload = json.dumps({
       "name": "test-content-injection-matrix",
       "content": content,
       "enabled": True,
   }).encode()
   req = urllib.request.Request(
       "http://127.0.0.1:8800/_bifrost/api/rules",
       data=payload,
       headers={"Content-Type": "application/json"},
       method="POST",
   )
   print(urllib.request.urlopen(req).read().decode())
   PY
   ```
3. 执行矩阵断言：
   ```bash
   python3 - <<'PY'
   import subprocess

   def body(host):
       return subprocess.check_output([
           "curl",
           "-sS",
           "-x",
           "http://127.0.0.1:8800",
           f"http://{host}/{host.split('-')[1]}.{host.split('-')[1]}",
       ]).decode()

   html_append = body("matrix-html-append.local")
   assert "<script>htmlAppend()</script>" in html_append, html_append
   assert html_append.rindex("<script>htmlAppend()</script>") < html_append.lower().rindex("</html>"), html_append
   html_prepend = body("matrix-html-prepend.local")
   assert html_prepend.startswith("<!doctype html><html><!--HTML_PREPEND-->"), html_prepend
   assert html_prepend.index("<!--HTML_PREPEND-->") > html_prepend.lower().index("<html>"), html_prepend
   assert html_prepend.index("<!--HTML_PREPEND-->") < html_prepend.lower().rindex("</html>"), html_prepend
   html_body = body("matrix-html-body.local")
   assert "<head><title>original</title></head>" in html_body, html_body
   assert html_body.index("<body><main>HTML_REPLACED</main>") < html_body.lower().rindex("</body>"), html_body
   assert "HTML_ORIGINAL" not in html_body, html_body

   assert body("matrix-js-append.local") == "window.original=1;window.appended=1;"
   assert body("matrix-js-prepend.local") == "window.prepended=1;window.original=1;"
   assert body("matrix-js-body.local") == "window.replaced=1;"
   assert body("matrix-css-append.local") == ".original{color:black;}.appended{color:red;}"
   assert body("matrix-css-prepend.local") == ":root{--prepended:1;}.original{color:black;}"
   assert body("matrix-css-body.local") == ".replaced{color:red;}"
   PY
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-content-injection-matrix
   # 在运行本地内容类型上游服务的终端按 Ctrl+C 停止服务
   ```

**预期结果**：
- `htmlAppend/htmlPrepend/htmlBody` 只在 `text/html` 响应上生效，分别完成 `</html>` 前追加、`<html>` 开始标签后前置、`<body>` 标签内部替换
- `jsAppend/jsPrepend/jsBody` 只在 JavaScript 响应上生效，分别完成末尾追加、开头前置、整段替换
- `cssAppend/cssPrepend/cssBody` 只在 CSS 响应上生效，分别完成末尾追加、开头前置、整段替换

---

### TC-PRA-29B：回归 - HTTPS 页面转发到 HTTP 上游后 htmlAppend 仍生效

**背景**：
- 已知失败请求：`REQ-69f08a65-002153` / sequence `1016158`
- 请求：`GET https://nextoncall.bytedance.net/assistant`
- 命中规则：`HtmlAppend` + `Http`
- 实际上游：`http://localhost:5173/assistant`
- 修复前现象：Traffic 记录显示命中 `HtmlAppend`，但响应体仍为原始 HTML，未插入 vConsole 脚本。

**操作步骤**：
1. 启动本地 HTML 上游服务，模拟 `localhost:5173`：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           body = """<!doctype html>
   <html lang="en">
     <head><title>Orca</title></head>
     <body>
       <div id="root"></div>
       <script type="module" src="/assistant/src/main.tsx"></script>
     </body>
   </html>"""
           data = body.encode()
           self.send_response(200)
           self.send_header("Content-Type", "text/html")
           self.send_header("Content-Length", str(len(data)))
           self.end_headers()
           self.wfile.write(data)

   ThreadingHTTPServer(("127.0.0.1", 18084), Handler).serve_forever()
   PY
   ```
2. 启动临时 Bifrost（独立数据目录、非 9900 端口、禁用系统代理）：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-human-https-htmlappend.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18884 --unsafe-ssl --no-system-proxy --skip-cert-check -y
   ```
3. 创建回归规则：
   ```bash
   curl -X POST http://127.0.0.1:18884/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-https-htmlappend-forwarded-http","content":"https://html-tunnel.local/ http://127.0.0.1:18084\nhttps://html-tunnel.local/ htmlAppend://(<script>httpsHtmlAppend()</script>)","enabled":true}'
   ```
4. 通过 HTTPS URL 访问代理：
   ```bash
   curl -k -sS -x http://127.0.0.1:18884 https://html-tunnel.local/assistant -o /tmp/htmlappend-tunnel.html
   python3 - <<'PY'
   body = open("/tmp/htmlappend-tunnel.html").read()
   marker = "<script>httpsHtmlAppend()</script>"
   assert marker in body, body
   assert body.rindex(marker) < body.lower().rindex("</html>"), body
   assert "<div id=\"root\"></div>" in body, body
   print("TC-PRA-29B passed")
   PY
   ```
5. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:18884/_bifrost/api/rules/test-https-htmlappend-forwarded-http
   # 停止临时 Bifrost 与本地 HTML 上游服务
   ```

**预期结果**：
- HTTPS 请求命中 `HtmlAppend` 和 `Http` 规则后，响应 HTML 中包含 `<script>httpsHtmlAppend()</script>`
- 注入脚本位于最后一个 `</html>` 之前
- 原始 HTML 主体仍存在

---

### TC-PRA-29D：回归 - gzip HTML 响应命中 htmlAppend 后仍保持有效压缩响应

**背景**：
- 已知失败请求：sequence `278778` / traffic id `REQ-69f0dc94-000857`
- 请求：`GET https://nextoncall.bytedance.net/?appType=all&page=1`
- 命中规则：`HtmlAppend` 内容注入规则
- 原始响应头：`Content-Encoding: gzip`、`Content-Type: text/html; charset=utf-8`
- 修复前现象：Traffic 缓存体表现为 gzip/二进制内容后直接拼接明文 vConsole 脚本，`gzip -t` 无法校验，浏览器也无法把响应作为正常 HTML 解析。

**操作步骤**：
1. 启动 gzip HTML 上游服务：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
   import gzip

   HTML = b'<!doctype html><html><head><title>gzip</title></head><body>page</body></html>'

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           data = gzip.compress(HTML)
           self.send_response(200)
           self.send_header("Content-Type", "text/html; charset=utf-8")
           self.send_header("Content-Encoding", "gzip")
           self.send_header("Content-Length", str(len(data)))
           self.end_headers()
           self.wfile.write(data)

   ThreadingHTTPServer(("127.0.0.1", 18086), Handler).serve_forever()
   PY
   ```
2. 启动临时 Bifrost（独立数据目录、非 9900 端口、禁用系统代理）：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-human-gzip-htmlappend.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy --skip-cert-check -y
   ```
3. 创建 gzip HTML 注入规则：
   ```bash
   curl -X POST http://127.0.0.1:18886/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-gzip-htmlappend","content":"gzip-html.local host://127.0.0.1:18086\ngzip-html.local htmlAppend://(<script>gzipHtmlAppend()</script>)","enabled":true}'
   ```
4. 通过代理请求 gzip HTML，并要求 curl 解压验证：
   ```bash
   curl --compressed -sS -D /tmp/gzip-htmlappend.headers \
     -x http://127.0.0.1:18886 \
     http://gzip-html.local/page.html \
     -o /tmp/gzip-htmlappend.html

   python3 - <<'PY'
   headers = open("/tmp/gzip-htmlappend.headers").read().lower()
   body = open("/tmp/gzip-htmlappend.html").read()
   marker = "<script>gzipHtmlAppend()</script>"
   assert "content-encoding: gzip" in headers, headers
   assert marker in body, body
   assert body.rindex(marker) < body.lower().rindex("</html>"), body
   assert "<body>page" in body, body
   print("TC-PRA-29D passed")
   PY
   ```
5. 更新规则为同时删除 `Content-Encoding`，验证最终响应头和响应体保持一致：
   ```bash
   curl -X PUT http://127.0.0.1:18886/_bifrost/api/rules/test-gzip-htmlappend \
     -H "Content-Type: application/json" \
     -d '{"name":"test-gzip-htmlappend","content":"gzip-html.local host://127.0.0.1:18086\ngzip-html.local htmlAppend://(<script>gzipHtmlAppend()</script>) delete://resHeaders.Content-Encoding","enabled":true}'

   curl -sS -D /tmp/gzip-htmlappend-identity.headers \
     -x http://127.0.0.1:18886 \
     http://gzip-html.local/page.html \
     -o /tmp/gzip-htmlappend-identity.html

   python3 - <<'PY'
   headers = open("/tmp/gzip-htmlappend-identity.headers").read().lower()
   body = open("/tmp/gzip-htmlappend-identity.html").read()
   marker = "<script>gzipHtmlAppend()</script>"
   assert "content-encoding:" not in headers, headers
   assert marker in body, body
   assert "<body>page" in body, body
   print("TC-PRA-29D identity fallback passed")
   PY
   ```
6. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:18886/_bifrost/api/rules/test-gzip-htmlappend
   # 停止临时 Bifrost 与 gzip HTML 上游服务
   ```

**预期结果**：
- 响应头仍包含 `Content-Encoding: gzip`
- `curl --compressed` 能成功解压响应体，不出现 gzip 校验或传输错误
- 解压后的 HTML 中包含 `<script>gzipHtmlAppend()</script>`
- 注入脚本位于最后一个 `</html>` 之前，原始 HTML 主体 `page` 仍存在（默认 badge 可能继续向 body 内注入面板样式和脚本）
- 当响应头规则删除 `Content-Encoding` 时，返回体为可直接读取的明文 HTML，不会保留 gzip 字节

---

### TC-PRA-29C：回归 - mock/file/template 生成资源也执行 HTML/JS/CSS 内容注入

**背景**：
- `file://`、`rawfile://`、`tpl://`、`statusCode` 等规则会在代理 handler 中直接生成响应并提前返回。
- 修复前这些立即响应不会经过普通上游响应的 `apply_content_injection`，因此可能出现 Traffic 显示命中 `htmlAppend/jsAppend/cssAppend`，但生成的资源体没有变化。

**操作步骤**：
1. 启动临时 Bifrost（独立数据目录、非 9900 端口、禁用系统代理）：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-human-mock-injection.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18885 --unsafe-ssl --no-system-proxy --skip-cert-check -y
   ```
2. 准备 mock 文件资源并创建注入规则：
   ```bash
   MOCK_DIR="$(mktemp -d /tmp/bifrost-human-mock-files.XXXXXX)"
   printf '<html><body>mock</body></html>' > "$MOCK_DIR/mock.html"
   printf 'window.mock=1;' > "$MOCK_DIR/mock.js"
   printf '.mock{color:black;}' > "$MOCK_DIR/mock.css"

   python3 - <<'PY' > /tmp/bifrost-mock-content-injection-rule.json
   import json, os
   mock_dir = os.environ["MOCK_DIR"]
   content = "\n".join([
       f"mock-html.local file://{mock_dir}/mock.html",
       "mock-html.local htmlAppend://(<script>mockHtml()</script>)",
       f"mock-js.local file://{mock_dir}/mock.js",
       "mock-js.local jsAppend://(window.mockAppend=1;)",
       f"mock-css.local file://{mock_dir}/mock.css",
       "mock-css.local cssAppend://(.mockAppend{color:red;})",
   ])
   print(json.dumps({
       "name": "test-mock-content-injection",
       "content": content,
       "enabled": True,
   }))
   PY

   curl -X POST http://127.0.0.1:18885/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     --data-binary @/tmp/bifrost-mock-content-injection-rule.json
   ```
3. 通过代理请求三类 mock 资源并断言：
   ```bash
   python3 - <<'PY'
   import subprocess

   proxy = "http://127.0.0.1:18885"

   def body(url):
       return subprocess.check_output(["curl", "-sS", "-x", proxy, url]).decode()

   html = body("http://mock-html.local/index.html")
   marker = "<script>mockHtml()</script>"
   assert marker in html, html
   assert html.rindex(marker) < html.lower().rindex("</html>"), html

   js = body("http://mock-js.local/app.js")
   assert js == "window.mock=1;window.mockAppend=1;", js

   css = body("http://mock-css.local/style.css")
   assert css == ".mock{color:black;}.mockAppend{color:red;}", css

   print("TC-PRA-29C passed")
   PY
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:18885/_bifrost/api/rules/test-mock-content-injection
   rm -rf "$MOCK_DIR" /tmp/bifrost-mock-content-injection-rule.json
   # 停止临时 Bifrost
   ```

**预期结果**：
- `file://` 生成的 HTML 响应会执行 `htmlAppend`，且注入位于 `</html>` 前
- `file://` 生成的 JavaScript 响应会执行 `jsAppend`
- `file://` 生成的 CSS 响应会执行 `cssAppend`

---

### TC-PRA-29E：回归 - 通配域名根路径 htmlAppend 匹配根路径和子路径

**背景**：
- 真实规则 `*.qq.com/ htmlAppend://{vconsole_inject}` 预期匹配单级 qq 子域名的根路径和所有子路径。
- 修复前 `WildcardMatcher` 会把尾部 `/` 和自动追加的 `(/.*)?` 叠加，导致 `https://news.qq.com/rain/a/20260428A07D9U00` 这类子路径没有命中；`https://www.qq.com` 这类无显式尾斜杠 URL 也可能漏匹配。

**操作步骤**：
1. 启动临时 HTML 上游服务：
   ```bash
   python3 - <<'PY'
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

   HTML = b'<!doctype html><html><head><title>qq</title></head><body>QQ_ORIGINAL</body></html>'

   class Handler(BaseHTTPRequestHandler):
       def do_GET(self):
           self.send_response(200)
           self.send_header("Content-Type", "text/html; charset=utf-8")
           self.send_header("Content-Length", str(len(HTML)))
           self.end_headers()
           self.wfile.write(HTML)

   ThreadingHTTPServer(("127.0.0.1", 18087), Handler).serve_forever()
   PY
   ```
2. 启动临时 Bifrost（独立数据目录、非 9900 端口、禁用系统代理）：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-human-wildcard-htmlappend.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy --skip-cert-check -y
   ```
3. 创建通配域名根路径 HTML 注入规则：
   ```bash
   python3 - <<'PY'
   import json
   import textwrap
   import urllib.request

   content = textwrap.dedent("""\
   ```vconsole_inject
   <script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>
   <script>new VConsole();</script>
   ```

   *.qq.com/ host://127.0.0.1:18087
   *.qq.com/ htmlAppend://{vconsole_inject}
   """)
   payload = json.dumps({
       "name": "test-wildcard-root-htmlappend",
       "content": content,
       "enabled": True,
   }).encode()
   req = urllib.request.Request(
       "http://127.0.0.1:18887/_bifrost/api/rules",
       data=payload,
       headers={"Content-Type": "application/json"},
       method="POST",
   )
   print(urllib.request.urlopen(req).read().decode())
   PY
   ```
4. 请求根路径和子路径并断言都已注入：
   ```bash
   python3 - <<'PY'
   import subprocess

   proxy = "http://127.0.0.1:18887"
   urls = [
       "http://www.qq.com",
       "http://www.qq.com/",
       "http://news.qq.com/rain/a/20260428A07D9U00",
   ]
   inject_start = '<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>'
   inject_end = '<script>new VConsole();</script>'

   for url in urls:
       html = subprocess.check_output(["curl", "-sS", "-x", proxy, url]).decode()
       assert "QQ_ORIGINAL" in html, (url, html)
       assert inject_start in html, (url, html)
       assert inject_end in html, (url, html)
       assert html.rindex(inject_start) < html.rindex(inject_end) < html.lower().rindex("</html>"), (url, html)

   print("TC-PRA-29E passed")
   PY
   ```
5. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:18887/_bifrost/api/rules/test-wildcard-root-htmlappend
   # 停止临时 Bifrost 与 HTML 上游服务
   ```

**预期结果**：
- `http://www.qq.com`、`http://www.qq.com/`、`http://news.qq.com/rain/a/20260428A07D9U00` 均返回 HTML。
- 三个响应都包含 vConsole 双 script 片段。
- 注入片段位于最后一个 `</html>` 之前，原始 `QQ_ORIGINAL` 内容仍存在。

---

### 四、控制协议

### TC-PRA-30：delete 协议（丢弃请求）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-delete", "content": "httpbin.org/delete delete://", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code}" --max-time 5 http://httpbin.org/delete
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-delete
   ```

**预期结果**：
- 请求被代理拦截并丢弃
- 返回错误状态码或连接被重置（不返回正常的 200 响应）

---

### TC-PRA-31：passthrough 协议（跳过处理直接透传）

**操作步骤**：
1. 先创建一个会修改响应的规则，再创建 passthrough 规则使特定路径跳过处理：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-passthrough", "content": "httpbin.org resHeaders://{X-Modified: true}\nhttpbin.org/ip passthrough://", "enabled": true}'
   ```
2. 执行被 passthrough 的请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/ip | grep "X-Modified"
   ```
3. 执行未被 passthrough 的请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Modified"
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-passthrough
   ```

**预期结果**：
- `/ip` 请求的响应头不包含 `X-Modified`（passthrough 跳过了所有规则处理）
- `/get` 请求的响应头包含 `X-Modified: true`（正常应用规则）

---

### TC-PRA-32：skip 协议（跳过剩余规则继续匹配）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-skip", "content": "httpbin.org/get resHeaders://{X-First: applied}\nhttpbin.org/get skip://\nhttpbin.org/get resHeaders://{X-Second: skipped}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -E "X-First|X-Second"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-skip
   ```

**预期结果**：
- 响应头包含 `X-First: applied`
- 响应头不包含 `X-Second`（skip 后的规则被跳过）

---

### TC-PRA-33：tlsIntercept 协议（强制 TLS 拦截特定域名）

**前置条件**：服务未使用 `--intercept` 全局拦截参数启动。

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-tlsintercept", "content": "httpbin.org tlsIntercept://\nhttpbin.org/get resHeaders://{X-TLS-Intercepted: true}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -k -sD - https://httpbin.org/get | grep "X-TLS-Intercepted"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-tlsintercept
   ```

**预期结果**：
- 响应头包含 `X-TLS-Intercepted: true`
- 即使未全局开启 `--intercept`，该域名的 HTTPS 流量也被拦截解密
- 需要 `-k` 跳过证书验证（Bifrost 使用自签 CA）

---

### TC-PRA-34：tlsPassthrough 协议（强制 TLS 透传）

**前置条件**：使用 `--intercept` 全局拦截参数重新启动服务：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept
```

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-tlspassthrough", "content": "example.com tlsPassthrough://", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - https://example.com/ 2>&1 | head -20
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-tlspassthrough
   ```

**预期结果**：
- 请求成功完成，TLS 证书为 example.com 的真实证书（而非 Bifrost 自签 CA）
- 即使全局开启了 `--intercept`，该域名的流量仍保持 TLS 透传

---

### 五、路由协议

### TC-PRA-35：proxy 协议（通过上游 HTTP 代理转发）

**操作步骤**：
1. 通过 API 创建规则（需要一个可用的上游代理，此处假设有一个可用代理）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-proxy", "content": "httpbin.org/get proxy://127.0.0.1:8800", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code}" http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-proxy
   ```

**预期结果**：
- 请求通过指定的上游代理服务器转发
- 返回正常响应

---

### TC-PRA-36：xhost 协议（转发但保留原始 Host 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-xhost", "content": "httpbin.org xhost://httpbin.org:80", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-xhost
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象的 `Host` 值为 `httpbin.org`（保留原始 Host 头）
- 请求被转发到指定目标地址

---

### TC-PRA-37：tpl 协议（模板文件响应，支持变量替换）

**操作步骤**：
1. 创建模板文件：
   ```bash
   echo '{"timestamp": "${now}", "method": "${method}", "url": "${url}", "clientIp": "${clientIp}"}' > /tmp/bifrost-test-tpl.json
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-tpl", "content": "httpbin.org/get tpl:///tmp/bifrost-test-tpl.json", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-tpl
   rm /tmp/bifrost-test-tpl.json
   ```

**预期结果**：
- 响应体为 JSON 格式
- `timestamp` 字段包含当前时间戳（非字面量 `${now}`）
- `method` 字段为 `GET`
- `url` 字段包含实际请求的 URL
- `clientIp` 字段包含客户端 IP

---

### TC-PRA-38：rawfile 协议（原始文件响应，不修改头）

**操作步骤**：
1. 创建测试文件：
   ```bash
   printf "HTTP/1.1 200 OK\r\nX-Custom-Raw: true\r\nContent-Type: text/plain\r\n\r\nRaw file response body" > /tmp/bifrost-test-raw.txt
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rawfile", "content": "httpbin.org/get rawfile:///tmp/bifrost-test-raw.txt", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-rawfile
   rm /tmp/bifrost-test-raw.txt
   ```

**预期结果**：
- 响应以原始文件内容直接返回
- 响应体包含 `Raw file response body`

---

### 六、脚本协议

### TC-PRA-39：reqScript 协议（执行请求脚本）

**操作步骤**：
1. 通过 API 创建请求脚本：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/scripts \
     -H "Content-Type: application/json" \
     -d '{"name": "test-req-script", "type": "req", "content": "module.exports = function(req) { req.headers[\"X-Script-Injected\"] = \"from-req-script\"; return req; };"}'
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqscript", "content": "httpbin.org/headers reqScript://test-req-script", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqscript
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/scripts/test-req-script
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 `"X-Script-Injected": "from-req-script"`
- 请求脚本在转发前修改了请求头

---

### TC-PRA-40：resScript 协议（执行响应脚本）

**操作步骤**：
1. 通过 API 创建响应脚本：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/scripts \
     -H "Content-Type: application/json" \
     -d '{"name": "test-res-script", "type": "res", "content": "module.exports = function(req, res) { res.headers[\"X-Res-Script\"] = \"modified-by-script\"; return res; };"}'
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resscript", "content": "httpbin.org/get resScript://test-res-script", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Res-Script"
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resscript
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/scripts/test-res-script
   ```

**预期结果**：
- 响应头中包含 `X-Res-Script: modified-by-script`
- 响应脚本在返回给客户端前修改了响应头

---

### 七、高级特性

### TC-PRA-41：Values 引用 — 在操作值中使用 {varName}

**操作步骤**：
1. 通过 API 创建 Value：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/values \
     -H "Content-Type: application/json" \
     -d '{"name": "mockResponse", "content": "{\"code\": 0, \"message\": \"mock from value\"}"}'
   ```
2. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-value-ref", "content": "httpbin.org/get resBody://{mockResponse}", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-value-ref
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/values/mockResponse
   ```

**预期结果**：
- 响应体为 `{"code": 0, "message": "mock from value"}`
- `{mockResponse}` 被替换为 Value 中定义的实际内容

---

### TC-PRA-42：模板字符串 — 使用反引号和 ${variable} 语法

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-template", "content": "httpbin.org/get resHeaders://`(X-Request-Method: ${method})`\nhttpbin.org/get resHeaders://`(X-Request-Id: ${randomUUID})`", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -E "X-Request-Method|X-Request-Id"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-template
   ```

**预期结果**：
- 响应头 `X-Request-Method` 的值为 `GET`（动态替换了 `${method}`）
- 响应头 `X-Request-Id` 的值为一个 UUID 格式字符串（动态替换了 `${randomUUID}`）

---

### TC-PRA-43：正则捕获组 — 使用 $1, $2 引用

**操作步骤**：
1. 通过 API 创建规则（使用正则匹配并捕获组）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-capture", "content": "/httpbin\\.org\\/status\\/(\\d+)/ resHeaders://`(X-Captured-Status: $1)`", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/status/418 | grep "X-Captured-Status"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-capture
   ```

**预期结果**：
- 响应头 `X-Captured-Status` 的值为 `418`
- 正则捕获组 `$1` 正确匹配并替换了 URL 中的状态码部分

---

### TC-PRA-44：规则优先级 — lineProps://important

**操作步骤**：
1. 创建第一个规则（普通优先级）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-priority-normal", "content": "httpbin.org/get resHeaders://{X-Priority: normal}", "enabled": true}'
   ```
2. 创建第二个规则（important 优先级）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-priority-important", "content": "httpbin.org/get statusCode://201 lineProps://important", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code}" http://httpbin.org/get
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-priority-normal
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-priority-important
   ```

**预期结果**：
- 状态码为 `201`
- `lineProps://important` 标记的规则优先级更高，覆盖了普通规则

---

### TC-PRA-45：内联值定义 — 在规则文件中使用 ``` 块

**操作步骤**：
1. 通过 API 创建规则（包含内联值定义）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-inline-value", "content": "``` inlineJson\n{\"inline\": true, \"source\": \"bifrost-rule\"}\n```\n\nhttpbin.org/get resBody://{inlineJson}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-inline-value
   ```

**预期结果**：
- 响应体为 `{"inline": true, "source": "bifrost-rule"}`
- 内联值 `inlineJson` 通过 ``` 块在规则文件中直接定义并被引用

---

### TC-PRA-46：urlReplace 协议（URL 路径替换）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-urlreplace", "content": "httpbin.org urlReplace://get/ip/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-urlreplace
   ```

**预期结果**：
- 响应体为 `httpbin.org/ip` 的响应内容（包含 `origin` 字段）
- URL 中的 `/get` 被替换为 `/ip`，请求实际访问了 `/ip` 端点

---

### TC-PRA-47：replaceStatus 协议（请求后替换状态码）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-replacestatus", "content": "httpbin.org/status/500 replaceStatus://200", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code}" http://httpbin.org/status/500
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-replacestatus
   ```

**预期结果**：
- 状态码为 `200`（而非上游返回的 `500`）
- 请求实际转发到了上游，但响应状态码被代理替换

---

### TC-PRA-48：forwardedFor 协议（设置 X-Forwarded-For 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-forwardedfor", "content": "httpbin.org/headers forwardedFor://10.0.0.1", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-forwardedfor
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 `"X-Forwarded-For": "10.0.0.1"`
- 代理注入了 X-Forwarded-For 头

---

### TC-PRA-49：headerReplace 协议（替换请求/响应头内容）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-headerreplace", "content": "httpbin.org/get headerReplace://application\\/json/text\\/plain/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Content-Type"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-headerreplace
   ```

**预期结果**：
- 响应头中 `Content-Type` 的值中 `application/json` 被替换为 `text/plain`
- headerReplace 协议对头部内容执行了搜索替换

---

### TC-PRA-50：多协议组合 — 请求修改 + 响应修改同时生效

**操作步骤**：
1. 通过 API 创建规则（单行组合多个协议）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-multi-protocol", "content": "httpbin.org/headers ua://MultiProtoTest/1.0 referer://https://test.bifrost.dev/ reqCookies://(multi=true) resHeaders://{X-Multi-Proto: combined} resCookies://(resp_token=abc)", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-multi-protocol
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含：
  - `"User-Agent": "MultiProtoTest/1.0"`
  - `"Referer": "https://test.bifrost.dev/"`
  - `"Cookie"` 包含 `multi=true`
- 响应头包含 `X-Multi-Proto: combined`
- 响应头包含 `Set-Cookie`，值包含 `resp_token=abc`

---

### TC-PRA-51：模板变量 — ${now}、${clientIp}、${url.hostname} 等

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-tpl-vars", "content": "httpbin.org/get resHeaders://`(X-Timestamp: ${now})`\nhttpbin.org/get resHeaders://`(X-Client-IP: ${clientIp})`\nhttpbin.org/get resHeaders://`(X-Host: ${url.hostname})`", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -E "X-Timestamp|X-Client-IP|X-Host"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-tpl-vars
   ```

**预期结果**：
- `X-Timestamp` 包含当前时间戳（数字格式）
- `X-Client-IP` 包含客户端 IP 地址（如 `127.0.0.1`）
- `X-Host` 值为 `httpbin.org`

---

### TC-PRA-52：协议别名 — ignore:// 等效于 passthrough://

**操作步骤**：
1. 通过 API 创建规则（使用 `ignore://` 旧语法，系统自动转换为 `passthrough://`）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-alias-ignore", "content": "httpbin.org resHeaders://{X-Should-Apply: true}\nignore://httpbin.org/ip", "enabled": true}'
   ```
2. 执行被忽略的请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/ip | grep "X-Should-Apply"
   ```
3. 执行未被忽略的请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Should-Apply"
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-alias-ignore
   ```

**预期结果**：
- `/ip` 请求的响应头不包含 `X-Should-Apply`（被 ignore/passthrough 跳过）
- `/get` 请求的响应头包含 `X-Should-Apply: true`

---

### TC-PRA-53：协议别名 — download:// 等效于 attachment://

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-alias-download", "content": "httpbin.org/get download://data.json", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Content-Disposition"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-alias-download
   ```

**预期结果**：
- 响应头包含 `Content-Disposition: attachment; filename="data.json"`
- `download://` 别名与 `attachment://` 行为完全一致

---

### TC-PRA-54：模板变量 — ${randomInt(min-max)} 随机整数

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-random-int", "content": "httpbin.org/get resHeaders://`(X-Random: ${randomInt(100-999)})`", "enabled": true}'
   ```
2. 多次执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Random"
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Random"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-random-int
   ```

**预期结果**：
- 每次请求的 `X-Random` 值为 100 到 999 之间的随机整数
- 多次请求返回不同的随机值（概率极高）

---

### TC-PRA-55：模板变量 — ${reqHeaders.xxx} 引用请求头

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqheader-ref", "content": "httpbin.org/get resHeaders://`(X-Echo-Accept: ${reqHeaders.accept})`", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -H "Accept: text/html" -sD - http://httpbin.org/get | grep "X-Echo-Accept"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqheader-ref
   ```

**预期结果**：
- 响应头 `X-Echo-Accept` 的值为 `text/html`
- 模板变量 `${reqHeaders.accept}` 正确引用了请求中的 Accept 头

---

### TC-PRA-56：trailers 协议（设置响应 Trailers）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-trailers", "content": "httpbin.org/get trailers://(X-Checksum:abc123)", "enabled": true}'
   ```
2. 执行命令（使用 HTTP/1.1 chunked transfer 或 HTTP/2）：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-trailers
   ```

**预期结果**：
- 在支持 trailers 的传输模式下，响应包含 trailer 头 `X-Checksum: abc123`
- 如果客户端不支持 trailers，行为降级但不影响正常响应

---

### TC-PRA-57：dns 协议（自定义 DNS 解析）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-dns", "content": "httpbin.org dns://8.8.8.8", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-dns
   ```

**预期结果**：
- 请求成功返回 HTTP 200
- 代理使用指定的 DNS 服务器（8.8.8.8）解析 httpbin.org 域名
- 响应内容与未使用自定义 DNS 时一致

---

### TC-PRA-58：responseFor 协议（设置 x-bifrost-response-for 头）

**操作步骤**：
1. 通过 API 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-responsefor", "content": "httpbin.org/get responseFor://mock-server-1", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "x-bifrost-response-for"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-responsefor
   ```

**预期结果**：
- 响应头中包含 `x-bifrost-response-for: mock-server-1`
- 该头标记了响应的来源标识

---

### TC-PRA-59：回归 - culture.shtml 经 HTTPS MITM 后背景图不再白屏

**背景**：
- 原始页面：`https://h5.news.qq.com/static/culture.shtml`
- 页面首屏视觉内容来自背景图：`https://mat1.gtimg.com/www/pics/hv1/62/63/2304/202309061523.png`
- 修复前现象：主文档加载完成，但背景 PNG 经 HTTPS MITM tunnel 传输约 180KB 后断流，curl 报 `HTTP/2 stream 1 was not closed cleanly: INTERNAL_ERROR` 或 `transfer closed with ... bytes remaining to read`，浏览器报 `net::ERR_HTTP2_PROTOCOL_ERROR`，页面视觉为空白。

**前置条件**：
- 使用临时 `BIFROST_DATA_DIR`，端口不得使用 9900。
- 启动 Bifrost 时必须带 `--no-system-proxy`。
- 不安装测试 CA 到系统证书；浏览器/CLI 通过忽略证书错误完成验证。
- 需要本机可访问 `h5.news.qq.com`、`mat1.gtimg.com`、`vm.gtimg.cn`。

**操作步骤**：
1. 启动隔离代理：
   ```bash
   TEST_DIR="$(mktemp -d /tmp/bifrost-human-culture.XXXXXX)"
   BIFROST_DATA_DIR="$TEST_DIR" \
   cargo run --bin bifrost -- start \
     -p 18892 \
     --host 127.0.0.1 \
     --unsafe-ssl \
     --no-system-proxy \
     --intercept-include 'h5.news.qq.com,mat1.gtimg.com,vm.gtimg.cn'
   ```
2. 如出现 CA 安装交互，选择 `No, skip (HTTPS interception may not work properly)`。
3. 使用 curl 通过代理下载背景 PNG：
   ```bash
   HTTPS_PROXY=http://127.0.0.1:18892 \
   HTTP_PROXY=http://127.0.0.1:18892 \
   curl -k -L -sS --http2 \
     -D /tmp/culture-image.headers \
     'https://mat1.gtimg.com/www/pics/hv1/62/63/2304/202309061523.png' \
     -o /tmp/culture-image.png
   wc -c /tmp/culture-image.png
   file /tmp/culture-image.png
   ```
4. 使用真实浏览器自动化通过代理打开页面，并记录背景图加载结果：
   ```bash
   NODE_PATH=/Users/eden/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules \
   /Users/eden/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node <<'NODE'
   const { chromium } = require('playwright');
   (async () => {
     const browser = await chromium.launch({
       headless: true,
       proxy: { server: 'http://127.0.0.1:18892' },
     });
     const context = await browser.newContext({
       ignoreHTTPSErrors: true,
       viewport: { width: 1280, height: 900 },
     });
     const page = await context.newPage();
     const failed = [];
     page.on('requestfailed', request => {
       failed.push({
         url: request.url(),
         error: request.failure()?.errorText,
       });
     });
     await page.goto('https://h5.news.qq.com/static/culture.shtml', {
       waitUntil: 'load',
       timeout: 45000,
     });
     await page.waitForTimeout(3000);
     const info = await page.evaluate(async () => {
       const box = document.querySelector('.box');
       const bg = box ? getComputedStyle(box).backgroundImage : '';
       const imageUrl = bg.match(/url\(["']?(.*?)["']?\)/)?.[1];
       const image = await new Promise(resolve => {
         const img = new Image();
         img.onload = () => resolve({
           ok: true,
           width: img.naturalWidth,
           height: img.naturalHeight,
         });
         img.onerror = () => resolve({
           ok: false,
           width: img.naturalWidth,
           height: img.naturalHeight,
         });
         img.src = imageUrl;
         setTimeout(() => resolve({
           ok: false,
           timeout: true,
           width: img.naturalWidth,
           height: img.naturalHeight,
         }), 5000);
       });
       return { title: document.title, bg, image };
     });
     await page.screenshot({ path: '/tmp/culture-proxy.png', fullPage: false });
     await browser.close();
     console.log(JSON.stringify({ info, failed }, null, 2));
     if (!info.image.ok || info.image.width !== 1800 || info.image.height !== 2544) {
       process.exit(1);
     }
     if (failed.some(item => item.url.includes('202309061523.png'))) {
       process.exit(1);
     }
   })();
   NODE
   ```

**预期结果**：
- curl 返回码为 0。
- `/tmp/culture-image.png` 大小为 `3457877` 字节，`file` 识别为 `PNG image data, 1800 x 2544`。
- 浏览器脚本输出 `image.ok=true`、`width=1800`、`height=2544`。
- `failed` 列表中不包含背景 PNG，不能出现 `net::ERR_HTTP2_PROTOCOL_ERROR`。
- 截图 `/tmp/culture-proxy.png` 不是空白白屏。

---

### TC-PRA-60：回归 - 上游 HTTP/2 响应体中途失败时不转发半截 body

**背景**：
- `TC-PRA-59` 暴露的是真实站点的大 PNG 在 HTTPS MITM tunnel 中经上游 HTTP/2 中途断流。
- 同类风险还包括普通 HTTP handler 通过规则转发到 HTTPS upstream 时，上游 HTTP/2 响应体已经开始发送但随后 reset；如果代理已经开始向客户端流式转发，就会出现资源不完整、浏览器渲染异常、curl 报传输错误或最终 502。

**前置条件**：
- 不使用 9900 端口，不修改系统代理。
- 使用测试内置的临时端口和自签名 TLS 上游，测试上游会在 HTTP/2 响应体发送 `partial` 后主动失败，并在 HTTP/1.1 重试时返回完整 body。

**操作步骤**：
1. 执行策略单元测试，确认 HTTP/2 响应体恢复策略覆盖小响应探测、大二进制/未知长度二进制 fallback、SSE/POST 跳过：
   ```bash
   cargo test -p bifrost-proxy h2_body_recovery_policy -- --nocapture
   ```
2. 执行集成回归，覆盖 HTTPS MITM tunnel 与普通 HTTP handler 转 HTTPS upstream 两条入口：
   ```bash
   cargo test -p bifrost-tests --test https_proxy_test retries_h2_body_failure -- --nocapture
   ```
3. 确认输出中包含：
   - `test_https_interception_retries_h2_body_failure_with_http1 ... ok`
   - `test_http_handler_retries_h2_body_failure_with_http1 ... ok`

**预期结果**：
- 策略单元测试 3 个用例全部通过。
- 集成回归 2 个用例全部通过。
- 两条集成回归都先命中一次 HTTP/2 upstream，再命中一次 HTTP/1.1 upstream fallback。
- 客户端最终收到完整 body，状态码为 200，不出现 502、`http2 error`、`connection error` 或半截 body。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
rm -f /tmp/bifrost-test-*.json /tmp/bifrost-test-*.txt
```
