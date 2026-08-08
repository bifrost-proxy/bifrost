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

### TC-RMH-08: reqHeaders 使用 `&` 写入多个独立 Header（用户反馈回归）

**操作步骤**：
1. 运行共享 Header parser 单元测试：
```bash
cargo test -p bifrost-core rule_headers -- --nocapture
```
2. 运行与用户截图配置等价的真实代理 E2E：
```bash
cargo run -p bifrost-e2e -- --test req_headers_ampersand_separated
cargo run -p bifrost-e2e -- --test res_headers_ampersand_separated
cargo run -p bifrost-e2e -- --test req_headers_value_ref_literal_ampersand
cargo run -p bifrost-e2e -- --test req_headers_referer_equals_url
cargo run -p bifrost-e2e -- --test req_headers_template_literal_ampersand
cargo run -p bifrost-e2e -- --test req_headers_json_scalar_template
```
3. 运行请求 Header 规则夹具：
```bash
(cd e2e-tests && \
  BIFROST_DISABLE_TRAY=1 \
  BIFROST_E2E_PROXY_READY_TIMEOUT=180 \
  BIFROST_DATA_DIR=./.bifrost-e2e-header-ampersand \
  ./test_rules.sh -p 18808 rules/request_modify/headers.txt)
```
4. 运行 Values Header 特殊字符兼容夹具，确认多 Header 断言不会截断值中的 `|`：
```bash
(cd e2e-tests && \
  BIFROST_DISABLE_TRAY=1 \
  BIFROST_E2E_PROXY_READY_TIMEOUT=180 \
  BIFROST_DATA_DIR=./.bifrost-e2e-header-values \
  ./test_rules.sh -p 18809 rules/template/values.txt)
```

**预期结果**：
- `reqHeaders://(x-tt-env:ppe_doubao_connect_lark&x-flow-env=ppe_doubao_connect_lark&x-use-ppe=1)`
  混用 `:` 与 `=` 时被解析为三个独立 Header，mock upstream 分别收到 `x-tt-env: ppe_doubao_connect_lark`、
  `x-flow-env: ppe_doubao_connect_lark`、`x-use-ppe: 1`。
- `x-tt-env` 的值不包含 `&x-flow-env=...` 或 `&x-use-ppe=...`。
- `resHeaders://(X-Header-A=value-a&X-Header-B=value-b)` 同样产生两个独立响应 Header。
- JSON、多行 Values 和单 Header 旧写法继续通过同一组回归测试。
- 单行引用 Value `X-Query: a=1&b=2` 保持一个 Header，字面 `&` 不被拆分。
- `reqHeaders://Referer=https://example.test/` 保留完整 URL 值。
- Values 中 `X-Symbols: !@#$%^*()_+-[]{}|` 的尾部 `|` 被完整保留并由夹具准确断言。
- `${url}` 展开出的 `?a=1&b=2` 完整保留在 `X-Full-Url` 中；客户端
  `X-Source: safe&X-Injected=yes` 经 `${reqHeaders.x-source}` 复制后仍只有 `X-Copied`
  一个规则生成的 Header，不会额外生成 `X-Injected`。
- JSON map 中未加引号的标量模板会在解析字段前展开，真实 upstream 收到
  `x-now: 42`；JSON 字符串模板输出会被安全转义，不能注入额外 Header 字段。

### TC-RMH-09: WebUI 有效性分析识别 `&` 分隔 Header

**操作步骤**：
1. 先运行 effectiveness 单元回归：
```bash
pnpm --dir web exec vitest run src/utils/ruleEffectiveness.test.ts
```
2. 启动 WebUI 测试环境，在 Chrome 打开 `/_bifrost/traffic`。测试环境的
   `/rules/active-summary` 返回以下合并规则：
```text
https://partial.example.test/api/ reqHeaders://(x-env=one&x-stable=keep)
https://partial.example.test/api/ reqHeaders://x-env=two
```
3. 点击 Rules 状态胶囊，展开 `Merged Rules`，检查两行的状态、文本和悬浮提示。

**预期结果**：
- 第一条规则 `reqHeaders://(x-env=one&x-stable=keep)` 被识别为两个独立字段。
- 后续同 matcher 的 `reqHeaders://x-env=two` 只覆盖 `x-env`，第一条规则显示 partial，
  `x-stable` 仍保持有效。
- Chrome 中 Merged Rules 对第一行渲染 `data-effect-status="partial"`，行文本完整保留
  `x-stable=keep`；悬浮提示说明 `x-env` 由后续同 matcher 规则写入。

### TC-RMH-10: reqCookies、resCookies 与 trailers 使用 `&` 拆分多个字段

