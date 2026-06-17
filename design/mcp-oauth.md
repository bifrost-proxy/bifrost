# MCP OAuth

## 功能模块说明

MCP Streamable HTTP transport 支持 OAuth 2.0 认证，token 可按 `OAuthCredentialsStoreMode` 存储到文件或系统 keyring。`Auto` 模式应在可用时使用 keyring，在不可用或不可可靠读写时回退到文件存储。

## 实现逻辑

- `KeyringTokenStore::is_available()` 不只检查 keyring entry 是否可创建，还执行一次临时 `set/get/delete` roundtrip，只有能读回同一 probe 值才视为可用。
- `Auto` 模式 save 时再做一次保存校验：写入 keyring 后立即 `load` 比对 `access_token`，不一致或失败时降级到 `OAuthTokenStore` 文件路径。
- `Auto` 模式 load 时先尝试 keyring，未命中或失败再退回文件路径；delete 同样先 keyring 后文件，任一成功即视为已删除。
- 在 headless Linux CI、DBus Secret Service 不完整、credential backend 无法持久读写等环境中，`Auto` 模式回退到 `OAuthTokenStore` 文件路径。
- 未启用 `keyring-store` feature 时，`Keyring` 模式发出 warn 并降级到文件存储，`Auto` 模式直接走文件路径。

## 依赖项

- Rust `keyring` crate，仅在 `keyring-store` feature 启用时参与；服务名常量 `KEYRING_SERVICE`，entry username 通过 `compute_key(server_name, url)` 生成（与 Codex 的 `compute_store_key` 对齐：`{server_name}|{sha256_prefix_16}`）。
- 文件 fallback 使用 `OAuthTokenStore`，token 落盘到 `{data_dir}/oauth/{server_name}.json`。

## 测试方案

- 单元测试：`bifrost-agent` crate 中 `mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file` 验证 `Auto` 模式可保存并重新加载 token（实测文件路径：`crates/agent/src/mcp/oauth.rs`）。
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
