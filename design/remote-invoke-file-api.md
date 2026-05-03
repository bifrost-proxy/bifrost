# Remote Invoke File API 设计方案

> **状态**：已实现（Phase 1/2/3 核心已合入 `feat/remote-file-api`）
> **最后对齐**：`a7a5115b` — 2026-04-26

## 背景与目标

当前 `bifrost remote` 已经覆盖两类能力：

1. 只读查询类（`query.readonly`）：`status`、`search.stream`、`traffic.list`、`traffic.get`。
2. 远程设备控制类（`shell.exec`）：在 Shell Access policy 允许下执行命令。

在 coding agent 场景中，仅靠 `shell.exec` 拼 `cat`/`sed`/`awk` 去读写文件存在以下痛点：

- 大段文本/非 ASCII 内容通过 `shell_text` 注入会遇到引号、换行、编码转义问题。
- 批量 `apply patch` 依赖 `patch`/`sed`，各平台行为差异大。
- 无法做大文件分块读取、SHA 校验、原子写入。
- shell 语义无法区分"读/写/编辑/列目录/搜索"等原语，可观测性和审计粒度粗。
- 每一次操作都要手写一次性脚本，违背了 agent "工具化"抽象。

因此在 `query.readonly` 与 `shell.exec` 之间，新增 **File API** 作为第三类 remote 能力，提供 coding agent 所需的语义化文件操作原语，复用现有 relay + encrypted remote invoke 通道，沿用现有授权模型。

---

## 1. 能力矩阵

### 1.1 Phase 1 — 只读

| method | 语义 | 必需权限 |
|---|---|---|
| `file.read` | 读取文件（支持行号 offset/limit、base64 传输） | `file_access ≥ read` |
| `file.list` | 列出目录（支持 recursive、max_depth、gitignore 感知） | `file_access ≥ read` |
| `file.stat` | 查询元信息（kind/size/mtime/mode/symlink_target） | `file_access ≥ read` |
| `file.glob` | 路径通配匹配（gitignore 感知、truncated 标志） | `file_access ≥ read` |
| `file.search` | 内容检索（regex、case_insensitive、gitignore 感知） | `file_access ≥ read` |
| `file.hash` | 计算文件哈希（sha256） | `file_access ≥ read` |

### 1.2 Phase 2 — 写入

| method | 语义 | 必需权限 |
|---|---|---|
| `file.write` | 整文件覆盖写（原子 tmp+rename、base_sha256 乐观锁、create_parents） | `file_access = read_write` |
| `file.edit` | 行号范围编辑（EditRange[]、base_sha256 乐观锁） | `file_access = read_write` |
| `file.mkdir` | 创建目录（支持 parents） | `file_access = read_write` |
| `file.move` | 重命名/移动 | `file_access = read_write` |
| `file.delete` | 删除文件或目录（recursive 需 policy 显式开启） | `file_access = read_write` |

### 1.3 Phase 3 — Patch

| method | 语义 | 必需权限 |
|---|---|---|
| `file.apply_patch` | 应用 unified diff（多文件、两阶段 rename+rollback） | `file_access = read_write` |

### 1.4 未实现 / 显式推迟

| 能力 | 状态 |
|---|---|
| `file.watch` | 长连接推送文件变更 — 不在当前分支 |
| `file.chmod` / `file.chown` | 未实现 |
| `file.symlink` | 未实现 |
| `GIT binary patch` | apply_patch 遇到 binary diff 返回 `file.binary_patch_unsupported` |
| rename/copy-only diff | apply_patch 遇到 rename/copy 返回 `file.unsupported_diff`，提示用 `file.move` / `file.read + file.write` |

---

## 2. 授权模型

### 2.1 正交双轴模型

File 权限从 `GrantScope` 中独立出来，以 `file_access` 字段与 `grant_scope` 正交：

