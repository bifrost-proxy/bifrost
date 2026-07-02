# Upgrade TLS Trust

## 背景

`bifrost version-check`、托盘/Admin 更新检查、`bifrost upgrade`、`bifrost install-skill`、Sync/登录/云端接口、AI/Agent provider、ASR/语音模型下载、脚本 `net.fetch`、规则远程 value 等路径都会发起外部 HTTPS 请求。此前部分路径使用 direct HTTP client 默认 TLS 配置或裸 `ureq`，可能在企业出口、Linux 沙箱或 CI MITM 环境中遇到 `UnknownIssuer` / `invalid peer certificate`。同一环境下 curl/openssl 能通，是因为它们读取系统 CA 或 `SSL_CERT_FILE`，而 Bifrost 外部出口链路没有统一读取这些私有根证书来源。

## 目标

- 外部 HTTPS 请求默认同时信任 webpki 公网根和系统 native root store。
- 支持 GitHub/upgrade 专用 CA bundle、remote relay 专用 CA bundle、全局 Bifrost CA bundle 和常见工具链 CA 环境变量。
- 支持 GitHub/upgrade 专用 unsafe SSL 环境变量作为最终兜底，且不复用 remote relay 的 `BIFROST_REMOTE_UNSAFE_SSL`。
- 支持 `BIFROST_UNSAFE_SSL=1` 作为通用外部 HTTPS 请求最终兜底。
- 保持更新链路默认不读取系统代理环境变量，避免被 shell 代理意外劫持；用户仍可通过既有 mirror 环境变量选择下载源。

## 实现方案

### 1. 共享 TLS trust profile

`bifrost-core::http_client` 增加 GitHub trust profile：

- `github_reqwest_client_builder()`
- `github_blocking_reqwest_client_builder()`
- `outbound_reqwest_client_builder()`
- `outbound_blocking_reqwest_client_builder()`

builder 从 `direct_reqwest_client_builder().no_proxy()` 出发，显式启用：

- `tls_built_in_webpki_certs(true)`
- `tls_built_in_native_certs(true)`

并追加文件/目录型 CA bundle 与 `openssl-probe` 探测出的系统 OpenSSL CA 路径。

### 2. CA 来源

GitHub/upgrade 链路按以下来源追加私有根证书：

- `BIFROST_GITHUB_CA_BUNDLE`
- `BIFROST_UPGRADE_CA_BUNDLE`
- `BIFROST_CA_BUNDLE`
- `SSL_CERT_FILE`
- `REQUESTS_CA_BUNDLE`
- `CURL_CA_BUNDLE`
- `NODE_EXTRA_CA_CERTS`
- `GIT_SSL_CAINFO`
- `AWS_CA_BUNDLE`
- `PIP_CERT`
- `NPM_CONFIG_CAFILE`
- `npm_config_cafile`
- `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`

目录型来源：

- `BIFROST_GITHUB_CA_DIR`
- `BIFROST_UPGRADE_CA_DIR`
- `BIFROST_CA_DIR`
- `SSL_CERT_DIR`

文件型变量支持平台 path-list 分隔符传多个 PEM bundle；目录型变量读取目录内文件并逐个尝试解析。无法读取或不是 PEM 的文件只记录 warning/debug，不阻断其它 CA 来源。

通用外部 HTTPS 链路按以下来源追加私有根证书：

- `BIFROST_CA_BUNDLE`
- `SSL_CERT_FILE`
- `REQUESTS_CA_BUNDLE`
- `CURL_CA_BUNDLE`
- `NODE_EXTRA_CA_CERTS`
- `GIT_SSL_CAINFO`
- `AWS_CA_BUNDLE`
- `PIP_CERT`
- `NPM_CONFIG_CAFILE`
- `npm_config_cafile`
- `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`

目录型来源：

- `BIFROST_CA_DIR`
- `SSL_CERT_DIR`

### 3. Unsafe 最终兜底

- `BIFROST_GITHUB_UNSAFE_SSL=1`
- `BIFROST_UPGRADE_UNSAFE_SSL=1`
- `BIFROST_UNSAFE_SSL=1`

