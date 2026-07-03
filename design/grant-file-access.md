# Grant File Access — 正交权限模型

## 背景

Bifrost Remote Invoke 通过 grant 授权模型控制远端设备对本机的能力。历史 `GrantScope` 是单值枚举，把 shell、file、power、IM gateway 等能力放在同一维度，其中 file 权限只有两个粒度：

```
RemoteQuery              → 只允许 query.readonly
RemoteShellExec          → 允许 query + shell.exec
RemoteShellInteractive   → 允许 query + shell.exec (interactive)
RemoteFileRead           → 只允许 file (读)
RemoteFileWrite          → 只允许 file (读+写)
```

单值枚举的结果是 shell 权限和 file 权限互斥——一个 grant 无法同时拥有 `remote_shell_interactive` 和 file 读写。"Full Access（完全访问）"模式历史上也不包含 file 操作，这与用户直观预期"我给对方完全权限就应该包含文件"完全不符。

真实场景下用户很快会遇到这个坑：授权对端一个 grant，让对端 remote coding agent 既跑 shell 又改代码，就必须回到 Settings 里把 file access 手动叠加，非常反直觉。

本方案将 file 权限从 `GrantScope` 中拆出，作为独立 `file_access` 字段，与 `grant_scope` **正交**：一个 grant 可以同时拥有 `remote_shell_interactive` + `file_access: read_write`。File Access Policy 的配置入口也从独立模块迁移到 Grants 列表的行级按钮，避免"配置了一堆 policy 却对不上任何 active grant"的幽灵配置。

## 用户目标验证清单

### 必须实现

- `GrantScope` 保留 `remote_query` / `remote_shell_exec` / `remote_shell_interactive` / `remote_power_mgmt` / `remote_im_gateway`，删除 `remote_file_read` / `remote_file_write`。
- 新增 `file_access` 字段（`none` / `read` / `read_write`），默认 `none`，独立于 `grant_scope`。
- 权限检查按 `(grant_scope, file_access, command_kind)` 三元判定，file 命令看 `file_access`，shell 命令看 `grant_scope`，query 命令始终允许。
- Grants 列表每行提供 `File Access` 按钮，只能配置当前 grant 的 exact `grant_id` 策略；Grant ID 只读，不允许跨 grant 或手动填写。
- SSH Key 卡片直接暴露默认 File Policy（`match.ssh_fingerprint`），支持 read-only / read-write、指定 roots / 所有目录、覆盖开关、递归删除开关。
- Grant 被删除 / revoke / 过期清理时，同步删除对应 exact `grant_id` 的 file access entry，避免幽灵配置；`match.ssh_fingerprint` / `match.caller_fingerprint` 默认策略不受影响。
- 弹窗首次打开必须按执行侧 resolver 同一优先级预填有效策略：`grant_id` exact → `match.ssh_fingerprint` → `match.caller_fingerprint`。
- Reset SSH Key 时把旧 fingerprint 的 policy 迁移到新 fingerprint；只有旧 fingerprint 无显式策略时才 seed 默认。
- 兼容旧 grant：`ssh_key_fingerprint` 缺失或等于 `caller_fingerprint` 的记录，`call_open` 校验通过后用当前 active SSH key fingerprint 修复并持久化，再解析 file policy。

### 必须不破坏

- 现有 grant 的 shell 权限行为不变；`RemoteShellExec` / `RemoteShellInteractive` 命令允许列表保持一致。
- `remote_query` / `remote_power_mgmt` / `remote_im_gateway` scope 保持独立能力。
- `file-access.toml` 语法结构保持兼容（`[[grant]] match.grant_id / match.ssh_fingerprint / match.caller_fingerprint` 均可）；只是 UI 入口迁移。
- `remote grant list` / `remote grant show` CLI 输出结构保持向后兼容，只新增 `file_access` 字段。
- Relay 服务端拒绝旧 `remote_file_read` / `remote_file_write` scope 值时给出稳定错误，便于客户端定位。

### 必须真实验证

- 真实创建一个 grant，通过 WebUI 授权 Full Access → `grant_scope=remote_shell_interactive` + `file_access=read_write` 自动锁定。
- 真实通过 SSH Key 建立 grant，Settings 中 SSH Key 卡片显示默认 policy 状态；执行 `bifrost remote file read/write` 命中 `match.ssh_fingerprint` 策略。
- 删除 grant 后 `file-access.toml` 中对应 exact `grant_id` entry 被清除。
- Reset SSH Key 后新 fingerprint 继承旧策略，file 操作仍成功。

## 产品语义

### `grant_scope` 与 `file_access` 正交

两个字段完全独立：

