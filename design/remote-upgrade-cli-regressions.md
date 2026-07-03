# Remote CLI 回归修复（remote invoke args_json / force update notice / shell E2E relay 回退）

> 状态：已实现 | 更新时间：2026-07-03

## 背景

近一轮 shell E2E 回归里暴露出三类兼容问题，需要在本方案中一次性收敛：

1. **Remote query `args_json` 丢失**：`remote status/search/traffic` 已切到 canonical query 主链路，但 `remote-invoke/calls` 里不再稳定保留旧有 `args_json` 字段，导致以下依赖字段的回归测试失效，Recent Calls 参数预览也变脆弱：
   - `e2e-tests/tests/test_remote_invoke_e2e.sh`
   - `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
   - `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
2. **强制更新提示不能绕过 TTY**：`BIFROST_FORCE_UPDATE_CHECK=1 bifrost status | cat` 这类非 TTY 验证场景收不到更新提示，`e2e-tests/tests/test_upgrade_cli.sh` 失败。
3. **shell CI relay 启动入口不一致**：`bash scripts/run_all_e2e.sh --skip-build` 复用预编译 Rust 二进制，但本地 relay 夹具对 `packages/bifrost-sync-server` 的启动入口不一致：
   - `test_remote_invoke_e2e.sh` 硬编码 `node dist/cli.js`。
   - 其他脚本走 `npx/pnpm exec tsx src/cli.ts`。
   当 CI 镜像没有事先构建 `packages/bifrost-sync-server/dist/cli.js` 时，remote invoke shell 套件因 `MODULE_NOT_FOUND` 直接失败。

## 用户目标验证清单

### 必须实现

- `remote search` / `remote traffic list` / `remote traffic get` 在使用 canonical query 主链路的同时，同步生成稳定 `args_json`，供 Recent Calls、call 审计与旧 E2E 消费。
- `open_call` 请求显式携带 `command_summary.command_preview + masked_args_json`，caller / target 两侧 Recent Calls 都能直接读取参数摘要，不再依赖本地解密回退。
- `BIFROST_FORCE_UPDATE_CHECK=1` 存在时，`status` 与普通命令即使 stdout 非 TTY 也执行更新提示；`version-check` / `upgrade` 不重复插入 notice。
- shell E2E 优先使用 `packages/bifrost-sync-server/dist/cli.js`，缺失时自动回退 `pnpm exec tsx src/cli.ts` → `npx tsx src/cli.ts`。
- shell E2E 使用默认 `target/release/bifrost` 且未显式 `SKIP_BUILD=true` 时，只在 release binary 缺失、`Cargo.toml` / `Cargo.lock` / `crates/` 相关源码比二进制新时才重建，避免“源码修复但脚本继续用旧 release binary”的假回归。

### 必须不破坏

- canonical query 主链路的能力与协议版本，不因为回填 `args_json` 而下探到旧协议。
- 日常交互 shell 的 update notice 默认体验（非 TTY 不刷屏）保持不变，只在显式开关下才绕过 TTY 限制。
- sync-server 运行时协议与产物格式不变；本次仅修正 shell E2E 夹具启动入口。
- `--skip-build` 语义收敛为“不重新构建 Rust release binary”，不再隐式要求 Node relay 也有预编译 dist。

### 必须真实验证

- 真实链路 `bifrost remote search / remote traffic list / remote traffic get` 后 Recent Calls 与 relay call 记录都能读到 `args_json`。
- 真实场景 `BIFROST_FORCE_UPDATE_CHECK=1 bifrost status | cat` 显示新版本提示。
- 干净 CI 镜像（没有 `dist/cli.js`）执行 shell E2E 全套通过。

## 产品语义

### 1. canonical query + `args_json` 双写

`remote search` / `remote traffic list` / `remote traffic get` 命令保留 `query: CanonicalQueryCommand` 主传输链路，同时生成一份稳定的 `args_json`：

- 执行端仍优先走 canonical query，避免协议能力回退。
- 调用记录、Recent Calls、旧 E2E 与兼容观察工具继续从 `args_json` 读取参数。
- `args_json` 内容与 query 语义一致，避免 caller / worker 看到两套不同参数。
- `open_call` 请求同时显式携带 `command_summary { command_preview, masked_args_json }`，供 relay / client 展示简明摘要。

### 2. 强制更新检查绕过 TTY 限制

