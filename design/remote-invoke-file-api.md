# Remote Invoke File API 设计方案

> **状态**：Phase 1 / 2 / 3 已实现并合入 `feat/remote-file-api`；P0 hardening 与 coding-agent UX 增强（scratch-dir、read_many gate、outline、SSH-key Full Trust 兼容）已上线。
> **最后对齐**：2026-06-16。

## 背景

Bifrost `remote` 已经覆盖两类能力：

1. **只读查询** (`query.readonly`)：`status`、`search.stream`、`traffic.list`、`traffic.get`；
2. **Shell 控制** (`shell.exec`)：在 Shell Access policy 下执行命令。

Agent 场景中，仅靠 `shell.exec` 拼 `cat/sed/awk` 去读写文件存在以下痛点：

- 大段文本 / 非 ASCII 内容通过 `shell_text` 注入会遇到引号、换行、编码转义问题；
- 批量 `apply patch` 依赖 `patch/sed`，各平台行为差异大；
- 无法做大文件分块读取、SHA 校验、原子写入；
- shell 语义无法区分"读/写/编辑/列目录/搜索"等原语，可观测性和审计粒度粗；
- 每一次操作都要手写一次性脚本，违背 agent 工具化抽象。

因此在 `query.readonly` 与 `shell.exec` 之间新增 **File API** 作为第三类 remote 能力，提供 coding agent 所需的语义化文件操作原语，复用现有 relay + encrypted remote invoke 通道，沿用现有 grant 授权模型，并与 `FileAccessScope` / `FileAccessPolicy` 双轴权限系统一致。

## 用户目标验证清单

### 必须实现

- 支持只读（read / read_many / list / stat / glob / search / hash / outline）与写（write / edit / mkdir / move / delete / apply_patch）两类共 14 个 method。
- 复用 `RemoteInvokeRequest` 包络，`CommandKind::File` + `command="file.<method>"` 分发。
- 独立 `FileAccessScope` 与 `GrantScope` 正交：一个 grant 可同时是 `remote_shell_interactive` + `file_access=read_write`。
- `FileAccessPolicy` 支持 roots 白名单、denies / write_denies glob、max_read_bytes / max_write_bytes、respect_gitignore、allow_overwrite、allow_recursive_delete、ops 白名单。
- 路径 canonicalize 到 roots 内部，`..` 逃逸与符号链接逃逸都被拒绝。
- `file.write` / `file.edit` / `file.move` 支持 `base_sha256` 乐观锁与 `allow_overwrite` per-call override。
- `file.edit` 支持 anchored（content-based）与 line-range 两种模式，每次调用必须全部同一模式。
- `file.apply_patch` 支持多文件 unified diff，两阶段 stage+commit + 失败 rollback。
- CLI 暴露 15 个子命令（含 `read-many` / `scratch-dir` / `outline` / `move --base-sha256`）。
- SSH-key Full Trust 授权路径在新 `FileOp` 引入后仍能自动补齐 ops（narrow 兼容规则）。
- `file.read_many` 走两级授权：request-level `policy.ops` 需含 `read_many`；per-file 仍走 `read` 权限检查。

### 必须不破坏

- 现有 `query.readonly` / `shell.exec` 路径完全不受影响。
- 已有 grant 授权流程（pairing、QR、reusable grant）不改动。
- Relay 只是端到端加密通道，不理解 file API 内容。
- 现有的 SSH-key 授权 policy（历史落地的 ops 列表）在 Full Trust 场景下自动获得后来新增的 ops，不需要重新配对。

### 必须真实验证

- CLI 15 个子命令的 hermetic 契约测试全通过。
- E2E：`test_remote_file_api_e2e.sh`、`test_remote_file_relay_e2e.sh`。
- 单元测试覆盖 policy check、edit 模式互斥、apply_patch 两阶段 rollback、`scratch-dir` policy 校验。

## 产品语义

### 能力矩阵

#### Phase 1 — 只读

