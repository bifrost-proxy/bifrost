# Remote Relay TLS Trust

> 状态：已交付并回归 | 关联：`design/upgrade-tls-trust.md`

## 背景

`bifrost remote conn up`、`remote exec`、`remote file` 等 caller 侧命令，以及 target 侧 Remote Invoke worker，都需要主动连接 relay。此前这条链路复用 `reqwest` + `rustls-tls` 默认配置，只信任 webpki 公网根证书，不读取系统 CA，也不读取常见沙箱 / CLI CA 环境变量。

在企业出口、Linux 沙箱或受控 CI 环境里，HTTPS relay 可能被内部安全网关做 TLS inspection，服务端证书会被替换为私有根证书签发。curl / openssl 因为读取系统 CA 或 `SSL_CERT_FILE` 可以握手成功，但 Bifrost remote relay client 会在 TLS 握手阶段失败。

## 用户目标验证清单

### 必须实现

- Remote relay HTTP / SSE client 默认同时信任 webpki 公网根 **和** 系统 native root store。
- 支持显式追加私有 CA bundle（文件、path list、目录）。
- 兼容常见工具、沙箱、CI 与语言运行时 CA 环境变量。
- 支持 `BIFROST_REMOTE_UNSAFE_SSL` 作为 remote relay 专用最终兜底开关；默认不启用；不复用代理服务的 `--unsafe-ssl`。
- Relay 请求继续走 `no_proxy()`，不受系统 `HTTP_PROXY` / `HTTPS_PROXY` 影响；PPE header 注入继续使用 `BIFROST_REMOTE_RELAY_HEADERS`。

### 必须不破坏

- 本地代理转发、replay、upgrade check、sync、agent provider 等其它 HTTP client 保持原样。
- 已在系统 trust store 中注册的私有 CA 不需要重新配置。
- 现有 relay URL 解析与端点契约不受影响。

### 必须真实验证

- 单测：`crates/bifrost-core/src/http_client.rs` 的 `remote_relay_*` 系列测试。
- E2E：`e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh` 覆盖三条路径：无 CA 失败 / 配 CA 成功 / unsafe 兜底成功。
- Human tests：`human_tests/remote-invoke-tls-trust.md`。

## 产品语义

Remote relay TLS 信任源按下列顺序**叠加**：

1. webpki built-in roots（`tls_built_in_webpki_certs(true)`）；
2. 平台 native root store（`tls_built_in_native_certs(true)`，来自 `rustls-tls-native-roots` feature）；
3. `openssl-probe` 扫描到的 Linux / Unix 常见系统 CA bundle 与 hashed cert dir（Debian / Ubuntu、RHEL / Fedora、Alpine、OpenSUSE、OpenHarmony 等）；
4. 显式 CA 环境变量（`BIFROST_REMOTE_RELAY_CA_BUNDLE` 优先，其它工具变量作为补充）追加的 PEM bundle；
5. 若 `BIFROST_REMOTE_UNSAFE_SSL` 显式启用，最后叠加 `danger_accept_invalid_certs(true)`，作为不可控环境兜底。

**Unsafe 只影响 remote relay HTTP / SSE client**，不影响本地代理转发、replay、upgrade、sync、agent provider。

## 技术细节

### 1. reqwest feature

`Cargo.toml` workspace `reqwest` feature 追加 `rustls-tls-native-roots`，保持既有 `rustls-tls` 与 webpki roots，同时允许 reqwest 加载平台根证书。

### 2. Remote relay 专用 builder

`crates/bifrost-core/src/http_client.rs` 暴露：

- `remote_relay_reqwest_client_builder()`（`:137`）：从 `direct_reqwest_client_builder().no_proxy()` 出发，显式启用 `tls_built_in_webpki_certs(true)` + `tls_built_in_native_certs(true)`，追加环境变量指向的 CA bundle，按 `BIFROST_REMOTE_UNSAFE_SSL` 决定是否 `danger_accept_invalid_certs(true)`。
- `remote_relay_sse_reqwest_client_builder()`（`:141`）：在前者基础上适配 SSE（`http1_only`、无 idle timeout 等）。

