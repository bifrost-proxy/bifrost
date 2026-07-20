# Remote Relay URL Fallback

> 状态：已交付并回归 | 关联：`design/api-sync.md`

## 背景

`bifrost remote connect` 及同组 `bifrost remote *` 命令此前只支持两种 relay URL 来源：

1. 显式传入 `--relay-url`；
2. 从运行中的本地 Bifrost `/_bifrost/api/sync/status` 读取 `remote_base_url`。

当本地实例未运行时，CLI 会直接报 `--relay-url is required`，无法继续回退到本地配置文件，也无法使用项目默认 relay 地址。这与 `start` 路径已经使用 `sync.remote_base_url` 的行为不一致，导致：

- CI / 沙箱 / macOS Launch Agent 首次拉起 remote 命令时必然失败；
- 用户即使把 relay URL 写入 `config.toml` 也无法跳过 `bifrost start`；
- 内部默认 relay `${BIFROST_DEFAULT_REMOTE_URL}` 每次都得手动传，与 `SyncConfig::default()` 硬编码不一致。

## 用户目标验证清单

### 必须实现

- `bifrost remote` 全组命令统一 relay URL 解析优先级：
  1. CLI 显式参数 `--relay-url`
  2. 正在运行的本地 Bifrost 实例中的 `sync.remote_base_url`（`/_bifrost/api/sync/status`）
  3. 本地配置文件 `config.toml` 中的 `sync.remote_base_url`
  4. 默认常量 `bifrost_storage::DEFAULT_REMOTE_BASE_URL`（`${BIFROST_DEFAULT_REMOTE_URL}`）
- 空字符串 / 空白字符视为“未配置”，跳过该层。
- 默认值仅在前三层都未命中时生效；不能把默认值误判为“配置层命中”。
- “运行环境”明确指本地 Bifrost 服务，不引入任何环境变量兜底。

### 必须不破坏

- `bifrost start` 路径继续从 `sync.remote_base_url` 初始化 remote invoke worker。
- `SyncConfig::default()` 继续使用同一个默认常量。
- 显式 `--relay-url` 仍然拥有最高优先级，可用于临时调试。

### 必须真实验证

- 单测：`crates/bifrost-cli/src/main.rs` 的 `normalize_relay_url` / `select_remote_relay_url` / `read_configured_relay_url_*` 系列（`~L810 +`）。
- E2E：`e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`。
- Human tests：`human_tests/remote-invoke.md` 中 URL fallback 相关回归条目。

## 产品语义

Relay URL 是一个纯读的字符串配置：

- 优先取显式 > 运行时 > 磁盘 > 默认。
- 每一层解析都必须调用 `normalize_relay_url()` 做 trim + 空串过滤。
- 只有 `select_remote_relay_url()` 返回默认值时才用 `DEFAULT_REMOTE_BASE_URL`，其它路径必须诚实地反映配置来源。

不在设计范围：环境变量、TOML `[remote]` 段、CLI profile、agent config；这些如果未来需要，另行设计并单独 opt-in。

## 技术细节

### 1. 统一解析入口（`crates/bifrost-cli/src/main.rs`）

- `normalize_relay_url(value: Option<String>) -> Option<String>`（`:47`）：trim 后为空字符串则返回 `None`。
- `select_remote_relay_url(explicit, runtime, configured)`（`:54`）：依次尝试三层，均无则回退到 `DEFAULT_REMOTE_BASE_URL`。
- `read_runtime_relay_url(port)`（`:65`）：调用 `ConfigApiClient::get_sync_status()`，取 `status.remote_base_url` 并 normalize。
- `read_configured_relay_url(data_dir_path)`（`:73`）：读取 `<data_dir>/config.toml` 中的 `sync.remote_base_url`，normalize。
- `resolve_remote_relay_url(explicit, cli_port)`（`:85`）：编排 runtime + configured + default，并把结果给 `Commands::Remote`（`:482`）统一使用。

### 2. 配置来源

- 磁盘字段：`config.toml` 的 `sync.remote_base_url`。
- 运行时端点：`GET /_bifrost/api/sync/status` 返回 `remote_base_url`。
- `bifrost start` 路径继续从 `sync.remote_base_url` 初始化 remote invoke worker。

### 3. 共享默认常量

`crates/bifrost-storage/src/lib.rs` 暴露由 Base64 host 按需解码的共享默认值：

```rust
pub static DEFAULT_REMOTE_BASE_URL: LazyLock<String> = LazyLock::new(decode_default_remote_url);
```

供以下位置复用（`grep DEFAULT_REMOTE_BASE_URL`）：

