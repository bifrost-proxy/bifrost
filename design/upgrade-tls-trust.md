# Upgrade TLS Trust

## 背景

Bifrost 有大量外部 HTTPS 出口链路: `bifrost version-check`、托盘/Admin 更新检查、`bifrost upgrade`、`bifrost install-skill`、Sync/登录/云端接口、AI/Agent provider、ASR 与语音模型下载、脚本 `net.fetch`、规则远程 value 等。这些链路早期各自使用 `reqwest` 默认 TLS 配置或裸 `ureq`,只信任 webpki 公网根证书,在企业出口、Linux 沙箱、CI MITM 环境下经常遇到 `UnknownIssuer` / `invalid peer certificate`。

同一环境下 `curl` / `openssl` 能通,是因为它们读取系统 CA 存储或 `SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` 等广泛使用的 CA 环境变量。Bifrost 缺乏统一的 CA trust profile,每条链路都各自演化了不同的 TLS 客户端,升级/skill/version-check 三条最高频路径直接受害,用户无法完成升级或安装 skill。

本方案在 `bifrost-core::http_client` 里下沉两套 trust profile —— **GitHub / upgrade 专用** 与 **通用外部** —— 支持文件/目录型 CA bundle、常见工具链 CA 环境变量、scoped unsafe-SSL 兜底,并将所有外部出口链路接入这两套 profile。原有 remote relay(bifrost 之间的转发)保持独立 profile 与独立 unsafe-SSL 环境变量,不与外部出口混淆。

代码入口:

- `crates/bifrost-core/src/http_client.rs` — trust profile 与 CA 环境变量常量。
- `crates/bifrost-core/src/version_check.rs` — sync / async / redirect / HTML fallback 全部接入 GitHub profile。
- `crates/bifrost-cli/src/commands/upgrade.rs` — release mirror probe + archive download。
- `crates/bifrost-cli/src/commands/install_skill.rs` — GitHub raw skill 拉取。
- `crates/bifrost-sync/src/client.rs`、`crates/agent/src/client.rs`、`crates/agent/src/mcp/oauth.rs` — 云端 / provider / OAuth。
- `crates/bifrost-admin/src/handlers/asr.rs`、`.../handlers/voice/wake.rs`、`.../im_gateway/*` — ASR / wake / IM 外部请求。
- `install-binary.sh` — 一键安装脚本,兼容 macOS bash 3.2 + `set -u`。

## 用户目标验证清单

### 必须实现

- 外部 HTTPS 请求默认同时信任 webpki 公网根 + 系统 native root store。
- 支持 GitHub/upgrade 专用 CA bundle:`BIFROST_GITHUB_CA_BUNDLE`、`BIFROST_UPGRADE_CA_BUNDLE`,以及全局 `BIFROST_CA_BUNDLE`。
- 支持常见工具链 CA 环境变量:`SSL_CERT_FILE`、`REQUESTS_CA_BUNDLE`、`CURL_CA_BUNDLE`、`NODE_EXTRA_CA_CERTS`、`GIT_SSL_CAINFO`、`AWS_CA_BUNDLE`、`PIP_CERT`、`NPM_CONFIG_CAFILE`、`npm_config_cafile`、`GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`。
- 支持目录型 CA 来源:`BIFROST_GITHUB_CA_DIR`、`BIFROST_UPGRADE_CA_DIR`、`BIFROST_CA_DIR`、`SSL_CERT_DIR`。
- 支持 scoped unsafe-SSL 兜底:`BIFROST_GITHUB_UNSAFE_SSL=1`、`BIFROST_UPGRADE_UNSAFE_SSL=1`、通用 `BIFROST_UNSAFE_SSL=1`(全部支持 `1/true/yes/on` 开启)。
- 更新链路默认不读取系统代理环境变量(`no_proxy()`),避免 shell 代理意外劫持;用户仍可通过既有 mirror 变量选择下载源。

### 必须不破坏