| method | 语义 | 必需权限 |
|---|---|---|
| `file.read` | 读取文件（行号 offset/limit、base64 传输、`file_sha256` 全文件哈希） | `file_access ≥ read` |
| `file.read_many` | 单次往返批量读；单文件失败不致命，作为 per-item error 返回 | `file_access ≥ read` + `policy.ops` 含 `read_many` |
| `file.list` | 列目录（recursive、max_depth、gitignore 感知） | `file_access ≥ read` |
| `file.stat` | 元信息（kind / size / mtime / mode / symlink_target） | `file_access ≥ read` |
| `file.glob` | 路径通配（gitignore 感知、truncated 标志） | `file_access ≥ read` |
| `file.search` | 内容检索（regex、case_insensitive、gitignore 感知） | `file_access ≥ read` |
| `file.hash` | sha256 | `file_access ≥ read` |
| `file.outline` | 解析源文件输出顶层符号大纲（fn/struct/class 等） | `file_access ≥ read` + `policy.ops` 含 `outline` |

#### Phase 2 — 写入

| method | 语义 | 必需权限 |
|---|---|---|
| `file.write` | 整文件覆盖写（tmp+rename 原子、`base_sha256`、`create_parents`） | `file_access = read_write` |
| `file.edit` | 行号范围或 anchored 编辑（`base_sha256`） | `file_access = read_write` |
| `file.mkdir` | 创建目录（可选 parents） | `file_access = read_write` |
| `file.move` | 重命名/移动（`base_sha256` + `allow_overwrite`） | `file_access = read_write` |
| `file.delete` | 删除文件或目录（recursive 需 policy 显式开启） | `file_access = read_write` |

#### Phase 3 — Patch

| method | 语义 | 必需权限 |
|---|---|---|
| `file.apply_patch` | 应用 unified diff（多文件、两阶段 rename + rollback） | `file_access = read_write` |

#### 未实现 / 显式推迟

- `file.watch`（长连接推送）— 不在当前分支；
- `file.chmod` / `file.chown` — 未实现；
- `file.symlink` — 未实现；
- Binary patch — apply_patch 遇到 binary diff 返回 `file.binary_patch_unsupported`；
- Rename/copy-only diff — apply_patch 返回 `file.unsupported_diff`，提示改用 `file.move` 或 `file.read + file.write`；
- `file.delete` 的 `if_match_sha256` 前置校验（下一 PR 补齐）；
- `FileAccessPolicyStore` 缓存（每请求 `load_default()` 读盘，未加 mtime 失效）；
- handler tracing / audit event（当前零 tracing，计划补齐）；
- 硬链接 inode 对照（跨平台复杂，显式推迟）。

## 技术细节

### 授权模型（正交双轴）

```rust
// crates/bifrost-admin/src/remote_invoke/types.rs

pub enum GrantScope {
    RemoteQuery,
    RemoteShellExec,
    RemoteShellInteractive,
    RemotePowerMgmt,
    RemoteImGateway,
}

pub enum FileAccessScope {
    None,       // 默认
    Read,
    ReadWrite,
}

pub enum CommandKind {
    QueryReadonly,   // "query.readonly"
    ShellExec,       // "shell.exec"
    File,            // "file" — 所有 file.* 方法
    PowerMgmt,       // "power.mgmt"
    ImGateway,       // "im.gateway"
}

pub fn scope_allows_command(grant_scope, file_access, kind) -> bool {
    match kind {
        CommandKind::File => matches!(file_access, FileAccessScope::Read | FileAccessScope::ReadWrite),
        CommandKind::ShellExec => matches!(grant_scope, GrantScope::RemoteShellExec | GrantScope::RemoteShellInteractive),
        CommandKind::PowerMgmt => matches!(grant_scope, GrantScope::RemotePowerMgmt),
        CommandKind::ImGateway => matches!(grant_scope, GrantScope::RemoteImGateway),
        CommandKind::QueryReadonly => true,
    }
}
```

