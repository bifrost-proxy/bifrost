# Bifrost 规则端到端测试框架

本目录包含用于测试 Bifrost 代理规则的端到端测试框架。

## 设计原则

**测试即需求规范** - 测试用例定义了代理服务应该具备的能力，测试失败表明代理服务存在缺陷需要修复，而非测试问题。

## 目录结构

```
tests/                      # E2E / Admin API / 回归测试脚本
│   ├── test_frames_admin_api.sh    # Frames API 测试 (11 tests)
│   ├── test_rules_admin_api.sh     # Rules API 测试 (10 tests)
│   ├── test_values_admin_api.sh    # Values API 测试 (9 tests)
│   ├── test_whitelist_admin_api.sh # Whitelist API 测试 (14 tests)
│   ├── test_cert_admin_api.sh      # Cert API 测试 (7 tests)
│   ├── test_proxy_admin_api.sh     # Proxy API 测试 (6 tests)
│   ├── test_system_admin_api.sh    # System API 测试 (10 tests)
│   ├── test_scripts_admin_api.sh   # Scripts API 测试 (12 tests)
│   ├── test_tls_logic_simple.sh    # TLS 逻辑测试脚本
│   └── test_tls_intercept_e2e.sh   # TLS 拦截 E2E 测试脚本

mock_servers/               # Echo 服务器 (返回请求详情用于验证)
│   ├── http_echo_server.py     # HTTP Echo 服务器
│   ├── https_echo_server.py    # HTTPS Echo 服务器 (自签名证书)
│   ├── ws_echo_server.py       # WebSocket Echo 服务器
│   └── start_servers.sh        # 服务器管理脚本

test_utils/                 # 测试工具库
│   ├── assert.sh               # 断言库
│   ├── http_client.sh          # HTTP 请求封装
│   └── admin_client.sh         # Admin API 客户端封装

rules/                      # 规则测试用例 (按类别组织)
    ├── forwarding/             # 转发测试
    │   ├── http_to_http.txt        # HTTP→HTTP 转发
    │   ├── https_to_http.txt       # HTTPS→HTTP (TLS 终止)
    │   ├── http_to_https.txt       # HTTP→HTTPS (TLS 建立)
    │   └── ws_forward.txt          # WebSocket 转发
    │
    ├── request_modify/         # 请求修改测试
    │   ├── headers.txt             # 请求头增删改
    │   ├── method.txt              # 请求方法修改
    │   ├── ua.txt                  # User-Agent 修改
    │   ├── referer.txt             # Referer 修改
    │   └── cookies.txt             # 请求 Cookie 修改
    │
    ├── response_modify/        # 响应修改测试
    │   ├── status.txt              # 状态码修改
    │   ├── headers.txt             # 响应头增删改
    │   ├── cookies.txt             # Set-Cookie 设置
    │   ├── cors.txt                # CORS 头添加
    │   └── delay.txt               # 请求/响应延迟
    │
    ├── redirect/               # 重定向测试
    │   └── redirect.txt            # 301/302 重定向
    │
    ├── priority/               # 规则优先级测试
    │   ├── exact_vs_wildcard.txt   # 精确匹配 vs 通配符
    │   ├── wildcard_level.txt      # 通配符层级
    │   ├── order.txt               # 规则顺序优先级
    │   └── ip_vs_cidr.txt          # IP vs CIDR 匹配
    │
    └── control/                # 控制规则测试
        ├── ignore.txt              # passthrough 透传规则（兼容旧 ignore 命名）
        └── filter.txt              # 过滤规则
```

## 前置条件

- Rust 工具链 (用于编译 Bifrost)
- Python 3 (用于运行 Echo 服务器)
- curl (用于发送 HTTP 请求)
- jq (可选，用于 JSON 断言)
- lsof (用于检测端口占用)
- macOS 系统代理权限（仅 macOS）：启用/关闭系统代理可能需要管理员权限。建议在终端使用 sudo 运行 CLI：`sudo bifrost start --system-proxy --proxy-bypass "localhost,127.0.0.1,::1,*.local"`。非管理员运行时，CLI 会提示是否通过 sudo 授权设置系统代理；选择授权后终端将出现密码提示，由系统处理。

## 快速开始

