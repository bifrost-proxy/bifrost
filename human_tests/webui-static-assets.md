# WebUI Static Assets

## 功能模块说明

验证 Bifrost WebUI 静态资源以 gzip 压缩内容嵌入发布二进制，并在真实 HTTP 访问时直接下发 gzip 响应。WebUI 静态资源客户端必须支持 gzip；不支持 gzip 的客户端应收到明确升级提示。

## 前置条件

1. 在仓库根目录构建最新二进制：
   ```bash
   cargo build --release --bin bifrost
   ```
2. 使用临时数据目录启动服务，避免影响本机真实配置：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18891 start --skip-cert-check --unsafe-ssl --no-system-proxy
   ```
3. 管理端 API 可访问：
   ```bash
   curl -fsS http://127.0.0.1:18891/_bifrost/api/proxy/address
   ```

## 测试用例列表

### TC-WSA-01 gzip 客户端访问 WebUI 首页

操作步骤：

1. 执行：
   ```bash
   curl -sS -D /tmp/bifrost-wsa-01.headers -o /tmp/bifrost-wsa-01.body -H 'Accept-Encoding: gzip' http://127.0.0.1:18891/_bifrost/
   ```
2. 检查响应头：
   ```bash
   grep -i '^Content-Encoding: gzip' /tmp/bifrost-wsa-01.headers
   grep -i '^Vary: Accept-Encoding' /tmp/bifrost-wsa-01.headers
   ```
3. 解压响应体：
   ```bash
   gzip -dc /tmp/bifrost-wsa-01.body | grep -i '<!doctype html'
   ```

预期结果：

- HTTP 状态码为 `200`。
- 响应头包含 `Content-Encoding: gzip`。
- 响应头包含 `Vary: Accept-Encoding`。
- 解压后的响应体是 WebUI HTML。

### TC-WSA-02 非 gzip 客户端收到升级提示

操作步骤：

1. 执行：
   ```bash
   curl -sS -D /tmp/bifrost-wsa-02.headers -o /tmp/bifrost-wsa-02.body -H 'Accept-Encoding: identity' http://127.0.0.1:18891/_bifrost/
   ```
2. 检查状态码和响应体：
   ```bash
   head -n 1 /tmp/bifrost-wsa-02.headers
   grep 'gzip-capable browser' /tmp/bifrost-wsa-02.body
   ```

预期结果：

- HTTP 状态码为 `426 Upgrade Required`。
- 响应头包含 `Vary: Accept-Encoding`。
- 响应体明确提示 WebUI 需要支持 gzip 的浏览器并请用户升级浏览器。

### TC-WSA-03 gzip 客户端访问 SPA 深链

操作步骤：

1. 执行：
   ```bash
   curl -sS -D /tmp/bifrost-wsa-03.headers -o /tmp/bifrost-wsa-03.body -H 'Accept-Encoding: gzip' http://127.0.0.1:18891/_bifrost/rules/not-a-real-asset
   ```
2. 检查响应头和解压后的响应体：
   ```bash
   grep -i '^Content-Encoding: gzip' /tmp/bifrost-wsa-03.headers
   gzip -dc /tmp/bifrost-wsa-03.body | grep -i '<!doctype html'
   ```

预期结果：

- HTTP 状态码为 `200`。
- 响应头包含 `Content-Encoding: gzip`。
- 未命中的 SPA 深链回退到 gzip 压缩的 `index.html`。

## 清理步骤

1. 停止 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost stop
   ```
2. 删除临时目录：
   ```bash
   rm -rf "$TEST_DATA_DIR"
   ```
