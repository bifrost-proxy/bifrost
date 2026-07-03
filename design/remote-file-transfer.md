# Remote 大文件上传/下载（Upload / Download）设计方案

> **状态**：已实现并持续演进（截至 2026-07-03）。基础分块协议、resume、Phase 4.1（pipelining + zstd + skip-identical）、Phase 4.2（fast-path + 自适应块大小）均已合并；Phase 4.3（去第二层 base64、delta transfer）作为独立后续 PR，尚未合并 (planned, not yet shipped as of 2026-07-03)。
> **依赖设计**：`design/remote-invoke-file-api.md`（File API 授权模型、FileOp、executor/file_ops 分层、错误码、SSH-key Full Trust op-set 迁移）。

## 背景

现有 File API (`file.write` / `file.read`) 通过一次 remote-invoke 调用整文件 base64 传输，受两个硬约束限制：

1. **Relay 单帧 POST body 上限 2 MiB**：Relay 通道对单个 remote-invoke 请求/响应包络有 2 MiB 硬上限（错误文案写的是 “max 1MB”，实际阈值 2 MiB）。`file.write` 把整文件 base64 塞进一次 `args_json`，任何超过约 700 KiB 原始字节的文件都会因 base64 膨胀（4/3 倍）+ JSON/POP 加密包络溢出而失败。
2. **策略字节上限**：`FileAccessPolicy.max_write_bytes` 默认 2 MiB，进一步限制单次写入。

因此新增 **分块（chunked）上传/下载** 能力：切成多个独立 sub-2MiB remote-invoke 调用，支持 **断点续传（resume）** 与 **端到端完整性校验（sha256）**。

## 用户目标验证清单

### 必须实现

- 能可靠传输 > 10 MB 二进制、> 100 MB 大文件、压缩归档。
- 传输中断后可用 `--resume` 从断点续传，无需重头再来。
- 上传/下载两端 sha256 逐块 + 整文件双重校验。
- 完全走真实 Relay 分块链路，复用现有授权模型（FileAccessPolicy + grant file_access）。
- 相同内容的文件二次上传短路（skip-if-identical）。
- 小文件走单次往返快速通道，节省 3+ RTT。
- 块大小按 RTT 自适应，弱链路用大块摊薄 RTT，快链路不做无谓放大。

### 必须不破坏

- 已有 `file.write` / `file.read` 小文件路径行为兼容。
- 已有 File API 权限模型（roots / write_denies / op allowlist）继续生效。
- 断点续传的 `.part` **append-only 连续前缀** 不变量。
- 现有 SSH-key Full Trust grant 无缝获得 Upload/Download 能力（对显式收窄的 grant 不追加）。

### 必须真实验证

- E2E `TC-FILE-XFER-01..07` 覆盖分块往返、resume、skip-identical、zstd 往返、fast-path、fast-path 回落、自适应块大小。
- human_tests 覆盖 > 100 MB 大文件真实场景。
- Grant 授权 + SSH-key Full Trust 迁移单测。

## 产品语义

### Chunk 尺寸预算

真正的约束是 Relay 的“单帧 POST body 上限”，而不是请求方向的调用上限。下载分块的数据是作为加密 call frame **回传**给 caller 的（target → relay `post_call_frame` → caller），bifrost-sync-server 的 `MAX_BODY_SIZE = 2 MiB` 会对该 POST body 直接 413 拒绝。

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

- 服务端 clamp 上限 `transfer_chunk_max_bytes` 默认 **1 MiB**（raw），保证回传 frame 稳定在 2 MiB 之下。
- 客户端默认请求 **512 KiB**，给包络与 JSON 留余量。
- 客户端可用 `--chunk-size` 请求任意值，服务端会对 **上传与下载两端** clamp 到 `min(requested, transfer_chunk_max_bytes)` 并把 `effective_chunk_size` 回传，客户端必须以回传值为准。

## 技术细节

### 核心层改动（`crates/bifrost-core/src/file_access/`）

**FileOp 枚举新增**：

```rust
pub enum FileOp {
    // ... 现有 Phase 1/2/3 ...
    Upload,    // caller -> remote 分块写
    Download,  // remote -> caller 分块读
}
```

- `Upload.is_write() == true`（写类，受 grant `file_access = read_write` 门禁）。
- `Download.is_write() == false`（读类，`file_access >= read` 即可）。

**FileAccessPolicy 新增字段**：

```rust
pub struct FileAccessPolicy {
    // ... 现有字段 ...
    pub max_transfer_bytes: u64,        // 默认 5 GiB
    pub transfer_chunk_max_bytes: u64,  // 默认 1 MiB
}
```

两者带 `#[serde(default = ...)]`，旧 TOML 无该字段时回落默认值。`new_readonly` 允许 `Download`，`new_read_write` 允许 `Upload` + `Download`。

