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
| `file.read_many` | 单次往返批量读取多个文件；单文件失败不致命，作为 per-item error 返回 | `file_access ≥ read` + policy.ops 含 `read_many` |
| `file.list` | 列出目录（支持 recursive、max_depth、gitignore 感知） | `file_access ≥ read` |
| `file.stat` | 查询元信息（kind/size/mtime/mode/symlink_target） | `file_access ≥ read` |
| `file.glob` | 路径通配匹配（gitignore 感知、truncated 标志） | `file_access ≥ read` |
| `file.search` | 内容检索（regex、case_insensitive、gitignore 感知） | `file_access ≥ read` |
| `file.hash` | 计算文件哈希（sha256） | `file_access ≥ read` |
| `file.outline` | 解析源文件输出顶层符号大纲（fn/struct/class 等） | `file_access ≥ read` + policy.ops 含 `outline` |

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
| `grant_scope` | Shell / query 访问级别 | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` / `remote_power_mgmt` / `remote_im_gateway` |
| `file_access` | 文件访问级别 | `none`（默认）/ `read` / `read_write` |

两字段独立设置、独立检查。一个 grant 可以同时拥有 `remote_shell_interactive` + `file_access: read_write`。

### 2.2 类型定义（实现）

```rust
// crates/bifrost-admin/src/remote_invoke/types.rs

/// 5 种级别
pub enum GrantScope {
    RemoteQuery,
    RemoteShellExec,
    RemoteShellInteractive,
    RemotePowerMgmt,
    RemoteImGateway,
}

/// 独立的 file 级别
pub enum FileAccessScope {
    None,       // 默认
    Read,
    ReadWrite,
}

/// 统一的命令类别（5 变体，file.* 统一为 File）
pub enum CommandKind {
    QueryReadonly,   // "query.readonly"
    ShellExec,       // "shell.exec"
    File,            // "file" — 所有 file.* 方法
    PowerMgmt,       // "power.mgmt"
    ImGateway,       // "im.gateway"
}

