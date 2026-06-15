# 规则合并 - reqHeaders/resHeaders 同名覆盖

## 功能模块说明

当多条 `reqHeaders://` 或 `resHeaders://` 规则匹配同一请求时，更具体的路径规则应该覆盖更宽泛路径规则的同名 header 值。不同名的 header 应该累积合并。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录）：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
```

2. 准备测试规则文本（通过 CLI 或 API 添加规则）

## 测试用例列表

### TC-RMH-01: reqHeaders 同名 header 被更具体路径覆盖（单元测试验证）

**操作步骤**：
1. 运行单元测试：
```bash
cargo test -p bifrost-cli -- test_later_reqheaders_rule_should_override_earlier_same_header --no-capture
```

**预期结果**：
- 测试通过
- `x-tt-env` 的值为 `ppe_fix_disabled_skill_loading`（来自更具体的 `/api/v1/` 规则）
- `x-use-ppe` 的值为 `1`（两条规则值相同，不冲突）

### TC-RMH-02: DomainMatcher 路径深度影响优先级（单元测试验证）

**操作步骤**：
1. 运行路径深度优先级测试：
```bash
cargo test -p bifrost-core -- test_priority_path_depth_exact --no-capture
cargo test -p bifrost-core -- test_priority_path_depth_prefix --no-capture
cargo test -p bifrost-core -- test_priority_specific_path_beats_root_with_protocol --no-capture
```

**预期结果**：
- 所有测试通过
- `example.com/api/v1/` (depth=2) 优先级高于 `example.com/api` (depth=1) 高于 `example.com/` (depth=0)
- `https://example.com/api/v1/` priority=122 > `https://example.com/` priority=120
- Prefix 路径同理：`/api/v1/*` priority > `/api/*` priority

### TC-RMH-03: 真实代理场景 - reqHeaders 通过 API 验证

**操作步骤**：
1. 启动服务后，通过 API 创建规则：
```bash
curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
  -H "Content-Type: application/json" \
  -d '{
    "name": "test-header-merge",
    "content": "`httpbin.org/` reqHeaders://{env1}\n`httpbin.org/get` reqHeaders://{env2}",
    "enabled": true
  }'
```
2. 通过 API 创建对应的 values：
```bash
curl -s -X POST http://127.0.0.1:8800/_bifrost/api/values \
  -H "Content-Type: application/json" \
  -d '{"name": "env1", "value": "x-test-header: value-from-root\nx-extra: extra-value"}'

curl -s -X POST http://127.0.0.1:8800/_bifrost/api/values \
  -H "Content-Type: application/json" \
  -d '{"name": "env2", "value": "x-test-header: value-from-specific"}'
```
3. 通过代理访问：
```bash
curl -x http://127.0.0.1:8800 http://httpbin.org/get 2>/dev/null | jq '.headers'
```

**预期结果**：
- `X-Test-Header` 值为 `value-from-specific`（具体路径 `/get` 规则覆盖根路径 `/` 规则）
- `X-Extra` 值为 `extra-value`（仅根路径规则设置，不冲突，应保留）

### TC-RMH-04: 转发类协议仍保持 first-match-wins

**操作步骤**：
1. 运行现有转发类测试确认无回归：
```bash
cargo test -p bifrost-core -- matcher::domain --no-capture
```

**预期结果**：
- 所有 domain matcher 测试通过
- 转发类协议（host://, http://, https://）行为未改变

### TC-RMH-05: 两条 reqHeaders 同名 key 覆盖 + 客户端请求也带同名 header（E2E 验证）

**操作步骤**：
1. 运行 E2E 测试：
```bash
cargo test -p bifrost-e2e -- test_reqheaders_same_key_override --no-capture
```

**预期结果**：
- 测试通过
- 远端 mock 服务只收到一个 `x-same-key` header
- 该 header 的值为 `second`（第二条规则覆盖第一条规则和客户端原始值）
- 客户端发送的 `X-Same-Key: client-original` 被规则覆盖，不会到达远端

