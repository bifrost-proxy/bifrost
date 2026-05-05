# MCP OAuth

## 功能模块说明

MCP Streamable HTTP transport 支持 OAuth 2.0 认证，token 可按 `OAuthCredentialsStoreMode` 存储到文件或系统 keyring。`Auto` 模式应在可用时使用 keyring，在不可用或不可可靠读写时回退到文件存储。

## 实现逻辑

- `KeyringTokenStore::is_available()` 不只检查 keyring entry 是否可创建，还执行一次临时 `set/get/delete` roundtrip。
- 只有 roundtrip 能成功读回同一 probe 值时，`Auto` 模式才选择 keyring。
- 在 headless Linux CI、DBus Secret Service 不完整、credential backend 无法持久读写等环境中，`Auto` 模式回退到 `OAuthTokenStore` 文件路径。

## 依赖项

- Rust `keyring` crate，仅在 `keyring-store` feature 启用时参与。
- 文件 fallback 使用当前 `data_dir/oauth/<server>.json`。

## 测试方案

- 单元测试：`mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file` 验证 `Auto` 模式可保存并重新加载 token。
- E2E 测试：不需要新增独立 E2E，OAuth token store 是本地持久化选择逻辑，单元测试可覆盖 CI 失败路径。
- 真实场景测试：`human_tests/mcp-oauth.md` 中 `TC-MCP-OAUTH-01` 执行同一 Auto fallback 回归命令。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file --all-features -- --nocapture`
- 提交前按仓库规则执行 `cargo test --workspace --all-features`。

## 文档更新要求

- 更新 `human_tests/mcp-oauth.md`。
- 更新 `human_tests/readme.md` 索引。