| 模式 | grant_scope | file_access |
|------|-------------|-------------|
| Query only | `remote_query` | `none` |
| Shell only | `remote_shell_exec` | `none` |
| File only | `remote_query` | `read` / `read_write` |
| Full Access | `remote_shell_interactive` | `read_write`（自动锁定） |
| Custom | 用户选择 | 用户独立选择 |

"Full Access" 语义变为"shell + file 都最大"，与用户直观理解一致。

### File Access Policy 绑定 active grant

- Grants 列表行级 `File Access` 按钮只能配置当前 grant 的 exact `grant_id` 策略。
- 弹窗中 Grant ID 只读；不提供跨 grant 输入或选择，避免写入不存在的 grant 造成幽灵。
- 每个 active grant 预期绑定一个 exact `grant_id` 策略；首次打开时基于当前 `file_access` 生成默认草稿。
- 保存后写入 `[[grant]] grant_id = "<active grant id>"`。
- Grant 生命周期结束（delete / revoke / 过期清理），只清理 `match.grant_id` / legacy flat `grant_id`；不清理 `match.ssh_fingerprint` / `match.caller_fingerprint`。

### SSH Key 默认策略在卡片上直接暴露

SSH Key 连接没有 pair-code 授权弹窗，因此 Settings → Remote Invoke → SSH Key 卡片上直接暴露默认 File Policy：

- 活跃 SSH Key 展示 `match.ssh_fingerprint` 策略状态：未配置 / read-only / read-write，roots 数量。
- Configure：设置访问级别、目录范围（指定 roots / 所有目录）、允许覆盖、允许递归删除。
- 保存写入 `[[grant]] match.ssh_fingerprint = "<current key fingerprint>"`，新 SSH Key grant 自动命中同一策略。
- Reset Key：把旧 fingerprint 的 policy 迁移到新 fingerprint；只有旧 fingerprint 无显式策略时才 seed 默认。
- 用户误删当前 fingerprint 策略时，`GET/PUT /remote-invoke/file-access-config` 必须自动恢复默认 fingerprint 策略并写回，避免刷新后进入无 policy 状态。

### 弹窗预填与执行侧 resolver 一致

Grants 行级 `File Access` 编辑器首次打开时按执行侧 resolver 同一优先级预填有效策略：`grant_id` exact → `match.ssh_fingerprint` → `match.caller_fingerprint`。这样 SSH Key 默认策略配置为 `roots = ["/"]` 时，通过该 key 连接出的 active grant 即使没有 exact policy，弹窗也必须显示 `Directories = All`，不能回退成空 `Selected`。用户点击保存后再把当前有效值落成该 grant 的 exact 策略。

## 技术细节

### 类型定义

```rust
// types.rs
pub enum GrantScope {
    RemoteQuery,
    RemoteShellExec,
    RemoteShellInteractive,
    RemotePowerMgmt,
    RemoteIMGateway,
}

pub enum FileAccessScope {
    None,
    Read,
    ReadWrite,
}

pub struct GrantInfo {
    pub grant_scope: GrantScope,
    pub file_access: FileAccessScope,
    // ...
}
```

### 权限检查

```rust
fn scope_allows_command(
    grant_scope: GrantScope,
    file_access: FileAccessScope,
    command_kind: CommandKind,
) -> bool {
    match command_kind {
        CommandKind::File => matches!(file_access, FileAccessScope::Read | FileAccessScope::ReadWrite),
        CommandKind::FileWrite => matches!(file_access, FileAccessScope::ReadWrite),
        CommandKind::ShellExec => matches!(
            grant_scope,
            GrantScope::RemoteShellExec | GrantScope::RemoteShellInteractive
        ),
        CommandKind::QueryReadonly => true,
    }
}
```

### 后端修改点

- `crates/bifrost-admin/src/handlers/remote_invoke.rs`：`ApproveBody` / `UpdateGrantBody` 新增 `file_access`。
- `crates/bifrost-admin/src/remote_invoke/worker.rs`：`ShellGrantProvision` 增加 `file_access`；权限检查用 `scope_allows_command()`。
- `crates/bifrost-admin/src/remote_invoke/grant_policy_store.rs`：`StoredGrantPolicy` 增加 `file_access`。
- Migration：读取旧 grant 存储时，把 `RemoteFileRead` → `grant_scope=RemoteQuery, file_access=Read`；`RemoteFileWrite` → `grant_scope=RemoteQuery, file_access=ReadWrite`。若 Relay 侧已拒绝旧 scope，则按项目约定清空重建。

### file-access.toml 结构

