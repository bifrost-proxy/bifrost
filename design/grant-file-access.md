# Grant File Access — 正交权限模型

## 问题

原有 `GrantScope` 是单值枚举，shell 权限和 file 权限**互斥**：

```
RemoteQuery → 只允许 query.readonly
RemoteShellExec → 允许 query + shell.exec
RemoteShellInteractive → 允许 query + shell.exec (interactive)
RemoteFileRead → 只允许 file (读)
RemoteFileWrite → 只允许 file (读+写)
```

一个 grant 无法同时拥有 shell 执行权限和 file 操作权限。"完全访问"模式也不包含文件操作权限。

## 解决方案

将 file 权限从 `GrantScope` 中拆出，作为独立的 `file_access` 字段，与 `grant_scope` 正交。

### 新模型

| 字段 | 作用 | 可选值 |
|------|------|--------|
| `grant_scope` | 控制 shell / 查询 / 电源 / IM 网关访问级别 | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` / `remote_power_mgmt` / `remote_im_gateway` |
| `file_access` | 控制文件访问级别 | `none`（默认）/ `read` / `read_write` |

两个字段**独立设置、独立检查**，一个 grant 可以同时拥有 `remote_shell_interactive` + `file_access: read_write`。

### 权限检查逻辑

```rust
fn scope_allows_command(grant_scope, file_access, command_kind) -> bool {
    match command_kind {
        File => file_access ∈ {Read, ReadWrite},
        ShellExec => grant_scope ∈ {RemoteShellExec, RemoteShellInteractive},
        QueryReadonly => true, // 始终允许
    }
}
```

### WebUI 交互

| 模式 | grant_scope | file_access |
|------|-------------|-------------|
| Query only | `remote_query` | `none` |
| Full Access (all) | `remote_shell_interactive` | `read_write`（自动锁定） |
| Custom (selected) | 用户选择 shell 策略 | 用户独立选择 `none`/`read`/`read_write` |

### File Access Policy 配置流程

Settings → Remote Invoke 不再提供独立的 File Access 管理模块。File Access 策略入口迁移到 Grants 列表，每个 active grant 行展示 `File Access` 按钮：

- 点击某个 grant 行的 `File Access` 只能配置该行对应的 `grant_id`，Grant ID 在弹窗中只读展示，不提供手动输入或跨 grant 选择，避免把不存在的 grant 写入 `file-access.toml`。
- 每个 active grant 预期绑定一个 exact `grant_id` 策略。首次打开时如果没有现存策略，编辑器基于当前 grant 的 `file_access` 生成默认草稿；保存后写入 `[[grant]] grant_id = "<active grant id>"`。
- 当一个 grant 被删除 / revoke / 本地过期清理时，绑定该 exact `grant_id` 的 file access entry 必须同步删除，避免 `file-access.toml` 留下幽灵配置。只清理 `match.grant_id` / legacy flat `grant_id`，不清理 `match.ssh_fingerprint` 或 `match.caller_fingerprint` 默认策略。
- 配置过程优先暴露四个核心选择：
  - Grant：当前行 active grant，只读。
  - 类型：只读 / 读写，对应 `ops = FILE_READ_OPS` / `ops = ALL_FILE_OPS`。
  - 目录范围：指定目录 / 所有目录。
  - 指定目录：当目录范围为指定目录时填写，每行一个绝对路径；所有目录保存为 `roots = ["/"]`。
- 进阶字段（deny patterns、字节限制、覆盖、递归删除、gitignore）仍保留在同一个 per-grant 弹窗内编辑，但不再通过全局列表批量增删策略。

### SSH Key 默认 File Policy

SSH Key 连接没有 pair-code 授权弹窗，因此需要在 Settings → Remote Invoke → SSH Key 卡片上直接暴露默认 File Policy：

- 活跃 SSH Key 展示当前 `match.ssh_fingerprint` 策略状态：未配置 / read-only / read-write，以及 roots 数量。
- 点击 Configure 后可设置访问级别（read-only/read-write）、目录范围（指定 roots/所有目录）、是否允许覆盖、是否允许递归删除。
- 保存时写入 `file-access.toml` 的 `[[grant]] match.ssh_fingerprint = "<current key fingerprint>"`，这样通过该 SSH Key 新建的 grant 会自动命中同一文件策略，不再要求用户手动补某个短 grant_id。
- Reset Key 会生成新的 SSH fingerprint。为了不让用户配置回退到 `$HOME + 全部 ops`，reset 时必须把旧 fingerprint 的 file policy 迁移到新 fingerprint；只有旧 fingerprint 没有显式策略时才重新 seed 默认策略。
- 如果用户误删了当前 SSH Key 的 fingerprint 策略，`GET/PUT /remote-invoke/file-access-config` 必须自动恢复默认 fingerprint 策略并写回 `file-access.toml`，避免 UI 刷新或保存后进入无法恢复的无 file policy 状态。
- Grants 列表行级 `File Access` 入口面向 per-grant 策略；SSH Key 默认策略作为 fingerprint 策略保留展示，但不要求在 per-grant 编辑器中绑定 active grant。
- Grants 行级 `File Access` 编辑器首次打开时必须按执行侧 policy resolver 的同一优先级预填有效策略：`grant_id` exact 策略优先，其次继承 `match.ssh_fingerprint`，再其次继承 `match.caller_fingerprint`。因此 SSH Key 默认策略配置为 `roots = ["/"]` 时，通过该 key 连接出的 active grant 即使还没有 exact `grant_id` 策略，弹窗也必须显示 `Directories = All`，不能回退成空 `Selected`。用户点击保存后再将当前有效值落成该 grant 的 exact 策略。
- 兼容旧 grant：早期 SSH key grant 可能把 `ssh_key_fingerprint` 错存成 caller fingerprint，导致执行端 file.write 注入的 `command.ssh_fingerprint` 无法命中当前 `match.ssh_fingerprint` 默认策略，最终退回 cwd readonly fallback。`call_open` 校验通过后必须识别 `auth_method=ssh_publickey` 且 `ssh_key_fingerprint` 缺失/等于 `caller_fingerprint` 的旧记录，用当前 active SSH key fingerprint 修复并持久化，再执行 file policy 解析。

## 修改范围

### 后端 (Rust)
- `types.rs`: 新增 `FileAccessScope` 枚举，移除 `RemoteFileRead`/`RemoteFileWrite`，添加 `file_access` 到 `GrantInfo`/`GrantDecisionRequest`/`UpdateGrantRequest`/`GrantCreated`
- `worker.rs`: `ShellGrantProvision` 增加 `file_access`，权限检查使用 `scope_allows_command()`
- `handlers/remote_invoke.rs`: `ApproveBody`/`UpdateGrantBody` 增加 `file_access`
- `grant_policy_store.rs`: `StoredGrantPolicy` 增加 `file_access`

### WebUI
- `api/remoteInvoke.ts`: 新增 `FileAccessScope` 类型
- `PairingRequestModal`: 预设策略模式（Full Access / Shell Only / File Only / Query Only / Custom）
- `RemoteInvokeTab`: Grant Editor 同样使用预设策略模式，grant 列表显示 file access Tag

### CLI
- `remote grant update` 新增 `--file-access` 参数（`none`/`read`/`read_write`）
- 移除旧的 `--scope remote_file_read/remote_file_write` 选项

### Relay (TypeScript)
- 两个实现（本地测试 + 生产）均更新：
  - 移除 `remote_file_read`/`remote_file_write` scope 值
  - 新增 `normalizeFileAccess()` 函数
  - `grantScopeAllowsCommand()` 增加 `fileAccess` 参数
  - grant 创建/更新/SSE 事件中传递 `file_access`

## 向后兼容

不兼容旧数据。旧 grant 如果存储了 `remote_file_read`/`remote_file_write` scope，将被 Relay 的 `normalizeGrantScope` 拒绝。这符合项目约定：协议更新时直接删除旧数据库重建。