- 已存在的 remote relay CA/unsafe 行为独立:`BIFROST_REMOTE_RELAY_CA_BUNDLE`、`BIFROST_REMOTE_UNSAFE_SSL` 语义保留,不被外部出口 unsafe 变量污染;`BIFROST_UNSAFE_SSL` 只作为 GitHub / remote relay profile 的最终兜底,不改变默认 relay 行为。
- 现有 CLI 命令 / Admin API / Web UI 参数不变,只是 TLS 客户端替换。
- reqwest 客户端仍是 async(`Client`)与 blocking(`blocking::Client`)双入口,builder 直接暴露不新增 async runtime 依赖。
- 规则远程 URL value 的同步解析仍在独立线程执行 blocking reqwest,不触发 async runtime nested drop panic。
- `install-binary.sh` 在无自定义 CA 环境变量时,optional CA 参数数组必须兼容 macOS bash 3.2 / `set -u`,不因 `${CA_ARGS[@]}` 未定义而报错。

### 必须真实验证

- E2E:私有 CA 签发的 `127.0.0.1` HTTPS mirror,未配置 CA 时 upgrade 失败;`BIFROST_GITHUB_CA_BUNDLE=<ca.pem>` 后 upgrade 成功;`BIFROST_UPGRADE_UNSAFE_SSL=1` 后 upgrade 成功。
- 单元:GitHub / 通用 profile 均能从 `BIFROST_CA_BUNDLE` + 常见 CA env 加载;`BIFROST_UNSAFE_SSL` 作为 GitHub profile 兜底。
- Human:企业环境私有 CA 部署后,`bifrost version-check` / `bifrost upgrade` / `bifrost install-skill` 全部通;Sync 登录、AI provider 请求也通。

## 产品语义

### 两套 profile 分层

- **GitHub / upgrade profile**(`github_reqwest_client_builder` / `github_blocking_reqwest_client_builder`):仅用于 GitHub Release / raw content / version-check / upgrade / install-skill 相关的高安全链路。
- **通用外部 profile**(`outbound_reqwest_client_builder` / `outbound_blocking_reqwest_client_builder`):用于 AI provider、Sync、ASR/wake 模型下载、脚本 `net.fetch`、规则远程 value 等其它外部 HTTPS。
- **remote relay profile**:独立存在,`BIFROST_REMOTE_RELAY_CA_BUNDLE` + `BIFROST_REMOTE_UNSAFE_SSL`,不与外部出口共享。

两套 profile 都基于 `direct_reqwest_client_builder().no_proxy()`,显式启用:

```rust
.tls_built_in_webpki_certs(true)
.tls_built_in_native_certs(true)
```

并按顺序追加 file/directory 型 CA bundle,以及 `openssl-probe` 探测出的系统 OpenSSL CA 路径。

### CA 加载顺序

推荐排查/接入顺序(从优先到兜底):

1. 系统 trust store(webpki + native)
2. scoped CA bundle:`BIFROST_GITHUB_CA_BUNDLE` / `BIFROST_UPGRADE_CA_BUNDLE`(GitHub profile);无 scoped 时直接落到通用 profile
3. 全局 `BIFROST_CA_BUNDLE` / `BIFROST_CA_DIR`
4. 常见工具链 CA env(见列表)
5. scoped unsafe-SSL 临时兜底(`BIFROST_GITHUB_UNSAFE_SSL=1` / `BIFROST_UPGRADE_UNSAFE_SSL=1`)
6. 全局 `BIFROST_UNSAFE_SSL=1`

`_UNSAFE_SSL` 与代理层面的 `--unsafe-ssl` 不等同 —— 前者是外部出口客户端的 TLS 校验,后者是 Bifrost 作为代理服务器接收上游证书时的策略。

### 文件与目录变量

- 文件型:支持平台 path-list 分隔符(`:` / `;`)传多个 PEM bundle,逐个加载。
- 目录型:读取目录内文件并逐个尝试解析。
- 无法读取或非 PEM 的文件只记录 warning/debug,不阻断其它 CA 来源,避免一个损坏文件挡住其它可用 CA。

## 技术细节

### 常量

`crates/bifrost-core/src/http_client.rs`:

