# Remote Invoke Shell E2E 回归修复

## 背景

近期 `shell` 套件中的 3 个 Remote Invoke 相关用例在 CI 中失败：

1. `test_remote_connect_overload_retry_e2e.sh`
2. `test_remote_relay_url_fallback_e2e.sh`
3. `test_remote_invoke_ssh_e2e.sh`

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

### 2. 修复 SSH E2E 调用路径

更新 `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：

- 不再手工构造旧版 `calls/open` 明文 payload
- 改为复用真实 CLI：
  - `bifrost remote status`
  - `bifrost remote search`
  - `bifrost remote traffic get`

这样测试会自动走当前 caller 侧的：

- reusable grant 解析
- 本地 transport context 复用
- `command_encrypted` 加密发送
- SSE 解密与结果打印

同时继续通过 Admin API 校验 Recent Calls 中的命令记录与参数透传，保持对 SSH grant 复用能力的覆盖。

## 依赖项

- `target/release/bifrost`
- `packages/bifrost-sync-server`
- shell E2E 断言库：`e2e-tests/test_utils/assert.sh`
- Admin API 工具：`e2e-tests/test_utils/admin_client.sh`

## 测试方案

### 单元测试

- 本轮不新增 Rust 业务代码，单元测试以工作区既有测试回归为主：
  - `cargo test --workspace --all-features`

### E2E 测试

- `bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
- `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`

### 真实场景测试（human_tests）

更新 `human_tests/remote-invoke.md`，新增一条回归用例，验证：

- pair-code connect 在当前加密协议下仍能完成 overload retry
- 显式 `--relay-url` 仍优先于运行中实例 / 本地配置
- `remote connect --ssh-key` 后，`remote search` 与 `remote traffic get` 能通过真实 CLI 成功执行

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