/// 组合检查
pub fn scope_allows_command(grant_scope, file_access, kind) -> bool {
    match kind {
        CommandKind::File => file_access ∈ {Read, ReadWrite},
        CommandKind::ShellExec => grant_scope ∈ {RemoteShellExec, RemoteShellInteractive},
        CommandKind::PowerMgmt => grant_scope == RemotePowerMgmt,
        CommandKind::ImGateway => grant_scope == RemoteImGateway,
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
    pub max_read_bytes: u64,          // 默认 2 MiB
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
    Read, ReadMany, List, Stat, Glob, Search, Hash, Outline,  // Phase 1
    Write, Edit, Mkdir, Move, Delete,                          // Phase 2
    ApplyPatch,                                                // Phase 3
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

支持两种 edit 模式，每次调用必须**全部 anchored 或全部 line-range**，混用返回 `file.invalid_args`：

**Anchored 模式（推荐，content-based）**

```jsonc
{
  "path": "src/main.rs",
  "base_sha256": "abc…",
  "edits": [
    { "old_string": "fn foo()", "new_string": "fn bar()", "expected_count": 1 }
  ]
}
```

- `old_string`：被替换的字面文本（不能为空，否则 `file.invalid_args`）。EOL 自动归一到源文件风格，所以 LF/CRLF 不会引起 anchor miss。
- `new_string`：替换文本（同样 EOL 归一）。
- `expected_count`：期望命中次数，默认 1。命中数不等于此值返回 `file.anchor_not_unique`；完全不命中返回 `file.anchor_not_found`。
- 多个 anchored item 在同一份 buffer 上按顺序串行 apply。

**Line-range 模式**

```jsonc
{
  "path": "src/main.rs",
  "base_sha256": "abc…",
  "edits": [
    { "start_line": 10, "end_line": 15, "replacement": "// new content\n" }
  ]
}
```

`start_line` / `end_line` 为 1-based、inclusive。

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
{
  "from": "src/old.rs",
  "to": "src/new.rs",
  "base_sha256": "abc…",       // 可选，对源 regular file 做乐观锁校验
  "allow_overwrite": false       // 可选，覆盖 policy 默认值
}
// 响应：{ "from": "…", "to": "…" }
```

实现细节：
- `base_sha256` 不匹配 → `file.sha_mismatch`，源文件保留不动。
- `allow_overwrite=false` 且目标已存在 → `file.precondition_failed`。
- 对 regular file + 目标不存在的常见路径，采用 create-if-absent 链接 + 删源的原子序列，避免 check-then-rename 的 TOCTOU race。
- 目录仍走平台 rename 语义（目录不可 hard-link）。
- 源与目标两侧都会走 `FileOp::Move` 的 policy 校验。

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
| `file.already_exists` | handler | 目标已存在（mkdir 无 parents、move 目标已存在等），由 `io::ErrorKind::AlreadyExists` 映射 |
| `file.cross_device` | handler | move 时源与目标跨设备，无法 atomic rename，由 EXDEV 映射 |
| `file.is_a_directory` | handler | 期望文件却拿到目录（如 read/edit 作用于目录） |
| `file.not_a_directory` | handler | 期望目录的路径分量是文件 |
| `file.permission_denied` | policy / handler | 只读 policy 拒绝写、recursive delete 被关闭等 |
| `file.deny_pattern` | policy | 路径命中 `denies` / `write_denies` glob（取代早期文档里使用的「permission_denied for deny」） |
| `file.out_of_scope` | policy | 路径超出 roots；CLI hint 会提示 `/tmp -> /private/tmp` 与 `remote file scratch-dir` fallback |
| `file.symlink_escape` | policy | 符号链接解析后超出 roots |
| `file.ignored_by_gitignore` | policy | respect_gitignore 命中 |
| `file.binary_not_allowed` | policy | 非 UTF-8 文件 + allow_binary=false |
| `file.op_not_permitted` | policy | 请求的 op 不在 policy.ops 中；`read_many` 单独可被关闭，CLI hint 提示降级为逐个 `remote file read` |
| `file.anchor_not_found` | handler (edit) | anchored edit 的 `old_string` 在目标文件中找不到 |
| `file.anchor_not_unique` | handler (edit) | anchored edit 的 `old_string` 命中次数 ≠ `expected_count` |
| `file.invalid_args` | handler | 参数缺失/非法、anchored 与 line-range 混用、`old_string` 为空等 |
| `file.invalid_glob` | policy / handler | glob 语法错误 |
| `file.invalid_regex` | handler | 正则语法错误 |
| `file.invalid_deny` | policy | deny 模式语法错误 |
| `file.io_error` | handler | 底层 IO 错误（未匹配上面任何特化 ErrorKind 时的兜底） |
| `file.precondition_failed` | handler | 通用前置条件不满足（apply_patch context 不匹配、`allow_overwrite=false` 时目标已存在等） |
| `file.sha_mismatch` | handler | `base_sha256` 校验不通过（write/edit/move 通用） |
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

CLI 当前导出 15 个子命令（`crates/bifrost-cli/src/cli/remote.rs::RemoteFileCommands`），hermetic CLI contract 测试覆盖全集：

```
bifrost remote file read        <path> [--offset N] [--limit N] [--max-bytes N] [--allow-binary] [--cwd DIR] [--output human|json]
bifrost remote file read-many   --path A --path B ... [--max-bytes N] [--allow-binary] [--cwd DIR] [--output human|json]
bifrost remote file scratch-dir [--cwd DIR] [--name .bifrost-tmp] [--output human|json]
bifrost remote file list        [path] [--depth N] [--max-matches N] [--cursor TOK] [--no-ignore] [--exclude NAME]... [--cwd DIR]
bifrost remote file stat        <path> [--cwd DIR]
bifrost remote file glob        <pattern> [--max-matches N] [--cursor TOK] [--no-ignore] [--exclude NAME]... [--cwd DIR]
bifrost remote file find        [pattern] [-e REGEX]... [--path P] [-i] [-F] [-w] [--glob G]
                                 [-A N] [-B N] [-C N] [--max-matches N] [--max-scan N] [--cursor TOK]
                                 [--no-ignore] [--exclude NAME]... [--cwd DIR]   # alias: search
bifrost remote file hash        <path> [--algo sha256] [--cwd DIR]
bifrost remote file outline     <path> [--max-symbols N] [--max-bytes N] [--cwd DIR]
bifrost remote file write       <path> [--content STR | --content-file <local|-> | --content-b64 B64]
                                 [--base-sha256 SHA] [--allow-overwrite BOOL] [--create-parents] [--cwd DIR]
bifrost remote file edit        <path> --edits JSON [--base-sha256 SHA] [--cwd DIR]
bifrost remote file mkdir       <path> [--parents] [--cwd DIR]
bifrost remote file move        <from> <to> [--base-sha256 SHA] [--allow-overwrite BOOL] [--cwd DIR]   # alias: mv
bifrost remote file delete      <path> [--recursive] [--cwd DIR]                                      # alias: rm
bifrost remote file patch       [--patch-file <local|-> | --patch-b64 B64] [--base-sha PATH=SHA]... [--cwd DIR]   # alias: apply-patch
```

注意：
- 内容检索子命令真名为 `find`（`search` 是 visible alias），同时支持位置正则与可重复 `-e/--regex` 多模式 OR 组合，并提供 `-A/-B/-C` 上下文与 `-F/-w/--glob/--no-ignore/--exclude` 控制；服务端方法名保持 `file.search`。
- `move` / `delete` / `patch` 是真名，`mv` / `rm` / `apply-patch` 为 visible alias，保持向后兼容。
- `read` / `list` / `glob` / `find` 等读路径均支持 `--cursor`/`--max-matches` 分页与 `--no-ignore`/`--exclude` 控制；`write` 接受 `--content`/`--content-file`/`--content-b64`/stdin 四种内容输入。
- `move` 暴露 `--base-sha256` 与 `--allow-overwrite`，覆盖了早期文档中 "file.move 未实现 sha256 前置校验" 的限制。

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
| R-1 | ~~`file.move` 加 `base_sha256`~~ | done | 已实现：`--base-sha256` + `--allow-overwrite`，详见 §4.11 与底部 hardening |
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
- Claude Code / Cursor 的 file_read / file_edit / grep / glob / apply_patch 工具
- 现有 `crates/bifrost-admin/src/remote_invoke/` 的 executor/worker/types 分层


## P0 hardening follow-ups: read_many, move, and CLI contract

### file.read_many authorization model

`file.read_many` now has two explicit authorization layers:

1. **Request-level capability**: the effective `FileAccessPolicy.ops` must include `read_many`. This lets an administrator allow normal single-file reads while disabling high-fanout batch reads.
2. **Per-file content access**: every requested path is still checked with `read` before content is returned. A denied or missing item is represented as an item-level error in the batch response after the request-level `read_many` gate has passed.

This preserves the coding-agent ergonomics of partial batch success while making `ReadMany` a meaningful policy capability instead of only an enum/documentation alias.

### file.move safety contract

`file.move` exposes the same optimistic-lock and overwrite controls expected from top-level coding-agent file mutations:

- `--base-sha256 <SHA>` verifies the source regular file before moving it. A mismatch returns `file.sha_mismatch` and leaves the source in place.
- `--allow-overwrite <bool>` overrides the policy default for this call. When false and the destination exists, the operation returns `file.precondition_failed`.
- For regular files whose destination does not exist at validation time, the implementation uses an atomic create-if-absent link step before removing the source. This avoids the classic destination check-then-rename race for the common no-overwrite path.

Directories still use platform rename semantics because directories cannot be hard-linked. The operation remains policy-confined at both source and destination through `FileOp::Move` decisions.

### CLI contract coverage

The hermetic CLI contract test now treats the remote file surface as fifteen subcommands and verifies `read-many`, `scratch-dir`, `outline`, and the `move` safety flags in addition to the older read/write/search/patch commands.

## 2026-06-16 coding-agent UX hardening

### scratch-dir and policy-deny fallbacks

真实远端使用中，临时脚本写入 `/tmp` 会因为 macOS `/tmp -> /private/tmp` symlink 或 policy roots 不包含临时目录而失败。新增 CLI 入口：

```bash
bifrost remote file scratch-dir --cwd <repo>
```

该命令在授权 cwd 下创建 `.bifrost-tmp`，底层仍走 `file.mkdir` 与 FileAccessPolicy 写权限校验，不绕过目标端授权边界。后续临时脚本可写入该目录并由调用方清理。

### read/read-many/concurrency visibility

- `file.out_of_scope` hint 明确提示 `/tmp`/`/private/tmp` 与 `scratch-dir` fallback。
- `file.op_not_permitted` hint 明确提示 `read-many` 可被 policy 独立禁用，并建议降级为逐个 `remote file read`。
- human `file.read` footer 继续输出 sha256，同时新增 `mtime_unix`，帮助多 agent 在 read/edit 之间发现文件近期变化。

验证计划：

- `cargo test -p bifrost-cli remote::tests::test_build_remote_file_scratch_dir_uses_policy_checked_mkdir --lib`
- `cargo test -p bifrost-cli remote:: --lib --no-run`