**覆盖场景说明**：
- 两条规则：`reqHeaders://X-Same-Key=first` 和 `reqHeaders://X-Same-Key=second`
- 客户端请求自带 `X-Same-Key: client-original`
- 规则按顺序依次 insert 到 HeaderMap，后设置覆盖先设置
- 最终远端只收到一个 `X-Same-Key: second`

### TC-RMH-06: HTTPS passthrough/tunnel 下客户端同名 header 会被规则覆盖而不是重复发送

**操作步骤**：
1. 生成临时证书并启动本地 HTTPS 回显服务（服务会把 `X-Same-Key` 收到的全部值写回响应体）：
```bash
mkdir -p ./.bifrost-test-rmh/certs
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout ./.bifrost-test-rmh/certs/key.pem \
  -out ./.bifrost-test-rmh/certs/cert.pem \
  -days 1 -subj "/CN=127.0.0.1"

python3 - <<'PY'
import json
import ssl
from http.server import BaseHTTPRequestHandler, HTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        values = self.headers.get_all("X-Same-Key") or []
        body = json.dumps({
            "x_same_key_values": values,
            "x_same_key_count": len(values),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

httpd = HTTPServer(("127.0.0.1", 9443), Handler)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(
    "./.bifrost-test-rmh/certs/cert.pem",
    "./.bifrost-test-rmh/certs/key.pem",
)
httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
httpd.serve_forever()
PY
```
2. 使用临时数据目录启动 Bifrost（禁止使用 9900，且必须禁用系统代理）：
```bash
BIFROST_DATA_DIR=./.bifrost-test-rmh BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy \
  --rules "https://127.0.0.1:9443/ passthrough://" \
  --rules "https://127.0.0.1:9443/ reqHeaders://X-Same-Key=rule-value"
```
3. 发送一个本身已经带了同名 header 的请求：
```bash
curl -sk -x http://127.0.0.1:8800 https://127.0.0.1:9443/ \
  -H "X-Same-Key: client-original" \
  -H "Content-Type: application/json" \
  -d '{"ping":true}'
```
4. 检查回显服务响应：
```bash
curl -sk -x http://127.0.0.1:8800 https://127.0.0.1:9443/ \
  -H "X-Same-Key: client-original" \
  -H "Content-Type: application/json" \
  -d '{"ping":true}' | jq
```
5. 找到最新流量并检查请求详情：
```bash
cargo run --bin bifrost -- -p 8800 traffic list --limit 1
cargo run --bin bifrost -- -p 8800 traffic get <sequence>
```

**预期结果**：
- 回显响应中的 `x_same_key_count` 为 `1`
- 回显响应中的 `x_same_key_values` 仅包含 `rule-value`
- `traffic get` 的 `request_headers` 中 `x-same-key` 只出现 1 次
- 最终发往上游的值等于规则值，不会保留客户端原始重复值

### TC-RMH-07: reqHeaders JSON object 写法兼容 NextOncall PPE 头

**操作步骤**：
1. 运行语法校验和规则解析相关单元测试：
```bash
cargo test -p bifrost-core test_header_json_object_values_are_validated -- --nocapture
cargo test -p bifrost-core test_invalid_header_json_object_reports_e021 -- --nocapture
cargo test -p bifrost-cli json_object -- --nocapture
```
2. 运行请求修改 E2E 测试：
```bash
cargo run -p bifrost-e2e -- --test req_headers_json_object
```

**预期结果**：
- 合法规则 `reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1","x-tt-env-fe":"dev"}` 语法校验通过。
- malformed JSON header object 会返回 E021 语法错误，不会被当作 `{name}` 值引用。
- E2E mock upstream 收到 `x-tt-env: ppe_next_agent_new`、`x-use-ppe: 1`、`x-tt-env-fe: dev`。
- malformed JSON object 不会回退到旧冒号拆分路径生成非法 header name。

## 清理步骤

1. 停止本地 HTTPS 回显服务
2. 停止测试服务：`cargo run --bin bifrost -- stop -p 8800`
3. 删除临时数据目录：`rm -rf ./.bifrost-test ./.bifrost-test-rmh`