> 早期设计曾在 `GrantScope` 中包含 `RemoteFileRead` / `RemoteFileWrite`，已移除。`CommandKind` 不拆 per-method 变体，统一 `File`；读写细粒度由 `FileAccessScope` + `policy.ops` 两层控制。

### `FileAccessPolicy`

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

pub enum FileOp {
    Read, ReadMany, List, Stat, Glob, Search, Hash, Outline,   // Phase 1
    Write, Edit, Mkdir, Move, Delete,                           // Phase 2
    ApplyPatch,                                                 // Phase 3
}
```

Policy 检查流程：

1. `canonicalize_within_roots(path, &roots)`：拒绝 `..` 逃逸、拒绝绝对路径超出 roots。
2. 符号链接检查：resolve 后 canonical path 必须在 roots 内，否则 `file.symlink_escape`。
3. Deny 匹配：`DenyMatcher` 匹配 root-relative 路径；写操作额外叠加 `write_denies`。
4. Op 检查：请求 op 必须在 `policy.ops` 内；只读 policy 对写操作 → `file.permission_denied`。
5. Gitignore：`respect_gitignore=true` 时 `list/glob/search` 走 `ignore` crate 过滤。
6. 通过后返回 `PolicyDecision { path (canonical), op, max_read_bytes, max_write_bytes, respect_gitignore, allow_overwrite, allow_recursive_delete, input_abs (lstat 用原始绝对路径) }`。

### SSH-key Full Trust 兼容规则

SSH-key 授权没有 pair-code scope 对话框，自动 seed 出 Full Trust policy（full-computer roots + 全 `file.*` ops）。新增 `FileOp`（如 `ReadMany`、`Outline`）时，已有 SSH-key policy 不能被卡在旧 ops 列表：

- 若已有 `match.ssh_fingerprint` policy 包含全部 legacy Full Trust ops，worker 自动追加新引入的 Full Trust ops；
- roots、denies、write_denies、byte caps、gitignore 行为、destructive 操作 flag 保留；
- 明确 narrow 的 policy（如 `ops = ["read", "list"]`）不自动扩展。

### `file.read_many` 两级授权

1. Request-level：`policy.ops` 必须含 `ReadMany`。管理员可以允许普通单文件 read 但禁用高扇出批量 read。
2. Per-file：每个 path 仍走 `Read` 权限检查。denied 或 missing 作为 item-level error 返回在批量响应内。

这样保留 coding-agent 的 partial batch 成功语义，同时让 `ReadMany` 成为有意义的 policy capability。

### `file.move` 安全契约

- `--base-sha256 <SHA>`：源 regular file 乐观锁校验。不匹配返回 `file.sha_mismatch`，源不动。
- `--allow-overwrite <bool>`：per-call 覆盖 policy 默认值。false + 目标存在 → `file.precondition_failed`。
- Regular file + 目标不存在的常见路径：atomic create-if-absent link + 删源，避免 check-then-rename TOCTOU。
- 目录仍走平台 rename 语义（目录不可 hard-link）。
- 源与目标两侧都走 `FileOp::Move` policy 校验。

### `file.apply_patch` 两阶段原子

1. `parse_patch()` 拆为 `Vec<PatchEntry>`；每个 entry 含 `old_path` / `new_path` / `body` / `kind`。
2. `PatchKind` 优先级分类：
   - `Binary` → `file.binary_patch_unsupported`；
   - `RenameOnly` → `file.unsupported_diff`（提示 `file.move`）；
   - `CopyOnly` → `file.unsupported_diff`（提示 `file.read + file.write`）；
   - `ModeOnly` → `file.unsupported_diff`；
   - `Modify` / `Create` / `Delete` → 正常处理。
3. 权限：逐文件 `FileAccessPolicy::check(path, cwd, FileOp::ApplyPatch)`。
4. Stage：写 `<parent>/.bifrost-patch.<pid>.<nanos>.<i>.tmp`；逐 hunk 校验 context；快照 `prior_mode` + `existed_before` 用于 rollback。
5. Commit：所有 tmp 校验通过后逐个 `rename(tmp, target)` + `chmod`。任一 rename 失败 → rollback 已 committed 文件（用快照恢复原内容 / 删除新建文件）。
6. EOL：侦测目标文件 EOL 风格，hunk 输出保持一致；`\ No newline at end of file` 正确处理。
7. `diff --git` 扩展头解析：`index`、`similarity index`、`rename from/to`、`copy from/to`、`old/new mode`、`new file mode`、`deleted file mode`、`Binary files … differ`。

### `file.edit` 双模式

**Anchored（推荐）**：`{ old_string, new_string, expected_count }`。`old_string` 不能为空，EOL 自动归一到源文件风格，命中数 ≠ `expected_count` → `file.anchor_not_unique`；完全不命中 → `file.anchor_not_found`；多个 anchored item 在同一 buffer 上串行 apply。

**Line-range**：`{ start_line, end_line, replacement }`；1-based inclusive。

混用 anchored 与 line-range → `file.invalid_args`。EOL 保留：侦测源文件行尾（CRLF/LF）保持一致；文件末尾换行按原状况自动补齐。

### 错误码表

| 错误码 | 触发场景 |
|---|---|
| `file.not_found` | 路径不存在 |
| `file.already_exists` | 目标已存在（mkdir 无 parents、move 目标已存在） |
| `file.cross_device` | move 跨设备无法 atomic rename（EXDEV） |
| `file.is_a_directory` | 期望文件却拿到目录 |
| `file.not_a_directory` | 期望目录的路径分量是文件 |
| `file.permission_denied` | 只读 policy 拒绝写、recursive delete 被关闭 |
| `file.deny_pattern` | 路径命中 `denies` / `write_denies` glob |
| `file.out_of_scope` | 路径超出 roots；CLI hint 提示 `/tmp -> /private/tmp` 与 `remote file scratch-dir` fallback |
| `file.symlink_escape` | 符号链接解析后超出 roots |
| `file.ignored_by_gitignore` | `respect_gitignore` 命中 |
| `file.binary_not_allowed` | 非 UTF-8 + `allow_binary=false` |
| `file.op_not_permitted` | 请求 op 不在 `policy.ops`；`read_many` 单独可被关闭，CLI hint 提示降级为逐个 `remote file read` |
| `file.anchor_not_found` | anchored edit 找不到 `old_string` |
| `file.anchor_not_unique` | anchored edit 命中次数 ≠ `expected_count` |
| `file.invalid_args` | 参数缺失/非法、anchored 与 line-range 混用、`old_string` 为空 |
| `file.invalid_glob` | glob 语法错误 |
| `file.invalid_regex` | 正则语法错误 |
| `file.invalid_deny` | deny 模式语法错误 |
| `file.io_error` | 底层 IO（未匹配上面特化 ErrorKind 的兜底） |
| `file.precondition_failed` | apply_patch context 不匹配、`allow_overwrite=false` 目标已存在 |
| `file.sha_mismatch` | `base_sha256` 校验失败（write/edit/move 通用） |
| `file.size_too_large` | 超过 `max_read_bytes` / `max_write_bytes` |
| `file.unsupported_algo` | hash 算法不支持 |
| `file.unsupported_diff` | apply_patch 遇到 rename/copy/mode-only diff |
| `file.binary_patch_unsupported` | apply_patch 遇到 binary diff |

### 架构分层

```
┌──────────────┐ request (kind=file, command=file.read) ┌────────────┐
│ Caller (CLI) │ ─────────────────────────────────────▶ │   Relay    │
└──────────────┘                                        └─────┬──────┘
                                                              ▼
                                                    ┌────────────────┐
                                                    │ bifrost-admin  │
                                                    │  executor.rs   │
                                                    └───────┬────────┘
                                                            ▼
                                                    ┌────────────────┐
                                                    │FileAccessPolicy│
                                                    │  policy.rs     │
                                                    │(bifrost-core)  │
                                                    └───────┬────────┘
                                                            ▼
                                                    ┌────────────────┐
                                                    │ file_ops.rs    │
                                                    │  (~5540 行)    │
                                                    └────────────────┘
