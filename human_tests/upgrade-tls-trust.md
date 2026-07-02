# Upgrade TLS Trust 真实场景测试

## 功能模块说明

验证 `bifrost version-check` / `bifrost upgrade` / `bifrost install-skill` / Sync / AI provider / ASR 资源下载 / 脚本 fetch / 规则远程 URL 等外部 HTTPS 请求，能兼容系统 CA、私有 CA bundle、常见沙箱 CA 环境变量，并在受控环境中提供 unsafe SSL 最终兜底。

## 前置条件

- 在仓库根目录执行。
- 本机安装 `openssl`、`python3`、`tar`、`rustc`。
- 使用临时数据目录与临时复制的 Bifrost binary，不修改当前真实安装路径。
- 如果 `target/release/bifrost` 不存在，测试脚本会先构建 release binary。

## 测试用例列表

### TC-UTT-01：私有 CA mirror 未配置时 upgrade 失败

操作步骤：

1. 执行 `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`。
2. 脚本生成私有 CA、HTTPS mirror 和临时 Bifrost binary。
3. 观察第一段未设置额外 CA 的 `bifrost upgrade` 输出。

预期结果：

- 第一段 upgrade 非 0 退出。
- 输出包含 `certificate`、`tls`、`ssl`、`UnknownIssuer` 或 `invalid peer` 之一。
- 未替换真实安装路径，只影响临时复制的 binary。

### TC-UTT-02：`BIFROST_GITHUB_CA_BUNDLE` 允许 upgrade 下载私有 CA mirror

操作步骤：

1. 继续执行 `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`。
2. 观察设置 `BIFROST_GITHUB_CA_BUNDLE=<ca.pem>` 的 upgrade 阶段。

预期结果：

- upgrade 0 退出。
- 输出包含 `Upgrade completed successfully`。
- 证明 GitHub/version-check/upgrade 链路能读取显式私有 CA bundle。

### TC-UTT-03：`BIFROST_UPGRADE_UNSAFE_SSL=1` 可作为最终兜底

操作步骤：

1. 继续执行 `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`。
2. 观察设置 `BIFROST_UPGRADE_UNSAFE_SSL=1` 的 upgrade 阶段。

预期结果：

- upgrade 0 退出。
- 输出包含 `Upgrade completed successfully`。
- 该 unsafe 开关只作用 GitHub/upgrade HTTP client，不要求设置 `BIFROST_REMOTE_UNSAFE_SSL`。

### TC-UTT-04：使用说明包含 scoped/global CA 与 unsafe 排障入口

操作步骤：

1. 执行：
   ```bash
   rg -n "BIFROST_GITHUB_CA_BUNDLE|BIFROST_UPGRADE_CA_BUNDLE|BIFROST_CA_BUNDLE|BIFROST_UNSAFE_SSL|BIFROST_REMOTE_UNSAFE_SSL" README.md SKILL.md skill_remote.md design/upgrade-tls-trust.md
   ```

预期结果：

- README、SKILL、skill_remote 和 design 文档均能检索到 TLS trust 相关环境变量。
- 说明文字明确优先使用系统 CA/CA bundle，unsafe 仅作为受控环境最终兜底。

### TC-UTT-05：外部 HTTPS 调用点使用统一 trust builder

操作步骤：

1. 执行：
   ```bash
   rg -n "github_blocking_reqwest_client_builder|github_reqwest_client_builder|outbound_reqwest_client_builder|outbound_blocking_reqwest_client_builder" \
     crates/bifrost-cli/src/commands/install_skill.rs \
     crates/bifrost-sync/src/client.rs \
     crates/agent/src/client.rs \
     crates/agent/src/mcp/oauth.rs \
     crates/agent/src/mcp/mod.rs \
     crates/bifrost-admin/src/handlers/asr.rs \
     crates/bifrost-admin/src/handlers/asr_jobs/diarization.rs \
     crates/bifrost-admin/src/handlers/voice/wake.rs \
     crates/bifrost-admin/src/handlers/im_gateway \
     crates/bifrost-admin/src/im_gateway \
     crates/bifrost-script/src \
     crates/bifrost-core/src/rule/value_source.rs
   ```

预期结果：

- `install_skill.rs` 使用 GitHub scoped builder。
- Sync、AI provider、MCP OAuth、ASR/voice/IM/script/rule value 等外部 HTTPS 调用点使用 outbound builder。

### TC-UTT-06：安装脚本桥接 Bifrost CA/unsafe 环境变量到下载工具

操作步骤：

1. 执行：
   ```bash
   bash -n install-binary.sh
   rg -n "select_ca_bundle_file|select_ca_bundle_dir|unsafe_ssl_enabled|--cacert|--ca-certificate|--ca-directory|--capath|--insecure|--no-check-certificate|--check-certificate=false" install-binary.sh
   ```

预期结果：

- `install-binary.sh` shell 语法检查通过。
- `curl`、`wget`、`aria2c` 下载/probe/API/redirect 路径能看到 CA/unsafe 参数桥接。

## 清理步骤

- `test_upgrade_tls_trust_e2e.sh` 通过 `trap` 自动停止本地 HTTPS mirror 并删除临时目录。
- 无需清理系统代理或真实 Bifrost 数据目录。

## 执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-02 | TC-UTT-01 / TC-UTT-02 / TC-UTT-03 | 通过 | 执行 `bash e2e-tests/tests/test_upgrade_tls_trust_e2e.sh`。脚本构建当前 release binary，生成私有 CA HTTPS mirror `https://127.0.0.1:64244`，未配置 CA 的 upgrade 下载因证书信任失败；设置 `BIFROST_GITHUB_CA_BUNDLE=<ca.pem>` 后 upgrade 成功；设置 `BIFROST_UPGRADE_UNSAFE_SSL=1` 后 upgrade 成功。最终输出 `Passed: 3`、`Failed: 0`。 |
| 2026-07-02 | TC-UTT-04 | 通过 | 执行 `rg -n "BIFROST_GITHUB_CA_BUNDLE\|BIFROST_UPGRADE_CA_BUNDLE\|BIFROST_CA_BUNDLE\|BIFROST_UNSAFE_SSL\|BIFROST_REMOTE_UNSAFE_SSL" README.md SKILL.md skill_remote.md design/upgrade-tls-trust.md`，README、SKILL、skill_remote 与 design 文档均包含 scoped/global CA 与 unsafe 说明。 |
| 2026-07-02 | TC-UTT-05 | 通过 | 执行外部 HTTPS 调用点 builder 扫描，`install_skill.rs` 命中 GitHub builder，Sync、AI provider、MCP OAuth、ASR/voice/IM/script/rule value 等路径命中 outbound builder。 |
| 2026-07-02 | TC-UTT-06 | 通过 | 执行 `bash -n install-binary.sh` 与 CA/unsafe 参数扫描，脚本语法通过，curl/wget/aria2c/axel 相关下载/probe/API/redirect 路径包含 CA/unsafe 桥接参数。 |