**操作步骤**：
1. 运行 source-aware KV parser 与三个协议的 resolver 单元测试：
```bash
cargo test -p bifrost-core rule_key_values_split_inline_ampersands_but_not_referenced_values
cargo test -p bifrost-core cookie_and_trailer_templates_expand_after_authored_separators_are_parsed
cargo test -p bifrost-core response_cookie_json_with_attributes_stays_structured
cargo test -p bifrost-cli test_req_cookies_ampersand_separated
cargo test -p bifrost-cli test_res_cookies_ampersand_and_json_attributes
cargo test -p bifrost-cli test_trailers_ampersand_separated
```
2. 逐个运行真实代理 E2E：
```bash
cargo run -p bifrost-e2e -- --test req_cookies_ampersand_separated
cargo run -p bifrost-e2e -- --test req_cookies_value_ref_literal_ampersand
cargo run -p bifrost-e2e -- --test req_cookies_template_literal_ampersand
cargo run -p bifrost-e2e -- --test res_cookies_ampersand_separated
cargo run -p bifrost-e2e -- --test res_cookies_with_attrs
cargo run -p bifrost-e2e -- --test trailers_ampersand_separated
```

**预期结果**：
- `reqCookies://(sessionid=xxx&a=c&b=two=parts)` 生成三个请求 Cookie，`b` 的值保留第二个 `=`。
- `resCookies://(sid=xxx&theme=dark)` 生成两条独立 `Set-Cookie`。
- `resCookies` JSON 属性对象继续产生 `Max-Age`、`Secure`、`HttpOnly` 等属性。
- `trailers://(X-Trace=abc&X-Checksum=xyz)` 宣告两个独立 Trailer 名。
- Values 引用中的 `session=safe&injected=yes` 保持为一个 Cookie 值，不生成 `injected` Cookie。
- 模板展开产生的 `&` 同样只属于当前字段值，不能注入新 Cookie 或 Trailer。
- rules 文件中的单行 `{value}` 在 parser 阶段继续保留 Value 引用来源，不能在 resolver
  之前被展平成可再次解释的内联语法。
- 请求 Cookie fixture 在没有可选 `jq` 时仍通过已检测的 Python 3 执行值断言，不能只检查
  HTTP 状态码后误报通过。
- 响应 Cookie fixture 断言完整 `name=value`，引用值中的 `&` 不得被截断或产生额外 Cookie。

## 清理步骤

1. 停止本地 HTTPS 回显服务
2. 停止测试服务：`cargo run --bin bifrost -- stop -p 8800`
3. 删除临时数据目录：`rm -rf ./.bifrost-test ./.bifrost-test-rmh e2e-tests/.bifrost-e2e-header-ampersand e2e-tests/.bifrost-e2e-header-values`

## 本次执行记录（2026-08-09）

- TC-RMH-08：PASS。相关独立真实代理 E2E 全部 `1/1 passed`；模板边界用例确认
  `X-Full-Url=http://test.local/api?a=1&b=2` 和
  `X-Copied=safe&X-Injected=yes`，且不存在额外 `X-Injected` Header。请求 Header
  夹具复跑结果 `20/20 passed`，Values 夹具复跑结果 `69/69 passed`；JSON 标量模板
  真实 E2E `1/1 passed`，core 防字段注入回归同时通过。
- TC-RMH-09：AUTOMATION PASS / CHROME BLOCKED。effectiveness Vitest 结果 `12/12 passed`；
  Chromium Playwright 真实页面回归 `1/1 passed`，在 `/_bifrost/traffic` 展开 Dynamic Island
  后第一条 `&` 分隔规则为 partial、文本保留 `x-stable=keep`，悬浮提示确认 `x-env` 由
  后续同 matcher 规则写入。尝试连接用户 Chrome 时浏览器扩展不可用，因此尚不能把仓库
  要求的 Chrome 人工执行标为 PASS；需启用 Settings → Computer use 的 Chrome 扩展后补跑。
- TC-RMH-10：PASS。core、CLI 和 Admin 的 8 条定向单测全部通过；6 条真实代理
  E2E 均为 `1/1 passed`。代理日志确认请求 Cookie 为 `sessionid=xxx`、`a=c`、
  `b=two=parts`，Values 引用只生成 `session=safe&injected=yes` 一个 Cookie；响应输出
  `sid=xxx`、`theme=dark` 两条 Set-Cookie，JSON 属性 Cookie 保留 Max-Age/Secure/HttpOnly，
  Trailer 为 `X-Trace, X-Checksum`。review 后追加 parser/CLI 来源保留单测并通过；请求 Cookie、
  响应 Cookie、Trailer 三组真实 fixture 分别为 `12/12`、`31/31`、`4/4`，引用值中的
  `&` 均未生成额外字段。额外隐藏 `jq` 后，请求 Cookie fixture 明确提示 JSON 断言降级，
  但 Python 3 Cookie 值断言仍全部执行并得到 `12/12`。
