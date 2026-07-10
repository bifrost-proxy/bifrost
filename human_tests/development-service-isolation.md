# 开发与测试服务隔离真实场景测试

## 功能模块说明

验证本地开发、E2E 清理和 IM provider 持久化不会停止、重启、覆盖正式 `9900` Bifrost 服务，也不会删除或重置 `~/.bifrost/admin/im_gateway_providers.json`。

## 前置条件

- 正式 Bifrost 服务已运行，`bifrost status --format json` 返回 `running=true`、`listener.port=9900`。
- 正式数据目录为 `~/.bifrost`，已存在 `admin/im_gateway_providers.json`。
- 当前 worktree 已构建 `target/release/bifrost`。
- 测试只读取正式服务状态；所有新实例使用随机端口和隔离数据目录，不修改系统代理。

## 测试用例

### TC-DTSI-01：E2E 清理不停止正式服务或修改 IM 配置

操作步骤：

1. 读取 `bifrost status --format json`，记录正式服务 PID、版本、端口和数据目录。
2. 记录 `~/.bifrost/admin/im_gateway_providers.json` SHA-256，并执行 `bifrost im provider list` 记录 provider ID。
3. 模拟父进程继承正式数据目录后执行：
   `BIFROST_DATA_DIR="$HOME/.bifrost/should-never-be-used" BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_e2e_process_cleanup_isolation.sh`。
4. 再次读取正式服务状态、provider 列表和文件 SHA-256。
5. 检查 `~/.bifrost/should-never-be-used` 和 `~/.bifrost/process-cleanup-isolation` 均不存在。

预期结果：

- E2E 的 10 条断言全部通过：继承到的正式目录被改道、正式根目录拒绝清理、只停止沙箱实例、沙箱外实例保持健康、受保护端口拒绝清理、模拟 IM 文件哈希不变。
- 正式服务 PID、版本、端口、数据目录前后完全一致，证明没有 restart/stop/覆盖。
- 正式 provider ID 和配置文件哈希前后完全一致。
- 正式数据目录下不残留测试子目录。

### TC-DTSI-02：开发入口默认使用隔离目录和非正式端口

操作步骤：

1. 执行 `make -n run run-release dev`，检查展开后的后端命令。
2. 执行 `bash -n scripts/dev-run.sh`。
3. 检查 `Makefile` 和 `scripts/dev-run.sh` 中的数据目录、端口、系统代理、Tray、Sync 登录护栏。

预期结果：

- Make 入口默认包含 `.bifrost-dev`、`-p 8800`、`--no-system-proxy`、`--skip-cert-check`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- `scripts/dev-run.sh` 语法通过并包含同等护栏。
- 不存在默认 `cargo run ... start` 直接接管 `~/.bifrost:9900` 的开发入口。

### TC-DTSI-03：IM provider 主文件损坏时恢复且不删除证据

操作步骤：

1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::provider_store::tests --lib -- --nocapture`。
2. 核对五个用例分别覆盖首次备份、截断恢复、无备份损坏保留、主备双损坏保留、不兼容版本保留。

预期结果：

- 5 个测试全部通过。
- 截断主文件从 `.bak` 自动恢复 provider。
- 无有效备份时不删除主文件或备份，原始坏字节保持可恢复。
- 主文件和备份都不可恢复时，`add/update/delete` 进入只读保护，不会用空内存状态覆盖坏文件。

## 清理步骤

1. E2E 脚本通过 trap 停止两个随机端口实例并删除临时数据。
2. 确认 `lsof -nP -iTCP:9900 -sTCP:LISTEN` 仍指向步骤 1 记录的正式 PID。
3. 确认 `~/.bifrost/should-never-be-used` 和 `~/.bifrost/process-cleanup-isolation` 均不存在。
4. 不停止、不重启正式服务，不修改正式 IM provider。

## 执行记录

2026-07-10 在 `/Users/eden_studio/work/github/bifrost-test-service-isolation` 执行：

| 用例 | 结果 | 实际结果 |
| --- | --- | --- |
| TC-DTSI-01 | PASS | 隔离 E2E 10/10 通过，包含正式目录子路径自动改道和正式根目录清理拒绝。正式服务前后 PID 均为 `23609`，版本均为 `0.0.148`，端口均为 `9900`，数据目录均为 `/Users/eden_studio/.bifrost`；`lsof` 仍显示 PID `23609` 监听 `*:9900`。provider ID 前后均为 `feishu-main`，配置 SHA-256 前后均为 `d70cc3bea8186981759f94be48c6d8764d9b94426d17a372b4068b55b2e82fea`；正式目录无 `should-never-be-used` 或 `process-cleanup-isolation` 残留。 |
| TC-DTSI-02 | PASS | `make -n run run-release dev` 均展开为 `.bifrost-dev:8800`，包含 `--no-system-proxy --skip-cert-check`、Tray 和 Sync 登录护栏；`bash -n scripts/dev-run.sh` 通过且同样包含隔离变量。 |
| TC-DTSI-03 | PASS | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::provider_store::tests --lib` 5/5 通过；截断恢复成功，无备份、双损坏和不兼容版本场景均保留原文件。 |