- `SyncConfig::default()`（`crates/bifrost-storage/src/unified_config.rs`）
- `bifrost remote` fallback（`crates/bifrost-cli/src/main.rs:4`）
- `bifrost start` remote invoke worker 初始化（`crates/bifrost-cli/src/commands/start.rs`）
- Admin `sync` handler / API 层（`crates/bifrost-admin/src/handlers/sync.rs`）

这样任何一次修改都不会漂移。

## CLI + Web + Admin API

- CLI：`bifrost remote` 全组子命令的 `--relay-url` 变为可选（此前必须显式提供）。CLI help 需说明四级优先级。
- Web：Sync 设置页展示 relay URL 时按同一优先级排序，且清空输入等价于 `未配置`（回退下一层）。
- Admin API：`GET /_bifrost/api/sync/status` 与 `PUT /_bifrost/api/sync/config` 的 `remote_base_url` 语义保持一致。

## Sync 边界

Relay URL 属于本机运行时配置，不通过 rule / group sync 广播；跨设备一致性由用户手动或部署脚本负责。

## Phase 拆分

### Phase 1：抽出统一解析

- `normalize_relay_url` / `select_remote_relay_url` / `resolve_remote_relay_url`。
- `Commands::Remote` 全部走 `resolve_remote_relay_url`。

### Phase 2：配置文件回退

- `read_configured_relay_url` 读取真实 `config.toml`。
- 保证空文件 / 缺失文件 / 空字段 都不误判为命中。

### Phase 3：默认常量收敛

- `DEFAULT_REMOTE_BASE_URL` 抽到 `bifrost-storage`。
- `SyncConfig::default()` / start / admin sync handler 全部复用。

### Phase 4：测试 + 文档

- 单测覆盖优先级与空值处理。
- E2E 覆盖运行时命中、配置文件命中、默认命中三条路径。
- `docs/cli.md` 与 CLI help 更新四级优先级说明。

## 测试方案

### 单元测试（`crates/bifrost-cli/src/main.rs`）

- `normalize_relay_url_trims_and_rejects_empty_values`（`:810`）：`None` / `""` / `"   "` / `" https://relay/path "` 分别 trim + 判空。
- `select_remote_relay_url_honors_precedence_order`（`:821`）：分别只提供 explicit / runtime / configured / 都不提供，断言各自命中或落到 `DEFAULT_REMOTE_BASE_URL`。
- `read_configured_relay_url_uses_sync_remote_base_url_from_config_file`（`:856`）：真实 `config.toml` 中读出 `sync.remote_base_url`。
- `read_configured_relay_url_treats_empty_config_as_missing`（`:877`）：空文件 / 空字段视为未配置。
- `read_configured_relay_url_returns_none_when_config_file_is_missing`（`:895`）：文件不存在返回 `None`，绝不误命中默认值。

### E2E 测试

`e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`：

- 显式 `--relay-url` 覆盖运行中实例配置：断言最终命中显式值。
- 未传 `--relay-url` 且本地实例正在跑：断言命中 `sync.remote_base_url`。
- 本地实例未启动但存在 `config.toml`：断言命中配置文件值。
- 三层都缺时：断言命中 `DEFAULT_REMOTE_BASE_URL`。

### Human Tests

`human_tests/remote-invoke.md` 已新增回归用例覆盖：

- 显式参数优先。
- 运行中实例配置优先。
- 本地配置文件回退。
- 默认值回退（无本地实例 & 无配置文件）。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核所有 `Commands::Remote` 子命令都走 `resolve_remote_relay_url`，未遗漏历史分支。
- 复核 `read_configured_relay_url` 不吃掉 IO 错误（缺失 = None，损坏 = 错误上抛，避免默默回退）。
- 复测：单测 + E2E 全套。

### 第 2 轮

- 复核 `SyncConfig::default()` 与 CLI fallback 引用同一个常量。
- 复核 CLI help 展示优先级顺序，`docs/cli.md` 同步。
- 复测：跨平台跑一遍 fallback E2E（含 macOS Launch Agent 场景）。

## 风险与决策

| 风险 | 缓解 |
|---|---|
| 用户改本地 `config.toml` 忘 restart | `read_configured_relay_url` 每次都实时读盘；无缓存 |
| 运行时 status 与磁盘配置漂移 | 明确优先级：运行时 > 磁盘，避免同一台机器上两套值互相覆盖 |
| 默认值升级需要同步多处 | 单一常量 `DEFAULT_REMOTE_BASE_URL`，编译期检查 |
| 空字符串误命中 | `normalize_relay_url` 是唯一入口；每层解析都必须过 |

## 校验要求

- 先执行 remote 相关 E2E 回归：`bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`。
- 再执行 `cargo test --workspace --all-features`。
- 再执行 `cargo fmt --all -- --check`。
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 按项目要求执行 `bash scripts/ci/local-ci.sh --skip-e2e`。
- 全绿后 commit + push + MR + 远端 CI 看护。