```rust
pub const BIFROST_CA_BUNDLE_ENV: &str = "BIFROST_CA_BUNDLE";
pub const BIFROST_CA_DIR_ENV: &str = "BIFROST_CA_DIR";
pub const BIFROST_UNSAFE_SSL_ENV: &str = "BIFROST_UNSAFE_SSL";
pub const GITHUB_CA_BUNDLE_ENV: &str = "BIFROST_GITHUB_CA_BUNDLE";
pub const GITHUB_CA_DIR_ENV: &str = "BIFROST_GITHUB_CA_DIR";
pub const GITHUB_UNSAFE_SSL_ENV: &str = "BIFROST_GITHUB_UNSAFE_SSL";
pub const UPGRADE_CA_BUNDLE_ENV: &str = "BIFROST_UPGRADE_CA_BUNDLE";
pub const UPGRADE_CA_DIR_ENV: &str = "BIFROST_UPGRADE_CA_DIR";
pub const REMOTE_RELAY_CA_BUNDLE_ENV: &str = "BIFROST_REMOTE_RELAY_CA_BUNDLE";
pub const REMOTE_UNSAFE_SSL_ENV: &str = "BIFROST_REMOTE_UNSAFE_SSL";
```

常量数组:

- `GITHUB_CA_FILE_ENVS = [GITHUB_CA_BUNDLE_ENV, UPGRADE_CA_BUNDLE_ENV, BIFROST_CA_BUNDLE_ENV, "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", …]`
- `OUTBOUND_CA_FILE_ENVS = [BIFROST_CA_BUNDLE_ENV, "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", …]`
- `COMMON_CA_FILE_ENVS`、`COMMON_CA_DIR_ENVS`(工具链通用)
- `REMOTE_RELAY_UNSAFE_SSL_ENVS = [REMOTE_UNSAFE_SSL_ENV, BIFROST_UNSAFE_SSL_ENV]`
- `OUTBOUND_UNSAFE_SSL_ENVS = [BIFROST_UNSAFE_SSL_ENV]`

### Builder 入口

```rust
pub fn github_reqwest_client_builder() -> reqwest::ClientBuilder;
pub fn github_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder;
pub fn outbound_reqwest_client_builder() -> reqwest::ClientBuilder;
pub fn outbound_blocking_reqwest_client_builder() -> reqwest::blocking::ClientBuilder;
```

内部实现:

1. 从 `direct_reqwest_client_builder().no_proxy()` 出发。
2. 启用 webpki + native roots。
3. 遍历 scoped + global + common CA file env,`load_reqwest_certificate` 加载 PEM。
4. 遍历 CA dir env,枚举文件后逐个加载。
5. 若 scoped 或全局 unsafe-SSL env 命中,调用 `.danger_accept_invalid_certs(true)`。

### CLI / Admin / Web

本方案不新增 CLI 命令或 Admin API 端点。使用面:

- CLI 用户:通过 env 变量传 CA / unsafe。`bifrost --help` 中无新增子命令。
- Admin API:handlers 内 `http_client` 引用切换到 GitHub 或 outbound builder,协议表面无变化。
- Web UI:无变化(所有 CA 配置在环境层)。

`install-binary.sh` 更新:

- 无自定义 CA env 时,`CA_ARGS=()` 初始化,使用 `"${CA_ARGS[@]+"${CA_ARGS[@]}"}"` 展开兼容 bash 3.2 + `set -u`。
- 有 `BIFROST_GITHUB_CA_BUNDLE` 时,追加 `--cacert <path>`(`curl`)或 `--ca-certificate <path>`(`wget`)。
- 有 `BIFROST_GITHUB_UNSAFE_SSL=1` 时,追加 `--insecure`(`curl`) / `--no-check-certificate`(`wget`)。

### 接入范围(全量清单)

- `version_check::fetch_latest_release_sync`、redirect、release body、HTML fallback。
- `version_check::fetch_latest_release_async` + Admin/tray 更新检查 fallback。
- `upgrade.rs` release mirror probe + archive download。
- `install_skill.rs` 拉 GitHub raw skill。
- `bifrost-sync/client.rs` Sync/登录/云端接口。
- `agent/client.rs` provider 请求、Responses API;`agent/mcp/oauth.rs` OAuth discovery/register/token/refresh。
- `bifrost-admin` ASR (`handlers/asr.rs`)、`handlers/voice/wake.rs`、`im_gateway/{feishu,weixin,chatgpt_web/*,agent_reply,providers}`、`handlers/replay.rs`、`replay.rs`。
- `bifrost-cli/asr.rs` diarization / 语音模型下载。
- `bifrost-script` remote parser download 与 sandbox `net.fetch`。
- `bifrost-core` 规则 value 的远程 URL 来源;同步规则解析路径通过独立线程执行 reqwest blocking 拉取,避免 nested runtime drop panic;不可达 URL 字面量按既有语义 fallback。