**SSH-key Full Trust op-set 迁移**：

`file_policy_store.rs::full_file_ops()` 追加 `Upload` + `Download`。`legacy_ssh_full_file_ops()` **不含**这两个新 op —— 既有 SSH-key Full Trust grant 会被 `ensure_ssh_fingerprint_grant_full_ops_in_place` 自动追加 `Upload`/`Download`，被显式收窄的策略不变。机制与 `read_many`/`outline` 引入时相同。

### 服务端层（`crates/bifrost-admin/src/remote_invoke/file_transfer.rs`）

独立模块（避免继续膨胀已 5000+ 行的 `file_ops.rs`），实现全部 handler；`executor.rs` 负责 dispatch。

**会话状态（in-memory）**：

```rust
struct TransferSession {
    id: String,
    final_path: PathBuf,
    part_path: PathBuf,       // <dir>/.bifrost-upload.<id>.part
    total_size: u64,
    total_sha256: String,
    chunk_size: u64,          // effective
    prior_mode: Option<u32>,
    created_at: Instant,
    last_activity: Instant,
}
```

进程级 `OnceLock<Mutex<HashMap<String, TransferSession>>>` 跟踪。TTL 30 分钟无活动惰性清理。upload_id / download_id 随机 hex，防猜测。

### Upload 方法

| method | 参数 | 返回 | 源码定位 |
|---|---|---|---|
| `file.upload_begin` | path, total_size, total_sha256, chunk_size, overwrite, create_parents | upload_id, effective_chunk_size, received_offset, chunk_encoding, already_complete | `file_transfer.rs:305` |
| `file.upload_chunk` | upload_id, offset, chunk_b64, chunk_sha256 | next_offset, received_offset | `file_transfer.rs:706` |
| `file.upload_commit` | upload_id, total_sha256 | path, bytes_written, sha256 | `file_transfer.rs:797` |
| `file.upload_abort` | upload_id | aborted:true | `file_transfer.rs` |
| `file.upload_status` | upload_id | received_offset, total_size, chunk_size | `file_transfer.rs` |
| `file.upload_small` | path, total_size, total_sha256, chunk_b64, encoding | path, sha256（Phase 4.2 fast-path） | `file_transfer.rs:444` |

### Download 方法

| method | 参数 | 返回 | 源码定位 |
|---|---|---|---|
| `file.download_begin` | path, accept_encodings | download_id, total_size, total_sha256, effective_chunk_size, content_encoding | `file_transfer.rs:563` |
| `file.download_chunk` | download_id, offset, length | chunk_b64, chunk_sha256, chunk_encoding | `file_transfer.rs:890` |

### Phase 4.1 流水线 + 自适应 zstd + skip-identical

- **Pipelining**：`UPLOAD_WINDOW = 8`，`futures::buffer_unordered` 并发发起。服务端 `UploadWriteState.pending: BTreeMap<offset, bytes>` 有界重排缓冲（`MAX_PENDING_CHUNKS = 32`），保证 `.part` 始终是连续前缀。
- **自适应 zstd**：begin 携带 `accept_encodings: ["zstd"]`；`encode_chunk` 仅在压缩后更小时打 `chunk_encoding="zstd"`（level 3）。`chunk_sha256` 始终对**原始字节**计算。`decode_chunk` 以协商 chunk_size 为上限防解压炸弹。
- **Skip-if-identical**：`upload_begin` 在目标已存在且 sha 与源一致时返回 `already_complete: true`，caller 跳过整个循环，幂等重推变为单次往返、零分块。

### Phase 4.2 fast-path + 自适应块大小

- **`file.upload_small`**：单次 policy-checked 调用完成「解码(可选 zstd) → 校验整文件 sha256 → 原子 `.part` 写入 + rename」。复用与分块路径**完全相同**的 policy、overwrite、skip-identical、size 上限。
- **预算守卫**：caller 仅当文件 ≤ `SMALL_FILE_FASTPATH_MAX`（默认 512 KiB）才走快速通道；server 二次校验，预算不足时返回 `[file.precondition_failed]`（文案含 "fast-path budget"），caller **透明回落**分块协议。
- **自适应块大小**：caller 把本就必须发生的 begin 往返作为 RTT 探针（零额外往返）。auto 路径请求 `MAX_ADAPTIVE_CHUNK`（512 KiB）以探明 server 上限，再据 RTT 收敛：
  - `rtt ≤ 20ms` → `baseline`(512 KiB)。
  - `rtt ≥ 200ms` → `ceiling`(server 上限)。
  - 中间线性爬坡，按 `ADAPTIVE_GRANULE`(64 KiB) 取整，再 `clamp(floor, ceiling)`。
