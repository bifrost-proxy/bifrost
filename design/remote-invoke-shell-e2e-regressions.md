# Remote Invoke Shell E2E 回归修复

## 背景

Remote Invoke 已进入 v2 加密链路（`command_encrypted` + X25519 ephemeral key + SSH key 复用配对）。生产链路已稳定，但历史 shell E2E 脚本仍在用旧协议 payload 或裸 mock relay，出现三类误报回归：

1. `test_remote_connect_overload_retry_e2e.sh`、`test_remote_relay_url_fallback_e2e.sh`：pair-code approve payload 缺 `client_ephemeral_pub`，CLI 拿不到派生共享密钥所需的公钥，`connect` 批准成功后反而报错。
2. `test_remote_invoke_ssh_e2e.sh`：仍手工构造旧版明文 `command / args_json` 直接 POST `/v4/remote-invoke/calls/open`，被服务端 `command_kind + command_encrypted` 校验拒绝，返回 HTTP 500/400。
3. `test_remote_invoke_recent_calls_args_preview_e2e.sh`：依赖公网 `httpbin.org`，runner 出网抖动直接 `set -e` 退出；对 caller `remote connect` 使用裸 `wait` 也会被 `errexit` 截断，日志把「上一条成功日志」误报成失败原因。

同时 macOS shell shard 上还出现 `test_remote_connect_overload_retry_e2e.sh` 一直卡在 `Build bifrost (release)...` 的超时问题（>900s）——workflow 已经从 `build-cli-macos-aarch64` 下载了 release artifact，但脚本没走 `SKIP_BUILD` 分支，重复编译。

本方案的目标是把 shell E2E 夹具对齐当前生产协议、消除公网依赖、加固可诊断性，同时收紧 pair-code connect 后的 grant 可见性断言，防止 CI 时序抖动被误判为回归。

## 用户目标验证清单

### 必须实现

- pair-code mock relay 在 approved payload 中包含稳定 `client_ephemeral_pub`（固定有效 X25519 公钥）。
- SSH E2E 全部通过真实 CLI（`bifrost remote conn status` / `bifrost remote exec --shell-text` / `bifrost remote traffic search|get`）执行，不再手拼 `/calls/open`。
- Recent Calls 参数预览用本地 `http_echo_server.py` 提供 fixture，脚本能解析实际绑定端口并使用它注入流量。
- 未指定 `SKIP_BUILD` 时按 `BIFROST_BIN` 优先，二进制存在则跳过 `cargo build --release`。
- pair-code approve 后 grants 断言改为 `wait_for_client_grants_at_least 1 20` 短轮询，避免时序抖动误报。
- Target admin 重启后 Recent Calls / Grants API 从 `503` 恢复 `200` 再断言。

### 必须不破坏

- `command_encrypted` 端到端加密协议的正确性保持。
- Recent Calls 参数预览的核心断言（参数摘要、长参数截断、落盘恢复、清理）不放宽。
- 已有工作区其他脚本继续使用 `http_echo_server.py --retries 5` 的行为不变。
- `packages/bifrost-sync-server` 构建流程与 `sync_server_exec` 约定不动。

### 必须真实验证

- macOS shell shard 3 在 `SKIP_BUILD=true` 下 `test_remote_connect_overload_retry_e2e.sh` 不再重复 release 编译，运行 <300s。
- 预占 `MOCK_HTTP_PORT` 场景下脚本自动 fallback 并成功造流量。
- 断网环境跑 Recent Calls 预览 E2E 全绿。
- SSH E2E 覆盖 SSH key 授权 + 命令 + traffic 检索全链路。

## 产品语义

### E2E 夹具必须跟上生产协议

Relay v2 的加密链路要求：

- `POST /pairings/start` 要求 `caller_ephemeral_pub`；服务端在 `submitGrantDecision` 返回 `client_ephemeral_pub`。
- `POST /calls/open` 要求 `command_kind + command_encrypted`，明文 `command` 字段已被移除。

