# MCP OAuth 真实场景测试

## 功能模块说明

验证 MCP OAuth token 存储在 `Auto` 模式下能正确选择 keyring 或文件 fallback，尤其覆盖 headless CI 中 keyring entry 可创建但无法可靠读写时必须回退文件存储的场景。

## 前置条件

- 工作目录：`<REPO_ROOT>`
- 不需要启动 Bifrost 服务。
- 测试端口禁止使用 9900；本用例不占用端口。

## 测试用例列表

### TC-MCP-OAUTH-01 Auto 模式 keyring 不可用时回退文件存储

操作步骤：
1. 运行：
   `cargo test -p bifrost-agent mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file --all-features -- --nocapture`
2. 检查测试输出。

预期结果：
- 测试通过。
- `OAuthCredentialsStoreMode::Auto` 能保存 token，并通过同一模式重新加载。
- 在 keyring backend 不可可靠读写的环境中，Auto 模式不会误判 keyring 可用导致返回 `None`。

实际结果：
- 通过。2026-05-05 执行该命令 passed，`test_save_load_with_auto_mode_fallback_to_file` 1/1 通过。

## 清理步骤

无。测试使用 `tempfile::tempdir()` 管理临时文件。
