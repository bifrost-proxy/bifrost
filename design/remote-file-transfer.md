# Remote 大文件上传/下载（Upload / Download）设计方案

> **状态**：实现中（分支 `codex/remote-file-transfer`，基于 `origin/main` / v0.0.132）
> **依赖设计**：`design/remote-invoke-file-api.md`（File API 授权模型、FileOp、executor/file_ops 分层、错误码、SSH-key Full Trust op-set 迁移）

## 1. 背景与目标

现有 File API（`file.write` / `file.read`）通过一次 remote-invoke 调用整文件 base64 传输，受两个硬约束限制：

1. **Relay 单次调用上限 10 MB**：Relay 通道对单个 remote-invoke 请求/响应包络有 10 MB 的硬上限。`file.write` 把整文件 base64 塞进一次 `args_json`，任何超过约 7 MB 原始字节的文件都会因 base64 膨胀（4/3 倍）+ JSON/POP 加密包络溢出而失败。
2. **策略字节上限**：`FileAccessPolicy.max_write_bytes` 默认 2 MiB，进一步限制了单次写入。

因此需要新增 **分块（chunked）上传/下载** 能力：把一个超大文件（超大内容文件、压缩归档、任意二进制）切成多个独立的 sub-10MB remote-invoke 调用，支持**断点续传（resume）**与**端到端完整性校验（sha256）**。

**用户目标**：
- 能可靠传输 > 10 MB 的二进制、> 100 MB 的大文件、压缩归档。
- 传输中断后可用 `--resume` 从断点续传，无需重头再来。
- 上传/下载两端 sha256 逐块 + 整文件双重校验，保证字节级一致。
- 完全走真实 Relay 分块链路，复用现有授权模型（FileAccessPolicy + grant file_access）。

## 2. Chunk 尺寸预算（为什么默认 512 KiB 原始 / 1 MiB 传输上限）

**真正的约束是 Relay 的“单帧 POST body 上限”，而不是请求方向的调用上限。** 下载分块的数据是作为加密 call frame **回传**给 caller 的（target → relay `post_call_frame` → caller），bifrost-sync-server 的 `MAX_BODY_SIZE = 2 MiB` 会对该 POST body 直接 413 拒绝（错误文案写的是 “max 1MB”，实际阈值 2 MiB）。

一块原始字节在回传路径上被 **两次 base64 膨胀**：

```
raw_chunk  --base64-->  chunk_b64（~1.34x）  --放入 stdout JSON-->  --POP/加密包络 + 再次 base64-->  ~2x 的最终 frame body
```

预算推导（目标 frame body < 2 MiB，留足包络余量）：

| 项 | 大小 |
|---|---|
| 默认请求 chunk（raw） | 512 KiB = 524 288 B |
| chunk_b64 后 | 约 683 KiB |
| stdout JSON + sha256 + 结构 | 约数百 B |
| POP / 加密包络（再次 base64） | 使整体约 2x |
| 最终 frame body | 约 1 MiB < 2 MiB ✅ |

因此：
- 服务端 clamp 上限 `transfer_chunk_max_bytes` 默认 **1 MiB**（raw）——即便 caller 请求更大的块也会被夹到 1 MiB，保证回传 frame 稳定在 2 MiB 之下。
- 客户端默认请求 **512 KiB**，给包络与 JSON 留余量。
- 客户端可用 `--chunk-size` 请求任意值，服务端会对 **上传与下载两端** clamp 到 `min(requested, transfer_chunk_max_bytes)` 并把 `effective_chunk_size` 回传，客户端必须以回传值为准。
- （上传方向的 request body 上限更宽松，但为对称与简单起见统一采用同一 clamp。）

## 3. 核心层改动（`crates/bifrost-core/src/file_access/`）

### 3.1 FileOp 枚举新增

```rust
pub enum FileOp {
    // ... 现有 Phase 1/2/3 ...
    Upload,    // caller -> remote 分块写
    Download,  // remote -> caller 分块读
}
```

