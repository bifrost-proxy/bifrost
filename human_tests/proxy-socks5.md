# SOCKS5 代理功能测试用例

## 功能模块说明

验证 Bifrost 作为 SOCKS5 代理服务器的核心功能，包括基本 SOCKS5 代理转发、DNS 解析、HTTPS 流量透传、UDP ASSOCIATE 启动就绪等。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录，启用 SOCKS5 端口）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy --socks5-port 1180
   ```
2. 确保端口 8800 和 1180 均未被占用
3. SOCKS5 代理监听在 `127.0.0.1:1180`

---

## 测试用例

### TC-PSK-01：SOCKS5 基本 HTTP 代理（curl --socks5）

**操作步骤**：
1. 执行命令：
   ```bash
   curl --socks5 127.0.0.1:1180 http://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "http://httpbin.org/get"`
- 请求通过 SOCKS5 代理成功转发

---

### TC-PSK-02：SOCKS5 代理 DNS 解析（--socks5-hostname）

**操作步骤**：
1. 使用 `--socks5-hostname` 让代理服务器负责 DNS 解析：
   ```bash
   curl --socks5-hostname 127.0.0.1:1180 http://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "http://httpbin.org/get"`
- DNS 解析由 Bifrost 代理服务器完成（而非客户端本地解析）
- 与 TC-PSK-01 的区别在于域名解析发生在代理端

---

### TC-PSK-03：SOCKS5 代理 HTTPS 流量

**操作步骤**：
1. 执行命令：
   ```bash
   curl --socks5-hostname 127.0.0.1:1180 https://httpbin.org/get
   ```

**预期结果**：
- 返回 HTTP 200 状态码
- 响应体为 JSON 格式，包含 `"url": "https://httpbin.org/get"`
- HTTPS 流量通过 SOCKS5 隧道正确传输
- TLS 握手在客户端与目标服务器之间完成，代理仅做透传

---

### TC-PSK-04：SOCKS5 UDP ASSOCIATE 启动就绪回归

**操作步骤**：
1. 使用临时数据目录和非固定端口执行 UDP ASSOCIATE E2E：
   ```bash
   TMP_DIR="$(mktemp -d /tmp/bifrost-socks5-human.XXXXXX)"
   PROXY_PORT=18880 SOCKS5_PORT=18881 BIFROST_DATA_DIR="$TMP_DIR" bash e2e-tests/tests/test_socks5_udp.sh
   ```
2. 观察脚本启动阶段输出，确认脚本在发起 UDP ASSOCIATE 前等待 admin API 与 SOCKS5 端口就绪。
3. 测试结束后清理临时目录：
   ```bash
   rm -rf "$TMP_DIR"
   ```

**预期结果**：
- 脚本不出现 `Connection refused`
- `Test 1: UDP ASSOCIATE Handshake` 输出连接 SOCKS5 server 成功
- UDP relay 地址和端口解析成功
- 脚本输出 `SOCKS5 UDP Tests Completed`

**本轮执行记录（2026-05-01）**：
- 使用临时数据目录 `/tmp/bifrost-socks5-human.scZy33`、测试代理端口 `53877`、SOCKS5 端口 `53878` 执行本用例；未使用正式端口 `9900`。
- 启动阶段等待 admin API 与 SOCKS5 listener 就绪后才进入 UDP ASSOCIATE。
- `Test 1: UDP ASSOCIATE Handshake` 成功连接 `127.0.0.1:53878`，返回 `Reply: 0`，relay 地址为 `127.0.0.1:53042`。
- 脚本最终输出 `SOCKS5 UDP Tests Completed`，退出码为 0。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
