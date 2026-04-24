# Remote Invoke File API 设计方案

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

## 能力矩阵（当前实现：Phase 1/2/3）

| method           | 语义                                           | 必需 scope              |
| ---------------- | ---------------------------------------------- | ----------------------- |
| `file.read`      | 读取文件（支持 offset/length/encoding）        | `remote_file_read`      |
| `file.list`      | 列出目录（支持 recursive/glob/max_depth）      | `remote_file_read`      |
| `file.stat`      | 查询元信息（类型/大小/mtime/mode/symlink）     | `remote_file_read`      |
| `file.glob`      | 路径通配匹配                                    | `remote_file_read`      |
| `file.search`    | 内容检索（ripgrep 语义）                       | `remote_file_read`      |
| `file.hash`      | 计算文件哈希（sha256）                         | `remote_file_read`      |

Phase 1 的只读能力、Phase 2 的写入/编辑能力与 Phase 3 的 unified diff apply 已在当前分支串通。`file.watch` 仍是后续能力，不属于本轮实现。

### Phase 2 写能力

| method             | 语义                                            | 必需 scope           |
| ------------------ | ----------------------------------------------- | -------------------- |
| `file.write`       | 整文件覆盖写（原子 temp + rename）              | `remote_file_write`  |
| `file.edit`        | 局部编辑（replace_range/insert_after/find_replace） | `remote_file_write`  |
| `file.mkdir`       | 创建目录                                         | `remote_file_write`  |
| `file.move`        | 重命名/移动                                      | `remote_file_write`  |
| `file.delete`      | 删除文件或目录                                   | `remote_file_write`  |

### Phase 3 patch 能力

| method             | 语义                                            | 必需 scope           |
| ------------------ | ----------------------------------------------- | -------------------- |
| `file.apply_patch` | 应用 unified diff（多文件）                     | `remote_file_write`  |
| `file.watch`       | 长连接推送文件变更                              | `remote_file_read`   |

## 架构分层

```
┌──────────────┐  request (method=file.read, params=…)  ┌────────────┐
│ Caller (CLI) │ ───────────────────────────────────▶ │   Relay    │
└──────────────┘                                        └─────┬──────┘
                                                              ▼
                                                    ┌─────────────────┐
                                                    │ Target Client   │
                                                    │  (bifrost-admin)│
                                                    └─────────┬───────┘
                                                              ▼
                                                    ┌─────────────────┐
                                                    │ File Access     │
                                                    │ Policy Guard    │
                                                    └─────────┬───────┘
                                                              ▼
                                                    ┌─────────────────┐
                                                    │ FS operations   │
                                                    └─────────────────┘
```

- Caller 端新增 `bifrost remote file <subcmd>` 系列子命令，打包请求走 relay。
- Relay 仅承担路由、转发、审计，不理解 file API 内容（与现有 `query.readonly` 一致，保持端到端加密）。
- Target 端 `bifrost-admin` 中新增 `remote_invoke::file` 模块，负责：
  1. 依据 `grant_scope` 拒绝无权请求。
  2. 依据 `FileAccessPolicy` 做路径归一化、白名单匹配、大小限制、符号链接处理。
  3. 执行具体文件操作，返回结构化结果。
- FileAccessPolicy 落在 `crates/bifrost-core` 中作为可复用类型，供 admin 层和 CLI 配置入口共享。

## 授权模型扩展

### 新 grant scope

在 `GrantScope` 枚举上新增两个变体：

```rust
#[serde(rename = "remote_file_read")]
RemoteFileRead,
#[serde(rename = "remote_file_write")]
RemoteFileWrite,
```

`CommandKind` 枚举上新增：

```rust
#[serde(rename = "file.read")]
FileRead,
#[serde(rename = "file.list")]
FileList,
#[serde(rename = "file.stat")]
FileStat,
#[serde(rename = "file.glob")]
FileGlob,
#[serde(rename = "file.search")]
FileSearch,
#[serde(rename = "file.hash")]
FileHash,
#[serde(rename = "file.write")]
FileWrite,
#[serde(rename = "file.edit")]
FileEdit,
#[serde(rename = "file.mkdir")]
FileMkdir,
#[serde(rename = "file.move")]
FileMove,
#[serde(rename = "file.delete")]
FileDelete,
#[serde(rename = "file.apply_patch")]
FileApplyPatch,
```

`GrantScope::allows_command` 规则：

- `RemoteFileRead` → 允许所有 `file.*` 只读类命令。
- `RemoteFileWrite` → 允许所有 `file.*` 命令（含只读）。
- `RemoteQuery`/`RemoteShellExec`/`RemoteShellInteractive` 均 **不** 授予 file API 能力；避免误授权覆盖。

### File Access Policy