任何简化过的 mock relay 或手工命令 payload 都必须显式携带这两组字段；否则 caller 端会在共享密钥派生 / relay 请求阶段被拒绝。

### 可诊断优先

E2E 脚本失败时必须能定位「哪一层出问题」：

- fixture 日志 `Starting HTTP Echo Server on 127.0.0.1:<port>...` 用于识别实际端口。
- `_log_fail` 在退出前必须完成执行，不能被 `set -e` 提前截断。
- 断言超时时必须 dump mock server 日志、caller connect 日志、target admin 日志。

## 技术细节

### 0. Streaming stdin 首帧竞态

`remote exec --interactive` 的 caller-to-client stdin 通过 encrypted `call_frame`
到达 target。CI 曾暴露一个竞态：relay 先把 stdin 首帧推给 target，而
target 的 `call_open` 还没完成 active call 登记时，worker 会把 frame 当作
inactive call 丢弃，导致远端子进程已经输出 `READY` 但读不到
`EARLY_STDIN_OK`。

当前修复采用两层约束：

- target worker 仅对已解析且方向为 `CallerToClient` 的早到 encrypted
  `call_frame` 建立短期缓冲，TTL 为 10 秒；每 call 最多 64 帧 / 256 KiB、
  全局最多 8 MiB、同时最多 128 个 call，防止 relay 抖动变成无界内存增长，
  同时保证 flush 不会在 executor 启动前填满 64-slot stdin channel。
- 超过每 call/global 字节预算、超过 call 数量或等待超过 TTL 时，worker
  必须拒绝整个 call 并返回 `remote.stdin_early_buffer_rejected`，禁止丢掉部分
  stdin 后继续执行命令。全局饱和会短暂打开 circuit breaker，使未建档的
  stdin call 显式失败而不是无提示截断；不接受 stdin 且没有 pending frame
  的 query/file/shell call 不受该 circuit breaker 影响。
- `call_open` 创建 active call 并准备 stdin channel 后立即 flush 早到帧；
  回放仍走原有 grant crypto、counter nonce、解密、replay window 与
  stdin sender 路径，不绕过安全校验。
- 生产执行路径 `execute_with_stdout_sink -> execute_shell_exec` 在
  `stdin_mode=stream` 时打开 child stdin pipe 并消费 mpsc stdin；测试不得以
  当前未接 worker 的 `execute_shell_exec_streaming` 代替生产路径验证。

回归验证：

```bash
cargo test -p bifrost-admin stdin -- --nocapture
cargo test -p bifrost-admin handle_call_frame -- --nocapture
bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
```

### 1. pair-code mock relay 补 `client_ephemeral_pub`

- 位置：`e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`、`e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh` 中的 approve mock 响应体。
- 使用固定有效 X25519 公钥（base64），配合已有的固定私钥；脚本不真正加密命令，只验证 connect / relay URL 选择路径。
- CLI 侧 `LocalConnection.client_ephemeral_pub` 字段会被填，`transport_context_version` 递增。

### 2. `SKIP_BUILD=true` 直用现成二进制

`test_remote_connect_overload_retry_e2e.sh` 现在遵循统一 E2E 约定：

- 若 `SKIP_BUILD=true`，直接使用 `BIFROST_BIN` 或默认 `target/release/bifrost`。
- 二进制不存在或不可执行 → 快速失败并输出明确路径。
- 未设置 `SKIP_BUILD` 时才本地 `cargo build --release --bin bifrost`。

Workflow 侧通过 `scripts/ci/run-e2e-shell.sh --skip-build` 传入 `SKIP_BUILD=true`。

### 3. SSH E2E 改走真实 CLI（`e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`）

不再手工构造 `/calls/open` payload。脚本按顺序执行：

```bash
"$BIFROST_BIN" remote conn status --relay-url "$RELAY_URL"
"$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CID:0:12}" -- status
"$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CID:0:12}" -- search --include ...
"$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CID:0:12}" -- traffic get --ids ...
"$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CID:0:12}" -- traffic auth-status
"$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CID:0:12}" -- traffic export --as curl
```

