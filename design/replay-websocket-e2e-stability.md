# Replay WebSocket E2E 稳定性方案

## 背景

`e2e-tests/tests/test_replay_websocket_frames.sh` 是 Replay 页面 WebSocket 相关能力的 shell E2E 主脚本，覆盖 echo 转发、帧捕获、header 规则应用、ping/pong 抓取、permessage-deflate + 分片解压、非法控制帧拒绝、超大 payload length 拒绝、WSS upstream 转发、subprotocol 协商共 10 个子场景。

历史上该脚本在 CI 上出现过启动阶段失败，且 runner 汇总只显示 `unknown failure`，无日志可查。根因有几类：

- 端口冲突：runner 传入固定 `WS_PORT` / `WSS_PORT`，被同机其他并行 job 占用。
- 系统代理侧漏：脚本启动 Bifrost 时依赖了本机 9900 数据目录或已有系统代理，导致后续 WSS 握手失败。
- Mock server 崩溃日志被 `>/dev/null`，看不到 python traceback。
- Bifrost 二进制未 ready，wait_for_port 超时后无 tail 输出。

本方案的目标是把 shell E2E 的启动/清理/端口/日志基础设施做实，保证 10 个子场景在共享 runner 上稳定 `PASSED=10 FAILED=0`，同时保留可读诊断，避免下一次 CI 抖动被埋没。

## 用户目标验证清单

### 必须实现

- 脚本自带端口可用性检查：`WS_PORT` / `WSS_PORT` 被占用时通过 `allocate_free_port` 自动重分配，并更新 `WS_BASE_URL` / `WSS_BASE_URL`。
- 保留外部 runner 传入的 `WS_PORT` / `WSS_PORT` / `PROXY_PORT` 优先级；只有可用性检查未通过才 override。
- WS/WSS mock server 的 stdout/stderr 落盘到 `mock-logs/replay_ws_${port}.log` / `replay_wss_${port}.log`，失败时 tail 尾部。
- Bifrost 测试代理独立 `BIFROST_DATA_DIR` 子目录、`--no-system-proxy`，绝不复用 9900 端口。
- Bifrost 启动失败时 tail 其日志，暴露具体错误（cert 生成、port bind、config parse 等）。
- 主端到端结果打印 `PASSED=X FAILED=Y`，非 0 即视为脚本失败。

### 必须不破坏

- 现有 10 个子场景函数签名（`test_ws_replay_*`）、依赖的 `WS_HOST`、`ADMIN_HOST/PORT` 逻辑保持兼容。
- 已有 `ws_echo_server.py` mock 接口与命令行参数（`--port`、`--ssl`）不变。
- 用户手工执行脚本无需额外环境变量；外部 CI 传入 `WS_PORT=xxxx` 仍可覆盖默认。

### 必须真实验证

- 手工在本地跑 `bash e2e-tests/tests/test_replay_websocket_frames.sh` 得到 `PASSED=10 FAILED=0`。
- 制造端口冲突（提前 `python3 -m http.server $WS_PORT`）后脚本能自动 fallback 到新端口并通过。
- CI runner 至少一次绿灯，日志中包含 mock-log 路径。

## 产品语义

Replay WebSocket 是一个"从 Traffic 里挑一条 WS 记录 → 在 Replay 面板里重放 → 观察帧、rule 应用、ping/pong、控制帧、subprotocol"的诊断能力。E2E 脚本是这套能力的门槛回归；只要脚本挂了或输出不可读，就无法定位是 replay executor、rule pipeline、mock server 还是 CI 环境的问题。

因此本设计把"E2E 脚本本身"当成产品：

- 输入契约：`WS_PORT` / `WSS_PORT` / `PROXY_PORT` / `ADMIN_HOST` / `ADMIN_PORT` 可外部注入。
- 输出契约：`PASSED=X FAILED=Y`，失败时 stderr 有明确定位。
- 环境契约：临时数据目录、非 9900 端口、不改系统代理。

## 技术细节

### 端口分配