- 用户传 `--chunk-size` 时自适应关闭。

## CLI

```
bifrost remote file upload   <local> <remote> [--chunk-size N] [--overwrite] [--create-parents] [--resume] [--no-progress] [--cwd DIR]
bifrost remote file download <remote> <local> [--chunk-size N] [--resume] [--no-progress] [--cwd DIR]
```

编排位置：`crates/bifrost-cli/src/commands/remote/transfer.rs`（新独立模块，避免继续膨胀已 11000+ 行的 `commands/remote.rs`）。

- upload：本地按 `effective_chunk_size` 分块读 → base64 + per-chunk sha256 → 顺序调用 `upload_chunk`（失败重试 N 次）→ 进度条 → `upload_commit`。
- 断点续传：caller-side resume state 写在 `<local>.bifrost-upload-state.json`。`--resume` 时先调 `upload_status` / `upload_begin` 拿 `received_offset` 后续传。
- download：`download_begin` → 循环 `download_chunk` 写入 `<local>.part` → rename → 校验整文件 sha256。resume 用本地 `.part` 已有大小作为起始 offset。

## Web

不适用。传输链路仅通过 CLI 与 remote-invoke API 暴露。

## Admin API

- `POST /_bifrost/api/remote-invoke/exec`（`method = file.upload_begin | file.upload_chunk | file.upload_commit | file.upload_abort | file.upload_status | file.upload_small | file.download_begin | file.download_chunk`）。
- `GET /_bifrost/api/remote-invoke/file-access-config`（读取 policy，包含 `max_transfer_bytes` / `transfer_chunk_max_bytes`）。
- `PUT /_bifrost/api/remote-invoke/file-access-config`（写盘 + 缓存 invalidate）。

## Sync 边界

- 传输会话（`TransferSession`）纯内存，不 sync、不落盘（除 `.part` 临时文件）。
- Policy `max_transfer_bytes` / `transfer_chunk_max_bytes` 属本地策略，不 sync。

## Phase 1：基础分块协议（已完成）

- `FileOp::Upload/Download`、`FileAccessPolicy` 新字段。
- Server 侧 `upload_begin/chunk/commit/abort/status` + `download_begin/chunk`。
- CLI `bifrost remote file upload/download`。
- 顺序分块（`offset == part_size` 严格追加）。

## Phase 2：断点续传（已完成）

- Server 侧 `upload_status`，caller state 文件 `<local>.bifrost-upload-state.json`。
- Download 用本地 `.part` 大小作为起始 offset。

## Phase 3：SSH-key Full Trust 迁移（已完成）

- `ensure_ssh_fingerprint_grant_full_ops_in_place` 自动追加 Upload/Download。

## Phase 4.1：流水线 + zstd + skip-identical（已完成）

- Pipelining window 8 + 有界重排缓冲 32。
- 自适应 zstd，`chunk_sha256` 对解码后原始字节计算。
- `upload_begin` skip-if-identical。

## Phase 4.2：fast-path + 自适应块大小（已完成）

- `file.upload_small` 单次往返。
- Caller 侧 RTT 探针 + 自适应块大小。

## Phase 4.3：（后续 PR，本分支范围外）

- 去掉冗余的第二层 base64（P1-#4，触碰共享 remote_invoke 包络层，独立 PR）。
- delta / 增量传输（P2-#7 rsync 式 + P2-#8 块级去重，合并为单个 PR，会破坏 `.part` append-only 前缀不变量）。

状态：(planned, not yet shipped as of 2026-07-03)。

## 协议时序

### Upload（含 resume）