配套辅助（同文件）：

- `REMOTE_RELAY_CA_BUNDLE_ENV`（`:11`）常量、`REMOTE_UNSAFE_SSL_ENV`（`:13`）常量。
- `load_reqwest_certificate_bundle(&path)`（`:170`）解析 PEM。
- `remote_relay_ca_file_paths_from_env()`（`:414`）与 `remote_relay_ca_dir_paths_from_env()`（`:430`）汇总所有支持的环境变量。
- `openssl_probe::probe()`（`:463 / :472`）扫描系统 bundle & hashed dir。

### 3. 私有 CA 环境变量

优先支持 Bifrost 专用变量：

- `BIFROST_REMOTE_RELAY_CA_BUNDLE`：文件或 path-list（平台分隔符）。

同时兼容常见工具 / 沙箱 / CI / 运行时约定：

- `SSL_CERT_FILE`
- `REQUESTS_CA_BUNDLE`
- `CURL_CA_BUNDLE`
- `NODE_EXTRA_CA_CERTS`
- `GIT_SSL_CAINFO`
- `AWS_CA_BUNDLE`
- `PIP_CERT`
- `NPM_CONFIG_CAFILE` / `npm_config_cafile`
- `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`
- `SSL_CERT_DIR`（目录，逐个 PEM 加载）

文件型变量允许平台 path-list 分隔多个 PEM bundle；目录型变量遍历目录下 PEM 文件。无法读取或解析的文件只记录 warning/debug，不影响其它 CA 源，避免单个坏文件让 remote CLI 完全不可用。

### 4. Unsafe 最终兜底

`BIFROST_REMOTE_UNSAFE_SSL` 接受 `1` / `true` / `yes` / `on`（大小写不敏感）表示启用；`0` / `false` / `no` / `off` / 空值 / 无法识别值都视为不启用。

作用范围只限 remote relay HTTP / SSE client，即 `remote conn/exec/file/job/traffic` 等 relay-backed 命令与 target worker → relay 的连接。**不影响**本地代理转发、replay、upgrade check、sync、agent provider 等其它 HTTP client。

### 5. 接入范围

- caller 侧：`crates/bifrost-cli/src/commands/remote.rs` 中的 `CallerRelayClient` 普通 HTTP + SSE watch 全部通过 `remote_relay_reqwest_client_builder` / `remote_relay_sse_reqwest_client_builder`。
- target 侧：`crates/bifrost-admin/src/remote_invoke/relay_client.rs::new`（`:46 / :52`）的 `RelayClient` 用同一对 builder 构造 register / heartbeat / client stream / call frame / exit 等请求。
- 其它 direct HTTP client 保持原样。

## CLI + Web + Admin API

- CLI：无新增子命令；`bifrost remote conn up` 的 `--relay-url` 与 `--ca-bundle` 等 flag 语义不变，环境变量优先级仍然为 `BIFROST_REMOTE_RELAY_CA_BUNDLE` > 通用 CA 变量 > 系统 store。
- Web：无变化。
- Admin API：无新增端点；启用 unsafe 后可通过 `bifrost auth-status --format json` 中看到 `remote_relay.tls_relaxed=true` 提示（如已接入）。

## Sync 边界

TLS 信任配置纯本机；不同设备间不通过 sync 广播 CA bundle 或 unsafe 开关，避免安全策略被误覆盖。

## Phase 拆分

### Phase 1：reqwest feature + 双源 builder

- workspace 追加 `rustls-tls-native-roots`。
- `remote_relay_reqwest_client_builder` 默认 webpki + native。

### Phase 2：显式 CA bundle

- `BIFROST_REMOTE_RELAY_CA_BUNDLE` + 通用工具变量收敛。
- `openssl-probe` 兜底扫描。

### Phase 3：Unsafe 兜底

- `BIFROST_REMOTE_UNSAFE_SSL` 解析与叠加 `danger_accept_invalid_certs(true)`。
- 只作用于 remote relay client。

### Phase 4：接入 + 回归

