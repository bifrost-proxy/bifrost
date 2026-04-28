# Sync API

## 模块说明

Sync API 负责把本机 Bifrost 的规则与远端同步服务绑定。原有 `bifrost sync login` 只支持打开浏览器并等待远端 SSO 回调把 token 写回本地服务；在 CI、沙箱或无浏览器环境中无法完成登录。

## Token + URL 直登方案

- CLI 新增 `bifrost sync login --token <token> --url <remote-url>`。
- 参数必须成对提供；无参数时保持原有打开浏览器登录行为。
- CLI 调用 `POST /_bifrost/api/sync/login`，请求体为：

```json
{
  "token": "<sync-session-token>",
  "remote_base_url": "https://bifrost.bytedance.net"
}
```

- Admin API 收到 `token + remote_base_url` 时不打开浏览器，直接更新 sync 配置中的 `remote_base_url`，保存 session token，并唤醒后台同步任务。
- Admin API 收到空 body 或 `{}` 时保持原浏览器登录流程。
- 只提供 token 或只提供 remote url 返回 HTTP 400，避免 CI 中误以为已经完成登录。
- `remote_base_url` 会裁剪尾部 `/`，并要求以 `http://` 或 `https://` 开头。

## 依赖项

- `crates/bifrost-cli`：CLI 参数和 Config API client。
- `crates/bifrost-admin`：`/api/sync/login` 请求体解析与直接保存入口。
- `crates/bifrost-sync`：session token 与 remote url 的持久化。

## 测试方案

- 单元测试：
  - `save_login_session_updates_remote_url_and_token` 验证 token 与 URL 会落入状态和配置，且启用 auto sync。
  - `save_login_session_rejects_empty_or_invalid_input` 验证空 token 和非法 URL 被拒绝。
  - `sync_login_direct_options_parse` 验证 CLI `--token/--url` 参数解析与 help 输出。
- E2E 测试：
  - `e2e-tests/tests/test_sync_login_direct_e2e.sh` 启动隔离数据目录、动态端口 Bifrost 与本地 mock sync server，执行 `bifrost sync login --token ci-token --url <mock-url>`，断言不会依赖浏览器且最终授权成功。
  - 同一脚本验证缺少 `remote_base_url` 的 API 请求返回 400。
- 真实场景测试：
  - 更新 `human_tests/api-sync.md`，新增 CI/沙箱 token+url 直登用例和缺参错误用例。
  - 更新 `human_tests/readme.md` 索引后，按新增用例真实执行。

## 校验要求

- 先执行新增 E2E 脚本和 human_tests 用例。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 最后执行 `cargo test --workspace --all-features`。

## 文档更新要求

- 本设计文档记录 API/CLI 合约。
- `human_tests/api-sync.md` 和 `human_tests/readme.md` 必须同步更新。
