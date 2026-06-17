# CLI version-check 输出修复

## 功能模块详细描述

修复 `bifrost version-check` 在管理端成功返回版本检查结果时没有任何输出的问题。

当前 CLI 子命令已经能正确请求 `/_bifrost/api/system/version-check`，但打印逻辑读取了错误的 JSON 字段名，导致命令成功退出却没有可见输出。这会让用户误以为命令未执行、网络异常或版本检查被静默跳过。

本次改动聚焦于 CLI 输出回归修复，不调整版本缓存、GitHub 请求或升级流程。

## 实现逻辑

### 1. 对齐 CLI 与服务端的字段协议

- 服务端返回字段为：
  - `current_version`
  - `latest_version`
  - `has_update`
  - `release_highlights`（可选，字符串数组；用于展示版本亮点）
  - `release_url`（可选，发布页 URL）
- CLI `handle_version_check` 改为读取上述字段，而不是旧的：
  - `current`
  - `latest`
  - `update_available`
- 当 `handle_version_check` 无法访问运行中的代理（如未启动）时，回落到 `handle_version_check_standalone`，直接通过 `update_check::get_latest_version_fresh_with_diagnostics` 向 GitHub 查询，并组装出同样字段名的 JSON 再交给统一的输出函数渲染。

### 2. 为缺失最新版本信息提供明确提示

- 当服务端返回 `latest_version` 时：
  - 输出当前版本
  - 输出最新版本
  - 根据 `has_update` 输出可升级提示（`🚀 Update available!`，附带 `release_highlights` 列表与 `release_url`，并提示 `To upgrade, run: bifrost upgrade`）或“已是最新版本”（`✅ You are running the latest version.`）
- 当服务端无法提供 `latest_version` 时：
  - 仍输出当前版本
  - 输出明确提示（`⚠ Could not determine the latest version. Check your network connection.`），说明暂时无法确定最新版本，建议检查网络后重试
- 整段输出由上下两条 `─` 黄色分隔线包裹，便于在终端中识别 version-check 区块

### 3. 抽离可测试的输出格式化逻辑

- 将 `version-check` 的输出组装提取为独立辅助函数
- 单元测试直接验证不同 JSON 返回下的行文本，避免依赖 stdout 捕获

### 4. 收紧 CLI 回归测试

- 更新 `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- 不再将“空输出”视为成功
- 允许的成功输出应为：
  - 版本信息
  - 已是最新版本提示
  - 明确的网络/无法获取最新版本提示

## 依赖项

- `crates/bifrost-cli/src/commands/mod.rs`
- `crates/bifrost-cli/src/commands/update_check.rs`（standalone fallback 直查 GitHub 时使用）
- `crates/bifrost-core/src/version_check.rs`（`is_newer_version` / `release_page_url` 等辅助函数）
- `crates/bifrost-cli/src/commands/config/client.rs`（`ConfigApiClient::version_check` 调用 `/system/version-check?refresh=true`）
- `e2e-tests/tests/test_cli_online_commands_e2e.sh`
- `human_tests/cli-import-export.md`
- `human_tests/readme.md`

## 测试方案

### 单元测试

- 验证返回 `current_version/latest_version/has_update=true` 且带 `release_highlights` 与 `release_url` 时输出当前版本、最新版本、升级提示、亮点列表与发布页链接（`version_check_prints_update_available`）
- 验证返回 `current_version/latest_version/has_update=false` 时输出当前版本、最新版本和“已是最新版本”（`version_check_prints_latest`）
- 验证返回 `current_version` 且 `latest_version=null` 时输出当前版本和“暂时无法确定最新版本”的提示（`version_check_prints_missing_latest`）
- 验证 `has_update=true` 但 `release_highlights` 为空时仍能输出升级提示且不渲染空的亮点段落（`version_check_prints_update_no_highlights`）

### E2E 测试

- 更新 `test_cli_online_commands_e2e.sh` 中的 `version-check` 断言
- 执行该脚本，确认不再放过空输出

### 真实场景测试

- 更新 `human_tests/cli-import-export.md` 中 `TC-CIE-14`
- 明确回归点：命令执行成功时不得出现空输出
- 使用临时数据目录执行 `cargo run --bin bifrost -- version-check`，逐条比对实际输出

## 校验要求（含 rust-project-validate）

- 先执行本次改动涉及的单元测试、E2E 和 human_tests
- 再执行 `cargo test --workspace --all-features`
- 执行 `bash scripts/ci/local-ci.sh`
- 最后执行 `rust-project-validate`

## 文档更新要求

- 更新 `human_tests/cli-import-export.md` 的回归用例说明
- 同步更新 `human_tests/readme.md` 中对应条目的测试用例数与说明
