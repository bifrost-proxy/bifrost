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

## 14. Phase 4.2 —— 单次往返快速通道 / 自适应块大小

> **动机**:Phase 4.1 的分块协议对**每一个**上传都要求「begin → 逐块 → finish」至少三段往返;但绝大多数配置/脚本/小产物只有几十 KiB,一帧就能装下。同时 chunk 大小此前是编译期常量(512 KiB),对高带宽链路欠切分、对弱链路又可能逼近 relay frame 上限。本阶段两项优化 **完全在 caller 侧 + 一个新 server op** 完成,不改 `.part` append-only 不变量、不改断点续传语义、不改现有分块协议。

### 14.1 小文件快速通道(P1-#5,`file.upload_small`)

- **新增 server op `file.upload_small`**:单次 policy-checked 调用完成「解码(可选 zstd)→ 对**原始字节**校验整文件 sha256 → 原子 `.part` 写入 + rename」。复用与分块路径**完全相同**的 policy 决策、overwrite/create_parents 门禁、skip-if-identical 短路、size 上限。
- **预算守卫**:caller 仅当文件 ≤ `SMALL_FILE_FASTPATH_MAX`(默认 512 KiB,为 base64 + remote-invoke envelope 预留 relay body 余量)才走快速通道;server 仍用 policy 单帧预算二次校验,预算不足时返回 `[file.precondition_failed]`(文案含 "fast-path budget"),caller **透明回落**到分块协议。
- **完整性**:sha256 始终对解码后的**原始**字节计算,与线上是否 zstd 无关;写入沿用 sha-keyed `.part` + rename,失败清理,与分块路径一致的原子性保证。
- **Caller**:`run_upload` 在 `!resume && total_size <= SMALL_FILE_FASTPATH_MAX` 时先试快速通道;仅当错误信号为 "fast-path budget" 时回落分块,其他错误直接上抛(不吞错)。

### 14.2 自适应块大小(P1-#6,纯 caller 侧)

- **无需改协议**:server 的 `clamp_chunk_size(requested, policy)` 早已接受 caller 请求的块大小并夹到 `[1, transfer_chunk_max_bytes]`,`upload_chunk`/`download_chunk` 均按请求大小逐块处理并回传 `effective_chunk_size`。因此自适应**完全是 caller 侧**决策。
- **RTT 探针**:caller 把**本就必须发生**的 begin 往返计时作为 RTT 探针(零额外往返)。auto 路径请求 `MAX_ADAPTIVE_CHUNK`(512 KiB)以探明 server 上限(`effective_chunk_size`),再据 RTT 收敛:
  - `rtt ≤ FAST_RTT_MS(20ms)` → `baseline`(512 KiB),快链路无谓放大无收益。
  - `rtt ≥ SLOW_RTT_MS(200ms)` → `ceiling`(server 上限),弱链路用最大块摊薄 RTT。
  - 中间线性爬坡,按 `ADAPTIVE_GRANULE`(64 KiB)取整,再 `clamp(floor, ceiling)`。
- **显式优先**:用户传 `--chunk-size` 时自适应关闭;caller 会先将显式值夹到 wire-safe ceiling,再进入 server clamp,避免协商出 relay 无法承载的块。
- **上限安全**:`MAX_ADAPTIVE_CHUNK = 512 KiB`,不可压缩载荷经 base64 + remote-invoke envelope 膨胀后仍低于 CI relay 的 1 MiB open-call body 限制。下载走同一自适应路径。

### 14.3 Phase 4.2 测试

- 单元(server):`upload_small` 写入并校验、空文件 / 1 字节、预算边界(`== budget` 走通、`> budget` 拒绝)、超大先于预算被拒、坏 sha / 错 size 拒绝、zstd 载荷往返、未知编码拒绝、overwrite 门禁 + skip-identical、create_parents、非 hex sha 的 `.part` 命名字符安全。
- 单元(caller):`read_chunk_at` 零长度、`upload_small_args` 字段形状、fast-path 仅在 budget 信号回落、阈值等于 default chunk、`plan_chunk_request` 显式/auto、自适应块在快/慢/中链路的 baseline/ceiling/单调有界、小 ceiling 夹取、floor 下限保护。
- E2E(真实 caller→relay→target):
  - `TC-FILE-XFER-05`:小文件(≤ 512 KiB)单次往返上传下载,sha 一致;空文件与恰好预算边界文件。
  - `TC-FILE-XFER-06`:恰好超过快速通道预算的文件透明回落分块,sha 一致。
  - `TC-FILE-XFER-07`:auto 路径(不带 `--chunk-size`)自适应上传大文件,`effective_chunk_size` 合法且落盘 sha 一致;显式 wire-safe `--chunk-size` 被逐字尊重。

## 15. 后续 PR(本分支范围外,已作用域界定)

以下三项经评估会触碰**共享包络层**或破坏当前 `.part` **append-only 前缀不变量**,故不在本分支实现,各自作为独立后续 PR 交付,以控制本分支的爆炸半径(仅限文件传输)。

### 15.1 去掉冗余的第二层 base64(P1-#4)——独立 PR

- 现状:下载回传路径对一块原始字节 **两次 base64 膨胀**(见 §2),第二层来自 remote-invoke 包络把 stdout JSON 再次 base64。去掉第二层可把线上包体从 ~2x 降到 ~1.34x,直接放宽单帧能装的原始字节。
- **为何独立 PR**:第二层 base64 位于 `remote_invoke` **共享包络层**,被**所有** remote op(exec / file.* / 等)复用,不是文件传输独有。在本分支改动会把爆炸半径扩大到全部远程能力,需独立 PR + 全 op 回归。

### 15.2 delta / 增量传输(P2-#7 rsync 式 + P2-#8 块级去重)——合并为单个「delta transfer」PR

- P2-#7(rsync 式增量)与 P2-#8(块级去重)本质是**同一套机制**:都需要 server 侧**块清单交换**(target 现有文件的固定/滚动窗口块 sha)+ 从现有 target **随机偏移拼接**未变块进 `.part`。
- **为何独立 PR 且合并**:随机偏移拼接**直接违反**当前 `.part` **append-only 连续前缀不变量**——而断点续传(`received_offset = part 大小`)与 P0-1 乱序重排缓冲都依赖该不变量。引入 delta 需要新的 sparse-`.part` 状态机与块清单协议(`file.upload_probe_blocks` 类新 op),是比 Phase 4.1/4.2 重得多的协议面。两者共享清单+拼接机制,合并为单个 PR 避免重复造轮子。