- `Upload.is_write() == true`（写类，受 grant `file_access = read_write` 门禁）。
- `Download.is_write() == false`（读类，`file_access >= read` 即可）。

### 3.2 FileAccessPolicy 新增字段

```rust
pub struct FileAccessPolicy {
    // ... 现有字段 ...
    /// 单个文件传输（上传或下载）的字节上限。默认 5 GiB。
    pub max_transfer_bytes: u64,

    /// 保证 raw + base64 + JSON + POP 包络 < 10 MB Relay 上限。
    pub transfer_chunk_max_bytes: u64,
}
```

默认值：`max_transfer_bytes = 5 GiB`，`transfer_chunk_max_bytes = 1 MiB`。两者均带 `#[serde(default = ...)]`，旧 TOML 无该字段时回落默认值。

`new_readonly` 允许 `Download`（读类），`new_read_write` 允许 `Upload` + `Download`。

### 3.3 SSH-key Full Trust op-set 迁移

`file_policy_store.rs::full_file_ops()` 追加 `Upload` + `Download`。`legacy_ssh_full_file_ops()` **不含**这两个新 op —— 于是既有 SSH-key Full Trust grant（拥有全部 legacy op）会被 `ensure_ssh_fingerprint_grant_full_ops_in_place` 自动追加 `Upload`/`Download`，而被显式收窄的策略（如 `ops = ["read","list"]`）保持不变。这与 `read_many`/`outline` 引入时的机制完全一致。

## 4. 服务端层（`crates/bifrost-admin/src/remote_invoke/`）

新增独立模块 `file_transfer.rs`（避免继续膨胀已 5000+ 行的 `file_ops.rs`），实现全部 handler；`executor.rs` 负责 dispatch。

### 4.1 会话状态（in-memory）

```rust
struct TransferSession {
    id: String,
    final_path: PathBuf,      // canonical 目标/源路径
    part_path: PathBuf,       // <dir>/.bifrost-upload.<id>.part（仅 upload）
    total_size: u64,
    total_sha256: String,
    chunk_size: u64,          // effective
    prior_mode: Option<u32>,  // upload commit 时恢复
    created_at: Instant,
    last_activity: Instant,
}
```

- 用进程级 `OnceLock<Mutex<HashMap<String, TransferSession>>>` 跟踪。
- 会话 TTL（如 30 分钟无活动）惰性清理；`upload_abort`/`upload_commit`/`download` 完成后移除。
- upload_id / download_id 用随机 hex（不可预测），避免会话被猜测劫持。

### 4.2 Upload（caller -> remote）方法

| method | 参数 | 返回 |
|---|---|---|
| `file.upload_begin` | path, total_size, total_sha256, chunk_size(请求值), overwrite, create_parents | upload_id, effective_chunk_size, received_offset（已接收字节，用于 resume） |
| `file.upload_chunk` | upload_id, offset, chunk_b64, chunk_sha256 | next_offset（下一个期望 offset） |
| `file.upload_commit` | upload_id, total_sha256(期望) | path, bytes_written, sha256（整文件） |
| `file.upload_abort` | upload_id | aborted:true |
| `file.upload_status` | upload_id | received_offset, total_size, chunk_size |

**begin 语义**：
- policy.check(path, FileOp::Upload)；校验 `total_size <= max_transfer_bytes`；`overwrite`/`create_parents` 遵循 policy。
- `chunk_size` clamp 到 `min(requested, transfer_chunk_max_bytes)`,默认上限 1 MiB(未指定时服务端默认请求 512 KiB)。
- part 文件路径 `<target_dir>/.bifrost-upload.<upload_id>.part`，与目标同文件系统（保证 commit 时 atomic rename + 支持 resume）。
- 若已存在同 upload_id 的 part（resume 场景由 upload_status 触发），返回已有字节数作为 `received_offset`。

