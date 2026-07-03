# Security Hardening

## 背景

2026-07 内部与合规安全审计对 Bifrost 提出 11 项发现（C1、C2、H1–H5、M1–M4），覆盖受限远程执行、Admin API、脚本沙箱、SSH 密钥、安装链路、Sync 与 TLS 例外。目标是把默认策略统一收敛到“默认安全、显式授权、失败关闭”，避免任何一条路径悄悄拉宽信任面：

- **C1 Shell 策略**：`ShellText` 白名单正则如果不覆盖整条命令，`bifrost status; rm -rf /` 会被前缀命中后整句 `sh -c` 执行。
- **C2 登录失败**：失败计数不应触发破坏性动作（关闭远程访问 / 清空管理员密码 / 吊销全部会话）。
- **H1 脚本 net.fetch**：默认应拒绝 loopback、私网、link-local、metadata、文档网段、多播、未指定与广播地址。
- **H2 Admin 虚拟主机**：远程访问关闭时非可信 loopback / 非允许 Host 必须 401，`peer_addr=None` 也要鉴权。
- **H3 SSH 密钥落盘权限**：主密钥 `0600`、admin 目录 `0700`、数据库 `0600`。
- **H4 SSH 授权有效期**：`normalize_ssh_grant_mode` 必须尊重请求 `GrantMode`，不再静默改成永久授权。
- **H5 安装供应链**：默认镜像只保留 `https://github.com`；第三方镜像通过 `BIFROST_GITHUB_MIRROR` 显式启用；checksum mismatch 立即失败。
- **M1 file glob**：symlink 目标必须 canonicalize 后仍在 root 内才允许。
- **M2 open_call AAD**：加密远程命令携带 AAD；CLI 生成稳定 `call_id` 并按同一上下文加密。
- **M3 Sync 明文 relay**：拒绝 `http://` remote base URL 保存 login session。
- **M4 unsafe SSL**：默认 `false`，只允许 CLI/UI/规则显式 opt-in；本轮以默认值与显式分区回归固化。

本设计把这 11 项与既有 E2E/单元测试的映射固化，并在 `human_tests/security-hardening.md` 逐条留痕，避免下一轮审计出现 regression。

## 用户目标验证清单

### 必须实现（按审计编号）

- **C1**：`ShellText` 白名单 `Regex::new(pattern).unwrap()` 必须 anchored（`^...$` 或 `is_match` 前显式检查完整命令）；`bifrost status; ...` 前缀命中不能通过。
- **C2**：`AdminAuth::record_failed_login` 只增加节流窗口，不修改密码、不吊销 remote access。正确密码达到阈值前后仍能登录成功。
- **H1**：`SandboxConfig` 默认 `allow_network=false` / `allow_private_network=false`；`validate_net_fetch_target` 阻断整个私有 IP 家族。
- **H2**：Admin listener `peer_addr=None` 或来源非 loopback 时必须走 token/session 校验；DevTools bridge 走 token protection 不被误伤。
- **H3**：`bifrost-admin/src/remote_invoke/ssh_keys.rs` 写文件后 `chmod` 到 `0600`、目录 `0700`、`.db` `0600`。
- **H4**：`normalize_ssh_grant_mode` 只在 `GrantMode::Ephemeral` 未指定 TTL 时补默认值，永远不把 `Ephemeral` 升成 `Permanent`。
- **H5**：`install-binary.sh` 只默认 `https://github.com`；`BIFROST_GITHUB_MIRROR` 显式解锁镜像；下载后 checksum 校验失败立即 `exit 1`。
- **M1**：`bifrost remote file glob` 使用 `symlink_metadata` 判断类型；symlink 通过 `fs::canonicalize` 检查在 root 内才允许。
- **M2**：`OpenCallRequest` 携带 CLI 生成的 `call_id`；加密封装带 AAD，服务器解密时验证。
- **M3**：`bifrost sync login --relay-url http://...` 直接拒绝并给出错误。
- **M4**：Proxy `unsafe_ssl` 默认 `false`；显式规则/CLI/UI 打开时明确留痕。

### 必须不破坏

