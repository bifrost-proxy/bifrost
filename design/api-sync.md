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

## 启动登录预检方案

- 新安装默认开启 Sync 模块：`sync.enabled = true`，默认远端仍为内置 Bifrost Provider `https://bifrost.bytedance.net`。
- 服务启动时，如果本地已经有 sync session token，则不为了“是否弹登录页”额外探测远端，也不自动打开登录页；后续同步 tick 仍按已登录同步逻辑运行。
- 服务启动时，如果没有 sync session token，且此前从未自动弹过启动登录页，则进入 startup login preflight。
- Startup login preflight 最多探测当前 `remote_base_url` 3 次，探测目标沿用 `GET /v4/sso/check`；只有返回成功或 401 这类可达结果时，才自动打开登录页。
- 如果 3 次探测后仍不可达，本次启动不弹登录页，并结束 startup login preflight；未登录状态下不再继续按 `probe_interval_secs` 高频探测远端。
- 自动打开登录页后，必须持久化记录“已经自动弹过”，后续重启也不再自动弹；这是为了避免用户反复看到登录窗口而误以为功能异常。
- 调试环境可通过 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 禁用启动自动登录引导；该开关只跳过 startup login preflight 和自动打开浏览器，不关闭 Sync 配置，也不影响手动 `bifrost sync login`。
- 手动登录不受自动弹窗去重限制：`bifrost sync login` 或 Admin API `POST /api/sync/login` 空 body 仍会强制打开浏览器。
- `logout` 仍会清除当前进程内的手动弹窗节流状态，但不会清除“已经自动弹过”的持久化记录；自动登录引导默认一生只弹一次。

## 依赖项

- `crates/bifrost-cli`：CLI 参数和 Config API client。
- `crates/bifrost-admin`：`/api/sync/login` 请求体解析与直接保存入口。
- `crates/bifrost-sync`：session token 与 remote url 的持久化。
- `crates/bifrost-storage`：Sync 默认配置。

## 测试方案

- 单元测试：
  - `sync_config_defaults_to_enabled` 验证新安装默认开启 Sync 且默认远端为 `https://bifrost.bytedance.net`。
  - `save_login_session_updates_remote_url_and_token` 验证 token 与 URL 会落入状态和配置，且启用 auto sync。
  - `save_login_session_rejects_empty_or_invalid_input` 验证空 token 和非法 URL 被拒绝。
  - `startup_login_preflight_*` 系列验证无 token 时最多 3 次探测、可达时只自动打开一次、不可达不弹、已有 token 不探测、已经自动弹过后跨重启不再弹，以及 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 禁用自动弹窗。
  - `sync_login_direct_options_parse` 验证 CLI `--token`、`--token/--url` 参数解析、缺少 token 值时进入业务校验，以及 help 输出中的 token 获取地址。
  - `sync_token_login_url_*` 验证缺少 token 值时默认使用 `https://bifrost.bytedance.net/v4/sso/token-login`，显式自定义 relay URL 时拼接 `<relay>/v4/sso/token-login` 且不会出现双斜杠。
- E2E 测试：
  - `e2e-tests/tests/test_sync_login_direct_e2e.sh` 验证 `bifrost sync login --token` 缺少 token 值时返回明确错误，并展示默认 token 获取地址。
  - 同一脚本验证 `bifrost sync login --token --url <mock-url>` 缺少 token 值时展示自定义 relay 的 token 获取地址。
  - `e2e-tests/tests/test_sync_login_direct_e2e.sh` 启动隔离数据目录、动态端口 Bifrost 与本地 mock sync server，执行 `bifrost sync login --token ci-token-default` 验证省略 `--url` 使用内置默认 Provider。
  - 同一脚本先将 sync config 指向 mock server，再执行 `bifrost sync login --token ci-token`，验证 token-only 走当前配置且最终授权成功。
  - 同一脚本保留 `bifrost sync login --token ci-token --url <mock-url>` 显式 URL 回归，并验证 API token-only payload 成功、URL-only payload 返回 400。
  - `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 使用隔离数据目录和 mock sync server，验证启动无 token 时可达才自动打开登录页一次、重启后不再自动打开，且 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 时不自动打开。
- 真实场景测试：
  - 更新 `human_tests/api-sync.md`，新增 CI/沙箱 token-only 默认 URL、token+url 直登和 URL-only 错误用例。
  - 更新 `human_tests/api-sync.md`，新增 `bifrost sync login --token` 缺少 token 值时的默认 token 获取地址与自定义 relay 地址回归用例。
  - 更新 `human_tests/api-sync.md`，新增启动登录预检用例，覆盖无 token、可达自动弹一次、持久化后重启不重复弹、不可达不弹，以及调试环境变量禁用自动弹窗。
  - 更新 `human_tests/readme.md` 索引后，按新增用例真实执行。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核默认配置、startup preflight 状态机、持久化去重、手动登录 force 行为；执行 `git status --short`、`git diff`、定向单测和新增 E2E，修复发现的问题。
- 第 2 轮：复查第 1 轮修复后的最新 diff、human_tests 索引和未登录 tick 行为；复跑受影响单测、E2E 和 human_tests，用最新结果决定是否追加轮次。

## 校验要求

- 先执行新增 E2E 脚本和 human_tests 用例。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 最后执行 `cargo test --workspace --all-features`。

## 文档更新要求

- 本设计文档记录 API/CLI 合约。
- `human_tests/api-sync.md` 和 `human_tests/readme.md` 必须同步更新。
