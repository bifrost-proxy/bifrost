# Security Hardening

## 功能模块说明

本文记录 2026-07 安全审计中 C1、C2、H1、H2、H3、H4、H5、M1、M2、M3、M4 的修复方案。目标是把受限远程执行、Admin API、脚本沙箱、SSH 密钥、安装链路、Sync 与 TLS 例外统一收敛到默认安全、显式授权、失败关闭。

## 实现逻辑

- C1 Shell 策略：`ShellText` 白名单正则必须覆盖完整命令文本，避免 `bifrost status; ...` 这类前缀命中后把整条字符串交给 `sh -c`。
- C2 登录失败：失败计数只触发非破坏性节流语义，不再关闭远程访问、不清空管理员密码、不吊销已有会话。
- H1 脚本沙箱网络：`net.fetch` 默认拒绝 loopback、私网、link-local、metadata、文档网段、多播、未指定与广播地址；新增 `sandbox.net.allow_private_network` 作为显式 opt-in，并同步 Admin API 与 Web Scripts 设置。
- H2 Admin 虚拟主机：远程访问关闭时只允许可信 loopback + 允许 Host 直接访问 Admin API；`peer_addr=None` 和非 loopback 来源必须鉴权或被拒。
- H3 SSH 密钥落盘：SSH 主密钥文件写入后设为 `0600`，admin 目录设为 `0700`，数据库文件设为 `0600`。
- H4 SSH 授权有效期：`normalize_ssh_grant_mode` 尊重请求的 `GrantMode`，不再静默改成永久授权。
- H5 安装供应链：默认镜像只保留 `https://github.com`；第三方镜像只通过 `BIFROST_GITHUB_MIRROR` 显式启用；checksum mismatch 立即失败退出。
- M1 文件 glob：glob 遍历使用 `symlink_metadata`，symlink 目标必须 canonicalize 后仍位于 root 内才允许进入结果。
- M2 open_call AAD：加密远程命令必须携带 AAD；CLI 在 `OpenCallRequest.call_id` 上生成稳定 call id 并按同一上下文加密命令。
- M3 Sync 明文远端：保存登录 session 时拒绝 `http://` remote base URL。
- M4 unsafe SSL：保持默认 false，只允许 CLI/UI/规则显式 opt-in；本次以默认值和显式分区回归测试固化行为。

## 依赖项

- Rust crates: `bifrost-admin`、`bifrost-cli`、`bifrost-script`、`bifrost-storage`、`bifrost-sync`、`bifrost-proxy`
- TypeScript package: `packages/bifrost-sync-server`
- Web UI: `web`
- Shell installer: `install-binary.sh`

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin shell_text_allow_pattern_requires_full_match --lib`
- `cargo test -p bifrost-admin failed_login_limit --lib`
- `cargo test -p bifrost-script net_fetch --lib`
- `cargo test -p bifrost-admin test_check_api_auth --lib`
- `cargo test -p bifrost-admin remote_invoke::ssh_keys --lib`
- `cargo test -p bifrost-admin glob_does_not_follow_symlink_outside_root --lib`
- `cargo test -p bifrost-admin decrypt_remote_command_payload --lib`
- `cargo test -p bifrost-sync save_login_session_rejects_empty_or_invalid_input --lib`
- `cargo test -p bifrost-proxy unsafe_ssl --lib`

### E2E 测试

- `bash e2e-tests/tests/test_security_hardening.sh`
- 其中 H5 复用并扩展 `e2e-tests/tests/test_install_binary_adaptive_download.sh`，真实执行 installer 函数路径，验证 checksum fail-close 与第三方镜像 opt-in。
- sync relay 使用 `pnpm --dir packages/bifrost-sync-server test -- --runInBand --testPathPattern remote-invoke` 验证 open-call call id 与加密信封兼容。
- Web UI 使用 `pnpm --dir web build` 验证 Scripts 设置中 sandbox private-network opt-in 类型和构建链路。

### 真实场景测试

- `human_tests/security-hardening.md` 覆盖 11 个安全回归用例。
- 执行入口为 `bash e2e-tests/tests/test_security_hardening.sh`，按输出 section 逐条核对 C1/C2/H1/H2/H3/H4/H5/M1/M2/M3/M4。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核所有审计编号是否都有代码变更或明确保留理由，执行 `git status --short`、`git diff`、安全 E2E wrapper 与相关最小测试，修复缺口。
- 第 2 轮：基于第 1 轮修复后的最新 diff，复查 Admin API、Remote Invoke、sandbox、installer、Sync、Web UI 的边界与文档一致性，复跑受影响测试并确认无需追加轮次。

## 校验要求

- 必须执行 `cargo fmt --all`。
- 必须在 E2E 后执行 rust-project-validate 技能要求的 Rust 验证。
- 本机不运行 `make coverage` 或 `make coverage-unit`，遵循当前工作区 no-local-coverage 规则；覆盖率门禁交由远端 CI。

## 文档更新要求

- 更新本文件。
- 新增 `human_tests/security-hardening.md` 并更新 `human_tests/readme.md` 索引。
- 更新 `crates/bifrost-admin/ADMIN_API.md`，记录 `sandbox.net.allow_private_network` 默认关闭与显式 opt-in 语义。
