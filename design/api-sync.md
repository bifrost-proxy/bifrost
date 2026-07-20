# Sync API 设计方案

## 背景

Sync API 负责把本机 Bifrost 的规则、Group、部分配置与远端同步服务绑定。原有 `bifrost sync login` 只支持打开浏览器并等待远端 SSO 回调把 token 写回本地服务；在 CI、沙箱、SSH 无 GUI、Docker 容器或无浏览器环境中无法完成登录。而且服务首次启动时缺少统一的登录引导：老用户不知道 Sync 已默认开启，也没有明确入口指出“可以扫码/直登拿到 token”。

本方案在 CLI 上把 `login` 提升为一级命令并同时保留 `bifrost sync login`，两者对齐支持 `--token / --url` 直登；在 Admin API 上支持 body 直登；在服务启动路径上加一次“login preflight”，可达才自动弹一次登录页，之后不再骚扰用户；同时把默认远端固定为内置 Bifrost Provider。下文的 `${BIFROST_DEFAULT_REMOTE_URL}` 表示从仓库内 Base64 常量按需解码得到的运行时 URL，不是写入源码或配置的字面量。

## 用户目标验证清单

### 必须实现

- CLI 支持一级命令 `bifrost login`，语义等价于 `bifrost sync login`。
- CLI 支持 `bifrost login --token <token>` / `bifrost sync login --token <token>` 直登。
- CLI 支持 `bifrost login --token <token> --url <remote-url>` / `bifrost sync login --token <token> --url <remote-url>` 显式覆盖远端同步服务地址。
- 只提供 `--token` 时，复用当前同步配置中的 `remote_base_url`；新安装默认 `${BIFROST_DEFAULT_REMOTE_URL}`。
- 无参数时保持原有打开浏览器登录行为。
- 只提供 `--url` 返回错误，避免误以为完成登录。
- CLI help 输出必须展示 token 获取地址：`${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login`，自定义 relay 时替换为 `<relay>/v4/sso/token-login`。
- Admin API `POST /_bifrost/api/sync/login` 接收 `{ token, remote_base_url }`，token-only 走当前 remote，token+url 覆盖 remote，空 body 保持原浏览器登录流程。
- 服务启动时如果本地有 sync session token，不再为“是否弹登录页”额外探测远端。
- 服务启动时如果没有 session token 且此前从未自动弹过登录页，进入 startup login preflight：最多 3 次探测 `GET /v4/sso/check`，可达才自动打开登录页；不可达不弹。
- 自动打开登录页后持久化“已经自动弹过”，跨重启不再自动弹。
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 禁用启动自动弹窗，仅跳过 preflight，不关闭 Sync。

### 必须不破坏

- 已有 `bifrost sync login`、`bifrost sync logout`、`bifrost sync status`、`bifrost sync setup` 等命令继续可用。
- 手动 `POST /api/sync/login` 空 body 仍强制打开浏览器登录，不受自动弹窗节流影响。
- 规则同步、Group 同步、Sync tick、rule share、Provider owner 等业务逻辑不受影响。
- 现有 `probe_interval_secs`、`sync.enabled` 语义不变，只是未登录状态下不再高频探测远端。

### 必须真实验证

- CLI 真实 `bifrost login`、`bifrost login --token`、`bifrost sync login --token --url` 都能拿到期望行为，help 中出现 token 获取地址。
- CI 沙箱环境使用 `--token` 直登，不打开浏览器仍能把 token 写入 sync 状态。
- 服务重启后不再自动弹登录页；调试环境变量禁用后不弹。
- Startup login preflight 不可达时不弹，可达时弹一次，重启不再弹。

## 产品语义

### 一级 `bifrost login` 与 `bifrost sync login` 完全等价

`bifrost login` 是给新用户和 CI 的“低学习成本”一级命令。它与 `bifrost sync login` 在参数、help、错误消息、行为上完全对齐；两者共享同一个 Admin API 路径与业务逻辑，只是 CLI 顶层入口不同。`bifrost sync login` 保留是为了兼容既有脚本与文档。

Help 输出必须同时告知：

