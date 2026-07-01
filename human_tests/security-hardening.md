# Security Hardening 测试用例

## 功能模块说明

本用例覆盖 2026-07 安全审计修复：Remote Invoke Shell 策略、Admin 登录失败处理、脚本沙箱网络、Admin 虚拟主机鉴权、SSH 密钥权限与授权有效期、安装脚本供应链、file glob symlink、open_call AAD、Sync 明文 URL 与 unsafe SSL 默认值。除安全断言外，还覆盖 Admin、Remote Invoke、Sync、BP parser、DevTools 的真实功能回归链路。

## 前置条件

1. 在仓库根目录执行。
2. 不启动正式 Bifrost 服务；本用例只运行临时测试进程和离线脚本。
3. 如需启动服务的后续扩展测试，必须使用临时 `BIFROST_DATA_DIR`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1`，且启动参数包含 `--no-system-proxy`。

## 测试用例列表

### TC-SH-01: Shell 策略不允许前缀注入

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin shell_text_allow_pattern_requires_full_match --lib`

**预期结果**：
- 测试通过。
- `^bifrost\s+` 只能匹配完整允许命令，不接受追加 `;`、管道等后续命令文本。

### TC-SH-02: 登录失败阈值不清空管理员密码

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin failed_login_limit --lib`

**预期结果**：
- 测试通过。
- 连续失败达到阈值后，远程访问状态和管理员密码仍保留，正确密码仍可恢复登录并重置失败计数。

### TC-SH-03: 脚本沙箱 net.fetch 默认拒绝私网

**操作步骤**：
1. 执行 `cargo test -p bifrost-script net_fetch --lib`

**预期结果**：
- 测试通过。
- 默认配置拒绝 `127.0.0.1`、私网、link-local、metadata 等目标。
- 显式 `allow_private_network=true` 的测试 fixture 仍可访问本地临时 HTTP 服务。

### TC-SH-04: Admin 虚拟主机 peer=None 不再绕过鉴权

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin test_check_api_auth --lib`

**预期结果**：
- 测试通过。
- 远程访问关闭时，`peer_addr=None` 的 Admin API 请求被拒绝；可信 loopback + 合法 Host 仍可访问。

### TC-SH-05: SSH 密钥文件权限硬化且授权模式不被改永久

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin remote_invoke::ssh_keys --lib`

**预期结果**：
- 测试通过。
- Unix 下 key 文件为 `0600`、admin 目录为 `0700`、数据库文件为 `0600`。
- `OneHour`、`ThirtyMinutes` 等请求授权模式按原样保存。

### TC-SH-06: 安装脚本 checksum fail-close 且第三方镜像显式 opt-in

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`

**预期结果**：
- 测试通过并输出 `Passed: 11`。
- 默认镜像列表只含 `https://github.com`。
- `BIFROST_GITHUB_MIRROR` 显式设置时才使用第三方镜像。
- checksum mismatch 会中止安装。

### TC-SH-07: file.glob 不跟随 root 外 symlink

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin glob_does_not_follow_symlink_outside_root --lib`

**预期结果**：
- 测试通过。
- 指向 root 外目录的 symlink 不进入 glob 结果。

### TC-SH-08: open_call 加密 payload 强制 AAD

**操作步骤**：
1. 执行 `cargo test -p bifrost-admin decrypt_remote_command_payload --lib`

**预期结果**：
- 测试通过。
- 缺少 AAD 的加密命令 payload 被拒绝。
- 携带正确 AAD 的 payload 可正常解密。

### TC-SH-09: Sync 登录拒绝明文 HTTP remote URL

**操作步骤**：
1. 执行 `cargo test -p bifrost-sync save_login_session_rejects_empty_or_invalid_input --lib`

**预期结果**：
- 测试通过。
- `http://` remote base URL 被拒绝，避免 token 通过明文链路保存为登录目标。

### TC-SH-10: unsafe SSL 保持显式 opt-in

**操作步骤**：
1. 执行 `cargo test -p bifrost-proxy unsafe_ssl --lib`

**预期结果**：
- 测试通过。
- 默认代理/SOCKS/TLS 客户端路径 `unsafe_ssl=false`，显式 true/false 配置按预期生效并进入连接池分区。

### TC-SH-11: 全量安全回归 E2E wrapper

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_security_hardening.sh`

**预期结果**：
- 脚本所有 section 均通过。
- 输出覆盖 C1、C2、H1、H2、H3/H4、H5、M1、M2、M3、M4、sync relay、Web build 和功能回归 wrapper。

### TC-SH-12: Admin 登录失败功能回归

**操作步骤**：
1. 执行 `cargo run -p bifrost-e2e -- --category admin --test brute_force_lockout_after_max_failures --test-timeout 80`

**预期结果**：
- 测试通过。
- 连续错误密码达到阈值后，Admin API 返回 lockout 语义但不清空密码、不关闭远程访问。
- 随后使用正确密码仍可恢复登录并重置失败计数。

### TC-SH-13: Remote Invoke pair/claim/open/revoke 功能回归

**操作步骤**：
1. 执行 `cargo run -p bifrost-e2e -- --category remote_invoke --test remote_invoke_pop_pair_claim_lookup_open_revoke --test-timeout 180`

**预期结果**：
- 测试通过。
- 真实 relay 流程可完成 pair、claim、grant lookup、open call、events、exit、revoke。
- AAD/call id 加固后，正常加密 open call 链路不回归。

### TC-SH-14: Sync login CLI/API 功能回归

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_sync_login_direct_e2e.sh`

**预期结果**：
- 测试通过。
- 配置到明文 HTTP relay 后，CLI/API token login fail-close。
- 使用默认 HTTPS provider 的 token login 仍能保存 token，并输出默认远端 URL。

### TC-SH-15: BP parser sandbox 私网 opt-in 功能回归

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_bp_parser_e2e.sh`

**预期结果**：
- 测试通过。
- sandbox 默认收紧后，测试通过 Admin config 显式开启 `sandbox.net.allow_private_network=true`。
- local/remote/build-in BP parser、Thrift、HTTP-RPC、BAM mock 解码链路均保持可用。

### TC-SH-16: DevTools bridge 功能回归

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`

**预期结果**：
- 测试通过。
- H2 的 Admin 虚拟主机鉴权加固不影响 token-protected DevTools page bridge。
- DevTools elements、network、storage、console、page switching、reload recovery 与 Chrome frontend cleanup 均通过。

### TC-SH-17: 功能回归 wrapper

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_security_hardening_functional.sh`

**预期结果**：
- 脚本所有 section 均通过。
- 输出覆盖 TC-SH-12 至 TC-SH-16 的真实功能链路。

## 清理步骤

- 本用例不创建持久服务数据目录。
- 若本地构建产生 `web/dist`、`web/dist-gzip` 或 `target/`，按仓库忽略规则保留为构建产物，不提交。