### Sync 边界

- CA 配置来自本机环境变量与 `unified_config`,不进入 rule/group sync 通道。
- 组织/团队不能通过 sync 强制其它成员放松 TLS 校验。
- `BIFROST_UNSAFE_SSL` 等 unsafe 变量仅在本机生效,禁止通过任何 admin API 远端设置。

## Phase 1 - 4

### Phase 1:core trust profile

- `http_client.rs` 增加 GitHub / outbound profile builder + 常量数组 + CA 加载函数。
- 单元测试覆盖 CA file / dir env、unsafe SSL env、remote relay 隔离。

### Phase 2:接入 version-check + upgrade + install-skill

- `version_check.rs` 全部 fetch 改用 `github_*_builder`。
- `upgrade.rs` mirror probe + archive download 改用 GitHub builder。
- `install_skill.rs` GitHub raw 拉取改用 GitHub builder。

### Phase 3:接入 Sync / Agent / MCP / ASR / IM Gateway

- Sync `client.rs`、Agent `client.rs`、MCP `oauth.rs`、ASR handlers、IM providers 改用 outbound builder。
- 规则远程 URL value 独立线程 blocking 拉取。

### Phase 4:install-binary.sh + human_tests + 文档

- `install-binary.sh` 支持 CA env + unsafe env,bash 3.2 兼容。
- `human_tests/upgrade-tls-trust.md` 新增;README/skill 说明企业 CA 排障步骤。

## 测试方案

### 单元测试

`crates/bifrost-core/src/http_client.rs` 内(测试模块):

- GitHub trust profile 接受 `BIFROST_GITHUB_CA_BUNDLE` / `BIFROST_UPGRADE_CA_BUNDLE` / `BIFROST_CA_BUNDLE` 与常见 CA file env。
- outbound trust profile 接受 `BIFROST_CA_BUNDLE` 与常见 CA file env。
- GitHub async/blocking builder 能加载显式 CA bundle(`assert!(github_reqwest_client_builder().build().is_ok())`)。
- outbound async/blocking builder 能加载显式 CA bundle。
- `BIFROST_GITHUB_UNSAFE_SSL` 与 `BIFROST_UPGRADE_UNSAFE_SSL` 均可启用 unsafe SSL。
- `BIFROST_UNSAFE_SSL` 可启用通用 outbound unsafe SSL,并作为 GitHub profile 兜底。
- Remote relay 测试继续验证 remote 专用 CA/unsafe 行为不变。

`crates/bifrost-core/src/version_check.rs::tests`:

- 私有 CA mirror 场景 sync / async 都能通。
- HTML fallback 不因 CA 配置差异丢失路径。

规则远程 URL:

- Tokio runtime 内不可达时不 panic,fallback 为原始 URL 字面量。

### E2E 测试

`e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`(已在仓库中):

1. 生成临时私有 CA + 签发的 `127.0.0.1` HTTPS mirror 证书。
2. 构造本地 GitHub release archive mirror。
3. 复制临时 Bifrost binary 执行 `upgrade`,不配置 CA → 断言失败(证书信任失败)。
4. 设置 `BIFROST_GITHUB_CA_BUNDLE=<ca.pem>` → 执行 `upgrade` → 断言成功(`BIFROST_GITHUB_CA_BUNDLE lets upgrade download from private-CA mirror`)。
5. 设置 `BIFROST_UPGRADE_UNSAFE_SSL=1` → 执行 `upgrade` → 断言成功(`BIFROST_UPGRADE_UNSAFE_SSL lets upgrade download from private-CA mirror as fallback`)。

`e2e-tests/tests/test_install_binary_adaptive_download.sh`:验证 `install-binary.sh` 在有/无 CA env 时的 curl / wget 参数展开。

### 真实场景测试

`human_tests/upgrade-tls-trust.md`(已在仓库中):