```bash
# 运行真正的统一 E2E 入口（规则 + shell + bifrost-e2e runner + Playwright）
bash ../scripts/run_all_e2e.sh

# CI 全量入口
bash ../scripts/run_all_e2e.sh --ci

# 只跑规则夹具套件
./run_all_tests_parallel.sh -c forwarding

# 运行单个规则文件
./test_rules.sh rules/forwarding/http_to_http.txt

# 运行单个 shell Admin API 测试
./tests/test_rules_admin_api.sh      # 运行 Rules API 测试
./tests/test_values_admin_api.sh     # 运行 Values API 测试
./tests/test_whitelist_admin_api.sh  # 运行 Whitelist API 测试

# 运行 bifrost-e2e 自定义 runner
cargo run -p bifrost-e2e -- --list

# 运行 Web Playwright E2E
pnpm --dir ../web run test:ui
```

## test_rules.sh - 单文件测试

```bash
# 基本用法
./test_rules.sh <规则文件>

# 示例
./test_rules.sh rules/forwarding/http_to_http.txt
./test_rules.sh rules/request_modify/headers.txt

# 指定代理端口
./test_rules.sh -p 9090 rules/redirect/redirect.txt

# 跳过编译步骤
./test_rules.sh --no-build rules/forwarding/http_to_http.txt

# 测试完成后保持代理运行 (用于调试)
./test_rules.sh --keep-proxy rules/forwarding/http_to_http.txt
```

### 选项

| 选项              | 说明                      |
| ----------------- | ------------------------- |
| `-h, --help`      | 显示帮助信息              |
| `-p, --port PORT` | 指定代理端口 (默认: 8080) |
| `-l, --list`      | 列出所有可用的规则文件    |
| `--no-build`      | 跳过编译步骤              |
| `--keep-proxy`    | 测试完成后保持代理运行    |

## run_all_tests_parallel.sh - 规则批量测试

```bash
# 运行所有规则文件
./run_all_tests_parallel.sh

# 只运行指定分类
./run_all_tests_parallel.sh -c forwarding
./run_all_tests_parallel.sh -c request_modify
./run_all_tests_parallel.sh -c response_modify

# 详细输出
./run_all_tests_parallel.sh -v
```

### 选项

| 选项                 | 说明                               |
| -------------------- | ---------------------------------- |
| `-h, --help`         | 显示帮助信息                       |
| `-j, --jobs N`       | 并行任务数                         |
| `-c, --category CAT` | 只运行指定分类的规则文件           |
| `--no-build`         | 跳过 release 二进制编译            |
| `--base-port PORT`   | 并行执行时的起始代理端口           |
| `-v, --verbose`      | 详细输出                           |

### 可用分类

- `forwarding` - 转发测试 (HTTP/HTTPS/WebSocket)
- `request_modify` - 请求修改测试
- `response_modify` - 响应修改测试
- `redirect` - 重定向测试
- `priority` - 规则优先级测试
- `control` - 控制规则测试

## Admin API 测试

Admin API 测试脚本位于 `tests/` 目录，用于测试 Bifrost 的管理 API 接口。

### 测试脚本列表

| 脚本                          | 测试数 | 覆盖 API                             |
| ----------------------------- | ------ | ------------------------------------ |
| `test_frames_admin_api.sh`    | 11     | 流量帧列表、详情、WebSocket 连接管理 |
| `test_rules_admin_api.sh`     | 10     | 规则 CRUD、启用/禁用                 |
| `test_values_admin_api.sh`    | 9      | 预定义值 CRUD                        |
| `test_whitelist_admin_api.sh` | 14     | 白名单管理、授权管理、临时白名单     |
| `test_cert_admin_api.sh`      | 7      | 证书信息、下载、二维码               |
| `test_proxy_admin_api.sh`     | 6      | 系统代理状态、设置                   |
| `test_system_admin_api.sh`    | 10     | 系统信息、概览、指标历史             |
| `test_scripts_admin_api.sh`   | 12     | 脚本 CRUD、列表、内置脚本            |

### 运行 Admin API 测试

```bash
cd e2e-tests

# 运行单个测试脚本
./tests/test_rules_admin_api.sh

# 运行 CI shell 套件（由统一入口代管）
bash ../scripts/run_all_e2e.sh --ci --skip-rules --skip-runner --skip-ui
```

## 统一入口

仓库当前存在 4 类端到端资产：

- `e2e-tests/rules/**`：规则夹具，通过 `test_rules.sh` / `run_all_tests_parallel.sh` 执行
- `e2e-tests/tests/test_*.sh`：shell 端到端与 Admin API 场景
- `crates/bifrost-e2e`：Rust 自定义 E2E runner
- `web/tests/ui/*.spec.ts`：Playwright UI E2E

建议统一使用仓库根目录的脚本：