```

| 路径 | 职责 |
|---|---|
| `crates/bifrost-core/src/file_access/policy.rs` | `FileAccessPolicy` / `PolicyDecision` / `FileOp` |
| `crates/bifrost-core/src/file_access/matcher.rs` | `DenyMatcher` / `GlobMatcher` |
| `crates/bifrost-core/src/file_access/path.rs` | `CanonicalPath` / `canonicalize_within_roots` |
| `crates/bifrost-core/src/file_access/error.rs` | `FileAccessError` 枚举 |
| `crates/bifrost-admin/src/remote_invoke/executor.rs` | 请求分发、权限检查、apply_patch 解析委托 |
| `crates/bifrost-admin/src/remote_invoke/file_ops.rs` | 所有 handler 实现、`parse_patch` / `PatchEntry` / `PatchKind` |
| `crates/bifrost-admin/src/remote_invoke/file_policy_store.rs` | Policy 存储（TOML load） |
| `crates/bifrost-admin/src/remote_invoke/file_access_roots.rs` | 全机器 roots 计算 |
| `crates/bifrost-admin/src/remote_invoke/file_transfer.rs` | 大文件分块 upload/download 桥接 |

### `scratch-dir` fallback

真实远端使用中，临时脚本写入 `/tmp` 会因为 macOS `/tmp -> /private/tmp` symlink 或 policy roots 不含临时目录而失败。CLI 新增：

```bash
bifrost remote file scratch-dir --cwd <repo>
```

在授权 cwd 下创建 `.bifrost-tmp`，底层仍走 `file.mkdir` + FileAccessPolicy 写权限校验，不绕过目标端授权边界。后续临时脚本写入该目录并由调用方清理。

## CLI / Web / Admin API 表面

### CLI 15 个子命令

```
bifrost remote file read        <path> [--offset N] [--limit N] [--max-bytes N] [--allow-binary] [--cwd DIR] [--output human|json]
bifrost remote file read-many   --path A --path B ... [--max-bytes N] [--allow-binary] [--cwd DIR] [--output human|json]
bifrost remote file scratch-dir [--cwd DIR] [--name .bifrost-tmp] [--output human|json]
bifrost remote file list        [path] [--depth N] [--max-matches N] [--cursor TOK] [--no-ignore] [--exclude NAME]... [--cwd DIR]
bifrost remote file stat        <path> [--cwd DIR]
bifrost remote file glob        <pattern> [--max-matches N] [--cursor TOK] [--no-ignore] [--exclude NAME]... [--cwd DIR]
bifrost remote file find        [pattern] [-e REGEX]... [--path P] [-i] [-F] [-w] [--glob G]
                                [-A N] [-B N] [-C N] [--max-matches N] [--max-scan N] [--cursor TOK]
                                [--no-ignore] [--exclude NAME]... [--cwd DIR]     # alias: search