```toml
[[grant]]
match.grant_id = "abc12345"
ops = ["file.read", "file.list", "file.stat", "file.glob", "file.search", "file.hash",
       "file.write", "file.edit", "file.patch", "file.upload"]
roots = ["/Users/eden/work"]
deny_globs = ["**/.git/**", "**/.env"]
max_bytes = 5242880
allow_overwrite = true
allow_recursive_delete = false

[[grant]]
match.ssh_fingerprint = "SHA256:..."
ops = ["file.read", "file.list", "file.stat"]
roots = ["/"]
```

`match.grant_id`（exact）优先级最高，其次 `match.ssh_fingerprint`，最后 `match.caller_fingerprint`。

### 执行侧 resolver

`call_open` 处理流程：

1. 校验 grant 授权链、caller fingerprint、SSH key fingerprint 有效性。
2. 若 `auth_method=ssh_publickey` 且 `ssh_key_fingerprint` 缺失或等于 `caller_fingerprint`，用当前 active SSH key fingerprint 修复并持久化。
3. 按 `grant_id → ssh_fingerprint → caller_fingerprint` 顺序查 `file-access.toml`。
4. 未命中默认策略：cwd readonly fallback（只允许 `roots = [<cwd>]`、`ops = [FILE_READ_OPS]`）。

### WebUI 修改点

- `web/src/api/remoteInvoke.ts`：新增 `FileAccessScope` 类型；grant request/response 携带 `file_access`。
- `web/src/components/PairingRequestModal`：预设策略模式（Full Access / Shell Only / File Only / Query Only / Custom），Full Access 自动锁定 `file_access=read_write`。
- `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`：
  - Grants 列表新增每行 `File Access` 按钮 + File Access Tag（none/read/read_write）。
  - 弹窗 Grant ID 只读；预填值按 resolver 优先级。
  - 保存后写入 `[[grant]] grant_id = "<active grant id>"`。
  - Grant 删除 / revoke 时同步 DELETE `/remote-invoke/file-access-config?grant_id=...`。
  - SSH Key 卡片新增默认 File Policy 编辑器，支持访问级别、目录范围、覆盖、递归删除。

### CLI 修改点

- `bifrost remote grant update --file-access <none|read|read_write>` 新增参数。
- 移除 `--scope remote_file_read` / `--scope remote_file_write` 选项；若命中，输出稳定错误并提示新用法。
- `bifrost remote grant list` / `grant show` 输出新增 `file_access` 列。

### Relay (TypeScript)

`packages/bifrost-sync-server/src/remote-invoke/`：

- 移除 `remote_file_read` / `remote_file_write` scope 值。
- 新增 `normalizeFileAccess()` 函数。
- `grantScopeAllowsCommand()` 增加 `fileAccess` 参数。
- grant 创建 / 更新 / SSE 事件中传递 `file_access`。

## Admin API

- `POST /remote-invoke/grants/approve`：`{grant_scope, file_access}`。
- `PUT /remote-invoke/grants/:id`：`{grant_scope?, file_access?}`。
- `GET /remote-invoke/grants`：返回带 `file_access` 的列表。
- `GET /remote-invoke/file-access-config`：返回当前 `file-access.toml` 结构（`grants` 数组），若 SSH Key 默认策略缺失自动恢复。
- `PUT /remote-invoke/file-access-config`：写入 `file-access.toml`；缺失 SSH Key 默认策略时补齐。
- `DELETE /remote-invoke/file-access-config?grant_id=<id>`：清理 exact `grant_id` entry；不动 fingerprint entry。

## Sync 边界

- Grant scope 与 file_access 是本机权限决策，不参与 Sync；只有 grant metadata（id、caller fingerprint、创建时间）沿用现有 Sync 通道。
- `file-access.toml` 是本机存储，不同步；跨设备时用户在每台设备独立配置。
- Relay 服务端只做转发与 scope 白名单校验，不落 policy。

## 向后兼容

不兼容旧数据：旧 grant 存储 `remote_file_read` / `remote_file_write` scope 会被 Relay 的 `normalizeGrantScope` 拒绝。这符合项目约定：协议更新时直接删除旧数据库重建。发布 note 中提示用户清空 grant 后重新授权。

## 实现切分

### Phase 1：类型与后端

- `GrantScope` / `FileAccessScope` 类型拆分。
- `ShellGrantProvision`、`StoredGrantPolicy`、`ApproveBody`、`UpdateGrantBody` 扩展 `file_access`。
- `scope_allows_command` 三元判定。
- Migration 与旧数据兼容策略。
- 单元测试覆盖权限矩阵与 policy resolver。

### Phase 2：Admin API 与 CLI

- Grants API / File Access Config API 契约更新。
- CLI `remote grant update --file-access` 新增；旧选项拒绝。
- 集成测试覆盖 API + CLI。

### Phase 3：WebUI

