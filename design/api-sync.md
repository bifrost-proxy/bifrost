# Sync API

## 模块说明

Sync API 负责把本机 Bifrost 的规则与远端同步服务绑定。原有 `bifrost sync login` 只支持打开浏览器并等待远端 SSO 回调把 token 写回本地服务；在 CI、沙箱或无浏览器环境中无法完成登录。

## Token 直登方案

- CLI 支持 `bifrost sync login --token <token>`，用于 CI、沙箱或无浏览器环境。
- CLI 继续支持 `bifrost sync login --token <token> --url <remote-url>`，显式覆盖远端同步服务地址。
- 只提供 `--token` 时，Admin API 使用当前同步配置中的 `remote_base_url`；新安装默认值为内置 Bifrost Provider `https://bifrost.bytedance.net`。
- 无参数时保持原有打开浏览器登录行为。
- 只提供 `--url` 仍返回错误，避免误以为完成登录。
- CLI help 必须展示 token 获取地址：`https://bifrost.bytedance.net/v4/sso/token-login`。
- CLI 调用 `POST /_bifrost/api/sync/login`，请求体为：

```json
{
  "token": "<sync-session-token>",
  "remote_base_url": null
}
```

- Admin API 收到 `token + remote_base_url` 时不打开浏览器，直接更新 sync 配置中的 `remote_base_url`，保存 session token，并唤醒后台同步任务。
- Admin API 收到 `token` 且未收到 `remote_base_url` 时，复用当前配置的 `remote_base_url`，配置为空时回退内置 Bifrost Provider。
- Admin API 收到空 body 或 `{}` 时保持原浏览器登录流程。
- `remote_base_url` 会裁剪尾部 `/`，并要求以 `http://` 或 `https://` 开头。

## 依赖项

- `crates/bifrost-cli`：CLI 参数和 Config API client。
- `crates/bifrost-admin`：`/api/sync/login` 请求体解析与直接保存入口。
- `crates/bifrost-sync`：session token 与 remote url 的持久化。

## 测试方案

- 单元测试：
  - `save_login_session_updates_remote_url_and_token` 验证 token 与 URL 会落入状态和配置，且启用 auto sync。
  - `save_login_session_rejects_empty_or_invalid_input` 验证空 token 和非法 URL 被拒绝。
  - `sync_login_direct_options_parse` 验证 CLI `--token`、`--token/--url` 参数解析与 help 输出中的 token 获取地址。
- E2E 测试：
  - `e2e-tests/tests/test_sync_login_direct_e2e.sh` 启动隔离数据目录、动态端口 Bifrost 与本地 mock sync server，执行 `bifrost sync login --token ci-token-default` 验证省略 `--url` 使用内置默认 Provider。
  - 同一脚本先将 sync config 指向 mock server，再执行 `bifrost sync login --token ci-token`，验证 token-only 走当前配置且最终授权成功。
  - 同一脚本保留 `bifrost sync login --token ci-token --url <mock-url>` 显式 URL 回归，并验证 API token-only payload 成功、URL-only payload 返回 400。
- 真实场景测试：
  - 更新 `human_tests/api-sync.md`，新增 CI/沙箱 token-only 默认 URL、token+url 直登和 URL-only 错误用例。
  - 更新 `human_tests/readme.md` 索引后，按新增用例真实执行。

## 校验要求

- 先执行新增 E2E 脚本和 human_tests 用例。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 最后执行 `cargo test --workspace --all-features`。

## 文档更新要求

- 本设计文档记录 API/CLI 合约。
- `human_tests/api-sync.md` 和 `human_tests/readme.md` 必须同步更新。