- Admin 正常登录、正确密码恢复、远端 remote_invoke pair/claim/open/revoke 链路继续可用。
- HTTPS + WebSocket + 断点、CONNECT 隧道等真实代理链路无回归。
- 内网合法脚本（BP parser 通过 sandbox opt-in 访问 BAM mock）仍能跑通。
- Sync 通过 `https://` relay 的正常登录路径保持工作。
- `bifrost upgrade` / 一键安装脚本正常无镜像场景全流程通过。

### 必须真实验证

- 用真实 Admin API 达到失败阈值后不破坏密码 + 正确密码可恢复。
- 用真实 relay 复跑 remote_invoke pop-pair / claim / open-call / revoke。
- 用真实 installer 函数路径验证 checksum fail-close 与第三方镜像 opt-in。
- 用 sandbox `allow_private_network=false/true` 分别验证 BP parser 拒绝/成功。
- 用 Sync CLI 验证 `http://` relay 被拒、`https://` token login 保存成功。

## 产品语义

### 默认关闭 + 显式 opt-in

任何“可能扩大信任面”的开关都必须：
- 默认 `false`。
- CLI / Admin API 侧显式 opt-in，且带明确错误码。
- Web UI 打开时带醒目警示。
- 关闭状态下的行为可测。

### 失败关闭

- Shell 白名单不匹配 → 拒绝执行。
- Sandbox 网络地址不在允许集合 → `net.fetch` 抛错。
- Admin 未鉴权 → 401。
- Installer checksum mismatch → `exit 1`。
- Sync `http://` relay → 拒绝保存。

### 非破坏性节流

- C2 登录失败节流走窗口 backoff（例如指数递增），不动密码、不动会话、不动 remote access。
- 正确密码到达阈值后仍可登录，节流状态自然复位。

## 技术细节

### C1 ShellText 白名单

`crates/bifrost-admin/src/shell/*`：`ShellText::matches_allow_pattern` 必须对整条命令做匹配。测试：

- `cargo test -p bifrost-admin shell_text_allow_pattern_requires_full_match --lib`

### C2 登录失败

`crates/bifrost-admin/src/admin_auth.rs:490` 与 `:508`：

- `test_failed_login_limit_preserves_remote_and_password`
- `test_failed_login_limit_allows_successful_recovery_without_local_reset`

### H1 Sandbox 网络

`crates/bifrost-script/src/sandbox.rs:97 validate_net_fetch_target`、`:143 is_private_netfetch_ip`：

- IPv4：`is_loopback | is_private | is_link_local | is_unspecified | is_broadcast | is_documentation | is_multicast | metadata (169.254.169.254) | 100.64.0.0/10 CGNAT`。
- IPv6：`is_loopback | is_unspecified | ULA (fc00::/7) | link-local (fe80::/10) | metadata (fd00:ec2::254 等) | documentation`。
- `localhost / *.localhost` 域名前置拒绝。
- 只有 `allow_private_network=true` 才短路允许。

测试：`cargo test -p bifrost-script net_fetch --lib`。

### H2 Admin 虚拟主机

`crates/bifrost-admin/src/handlers/*` + `AdminAuth::check_api_auth`：

- `peer_addr=None`：视为不可信，必须走 token/session。
- 来源 IP 非 loopback：必须走 token/session。
- Host header 不在允许集合：401。
- DevTools bridge 走 token 授权，不因虚拟主机严格化被误杀。

测试：`cargo test -p bifrost-admin test_check_api_auth --lib`。

### H3 SSH 密钥落盘

`crates/bifrost-admin/src/remote_invoke/ssh_keys.rs`：

- `write_ssh_private_key` 后 `set_permissions(0o600)`。
- admin dir 创建时 `0o700`。
- 数据库 `.db` 写入后 `0o600`。

测试：`cargo test -p bifrost-admin remote_invoke::ssh_keys --lib`。

### H4 GrantMode

`crates/bifrost-admin/src/remote_invoke/worker.rs` `normalize_ssh_grant_mode`：只在 `Ephemeral` 未指定 TTL 时补默认；永远不改变 `GrantMode` 本身。

### H5 安装供应链

`install-binary.sh`：

- 默认 `MIRROR=https://github.com`。
- `BIFROST_GITHUB_MIRROR=<url>` 显式 opt-in 第三方镜像。
- `sha256sum -c` 验证失败即 `exit 1`。
- `test_install_binary_adaptive_download.sh` 覆盖 checksum fail-close 与第三方 opt-in 两条路径。