类比 Shell Access policy，新增独立的 File Access policy/profile 存储（放在 bifrost-admin 数据目录下）。首版数据结构：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccessPolicy {
    pub id: String,
    pub name: String,
    /// 允许访问的路径白名单（glob 表达式）。
    pub roots: Vec<String>,
    /// 强制拒绝的路径黑名单，优先级高于 roots。
    pub denies: Vec<String>,
    /// 允许的 method 名（例如 `file.read`）。
    pub ops: Vec<String>,
    /// 单文件最大字节数（读）。
    pub max_read_bytes: u64,
    /// 单次编辑最大字节数（写）。
    pub max_edit_bytes: u64,
    /// 是否跟随符号链接。
    pub follow_symlinks: bool,
    /// search / glob 是否默认尊重 .gitignore。
    pub respect_gitignore: bool,
}
```

内置默认 policy：

```toml
[[file_access_policy]]
id = "workspace-readonly"
name = "Workspace Read-only"
roots = ["${WORKSPACE}/**"]
denies = [
  "**/.env*",
  "**/*.pem",
  "**/*.key",
  "**/secrets/**",
  "**/.git/config",
  "**/node_modules/**",
  "**/.pnpm-store/**",
]
ops   = ["file.read", "file.list", "file.stat", "file.glob", "file.search", "file.hash"]
max_read_bytes   = 8_388_608
max_edit_bytes   = 0
follow_symlinks  = false
respect_gitignore = true
```

Web UI Settings → Remote Invoke 面板新增 "File Access" 子标签，用法对标现有 "Shell Access"：

- Policy 列表 / CRUD
- 授权弹窗中新增 "文件访问" 勾选，支持 `none` / `selected` / `all` 三档。
- `selected` 时支持绑定具体 File Access Policy。

## 协议细节

remote invoke 请求复用既有 `RemoteInvokeRequest` 包络，但在 `command.kind` 上引入新枚举类别 `FileOperation`（或直接扩展现有 `CommandKind`，本设计采用后者）。

### 通用请求字段

```jsonc
{
  "kind": "file.read",          // CommandKind
  "command": "file.read",       // 与 kind 同名，沿用现有 summary_label 路径
  "args_json": "{…}",           // 每个 method 独立 schema
  "policy_id": null,            // 可选，用于绑定具体 FileAccessPolicy
  "grant_scope": "remote_file_read"
}
```

### `file.read`

```jsonc
// request.args_json
{
  "path": "crates/bifrost-core/src/lib.rs",
  "offset": 0,               // 字节偏移，默认 0
  "length": 65536,           // 最大读取字节数，默认 min(policy.max_read_bytes, 8MiB)
  "encoding": "utf8"         // utf8 | binary（后者 content 为 base64）
}

