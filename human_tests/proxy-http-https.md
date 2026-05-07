# HTTP/HTTPS 代理核心功能测试用例

## 功能模块说明

验证 Bifrost 作为 HTTP/HTTPS 代理服务器的核心功能，包括基本代理转发、HTTPS CONNECT 隧道、TLS 拦截、规则匹配（host/file/redirect/statusCode/resHeaders/reqHeaders/resBody/reqBody/delay/cache/resCors 等）以及多种模式匹配（正则、通配符、域名、IP/CIDR）。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保端口 8800 未被占用
3. 部分测试需要通过 API 创建规则，API 地址为 `http://127.0.0.1:8800/_bifrost/api/rules`

---

## 测试用例

### TC-PHT-01：HTTP 代理基本转发（curl -x）

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "http://httpbin.org/get"`
- 响应头中包含 `Content-Type: application/json`

---

### TC-PHT-02：HTTPS 代理 CONNECT 隧道

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "https://httpbin.org/get"`
- TLS 握手通过 CONNECT 隧道完成，证书由目标服务器提供

---

### TC-PHT-03：HTTPS 代理 TLS 拦截模式（--intercept）

**前置条件**：重新启动服务，添加 `--intercept` 参数：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept
```

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -k https://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "https://httpbin.org/get"`
- 使用 `-k` 跳过证书验证（因为 Bifrost 会用自签 CA 重新签发证书）
- 在管理端 Traffic 页面可以看到该 HTTPS 请求的详细信息（请求头、响应体等）

---

### TC-PHT-04：代理配合 --unsafe-ssl 转发自签证书站点

**操作步骤**：
1. 创建一个使用自签证书的 HTTPS 服务（或使用已知自签证书的站点）
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -k https://self-signed.badssl.com/
   ```

**预期结果**：
- 代理成功转发请求，不因上游证书不可信而拒绝连接
- 返回目标站点的 HTML 内容

---

### TC-PHT-05：代理正确返回状态码

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -o /dev/null -s -w "%{http_code}" http://httpbin.org/status/404
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -o /dev/null -s -w "%{http_code}" http://httpbin.org/status/500
   ```

**预期结果**：
- 第一个命令返回 `404`
- 第二个命令返回 `500`
- 代理透传上游服务的原始状态码

---

### TC-PHT-06：代理保留请求和响应头

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -H "X-Custom-Header: test-value" http://httpbin.org/headers
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 `"X-Custom-Header": "test-value"`
- 代理正确传递了自定义请求头到上游服务器

---

### TC-PHT-07：代理处理大响应体

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -o /dev/null -s -w "%{size_download} %{http_code}" http://httpbin.org/bytes/1048576
   ```

**预期结果**：
- 下载大小约为 `1048576` 字节（1MB）
- 状态码为 `200`
- 代理正确处理大响应体，无截断或损坏

---

### TC-PHT-08：HTTP/2 代理转发

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -k --http2 https://httpbin.org/get -v 2>&1 | grep -i "HTTP/"
   ```

**预期结果**：
- 请求成功完成
- 返回 HTTP 200 状态码

---

### TC-PHT-09：host 规则（转发到指定主机）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-host", "content": "httpbin.org 127.0.0.1:8800", "enabled": true}'
   ```
   > 注意：这里仅为示例，实际应转发到一个可用的目标地址。如果本地有 HTTP 服务运行在其他端口（如 3000），可改为 `httpbin.org 127.0.0.1:3000`。
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 测试结束后删除规则：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-host
   ```

**预期结果**：
- 请求被转发到规则指定的目标主机
- 响应内容来自目标主机而非原始 httpbin.org

---

### TC-PHT-10：file 规则（返回本地文件内容作为响应）

**操作步骤**：
1. 创建测试文件：
   ```bash
   echo '{"mock": true, "message": "hello from file"}' > /tmp/bifrost-test-mock.json
   ```
2. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-file", "content": "httpbin.org/get file:///tmp/bifrost-test-mock.json", "enabled": true}'
   ```
3. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-file
   rm /tmp/bifrost-test-mock.json
   ```