| 字段 | 作用 | 可选值 |
|---|---|---|
| `grant_scope` | Shell / query 访问级别 | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` |
| `file_access` | 文件访问级别 | `none`（默认）/ `read` / `read_write` |

两字段独立设置、独立检查。一个 grant 可以同时拥有 `remote_shell_interactive` + `file_access: read_write`。

### 2.2 类型定义（实现）

```rust
// crates/bifrost-admin/src/remote_invoke/types.rs

/// 3 种 shell 级别
pub enum GrantScope {
    RemoteQuery,
    RemoteShellExec,
    RemoteShellInteractive,
}

/// 独立的 file 级别
pub enum FileAccessScope {
    None,       // 默认
    Read,
    ReadWrite,
}

/// 统一的命令类别（3 变体，file.* 统一为 File）
pub enum CommandKind {
    QueryReadonly,   // "query.readonly"
    ShellExec,       // "shell.exec"
    File,            // "file" — 所有 file.* 方法
}

/// 组合检查
pub fn scope_allows_command(grant_scope, file_access, kind) -> bool {
    match kind {
        CommandKind::File => file_access ∈ {Read, ReadWrite},
        CommandKind::ShellExec => grant_scope ∈ {RemoteShellExec, RemoteShellInteractive},
        CommandKind::QueryReadonly => true,
    }
}
```

> **注意**：早期设计曾在 `GrantScope` 中包含 `RemoteFileRead` / `RemoteFileWrite` 变体，已移除。`CommandKind` 也不再拆为 12 个 per-method 变体（`FileRead`/`FileList`/…），统一为单个 `File`。具体读写权限由 `FileAccessScope` + `FileAccessPolicy.ops` 两层控制。

### 2.3 WebUI 交互预设

| 模式 | grant_scope | file_access |
|---|---|---|
| Query only | `remote_query` | `none` |
| Full Access | `remote_shell_interactive` | `read_write` |
| Custom | 用户选择 | 用户独立选择 |

---

## 3. File Access Policy

### 3.1 数据结构（实现）

```rust
// crates/bifrost-core/src/file_access/policy.rs