bifrost remote file hash        <path> [--algo sha256] [--cwd DIR]
bifrost remote file outline     <path> [--max-symbols N] [--max-bytes N] [--cwd DIR]
bifrost remote file write       <path> [--content STR | --content-file/--from-local <local|-> | --content-b64 B64]
                                [--base-sha256 SHA] [--allow-overwrite BOOL] [--create-parents] [--cwd DIR]
bifrost remote file edit        <path> (--edits JSON | --from-local <local-json|->) [--base-sha256 SHA] [--cwd DIR]
bifrost remote file mkdir       <path> [--parents] [--cwd DIR]
bifrost remote file move        <from> <to> [--base-sha256 SHA] [--allow-overwrite BOOL] [--cwd DIR]   # alias: mv
bifrost remote file delete      <path> [--recursive] [--cwd DIR]                                      # alias: rm
bifrost remote file patch       [--patch-file/--from-local <local|-> | --patch-b64 B64] [--base-sha PATH=SHA]... [--cwd DIR]   # alias: apply-patch
```

要点：
- 内容检索子命令真名 `find`（`search` 是 visible alias），支持位置正则与可重复 `-e/--regex` 多模式 OR 组合，`-A/-B/-C` 上下文，`-F/-w/--glob/--no-ignore/--exclude` 控制；服务端方法名保持 `file.search`。
- `move` / `delete` / `patch` 为真名，`mv` / `rm` / `apply-patch` 为 visible alias，保持向后兼容。
- 读路径全部支持 `--cursor` / `--max-matches` 分页与 `--no-ignore` / `--exclude`。
- `write` 接受 `--content` / `--content-file` / `--from-local` / `--content-b64` / stdin。`edit --from-local` 读 caller 本地 edits JSON。`patch --from-local` 读 caller 本地 unified diff。
- `move` 暴露 `--base-sha256` 与 `--allow-overwrite`。

### Web UI

- Settings → Remote Invoke → File Access Policies：CRUD、roots / denies / write_denies / ops / byte caps 编辑。
- 授权向导预设：Query only（`remote_query` + `file_access=none`）、Full Access（`remote_shell_interactive` + `file_access=read_write`）、Custom。

### Admin API

- `POST /_bifrost/api/file-access-policy` / `GET` / `PUT` / `DELETE`：policy 管理。
- `POST /_bifrost/api/remote-invoke/execute`（内部）：executor 分发入口。
- Relay 侧：复用 `POST /v4/remote-invoke/client/calls` + `POST /client/calls/:id/frame` + `GET /calls/:id/events`。

## Sync 边界

- `FileAccessPolicy` 与 grant 关联，不参与 Bifrost Sync（多设备目录结构不同）。
- 文件内容永不落 relay，端到端加密。
- 审计日志仅记 metadata（method、path、grant_id、result），不含文件内容（当前 handler tracing 未上线；计划补齐）。

## Phase 1-4 实施路径

### Phase 1 — 只读能力（已完成）

- `FileAccessPolicy` / `FileAccessScope` / `CommandKind::File` 落地。
- `file.read` / `read_many` / `list` / `stat` / `glob` / `search` / `hash` / `outline` 全部实现。
- CLI 只读子命令与 hermetic 契约测试。

### Phase 2 — 写入能力（已完成）

- `file.write` / `edit` / `mkdir` / `move` / `delete` 落地。
- `base_sha256` 乐观锁、`allow_overwrite` per-call override、`create_parents`、EOL 保留。
- `move` 的 atomic create-if-absent link + 删源。

### Phase 3 — Patch（已完成）

- `parse_patch` + `PatchEntry` + `PatchKind`。
- 两阶段 stage+commit + rollback。
- `diff --git` 扩展头解析。

### Phase 4 — Hardening（进行中）

- ✅ `read_many` 两级授权。
- ✅ SSH-key Full Trust ops 自动补齐。
- ✅ `scratch-dir` + `out_of_scope` / `op_not_permitted` CLI hint。
- ✅ CLI 契约测试覆盖 15 子命令。
- ⏳ `file.delete` 加 `if_match_sha256`（P1）。
- ⏳ `FileAccessPolicyStore` 缓存（`OnceLock<RwLock>` + mtime 失效，P1）。
- ⏳ handler tracing + audit log（P2）。
- ⏳ 硬链接 inode 对照（P2）。
- ⏳ 默认 exclude 扩展（`.venv/dist/build/.next/.nuxt/.pytest_cache`，P2）。

## 测试方案

### 单元测试

- `crates/bifrost-core/src/file_access/policy.rs`：canonicalize_within_roots、symlink_escape、DenyMatcher、write_denies 叠加、respect_gitignore、byte caps。
- `crates/bifrost-admin/src/remote_invoke/file_ops.rs`：edit anchored / line-range 互斥、`file.write` `create_parents`、`file.move` atomic link fallback、`file.apply_patch` rollback、EOL 归一。
- `crates/bifrost-admin/src/remote_invoke/executor.rs`：`scope_allows_command` 各分支、`read_many` request-level gate。

### CLI 契约测试

- `crates/bifrost-cli/tests/cli_commands.rs`：15 个 `remote file` 子命令的 hermetic 参数解析测试。
- `crates/bifrost-cli/src/commands/remote.rs::tests::test_build_remote_file_scratch_dir_uses_policy_checked_mkdir`：`scratch-dir` 底层走 `file.mkdir` 权限校验。
- `cargo test -p bifrost-cli remote:: --lib --no-run`：编译校验。

### E2E 测试

- `e2e-tests/tests/test_remote_file_api_e2e.sh`：本地 relay 全 method 覆盖。
- `e2e-tests/tests/test_remote_file_relay_e2e.sh`：线上 relay 场景。
- `crates/bifrost-e2e/src/tests/remote_file_api.rs`：Rust 端 E2E 覆盖 write / edit / move / apply_patch 的乐观锁与 rollback。

### Human tests

- `human_tests/remote-invoke-file.md`：15 子命令真实覆盖、SSH-key Full Trust ops 自动补齐、scratch-dir、out_of_scope hint。
- `human_tests/remote-invoke-v5-pop-hardening.md`：安全模型回归。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：15 子命令是否全部有 CLI 契约测试；`read_many` 两级授权是否生效；SSH-key Full Trust 兼容是否 narrow。
- 复核 diff：`FileOp` 新增变体是否被 `scope_allows_command` / `FileAccessPolicy::check` 全部覆盖；executor dispatch 是否遗漏。
- 重点 review：
  - `canonicalize_within_roots` 对符号链接的处理；
  - `file.move` atomic link fallback 在 EXDEV 情况下是否正确降级；
  - `apply_patch` rollback 是否覆盖新建文件 + 修改文件混合场景。
- 复测：`test_remote_file_api_e2e.sh` + `test_remote_file_relay_e2e.sh` + human `remote-invoke-file.md`。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 检查 `git status --short` / `git diff`，确保 CLI help 与 site docs (`site/src/content/docs/reference/cli.md`) 同步。
- 重点 review：
  - `out_of_scope` / `op_not_permitted` CLI hint 覆盖 macOS `/tmp -> /private/tmp` 与 `read_many` 关闭两种典型情况；
  - Edit anchored EOL 归一在 CRLF 源文件上的表现；
  - `base_sha256` 全文件 vs 片段 sha 的区分（`file.read` 返回 `file_sha256`）。
- 复测：Rust E2E `remote_file_api.rs` + human tests 全套。

## 风险与决策

- **Policy store 缓存**：目前每请求 `load_default()` 读盘；高频调用下 IO 开销可感知。计划 `OnceLock<RwLock>` + mtime 失效。
- **审计 tracing 缺失**：handler 目前零 tracing，安全事件难追溯。计划每 handler 入口 `tracing::info!(target="audit.file", …)`。
- **硬链接 inode 对照**：跨平台方案不明确（Windows 不同语义），显式推迟。
- **`file.watch` 长连接**：改造成本大且 use case 有限，不在当前分支。
- **`file.chmod` / `file.chown` / `file.symlink`**：coding-agent 场景很少需要，未来按需引入。
- **P0 hardening 已上线**：`read_many` gate、SSH-key Full Trust 兼容、`move` safety、CLI 契约覆盖 15 子命令、scratch-dir + hints 均已合入。

## 参考

- GitHub Contents API — `If-Match-SHA` 乐观锁灵感。
- Claude Code / Cursor 的 `file_read` / `file_edit` / `grep` / `glob` / `apply_patch` 工具。
- `crates/bifrost-admin/src/remote_invoke/` 现有 executor / worker / types 分层。
- `design/grant-file-access.md` — grant 与 file_access 授权流程。
- `design/remote-invoke-shell-e2e-regressions.md` — Shell Access 安全模型。
- `design/remote-file-transfer.md` — 大文件分块 upload/download 补丁通道。