- TC-TLS-01:私有 CA mirror 未配置时 `bifrost upgrade` 失败,错误包含 `UnknownIssuer` 或 `invalid peer certificate`。
- TC-TLS-02:`BIFROST_GITHUB_CA_BUNDLE` 配置后 `bifrost upgrade` 成功,version-check 也通。
- TC-TLS-03:`BIFROST_UPGRADE_UNSAFE_SSL=1` 作为最终兜底成功。
- TC-TLS-04:`bifrost install-skill --tool all` 成功。
- TC-TLS-05:`bifrost-sync login` 在企业代理 + 私有 CA 下成功。
- TC-TLS-06:AI provider 请求(OpenAI / Anthropic 私有网关)成功。
- TC-TLS-07:ASR / wake 模型下载成功。
- TC-TLS-08:`install-binary.sh` 在 macOS bash 3.2 环境无 CA env 时不报 unbound variable。
- TC-TLS-09:文档 / skill 包含 version-check / upgrade TLS trust 排障说明。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标:两套 profile 分层、CA env 全覆盖、unsafe SSL 三档兜底、remote relay 独立、install-binary.sh 兼容 bash 3.2。
- Review 修改:`http_client.rs`、`version_check.rs`、`upgrade.rs`、`install_skill.rs`、Sync/Agent/MCP/ASR/IM handlers、`install-binary.sh`。
- 复跑 `cargo fmt --check`、`cargo clippy --workspace --all-features -D warnings`、`cargo test -p bifrost-core http_client::tests`、`cargo test -p bifrost-core version_check::tests`、`cargo test -p bifrost-cli upgrade_ --lib`、`bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`。

### 第 2 轮

- 复检:remote relay 未误接入 GitHub/outbound profile;`BIFROST_UNSAFE_SSL` 兜底顺序符合文档;规则远程 URL 独立线程无 nested runtime panic;install-binary.sh 在无 CA env 场景无 unbound variable。
- 执行 `human_tests/upgrade-tls-trust.md` 全部用例。
- `cargo test --workspace --all-features`;补齐 coverage 门禁;远端 CI 看护。

## 校验要求

1. `cargo test -p bifrost-core http_client::tests::github`
2. `cargo test -p bifrost-core http_client::tests::outbound`
3. `cargo test -p bifrost-core version_check::tests::`
4. `cargo test -p bifrost-cli upgrade_ --lib`
5. `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`
6. `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`
7. 按项目规则执行 human_tests、两轮 Review/Fix/Test、`make coverage` / `rust-project-validate`、本地 + 远端 CI 看护。

## 风险与决策

- **`BIFROST_UNSAFE_SSL` 全局兜底与安全边界**:提供最强兜底但会关闭所有外部出口的 TLS 校验,应该只在排障或严格受控环境使用。文档明确说明不建议长期开启;scoped `BIFROST_GITHUB_UNSAFE_SSL` / `BIFROST_UPGRADE_UNSAFE_SSL` 是更好的日常方案。
- **remote relay 隔离**:remote relay 是 bifrost 之间的转发,信任模型与外部 GitHub 出口不同;必须独立 profile + 独立 unsafe 变量,避免用户为 GitHub 打开 unsafe 结果 relay 也变得不校验。
- **CA env 数量**:同时支持 11 个文件型 + 4 个目录型环境变量看似冗余,但每一个都是不同工具链(pip / npm / gRPC / AWS SDK / git)约定俗成的路径,企业 CA 部署脚本往往只设置其中之一;全部支持避免了“Bifrost 特立独行”的痛苦。
- **规则远程 URL 独立线程**:在 async runtime 内直接调用 blocking reqwest 会触发 nested runtime drop panic;必须走独立线程 + 传值/传引用回来,不能省这层。
- **install-binary.sh bash 3.2 兼容**:macOS 系统 bash 仍是 3.2.57,`"${arr[@]}"` 在空数组 + `set -u` 下报错;必须使用 `"${arr[@]+"${arr[@]}"}"` 或先判 `[ ${#arr[@]} -gt 0 ]` 的 pattern。
- **`no_proxy()` 默认**:更新链路默认不读取 shell 代理,避免被 `HTTP_PROXY` / `HTTPS_PROXY` 意外劫持;用户需要显式使用 mirror 变量或自建代理 env。
- **无变化的 API 表面**:选择在 core 侧下沉 trust profile 而非在每个 handler 新增 CA 参数,保持用户接口稳定,升级零迁移成本。