pub struct FileAccessPolicy {
    pub name: String,
    pub roots: Vec<PathBuf>,
    pub denies: Vec<String>,          // glob，读写均生效
    pub write_denies: Vec<String>,    // glob，仅写操作叠加
    pub ops: Vec<FileOp>,             // 允许的操作集合
    pub max_read_bytes: u64,          // 默认 8 MiB
    pub max_write_bytes: u64,         // 默认 2 MiB
    pub respect_gitignore: bool,      // 默认 true
    pub allow_overwrite: bool,        // 默认 true
    pub allow_recursive_delete: bool, // 默认 false
}
```

> **与早期设计差异**：移除了 `id` 字段（由 store 层管理）、`follow_symlinks` 字段（符号链接始终 resolve 到 canonical 路径后做 root 内检查）。新增了 `write_denies`、`max_write_bytes`、`allow_overwrite`、`allow_recursive_delete`。

### 3.2 FileOp 枚举

```rust
pub enum FileOp {
    Read, List, Stat, Glob, Search, Hash,       // Phase 1
    Write, Edit, Mkdir, Move, Delete,            // Phase 2
    ApplyPatch,                                   // Phase 3
}
```

### 3.3 Policy 检查流程

1. **路径归一化**：`canonicalize_within_roots(path, &roots)` — 拒绝 `..` 逃逸、拒绝绝对路径超出 roots。
2. **符号链接检查**：resolve 后的 canonical path 必须仍在 roots 内，否则 `file.symlink_escape`。
3. **Deny 匹配**：`DenyMatcher` 匹配 root-relative 路径；写操作额外叠加 `write_denies`。
4. **Op 检查**：请求 op 必须在 `policy.ops` 内。只读 policy 对写操作返回 `file.permission_denied`。
5. **Gitignore**：`respect_gitignore=true` 时，`file.list`/`file.glob`/`file.search` 走 `ignore` crate 过滤。
6. 通过后返回 `PolicyDecision`，包含 `path`(canonical)、`op`、`max_read_bytes`、`max_write_bytes`、`respect_gitignore`、`allow_overwrite`、`allow_recursive_delete`、`input_abs`（lstat 用原始绝对路径）。

---

## 4. 协议细节

### 4.1 通用请求包络

复用 `RemoteInvokeRequest`：

```jsonc
{
  "kind": "file",              // CommandKind::File
  "command": "file.read",      // 具体方法名，用于 executor dispatch
  "args_json": "{…}",          // 每个 method 独立 schema
  "grant_scope": "remote_shell_exec",
  "file_access": "read_write"
}
```

### 4.2 `file.read`

请求参数：

```jsonc
{
  "path": "crates/bifrost-core/src/lib.rs",
  "offset": 0,           // 行号偏移（1-based），默认 0 表示从头
  "limit": 200,          // 最大返回行数
  "allow_binary": false   // 是否允许二进制文件（base64 返回）
}
```

响应：

```jsonc
{
  "content_b64": "<base64>",     // 内容（始终 base64 编码传输）
  "size": 4096,                  // 返回内容字节数
  "total_size": 12288,           // 文件总字节数
  "truncated": false,            // 是否被截断
  "sha256": "abc…",              // 返回内容的 sha256
  "file_sha256": "abc…",        // 整个文件的 sha256（truncated=true 时与 sha256 不同）
  "mtime_unix": 1714000000,
  "total_lines": 120,            // 仅 UTF-8 文件时返回
  "start_line": 1,               // 实际返回的起始行号（1-based）
  "end_line": 120                // 实际返回的结束行号
}
```

> **注意**：传输格式始终为 `content_b64`（base64），不是纯文本 `content`。`file_sha256` 是整个文件的哈希（用于传给后续 `file.write` 的 `base_sha256`），当 `truncated=true` 时与 `sha256`（片段哈希）不同。

### 4.3 `file.list`

请求参数：

```jsonc
{
  "path": "crates/bifrost-core/src",
  "recursive": false,
  "max_depth": 3
}
```

响应：

```jsonc
{
  "entries": [
    {
      "name": "lib.rs",
      "kind": "file",            // "file" | "dir" | "symlink"
      "size": 4096,
      "mtime_unix": 1714000000,
      "mode": "0644",
      "symlink_target": null      // 仅 kind="symlink" 时非 null
    }
  ],
  "truncated": false,             // 条目数达上限时为 true
  "root": "crates/bifrost-core/src"
}
```

> **与早期设计差异**：`type` 字段实际为 `kind`；时间戳为 `mtime_unix`（epoch 秒）不是 ISO 8601；新增 `symlink_target`/`truncated`/`root` 字段。

### 4.4 `file.stat`

响应：

```jsonc
{
  "kind": "file",
  "size": 4096,
  "mtime_unix": 1714000000,
  "mode": "0644",
  "symlink_target": null,
  "sha256": "abc…"               // 仅 is_file && size ≤ max_read_bytes 时计算
}
```

### 4.5 `file.glob`

请求参数：

```jsonc
{
  "pattern": "crates/**/*.rs",
  "cwd": "<REPO_ROOT>",
  "limit": 1000
}
```

响应：

```jsonc
{
  "matches": ["crates/bifrost-core/src/lib.rs", "…"],
  "truncated": false,
  "root": "<REPO_ROOT>"
}
```

> **与早期设计差异**：字段名为 `matches`（不是 `paths`）。

### 4.6 `file.search`

请求参数：

```jsonc
{
  "pattern": "RemoteInvokeRequest",
  "path": "crates",
  "regex": true,
  "case_insensitive": false,      // 注意：字段名是 case_insensitive（不是 case_sensitive）
  "max_results": 200,
  "context_lines": 2
}
```

响应：

```jsonc
{
  "matches": [
    {
      "file": "crates/bifrost-admin/src/remote_invoke/types.rs",
      "line": 120,
      "text": "pub struct RemoteInvokeRequest {"
    }
  ],
  "truncated": false,
  "root": "<REPO_ROOT>"
}
```

> **与早期设计差异**：参数名为 `case_insensitive`（不是 `case_sensitive`）；不返回 `col`/`context_before`/`context_after`/`scanned_files`。handler 内部遵循 `policy.denies`。

### 4.7 `file.hash`

```jsonc
// 响应
{ "algo": "sha256", "sha256": "abc…" }
```

### 4.8 `file.write`

请求参数：

```jsonc
{
  "path": "src/new_file.rs",
  "content_b64": "<base64>",
  "base_sha256": "abc…",          // 空/null = 期望新建
  "create_parents": false,
  "allow_overwrite": true          // 可覆盖 policy 默认值
}
```

响应：

```jsonc
{
  "path": "src/new_file.rs",
  "bytes_written": 1024,
  "sha256": "def…",               // 新文件哈希
  "previous_sha256": "abc…"       // 旧文件哈希；新建时为 null
}
```

实现细节：
- 写入流程：`decode b64 → 校验 base_sha256 → create_parents(可选) → write(tmp) → chmod(保留原 mode) → rename(tmp, target)`。
- `create_parents=true` 时自动创建不存在的父目录。

### 4.9 `file.edit`

请求参数：

```jsonc
{
  "path": "src/main.rs",
  "base_sha256": "abc…",
  "edits": [
    { "start_line": 10, "end_line": 15, "replacement": "// new content\n" }
  ]
}
```

`EditRange` 的 `start_line` / `end_line` 为 1-based、inclusive。

响应同 `file.write`：`{ path, bytes_written, sha256, previous_sha256 }`。

实现细节：
- EOL 保留：自动侦测源文件行尾风格（CRLF/LF），替换文本输出时保持一致。
- 最后一行尾换行保留：如果原文件末尾有换行、替换内容末尾无换行，自动补上（反之亦然）。

### 4.10 `file.mkdir`

```jsonc
{ "path": "src/new_dir", "parents": true }
// 响应：{ "path": "src/new_dir", "created": true }
```

### 4.11 `file.move`

```jsonc
{ "from": "src/old.rs", "to": "src/new.rs" }
// 响应：{ "from": "…", "to": "…" }
```

> **已知缺失**：当前不支持 `base_sha256` 前置校验。多 agent 场景可能丢写。计划下一 PR 补齐。

### 4.12 `file.delete`

```jsonc
{ "path": "src/old.rs", "recursive": false }
// 响应：{ "path": "…", "deleted": true, "recursive": false }
```

`recursive=true` 必须在 policy 中 `allow_recursive_delete=true` 才能生效。

> **已知缺失**：当前不支持 `if_match_sha256` 前置校验。计划下一 PR 补齐。

### 4.13 `file.apply_patch`

请求参数：

```jsonc
{
  "patch": "<unified diff text>"
}
```

实现细节：

1. **解析**：`parse_patch()` 将 diff 文本拆为 `Vec<PatchEntry>`。每个 entry 包含 `old_path`、`new_path`、`body`、`kind`。
2. **PatchKind 分类**（按优先级）：
   - `Binary { path }` → 拒绝，返回 `file.binary_patch_unsupported`
   - `RenameOnly { from, to }` → 拒绝，返回 `file.unsupported_diff`（提示用 `file.move`）
   - `CopyOnly { from, to }` → 拒绝，返回 `file.unsupported_diff`（提示用 `file.read + file.write`）
   - `ModeOnly { path }` → 拒绝，返回 `file.unsupported_diff`
   - `Modify` / `Create` / `Delete` → 正常处理
3. **权限检查**：逐文件走 `FileAccessPolicy::check(path, cwd, FileOp::ApplyPatch)`。
4. **两阶段原子提交**：
   - Stage 阶段：对每个文件写 `<parent>/.bifrost-patch.<pid>.<nanos>.<i>.tmp`，逐 hunk 校验 context。
   - 快照 `prior_mode` + `existed_before` 用于 rollback。
   - Commit 阶段：所有 tmp 校验通过后，逐个 `rename(tmp, target)` + `chmod`。
   - 任一 rename 失败 → rollback 已 committed 的文件（用快照恢复原内容/删除新建文件）。
5. **EOL 处理**：侦测目标文件 EOL 风格，hunk 输出保持一致。`\ No newline at end of file` 标记正确处理（不再强补尾换行）。
6. **diff --git 扩展头**：正确解析 `diff --git a/… b/…`、`index`、`similarity index`、`rename from/to`、`copy from/to`、`old mode`/`new mode`、`new file mode`、`deleted file mode`、`Binary files … differ`。

---

## 5. 错误码

### 5.1 完整错误码表（当前实现）

| 错误码 | 来源 | 触发场景 |
|---|---|---|
| `file.not_found` | policy / handler | 路径不存在 |
| `file.permission_denied` | policy / handler | deny 匹配 / 只读 policy 拒绝写 |
| `file.out_of_scope` | policy | 路径超出 roots |
| `file.symlink_escape` | policy | 符号链接解析后超出 roots |
| `file.ignored_by_gitignore` | policy | respect_gitignore 命中 |
| `file.binary_not_allowed` | policy | 非 UTF-8 文件 + allow_binary=false |
| `file.op_not_permitted` | policy | 请求的 op 不在 policy.ops 中 |
| `file.invalid_args` | handler | 参数缺失/非法 |
| `file.invalid_glob` | policy / handler | glob 语法错误 |
| `file.invalid_regex` | handler | 正则语法错误 |
| `file.invalid_deny` | policy | deny 模式语法错误 |
| `file.io_error` | handler | 底层 IO 错误 |
| `file.precondition_failed` | handler | 通用前置条件不满足（目标已存在/不存在等） |
| `file.sha_mismatch` | handler | `base_sha256` 校验不通过 |
| `file.size_too_large` | handler | 超过 max_read_bytes / max_write_bytes |
| `file.unsupported_algo` | handler | hash 算法不支持 |
| `file.unsupported_diff` | handler | apply_patch 遇到 rename/copy/mode-only diff |
| `file.binary_patch_unsupported` | handler | apply_patch 遇到 binary diff |

> **与早期设计差异**：
> - `file.too_large` → 实际为 `file.size_too_large`
> - `file.invalid_argument` → 实际为 `file.invalid_args`
> - `file.scope_required` → 由 `scope_allows_command()` 在 executor 层拒绝，不经 file handler
> - `file.invalid_encoding` → 由 `file.binary_not_allowed` 替代
> - `file.patch_rejected` / `file.partial_applied` → 未采用；apply_patch 失败统一走 `file.precondition_failed` + rollback
> - `file.exists` / `file.not_empty` / `file.write_quota_exceeded` / `patch.*` → 未实现，归入 `file.precondition_failed`

---

## 6. 架构分层

```
┌──────────────┐  request (kind=file, command=file.read)  ┌────────────┐
│ Caller (CLI) │ ─────────────────────────────────────▶   │   Relay    │
└──────────────┘                                          └─────┬──────┘
                                                                ▼
                                                      ┌─────────────────┐
                                                      │ bifrost-admin    │
                                                      │  executor.rs     │
                                                      └─────────┬───────┘
                                                                ▼
                                                      ┌─────────────────┐
                                                      │ FileAccessPolicy │
                                                      │  policy.rs       │
                                                      │  (bifrost-core)  │
                                                      └─────────┬───────┘
                                                                ▼
                                                      ┌─────────────────┐
                                                      │ file_ops.rs      │
                                                      │ (~3252 行)       │
                                                      └─────────────────┘
