# status TUI 远程调用状态面板

## 功能模块说明

`bifrost status -t` 新增 Remote Invoke 状态面板，用于在终端中查看本机 Remote Invoke 目标端的远程调用情况。面板需要和 WebUI Settings -> Remote Invoke 的事实源保持一致，展示已连接或授权过来的客户端，以及最新远程命令的执行状态和结果。

## 实现逻辑

- 在 `crates/bifrost-cli/src/commands/status_tui.rs` 中将 TUI tab 从 3 个扩展为 4 个，新增 `Remote Invoke` tab。
- 仅当用户切到 Remote Invoke tab 或强制刷新时拉取远程调用数据，避免默认 Overview 增加后台开销。
- 复用现有 Admin API：
  - `GET /_bifrost/api/remote-invoke/status`：读取 worker state、pending pairing 数和 active call 数。
  - `GET /_bifrost/api/remote-invoke/grants`：读取连接/授权过来的客户端。
  - `GET /_bifrost/api/remote-invoke/calls?limit=20`：读取最近调用历史。
- CLI 内部按 `grant_id` 或 `caller_fingerprint` 将最近调用聚合到客户端行，展示最新命令预览、调用状态、退出码、耗时和输出字节数。
- 面板下半区保留最近命令明细列表，便于直接排查最新调用。

## 依赖项

- 现有 Remote Invoke worker、grant 和 call history API。
- 现有 `ratatui` TUI 生命周期、tab 切换、低频刷新和 `ureq` 直连 Admin API 模式。

## 测试方案

### 单元测试

- `remote_invoke_latest_call_matches_grant_and_prefers_newest`：验证同一客户端的最新调用按时间聚合，且 grant 轮转后可按 caller fingerprint 兜底匹配。
- `remote_invoke_result_formats_terminal_and_running_calls`：验证成功终态和运行中调用的结果展示。
- `remote_invoke_labels_prefer_human_names_and_normalize_auth`：验证客户端展示名优先级和 SSH key / Pair code 连接方式展示。

### E2E 测试

- 扩展或新增 status TUI 相关 E2E：启动临时 Bifrost 服务，打开 `status --tui`，切到 Remote Invoke tab，断言界面包含 `Remote Invoke`、`Connected Clients`、`Recent Commands`、`State` 等关键字段。

### 真实场景测试

- 更新 `human_tests/cli-start-stop-status.md`，新增 `TC-CSS-35`：
  - 使用临时 `BIFROST_DATA_DIR` 和动态端口启动 Bifrost。
  - 调用 Remote Invoke status/grants/calls API 确认事实源可用。
  - 使用 PTY 运行 `bifrost status --tui`，发送右方向键切到 Remote Invoke tab，确认面板标题和关键区块可见。
  - 启动命令必须带 `--no-system-proxy` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 tab 切换、API 拉取时机、空数据降级、现有 Overview/Rules/Traffic tab 是否保持原行为；运行 focused 单测和 human_tests 对应用例。
- 第 2 轮：复核格式化、文档、human_tests 索引和新增聚合边界；复跑 focused 单测和 TUI 真实验证。

## 校验要求

- `cargo fmt --all -- --check`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli status_tui -- --nocapture`
- status TUI Remote Invoke human_tests 真实执行
- 收尾阶段按项目规则执行 E2E、coverage、rust-project-validate、workspace all-features 和远端 CI 看护。

## 文档更新要求

- 更新 `human_tests/cli-start-stop-status.md`
- 更新 `human_tests/readme.md`
- 不涉及 README 外部命令参数变更；`status -t` 原有入口不变。