`crates/bifrost-cli/src/main.rs::should_run_update_notice(stdout_is_terminal, command)`（line 92）：

- 当 `stdout_is_terminal == false` 且 `BIFROST_FORCE_UPDATE_CHECK` 未设置 → 不显示 notice（保持日常体验）。
- 当 `BIFROST_FORCE_UPDATE_CHECK` 存在 → 允许显示 notice，`status` / 普通命令即使非 TTY 也刷新版本提示。
- `version-check` / `upgrade` 命令走单独路径，不重复插入。

### 3. shell E2E relay 启动入口统一回退

`e2e-tests/test_utils/sync_server.sh` 提供 `remote_sync_server_entry()` helper：

```bash
if [[ -f "$dir/dist/cli.js" ]]; then
    printf '%q %q' "node" "$dir/dist/cli.js"      # 复用预构建 dist
elif command -v pnpm >/dev/null 2>&1; then
    printf '%q %q %q %q' "pnpm" "exec" "tsx" "src/cli.ts"
else
    printf '%q %q %q' "npx" "tsx" "src/cli.ts"
fi
```

以下脚本改为调用此 helper 而非硬编码：

- `e2e-tests/tests/test_remote_invoke_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `e2e-tests/tests/test_group_sync_e2e.sh`

### 4. shell E2E release binary 自动重建

`e2e-tests/tests/test_remote_invoke_e2e.sh` / `test_remote_invoke_recent_calls_args_preview_e2e.sh` / `test_remote_invoke_ssh_e2e.sh` 中 release binary 准备逻辑改为：

- 使用默认 `target/release/bifrost` 且未显式 `SKIP_BUILD=true` 时，仅在以下情况重建：
  - release binary 不存在；
  - `Cargo.toml` / `Cargo.lock` 比 binary 更新；
  - `crates/` 下相关源码比 binary 更新。
- 避免每次 shell E2E 都跑一次完整 release 编译，同时消除源码新但脚本复用旧 binary 的假回归。

## 技术细节

### 后端 / CLI 改动

- `crates/bifrost-cli/src/commands/remote.rs`：
  - `remote search` / `remote traffic list` / `remote traffic get` 分支同时生成 `query` 与 `args_json`。
  - `build_open_call_command_summary()` 生成 `command_summary { command_preview, masked_args_json }`。
- `crates/bifrost-cli/src/main.rs`：
  - `should_run_update_notice(stdout_is_terminal: bool, command: Option<&Commands>)`（line 92-93）实现三态判定。
  - `main()` line 214：`should_run_update_notice(std::io::stdout().is_terminal(), cli.command.as_ref())`。

### E2E 夹具

- `e2e-tests/test_utils/sync_server.sh`：dist / pnpm / npx 三级回退（line 11 / 39 / 48 / 52）。
- `e2e-tests/tests/test_remote_invoke_e2e.sh` 等：release binary 准备逻辑按 mtime 判定。

### 依赖项

- `crates/bifrost-cli/src/commands/remote.rs`
- `crates/bifrost-cli/src/main.rs`
- `e2e-tests/test_utils/sync_server.sh`
- `e2e-tests/tests/test_remote_invoke_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `e2e-tests/tests/test_group_sync_e2e.sh`
- `e2e-tests/tests/test_upgrade_cli.sh`

### CLI + Web + Admin API

- CLI 表面：命令名与参数保持不变；新增行为通过 `BIFROST_FORCE_UPDATE_CHECK` 环境变量与 Recent Calls `command_summary` 字段暴露。
- Web：Remote Invoke Recent Calls 直接消费 `command_summary`，不再依赖客户端本地解密回退。
- Admin API：无新增路径；`open_call` 请求体新增 `command_summary` 可选字段。

### Sync 边界

- 本方案只影响 caller 侧 CLI 与 shell E2E 夹具，不修改 sync-server 运行时协议、call meta 持久化字段与 grant 授权语义。
- `args_json` 生成属于纯 caller 语义，relay 只透传。

## Phase 1-4 拆分

### Phase 1：`args_json` 回填与 `command_summary`

- `remote search / traffic list / traffic get` 双写 `query + args_json`。
- 新增 `build_open_call_command_summary()`。
- 覆盖单元测试：`test_build_remote_command_for_search_uses_streaming_command`、`test_build_remote_command_for_traffic_search_uses_streaming_command`、`test_build_remote_command_for_traffic_list_includes_all_filters`、`test_build_remote_command_for_traffic_get_includes_body_flags`、`test_build_open_call_command_summary_uses_label_and_args_json`。

