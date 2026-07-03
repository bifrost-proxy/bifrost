# MCP OAuth

## 背景

Bifrost 的 MCP 客户端（`crates/agent/src/mcp/`）需要与远端 MCP Streamable HTTP transport 建立 OAuth 2.0 授权关系，并把颁发的 token（`access_token`、`refresh_token`、`expires_at`、`scope`、`resource`、`server_name`、`url` 等元数据）持久化，以便后续会话复用、自动刷新。

MCP 官方规范建议 token 存到系统 keyring（macOS Keychain / Windows Credential Manager / Secret Service DBus），但真实用户环境差异很大：

- macOS 桌面环境 keyring 一般可用。
- Windows 桌面 keyring 通常可用，headless / RDP 场景可能失败。
- Linux 桌面用户：Secret Service 可能存在但未解锁；`gnome-keyring-daemon` 可能不完整；CI 容器 / SSH-only 场景通常完全不可用。
- 各系统上 `keyring` crate 「entry 能创建」不代表「set/get 能可靠 roundtrip」；`is_available()` 必须真实读写才可靠。

因此 Bifrost 参考 Codex 提供 4 种存储模式 `OAuthCredentialsStoreMode`：`Auto` / `File` / `Keyring` / `Ephemeral`，默认 `Auto`：keyring 可用则用 keyring，否则回退文件；用户可显式选择。

主要实现在 `crates/agent/src/mcp/oauth.rs`，token 落盘走 `OAuthTokenStore`（`{data_dir}/oauth/{server_name}.json`），keyring 走 `KeyringTokenStore`（`keyring` crate，仅 `keyring-store` feature 启用时参与）。

## 用户目标验证清单

### 必须实现

- 支持四种模式 `OAuthCredentialsStoreMode`：
  - `Auto`（默认）：keyring 可用则 keyring，否则文件。
  - `File`：强制文件。
  - `Keyring`：强制 keyring，`keyring-store` feature 未启用时 warn 并降级到文件。
  - `Ephemeral`：仅内存，进程退出后丢失，用于短时 CLI 会话或 e2e。
- `KeyringTokenStore::is_available()` 通过临时 `set/get/delete` roundtrip 判断真实可用，不只判断 entry 是否可创建。
- `Auto` 模式 `save` 时先写 keyring 再立即 `load` 校验，`access_token` 不一致或失败则降级写入文件；`load` 先试 keyring 未命中/失败再回退文件；`delete` 先 keyring 后文件，任一成功即视为已删除。
- 在 headless Linux CI / Secret Service 不完整 / DBus 不通 / credential backend 无法持久读写等场景下，`Auto` 模式必须能落到文件路径，用户下次仍可加载。
- Token key 与 Codex 对齐：`compute_key(server_name, url) = "{server_name}|{sha256_prefix_16}"`，避免不同 URL 的同名 server 相互覆盖。
- Token refresh：`StoredOAuthTokens` 保存 `expires_at`；`needs_refresh()` 按预留时钟偏移判断；`OAuthPersistor` 在 `persist_if_needed` 中执行刷新并持久化最新 token。

### 必须不破坏

- 未启用 `keyring-store` feature 编译时不引入 `keyring` crate 符号；`Keyring` 模式 warn 后降级；`Auto` 模式直接走文件。
- `OAuthTokenStore` 的文件路径 `{data_dir}/oauth/{server_name}.json` 语义不变；同名 server + 不同 URL 走不同文件名（通过 `sanitize_filename`）。
- PKCE / authorization URL / discovery / metadata 解析行为保持稳定；`OAuthServerMetadata` 解析容忍最小字段。
- `StoredOAuthTokens` 序列化对老版本 tokens 兼容：`refresh_token` / `scope` / `resource` 可缺失。

### 必须真实验证