```bash
# 本地默认：规则 + 稳定 shell + bifrost-e2e + Playwright
bash scripts/run_all_e2e.sh

# CI 模式：规则 + 全量 shell（按平台跳过不适用脚本）+ bifrost-e2e + Playwright
bash scripts/run_all_e2e.sh --ci

# 本地扩展 shell 覆盖
bash scripts/run_all_e2e.sh --full-shell
```

说明：

- `--ci` 会在流水线中执行规则夹具、shell 套件、`bifrost-e2e` runner 和 Playwright；shell 脚本会按平台跳过明确不适用的场景，例如 macOS 专属系统代理脚本
- `--full-shell` 会在本地尝试更广的 shell 套件；默认本地入口仍保留稳定 shell 集合作为更快的日常回归
- 旧文档中的 `run_all_tests.sh` 已废弃，实际可执行入口为 `run_all_tests_parallel.sh` 和 `scripts/run_all_e2e.sh`

### Admin API 客户端工具

`test_utils/admin_client.sh` 提供 Admin API 的封装函数，可在自定义测试中使用：

```bash
source "$SCRIPT_DIR/test_utils/admin_client.sh"

# 示例：测试规则 CRUD
rule_id=$(create_rule '{"name":"test","content":"*.test.com http://localhost"}' | jq -r '.id')
get_rule "$rule_id"
update_rule "$rule_id" '{"name":"updated"}'
delete_rule "$rule_id"
```

## 规则文件约定

- 规则行为相关的测试规则必须存放在 `rules/` 下，不能直接写死在 `tests/` 中
- 所有 shell 测试脚本统一放在 `tests/`；`e2e-tests/scripts/` 已移除
- `rules/` 下按模块功能分目录，再按测试目标拆分 `.txt` 文件
- 每个规则文件顶部先写测试目标说明，再写规则内容
- 若脚本需要按运行时端口渲染规则，可在规则文件中使用占位符，由脚本读取后替换，但规则本体仍应保存在 `rules/`

## Echo 服务器

Echo 服务器返回 JSON 格式的请求详情，便于验证代理行为：

```bash
# 手动启动服务器
./mock_servers/start_servers.sh start

# 后台启动
./mock_servers/start_servers.sh start-bg

# 停止服务器
./mock_servers/start_servers.sh stop

# 查看状态
./mock_servers/start_servers.sh status
```

### Echo 服务器响应格式

```json
{
  "request": {
    "method": "GET",
    "path": "/test",
    "headers": {
      "Host": "example.com",
      "User-Agent": "curl/8.0"
    },
    "body": "",
    "cookies": {}
  },
  "server": {
    "protocol": "HTTP",
    "port": 3000,
    "tls_info": null
  },
  "timestamp": "2024-01-01T00:00:00Z"
}
```

### 默认端口

| 服务                  | 端口 | 环境变量          |
| --------------------- | ---- | ----------------- |
| HTTP Echo             | 3000 | `ECHO_HTTP_PORT`  |
| HTTPS Echo            | 3443 | `ECHO_HTTPS_PORT` |
| WebSocket Echo        | 3020 | `ECHO_WS_PORT`    |
| WebSocket Secure Echo | 3021 | `ECHO_WSS_PORT`   |

### 启动命令可选参数（系统代理）

- 通过环境变量控制脚本在启动代理时传入 CLI 选项：
  - ENABLE_SYSTEM_PROXY=true 启用系统代理（对应 CLI `--system-proxy`）
  - SYSTEM_PROXY_BYPASS=localhost,127.0.0.1,::1,\*.local 设置绕过列表（对应 CLI `--proxy-bypass`）
  - 例如：
    - `ENABLE_SYSTEM_PROXY=true SYSTEM_PROXY_BYPASS="localhost,127.0.0.1,::1,*.local" ./test_rules.sh rules/forwarding/http_to_http.txt`
    - `ENABLE_SYSTEM_PROXY=true PROXY_PORT=9900 ./test_pattern.sh`

## 断言库

`test_utils/assert.sh` 提供丰富的断言函数：

### HTTP 状态码断言

```bash
assert_status "200" "$HTTP_STATUS" "请求应成功"
assert_status_2xx "$HTTP_STATUS" "应返回成功状态码"
assert_status_3xx "$HTTP_STATUS" "应返回重定向状态码"
assert_status_4xx "$HTTP_STATUS" "应返回客户端错误"
assert_status_5xx "$HTTP_STATUS" "应返回服务器错误"
```

### 响应头断言