- caller / target relay client 全部迁移到 builder。
- 新增单测 + shell E2E + human_tests。

## 测试方案

### 单元测试（`crates/bifrost-core/src/http_client.rs`）

- `load_reqwest_certificate_bundle_accepts_valid_pem_bundle`：PEM bundle 可解析。
- `remote_relay_ca_bundle_env_accepts_path_lists`：`BIFROST_REMOTE_RELAY_CA_BUNDLE` 支持 path-list。
- `remote_relay_ca_file_envs_include_common_tooling_overrides`：验证 Git / AWS / pip / npm / gRPC / SSL_CERT_FILE 等常见 CA file 变量都在扫描列表里。
- `remote_relay_unsafe_ssl_env_parses_true_false_and_invalid_values`：unsafe env 只接受明确启用值。
- `remote_relay_builder_bypasses_proxy_env`（`:962`）：验证 remote relay builder 不受 `HTTP_PROXY` / `HTTPS_PROXY` 干扰。
- `remote_relay_builder_builds_with_explicit_ca_bundle`（`:993`）：显式 CA bundle 接入后 builder 可构造。

CLI 命令层测试：`crates/bifrost-cli/tests/cli_commands.rs`。

### E2E 测试

`e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`：

1. 生成临时私有 CA 和由该 CA 签发的 `127.0.0.1` HTTPS relay 证书。
2. 未设置额外 CA 时执行 `bifrost remote conn up --relay-url https://127.0.0.1:<port>`，断言 TLS 证书校验失败。
3. 设置 `BIFROST_REMOTE_RELAY_CA_BUNDLE=<ca.pem>` 后重复执行同一连接，断言连接成功并写入 `remote-connections.json`。
4. 不设置额外 CA，但设置 `BIFROST_REMOTE_UNSAFE_SSL=1` 后重复执行同一连接，断言连接成功。

### Human Tests

`human_tests/remote-invoke-tls-trust.md`：

- 私有 CA 未配置时连接失败。
- 专用 CA bundle 配置后连接成功。
- 兼容 `SSL_CERT_FILE` 配置后连接成功。
- `BIFROST_REMOTE_UNSAFE_SSL=1` 最终兜底连接成功。
- 复合场景：unsafe 只作用于 remote，`bifrost proxy` 转发仍走正常证书校验。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 builder 是否在所有 relay 请求路径（caller HTTP、caller SSE、target register / heartbeat / stream）都被使用。
- 复核 unsafe 只作用于 remote，不泄漏到本地 proxy / upgrade / sync。
- 复测：`cargo test -p bifrost-core http_client::tests::remote_relay`、E2E `test_remote_relay_tls_trust_e2e.sh`。

### 第 2 轮

- 复核 CA 环境变量顺序与错误恢复行为。
- 复核 `openssl-probe` 在 macOS / Linux / 沙箱下的探测结果日志级别。
- 复测：跨发行版 docker 快速跑 E2E（Debian / RHEL / Alpine）。

## 风险与决策

| 风险 | 缓解 |
|---|---|
| 用户误开 `BIFROST_REMOTE_UNSAFE_SSL` 后忘记关 | 仅作用于 remote relay，不覆盖代理转发；文档强调是最终兜底，推荐先用 CA bundle |
| `native-certs` 在特定 Linux 容器上返回空 | `openssl-probe` 兜底 + 显式 CA 环境变量支持 |
| 坏 CA 文件让整体不可用 | `load_reqwest_certificate_bundle` 单文件错误仅 warn，其他源继续加载 |
| PPE 环境的私有 CA 与常规环境冲突 | `BIFROST_REMOTE_RELAY_CA_BUNDLE` 独立于其它工具变量，允许在 shell 层显式切换 |

## 校验要求

- 先执行 focused 单元测试：`cargo test -p bifrost-core http_client::tests::remote_relay`。
- 再执行 remote relay TLS E2E：`bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`。
- 再按项目规则执行 E2E 全套、coverage、rust-project-validate、本地 CI、commit + push + PR + 远端 CI 看护。