**chunk 语义**：
- 校验 upload_id 存在；校验 `offset == 当前 part 文件大小`（严格顺序追加，拒绝乱序/重复覆盖）。
- base64 decode → 校验 `chunk_sha256`（per-chunk 完整性）→ 校验 `part_size + chunk_len <= total_size`（防溢出）→ 在 offset 处追加写入 part。
- 返回 `next_offset = offset + chunk_len`。

**commit 语义**：
- 校验 `part_size == total_size`；对整个 part 文件重算 sha256 → 与 `total_sha256` 比对（不符 → `file.sha_mismatch`，保留 part 供重试）。
- atomic rename part → final；恢复/设置 mode。
- 移除会话，返回整文件 sha256。

### 4.3 Download（remote -> caller）方法

| method | 参数 | 返回 |
|---|---|---|
| `file.download_begin` | path | download_id, total_size, total_sha256, effective_chunk_size |
| `file.download_chunk` | download_id, offset, length | chunk_b64, chunk_sha256 |

- begin：policy.check(path, FileOp::Download)；`total_size <= max_transfer_bytes`；打开文件快照（记录 size + 整文件 sha256，保证一致视图），返回 download_id。
- chunk：seek 到 offset 读 `min(length, effective_chunk_size)` 字节，base64 + per-chunk sha256 返回。

## 5. CLI 层（`crates/bifrost-cli/`）

### 5.1 子命令（`cli/remote.rs::RemoteFileCommands`）

```
bifrost remote file upload   <local> <remote> [--chunk-size N] [--overwrite] [--create-parents] [--resume] [--no-progress] [--cwd DIR]
bifrost remote file download <remote> <local> [--chunk-size N] [--resume] [--no-progress] [--cwd DIR]
```

### 5.2 编排（新增 `commands/remote/transfer.rs`，避免继续膨胀已 11000+ 行的 `commands/remote.rs`）

- upload：本地按 effective_chunk_size 分块读 → base64 + per-chunk sha256 → 顺序调用 `upload_chunk`（失败重试 N 次）→ 显示进度（除非 `--no-progress`）→ 全部完成后 `upload_commit`，校验返回 sha 与本地整文件 sha 一致。
- 断点续传：caller-side resume state 文件写在 `<local>.bifrost-upload-state.json`（记录 remote path/total_sha256/upload_id 等）。`--resume` 时先调 `upload_status`/`upload_begin` 拿 `received_offset`，从该 offset 续传。
- download：`download_begin` → 循环 `download_chunk` 写入本地 `<local>.part` → 完成后 rename → 校验整文件 sha256。resume 用本地 `.part` 已有大小作为起始 offset。

## 6. 协议时序（文本图）

### 6.1 Upload（含 resume）

```
Caller                                  Remote
  |  upload_begin(path,size,sha,chunk)    |
  | ------------------------------------> |  policy.check(Upload); clamp chunk
  | <------ upload_id, eff_chunk, recv=0 -|
  |                                       |
  |  upload_chunk(id,0,b64,csha)          |
  | ------------------------------------> |  verify csha; append @0
  | <------------------- next_offset=6MiB-|
  |  upload_chunk(id,6MiB,...)            |
  | ------------------------------------> |  ...
  |            (中断 / 进程被 kill)         |
  |  --resume: upload_status(id)          |
  | ------------------------------------> |
  | <---------------- recv=48MiB ---------|
  |  upload_chunk(id,48MiB,...)  续传      |
  | ------------------------------------> |
  |  upload_commit(id, total_sha)         |
  | ------------------------------------> |  verify whole-file sha; rename part->final
  | <------------- path, sha256 ----------|
```

### 6.2 Download

```
Caller                                  Remote
  |  download_begin(path)                 |
  | ------------------------------------> |  policy.check(Download); snapshot size+sha
  | <-- id, total_size, total_sha, chunk -|
  |  download_chunk(id, 0, chunk)         |
  | ------------------------------------> |  read@0; base64 + csha
  | <-------------- b64, csha ------------|
  |  ... 循环直到 offset==total_size ...    |
  |  本地 .part -> rename; verify sha256   |
```

## 7. 错误码（复用 File API 错误码表 + 传输特有）

