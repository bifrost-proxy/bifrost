# Remote CLI 回归修复（remote invoke args_json / force update notice）

## 背景

近期 shell 回归里暴露出三类兼容问题：

1. `remote status/search/traffic` 已切到 canonical query 主链路，但 `remote-invoke/calls` 中不再稳定保留旧有 `args_json` 字段，导致：
   - `test_remote_invoke_e2e.sh`
   - `test_remote_invoke_ssh_e2e.sh`
   - `test_remote_search_traffic_cli_isomorphic_e2e.sh`
   这类依赖调用记录字段的回归测试失效，也让 Recent Calls 参数预览的兼容回退变脆弱。
2. 更新提示只在 stdout 是 TTY 时显示，导致 `BIFROST_FORCE_UPDATE_CHECK=1 bifrost status | cat` 这类非 TTY 验证场景拿不到提示，`test_upgrade_cli.sh` 失败。
3. shell CI 固定使用 `bash scripts/run_all_e2e.sh --skip-build` 复用预编译的 Rust 二进制，但本地 relay 夹具对 `packages/bifrost-sync-server` 的启动入口并不一致：
   - `test_remote_invoke_e2e.sh` 仍硬编码 `node dist/cli.js`
   - 其他脚本则直接使用 `npx/pnpm exec tsx src/cli.ts`
   当 CI 镜像没有事先构建 `packages/bifrost-sync-server/dist/cli.js` 时，remote invoke shell 套件会在运行时因 `MODULE_NOT_FOUND` 直接失败。

## 实现逻辑

### 1. Remote query 命令补回 `args_json`

更新 `crates/bifrost-cli/src/commands/remote.rs`：

- `remote search`
- `remote traffic list`
- `remote traffic get`

在保留 `query: CanonicalQueryCommand` 的同时，同步生成一份稳定的 `args_json`：

- 执行端仍优先走 canonical query，避免协议能力回退
- 调用记录、Recent Calls、旧 E2E 与兼容观察工具继续可从 `args_json` 读取参数
- `args_json` 内容与 query 语义保持一致，避免 caller / worker 看到两套不同参数
- `open_call` 请求同时显式携带 `command_summary.command_preview + masked_args_json`，让 relay / client 两侧 Recent Calls 都能直接拿到参数摘要，而不是只依赖本地解密回退

### 2. 强制更新检查绕过 TTY 限制

更新 `crates/bifrost-cli/src/main.rs`：

- 当 `BIFROST_FORCE_UPDATE_CHECK=1` 存在时，`status` / 普通命令即使 stdout 不是 TTY，也允许执行更新提示逻辑
- `version-check` / `upgrade` 仍保持不重复插入额外 notice

目标是保留日常交互场景下“非 TTY 不刷屏”的默认体验，同时给测试与显式调试开一个稳定开关。

### 3. shell E2E relay 启动入口统一回退

更新 `e2e-tests/test_utils/sync_server.sh` 与以下 shell E2E：

- `e2e-tests/tests/test_remote_invoke_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `e2e-tests/tests/test_group_sync_e2e.sh`

统一规则：

- 若 `packages/bifrost-sync-server/dist/cli.js` 已存在，优先复用它，保持与预构建产物兼容
- 若 dist 不存在，则自动回退到源码入口：
  - 优先 `pnpm exec tsx src/cli.ts`
  - 无 `pnpm` 时回退到 `npx tsx src/cli.ts`
- 这样 `--skip-build` 仅表示“不重新构建 Rust release binary”，不再隐式要求 Node relay 也必须有预编译 dist

### 4. remote invoke shell E2E 在 release 过期时自动重建 CLI

更新以下脚本的 release binary 准备逻辑：

- `e2e-tests/tests/test_remote_invoke_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`

规则调整为：

- 当脚本使用默认 `target/release/bifrost` 且未显式 `SKIP_BUILD=true` 时，只在以下情况重建：
  - release 二进制不存在
  - `Cargo.toml` / `Cargo.lock` 比二进制更新
  - `crates/` 下相关 Rust 源码比二进制更新
- 避免“源码已修复，但脚本继续复用旧 release 二进制”导致的假回归，同时不把每次 shell E2E 都变成一次完整 release 重编

这次改动只修正测试夹具，不改变 sync-server 运行时协议和产物格式。

## 依赖项

- `crates/bifrost-cli/src/commands/remote.rs`
- `crates/bifrost-cli/src/main.rs`
- `e2e-tests/test_utils/sync_server.sh`
- `e2e-tests/tests/test_remote_invoke_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- shell E2E 脚本：
  - `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
  - `e2e-tests/tests/test_group_sync_e2e.sh`
  - `e2e-tests/tests/test_upgrade_cli.sh`

## 测试方案

### 单元测试

- `test_build_remote_command_for_search_uses_streaming_command`：验证 query search 同时携带 `args_json`
- `test_build_remote_command_for_traffic_search_uses_streaming_command`：验证 remote traffic search 同时携带 `args_json`
- `test_build_remote_command_for_traffic_list_includes_all_filters`：验证 `traffic.list` 的 `args_json` 含完整过滤参数
- `test_build_remote_command_for_traffic_get_includes_body_flags`：验证 `traffic.get` 的 `args_json` 含 `id/request_body/response_body`
- `test_build_open_call_command_summary_uses_label_and_args_json`：验证 caller 发起 `open_call` 时会把参数摘要写入 `command_summary`
- `test_should_run_update_notice_when_forced_even_without_tty`：验证 `BIFROST_FORCE_UPDATE_CHECK` 可绕过 TTY 限制

### E2E 测试

- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `bash e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `bash e2e-tests/tests/test_group_sync_e2e.sh`
- `bash e2e-tests/tests/test_upgrade_cli.sh`
- `bash scripts/ci/run-e2e-shell.sh`

### 真实场景测试（human_tests）

更新以下文档并真实执行：

- `human_tests/remote-invoke.md`
  - 回归 `remote traffic list/search/get` 的调用记录参数兼容
  - 回归 `remote connect --ssh-key` 后保存连接执行 `remote status/search/traffic get`
  - 回归 shell E2E 在 `--skip-build` 且缺失 `packages/bifrost-sync-server/dist/cli.js` 时，仍能自动回退到源码入口完成本地 relay 启动
- `human_tests/cli-import-export.md`
  - 回归 `BIFROST_FORCE_UPDATE_CHECK=1` 下非 TTY `status` 仍显示新版本提示

同步更新 `human_tests/readme.md`。

## 校验要求

1. 先跑定向单元测试 / 定向 shell E2E
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test --workspace --all-features`
5. `bash scripts/ci/local-ci.sh --e2e-only shell`
6. 执行 `rust-project-validate`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/cli-import-export.md`
- 更新 `human_tests/readme.md`