- headless Linux（无 DBus、无 Secret Service）下 `Auto` 模式 `save -> load` 全流程走文件路径且 token 可读回。
- macOS / Windows 桌面机 keyring 可用时 `Auto` 模式走 keyring；卸载 `keyring-store` feature 或触发 keyring 写失败后自动降级文件，用户体感一致。
- 同一 `server_name` 但两个不同 `url` 的 token 互不覆盖。

## 产品语义

### `OAuthCredentialsStoreMode`

```rust
pub enum OAuthCredentialsStoreMode {
    /// 默认：keyring 可用则 keyring，否则文件。
    Auto,
    /// 强制文件；不加载 keyring。
    File,
    /// 强制 keyring；未启用 feature 时 warn 后降级文件。
    Keyring,
    /// 仅内存，进程退出后丢失。
    Ephemeral,
}
```

### Auto 模式的三个操作对称性

- `save`：keyring 写 -> keyring load 校验 -> 不一致或失败 -> 文件写。
- `load`：keyring 查 -> 未命中或失败 -> 文件查。
- `delete`：keyring 删 -> 文件删；任一成功视为已删除，避免 keyring 记录残留。

### 兼容 Codex 的 key 计算

`compute_key(server_name, url) = format!("{server_name}|{}", sha256(url)[..16])`。相同 server_name 的不同 URL 生成不同 key，避免在同一 keyring service 下互相覆盖；同时 key 长度可控、不含 URL 中的敏感 path。

## 技术细节

### 主要类型（`crates/agent/src/mcp/oauth.rs`）

- `OAuthCredentialsStoreMode`（`Auto` / `File` / `Keyring` / `Ephemeral`）。
- `StoredOAuthTokens`（`access_token` / `refresh_token` / `token_type` / `expires_at` / `scope` / `resource` / `server_name` / `url` / `id_token` / `updated_at`）。
- `TokenResponse`（OAuth `/token` 响应）。
- `OAuthTokenStore`（文件存储，`{data_dir}/oauth/{server_name}.json`）。
- `KeyringTokenStore`（`KEYRING_SERVICE` 常量 + `compute_key(server_name, url)`；`is_available()` 通过 `set/get/delete` probe 判断可用性）。
- `OAuthPersistor`（刷新 + 持久化的自动化管理器）。
- `OAuthServerMetadata`（discovery 解析）。
- `PkceChallenge` / `generate_pkce_challenge` / `generate_state`。

### 顶层入口

```rust
pub fn save_oauth_tokens_with_mode(
    tokens: &StoredOAuthTokens,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<()>;

pub fn load_oauth_tokens_with_mode(
    server_name: &str,
    url: &str,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<Option<StoredOAuthTokens>>;

pub fn delete_oauth_tokens_with_mode(
    server_name: &str,
    url: &str,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<bool>;
```

- `Ephemeral`：`save` 返回 Ok 但不持久；`load` 恒返回 `None`；`delete` 返回 `Ok(false)`。
- `File`：直接走 `OAuthTokenStore`。
- `Keyring`（feature on）：走 `KeyringTokenStore`；feature off 时 warn 并降级到 `OAuthTokenStore`。
- `Auto`：先 keyring（若 `is_available()`），失败或未命中回退文件；见「Auto 模式对称性」。

### Keyring 可用性 probe

`KeyringTokenStore::is_available()` 步骤：

1. 使用固定 probe key（例如 `__bifrost_probe__`）。
2. `set_password` 一个随机 probe 值。
3. `get_password` 读回并比对。
4. `delete_password` 清理。
5. 只有全部成功且读写值相等才返回 `true`。

### 依赖项

- Rust `keyring` crate，仅在 `keyring-store` feature 启用时参与；服务名常量 `KEYRING_SERVICE`。
- 文件 fallback 使用 `OAuthTokenStore`，token 落盘到 `{data_dir}/oauth/{server_name}.json`。
- 不引入 keychain / DBus 之外的系统依赖。

### 相关文件