### Phase 2：强制更新提示绕过 TTY

- `should_run_update_notice` 三态判定实现。
- 单元测试：`should_run_update_notice_when_forced_even_without_tty`（line 783）、`should_run_update_notice_skips_non_tty_output`（line 901）、`should_run_update_notice_skips_explicit_version_related_commands`（line 912）、`should_run_update_notice_respects_command_filters`（line 1036）。

### Phase 3：shell E2E relay 入口统一

- `sync_server.sh` 三级回退 helper 实现。
- 各 shell E2E 改为调用 helper。
- 干净 CI 镜像验证：`bash scripts/ci/run-e2e-shell.sh` 全套通过。

### Phase 4：release binary 增量重建

- `test_remote_invoke_*.sh` 按 mtime 判定是否重建 release binary。
- 保留 `SKIP_BUILD=true` 显式跳过语义。

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_search_uses_streaming_command`
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_search_uses_streaming_command`
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_list_includes_all_filters`
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_for_traffic_get_includes_body_flags`
- `cargo test -p bifrost-cli remote::tests::test_build_open_call_command_summary_uses_label_and_args_json`
- `cargo test -p bifrost-cli should_run_update_notice_when_forced_even_without_tty`
- `cargo test -p bifrost-cli should_run_update_notice_skips_non_tty_output`
- `cargo test -p bifrost-cli should_run_update_notice_skips_explicit_version_related_commands`
- `cargo test -p bifrost-cli should_run_update_notice_respects_command_filters`

### E2E 测试

- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- `bash e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh`
- `bash e2e-tests/tests/test_group_sync_e2e.sh`
- `bash e2e-tests/tests/test_upgrade_cli.sh`
- `bash scripts/ci/run-e2e-shell.sh`

### 真实场景测试（human_tests）

`human_tests/remote-invoke.md` 新增回归条目：

- `TC-RI-回归-141`：`remote traffic list/search/get` 调用记录的 `args_json` 兼容回归。
- `TC-RI-回归-142`：`remote connect --ssh-key` 保存连接后执行 `remote status/search/traffic get`。
- `TC-RI-回归-143`：`--skip-build` 且缺失 `packages/bifrost-sync-server/dist/cli.js` 时 relay 自动回退。

`human_tests/cli-import-export.md` 新增 `TC-CIE-回归-018`：`BIFROST_FORCE_UPDATE_CHECK=1` 下非 TTY `status` 仍显示新版本提示。

同步更新 `human_tests/readme.md` 索引与用例数量。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：三类回归全部消除；canonical query 主链路能力未回退。
- 代码 review：`args_json` 序列化字段顺序是否稳定；`should_run_update_notice` 三态是否覆盖 `status` / `run` / `remote *` / `version-check` / `upgrade`。
- 复测：定向 shell E2E、cli 单元测试、`test_upgrade_cli.sh`、`test_remote_invoke_e2e.sh`。

### 第 2 轮

- 复核 shell E2E 夹具在 CI 镜像与本地 macOS 双环境下的执行差异。
- 检查 `git diff` 是否遗漏某个 shell E2E 未切换到 relay entry helper。
- 复测：`cargo test --workspace --all-features`、`bash scripts/ci/local-ci.sh --e2e-only shell`。

## 校验要求

1. 先跑定向单元测试 / 定向 shell E2E。
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test --workspace --all-features`
5. `bash scripts/ci/local-ci.sh --e2e-only shell`
6. 执行 `rust-project-validate`

## 文档更新要求

- `human_tests/remote-invoke.md`
- `human_tests/cli-import-export.md`
- `human_tests/readme.md`

## 风险与决策

- **`args_json` 双写字段稳定性**：字段顺序需要与旧版本严格一致，避免 relay 侧摘要 hash 抖动。
- **`BIFROST_FORCE_UPDATE_CHECK` 语义**：只作为“测试与显式调试开关”，不作为最终用户功能推广，避免默认体验回退。
- **shell E2E entry fallback**：`pnpm exec tsx` 与 `node dist/cli.js` 在 signal / exit code 传递上略有差异；`sync_server.sh` 需要保证进程组终止一致，以免 relay 泄漏。
- **release binary 增量重建**：以 mtime 判定；如果 CI 复用 cache 且 clock skew 超过分钟级，需要在脚本里加 `touch` 保底。
