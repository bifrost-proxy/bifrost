# 默认 TLS 应用白名单

## 功能模块说明

Bifrost 的 TLS 应用白名单默认配置用于在未显式传入 `--app-intercept-include` 或未持久化自定义配置时，限制哪些客户端应用会触发按应用维度的 TLS 解包。默认列表只覆盖常见浏览器进程模式，不包含 Codex CLI 或其他 Agent/终端类工具。

## 实现逻辑

- `crates/bifrost-storage/src/unified_config.rs` 通过 `TlsConfig::default()` 提供持久化配置默认值。
- `crates/bifrost-proxy/src/server.rs` 通过 `ProxyConfig::default()` 提供代理运行时默认值。
- 两处默认 `app_intercept_include` 均为：
  - `Google Chrome*`
  - `Microsoft Edge*`
  - `*Safari*`
  - `*Firefox*`
  - `*Opera*`
  - `*Brave*`
  - `*Arc*`
  - `*Vivaldi*`
- `Codex`、`Codex*`、`*Codex*` 不属于默认应用白名单；如果用户确实需要解包 Codex 发起的 HTTPS 流量，必须通过 CLI、Admin API 或 WebUI 显式添加。

## 依赖项

- `bifrost-storage` 的统一配置默认值。
- `bifrost-proxy` 的运行时代理配置默认值。

## 测试方案

- 单元测试：
  - `unified_config::tests::test_tls_config_default` 断言持久化默认白名单等于浏览器列表，且不包含 `codex`。
  - `server::tests::test_proxy_config_default_app_intercept_include_excludes_codex` 断言运行时默认白名单等于浏览器列表，且不包含 `codex`。
- E2E 测试：
  - 启动临时 Bifrost 服务并查询 `/_bifrost/api/config`，确认真实 Admin API 返回的默认 `tls.app_intercept_include` 等于浏览器列表且不包含 Codex。
  - 本次不新增代理流量 E2E。变更只涉及默认静态配置列表，不改变请求处理、隧道建立、WebUI 操作或用户手动配置行为。
- 真实场景测试：
  - `human_tests/default-tls-app-whitelist.md` 静态验收默认源码路径、默认列表、单元测试命令和真实临时服务 Admin API 返回值。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核默认列表路径、检查 `git diff`、运行两个 focused cargo test。
- 第 2 轮：复查最新 diff、确认 human_tests 索引和用例执行记录，再复跑 focused cargo test。

## 校验要求

- `cargo test -p bifrost-storage test_tls_config_default`
- `cargo test -p bifrost-proxy test_proxy_config_default_app_intercept_include_excludes_codex`
- `BIFROST_DATA_DIR=<temp> BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/debug/bifrost start -p <port> --host 127.0.0.1 --unsafe-ssl --no-system-proxy --access-mode allow_all`
- `curl http://127.0.0.1:<port>/_bifrost/api/config`
- 按 `rust-project-validate` 在收尾阶段执行格式、clippy、受影响测试和工作区兜底测试；如全量命令受环境资源限制阻塞，必须记录风险。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- 无 CLI 参数、Admin API 协议或 WebUI 文案变更，因此不需要更新 README 或 API 文档。
