# Remote Relay URL Fallback

## 背景

`bifrost remote connect` 及同组 `bifrost remote *` 命令此前只支持两种 relay URL 来源：

1. 显式传入 `--relay-url`
2. 从运行中的本地 Bifrost `/_bifrost/api/sync/status` 读取 `remote_base_url`

当本地实例未运行时，CLI 会直接报 `--relay-url is required`，无法继续回退到本地配置文件，也无法使用项目默认 relay 地址。这与 `start` 路径已经使用 `sync.remote_base_url` 的行为不一致。

## 目标

统一 `bifrost remote` 全组命令的 relay URL 解析策略，固定优先级为：

1. CLI 显式参数 `--relay-url`
2. 正在运行的本地 Bifrost 实例中的 `sync.remote_base_url`
3. 本地配置文件 `config.toml` 中的 `sync.remote_base_url`
4. 默认值 `https://bifrost.bytedance.net`

其中“运行环境”明确指正在运行的本地 Bifrost 服务，而不是环境变量。

## 实现方案

### 1. 抽出统一解析函数

在 `crates/bifrost-cli/src/main.rs` 中抽出 `resolve_remote_relay_url(...)`，供 `Commands::Remote` 统一调用。

- `normalize_relay_url` 负责去空白并把空串视为未配置
- `read_runtime_relay_url` 负责调用 `ConfigApiClient::get_sync_status()`
- `read_configured_relay_url` 负责读取本地数据目录下真实存在的 `config.toml`
- `select_remote_relay_url` 负责按优先级收敛多个候选来源

### 2. 配置来源保持一致

- 配置文件字段继续使用 `sync.remote_base_url`
- 运行中实例读取继续使用 `/_bifrost/api/sync/status`
- `start` 路径仍然从 `sync.remote_base_url` 初始化 remote invoke worker

### 3. 默认值收敛

把 `https://bifrost.bytedance.net` 提取为共享常量 `bifrost_storage::DEFAULT_REMOTE_BASE_URL`，供：

- `SyncConfig::default()`
- `bifrost remote` relay URL 回退
- `start` 路径 remote invoke worker 初始化

共同复用，避免多处硬编码漂移。

## 测试方案

### 单元测试

- 验证 `normalize_relay_url` 会 trim 且忽略空字符串
- 验证 `select_remote_relay_url` 的四级优先级
- 验证本地配置文件存在时可读出 `sync.remote_base_url`
- 验证本地配置文件缺失时不会把“默认值”误判为“配置层命中”

### E2E 测试

新增 shell E2E：`e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`

- 显式 `--relay-url` 覆盖运行中实例配置
- 未传 `--relay-url` 时命中运行中实例配置
- 本地实例不可用时回退命中本地 `config.toml`

### Human Tests

更新 `human_tests/remote-invoke.md`，新增回归用例覆盖：

- 显式参数优先
- 运行中实例配置优先
- 本地配置文件回退
- 默认值回退

## 校验要求

- 先执行 remote 相关 E2E 回归
- 再执行 `cargo test --workspace --all-features`
- 再执行 `cargo fmt --all -- --check`
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 按项目要求执行 `bash scripts/ci/local-ci.sh --skip-e2e`