这些变量支持 `1/true/yes/on` 开启，`0/false/no/off` 或空值关闭。GitHub/upgrade 专用变量作用于 GitHub release/version-check/upgrade/install-skill 相关 HTTP client；`BIFROST_UNSAFE_SSL` 作用于通用外部 HTTPS client，并作为 GitHub/remote relay profile 的全局兜底。它们不等同于代理服务 `--unsafe-ssl`。

推荐顺序是：系统 trust store → scoped CA bundle → `BIFROST_CA_BUNDLE`/`BIFROST_CA_DIR` → 常见 CA env → unsafe 临时兜底。

### 4. 接入范围

- `version_check::fetch_latest_release_sync`、redirect、release body、HTML fallback。
- `version_check::fetch_latest_release_async` 及 Admin/tray 更新检查 fallback。
- `upgrade.rs` release mirror probe 和 archive download。
- `install_skill.rs` 拉取 GitHub raw 最新 skill。
- `bifrost-sync` Sync/登录/云端接口。
- `agent` provider 请求、Responses API、MCP OAuth discovery/register/token/refresh。
- `bifrost-admin` ASR、语音唤醒、diarization 模型下载，以及 IM gateway/ChatGPT Web/Feishu/Weixin 外部请求。
- `bifrost-script` remote parser download 与 sandbox `net.fetch`。
- `bifrost-core` 规则 value 的远程 URL 来源；同步规则解析路径通过独立线程执行 reqwest blocking 拉取，确保在代理 async runtime 内遇到不可达 URL 字面量时仍按既有语义 fallback，不触发 nested runtime drop panic。
- `install-binary.sh` probe、latest redirect、GitHub API 查询和下载工具参数桥接。

## 测试方案

### 单元测试

- GitHub trust profile 接受 `BIFROST_GITHUB_CA_BUNDLE`、`BIFROST_UPGRADE_CA_BUNDLE`、`BIFROST_CA_BUNDLE` 和常见 CA file env。
- 通用 outbound trust profile 接受 `BIFROST_CA_BUNDLE` 和常见 CA file env。
- GitHub async/blocking reqwest builder 能加载显式 CA bundle。
- 通用 outbound async/blocking reqwest builder 能加载显式 CA bundle。
- `BIFROST_GITHUB_UNSAFE_SSL` 与 `BIFROST_UPGRADE_UNSAFE_SSL` 均可启用 unsafe SSL。
- `BIFROST_UNSAFE_SSL` 可启用通用 outbound unsafe SSL，并作为 GitHub profile 兜底。
- 规则远程 URL value 在 Tokio runtime 内不可达时不会 panic，并 fallback 为原始 URL 字面量。
- 既有 remote relay 测试继续验证 remote 专用 CA/unsafe 行为不变。

### E2E 测试

新增 `e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`：

1. 生成临时私有 CA 和由该 CA 签发的 `127.0.0.1` HTTPS mirror 证书。
2. 构造本地 GitHub release archive mirror。
3. 复制一份临时 Bifrost binary 执行 `upgrade`，不配置 CA 时断言下载因证书信任失败。
4. 设置 `BIFROST_GITHUB_CA_BUNDLE=<ca.pem>` 后执行 `upgrade`，断言临时 binary 成功升级。
5. 设置 `BIFROST_UPGRADE_UNSAFE_SSL=1` 后执行 `upgrade`，断言临时 binary 成功升级。

### Human Tests

新增 `human_tests/upgrade-tls-trust.md`，按真实 CLI 场景执行：

- 私有 CA mirror 未配置时 upgrade 失败。
- `BIFROST_GITHUB_CA_BUNDLE` 配置后 upgrade 成功。
- `BIFROST_UPGRADE_UNSAFE_SSL=1` 作为最终兜底成功。
- 文档/skill 中包含 version-check/upgrade TLS trust 排障说明。

## 校验要求

- `cargo test -p bifrost-core http_client::tests::github`
- `cargo test -p bifrost-core http_client::tests::outbound`
- `cargo test -p bifrost-core version_check::tests::`
- `cargo test -p bifrost-cli upgrade_ --lib`
- `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`
- 按项目规则执行 human_tests、两轮 Review/Fix/Test、coverage/rust-project-validate、本地与远端 CI 看护。