- `crates/agent/src/mcp/oauth.rs`
- `crates/agent/src/mcp/mod.rs`（`OAuthCredentialsStoreMode` 暴露）
- `crates/agent/src/mcp/` 内 discovery / http transport 消费方
- `human_tests/mcp-oauth.md`
- `human_tests/readme.md`
- `design/mcp-oauth.md`

## CLI + Web + Admin API

- CLI：MCP 相关命令通过配置文件 / 环境变量选择 `OAuthCredentialsStoreMode`；默认 `Auto`。
- Web：MCP 配置 UI 可选存储模式，展示当前 token 是否命中 keyring 或文件（可扩展）。
- Admin API：MCP server 配置 CRUD 接口可携带 `credentials_store_mode` 字段；token 本身不通过 Admin API 明文暴露。

## Sync 边界

- OAuth token 是**本机 MCP 授权凭据**，不参与 Bifrost 规则 / Group / 记忆的远端 sync。
- 用户跨机迁移时应重新在新机上完成 OAuth 授权流；不建议手工复制 keyring 项或 `oauth/*.json` 文件。
- `Ephemeral` 模式适合 CI / 一次性 CLI；进程退出后不会残留任何 token。

## Phase 1-4

### Phase 1：文件存储与 PKCE 底座

- `StoredOAuthTokens` / `OAuthTokenStore` / PKCE / state / discovery / server metadata 完成。
- `save_oauth_tokens_with_mode` / `load_...` / `delete_...` API 完整；`File` / `Ephemeral` 模式可用。

### Phase 2：keyring 支持

- `keyring-store` feature 引入 `keyring` crate；`KeyringTokenStore` `save` / `load` / `delete` 实现。
- `compute_key(server_name, url)` 与 Codex `compute_store_key` 对齐。

### Phase 3：Auto 模式与可用性 probe

- `KeyringTokenStore::is_available()` 通过 `set/get/delete` probe。
- `Auto` `save` 后立即 `load` 校验，`access_token` 不一致或失败降级到文件。
- `Auto` `load` / `delete` 双路径顺序回退。
- headless Linux CI 场景走文件路径。

### Phase 4：Token 刷新与运维

- `OAuthPersistor` 自动刷新 + 持久化，`needs_refresh()` 考虑时钟偏移。
- `sanitize_filename` 处理特殊字符；`urlencoding_decode` 覆盖 metadata 兼容。
- 未启用 `keyring-store` feature 时 `Keyring` warn 后降级、`Auto` 直接走文件。

## 测试方案

### 单元测试（`crates/agent/src/mcp/oauth.rs::tests`）

真实存在的测试，可用 `cargo test -p bifrost-agent mcp::oauth::tests` 覆盖：

- `test_stored_tokens_serialization_roundtrip`
- `test_stored_tokens_minimal_deserialization`
- `test_token_response_deserialization`
- `test_token_response_minimal`
- `test_needs_refresh_no_expiry`
- `test_needs_refresh_future_expiry`
- `test_needs_refresh_expired`
- `test_needs_refresh_within_skew`
- `test_pkce_challenge_generation`
- `test_pkce_uniqueness`
- `test_generate_state`
- `test_build_authorization_url`
- `test_build_authorization_url_no_scopes_no_resource`
- `test_discovery_paths_root`
- `test_discovery_paths_with_path`
- `test_discovery_paths_empty`
- `test_sanitize_filename`
- `test_urlencoding_decode`
- `test_token_store_save_load_delete`
- `test_oauth_server_metadata_deserialization`
- `test_oauth_server_metadata_minimal`
- `test_current_unix_secs_reasonable`
- `test_hex_val`
- `test_oauth_persistor_creation`
- `test_oauth_persistor_without_refresher`
- `test_oauth_persistor_persist_if_needed_unchanged`
- `test_oauth_persistor_persist_if_needed_changed`
- `test_oauth_persistor_refresh_not_needed`
- `test_credentials_store_mode_default`
- `test_save_load_with_file_mode`
- `test_save_load_with_auto_mode_fallback_to_file`（Auto 模式在 keyring 不可用/不可靠时回退文件的核心回归）

