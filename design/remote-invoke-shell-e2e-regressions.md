# Remote Invoke Shell E2E 回归修复

## 背景

近期 `shell` 套件中的 3 个 Remote Invoke 相关用例在 CI 中失败：

1. `test_remote_connect_overload_retry_e2e.sh`
2. `test_remote_relay_url_fallback_e2e.sh`
3. `test_remote_invoke_ssh_e2e.sh`

以及 1 个 Recent Calls 参数预览回归脚本存在稳定性问题：

4. `test_remote_invoke_recent_calls_args_preview_e2e.sh`

排查后发现失败点并不一致，但根因都与 shell E2E 夹具落后于当前 relay v2 / 加密链路协议有关：

- pair-code connect 成功后，caller 现在必须从 relay 获得 `client_ephemeral_pub` 才能完成本地加密上下文落盘；旧 mock relay 仍返回旧版最小 payload，导致 connect 在“批准成功”后反而报错。
- SSH E2E 仍在用明文 `command`/`args_json` 直接调用 `/v4/remote-invoke/calls/open`，而当前 relay 已要求 `command_kind + command_encrypted`，因此命中 `HTTP 500/400` 一类协议漂移问题。

这类失败会误报为产品回归，实际是测试夹具与真实协议不一致，需要优先修正。

## 实现逻辑

### 1. 修复 pair-code connect mock relay

更新以下 shell E2E 脚本中的 mock relay：

- `e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
- `e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`

在 approved payload 中补齐稳定的 `client_ephemeral_pub`，让 CLI 能完成：

- pairing approval 解析
- 本地共享密钥派生
- `remote-connections.json` 落盘

这些脚本只验证 connect 成功/失败与 relay URL 选择，不需要完整执行远程命令，因此使用固定有效的 X25519 公钥即可。

### 1.1 CI `--skip-build` 下禁止重复 release 编译

2026-05-10 CI 复测发现 `test_remote_connect_overload_retry_e2e.sh` 在 macOS shell shard 3 中超过 900s。日志显示脚本一直停在 `Build bifrost (release)...`，而 workflow 已经从 `build-cli-macos-aarch64` 下载了 `target/release/bifrost`，并通过 `scripts/ci/run-e2e-shell.sh` 以 `--skip-build` 运行 shell 套件。

该脚本现在遵循统一 E2E 约定：

- 当 `SKIP_BUILD=true` 时，直接使用 `BIFROST_BIN` 或默认 `target/release/bifrost`
- 如果指定二进制不存在或不可执行，快速失败并输出明确路径
- 只有未设置 `SKIP_BUILD` 时才本地执行 `cargo build --release --bin bifrost`

这样 macOS shard 不再在已下载 release artifact 后重复进行昂贵 release 编译。

### 2. 修复 SSH E2E 调用路径

更新 `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：

- 不再手工构造旧版 `calls/open` 明文 payload
- 改为复用真实 CLI（实际脚本调用，验证于 2026-06-16）：
  - `bifrost remote conn status`
  - `bifrost remote exec --shell-text "..."`（成功 / 拒绝路径都覆盖）
  - `bifrost remote traffic search`
  - `bifrost remote traffic get`

这样测试会自动走当前 caller 侧的：

- reusable grant 解析
- 本地 transport context 复用
- `command_encrypted` 加密发送
- SSE 解密与结果打印

同时继续通过 Admin API 校验 Recent Calls 中的命令记录与参数透传，保持对 SSH grant 复用能力的覆盖。

### 3. 收紧 pair-code connect 后的 grant 可见性断言

`test_remote_invoke_e2e.sh` 在 caller `remote connect` 成功退出后，曾立即单次读取
`/_bifrost/api/remote-invoke/grants` 并断言 `length > 0`。本地通常足够快，但 CI 上
授权审批成功、caller 连接完成、target grants 列表可见之间可能存在短暂时序窗口，
从而把瞬时延迟误判成产品回归。

本轮将该断言改为带超时的短轮询：

- approve pairing 成功后，最多等待 20 秒（`wait_for_client_grants_at_least 1 20`）
- 每秒重新读取一次 `/_bifrost/api/remote-invoke/grants`
- 只有超时后仍为 `0` 才判定失败

这样不会放宽真正的失败条件，只是避免把“授权刚创建但列表还没稳定可见”的 CI 抖动
记成回归。

### 4. Recent Calls 参数预览 E2E 改为本地 mock 流量并补齐可诊断失败日志

`test_remote_invoke_recent_calls_args_preview_e2e.sh` 原本在 approve pairing 之后直接通过
`curl --proxy http://127.0.0.1:<admin_port> http://httpbin.org/anything/<marker>` 造流量。

这有两个 CI 脆弱点：

- 该请求依赖公网可达与 `httpbin.org` 稳定性，一旦 runner 外网抖动，脚本会在 `set -e`
  下直接退出。
- 脚本对 caller `remote connect` 使用裸 `wait "$CALLER_CONNECT_PID"`，当 connect 子进程
  非零退出时，会在自定义 `_log_fail` 之前被 `errexit` 截断，shell runner 最终只能把
  “上一条成功日志”误报成失败原因，难以定位。

本轮调整：

- 启动 `e2e-tests/mock_servers/http_echo_server.py` 本地 echo fixture，使用
  `http://127.0.0.1:<mock_port>/anything/<marker>` 代替公网 `httpbin.org`
- 造流量请求改为显式重试 + 显式失败日志，确保失败时能看到 mock server 上下文
- caller `remote connect` 改为可诊断的等待逻辑，避免 `wait` 被 `set -e` 提前中断

这样 Recent Calls 参数预览回归就只验证“远程搜索调用是否把参数摘要写入调用历史”，
不再额外耦合公网网络质量。

## 依赖项

- `target/release/bifrost`
- `packages/bifrost-sync-server`（gitignored 产物，由 `e2e-tests/test_utils/sync_server.sh` 在测试启动时按需构建并通过 `sync_server_exec` 暴露二进制）
- shell E2E 断言库：`e2e-tests/test_utils/assert.sh`
- Admin API 工具：`e2e-tests/test_utils/admin_client.sh`
- 本地 mock：`e2e-tests/mock_servers/http_echo_server.py`（Recent Calls 预览脚本使用 `--port` / `--retries` 启动）

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

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`，新增一条回归用例，验证：

- pair-code connect 在当前加密协议下仍能完成 overload retry
- 显式 `--relay-url` 仍优先于运行中实例 / 本地配置
- `remote conn up --ssh-key` 后，`remote traffic search`、`remote traffic get` 与 `remote exec --shell-text` 能通过真实 CLI 成功执行
- Recent Calls 参数预览回归脚本在离线/受限 CI 网络下仍能通过本地 mock 流量稳定生成调用记录

同时同步更新 `human_tests/readme.md` 索引。

## 校验要求

按顺序执行：

1. 先跑本次修复涉及的 3 个 shell E2E
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `bash scripts/ci/local-ci.sh --e2e-only shell`
5. `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md`
