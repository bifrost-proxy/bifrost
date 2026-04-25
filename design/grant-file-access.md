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
| `grant_scope` | 控制 shell 访问级别 | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` |
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
