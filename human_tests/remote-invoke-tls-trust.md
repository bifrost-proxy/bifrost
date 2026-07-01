# Remote Invoke Relay TLS Trust 真实场景测试

## 功能模块说明

验证 `bifrost remote` 连接 HTTPS relay 时能够信任系统 CA 和显式私有 CA bundle，覆盖企业安全网关、Linux 沙箱 Safebox MITM、私有 relay 证书等场景；并验证 `BIFROST_REMOTE_UNSAFE_SSL` 可作为最终兜底跳过 remote relay 证书信任校验。

## 前置条件

- 当前仓库可编译 `bifrost` release 二进制。
- 本机可执行 `python3`、`openssl`、`cargo`。
- 测试必须使用临时 `BIFROST_DATA_DIR`。
- 测试不启动完整 Bifrost 服务，不修改系统代理，不安装系统 CA。

## 测试用例列表

### TC-RITT-01：未配置私有 CA 时 HTTPS relay 连接失败

操作步骤：

1. 执行 `bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`。
2. 脚本生成临时私有 CA 和 HTTPS relay 证书。
3. 脚本在未设置 `BIFROST_REMOTE_RELAY_CA_BUNDLE`、`BIFROST_REMOTE_UNSAFE_SSL`、`SSL_CERT_FILE`、`REQUESTS_CA_BUNDLE`、`CURL_CA_BUNDLE`、`NODE_EXTRA_CA_CERTS`、`GIT_SSL_CAINFO`、`AWS_CA_BUNDLE`、`PIP_CERT`、`NPM_CONFIG_CAFILE`、`GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`、`SSL_CERT_DIR` 的环境下执行 `bifrost remote conn up 882001 --relay-url https://127.0.0.1:<port>`。

预期结果：

- CLI 非 0 退出。
- 输出包含 `start pairing failed` 和 `error sending request`，说明 HTTPS relay 请求在配对开始前失败。
- 不生成成功连接记录。

### TC-RITT-02：`BIFROST_REMOTE_RELAY_CA_BUNDLE` 配置私有 CA 后连接成功

操作步骤：

1. 继续执行 `bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`。
2. 脚本设置 `BIFROST_REMOTE_RELAY_CA_BUNDLE=<临时 ca.pem>`。
3. 脚本执行 `bifrost remote conn up 882002 --relay-url https://127.0.0.1:<port>`。

预期结果：

- CLI 以 0 退出。
- 输出包含 `Connected! Authorization granted`。
- 临时 `remote-connections.json` 包含 HTTPS relay URL 和 `client-tls-123456`。

### TC-RITT-03：`BIFROST_REMOTE_UNSAFE_SSL` 作为最终兜底后连接成功

操作步骤：

1. 继续执行 `bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`。
2. 脚本不设置任何额外 CA bundle，设置 `BIFROST_REMOTE_UNSAFE_SSL=1`。
3. 脚本执行 `bifrost remote conn up 882003 --relay-url https://127.0.0.1:<port>`。

预期结果：

- CLI 以 0 退出。
- 输出包含 `Connected! Authorization granted`。
- 临时 `remote-connections.json` 包含 HTTPS relay URL 和 `client-tls-123456`。

### TC-RITT-04：系统 CA、常见 CA 环境变量和 unsafe env 解析兼容性不破坏

操作步骤：

1. 执行 `cargo test -p bifrost-core http_client::tests::remote_relay -- --nocapture`。
2. 检查测试输出中 remote relay builder 相关测试通过。

预期结果：

- `remote_relay_builder_bypasses_proxy_env` 通过，说明 remote relay client 保持不读取代理环境变量。
- `remote_relay_ca_bundle_env_accepts_path_lists` 通过，说明显式 CA bundle 支持平台 path-list。
- `remote_relay_ca_file_envs_include_common_tooling_overrides` 通过，说明 Git/AWS/pip/npm/gRPC 等常见 CA file 环境变量可作为私有根证书来源。
- `remote_relay_builder_builds_with_explicit_ca_bundle` 通过，说明显式 CA bundle 能被接入 builder。
- `remote_relay_unsafe_ssl_env_parses_true_false_and_invalid_values` 通过，说明 unsafe env 只有明确启用值才会生效。

## 清理步骤

- E2E 脚本通过 `trap` 自动停止 HTTPS mock relay。
- E2E 脚本通过 `trap` 自动删除 `.bifrost-e2e-remote-relay-tls.*` 临时目录。
- 若中断测试，手动执行 `pkill -f "remote-relay-tls"` 并删除仓库根目录下 `.bifrost-e2e-remote-relay-tls.*`。

## 执行结果

| 用例 | 状态 | 日期 | 证据 |
| --- | --- | --- | --- |
| TC-RITT-01 | 通过 | 2026-07-01 | 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`，Case 1 生成私有 CA HTTPS relay，在未设置任何 CA/unsafe env 时 `remote conn up` 非 0 退出，输出包含 `start pairing failed` 和 `error sending request`，未进入成功配对 |
| TC-RITT-02 | 通过 | 2026-07-01 | 同一脚本 Case 2 设置 `BIFROST_REMOTE_RELAY_CA_BUNDLE=<临时 ca.pem>` 后连接成功，输出包含 `Connected! Authorization granted`，`remote-connections.json` 包含 HTTPS relay URL 与 `client-tls-123456` |
| TC-RITT-03 | 通过 | 2026-07-01 | 同一脚本 Case 3 不设置额外 CA，仅设置 `BIFROST_REMOTE_UNSAFE_SSL=1` 后连接成功，输出包含 `Connected! Authorization granted`，`remote-connections.json` 包含 HTTPS relay URL 与 `client-tls-123456` |
| TC-RITT-04 | 通过 | 2026-07-01 | 执行 `cargo test -p bifrost-core http_client::tests -- --nocapture`，18 个 http_client 测试全部通过，覆盖 CA path-list、常见 CA env、proxy bypass、显式 CA bundle builder 与 unsafe env 解析 |
