# 代理管理 Admin API 测试用例

## 功能模块说明

验证 Bifrost 代理管理相关的 Admin API 接口，包括系统代理状态查询与控制、CLI 代理状态、代理地址信息查询、代理二维码生成等功能。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保端口 8800 未被占用
3. 服务启动成功后再执行测试用例

---

## 测试用例

### TC-APR-01：获取系统代理状态

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/proxy/system | jq .
   ```

**预期结果**：
- 返回 HTTP 200，响应为 JSON 格式
- JSON 包含以下字段：
  - `supported`：布尔值，表示当前平台是否支持系统代理（macOS 下为 `true`）
  - `enabled`：布尔值，表示系统代理是否已启用
  - `host`：字符串，系统代理主机地址（启用时通常为 `127.0.0.1`）
  - `port`：数字，系统代理端口
  - `bypass`：字符串，代理绕过规则

---

### TC-APR-02：启用系统代理

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/system \
     -H "Content-Type: application/json" \
     -d '{"enabled": true}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200，响应为 JSON 格式
- 返回的 JSON 中 `enabled` 为 `true`
- `host` 为 `127.0.0.1`
- `port` 为 `8800`
- `bypass` 包含默认绕过规则（如 `localhost,127.0.0.1,::1`），且默认不包含 `*.local`
- 系统代理已实际生效（可通过系统设置验证）

---

### TC-APR-03：启用系统代理并自定义绕过规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/system \
     -H "Content-Type: application/json" \
     -d '{"enabled": true, "bypass": "localhost,127.0.0.1,::1,*.local,*.example.com"}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- `bypass` 字段包含自定义的绕过规则 `*.example.com`

---

### TC-APR-04：禁用系统代理

**前置条件**：已通过 TC-APR-02 启用系统代理

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/system \
     -H "Content-Type: application/json" \
     -d '{"enabled": false}' | jq .
   ```

**预期结果**：
- 返回 HTTP 200
- `enabled` 为 `false`
- 系统代理已实际关闭

---

### TC-APR-05：设置系统代理请求体无效

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/system \
     -H "Content-Type: application/json" \
     -d '{"invalid": "data"}' -w "\n%{http_code}"
   ```

**预期结果**：
- 返回 HTTP 400（Bad Request）
- 响应包含 JSON 解析错误信息

---

### TC-APR-06：获取系统代理平台支持状态

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/proxy/system/support | jq .
   ```

**预期结果**：
- 返回 HTTP 200，响应为 JSON 格式
- JSON 包含以下字段：
  - `supported`：布尔值（macOS 下为 `true`）
  - `platform`：字符串，当前平台名称（macOS 下为 `macOS`）

---

### TC-APR-07：获取 CLI 代理状态

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/proxy/cli | jq .
   ```

**预期结果**：
- 返回 HTTP 200，响应为 JSON 格式
- JSON 包含以下字段：
  - `enabled`：布尔值，表示 CLI Shell 代理是否已配置
  - `shell`：字符串，当前 Shell 类型（如 `zsh`、`bash`）
  - `config_files`：字符串数组，Shell 配置文件路径列表
  - `proxy_url`：字符串，格式为 `http://127.0.0.1:8800`

---

### TC-APR-08：获取代理地址信息

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/proxy/address | jq .
   ```

**预期结果**：
- 返回 HTTP 200，响应为 JSON 格式
- JSON 包含以下字段：
  - `port`：数字，值为 `8800`
  - `local_ips`：字符串数组，包含本机局域网 IP
  - `addresses`：对象数组，每个元素包含：
    - `ip`：字符串，IP 地址
    - `address`：字符串，格式为 `<ip>:8800`
    - `qrcode_url`：字符串，格式为 `/_bifrost/public/proxy/qrcode?ip=<encoded_ip>`
    - `is_preferred`：布尔值，是否为首选地址

---

### TC-APR-09：获取代理二维码（默认 Host）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -sI http://127.0.0.1:8800/_bifrost/public/proxy/qrcode
   ```
2. 执行以下命令查看 SVG 内容：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/public/proxy/qrcode | head -5
   ```

**预期结果**：
- 返回 HTTP 200
- 响应头 `Content-Type` 为 `image/svg+xml`
- 响应头包含 `Access-Control-Allow-Origin`（CORS 支持）
- 响应体为有效的 SVG XML 内容，包含 `<svg` 标签
- 二维码编码的内容为代理地址 `127.0.0.1:8800`（基于请求的 Host 头）

---

### TC-APR-10：获取代理二维码（指定 IP）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/public/proxy/qrcode?ip=192.168.1.100" | head -5
   ```

**预期结果**：
- 返回 HTTP 200
- 响应头 `Content-Type` 为 `image/svg+xml`
- 响应体为有效的 SVG 二维码内容
- 二维码编码的内容为 `192.168.1.100:8800`（使用查询参数中指定的 IP）

---

### TC-APR-11：代理二维码 CORS 预检请求（OPTIONS）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X OPTIONS http://127.0.0.1:8800/_bifrost/public/proxy/qrcode -I
   ```

**预期结果**：
- 返回 HTTP 200
- 响应头包含 CORS 相关头部（如 `Access-Control-Allow-Origin`、`Access-Control-Allow-Methods`）

---

### TC-APR-12：对代理 API 使用不支持的 HTTP 方法

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/proxy/system -w "\n%{http_code}"
   ```
2. 执行以下命令：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/proxy/cli -w "\n%{http_code}"
   ```
3. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/address -w "\n%{http_code}"
   ```

**预期结果**：
- 三个请求均返回 HTTP 405（Method Not Allowed）

---

### TC-APR-13：访问不存在的代理 API 路径

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/proxy/nonexistent -w "\n%{http_code}"
   ```

**预期结果**：
- 返回 HTTP 404（Not Found）

---

## 清理

测试完成后清理临时数据并确保系统代理已关闭：
```bash
curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/proxy/system \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}'
rm -rf .bifrost-test
```