| 错误码 | 触发场景 |
|---|---|
| `file.op_not_permitted` | policy.ops 不含 Upload/Download |
| `file.permission_denied` | grant file_access 不足（upload 需 read_write） |
| `file.out_of_scope` / `file.deny_pattern` / `file.symlink_escape` | 路径越界/命中 deny/软链逃逸 |
| `file.size_too_large` | total_size 超过 max_transfer_bytes |
| `file.precondition_failed` | 目标已存在且 overwrite=false；offset 与 part 大小不符；part 大小超过 total_size |
| `file.sha_mismatch` | per-chunk sha 或整文件 sha 校验失败 |
| `file.invalid_args` | 缺失 upload_id / offset / total_size 等；未知 upload_id |
| `file.not_found` | download 源不存在 |

## 8. 安全与策略门禁

- 复用 `FileAccessPolicy::check`：roots 白名单、`..` 逃逸拒绝、软链 resolve、deny/write_denies glob、op allowlist。
- Upload 走 write 门禁（`write_denies` 叠加 + `allow_overwrite`），Download 走 read 门禁。
- part 文件落在目标目录内（受同一 policy 约束），upload_id 随机不可预测。
- per-chunk + whole-file 双重 sha256，防止传输错误或中间篡改。
- `max_transfer_bytes` 限制单文件规模，防止磁盘耗尽。

## 9. 测试方案

### 9.1 单元测试
- policy：`Upload`/`Download` op gating、`Download` 只读 policy 通过 / `Upload` 只读 policy 拒绝、`max_transfer_bytes` 超限、`transfer_chunk_max_bytes` clamp 逻辑（请求 > 上限时夹取、请求 0 时回落默认）。
- 迁移：`ensure_ssh_fingerprint_grant_full_ops_in_place` 对含全部 legacy op 的 grant 自动追加 Upload/Download，对收窄 grant 不追加。
- server：per-chunk sha 校验失败返回 `file.sha_mismatch`；乱序 offset 返回 `file.precondition_failed`；whole-file sha 校验；resume offset 计算。
- CLI：`build_remote_file_command` 对 upload/download 构造正确 args；resume 状态文件读写；chunk 分块偏移数学。