- 无参数走浏览器登录。
- 使用 `--token` 或 `--token/--url` 走 CI/沙箱直登。
- token 获取地址：`${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login`；自定义 relay 时是 `<relay>/v4/sso/token-login`（不允许双斜杠）。

### Token 直登遵循 “url-only 是错误” 原则

`--url` 只在 “更改远端地址” 语义下有意义；单独提供 `--url` 而不带 `--token` 会让用户误以为已经完成登录。所以直登组合是：

- `--token`：走当前配置 remote。
- `--token --url <remote>`：更新 remote 并直登。
- `--url` 只出现：Admin API 返回 400 `token is required`，CLI 同步返回失败。

### Startup Login Preflight 是“一生一次”的柔性引导

服务启动路径上没有登录 token 时，才有必要考虑“是否要提示用户登录”。preflight 的核心策略：

- 有 token：完全跳过，靠后续 sync tick 保持登录状态。
- 无 token + 未曾自动弹过：最多探测 3 次 `GET /v4/sso/check`；可达才自动打开 `/v4/sso/login?next=...`；不可达不弹。
- 自动弹一次后写入持久化标记，跨重启永远不再自动弹，避免用户误以为“Bifrost 一直骚扰我登录”。
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 只影响 preflight 与自动打开浏览器，不影响 sync config、手动 login、tick。
- 手动 `bifrost sync login` 或 API 空 body login 不受自动弹窗节流影响，强制打开浏览器。
- `logout` 清理进程内的手动弹窗节流状态，但不清理“已经自动弹过”的持久化记录。

## 技术细节

### CLI 参数与 help（`crates/bifrost-cli/src/cli.rs`）

`SyncLogin` 与顶级 `Login` 命令共享同名字段：