```
Caller                                  Remote
  |  upload_begin(path,size,sha,chunk)    |
  | ------------------------------------> |  policy.check(Upload); clamp chunk
  | <------ upload_id, eff_chunk, recv=0 -|
  |                                       |
  |  upload_chunk(id,0,b64,csha)          |
  | ------------------------------------> |  verify csha; append @0
  | <------------------- next_offset=... -|
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

### Download

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

## 错误码

| 错误码 | 触发场景 |
|---|---|
| `file.op_not_permitted` | policy.ops 不含 Upload/Download |
| `file.permission_denied` | grant file_access 不足（upload 需 read_write） |
| `file.out_of_scope` / `file.deny_pattern` / `file.symlink_escape` | 路径越界/命中 deny/软链逃逸 |
| `file.size_too_large` | total_size 超过 max_transfer_bytes |
| `file.precondition_failed` | 目标已存在且 overwrite=false；offset 与 part 大小不符；part 大小超过 total_size；fast-path 预算不足 |
| `file.sha_mismatch` | per-chunk sha 或整文件 sha 校验失败 |
| `file.invalid_args` | 缺失 upload_id / offset / total_size；未知 upload_id |
| `file.not_found` | download 源不存在 |

## 测试方案

### 单元测试

- Policy：`Upload`/`Download` op gating、`max_transfer_bytes` 超限、`transfer_chunk_max_bytes` clamp。
- 迁移：`ensure_ssh_fingerprint_grant_full_ops_in_place` 对含全部 legacy op 的 grant 自动追加 Upload/Download。
- Server：per-chunk sha 校验失败、乱序 offset、whole-file sha、resume offset。
- Fast-path：`upload_small` 空文件 / 1 字节、预算边界、坏 sha / 错 size、zstd 载荷往返、overwrite + skip-identical。
- Caller：`read_chunk_at`、`upload_small_args` 字段形状、fast-path budget 回落、`plan_chunk_request` 显式/auto、自适应块在快/慢/中链路的 baseline/ceiling/单调有界。

### E2E（`e2e-tests/tests/test_remote_file_relay_e2e.sh`）

- `TC-FILE-XFER-01`（第 2335 行）：分块 upload + download 往返，sha256 一致。
- `TC-FILE-XFER-02`（第 2400 行）：模拟中断后 `--resume` 续传成功。
- `TC-FILE-XFER-03`（第 2452 行）：skip-if-identical 短路（`skipped=true`）。
- `TC-FILE-XFER-04`：高可压缩载荷自适应 zstd 上传+下载往返 sha 一致。
- `TC-FILE-XFER-05`：小文件 ≤ 512 KiB 单次往返上传下载；空文件与恰好预算边界。
- `TC-FILE-XFER-06`：恰好超过 fast-path 预算的文件透明回落分块，sha 一致。
- `TC-FILE-XFER-07`：auto 路径自适应上传大文件，`effective_chunk_size` 合法；显式 wire-safe `--chunk-size` 被逐字尊重。

### human_tests

- `human_tests/remote-file-transfer.md`：> 10 MB 二进制、> 100 MB 大文件、压缩归档、中断+`--resume`。
- `human_tests/remote-invoke-file.md`：与既有 File API 联动。

## Review / Fix / Test 闭环

- 改动 `file_transfer.rs`：
  1. 补齐 `mod tests` 分支。
  2. 若涉及 wire 字段变化，更新本文档 §「Upload 方法」/「Download 方法」表格。
  3. 追加对应 `TC-FILE-XFER-*` E2E 用例。
- 改动 policy 或 clamp：
  1. 覆盖新旧默认值兼容单测。
  2. `save_raw_config` 主动 invalidate 缓存的路径必须被测试触及。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo test -p bifrost-admin remote_invoke::file_transfer`
- `bash e2e-tests/tests/test_remote_file_relay_e2e.sh`
- `make coverage` ≥ 90%

## 风险与决策

### 1. Frame body 上限溢出

- 风险：过大 chunk 使回传 frame > 2 MiB，Relay 413。
- 决策：Server clamp `transfer_chunk_max_bytes = 1 MiB`；caller 默认请求 512 KiB；auto 路径 ceiling 尊重 server 回传值。

### 2. `.part` 状态被并发扰动

- 风险：pipelining 并发块打乱 append-only 前缀不变量。
- 决策：有界重排缓冲 `MAX_PENDING_CHUNKS = 32`；`UPLOAD_WINDOW = 8 < 32` 保证不触发「收窄窗口」错误。

### 3. 压缩后反而变大

- 风险：已压缩内容（jpg/mp4/tar.gz）经 zstd 反而膨胀。
- 决策：`encode_chunk` 仅在压缩后确实更小时才用 zstd，否则原样返回 `"none"`。

### 4. 解压炸弹

- 风险：恶意小 zstd 块解压后占用大内存。
- 决策：`decode_chunk` 以协商 `chunk_size` 为上限（`cap`），单块解压后不可能超过该值。

### 5. Fast-path 无法承载

- 风险：文件恰好超预算，caller 已经发起 `upload_small` 后失败。
- 决策：Server 二次校验，返回 `[file.precondition_failed]` 且文案含 "fast-path budget"，caller 透明回落分块。

### 6. 幂等重推浪费带宽

- 风险：相同构建产物反复推送。
- 决策：`upload_begin` skip-if-identical 短路 `already_complete: true`，单次往返、零分块。

### 7. Delta transfer 引入协议破坏

- 风险：sparse-`.part` 违反 append-only 前缀不变量，破坏 resume。
- 决策：Delta / 块级去重合并为单独 PR，本分支不落地；先稳固分块 + fast-path + 自适应块。

## 文档更新

- 本文件、`human_tests/remote-file-transfer.md`、`human_tests/readme.md` 索引、`design/remote-invoke-file-api.md`（能力矩阵追加 upload/download）、README（如需）。
