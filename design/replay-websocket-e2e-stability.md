# Replay WebSocket E2E Stability

## 功能模块说明

`e2e-tests/tests/test_replay_websocket_frames.sh` 覆盖 Replay WebSocket 的 echo、帧捕获、ping/pong、长连接、permessage-deflate 分片、非法控制帧、超大 payload length、WSS upstream 与 subprotocol 协商。CI 中该脚本曾在启动阶段短时间失败，并在汇总中只显示 `unknown failure`，缺少可定位日志。

## 实现逻辑

- 脚本保留 runner 传入的 `WS_PORT` / `WSS_PORT` / `PROXY_PORT` 优先级。
- 启动 WS/WSS mock 前先用 `port_is_available` 检查端口；如端口已被占用，使用 `allocate_free_port` 重分配，并刷新 replay 使用的 `WS_BASE_URL` / `WSS_BASE_URL`。
- WS/WSS mock 的 stdout/stderr 写入测试数据目录下的 `mock-logs/`。
- Bifrost 测试代理使用隔离的 `BIFROST_DATA_DIR` 子目录启动，并继续显式携带 `--no-system-proxy`。
- WS/WSS mock 或 Bifrost 启动失败时，脚本输出对应日志 tail，避免 CI 只看到 `unknown failure`。

## 依赖项

- 复用 `e2e-tests/test_utils/process.sh` 中的 `port_is_available` 与 `allocate_free_port`。
- 复用现有 `e2e-tests/mock_servers/ws_echo_server.py`。
- 复用现有 `target/release/bifrost` 或由外层 CI 预构建的 release 二进制。

## 测试方案

- 单元测试：无新增 Rust 公共函数；本次只调整 shell E2E 基础设施。
- E2E 测试：执行 `e2e-tests/tests/test_replay_websocket_frames.sh`，验证 10 个 Replay WebSocket 子场景全部通过。
- 真实场景测试：更新并执行 `human_tests/proxy-websocket-sse.md` 中 `TC-PWS-07`，验证隔离端口、日志诊断、WebSocket Replay 请求/响应头规则和最终 `PASSED=10 FAILED=0`。

## 校验要求

- 先执行相关 E2E，再执行格式检查。
- 任务收尾前按仓库要求执行 `cargo test --workspace --all-features`；如因耗时或环境失败阻塞，必须在结果中说明。
- 本次改动不应启动或修改系统代理，不应使用 `9900` 作为测试代理端口。

## 文档更新要求

- 更新 `human_tests/proxy-websocket-sse.md` 添加回归用例。
- 更新 `human_tests/readme.md` 同步用例数量和说明。