真实 CLI 会自动走 reusable grant 解析、本地 transport context 复用、`command_encrypted` 加密发送、SSE 解密与结果打印。Recent Calls / Grants 校验继续通过 Admin API 完成。

### 4. Recent Calls fixture 端口 fallback（`test_remote_invoke_recent_calls_args_preview_e2e.sh`）

- L119 `MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-$(pick_free_port)}"` 预选端口。
- L175 `python3 -u http_echo_server.py --port "$MOCK_HTTP_PORT" --retries 5`（`-u` 关闭 stdout buffer）。
- L185 从日志中解析实际端口：
  ```
  actual_port="$(sed -nE 's/^Starting HTTP Echo Server on [^:]+:([0-9]+)\.\.\.$/\1/p' "$MOCK_SERVER_LOG" | tail -n 1)"
  ```
- L187-190 若与请求端口不同，覆盖 `MOCK_HTTP_PORT` 并打 `Local HTTP echo fixture fell back from port X to Y`。
- ready 预算提升到 60s（`wait_for_recent_calls_api`），避免 macOS 高并发下 Python 启动延迟被误判。
- caller `remote connect` 使用可诊断等待，替换裸 `wait $CALLER_CONNECT_PID`；失败时 dump caller / mock / target 日志。

### 5. 重启后 API 就绪等待

- Target admin 重启后 `wait_for_recent_calls_api "重启后读取 Recent Calls API 应返回 200"`（L511）。
- 主 E2E 类似地等待 `/api/remote-invoke/grants` 恢复 200 再断言缺失 crypto 的 grants 被清理。

### 6. Grant 可见性短轮询

`test_remote_invoke_e2e.sh` 中 approve pairing 成功后：

```bash
wait_for_client_grants_at_least 1 20
```

- 最长 20s，每秒轮询 `/api/remote-invoke/grants`。
- 只有超时后仍为 0 才判失败；产品回归被瞬时时序抖动误报的问题消失。

### 7. Stale grant 归类（呼应 `remote-invoke-resilience.md`）

Caller 在 `open_call` 收到 `grant_session_token_invalid` 且 refresh 无可复用 grant 时，把该条目从 `remote-connections.json` 移除并提示 `please run bifrost remote connect ... again`。E2E 侧新增断言覆盖该路径。

## CLI + Web + Admin API

### CLI（无新增，验证既有能力）

```
bifrost remote conn up --ssh-key ~/bifrost-dev.key --relay-url ...
bifrost remote conn status
bifrost remote exec --shell-text "..."
bifrost remote traffic search --include ...
bifrost remote traffic get --ids ...
```

### Admin API（验证接口）

- `GET /_bifrost/api/remote-invoke/grants` — 断言 `first_connected_at` 存在。
- `GET /_bifrost/api/remote-invoke/calls?limit=...` — 断言参数预览摘要正确。

### Web UI

- 本轮不涉及 UI 改动；仅通过后端 API 验证。

## Sync 边界

- E2E 全程使用临时 `BIFROST_DATA_DIR`，不影响用户本机数据。
- Mock relay 与本地 `http_echo_server.py` 仅在测试进程存活；`packages/bifrost-sync-server` 由 `e2e-tests/test_utils/sync_server.sh` 按需构建并通过 `sync_server_exec` 暴露二进制。

## Phase 1 – 协议对齐

- 补齐 pair-code mock relay `client_ephemeral_pub`。
- SSH E2E 改走真实 CLI。
- Recent Calls fixture 切换到本地 `http_echo_server.py`。

## Phase 2 – 构建约束

- `SKIP_BUILD=true` 分支避免重复 release 编译。
- macOS shard 3 超时问题消除。

## Phase 3 – 可诊断性加固

- 从 fixture 日志解析实际端口。
- caller `remote connect` 可诊断等待。
- Grant 可见性短轮询 20s。
- API 就绪等待 60s。