### M1 File glob

`crates/bifrost-admin/src/remote_invoke/file/*` glob 遍历：

- 使用 `symlink_metadata` 判类型，不跟随 symlink。
- 若命中 symlink，`fs::canonicalize` 后必须仍在 `root` 内才纳入结果，否则跳过。

测试：`cargo test -p bifrost-admin glob_does_not_follow_symlink_outside_root --lib`。

### M2 Open call AAD

`crates/bifrost-admin/src/remote_invoke/*`：

- `OpenCallRequest.call_id` 由 CLI 侧生成（时间戳 + 随机 + PID 派生，稳定可日志）。
- 加密 payload 使用 `AAD = call_id || channel_id`；服务端解密时验证。

测试：`cargo test -p bifrost-admin decrypt_remote_command_payload --lib`。

### M3 Sync 明文 relay

`crates/bifrost-sync/src/manager.rs::save_login_session`：

- 输入 URL 非 `https://` → 返回 `SyncError::InvalidRelayScheme`。
- 空字符串或非法 URL → `InvalidInput`。

测试：`cargo test -p bifrost-sync save_login_session_rejects_empty_or_invalid_input --lib`。

### M4 unsafe SSL

`crates/bifrost-proxy` unsafe_ssl 默认 `false`，测试：`cargo test -p bifrost-proxy unsafe_ssl --lib`。

## CLI / Web / Admin API 快照

| 层 | 入口 | 变更 |
|---|---|---|
| CLI | `bifrost sync login --relay-url http://...` | 拒绝（M3） |
| CLI | `bifrost remote grant` | GrantMode 尊重 Ephemeral（H4） |
| CLI | `bifrost admin login`（失败重试） | 节流不破坏密码（C2） |
| CLI | `install-binary.sh` | checksum fail-close，`BIFROST_GITHUB_MIRROR` opt-in（H5） |
| Admin API | `/api/config/sandbox` | `net.allow_private_network` 默认 false（H1） |
| Admin API | 任意需鉴权路由 | 严格虚拟主机 + peer_addr 校验（H2） |
| Admin API | `remote_invoke` open-call | `call_id` + AAD 校验（M2） |
| Admin API | `remote_invoke` file glob | symlink 越界拒绝（M1） |
| Web | Scripts / Sandbox 设置 | `allow_private_network` 显式 opt-in（H1） |

## Sync 边界

- Sync 层拒绝明文 relay（M3）。
- Remote invoke 通过 relay 走 AAD 加密（M2）。
- 其余安全项不改变 sync 数据模型。

## Phase 拆分

- **Phase 1**：C1 / C2 / H2 / H3 / H4 / M1 / M2 / M3 / M4 单元与集成测试收敛。
- **Phase 2**：H1 sandbox 私网默认收紧 + `allow_private_network` opt-in + Admin API + Web UI 类型。
- **Phase 3**：H5 installer 默认镜像 + `BIFROST_GITHUB_MIRROR` opt-in + checksum fail-close。
- **Phase 4**：E2E wrapper + human_tests + CI 边界收敛（`SKIP_BUILD=true` 复用 pre-built binary、Windows serial WebSocket 回归、macOS shard 不重复聚合）。

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

- `bash e2e-tests/tests/test_security_hardening.sh`（本地/release-gate 聚合入口，不进入默认 PR shell CI）
- `bash e2e-tests/tests/test_security_hardening_functional.sh`（PR CI 覆盖）
- `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`（H5 checksum + 第三方镜像 opt-in）
- `pnpm --dir packages/bifrost-sync-server test -- --runInBand --testPathPattern remote-invoke`（M2 call_id + AAD）
- `pnpm --dir web build`（H1 Web Scripts 设置类型 + 构建）
- `cargo run -p bifrost-e2e -- --category admin --test brute_force_lockout_after_max_failures --test-timeout 80`（C2）
- `cargo run -p bifrost-e2e -- --category remote_invoke --test remote_invoke_pop_pair_claim_lookup_open_revoke --test-timeout 180`（M2 + H4）
- `bash e2e-tests/tests/test_sync_login_direct_e2e.sh`（M3）
- `bash e2e-tests/tests/test_bp_parser_e2e.sh`（H1 opt-in 后 BAM mock 可访问）
- `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`（H2 不误伤 DevTools bridge）
- `bash e2e-tests/tests/test_breakpoint_performance_guard.sh`（`SKIP_BUILD=true` 复用 pre-built binary）