- `token: Option<String>`，help 明确要求“Sync session token for non-interactive login; get one at ${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login”。
- `url: Option<String>`，用于覆盖 remote。
- `long_about` 分行展示：token 获取地址、无参数说明、`bifrost login --token "$BIFROST_SYNC_TOKEN"` 示例、`bifrost login --token "$BIFROST_SYNC_TOKEN" --url ${BIFROST_DEFAULT_REMOTE_URL}` 示例。

自定义 relay 时 help 中的 token 获取地址被替换为 `<relay>/v4/sso/token-login`；拼接必须处理 trailing slash，避免出现 `https://x//v4/sso/token-login`。

### CLI 调用契约

CLI 通过 Config API client 调用 `POST /_bifrost/api/sync/login`，请求体：

```json
{
  "token": "<sync-session-token>",
  "remote_base_url": null
}
```

- token-only：`remote_base_url` 传 `null`，Admin API 使用当前配置。
- token+url：`remote_base_url` 显式传值；Admin API 保存新 remote。
- 空 body / `{}`：Admin API 保持原浏览器登录流程。
- CLI 命令没有 token 参数时进入 clap 校验错误分支，退出码非 0，错误消息包含默认或自定义 token 获取地址。

### Admin API `POST /_bifrost/api/sync/login`（`crates/bifrost-admin/src/handlers/sync.rs`）

请求结构：

```rust
struct SyncLoginRequest {
    token: Option<String>,
    remote_base_url: Option<String>,
}
```

处理分支：

- `token + Some(remote_base_url)`：校验 `remote_base_url` 非空、`http(s)://` 前缀、`url::Url::parse` 通过；裁剪 trailing `/`；调用 `sync_manager.save_login_session(token, remote_base_url).await`。
- `token + None`：读取 `sync_manager.status().await.remote_base_url`；空则回退 `DEFAULT_REMOTE_BASE_URL`；调用 `save_login_session`。
- `None + Some(_)`：`400 token is required`。
- `None + None`（空 body）：走原浏览器登录流程 `sync_manager.start_browser_login()`。

`save_login_session` 内部负责：

- 保存 session token。
- 更新配置 `sync.remote_base_url`。
- 唤醒后台 sync tick。
- 清理登录预检持久化状态里的错误标记（仅在需要重新引导时）。

回调路径 `GET /_bifrost/api/sync/login/callback?token=...` 保持原样，用于浏览器登录回调，调用 `save_token(token)` 并渲染成功页。

### Sync 配置默认值（`crates/bifrost-storage/src/unified_config.rs`）

```rust
const DEFAULT_REMOTE_HOST_BASE64: &str = "<base64-encoded-host>";

pub static DEFAULT_REMOTE_BASE_URL: LazyLock<String> = LazyLock::new(|| {
    let host = decode_base64(DEFAULT_REMOTE_HOST_BASE64);
    format!("https://{host}")
});

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            remote_base_url: DEFAULT_REMOTE_BASE_URL.to_string(),
            // ...
        }
    }
}
```

`SyncConfigUpdate.remote_base_url: Option<String>` 用于 partial update；空字符串通过校验被拒绝。

### Startup Login Preflight（`crates/bifrost-sync/src/manager.rs`）

关键常量与函数：

- `const DISABLE_AUTO_LOGIN_PROMPT_ENV: &str = "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT";`
- `fn startup_login_preflight_retry_delay() -> Duration`：从环境变量读取，默认几百毫秒或 seconds 级；测试通过覆盖 delay 提速。
- `fn startup_login_preflight_disabled_by_env() -> bool`：读取环境变量。
- `async fn startup_login_preflight(&self)`：外层入口。
- `async fn startup_login_preflight_with_delay(&self, retry_delay: Duration)`：真正实现，方便注入 delay 做测试。

流程：

1. 如果 env 禁用 → 直接返回。
2. 如果已经有 session token → 直接返回。
3. 如果已经自动弹过（持久化标记）→ 直接返回。
4. 探测最多 3 次 `GET <remote>/v4/sso/check`：
   - 网络错误 → 重试，间隔 `retry_delay`。
   - HTTP 2xx 或 401 类别（表明服务器可达） → 认为“可达”。
   - HTTP 5xx / 超时 → 重试。
5. 3 次仍不可达 → 结束 preflight，写入 “已探测但不可达” 状态（不再频繁探测）。
6. 可达 → `open <remote>/v4/sso/login?next=<callback>`，写入 “已经自动弹过” 持久化标记。
7. Preflight 结束后正常启动 sync tick loop；未登录状态下 tick 也不会高频探测远端。

Wake channel：`startup_login_preflight_wake_interrupts_retry_wait` 单测确认在等待 retry 间隔时被外部信号唤醒立即返回，不阻塞 sync manager shutdown。

### Sync Manager 状态与调用点

- `crates/bifrost-cli/src/commands/start.rs`：启动主代理前 `sync_manager.startup_login_preflight().await` 与后台 loop 共存。
- `crates/bifrost-cli/src/commands/sync_cmd.rs`：`sync login` 命令解析 `--token/--url`，转发给 Admin API。
- `crates/bifrost-cli/src/commands/tray/tray.rs`：托盘 login/logout 入口不改变自动弹窗持久化标记。
- `crates/bifrost-admin/src/handlers/sync.rs`：处理 login/callback。
- `crates/bifrost-sync/src/client.rs`：HTTP client，`GET /v4/sso/check`、`GET /v4/sso/info`、`POST /v4/sso/logout` 等。

## CLI 交互示例

```bash
# 浏览器登录
bifrost login
bifrost sync login

# CI 沙箱 token-only（默认 remote）
bifrost login --token "$BIFROST_SYNC_TOKEN"
bifrost sync login --token "$BIFROST_SYNC_TOKEN"

# token + 显式 remote
bifrost login --token "$BIFROST_SYNC_TOKEN" --url ${BIFROST_DEFAULT_REMOTE_URL}
bifrost sync login --token "$BIFROST_SYNC_TOKEN" --url https://relay.company.example

# 错误：只提供 url
bifrost login --url https://relay.company.example
# → error: token is required
```

Help 中必须出现：

```
Sync session token for non-interactive login; get one at
  ${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login

Examples:
  bifrost login --token "$BIFROST_SYNC_TOKEN"
  bifrost login --token "$BIFROST_SYNC_TOKEN" --url ${BIFROST_DEFAULT_REMOTE_URL}
```

## Web / Admin UI

本方案不新增 Web UI 页面；Sync 状态与登录入口仍在现有 Settings/Sync 面板。login preflight 由后端自动完成，前端只在下一次 `GET /_bifrost/api/sync/status` 时看到 `is_logged_in=true`。

## Admin API

- `POST /_bifrost/api/sync/login`：body `{ token?, remote_base_url? }`，四种分支见上文。
- `GET /_bifrost/api/sync/login/callback?token=...`：浏览器登录回调，仍返回 HTML 结果页。
- `POST /_bifrost/api/sync/logout`：清理 token，进程内节流状态清空。
- `GET /_bifrost/api/sync/status`：返回 `is_logged_in`、`remote_base_url` 等；startup preflight 完成后 `is_logged_in` 应仍为 false（除非用户已完成登录）。

## Sync / 导入导出 / 分享边界

- Sync 是本方案的主体；本方案不改变具体的 rule sync / group sync 载荷。
- Session token 与 remote_base_url 属于本地登录状态，不参与规则导入导出与 share URL。
- CLI `logout` 只清理本地 session；不会撤销远端 token。若需要远端登出，走 `sync_manager` 的 `POST /v4/sso/logout` 分支（现有能力，本次方案不改）。

## 实现切分

### Phase 1：Admin API 直登 + 默认 Provider

- `SyncConfig::default()` 固定 `enabled=true`、`remote_base_url=DEFAULT_REMOTE_BASE_URL`。
- `POST /_bifrost/api/sync/login` 接收 `{ token, remote_base_url }`，四分支处理。
- `SyncManager::save_login_session(token, remote_base_url)` 落地。
- 单元测试覆盖 default、四分支、非法 URL、空 token。

### Phase 2：CLI `--token / --url` 与一级 `login`

- `crates/bifrost-cli/src/cli.rs` 新增 `Login` 顶层命令，与 `SyncLogin` 对齐字段。
- help/long_about 展示 token 获取地址；自定义 relay 时替换。
- 单元测试 `sync_token_login_url_*`、`sync_login_direct_options_parse`、`top_level_login_options_parse_like_sync_login`。

### Phase 3：Startup Login Preflight

- `startup_login_preflight` / `_with_delay` 实现最多 3 次探测。
- 持久化 “已经自动弹过” 标记；跨重启读取。
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 短路。
- Wake channel 打断 retry 等待。
- 单元测试 `startup_login_preflight_*` 系列覆盖第三次可达、始终不可达、env 禁用、wake、跨重启不再弹。

### Phase 4：E2E + human_tests

- 新增 `e2e-tests/tests/test_sync_login_direct_e2e.sh` 覆盖 token-only 默认、token+url、URL-only 错误、缺 token help、一级 login 与 sync login 等价。
- 新增 `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 覆盖首次可达自动弹一次、重启不再弹、env 禁用不弹。
- 更新 `human_tests/api-sync.md` 与索引，逐条执行。

## 测试方案

### 单元测试

- `bifrost-storage::test_unified_config_default`：`SyncConfig::default().enabled=true` 且 `remote_base_url = DEFAULT_REMOTE_BASE_URL`。
- `save_login_session_updates_remote_url_and_token`：token 与 URL 落入状态和配置并 enable auto sync。
- `save_login_session_rejects_empty_or_invalid_input`：空 token / 非法 URL 被拒绝。
- `startup_login_preflight_opens_once_when_third_probe_is_reachable`。
- `startup_login_preflight_stops_after_three_unreachable_probes`。
- `startup_login_preflight_skips_when_disabled_by_env`。
- `startup_login_preflight_skips_when_auto_prompt_was_persisted`。
- `startup_login_preflight_wake_interrupts_retry_wait`。
- `startup_login_preflight_retry_delay_reads_env`。
- `sync_login_direct_options_parse`：`--token`、`--token/--url` 参数解析、缺 token 值时业务错误、help 输出的 token 获取地址。
- `top_level_login_options_parse_like_sync_login`：一级 `bifrost login` 的 help、`--token`、缺 token 值、`--token/--url` 与 `bifrost sync login` 等价。
- `sync_token_login_url_*`：默认 `${BIFROST_DEFAULT_REMOTE_URL}/v4/sso/token-login`；自定义 relay 时 `<relay>/v4/sso/token-login` 且不出现双斜杠。

### E2E 测试

- `e2e-tests/tests/test_sync_login_direct_e2e.sh`：
  - `bifrost sync login --token`（缺值） → 明确错误，展示默认 token 获取地址。
  - `bifrost sync login --token --url <mock-url>`（缺值） → 展示自定义 relay token 获取地址。
  - 一级 `bifrost login` 的 help、缺 token 默认/自定义 token 获取地址。
  - 一级 `bifrost login --token ci-token --url <mock-url>` 显式登录成功。
  - 隔离数据目录 + mock server：`bifrost sync login --token ci-token-default` 省略 `--url` 使用内置默认 Provider。
  - sync config 指向 mock server 后 token-only 走当前配置。
  - Admin API 直接 payload：token-only 成功；URL-only 返回 400。
- `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh`：
  - 首次启动 + mock server 可达 → 自动打开一次登录页。
  - 重启 → 不再自动打开。
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` → 不打开。

### 真实场景测试 human_tests

维护 `human_tests/api-sync.md`：

- token-only 默认 URL 直登。
- token+url 直登。
- URL-only 返回错误。
- 缺 token 值时的默认 / 自定义 relay token 获取地址回归。
- 一级 `bifrost login` 与 `bifrost sync login` 等价的真实 CLI 用例。
- 启动登录预检：无 token 首启动可达自动弹一次；持久化后重启不再弹；不可达不弹；env 禁用不弹。

真实执行时使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`；模拟服务器时启用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 避免误弹。

### 覆盖率与项目校验

- `cargo test -p bifrost-storage sync_config`
- `cargo test -p bifrost-admin sync_login`
- `cargo test -p bifrost-sync startup_login_preflight`
- `cargo test -p bifrost-cli sync_login_direct_options_parse top_level_login_options_parse_like_sync_login sync_token_login_url_`
- `bash e2e-tests/tests/test_sync_login_direct_e2e.sh`
- `bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 时不跑 `make coverage`，交付说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认 remote、四分支、preflight 状态机、持久化去重、手动登录 force 行为、一级 `login` 与 `sync login` 等价。
- 复核 diff：`git status --short` / `git diff` 是否覆盖 storage/admin/sync/cli/e2e/human_tests。
- 重点 review：URL 校验、trailing slash 处理、token 获取地址拼接、preflight 持久化 key。
- 复测：定向单测 + 新增 E2E。

### 第 2 轮

- 复核第 1 轮修复：未登录 tick 是否仍高频探测；手动 login 是否绕过节流；env 禁用是否只影响 preflight。
- 再次执行 `git status --short` / `git diff`；检查是否有遗漏 CLI help / long_about 场景。
- 复跑受影响单测、E2E 和 human_tests；出现阻塞新增轮次。

## 风险与决策点

- 默认 Provider 固定为 `${BIFROST_DEFAULT_REMOTE_URL}`：如果需要区域切换，需要单独“区域自动选择”方案，不复用本次直登。
- Preflight 持久化 key 属于本机状态，不参与规则 sync；跨设备不共享，避免用户在多台机器上都被抑制自动引导。
- 3 次探测阈值经验值：目标是覆盖冷启动网络抖动，同时不拖慢代理启动主线。若产品要求更高鲁棒性可提升到 5，但需要同步测试。
- `--url` 只出现返回 400 是显式选择：如果未来允许“先设 url 再手动 login”，应在 Sync UI 中单独提供“修改 remote”入口，而不是复用 login 命令。
- Preflight 与 tray 登录入口的交互：托盘手动登录不受节流影响，行为清晰；如果引入更多“弹窗渠道”（如首屏弹层），需要重新审视节流边界。
- `bifrost sync login` 与 `bifrost login` 双入口维护成本：两者共享同一段业务代码，只是顶层 clap 结构不同；未来任一方新增字段必须双向对齐，测试用例已覆盖等价性回归。
