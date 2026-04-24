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

## 能力矩阵（首版 Phase 1：只读最小集）

| method           | 语义                                           | 必需 scope              |
| ---------------- | ---------------------------------------------- | ----------------------- |
| `file.read`      | 读取文件（支持 offset/length/encoding）        | `remote_file_read`      |
| `file.list`      | 列出目录（支持 recursive/glob/max_depth）      | `remote_file_read`      |
| `file.stat`      | 查询元信息（类型/大小/mtime/mode/symlink）     | `remote_file_read`      |
| `file.glob`      | 路径通配匹配                                    | `remote_file_read`      |
| `file.search`    | 内容检索（ripgrep 语义）                       | `remote_file_read`      |
| `file.hash`      | 计算文件哈希（sha256）                         | `remote_file_read`      |

Phase 2（写入与原子编辑）与 Phase 3（unified diff & 进阶能力）在独立 PR 中单独实现，本设计文档记录完整能力矩阵以指导后续演进，但代码实现严格按 Phase 1 范围落地。

### Phase 2 预览（仅设计，不在本 PR 实现）

| method             | 语义                                            | 必需 scope           |
| ------------------ | ----------------------------------------------- | -------------------- |
| `file.write`       | 整文件覆盖写（原子 temp + rename）              | `remote_file_write`  |
| `file.edit`        | 局部编辑（replace_range/insert_after/find_replace） | `remote_file_write`  |
| `file.mkdir`       | 创建目录                                         | `remote_file_write`  |
| `file.move`        | 重命名/移动                                      | `remote_file_write`  |
| `file.delete`      | 删除文件或目录                                   | `remote_file_write`  |

### Phase 3 预览

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

Phase 1 子命令骨架：

```
bifrost remote file read    <path> [--offset N --length M --format json|text|base64]
bifrost remote file list    <path> [--recursive --glob '**/*.rs' --limit N]
bifrost remote file stat    <path> [--with-sha256]
bifrost remote file glob    '<pattern>' [--cwd <cwd> --limit N]
bifrost remote file search  <query> [--path P --regex --glob '**/*.rs' --context 2 --max-results N]
bifrost remote file hash    <path> [--algo sha256]
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