### CI 覆盖策略

- 默认 PR CI 走 workspace unit/integration + coverage gate + Web build + E2E runner + installer shell + `test_security_hardening_functional.sh` 分别覆盖同一组安全修复路径，避免聚合 wrapper 在 macOS shell shard 中重复执行触发 900s per-test timeout。
- Shell E2E shard 使用 `SKIP_BUILD=true` 复用预构建 `BIFROST_BIN`，`test_security_hardening_functional.sh` 在该模式下跳过 `cargo run -p bifrost-e2e` 子步骤，由 dedicated bifrost-e2e job 与本地 full wrapper 覆盖 Admin/Remote Invoke runner。
- Windows CI 继续执行 workspace 全量单测，并将 `test_https_interception_websocket_applies_request_and_response_header_rules` 作为单独串行回归运行，避免 TLS/WebSocket CONNECT 夹具与同文件其他网络测试并发在平台上抖动。

### 真实场景测试 human_tests

- `human_tests/security-hardening.md` 覆盖 C1/C2/H1/H2/H3/H4/H5/M1/M2/M3/M4 每一项安全断言与功能回归用例。
- 执行入口：`bash e2e-tests/tests/test_security_hardening.sh`，按输出 section 逐条核对。
- 每项都包含：
  - 期望 fail-close 行为的真实触发命令。
  - 期望 pass 行为的最小 happy path（正确密码登录、`allow_private_network=true` 后可 fetch 等）。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核所有审计编号是否都有代码变更或明确保留理由，执行 `git status --short`、`git diff`、安全 E2E wrapper 与相关最小测试。
- 核对每一项对应的单元测试名/文件路径存在且能跑通。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff，复查 Admin API、Remote Invoke、sandbox、installer、Sync、Web UI 的边界与文档一致性。
- 复跑受影响测试与 `rust-project-validate`；本机不跑 `make coverage`，交给远端 CI。
- 特别复核 C2 节流复位、H1 opt-in 语义、H5 checksum fail-close 三个高风险回归点。

## 校验要求

- `cargo fmt --all`
- `rust-project-validate`（skill 侧要求的 Rust 验证）
- 本地不运行 `make coverage` / `make coverage-unit`；coverage 门禁交由远端 CI。

## 文档更新要求

- 更新本设计文档。
- 更新 `human_tests/security-hardening.md`，并保持 `human_tests/readme.md` 索引。
- 更新 `crates/bifrost-admin/ADMIN_API.md`，记录 `sandbox.net.allow_private_network` 默认关闭与显式 opt-in 语义。
- 更新 `install-binary.sh` 内的 `BIFROST_GITHUB_MIRROR` 用法说明与 checksum fail-close 提示。

## 风险与决策

- **不做隐式扩大信任**：任何新开关默认 false；若产品未来需要“允许员工设备默认放开内网”这种批量决策，也必须走单独设计与 opt-in，不通过默认值悄悄改。
- **C2 节流不破坏密码**：抵御暴力破解的正确姿势是节流 + 观测，不是清空凭据；被吊销远程访问后恢复成本极高。
- **installer 镜像默认只留 GitHub**：其他镜像信任面差异大，`BIFROST_GITHUB_MIRROR` 显式 opt-in + checksum 校验双重保险。
- **sandbox 私网 opt-in**：BP parser 等确实需要访问内网 BAM mock 的场景走显式开关；不能通过 CLI flag 或规则悄悄让脚本拿到内网访问。
- **E2E 聚合 wrapper 不进默认 PR CI**：避免 macOS shell shard 重复执行触发 900s per-test timeout；PR CI 用分散 job + functional 子集覆盖，release-gate 与本地 full run 用聚合 wrapper。
- **上游安全告警持续**：Dependabot Rust security/quality 修复见 `design/rust-dependency-audit-ci.md`；本设计只负责应用层安全，依赖层收敛与不可 patched 上游项在其他文档留痕。