// response
{
  "ok": true,
  "path": "crates/bifrost-core/src/lib.rs",
  "size": 4096,
  "sha256": "…",
  "content": "…",
  "encoding": "utf8",
  "truncated": false
}
```

### `file.list`

```jsonc
{
  "path": "crates/bifrost-core/src",
  "recursive": false,
  "glob": "**/*.rs",
  "max_depth": 3,
  "include_hidden": false,
  "limit": 500
}
```

响应：

```jsonc
{
  "ok": true,
  "entries": [
    { "name": "lib.rs", "type": "file", "size": 4096, "mtime": "2026-04-24T10:00:00Z", "mode": "0644" },
    { "name": "matcher", "type": "dir", "size": 0, "mtime": "2026-04-23T08:00:00Z", "mode": "0755" }
  ],
  "truncated": false
}
```

### `file.stat`

```jsonc
{
  "path": "crates/bifrost-core/src/lib.rs",
  "with_sha256": false
}
```

响应：

```jsonc
{
  "ok": true,
  "type": "file",
  "size": 4096,
  "mtime": "2026-04-24T10:00:00Z",
  "mode": "0644",
  "symlink_target": null,
  "sha256": null
}
```

### `file.glob`

```jsonc
{
  "pattern": "crates/**/*.rs",
  "cwd": "/Users/eden/work/github/bifrost",
  "limit": 1000
}
```

响应：

```jsonc
{
  "ok": true,
  "paths": ["crates/bifrost-core/src/lib.rs", "…"],
  "truncated": false
}
```

### `file.search`

```jsonc
{
  "query": "RemoteInvokeRequest",
  "path": "crates",
  "regex": false,
  "glob": "**/*.rs",
  "case_sensitive": false,
  "max_results": 200,
  "context_lines": 2
}
```

响应：

```jsonc
{
  "ok": true,
  "matches": [
    { "file": "crates/bifrost-admin/src/remote_invoke/types.rs", "line": 120, "col": 8, "text": "pub struct RemoteInvokeRequest {", "context_before": ["…"], "context_after": ["…"] }
  ],
  "truncated": false,
  "scanned_files": 42
}
```

### `file.hash`

```jsonc
{
  "path": "crates/bifrost-core/src/lib.rs",
  "algo": "sha256"
}
```

响应：

```jsonc
{
  "ok": true,
  "path": "crates/bifrost-core/src/lib.rs",
  "algo": "sha256",
  "sha256": "…"
}
```

### 错误码

统一前缀 `file.`：

| code                    | 触发场景                                  |
| ----------------------- | ----------------------------------------- |
| `file.not_found`        | 路径不存在                                |
| `file.permission_denied`| 未授权 / policy 拒绝                      |
| `file.scope_required`   | grant_scope 未包含 `remote_file_read/write` |
| `file.too_large`        | 超过 `max_read_bytes` / `max_edit_bytes`  |
| `file.invalid_encoding` | encoding 声明与实际内容不符               |
| `file.invalid_argument` | 参数缺失/非法（例如 offset < 0）          |
| `file.io_error`         | 底层 IO 错误，保留 message                |
| `file.sha_mismatch`     | （Phase 2）`if_match_sha256` 不符         |
| `file.patch_rejected`   | （Phase 3）所有 hunk 均未应用             |
| `file.partial_applied`  | （Phase 3）部分 hunk 应用                 |

## 安全设计

- **路径归一化**：所有 `path` 在 guard 中统一 canonicalize，拒绝包含 `..` 的逃逸；拒绝绝对路径超出 `roots` 白名单。
- **符号链接**：`follow_symlinks=false` 时，解析出的 symlink 目标超出 `roots` 直接拒绝，避免用软链接绕过白名单。
- **硬链接**：write/edit 路径通过 inode 对照，发现与 deny 列表共享 inode 时拒绝（Phase 2 实现）。
- **二进制保护**：`encoding=utf8` 模式下，发现 non-UTF8 字节序列即返回 `file.invalid_encoding`。
- **大小保护**：`file.read` 默认上限 `min(policy.max_read_bytes, 8MiB)`，caller 可传更小的 `length`，不可传更大。
- **gitignore 感知**：`file.search` / `file.glob` 默认尊重 `.gitignore`，可由 caller 通过参数关闭；`file.read` 不受 gitignore 影响。
- **审计**：每次请求在 relay 侧记录 `grant_id + method + path_hash + size + sha256`，不存储文件内容；失败请求保留错误码。
- **路径脱敏**：审计日志在存入时将 `$HOME` / workspace 根路径做缩写。

## 可观测性

- `tracing` span 按 `remote_file.<method>` 打点，tag 包含 `method`、`path_len`、`size`、`duration_ms`。
- admin HTTP 层新增 `/remote-invoke/file/metrics` 汇总（Phase 2+，本 PR 不做）。
- CLI 输出支持 `--format table|compact|json|json-pretty`，默认为 `table`。

## CLI 映射（caller 侧）

当前子命令骨架：

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

## 实现拆分

- PR-1（本设计落地）：
  - `crates/bifrost-core/src/file_access/` 新模块：`FileAccessPolicy`、路径归一化、glob 匹配、错误码类型。
  - 设计文档与 human_tests 骨架。
  - **不引入 runtime 依赖变更、不改动现有 crate 公共 API**，保证 CI 可以直接跑通。
- PR-2：扩展 `GrantScope`、`CommandKind`，新增 `remote_invoke::file` 模块。
- PR-3：Caller CLI 子命令 `bifrost remote file <subcmd>`。
- PR-4：Web UI "File Access" 面板。
- PR-5：Phase 2（write/edit/mkdir/move/delete）。
- PR-6：Phase 3（apply_patch/watch）。

## 与现有能力的分工原则

| 需求                | 推荐路径                 | 说明                               |
| ------------------- | ------------------------ | ---------------------------------- |
| 读/改单个文件       | `file.*`                 | 编码/换行/原子性保障               |
| 批量 apply diff     | `file.apply_patch`       | Phase 3                            |
| 目录遍历、内容检索  | `file.list` / `file.search` | 结构化返回                         |
| 运行 `cargo build` | `shell.exec`             | 触发进程                           |
| 运行 `git` 命令     | `shell.exec`             | 属于工作流，不归文件 API 管        |
| 启动前端/运行测试   | `shell.exec`             | 进程级能力                         |

## 参考

- GitHub Contents API — `If-Match-SHA` 乐观锁
- Claude Code / Codex / Cursor 的 file_read / file_edit / grep / glob / apply_patch 工具
- 现有 `crates/bifrost-admin/src/remote_invoke/` 的 executor/worker/types 分层

---

## 9. Phase 2 — 写入与原子编辑（≈ 2 周）

Phase 2 将引入**写能力**，但遵循与 Phase 1 相同的铁律：任何写入都必须经 `FileAccessPolicy::check(op=Write)` 校验、经审计、并通过 sha 乐观锁防止覆盖他人改动。

### 9.1 新增方法

| 方法 | 说明 | 关键约束 |
|------|------|----------|
| `file.write` | 写/覆盖整个文件，可选创建父目录 | 必须携带 `base_sha256`（`new` 文件传空串）做乐观锁；超过 `max_write_bytes` 拒绝 |
| `file.edit` | 结构化编辑：`[{ range: {start_line, end_line}, replacement }]`；一次 request 一个文件多段 edits | 客户端传 `base_sha256`；服务端校验冲突后原子写入；失败返回 `file.precondition_failed` |
| `file.mkdir` | 创建目录（支持 `--parents`） | 命中 deny 或非 roots 均拒绝 |
| `file.move` | 重命名/移动 | 源和目标都必须在 roots 内；跨 root 拒绝 |
| `file.delete` | 删除文件或空目录（递归删除默认禁用） | `recursive=true` 时必须在 policy 中显式 `allow_recursive_delete=true` |

### 9.2 Grant scope 与 policy

- 新增 `GrantScope::RemoteFileWrite`（Phase 1 已预留枚举变体）。
- `FileAccessPolicy` 扩展：
  - `allow_write_roots: Vec<PathBuf>`（可以是 `roots` 子集，进一步收紧）
  - `max_write_bytes: u64`
  - `allow_overwrite: bool`（默认 true，只影响 `file.write`）
  - `allow_recursive_delete: bool`（默认 false）
  - `write_denies: Vec<String>`（与 read 的 `denies` 合并叠加）
- `GrantScope::allows_command` 将所有 write 类 CommandKind 收敛到 `RemoteFileWrite`（shell scope 不再默认放行写）。

### 9.3 协议要点

- 所有写请求 body 中必须包含 `base_sha256`（空串表示 "文件不存在，期望创建"）。
- 响应返回 `{ new_sha256, size, mtime_unix }`，便于调用端继续基于新状态做后续操作。
- 错误码新增：`file.precondition_failed`、`file.exists`、`file.not_empty`、`file.write_quota_exceeded`。

### 9.4 审计

每次写操作写一条审计记录：`{ grant_id, op, path, base_sha, new_sha, bytes, result }`。失败也写（`result=rejected/failed`），并记录拒绝原因。

### 9.5 human_tests 必须覆盖

- **并发写**：两个 caller 基于同一 `base_sha` 同时写，后到者必须 `file.precondition_failed`。
- **路径逃逸**：`../../etc/hosts` / 绝对路径 / 跨 root 全部拒绝；
- **Symlink**：通过符号链接写入 roots 外目标，`file.symlink_escape`；
- **大文件拒绝**：超过 `max_write_bytes` 必须拒绝，且不在磁盘留下临时残片（原子写：`tmpfile + rename`）；
- **mkdir/rename/delete 的 deny 拦截**。

---

## 10. Phase 3 — Unified diff & 进阶能力

### 10.1 `file.apply_patch`（多文件 diff）

- 接收标准 unified diff（`git apply` 兼容格式），支持一次 patch 多文件。
- 服务端流程：
  1. 解析 diff，提取每个 hunk 的 `before_sha256`。
  2. 对每个目标文件串行走 `FileAccessPolicy::check(op=Write)`。
  3. 逐 hunk 比对 `before_sha256`；任一失败则整个 patch 回滚（要么全提交，要么全不提交）。
  4. 通过后以 `write` 原子落盘，逐文件登记 audit。
- 错误码：`patch.parse_failed`、`patch.hunk_conflict`、`patch.no_such_file`、`patch.partial_aborted`（部分成功即 abort 的强保护）。

### 10.2 其余进阶能力

| 能力 | 描述 |
|------|------|
| `file.watch` | 长连接订阅目录变更事件（FSEvents / inotify），Phase 3.1 |
| `file.chmod` / `file.chown` | 权限/所有者管理，默认禁用，需显式 `allow_metadata_ops` |
| `file.symlink` | 创建符号链接，默认禁用；启用后目标必须在 roots 内 |
| CLI `bifrost remote file apply-patch --patch-file <patch.diff>` | 一键投递 unified diff |

### 10.3 与 Phase 2 的叠加约束

- Phase 3 不引入新的 GrantScope；`file.apply_patch` 复用 `RemoteFileWrite`。
- Phase 3 强制要求 Phase 2 的审计链已上线，任何 patch 失败都可以从 audit 单文件级别定位回滚点。