```

文件结构：

| 路径 | 职责 |
|---|---|
| `crates/bifrost-core/src/file_access/policy.rs` | `FileAccessPolicy`、`PolicyDecision`、`FileOp` |
| `crates/bifrost-core/src/file_access/matcher.rs` | `DenyMatcher`、`GlobMatcher` |
| `crates/bifrost-core/src/file_access/path.rs` | `CanonicalPath`、`canonicalize_within_roots` |
| `crates/bifrost-core/src/file_access/error.rs` | `FileAccessError` 枚举 |
| `crates/bifrost-admin/src/remote_invoke/executor.rs` | 请求分发、权限检查、apply_patch 解析委托 |
| `crates/bifrost-admin/src/remote_invoke/file_ops.rs` | 所有 handler 实现、`parse_patch`/`PatchEntry`/`PatchKind` |
| `crates/bifrost-admin/src/remote_invoke/file_policy_store.rs` | Policy 存储（TOML load） |

---

## 7. CLI 映射

```
bifrost remote file read        <path> [--max-bytes N] [--allow-binary]
bifrost remote file list        [path] [--depth N]
bifrost remote file stat        <path>
bifrost remote file glob        <pattern> [--max-matches N]
bifrost remote file search      <regex> [--path P] [--max-matches N] [--max-scan N]
bifrost remote file hash        <path> [--algo sha256]
bifrost remote file write       <path> [--content-file <local-path|->] [--base-sha256 SHA]
bifrost remote file edit        <path> --edits JSON [--base-sha256 SHA]
bifrost remote file mkdir       <path> [--parents]
bifrost remote file mv          <from> <to>
bifrost remote file rm          <path> [--recursive]
bifrost remote file apply-patch --patch-file <local-patch|->
```

---

## 8. 安全设计

| 机制 | 状态 | 说明 |
|---|---|---|
| 路径归一化 + roots 白名单 | ✅ 已实现 | `canonicalize_within_roots` 拒绝 `..` 逃逸 |
| 符号链接 resolve 检查 | ✅ 已实现 | resolve 后超出 roots → `file.symlink_escape` |
| deny / write_denies glob | ✅ 已实现 | `DenyMatcher` |
| 二进制保护 | ✅ 已实现 | `file.binary_not_allowed` |
| 大小保护 | ✅ 已实现 | `max_read_bytes` / `max_write_bytes` |
| gitignore 感知 | ✅ 已实现 | `ignore` crate 接入 |
| 文件 mode 保留 | ✅ 已实现 | 写前快照 mode → 写后 chmod |
| CRLF/EOL 归一 | ✅ 已实现 | 侦测 + 保持 |
| 硬链接 inode 对照 | ❌ 未实现 | 需跨平台方案，显式推迟 |
| 审计 tracing | ❌ 未实现 | handler 当前零 tracing；计划补齐 |
| policy store 缓存 | ❌ 未实现 | 每请求 `load_default()` 读盘 |

---

## 9. 已知遗留与下一步

| ID | 项 | 优先级 | 说明 |
|---|---|---|---|
| R-1 | `file.move` 加 `base_sha256` | P1 | 防多 agent race |
| R-2 | `file.delete` 加 `if_match_sha256` | P1 | 防误删 |
| R-3 | `FileAccessPolicyStore` 缓存 | P1 | `OnceLock<RwLock>` + mtime 失效 |
| R-4 | handler tracing + audit | P2 | 每个 handler 入口 `tracing::info!(target="audit.file", …)` |
| R-5 | 硬链接 inode 对照 | P2 | 跨平台，可延后 |
| R-6 | 默认 exclude 扩展 | P2 | `.venv/dist/build/.next/.nuxt/.pytest_cache` 等 |
| R-7 | `file.search` 不合并同文件命中 | L | 100 hits 分 5 file 时 agent 需自行 group-by |
| R-8 | `file.glob` 不返回 mtime | L | 查"近期修改"需额外 stat |

---

## 10. 与现有能力的分工原则

| 需求 | 推荐路径 | 说明 |
|---|---|---|
| 读/改单个文件 | `file.*` | 编码/换行/原子性保障 |
| 批量 apply diff | `file.apply_patch` | 多文件两阶段原子 |
| 目录遍历、内容检索 | `file.list` / `file.search` | 结构化返回 |
| 运行 `cargo build` | `shell.exec` | 触发进程 |
| 运行 `git` 命令 | `shell.exec` | 属于工作流 |

---

## 参考

- GitHub Contents API — `If-Match-SHA` 乐观锁
- Claude Code / Codex / Cursor 的 file_read / file_edit / grep / glob / apply_patch 工具
- 现有 `crates/bifrost-admin/src/remote_invoke/` 的 executor/worker/types 分层
