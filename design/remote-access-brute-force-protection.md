# 远程访问暴力破解防护

## 功能模块详细描述

为远程管理访问增加暴力破解防护机制，防止攻击者通过穷举密码方式获取管理权限。

### 核心机制

1. **登录失败计数**：在持久化的管理认证数据库 (`AuthDb`，sled-backed) 中按计数 key `login_failed_count` 跟踪连续失败次数（全局，非按 IP）
2. **自动锁定**：达到 5 次失败 (`MAX_LOGIN_ATTEMPTS`) 后，自动停用远程登录、删除密码 hash、撤销所有 admin 会话
3. **手动恢复**：锁定后必须从本机（loopback）重新设置密码并开启远程访问
4. **密码强度要求**：设置密码时强制最少 6 字符 (`MIN_PASSWORD_LENGTH`)，且必须同时包含字母与数字
5. **渐进式登录延时**：每次失败后响应延时 `min(failed_count, 10)` 秒，增加暴力破解成本

### 安全增强

- 失败登录的用户名、IP 和 User-Agent 通过 `admin_audit::record_failed_login_attempt` 记录到审计日志
- 锁定执行时通过 `warn!` 日志记录
- 登录失败响应在剩余次数 ≤ 3 时附加 "Few attempts remaining before lockout." 提示文案（注：当前并未在 JSON 中返回数值型 `remaining_attempts` 字段，仅返回 `error` 字符串）
- 锁定后的登录请求返回 HTTP 403 + `{ "error": ..., "locked_out": true }`
- `AuthStatus` API 返回 `locked_out`、`failed_attempts`、`max_attempts`、`min_password_length` 字段，前端据此展示状态

## 实现逻辑

### 后端变更

#### 1. `admin_auth.rs` - 登录失败计数与锁定

- 持久化位置：管理认证数据库 `AuthDb`（sled-backed，`admin_auth_db.rs`），通过 `get_failed_count` / `increment_failed_count` / `reset_failed_count` 读写 key `login_failed_count`（**非** Values Storage）
- 常量：`MAX_LOGIN_ATTEMPTS = 5`、`MIN_PASSWORD_LENGTH = 6`
- 主要函数：
  - `record_failed_login(state) -> Result<u32>`: 失败计数 +1，达到阈值时调用 `execute_lockout`
  - `reset_failed_login_count(state) -> Result<()>`: 重置失败计数（成功登录后调用）
  - `get_failed_login_count(state) -> u32`: 获取当前失败次数
  - `execute_lockout(state) -> Result<()>`: 停用远程访问 (`set_remote_access_enabled(false)`) + 清空密码 hash (`clear_admin_password`) + 撤销所有 admin 会话 (`revoke_all_admin_sessions`) + 重置失败计数
  - `validate_password_strength(password) -> Result<()>`: 校验非空、长度 ≥ 6、且同时包含字母与数字

#### 2. `handlers/auth.rs` - 登录接口变更

- `/api/auth/login`：
  - 登录失败时调用 `record_failed_login`，再通过 `admin_audit::record_failed_login_attempt` 写审计
  - 渐进式延时 `sleep(min(failed_count, 10) s)` 后再返回
  - 当 `failed_count >= MAX_LOGIN_ATTEMPTS` 时返回 HTTP 403 + `{ error, locked_out: true }`
  - 否则返回 HTTP 401 + `{ error }`，剩余次数 ≤ 3 时使用更醒目的提示文案
  - 成功登录后调用 `reset_failed_login_count`
- `/api/auth/status`：
  - 响应中新增 `locked_out` 布尔字段
  - 新增 `failed_attempts: u32` 字段（loopback 时为 `get_failed_login_count`，远程时回退为 0）
  - 新增 `max_attempts`、`min_password_length` 字段，便于前端展示
- `/api/auth/passwd`：
  - 调用 `validate_password_strength` 校验密码强度
  - 返回明确错误信息

### 前端变更

#### 1. `Login.tsx`
- 登录失败时显示剩余尝试次数
- 锁定后显示锁定提示信息

#### 2. `RemoteAccessTab.tsx`
- 显示锁定状态
- 密码输入时校验强度提示

#### 3. `adminAuth.ts`
- `AdminAuthStatus` 类型新增 `locked_out: boolean`、`failed_attempts: number`、`max_attempts: number`、`min_password_length: number` 字段

## 依赖项

- 无新增外部依赖
- 使用现有 `AuthDb` (sled, `admin_auth_db.rs`) 持久化失败计数；不依赖 ValuesStorage
## 测试方案

### 单元测试

- `test_record_failed_login_increments_count`: 验证失败次数递增
- `test_lockout_after_max_failures`: 验证达 5 次后自动锁定
- `test_lockout_clears_password_and_disables_remote`: 验证锁定后密码被删除、远程访问被禁用
- `test_reset_failed_count_on_success`: 验证成功登录重置计数
- `test_password_strength_rejects_short`: 验证 < 6 字符被拒绝
- `test_password_strength_accepts_valid`: 验证合法密码通过
- `test_lockout_resets_after_re_enable`: 验证本机重新设置后状态恢复

### E2E 测试

- 验证 5 次错误登录后返回锁定状态（403）
- 验证密码强度校验拒绝弱密码（400）
- 验证锁定后本机可重新设置密码并启用远程访问

### 真实场景测试

- 在 `human_tests/remote-access-brute-force-protection.md` 创建测试用例