## Phase 4 – Stale grant 归类

- Caller 在收到 `grant_session_token_invalid` 时清理本地条目。
- 新增 E2E 断言覆盖。

## 测试方案

### 单元测试

- 本轮不新增 Rust 业务代码，单元测试以工作区既有测试回归为主：
  - `cargo test --workspace --all-features`

### E2E 测试

- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
- `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
- `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`

### 真实场景测试 `human_tests/remote-invoke.md`

新增回归用例，验证：

- pair-code connect 在当前加密协议下仍能完成 overload retry。
- 显式 `--relay-url` 仍优先于运行中实例 / 本地配置。
- `remote conn up --ssh-key` 后 `remote traffic search|get`、`remote exec --shell-text` 通过真实 CLI 执行成功。
- Recent Calls 参数预览在离线 / 受限 CI 网络下通过本地 mock 稳定生成调用记录。
- Recent Calls 参数预览 / 持久化在重启 target admin 后等待 `/api/remote-invoke/calls` 恢复 `200`，并继续断言 JSONL 历史可恢复。
- 主 Remote Invoke E2E 在 target client 重启后等待 `/api/remote-invoke/grants` 恢复 `200`，断言丢失 grant crypto 后本地 grants 被清理。
- Caller 在 `open_call` 收到 `grant_session_token_invalid` 且 refresh 无可复用 grant 时归类为 stale grant，清理 `remote-connections.json`，提示重新 connect。

`human_tests/readme.md` 索引更新。

## Review/Fix/Test 闭环

### 第 1 轮

- Review：mock relay 的 `client_ephemeral_pub` 是固定有效 X25519 base64；密钥格式错误会被 CLI 直接拒绝。
- Review：SSH E2E 路径无残留 `curl /calls/open` 手拼调用。
- Test：`bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` + `test_remote_invoke_recent_calls_args_preview_e2e.sh`。

### 第 2 轮

- Review：`SKIP_BUILD=true` 分支覆盖 `BIFROST_BIN` 缺失场景。
- Review：`wait_for_client_grants_at_least` 超时后打印 API 响应体。
- Test：`bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh` + `test_remote_relay_url_fallback_e2e.sh`。

## 校验要求

按顺序执行：

1. 先跑本次修复涉及的 4 个 shell E2E（overload retry / relay url fallback / SSH / recent calls）。
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `bash scripts/ci/local-ci.sh --e2e-only shell`
5. `cargo test --workspace --all-features`

## 依赖项

- `target/release/bifrost`
- `packages/bifrost-sync-server`（由 `e2e-tests/test_utils/sync_server.sh` 按需构建）
- Shell E2E 断言库：`e2e-tests/test_utils/assert.sh`
- Admin API 工具：`e2e-tests/test_utils/admin_client.sh`
- 本地 mock：`e2e-tests/mock_servers/http_echo_server.py`（`--port` / `--retries` 启动）

## 风险与决策

- **固定 X25519 公钥的安全性**：mock relay 中的 `client_ephemeral_pub` 是测试用固定值，仅在 E2E 隔离数据目录内使用，绝不参与生产链路。
- **60s ready 预算**：macOS shard 上 Python 启动可能 >10s，60s 显著降低误判成本；失败发现慢 50s 可接受。
- **`SKIP_BUILD` 强依赖 workflow 传参**：如果开发者本地不设置该变量，脚本仍会 `cargo build`，行为向后兼容。
- **短轮询 vs 严格断言**：`wait_for_client_grants_at_least 1 20` 不放宽真正的失败条件（超时 20s 仍为 0 则失败），只吸收瞬时时序抖动。
- **真实 CLI E2E 依赖 grant / SSH key 已配置**：SSH E2E 脚本必须先跑 `remote conn up --ssh-key`，任何环境预置失败会直接体现在 `remote conn status` 输出中，便于定位。

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`（新增本轮回归用例）
- 更新 `human_tests/readme.md`（索引与用例数）
