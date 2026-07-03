# 远程访问暴力破解防护

## 背景

Bifrost Admin/Web 端支持 `/api/auth/login` 用户密码登录以及非 loopback 场景下的远程访问 (`set_remote_access_enabled(true)`)，密码通过 bcrypt 存到 `AuthDb`（sled-backed）。远程管理入口暴露在公网时会承受穷举攻击风险：

- 6 位以上密码依然可能通过慢速枚举被撞库。
- 未做失败率限制的情况下，攻击者可以并发爆破。
- 一旦成功登录，攻击者可获得 admin JWT（7 天 TTL），进而通过 API 完全控制 Bifrost 代理、规则、脚本。

本方案在管理认证层加固：登录失败计数、密码强度校验、渐进式延时、剩余次数提示与前端反馈，避免爆破成本被极端压低。

### 与最初设计的差异（2026-07-03 复核）

原设计要求达到阈值后 **自动执行 lockout**：`execute_lockout` 停用远程访问、清空密码 hash、撤销所有 admin 会话，之后必须 loopback 重设密码才能恢复。当前代码 (`crates/bifrost-admin/src/admin_auth.rs`) 已经**去掉了破坏性 lockout**，改为“非破坏性节流”（`record_failed_login` 会 `warn!("... applying non-destructive throttling")`），密码 hash 与 remote access 状态保持不动，成功登录会自然清零计数。

两个新增单测明确了这一变更：

- `test_failed_login_limit_preserves_remote_and_password`：达到 `MAX_LOGIN_ATTEMPTS` 后 `is_remote_access_enabled` 与 `has_admin_password` 仍为 true。
- `test_failed_login_limit_allows_successful_recovery_without_local_reset`：即使打满失败次数，只要下一次输入正确密码，仍可自然恢复；不需要 loopback 手动重置。

原文中的“执行锁定 → 清密码 → 强制 loopback 恢复”流程与代码不一致，本次文档更新以真实实现为准，并保留“如何再引入更严格 lockout（可选）”作为未来选项。

## 用户目标验证清单

### 必须实现（已 ship）

- 全局连续失败计数存储在 `AuthDb`（sled），键 `login_failed_count`，通过 `get_failed_count / increment_failed_count / reset_failed_count` 读写。
- 常量：`MAX_LOGIN_ATTEMPTS = 5`、`MIN_PASSWORD_LENGTH = 6`（`admin_auth.rs:13-14`）。
- `record_failed_login` 累加失败次数；到达阈值时打印 `warn!("Login attempts exhausted; applying non-destructive throttling")`（`admin_auth.rs:134-145`）。
- `reset_failed_login_count` 在成功登录后清零（`admin_auth.rs:147-151`）。
- `validate_password_strength` 拒绝空、长度 <6、纯字母或纯数字（`admin_auth.rs:73-91`）。
- `/api/auth/login`（`handlers/auth.rs`）：
  - 密码错误 → 记录失败 → 写审计（`admin_audit::record_failed_login_attempt(username, peer_ip, user_agent)`）。
  - 渐进式延时 `sleep(min(failed_count, 10) s)` 后再返回。
  - `failed_count >= MAX_LOGIN_ATTEMPTS` → 返回 `403 { error, locked_out: true }`。
  - 否则返回 `401 { error }`；剩余次数 ≤3 时错误串为 `"Invalid credentials. Few attempts remaining before lockout."`。
  - 成功登录 → `reset_failed_login_count`。
- `/api/auth/status`：字段 `locked_out`、`failed_attempts`、`max_attempts`、`min_password_length`（`handlers/auth.rs:40-43`、`:78-92`、`:346-355`）。`failed_attempts` 只在 loopback 上下文暴露真实数字，远程场景回退为 0。
- 前端 `AdminAuthStatus` 增加对应字段，`Login.tsx` 显示剩余次数与锁定提示，`RemoteAccessTab.tsx` 显示锁定与密码强度提示，`adminAuth.ts` 同步类型。

### 必须不破坏

- 成功登录仍立即发出 admin JWT（7d TTL），并携带 `Authorization: Bearer <token>` header；`Cookie` 语义不变。
- 密码 hash、remote access 开关、admin sessions **不因** 达到失败阈值被清理；本机 loopback 重设操作不是恢复的**必要条件**。
- `set_admin_password_hash` 仍然通过 `validate_password_strength` 拒绝弱密码（见 `test_set_admin_password_rejects_weak_password`）。
- `verify_admin_credentials` 仍走 bcrypt 校验，不受失败计数影响（`test_failed_login_limit_allows_successful_recovery_without_local_reset` 断言）。

### 必须真实验证