- `check_deps` 阶段调用 `port_is_available "$current_port"`（`e2e-tests/test_utils/process.sh` 提供）判断。
- 不可用时 `new_port="$(allocate_free_port)"` 分配 20000-60000 内的空闲端口。
- `refresh_base_urls()` 重新计算 `WS_BASE_URL = ws://${WS_HOST}:${WS_PORT}` / `WSS_BASE_URL = wss://${WS_HOST}:${WSS_PORT}`，供后续 `ws_replay_generate_*` 使用。
- 默认 fallback 起始：`WS_PORT = 20000 + ($$ % 1000)`；`WSS_PORT = WS_PORT + 1`。

### 日志与诊断

- `RUN_DATA_DIR/mock-logs/` 保存 WS/WSS mock server stdout+stderr。
- `RUN_DATA_DIR/bifrost.log` 保存测试 Bifrost 二进制日志。
- `tail_server_log <file>` 在失败路径打印最后 30-50 行。
- Bifrost 启动前打印 `Starting bifrost at ${PROXY_PORT}/${ADMIN_PORT}`；启动失败先 tail bifrost.log 再退出。

### 隔离启动

- `RUN_DATA_DIR=$(mktemp -d)`，`export BIFROST_DATA_DIR="$RUN_DATA_DIR/data"`。
- 显式 `--no-system-proxy` 传给 `bifrost` 二进制。
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 避免弹窗和后台任务。
- `cleanup` 挂 trap，强制 kill mock server 与 bifrost，删除临时数据目录（保留 log 以便 CI 拷贝）。

### 子场景清单（覆盖点）

对应函数（`e2e-tests/tests/test_replay_websocket_frames.sh`）：

1. `test_ws_replay_echo_forwarding`：基础 echo 转发。
2. `test_ws_replay_frames_capture`：帧捕获入 Traffic。
3. `test_ws_replay_rule_headers_applied`：Replay 规则 header 覆盖。
4. `test_ws_replay_ping_pong_capture`：ping/pong 帧记录。
5. `test_ws_replay_permessage_deflate_fragmentation_decompress`（543 行）：分片 + deflate 解压。
6. `test_ws_replay_reject_invalid_control_frame`（602 行）：非法控制帧（FIN=0）拒绝。
7. `test_ws_replay_reject_oversize_payload_len_header`：超大 payload length header 拒绝。
8. `test_ws_replay_wss_upstream_forwarding`：WSS upstream 转发。
9. `test_ws_replay_subprotocol_negotiation`（656 行）：subprotocol 协商。
10. 主 flow：`main()` 中长连接保持 + Replay 帧回传。

## CLI + Web + Admin API

本次改动位于 shell E2E 基础设施，不修改 CLI、Web、Admin API 公共协议。相关依赖：

- Admin API：Traffic 列表、Replay execute、Replay frames 端点。
- Web：无 UI 面向改动。
- CLI：无子命令新增。

## Sync 边界

不涉及 sync 与 group 共享；E2E 只跑本地数据目录。

## Phase 1 — 端口可用性

- 抽出 `ensure_port_available_or_reassign` 帮助函数（现已落地 147 行起）。
- `start_ws_server` / `start_wss_server` 前调用。
- `refresh_base_urls` 在 override 后更新 URL。

## Phase 2 — 日志落盘

- `prepare_run_data_dir` 创建 `mock-logs` 子目录。
- mock server 启动重定向 stdout/stderr 到具名 log。
- 失败路径统一 `tail_server_log`。

## Phase 3 — Bifrost 隔离

- `start_bifrost` 使用独立 `BIFROST_DATA_DIR`、非默认 admin/proxy 端口。
- 显式 `--no-system-proxy` 与关闭 tray/auto-login-prompt。
- 启动失败 tail `bifrost.log` 前 100 行。

## Phase 4 — 结果与文档

- `main` 最后打印 `Replay WebSocket E2E Results: PASSED=$TESTS_PASSED FAILED=$TESTS_FAILED`（现于 689 行）。
- `human_tests/proxy-websocket-sse.md` 中 `TC-PWS-07` 更新为"隔离端口 + 日志诊断 + PASSED=10 FAILED=0"。
- `human_tests/readme.md` 同步用例数量与说明。

## 测试方案

### Shell E2E（主验证）