```bash
assert_header_exists "Content-Type" "$HTTP_HEADERS"
assert_header_not_exists "X-Removed" "$HTTP_HEADERS"
assert_header_value "X-Custom" "expected-value" "$HTTP_HEADERS"
assert_header_contains "Content-Type" "json" "$HTTP_HEADERS"
```

### 响应体断言

```bash
assert_body_equals "$expected" "$HTTP_BODY"
assert_body_contains "keyword" "$HTTP_BODY"
assert_body_not_contains "forbidden" "$HTTP_BODY"
assert_body_matches "pattern.*regex" "$HTTP_BODY"
```

### JSON 断言 (需要 jq)

```bash
assert_json_field ".request.method" "GET" "$HTTP_BODY"
assert_json_field_exists ".request.headers" "$HTTP_BODY"
```

### 后端验证断言

```bash
assert_backend_received_header "X-Custom" "value" "$HTTP_BODY"
assert_backend_received_method "POST" "$HTTP_BODY"
assert_backend_received_path "/api/test" "$HTTP_BODY"
assert_backend_protocol "HTTP" "$HTTP_BODY"
```

## 规则文件格式

规则文件使用简单的文本格式，每行一条规则：

```
# 这是注释
pattern protocol://target
```

### 操作符值格式

操作符后的值支持以下格式：

| 格式             | 能否包含空格 | 说明                       | 示例                                |
| ---------------- | ------------ | -------------------------- | ----------------------------------- |
| `{name}`         | ✓            | 引用值，内容定义在代码块中 | `ua://{mobile_ua}`                  |
| `` `template` `` | ✓            | 模板字符串，支持变量替换   | `` resHeaders://`X-Id: ${reqId}` `` |
| `(content)`      | ✗            | 内联值，直接使用字面内容   | `file://({"ok":true})`              |
| 简单值           | ✗            | 普通值                     | `statusCode://200`                  |
| 文件路径         | -            | 从本地文件加载             | `file:///path/to/file`              |
| URL              | -            | 从远程加载                 | `resBody://https://...`             |

**重要**: 如果值包含空格，必须使用 `{name}` 引用值方式。

````
# 错误 - 值包含空格会导致解析错误
test.local ua://Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X)

# 正确 - 使用引用值
test.local ua://{mobile_ua}
``` mobile_ua
Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X) AppleWebKit/605.1.15
````

### 支持的协议类型

| 协议            | 说明            | 示例                                    |
| --------------- | --------------- | --------------------------------------- |
| `http://`       | HTTP 转发       | `example.com http://127.0.0.1:3000`     |
| `https://`      | HTTPS 转发      | `example.com https://127.0.0.1:3443`    |
| `host://`       | 保持原协议转发  | `example.com host://localhost:8080`     |
| `ws://`         | WebSocket 转发  | `example.com ws://127.0.0.1:3020`       |
| `wss://`        | WebSocket (TLS) | `example.com wss://127.0.0.1:3021`      |
| `redirect://`   | 302 重定向      | `old.com redirect://https://new.com`    |
| `reqHeaders://` | 修改请求头      | `example.com reqHeaders://{...}`        |
| `resHeaders://` | 修改响应头      | `example.com resHeaders://{...}`        |
| `statusCode://` | 修改状态码      | `example.com statusCode://201`          |
| `method://`     | 修改请求方法    | `example.com method://POST`             |
| `ua://`         | 修改 User-Agent | `example.com ua://CustomUA`             |
| `referer://`    | 修改 Referer    | `example.com referer://https://ref.com` |
| `reqDelay://`   | 请求延迟 (ms)   | `example.com reqDelay://500`            |
| `resDelay://`   | 响应延迟 (ms)   | `example.com resDelay://1000`           |
| `resCors://`    | 添加 CORS 头    | `example.com resCors://*`               |
| `reqCookies://` | 修改请求 Cookie | `example.com reqCookies://{...}`        |
| `resCookies://` | 设置响应 Cookie | `example.com resCookies://{...}`        |

## 测试流程

脚本执行以下步骤：

1. **检查依赖** - 验证 curl, python3, jq 等工具
2. **编译代理** - 自动编译最新的 Bifrost 二进制文件
3. **启动 Echo 服务器** - 启动 HTTP/HTTPS/WebSocket Echo 服务器
4. **启动代理** - 使用指定规则文件启动 Bifrost
5. **执行测试** - 发送请求并验证代理行为
6. **输出结果** - 显示通过/失败的断言统计

## 环境变量