- 5 次错误后立即出现 403 + `locked_out: true`；剩余次数 ≤3 时错误串明显。
- 弱密码在 `/api/auth/passwd` 与 `set_admin_password_hash` 处双端拒绝。
- 成功登录清零失败计数；下一次错误从 1 开始。

## 产品语义

### 非破坏性节流

达到 `MAX_LOGIN_ATTEMPTS` 后：

- 后续每一次错误尝试仍会累加 `failed_count`，触发 `min(count, 10)` 秒延时。
- 后续所有 API 响应保持 `403 { error, locked_out: true }`，直到有一次正确凭据登录才 reset 计数、回到 `401` 语义。
- 密码 hash 与远程访问开关不变；不需要重启 Bifrost 或 loopback 重设密码。
- 计数是全局的，不按 IP 分片：任何来源的攻击者与合法用户共享同一计数窗口。这是为了避免攻击者用不同来源规避阈值。

### 渐进式延时

`sleep(min(failed_count, 10) s)` 上限 10 秒。第一次错 1 秒、第二次 2 秒、…、第五次及以后 10 秒。合法用户偶尔敲错代价极小，攻击者串行成本按 QPS 显著下降。

### 前端语义

- `AdminAuthStatus.locked_out = failed_attempts >= max_attempts`（前端计算），后端 `AuthStatus.locked_out` 在当前实现里恒为 `false`，前端主要以 `failed_attempts >= max_attempts` 为判定源，同时 login 响应中的 `locked_out: true` 是权威信号。
- `Login.tsx` 在剩余次数 ≤3 时高亮红色提示，在锁定时展示引导用户等待或联系管理员的文案。
- `RemoteAccessTab.tsx` 使用 `min_password_length` 与常量提示密码规则。

## 技术细节

### 后端

| 文件 | 关键接口 | 说明 |
| --- | --- | --- |
| `crates/bifrost-admin/src/admin_auth.rs` | `MAX_LOGIN_ATTEMPTS`、`MIN_PASSWORD_LENGTH`、`validate_password_strength`、`record_failed_login`、`reset_failed_login_count`、`get_failed_login_count`、`set_admin_password_hash`、`verify_admin_credentials` | 计数、限制、密码校验中心逻辑 |
| `crates/bifrost-admin/src/admin_auth_db.rs` | `AuthDb::get_failed_count / increment_failed_count / reset_failed_count`、`get_password_hash / set_password_hash`、`get_revoke_before / set_revoke_before` | sled 存储层 |
| `crates/bifrost-admin/src/admin_audit.rs` | `record_failed_login_attempt(username, ip, user_agent)`、`record_login(...)` | 失败/成功审计 |
| `crates/bifrost-admin/src/handlers/auth.rs` | `/api/auth/login`、`/api/auth/status`、`/api/auth/passwd`、`AuthStatus.locked_out/failed_attempts/max_attempts/min_password_length` | HTTP 接口 |
| `crates/bifrost-cli/src/commands/admin.rs` | `bifrost admin reset-password`、`admin set-remote-access` 等本地入口 | loopback 恢复通道 |

### 前端

- `packages/bifrost-webui/src/services/adminAuth.ts`：`AdminAuthStatus` 扩展 `locked_out: boolean`、`failed_attempts: number`、`max_attempts: number`、`min_password_length: number`。
- `packages/bifrost-webui/src/pages/Login.tsx`：失败时展示剩余次数；锁定时展示 blocked 状态。
- `packages/bifrost-webui/src/pages/settings/RemoteAccessTab.tsx`：展示锁定状态、密码强度提示、`min_password_length` 常量说明。

### CLI

- `bifrost admin status`：显示远程访问是否启用、`failed_attempts` 与阈值（loopback 场景）。
- `bifrost admin reset-password`：本地 loopback 强制重设密码，作为保底恢复通道；重设时会重跑 `validate_password_strength`。

### Admin API 详解

- `POST /api/auth/login`
  - 200：`{ token, expires_at, username }` + `Authorization: Bearer <token>` header。
  - 401：`{ error }`，剩余 ≤3 时使用醒目文案。
  - 403：`{ error, locked_out: true }` 当 `failed_count >= MAX_LOGIN_ATTEMPTS`。
- `GET /api/auth/status`
  - 返回 `{ requires_password, remote_access_enabled, ..., locked_out, failed_attempts, max_attempts, min_password_length }`。
- `POST /api/auth/passwd`
  - 走 `validate_password_strength`；不合规返回 400 `{ error }`；成功后清零 `login_failed_count`。

### Sync 边界

- `AuthDb` 是本机管理数据库，不参与 rules/values sync。
- 失败计数、密码 hash、JWT secret 都不同步；每台设备独立管理。

## Phase 1 – 计数与限制