- `bash e2e-tests/tests/test_replay_websocket_frames.sh` 默认参数：`PASSED=10 FAILED=0`。
- 覆盖场景（子场景函数已在实现位置列出）：
  - `test_ws_replay_echo_forwarding`
  - `test_ws_replay_frames_capture`
  - `test_ws_replay_rule_headers_applied`
  - `test_ws_replay_ping_pong_capture`
  - `test_ws_replay_permessage_deflate_fragmentation_decompress`
  - `test_ws_replay_reject_invalid_control_frame`
  - `test_ws_replay_reject_oversize_payload_len_header`
  - `test_ws_replay_wss_upstream_forwarding`
  - `test_ws_replay_subprotocol_negotiation`
- 端口冲突回归：
  - `python3 -m http.server $WS_PORT &` 制造冲突。
  - 期望脚本 log 中出现 `port $WS_PORT unavailable, allocating new port`，最终 PASSED=10。

### 单元测试

- 不新增 Rust 公共函数；本任务不产生单元测试。
- `port_is_available` / `allocate_free_port` 在其他 test_utils 场景已被复用。

### 真实场景 human_tests

- `human_tests/proxy-websocket-sse.md`：
  - TC-PWS-07：隔离端口 + 日志诊断 + Replay WebSocket 请求/响应头规则 + `PASSED=10 FAILED=0`。
- `human_tests/readme.md`：更新用例编号与说明。

### 环境约束

- 不使用 9900 作为 proxy 端口，不修改系统代理。
- Bifrost 二进制路径优先 `target/release/bifrost`；CI 环境可外部预构建后通过 `BIFROST_BINARY` 注入。
- `wait_for_port` 上限 20 秒，避免整体脚本无限等待。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 10 个子场景是否都跑；日志是否被 tail。
- 复核 mock server 与 bifrost 是否都用独立端口。
- 手工 `bash e2e-tests/tests/test_replay_websocket_frames.sh`：PASSED=10。
- 制造端口冲突场景，重新跑一次。

### 第 2 轮

- 复核 diff：是否有 hardcoded port、hardcoded data dir、遗漏的 `--no-system-proxy`。
- 复核 CI runner 输出：是否能在 `mock-logs` 里找到 python traceback（故障演练）。
- 复跑失败场景，收敛错误信息稳定后即可交付。

## 风险与决策

- 决策：端口默认由 `20000 + ($$ % 1000)` 起，配合 fallback，覆盖并行 job 冲突。
- 决策：mock server 使用 python + `ws_echo_server.py`，不改造成 Rust 二进制，节省实现成本。
- 决策：脚本自带 tail 日志能力，避免依赖 CI runner 额外收集。
- 风险：`wait_for_port` 20s 上限在冷启动机器上偏紧；若 CI 频繁超时，可考虑 40s 或读取 `BIFROST_STARTUP_TIMEOUT`。
- 风险：并行运行多个副本时，`mktemp -d` + PID 后缀可以避免冲突；如果 CI 强制固定路径，需要修改 CI 侧脚本。
- 风险：`permessage-deflate + fragmentation` 场景依赖 mock server 的 zlib flush 语义，任何 python 版本差异都可能导致解压不通过，因此保留 `test_ws_replay_permessage_deflate_fragmentation_decompress || true`（681 行），失败时不中断后续子场景，但会计入 FAILED。

## 实现现状（截至 2026-07-03）

- `test_replay_websocket_frames.sh` 已实现 `ensure_port_available_or_reassign`（147 行）、`refresh_base_urls`（60 行）、`tail_server_log`、mock 日志落盘、bifrost 隔离启动、失败 tail、`PASSED/FAILED` 汇总（689 行）。
- 10 个子场景函数均已定义（`test_ws_replay_echo_forwarding` … `test_ws_replay_subprotocol_negotiation`）。
- 依赖 `e2e-tests/test_utils/process.sh` 中的 `port_is_available`、`allocate_free_port`、`wait_for_port` 均可用。
- `human_tests/proxy-websocket-sse.md` 中 TC-PWS-07 已经包含隔离端口 / 日志诊断 / Replay 请求响应头 / PASSED=10 FAILED=0 断言。
- 本设计文档无待落地项。