**预期结果**：
- 响应体为 `{"mock": true, "message": "hello from file"}`
- 请求未实际转发到 httpbin.org，而是直接返回本地文件内容

---

### TC-PHT-11：redirect 规则（URL 重定向）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-redirect", "content": "httpbin.org/get redirect://https://example.com/", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code} %{redirect_url}" http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-redirect
   ```

**预期结果**：
- 状态码为 `302`（或 `301`，取决于默认行为）
- `redirect_url` 为 `https://example.com/`
- 响应包含 `Location: https://example.com/` 头

---

### TC-PHT-12：statusCode 规则（直接返回指定状态码）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-statuscode", "content": "httpbin.org/get statusCode://403", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{http_code}" http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-statuscode
   ```

**预期结果**：
- 状态码为 `403`
- 请求未实际转发到上游，直接返回指定状态码

---

### TC-PHT-13：resHeaders 规则（修改响应头）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resheaders", "content": "httpbin.org/get resHeaders://{X-Bifrost-Test: injected-value}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "X-Bifrost-Test"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resheaders
   ```

**预期结果**：
- 响应头中包含 `X-Bifrost-Test: injected-value`
- 原始响应内容不受影响

---

### TC-PHT-14：reqHeaders 规则（修改请求头）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqheaders", "content": "httpbin.org/headers reqHeaders://{X-Injected-By: bifrost-proxy}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqheaders
   ```

**预期结果**：
- 响应体 JSON 中 `headers` 对象包含 `"X-Injected-By": "bifrost-proxy"`
- 代理在转发前注入了自定义请求头

---

### TC-PHT-15：resBody 规则（替换响应体）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-resbody", "content": "httpbin.org/get resBody://{\"replaced\": true}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-resbody
   ```

**预期结果**：
- 响应体为 `{"replaced": true}`
- 原始响应体被完全替换

---

### TC-PHT-16：reqBody 规则（设置请求体）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-reqbody", "content": "httpbin.org/post reqBody://{\"injected\": \"body\"}", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -X POST http://httpbin.org/post
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-reqbody
   ```

**预期结果**：
- 响应体 JSON 中 `data` 字段包含 `{"injected": "body"}`
- 代理在转发前设置了请求体内容

---

### TC-PHT-17：delay 规则（延迟响应）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-delay", "content": "httpbin.org/get resDelay://2000", "enabled": true}'
   ```
2. 执行命令并计时：
   ```bash
   curl -x http://127.0.0.1:8800 -s -o /dev/null -w "%{time_total}" http://httpbin.org/get
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-delay
   ```

**预期结果**：
- 总耗时至少 2 秒（`time_total >= 2.0`）
- 响应最终返回 HTTP 200
- 延迟由代理注入，而非上游服务器延迟

---

### TC-PHT-18：cache 规则（设置缓存控制）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-cache", "content": "httpbin.org/get cache://3600", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Cache-Control"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-cache
   ```

**预期结果**：
- 响应头中包含 `Cache-Control`，值包含 `max-age=3600`
- 代理注入了缓存控制头

---

### TC-PHT-19：resCors 规则（添加 CORS 响应头）

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rescors", "content": "httpbin.org/get resCors://", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -i "Access-Control"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-rescors
   ```

**预期结果**：
- 响应头中包含 `Access-Control-Allow-Origin: *`
- 可能还包含其他 CORS 相关头（如 `Access-Control-Allow-Methods`、`Access-Control-Allow-Headers`）

---

### TC-PHT-20：同一模式下多条规则叠加

**操作步骤**：
1. 通过 API 创建规则文件（包含多条规则）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-multi-rules", "content": "httpbin.org/get resHeaders://{X-Rule-A: value-a}\nhttpbin.org/get resHeaders://{X-Rule-B: value-b}\nhttpbin.org/get resCors://", "enabled": true}'
   ```
2. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep -iE "X-Rule-A|X-Rule-B|Access-Control"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-multi-rules
   ```

**预期结果**：
- 响应头中同时包含 `X-Rule-A: value-a`、`X-Rule-B: value-b`
- 响应头中包含 `Access-Control-Allow-Origin: *`
- 多条规则同时生效，互不干扰

