# 上游连接稳定性真实场景测试

## 功能模块说明

验证 Bifrost 在短连接突发和上游失败恢复场景下保持既有 HTTP/CONNECT 请求语义，同时通过全局背压、渐进连接池淘汰和资源错误恢复门避免连接风暴扩散。

## 前置条件

- 在仓库根目录执行。
- 已安装 Rust、Python 3、curl、jq、rg。
- 正式 9900 服务保持运行且不做任何修改。
- 测试服务必须使用 `mktemp -d`、随机端口、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。

## 测试用例列表

### TC-UCS-01：连接治理实现保持功能边界

**操作步骤**：

1. 执行：
   ```bash
   rg -n "HTTPS_CLIENT_EVICTION_BATCH|BIFROST_UPSTREAM_MAX_INFLIGHT_GLOBAL|begin_network_attempt" \
     crates/bifrost-proxy/src/proxy/http/tunnel/client.rs
   rg -n "BIFROST_UPSTREAM_CONNECT_CONCURRENCY|BACKOFF_BASE|BACKOFF_MAX|connect_tcp" \
     crates/bifrost-proxy/src/utils/upstream_stability.rs
   rg -n "不修改规则匹配|不自动重放非幂等请求|不监听 macOS network path" \
     design/upstream-connection-stability.md
   ```

**预期结果**：

- client cache 使用小批量渐进淘汰，并存在跨 partition 全局在途上限。
- 直连路径存在共享 connect concurrency 和 100ms～2s 有界资源恢复退避。
- 设计明确不改变规则、TLS 范围、请求重放和 MOSS 行为。

### TC-UCS-02：缓存、并发和恢复状态单元回归

**操作步骤**：

1. 执行：
   ```bash
   cargo test -p bifrost-proxy upstream_stability --all-features
   cargo test -p bifrost-proxy client_cache --all-features
   ```

**预期结果**：

- `upstream_stability` 的并发阀门、指数退避、single probe、probe 取消恢复测试全部通过。
- `client_cache` 的 LRU 次序、活跃池保护和已有 key 复用测试全部通过。

### TC-UCS-03：真实 HTTP 与 CONNECT 突发请求保持语义

**操作步骤**：

1. 执行：
   ```bash
   SKIP_BUILD=true \
   BIFROST_BIN=./target/debug/bifrost \
   GLOBAL_INFLIGHT=8 \
   CONNECT_CONCURRENCY=4 \
   REQUEST_COUNT=80 \
   CONNECT_REQUEST_COUNT=40 \
   CONCURRENCY=64 \
   bash e2e-tests/tests/test_upstream_connection_stability.sh
   ```

**预期结果**：

- 80 个 HTTP 请求与 40 个 CONNECT 请求全部返回 fixture 的原始 JSON 语义。
- 收到响应头前的 upstream HTTP 处理峰值并发位于 2～8，不被 64 路客户端并发放大。
- HTTP 突发完成后 upstream 活跃请求数回落到 0。
- 连接到未监听端口返回原有 502，紧接着的健康请求在 3 秒内成功。
- 代理日志不出现 `EADDRNOTAVAIL`、`ENOBUFS` 或 FD 耗尽。
- 脚本退出时清理隔离代理、upstream 和临时数据目录，不影响 9900。

### TC-UCS-04：新增 E2E 已进入 CI

**操作步骤**：

1. 执行：
   ```bash
   bash scripts/ci/check-e2e-shell-ci-coverage.sh
   rg -n "test_upstream_connection_stability.sh" e2e-tests/rules/COVERAGE.md
   ```

**预期结果**：

- CI coverage guard 报告所有 `test_*.sh` 均已选入 CI 或明确跳过。
- 覆盖清单包含本次非规则代理稳定性回归。

## 清理步骤

1. 确认没有测试端口残留：
   ```bash
   pgrep -af "upstream_connection_stability_server.py|bifrost.*--no-system-proxy" || true
   ```
2. 确认正式服务仍运行：
   ```bash
   bifrost status --format json | jq -e '.running == true and .listener.port == 9900'
   ```
3. 测试脚本通过 `trap` 自动停止隔离进程并删除 `mktemp` 数据目录；若发现残留，只清理命令行明确包含本测试 fixture 或随机测试端口的进程。