### 9.2 E2E（e2e-test 技能）
- 起本地 bifrost（非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`），走 CLI contract。

### 9.3 真实场景测试（human_tests/remote-file-transfer.md）
- TC：> 10 MB 二进制上传下载往返 sha 一致；> 100 MB 大文件；压缩归档；中断后 `--resume` 续传完成。

## 10. Review/Fix/Test 闭环
- 第 1 轮：复核 op 门禁、chunk 预算、part 路径原子性、sha 校验路径；复跑 policy + server 单测。
- 第 2 轮：复查 resume 边界（offset 越界/重复）、CLI 分块数学、文档一致性；复跑 CLI 单测 + human_tests。

## 11. 校验要求
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`make coverage` ≥ 90%、rust-project-validate。

## 12. 文档更新
- 本文件、`human_tests/remote-file-transfer.md`、`human_tests/readme.md` 索引、`design/remote-invoke-file-api.md`（能力矩阵追加 upload/download）、README（如需）。

## 13. Phase 4.1 —— 传输吞吐 / 数据包优化

> **动机**：Relay 是内容无关的哑管道，载荷如何编码、压缩、节流完全由应用层决定。基础版分块传输每块一次阻塞式往返（RTT 绑定）、无压缩、且服务端严格 `offset == part_size` 顺序追加。以下三项优化在 **完全不改变 `.part` append-only 不变量与断点续传语义** 的前提下提升吞吐、压缩线上包体。

### 13.1 流水线 / 窗口化（Pipelining）

- **Caller**：不再逐块阻塞，改为保持一个有界的在途窗口（`UPLOAD_WINDOW` / `DOWNLOAD_WINDOW = 8`），用 `futures::buffer_unordered` 并发发起多块调用，把 Relay RTT 摊薄到整个窗口。每块用独立文件句柄按 offset 读取，互不干扰。
- **Server（上传落盘顺序）**：分块可能乱序到达，服务端用 **有界重排缓冲区**（`UploadWriteState.pending: BTreeMap<offset, bytes>`，上限 `MAX_PENDING_CHUNKS = 32`）：
  - `offset < part_size`：重复块，幂等 ack。
  - `offset > part_size`：领先于写前沿，缓冲（缓冲满则返回 `file.precondition_failed` 让 caller 收窄窗口）。
  - `offset == part_size`：落盘该块后，循环 drain 缓冲区中已连续的后继块。
  - 由此 `.part` 始终是文件的连续前缀，crash `--resume` 仍是「从 part 大小继续」，续传逻辑零改动。
- **Server（下载）**：`download_chunk` 在 eof 时**不再删除会话**（流水线下末块可能先于前块完成，删除会使在途请求失败）；空闲会话由 TTL 回收。
- **Caller（下载落盘顺序）**：并发取回的块先进本地 `BTreeMap` 重排缓冲，按写前沿连续 drain 写入本地 `.part`，保证本地文件同样是连续前缀（resume-safe）。
- `UPLOAD_WINDOW = 8 < MAX_PENDING_CHUNKS = 32`，突发也不会触发「收窄窗口」错误。

### 13.2 自适应逐块 zstd 压缩

- **协商**：begin 请求携带 `accept_encodings: ["zstd"]`。上传时服务端总能解 zstd,故据此回传 `chunk_encoding`；下载时仅当 caller 声明可解码才压缩,回传 `content_encoding`。
- **自适应**：`encode_chunk` 仅在 **压缩后确实更小** 时才用 zstd（level 3）并打 `chunk_encoding="zstd"`,否则原样返回 `"none"`——已压缩内容（jpg/mp4/tar.gz）永不被膨胀。
- **完整性与编码解耦**：`chunk_sha256` 始终对 **原始（解码后）字节** 计算,故完整性与线上编码无关。
- **解压炸弹防御**：`decode_chunk` 的 zstd 解压以协商 chunk_size 为上限（`cap`）,单块解压后不可能超过该值。
- **预算影响**:压缩只会缩小线上包体,绝不会把块推过 2 MiB frame 上限。

### 13.3 跳过相同文件（Skip-if-identical）

- `upload_begin` 在目标已存在且其 sha256 与源 `total_sha256` 一致时,直接短路返回 `already_complete: true`(附 `received_offset = total_size`),caller 跳过整个分块循环——幂等重推（如同一构建产物)变为单次往返、零分块。
- CLI 在 begin(及 stale-part abort 后的 re-begin)响应含 `already_complete` 时短路,human 输出 “already up to date”,JSON 输出附 `skipped: true`。

### 13.4 新增/变更的返回字段

| 方法 | 新增字段 |
|---|---|
| `upload_begin` | `chunk_encoding`("none"|"zstd")、`already_complete`(bool) |
| `upload_chunk` | `received_offset`(= 当前写前沿,配合乱序 ack) |
| `download_begin` | `content_encoding`("none"|"zstd") |
| `download_chunk` | `chunk_encoding`("none"|"zstd") |

### 13.5 Phase 4.1 测试

- 单元:`encode_chunk` 对可压缩数据打 zstd、对随机数据回落 none 且不膨胀;`decode_chunk` 拒绝未知编码、以 cap 限界;上传乱序块(4,3,2,1,0)经重排缓冲仍落成连续正确文件;`upload_begin` 相同文件短路;下载往返解 `chunk_encoding` 后逐块 + 整文件 sha 一致;chunk offset 分区数学。
- E2E(真实 caller→relay→target,见 `e2e-tests/tests/test_remote_file_relay_e2e.sh`):
  - `TC-FILE-XFER-03`:相同内容二次上传短路(`skipped=true` + sha 一致 + 目标不变)。
  - `TC-FILE-XFER-04`:高可压缩载荷经自适应 zstd 上传+下载往返,两端 sha256 逐字节一致。
