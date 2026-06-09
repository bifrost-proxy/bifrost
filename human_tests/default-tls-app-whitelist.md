# 默认 TLS 应用白名单真实场景测试

## 功能模块说明

验证系统默认 TLS 应用白名单只包含常见浏览器模式，并且默认配置不包含 Codex。该用例覆盖持久化配置默认值、代理运行时默认值和真实临时服务 Admin API 返回值三个来源。

## 前置条件

- 在仓库根目录执行。
- 每条命令前执行 `source ~/.zshrc`。
- TC-DTA-01 和 TC-DTA-02 不需要启动 Bifrost 服务，不会修改系统代理。
- TC-DTA-03 会启动临时 Bifrost 服务，必须使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `--no-system-proxy`。

## 测试用例列表

### TC-DTA-01：持久化默认 TLS 应用白名单不包含 Codex

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n 'DEFAULT_APP_INTERCEPT_INCLUDE|Codex|app_intercept_include' crates/bifrost-storage/src/unified_config.rs
   ```
2. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-storage test_tls_config_default
   ```

预期结果：

- `DEFAULT_APP_INTERCEPT_INCLUDE` 包含 `Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`。
- 默认列表不包含 `Codex`、`Codex*` 或 `*Codex*`。
- 单元测试通过。

本次执行结果：

- 通过。2026-06-09 执行 `rg -n 'DEFAULT_APP_INTERCEPT_INCLUDE|Codex|app_intercept_include' crates/bifrost-storage/src/unified_config.rs`，命中默认常量和测试断言位置；默认常量只包含 `Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`，没有 Codex 项。执行 `cargo test -p bifrost-storage test_tls_config_default` 通过：`1 passed; 0 failed`。

### TC-DTA-02：代理运行时默认 TLS 应用白名单不包含 Codex

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   rg -n 'DEFAULT_APP_INTERCEPT_INCLUDE|Codex|app_intercept_include' crates/bifrost-proxy/src/server.rs
   ```
2. 执行：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-proxy test_proxy_config_default_app_intercept_include_excludes_codex
   ```

预期结果：

- `DEFAULT_APP_INTERCEPT_INCLUDE` 包含 `Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`。
- 默认列表不包含 `Codex`、`Codex*` 或 `*Codex*`。
- 单元测试通过。

本次执行结果：

- 通过。2026-06-09 执行 `rg -n 'DEFAULT_APP_INTERCEPT_INCLUDE|Codex|app_intercept_include' crates/bifrost-proxy/src/server.rs`，命中默认常量和测试断言位置；默认常量只包含 `Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`，没有 Codex 项。执行 `cargo test -p bifrost-proxy test_proxy_config_default_app_intercept_include_excludes_codex` 通过：`1 passed; 0 failed`，其余测试按 filter 跳过。

### TC-DTA-03：真实临时服务返回的默认 TLS 应用白名单不包含 Codex

操作步骤：

1. 构建当前源码二进制：
   ```bash
   source ~/.zshrc
   cargo build --bin bifrost
   ```
2. 使用临时数据目录启动服务：
   ```bash
   source ~/.zshrc
   TEST_DIR="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost start -p 18893 --host 127.0.0.1 --unsafe-ssl --no-system-proxy --access-mode allow_all
   ```
3. 查询配置：
   ```bash
   source ~/.zshrc
   curl -sS http://127.0.0.1:18893/_bifrost/api/config
   ```
4. 停止服务并清理临时目录。

预期结果：

- `tls.app_intercept_include` 包含 `Google Chrome*`、`Microsoft Edge*`、`*Safari*`、`*Firefox*`、`*Opera*`、`*Brave*`、`*Arc*`、`*Vivaldi*`。
- `tls.app_intercept_include` 不包含 `Codex`、`Codex*` 或 `*Codex*`。
- 服务启动命令包含 `--no-system-proxy`，不会修改系统代理。

本次执行结果：

- 通过。2026-06-09 执行 `cargo build --bin bifrost` 通过。使用临时数据目录 `/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/tmp.DdMc7CRxqj` 启动 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost start -p 18893 --host 127.0.0.1 --unsafe-ssl --no-system-proxy --access-mode allow_all`；首次临时目录提示安装 CA 时选择 `No, skip`，未修改系统信任。启动日志显示 `Force intercept apps: ["Google Chrome*", "Microsoft Edge*", "*Safari*", "*Firefox*", "*Opera*", "*Brave*", "*Arc*", "*Vivaldi*"]`。执行 `curl -sS http://127.0.0.1:18893/_bifrost/api/config | python3 -c ...` 断言通过，API 返回 `["Google Chrome*", "Microsoft Edge*", "*Safari*", "*Firefox*", "*Opera*", "*Brave*", "*Arc*", "*Vivaldi*"]`，且没有任何包含 `codex` 的默认项。测试后端口 `18893` 已释放，临时目录已删除；前台 PTY 进程在释放端口后未自行退出，已对本次测试 PID `3604` 强制清理并确认无监听残留。

## 清理步骤

- TC-DTA-03 执行后停止临时 Bifrost 服务，确认端口释放，并删除临时 `BIFROST_DATA_DIR`。