---

### TC-PHT-21：正则表达式模式匹配

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-regex", "content": "/httpbin\\.org\\/status\\/\\d+/ resHeaders://{X-Regex-Match: true}", "enabled": true}'
   ```
2. 执行匹配请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/status/200 | grep "X-Regex-Match"
   ```
3. 执行不匹配请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Regex-Match"
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-regex
   ```

**预期结果**：
- 第一个请求（`/status/200`）响应头包含 `X-Regex-Match: true`
- 第二个请求（`/get`）响应头不包含 `X-Regex-Match`
- 正则模式正确匹配 URL

---

### TC-PHT-22：通配符模式匹配

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-wildcard", "content": "*.httpbin.org resHeaders://{X-Wildcard: matched}", "enabled": true}'
   ```
2. 执行匹配请求（假设有子域名可用，或使用本地 hosts 映射）：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://www.httpbin.org/get | grep "X-Wildcard"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-wildcard
   ```

**预期结果**：
- 子域名请求（如 `www.httpbin.org`）响应头包含 `X-Wildcard: matched`
- 通配符 `*` 匹配单级子域名

---

### TC-PHT-23：域名精确匹配

**操作步骤**：
1. 通过 API 创建规则文件：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-domain", "content": "httpbin.org resHeaders://{X-Domain-Match: exact}", "enabled": true}'
   ```
2. 执行匹配请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://httpbin.org/get | grep "X-Domain-Match"
   ```
3. 执行不匹配请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://www.httpbin.org/get | grep "X-Domain-Match"
   ```
4. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-domain
   ```

**预期结果**：
- `httpbin.org` 请求响应头包含 `X-Domain-Match: exact`
- `www.httpbin.org` 请求响应头不包含该头（精确匹配不含子域名）

---

### TC-PHT-24：IP/CIDR 模式匹配

**操作步骤**：
1. 通过 API 创建规则文件（使用 IP 模式匹配）：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-ip-cidr", "content": "127.0.0.0/8 resHeaders://{X-IP-Match: cidr}", "enabled": true}'
   ```
2. 执行匹配请求：
   ```bash
   curl -x http://127.0.0.1:8800 -sD - http://127.0.0.1:8800/_bifrost/api/rules | grep "X-IP-Match"
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-ip-cidr
   ```

**预期结果**：
- 发往 `127.0.0.0/8` 网段的请求响应头包含 `X-IP-Match: cidr`
- CIDR 模式正确匹配 IP 范围

---

### TC-PHT-25：host 规则路径前缀回归（避免重复拼接）

**前置条件**：
1. 启动 Bifrost，使用临时数据目录、非 `9900` 端口，并开启 TLS 拦截：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-host-path cargo run --bin bifrost -- --port 8800 start --unsafe-ssl --intercept --no-system-proxy
   ```
2. 启动本地 echo 服务：
   ```bash
   HTTP_PORT=3000 HTTPS_PORT=3443 WS_PORT=3020 WSS_PORT=3021 SSE_PORT=3003 PROXY_PORT=9999 bash e2e-tests/mock_servers/start_servers.sh start-bg
   ```
3. 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-host-path-prefix","content":"https://ejt9lgzgu9.feishu-boe.cn/labor_cost/static/ http://127.0.0.1:3000/labor_cost/static/","enabled":true}'
   ```

**操作步骤**：
1. 执行命令：
   ```bash
   curl -x http://127.0.0.1:8800 -k "https://ejt9lgzgu9.feishu-boe.cn/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg?v=1"
   ```
2. 检查响应 JSON 中的 `request.parsed_path` 和 `request.query_string`
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-host-path-prefix
   HTTP_PORT=3000 HTTPS_PORT=3443 WS_PORT=3020 WSS_PORT=3021 SSE_PORT=3003 PROXY_PORT=9999 bash e2e-tests/mock_servers/start_servers.sh stop
   rm -rf ./.bifrost-test-host-path
   ```

**预期结果**：
- 请求返回 `200`
- 上游收到的 `request.parsed_path` 必须等于：
  ```text
  /labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
  ```
- 上游收到的 `request.query_string` 必须等于 `v=1`
- 路径中不能出现：
  ```text
  /labor_cost/static/labor_cost/static/
  ```

---

### TC-PHT-26：旧版 `^https://` path wildcard 规则兼容回归