- `PairingRequestModal` 预设策略模式与自动锁定。
- `RemoteInvokeTab` Grants 行级 `File Access` 按钮 + Tag。
- 弹窗预填、Grant ID 只读、生命周期同步删除。
- SSH Key 卡片默认 File Policy 编辑器。
- Playwright 覆盖模式切换、弹窗预填、SSH Key policy 保存。

### Phase 4：Relay & 迁移

- Relay TS 端 `normalizeFileAccess` + `grantScopeAllowsCommand`。
- 旧 scope 拒绝错误的稳定文案与错误码。
- 文档：`site/src/content/docs/reference/design/`、`human_tests/grant-file-access.md`、`human_tests/file-access-webui.md`。

## 测试方案

### 单元测试

- `scope_allows_command_full_access_grants_file_read_write`
- `scope_allows_command_shell_only_forbids_file`
- `scope_allows_command_file_only_forbids_shell`
- `file_policy_resolver_prefers_exact_grant_id_over_fingerprint`
- `file_policy_resolver_falls_back_to_cwd_readonly`
- `call_open_repairs_missing_ssh_fingerprint_from_active_key`
- `grant_delete_removes_exact_file_access_entry_but_not_fingerprint`
- Relay TS: `normalize_grant_scope_rejects_remote_file_read`、`grant_scope_allows_command_with_file_access`。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_grant_file_access.sh`：真实 grant 创建 + file.read/write 命中。
- SSH Key path：真实 SSH key 建立 grant，`bifrost remote file write` 命中 `match.ssh_fingerprint`。
- Grant 删除后 exact policy 被清理。
- Reset Key 后新 fingerprint 继承旧策略。
- WebUI Playwright：`web/tests/ui/file-access-roots.spec.ts` 与 `web/tests/ui/remote-invoke.spec.ts` 扩展 File Access 行为。

### 真实场景测试 human_tests

- 更新 `human_tests/grant-file-access.md` 与 `human_tests/file-access-webui.md`：
  - TC-GFA-01：Full Access 模式 shell + file 同时可用。
  - TC-GFA-02：Custom 模式独立选择 shell 与 file_access。
  - TC-GFA-03：SSH Key 卡片默认 policy 生效。
  - TC-GFA-04：Grant 删除同步清理 exact entry。
  - TC-GFA-05：Reset SSH Key 迁移策略。
  - TC-GFA-06：误删 fingerprint policy 自动恢复。
  - TC-GFA-07：旧 grant `ssh_key_fingerprint` 缺失时 `call_open` 修复。
- 所有命令使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin file_access_policy`
- `cargo test -p bifrost-cli remote_grant_update_file_access`
- `pnpm -C packages/bifrost-sync-server test -- remote-invoke-security`
- `pnpm -C web test -- remote-invoke`
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下不运行 `make coverage`；交付时说明豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：正交模型、Full Access 自动锁定、Grants 行级入口、SSH Key 卡片默认策略、旧 grant 修复。
- 复核 diff：`GrantScope` 是否只保留 5 个值、`file_access` 是否覆盖所有 API/CLI/UI 路径、Relay 是否拒绝旧 scope。
- 重点 review：Grant 生命周期同步清理是否只作用于 exact `grant_id`；弹窗预填是否用同一 resolver；`call_open` 修复是否幂等。
- 复测：Rust 单元测试、Relay TS 单测、WebUI Playwright、E2E grant file access 场景。

### 第 2 轮

- 复核第 1 轮修复的 diff。
- 复核 `file-access.toml` 迁移是否幂等，重启后策略不丢失。
- 复核旧 grant 拒绝提示是否稳定可测试。
- 复测：human_tests 采样、`git status --short` 与 `git diff`、workspace validate。

## 风险与决策点

- **旧 grant 破坏性升级**：项目约定协议更新时直接删旧数据；发布 note 明确用户需要重新授权。若未来产品希望平滑升级，需要额外 migration 层。
- **Full Access 自动锁定**：可能有用户希望 Shell 完全但 File 只读；本方案下需要走 Custom 模式，Full Access 语义等同 shell + file 双最大。
- **SSH Key policy 幽灵**：Reset Key 后旧 fingerprint policy 若无迁移，会残留；本方案明确"迁移优先、只有旧无策略才 seed 默认"。
- **弹窗预填与实际保存差异**：预填只在保存后才落地为 exact policy；用户点击"取消"不应新增策略。UI 必须区分"预览"与"保存"。
- **权限判定语义扩展**：未来若引入 `file_execute` 或 `file_link` 更细粒度，`FileAccessScope` 建议保持三档 + 单独 flag（如 `allow_recursive_delete`），不做频繁枚举扩展，避免破坏契约。