- 定义常量、`AuthDb` 计数字段、`record_failed_login / reset_failed_login_count / get_failed_login_count`。
- `/api/auth/login` 集成失败计数与延时。
- `/api/auth/status` 暴露 `locked_out / failed_attempts / max_attempts / min_password_length`。

## Phase 2 – 密码强度

- `validate_password_strength` 校验非空、长度、必须字母+数字。
- `set_admin_password_hash` 与 `/api/auth/passwd` 双端拒绝。

## Phase 3 – 前端反馈

- `Login.tsx` / `RemoteAccessTab.tsx` / `adminAuth.ts` 展示剩余次数、锁定态与密码强度提示。

## Phase 4 – 审计与运维

- `admin_audit::record_failed_login_attempt` 写入结构化日志，含 username / peer_ip / user_agent / timestamp。
- 到达阈值时打印 `warn!` 便于运维告警。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/admin_auth.rs` 已存在）

- `test_record_failed_login_increments_count`
- `test_reset_failed_login_count_works`
- `test_failed_login_limit_preserves_remote_and_password`
- `test_failed_login_limit_allows_successful_recovery_without_local_reset`
- `test_set_admin_password_rejects_weak_password`
- `test_set_admin_password_hash_and_verify_credentials`
- `test_set_admin_password_rejects_empty`
- 密码强度：`validate_password_strength` 对 `""`、`"ab1"`、`"123456"`、`"abcdef"` 应报错；对 `"abc123"`、`"Test99"`、`"longpassword1"` 通过（现有测试 line 430-461）。

### E2E 测试

- `curl -X POST /api/auth/login` 5 次错误后返回 403 + `locked_out: true`。
- `curl` 弱密码 `POST /api/auth/passwd` 返回 400。
- 5 次错误后本机 loopback `POST /api/auth/login` 输入正确密码，返回 200 并 reset 计数（覆盖“non-destructive throttling 恢复”）。

### 真实场景测试

- `human_tests/remote-access-brute-force-protection.md`
  - TC-BRUTE-01 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 启动 Bifrost。
  - TC-BRUTE-02 5 次错误密码 → 观察 401 → 401 → 401 → 401 → 403 `locked_out: true`；每一次响应延时递增。
  - TC-BRUTE-03 正确密码在 5 次错误后仍能登录成功，`failed_attempts` 归零。
  - TC-BRUTE-04 `/api/auth/passwd` 用弱密码（`"abc"`, `"123456"`, `"abcdef"`）分别返回 400。
  - TC-BRUTE-05 loopback `bifrost admin reset-password` 强制重设密码通道验证。
  - TC-BRUTE-06 `admin_audit` 日志中出现 `record_failed_login_attempt` 结构化条目，含 IP 与 UA。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin admin_auth`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `record_failed_login` 是否在密码校验失败后**再**写审计与延时，防止顺序颠倒导致定时器泄漏。
- 复核 `/api/auth/status` 在 loopback 与远程两种上下文下 `failed_attempts` 语义（远程返回 0 是设计选择）。
- 复核 `validate_password_strength` 与 `/api/auth/passwd` 的错误消息稳定可测试。
- 复测：`admin_auth` 单测、`test_failed_login_limit_preserves_remote_and_password`、`test_failed_login_limit_allows_successful_recovery_without_local_reset`。

### 第 2 轮

- 复核 loopback `bifrost admin reset-password` 是否绕过失败计数正确 reset。
- 复核前端锁定态判定逻辑：`locked_out` 由 login 响应权威；`AuthStatus` 上的 `locked_out` 保留 false，不能引起 UI 冲突。
- 复测：`human_tests/remote-access-brute-force-protection.md` 全部 6 例真实执行。

## 风险与决策点

- **是否恢复破坏性 lockout**：当前实现放弃了自动清密码/停远程访问。若产品判断风险高，可在后续版本加回 `execute_lockout`，但需增加“阈值触发前明确警告 + loopback 恢复入口更醒目”。当前非破坏性节流的优势是不会误伤合法用户被临时暴力破解风波。
- **按 IP 分片计数**：当前是全局计数。这样可以简单实现节流，但也意味着攻击者可以让合法用户陷入锁定。未来可引入按 `peer_ip` 或 `caller_fingerprint` 分片，并将全局阈值收紧为兜底。
- **延时上限**：`min(count, 10)` 秒；进一步收紧可能拖慢合法用户误敲。
- **JWT 主动撤销**：`revoke_all_admin_sessions` 仍保留，但不在阈值触发时自动调用。运维发现异常可通过 CLI/HTTP 显式调用。
- **审计存储**：`admin_audit` 目前写日志层，未持久化到独立表；如后续接告警系统需要暴露 metric（如 `bifrost_admin_failed_login_total`）。
