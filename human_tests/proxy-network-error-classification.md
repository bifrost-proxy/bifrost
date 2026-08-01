# 代理网络错误分类与日志限流真实场景测试

## 功能模块说明

验证 WebSocket 上游拒绝与 DNS/TCP/TLS 传输错误分离，重复拒绝日志有界，同时客户端 502 行为和正常 WebSocket 转发不变；panic stderr Broken pipe 不产生递归 panic。

## 前置条件

- 使用本地 Python mock、动态端口、`--no-system-proxy` 和隔离数据目录。
- 使用当前 `target/debug/bifrost`，不连接外部服务。

## 测试用例列表

### TC-PNEC-01：panic Broken pipe 防护

**操作步骤**：

```bash
cargo test -p bifrost-core panic_handler -- --nocapture
```

**预期结果**：

- 注入 Broken pipe writer 时返回 I/O 错误但测试进程不发生二次 panic。
- str/String payload 与异步 panic guard 行为保持通过。

### TC-PNEC-02：上游拒绝真实代理语义和日志限流

**操作步骤**：

```bash
BIFROST_BIN=target/debug/bifrost \
e2e-tests/tests/test_websocket_rejection_logging.sh
```

**预期结果**：

- 本地上游连续 6 次返回 401，代理每次都保持原有 `502 Bad Gateway`。
- 同一 host/status 的 30 秒窗口只写一条 warning。
- warning 包含 `error_category=upstream_handshake_rejected`，不再由顶层记录成通用 network error。
- 临时代理、mock server 和测试目录在退出时清理。

### TC-PNEC-03：正常 WebSocket 与 CI 收集回归

**操作步骤**：

```bash
cargo test -p bifrost-proxy websocket_handshake_ -- --nocapture
bash scripts/ci/check-e2e-shell-ci-coverage.sh
python3 scripts/ci/check-e2e-capabilities.py
```

**预期结果**：

- 正常 WebSocket handshake/echo 与拒绝分类单元测试通过。
- 新增 shell E2E 被 CI 自动收集，能力矩阵校验通过。

## 清理步骤

- 确认 `.bifrost-e2e-runs/` 下没有 `ws-rejection-*` 测试目录。
- 确认共享 9900 PID 未变化。

## 2026-07-31 执行记录

- `TC-PNEC-01`：通过，panic handler 5 个用例成功，Broken pipe writer 不触发二次 panic。
- `TC-PNEC-02`：通过，6 次真实上游 401 均返回 502，只记录 1 条 `upstream_handshake_rejected` warning。
- `TC-PNEC-03`：通过，WebSocket 分类单测 2 个、正常 handshake/echo 1 个成功；shell CI 收集 199=172+27，能力矩阵 10 项通过。
- 清理确认：无 `ws-rejection-*` 目录残留，共享 9900 PID 未变化。