**前置条件**：
1. 启动本地 approvals mock 服务：
   ```bash
   python3 -c 'from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(("mock approvals " + self.path).encode())
HTTPServer(("127.0.0.1", 8999), H).serve_forever()'
   ```
2. 启动 Bifrost，使用临时数据目录、非 `9900` 端口、禁止系统代理，并仅拦截目标 CDN 域名：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-path-wildcard cargo run --bin bifrost -- start -p 8891 --unsafe-ssl --no-system-proxy --intercept-include cdn-tos-cn.bytedance.net
   ```
3. 创建包含旧版 `^https://` path wildcard 的规则：
   ```bash
   curl -X POST http://127.0.0.1:8891/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-legacy-https-path-wildcard","content":"^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html http://127.0.0.1:8999/approvals","enabled":true}'
   ```

**操作步骤**：
1. 执行旧规则对应的真实 HTTPS URL：
   ```bash
   curl -k -x http://127.0.0.1:8891 "https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html"
   ```
2. 查看最新 traffic 详情，确认请求命中规则：
   ```bash
   bifrost traffic search "approvals-web/1.0.0.3505/index.html" --limit 5
   bifrost traffic get <SEQ>
   ```
3. 清理：
   ```bash
   curl -X DELETE http://127.0.0.1:8891/_bifrost/api/rules/test-legacy-https-path-wildcard
   rm -rf ./.bifrost-test-path-wildcard
   ```

**预期结果**：
- curl 返回 `mock approvals /approvals`
- traffic 详情中 `actual_url` 为 `http://127.0.0.1:8999/approvals`
- traffic 详情中 `has_rule_hit` 为 `true`
- `matched_rules` 包含原始规则：
  ```text
  ^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html http://127.0.0.1:8999/approvals
  ```
- 旧版 `^https://` path wildcard 规则无需改写即可命中 HTTPS URL

---

### TC-PHT-27：host 规则精确路径回归（不补尾斜杠）

**前置条件**：
1. 使用当前工作区源码构建最新 Bifrost release 二进制：
   ```bash
   cargo build --release --bin bifrost
   ```
2. 执行 host rule path rewrite E2E 脚本。脚本会使用临时数据目录、非 `9900` 端口、`--no-system-proxy`，启动本地 echo 服务，并创建精确路径规则：
   ```text
   https://ejt9lgzgu9.feishu-boe.cn/labor_cost/static/__webpack_hmr http://127.0.0.1:<echo_port>/__webpack_hmr
   ```

**操作步骤**：
1. 执行命令：
   ```bash
   PROXY_PORT=18881 ECHO_HTTP_PORT=13081 bash e2e-tests/tests/test_host_rule_path_rewrite.sh
   ```
2. 检查脚本输出中 `HTTPS exact host rule path rewrite without trailing slash` 小节。
3. 确认上游 echo 服务记录的 `request.parsed_path`。

**预期结果**：
- 请求返回 `200`
- 上游收到的 `request.parsed_path` 必须精确等于：
  ```text
  /__webpack_hmr
  ```
- 上游收到的 `request.parsed_path` 不能等于：
  ```text
  /__webpack_hmr/
  ```
- 脚本同时验证原有路径前缀场景仍通过，避免修复精确路径时破坏 TC-PHT-25

**执行记录**：
- 2026-05-07：PASS。执行 `cargo build --release --bin bifrost` 后运行 `PROXY_PORT=18881 ECHO_HTTP_PORT=13081 bash e2e-tests/tests/test_host_rule_path_rewrite.sh`。脚本返回 0，HMR 精确路径请求返回 200，上游 `request.parsed_path` 精确为 `/__webpack_hmr`，未变成 `/__webpack_hmr/`；原有路径前缀场景也通过。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test .bifrost-test-path-wildcard .bifrost-e2e-host-path-*
```