### E2E 测试

- 不需要新增独立 E2E 脚本：OAuth token store 是本地持久化选择逻辑，单元测试可完全覆盖 CI 失败路径。
- MCP OAuth 完整授权流会在 `crates/bifrost-e2e/` 的 MCP 集成用例（如 `mcp_streamable_http_*`）中被间接触达。

### 真实场景测试（`human_tests/mcp-oauth.md`）

- `TC-MCP-OAUTH-01`：执行 `Auto` 模式 fallback 回归命令 `cargo test -p bifrost-agent mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file --all-features -- --nocapture`。
- 桌面 macOS / Windows 手动完成一次真实 MCP OAuth 授权 -> 关闭 CLI -> 重开 -> 能自动 refresh 并复用。
- 桌面 Linux + Secret Service 已解锁场景下的授权 / 复用。
- headless Linux（SSH-only，未安装 gnome-keyring）授权 -> 复用走文件路径。
- 显式 `--credentials-store-mode file` / `--credentials-store-mode ephemeral` 覆盖路径。
- 同一 `server_name` 但两个不同 `url` 授权，token 文件互不覆盖，keyring key 互不冲突。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 用户目标：`Auto` fallback 正确、`File` / `Keyring` / `Ephemeral` 语义清晰、`is_available()` 通过真实 roundtrip 判断、`compute_key` 与 Codex 对齐。
- 变更范围：`git status --short` 覆盖 `crates/agent/src/mcp/oauth.rs`、`human_tests/mcp-oauth.md`、`human_tests/readme.md`。
- 重点：`Auto` `save` 校验流程；未启用 `keyring-store` feature 的 fallback；`sanitize_filename` 边界；`OAuthPersistor` 刷新是否落盘。
- 复测：`cargo test -p bifrost-agent mcp::oauth::tests --all-features`；`cargo test -p bifrost-agent mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file --all-features -- --nocapture`。

### 第 2 轮

- 检查 `compute_key` 与 Codex 版本是否漂移；不同 URL 的同 server 是否互不覆盖。
- 关注 headless Linux CI（无 DBus）路径：`KeyringTokenStore::is_available()` 必须返回 `false`。
- 手动执行 human_tests 中的桌面授权 + 复用 + fallback 回归。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent mcp::oauth::tests --all-features`
- `cargo test -p bifrost-agent mcp::oauth::tests::test_save_load_with_auto_mode_fallback_to_file --all-features -- --nocapture`
- 提交前按仓库规则执行 `cargo test --workspace --all-features`。

## 文档更新要求

- 更新 `human_tests/mcp-oauth.md`（新增 fallback / 显式模式 / 多 URL 用例）。
- 更新 `human_tests/readme.md` 索引。
- MCP 配置文档补充 `credentials_store_mode` 字段说明与默认值。

## 风险与决策

- **keyring 可用性通过真实 roundtrip 判断**：仅判断 entry 是否可创建的旧路径在 headless Linux / 未解锁 Secret Service 上会误判成功，导致 token 保存后无法读回。用户体感表现是「授权成功但每次都要重授」。
- **`Auto` 模式 save 后立即 load 校验**：keyring 写入偶尔会因 backend 限制 silent drop，只有 save-then-load 才能真正检测出并降级文件。
- **`compute_key` 使用 `server_name|sha256(url)[..16]`**：与 Codex `compute_store_key` 对齐，未来若需要迁移双向兼容成本最小。
- **不通过 Admin API 明文暴露 token**：只允许通过 CLI 与桌面授权流写入；查询接口只返回是否存在与 `expires_at`，不返回 `access_token`。
- **OAuth token 不参与 sync**：用户跨机迁移应重新授权；避免把用户的授权凭据在网络间传递。
- **`Ephemeral` 模式对 CI 极友好**：不写文件、不写 keyring，进程退出即忘；不会污染宿主机 keyring。