| 变量              | 说明                  | 默认值      |
| ----------------- | --------------------- | ----------- |
| `PROXY_PORT`      | 代理服务器监听端口    | `8080`      |
| `PROXY_HOST`      | 代理服务器主机地址    | `127.0.0.1` |
| `ECHO_HTTP_PORT`  | HTTP Echo 服务器端口  | `3000`      |
| `ECHO_HTTPS_PORT` | HTTPS Echo 服务器端口 | `3443`      |
| `ECHO_WS_PORT`    | WebSocket Echo 端口   | `3020`      |
| `ECHO_WSS_PORT`   | WebSocket Secure 端口 | `3021`      |

## 添加新测试

### 1. 在现有分类中添加规则

```bash
# 编辑现有规则文件
vim rules/forwarding/http_to_http.txt

# 添加新规则
echo "new-test.local http://127.0.0.1:3000" >> rules/forwarding/http_to_http.txt
```

### 2. 创建新的测试文件

```bash
# 创建新规则文件
cat > rules/forwarding/custom_forward.txt << 'EOF'
# 自定义转发测试
custom-domain.local http://127.0.0.1:3000
EOF

# 运行测试
./test_rules.sh rules/forwarding/custom_forward.txt
```

### 3. 创建新的测试分类

```bash
# 创建新分类目录
mkdir -p rules/my_category

# 创建测试文件
cat > rules/my_category/test.txt << 'EOF'
# My Category Tests
test.local http://127.0.0.1:3000
EOF

# 运行该分类的所有规则测试
./run_all_tests_parallel.sh -c my_category
```

## 故障排查

### 遗留进程清理

测试运行后可能会有 mock server 或代理进程未正确清理，导致后续测试失败或端口占用。运行新测试前建议先清理遗留进程：

```bash
# 清理所有 mock server 和代理进程
pkill -f "http_echo_server.py" 2>/dev/null
pkill -f "https_echo_server.py" 2>/dev/null
pkill -f "ws_echo_server.py" 2>/dev/null
pkill -f "sse_echo_server.py" 2>/dev/null
pkill -f "bifrost" 2>/dev/null

# 或者使用一行命令
pkill -f "echo_server.py"; pkill -f "bifrost"

# 检查进程是否清理干净
ps aux | grep -E "(echo_server|bifrost)" | grep -v grep
```

**提示**: 如果测试脚本因异常退出（如 Ctrl+C 中断），mock server 进程可能会残留。建议在每次运行测试前执行上述清理命令。

### 代理启动失败

```bash
# 检查端口占用
lsof -i :8080

# 使用其他端口
./test_rules.sh -p 9090 rules/forwarding/http_to_http.txt
```

### Echo 服务器启动失败

```bash
# 检查 Python 环境
python3 --version

# 手动启动查看错误
python3 mock_servers/http_echo_server.py

# 检查端口占用
lsof -i :3000
```

### 测试超时

```bash
# 增加 TIMEOUT 环境变量
TIMEOUT=30 ./test_rules.sh rules/forwarding/http_to_http.txt
```

## check_rules.py - 规则语法检查

检查规则文件中操作符后的值是否符合语法要求。

```bash
# 检查所有规则文件
python3 check_rules.py

# 检查单个文件
python3 check_rules.py rules/request_modify/ua.txt

# 仅显示错误
python3 check_rules.py --errors-only
```

### 检测的问题

| 问题类型     | 示例                       | 说明              |
| ------------ | -------------------------- | ----------------- |
| 值包含空格   | `ua://Mozilla/5.0 (iPhone` | 空格导致值被截断  |
| 内联值空格   | `file://(hello world)`     | `()` 内不能有空格 |
| 括号不匹配   | `resBody://{incomplete`    | 引用值括号未闭合  |
| 反引号不匹配 | ``ua://`template``         | 模板字符串未闭合  |

### 选项

| 选项               | 说明                         |
| ------------------ | ---------------------------- |
| `-h, --help`       | 显示帮助信息                 |
| `--errors-only`    | 仅显示错误，不显示通过的文件 |
| `--base-path PATH` | 指定规则文件基础路径         |

### 输出示例

```
检查规则文件...

✗ rules/request_modify/ua.txt:11
   错误: 值中包含未闭合的括号 (，可能因空格被截断，请使用引用值 {name}
   当前: ua://Mozilla/5.0(iPhone
   建议: 定义引用值 {ua_value} 并使用 ua://{ua_value}

──────────────────────────────────────────────────
检查完成: 39 个文件
  ✓ 通过: 38 个
  ✗ 错误: 1 个 (1 处问题)
```
