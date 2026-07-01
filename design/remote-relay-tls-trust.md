# Remote Relay TLS Trust

## 背景

`bifrost remote conn up`、`remote exec`、`remote file` 等 caller 侧命令，以及 target 侧 Remote Invoke worker，都需要主动连接 relay。此前这条链路复用 `reqwest` + `rustls-tls` 默认配置，只信任 webpki 公网根证书，不读取系统 CA，也不读取常见沙箱/CLI CA 环境变量。

在企业出口、Linux 沙箱或受控 CI 环境里，HTTPS relay 可能被内部安全网关做 TLS inspection，服务端证书会被替换为私有根证书签发。curl/openssl 因为读取系统 CA 或 `SSL_CERT_FILE` 可以握手成功，但 Bifrost remote relay client 会在 TLS 握手阶段失败。

## 目标

- Remote relay HTTP/SSE client 默认同时信任 webpki 公网根和系统 native root store。
- 支持显式追加私有 CA bundle，覆盖沙箱 MITM、企业代理、私有 relay 证书等场景。
- 支持 `BIFROST_REMOTE_UNSAFE_SSL` 作为 remote relay 专用最终兜底开关；默认不启用，不复用代理服务的 `--unsafe-ssl`，避免误把非 remote 链路也降级。
- 保持 relay 请求不读取系统代理环境变量，避免 remote control 流量被意外代理；PPE header 注入继续使用 `BIFROST_REMOTE_RELAY_HEADERS`。

## 实现方案

### 1. reqwest feature

在 workspace `reqwest` feature 中追加 `rustls-tls-native-roots`，保持既有 `rustls-tls` / webpki roots，同时允许 reqwest 加载平台根证书。

### 2. Remote relay 专用 builder

在 `bifrost-core` 增加：

- `remote_relay_reqwest_client_builder()`
- `remote_relay_sse_reqwest_client_builder()`

这两个 builder 仍从 `direct_reqwest_client_builder().no_proxy()` 出发，但显式启用：

- `tls_built_in_webpki_certs(true)`
- `tls_built_in_native_certs(true)`

并追加私有 CA bundle。

如果 `BIFROST_REMOTE_UNSAFE_SSL` 设置为 `1`、`true`、`yes` 或 `on`，remote relay builder 额外启用 `danger_accept_invalid_certs(true)`，作为 CA 配置不可控环境下的最后兜底。`0`、`false`、`no`、`off`、空值或无法识别的值都不启用。

### 3. 私有 CA 来源

优先支持 Bifrost 专用环境变量：

- `BIFROST_REMOTE_RELAY_CA_BUNDLE`

同时兼容常见工具、沙箱、CI 和语言运行时约定：

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
- `SSL_CERT_DIR`

文件型变量允许使用平台 path-list 分隔符传多个 PEM bundle；目录型变量读取目录下 PEM 文件并逐个尝试加载。无法读取或解析的文件只记录 warning/debug，不改变其它 CA 来源，避免单个无关文件让 remote CLI 完全不可用。

另外使用 `openssl-probe` 扫描 Linux/Unix 常见系统 CA bundle 和 hashed certificate directory，例如 Debian/Ubuntu 的 `/etc/ssl/certs/ca-certificates.crt`、RHEL/Fedora 的 `/etc/pki/...`、Alpine/OpenSUSE/OpenHarmony 等发行版路径。这个扫描是 native/system roots 的补充兜底：正常安装到系统 trust store 的私有根证书优先由 `rustls-native-certs` 加载；如果容器或沙箱只有传统 OpenSSL bundle 路径，也会被显式追加到 remote relay client。

### 4. Unsafe 最终兜底

- `BIFROST_REMOTE_UNSAFE_SSL=1`：跳过 remote relay HTTPS 证书信任校验。
- 作用范围只限 remote relay HTTP/SSE client，即 `remote conn/exec/file/job/traffic` 等 relay-backed 命令和 target worker 到 relay 的连接。
- 不影响本地代理转发、replay、upgrade、sync、agent provider 等其它 HTTP client。
- 推荐顺序是先使用系统 CA 或 `BIFROST_REMOTE_RELAY_CA_BUNDLE`，只有确认 CA 注入不可控或临时诊断时才使用 unsafe。

### 4. 接入范围

- caller 侧 `CallerRelayClient` 的普通 HTTP 请求和 SSE watch。
- target 侧 `RelayClient` 的 register、heartbeat、client stream、call frame/exit 等 relay 请求。

其它 direct HTTP client 保持原样，避免把版本检查、upgrade、agent provider 等无关链路的代理/CA 行为一起改变。

## 测试方案

### 单元测试

- `load_reqwest_certificate_bundle_accepts_valid_pem_bundle`：验证 PEM bundle 可解析。
- `remote_relay_ca_bundle_env_accepts_path_lists`：验证 `BIFROST_REMOTE_RELAY_CA_BUNDLE` 支持 path-list。
- `remote_relay_ca_file_envs_include_common_tooling_overrides`：验证兼容 Git/AWS/pip/npm/gRPC 等常见 CA file 变量。
- `remote_relay_unsafe_ssl_env_parses_true_false_and_invalid_values`：验证 unsafe env 只接受明确启用值。
- `remote_relay_builder_bypasses_proxy_env`：验证 remote relay builder 仍不受 `HTTP_PROXY` / `HTTPS_PROXY` 干扰。
- `remote_relay_builder_builds_with_explicit_ca_bundle`：验证显式 CA bundle 接入后 builder 可构造。

### E2E 测试

新增 `e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`：

1. 生成临时私有 CA 和由该 CA 签发的 `127.0.0.1` HTTPS relay 证书。
2. 未设置额外 CA 时执行 `bifrost remote conn up --relay-url https://127.0.0.1:<port>`，断言 TLS 证书校验失败。
3. 设置 `BIFROST_REMOTE_RELAY_CA_BUNDLE=<ca.pem>` 后重复执行同一连接，断言连接成功并写入 `remote-connections.json`。
4. 不设置额外 CA，但设置 `BIFROST_REMOTE_UNSAFE_SSL=1` 后重复执行同一连接，断言连接成功并写入 `remote-connections.json`。

### Human Tests

新增 `human_tests/remote-invoke-tls-trust.md`，按真实 CLI 场景逐条执行：

- 私有 CA 未配置时连接失败。
- 专用 CA bundle 配置后连接成功。
- 兼容 `SSL_CERT_FILE` 配置后连接成功。
- `BIFROST_REMOTE_UNSAFE_SSL=1` 最终兜底连接成功。

## 校验要求

- 先执行 focused 单元测试：`cargo test -p bifrost-core http_client::tests::remote_relay`
- 再执行 remote relay TLS E2E：`bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`
- 再按项目规则执行 E2E、coverage、rust-project-validate、本地 CI、提交/PR/远端 CI 看护。
