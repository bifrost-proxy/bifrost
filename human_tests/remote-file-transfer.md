# Remote 大文件上传/下载（Upload / Download）— 人工测试用例

> 关联设计：[`design/remote-file-transfer.md`](../design/remote-file-transfer.md)
> 关联分支：`codex/remote-file-transfer`
> 场景范围：`bifrost remote file upload` / `download` 分块传输、断点续传、端到端 sha256 校验
> 硬性约束：本能力涉及跨设备大文件传输，必须在真实设备上、走真实 Relay 分块链路人工执行；严禁仅依赖单测/mock 判定完成。>10 MB 二进制、>100 MB 大文件、压缩归档、中断续传四类场景为必测项。

---

## 0. 前置准备

1. **两台机器**：
   - `host-A`（调用端）：安装本 PR 构建的 `bifrost` CLI。
   - `host-B`（被控端）：运行 `bifrost` daemon（本 PR 构建产物，含新增的 `file.upload_*` / `file.download_*` handler），已通过 SSH key 或配对码授权 host-A。
2. **策略文件**：`host-B` 数据目录下的 `file-access.toml`（默认 `~/.bifrost/file-access.toml`；若设置 `BIFROST_DATA_DIR` 则写入 `$BIFROST_DATA_DIR/file-access.toml`），需允许 `upload` / `download` 两个 op：
   ```toml
   [[grant]]
   grant_id = "<目标 grant_id>"
   name = "workspace-read-write"
   roots = ["<USER_HOME>/work/github/bifrost-xfer-sandbox"]
   denies = ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"]
   ops = ["read", "read_many", "list", "stat", "glob", "search", "hash", "outline", "write", "edit", "mkdir", "move", "delete", "apply_patch", "upload", "download"]
   max_read_bytes = 2_097_152
   max_write_bytes = 2_097_152
   max_transfer_bytes = 5_368_709_120   # 5 GiB — 单文件传输上限
   transfer_chunk_max_bytes = 1_048_576 # 1 MiB — 服务端 chunk clamp 上限
   respect_gitignore = true
   allow_overwrite = true
   allow_recursive_delete = false
   ```
   > 说明：既有 SSH-key Full Trust grant 会由 `ensure_ssh_fingerprint_grant_full_ops_in_place` 自动追加 `upload`/`download`，无需手工补写；被显式收窄 `ops` 的 grant 才需要显式加上这两项。
3. **Grant**：host-A 通过 `bifrost remote grant update <grant_id> --file-access read_write` 授予读写（`upload` 走写门禁，`download` 走读门禁）。只读拒绝用例再降级到 `--file-access read`。

---

## 1. 大二进制往返（> 10 MB）

### TC-XFER-1.1 上传 + 下载 > 10 MB 二进制并双端 sha 校验
**步骤**（host-A）：
```bash
# 造一个 24 MiB 的伪随机二进制（不可压缩，避免相同块掩盖顺序 bug）
head -c 25165824 /dev/urandom > /tmp/xfer-24m.bin
SRC_SHA=$(shasum -a 256 /tmp/xfer-24m.bin | awk '{print $1}')

bifrost remote file upload /tmp/xfer-24m.bin xfer-24m.bin \
    --create-parents --overwrite \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox

bifrost remote file download xfer-24m.bin /tmp/xfer-24m.dl.bin \
    --overwrite \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
DL_SHA=$(shasum -a 256 /tmp/xfer-24m.dl.bin | awk '{print $1}')
```
**期望**：
- upload 结束打印 `commit` 结果，返回的整文件 `sha256 == SRC_SHA`；
- host-B 目标磁盘上的 `xfer-24m.bin` 逐字节等于源（在 host-B 上 `shasum -a 256` 亦等于 `SRC_SHA`）；
- `DL_SHA == SRC_SHA`；
- 传输过程默认展示进度；`--no-progress` 时无进度输出但结果一致；
- 由于默认 chunk（512 KiB）远小于 24 MiB，本用例实际经历约 48 个 `upload_chunk` / `download_chunk` 调用，验证多块顺序拼接正确。

### TC-XFER-1.2 自定义 chunk size
**步骤**：对上例追加 `--chunk-size 262144`（256 KiB）与 `--chunk-size 4194304`（请求 4 MiB，超过服务端 1 MiB clamp 上限）。
**期望**：
- 两种请求均传输成功、sha 一致；
- 请求 4 MiB 时服务端 clamp 到 1 MiB 并回传 `effective_chunk_size = 1048576`，客户端以回传值分块（不报错、不溢出 Relay body 上限）。

---

## 2. 超大文件（> 100 MB）

### TC-XFER-2.1 上传 + 下载 > 100 MB 文件
**步骤**：
```bash
head -c 157286400 /dev/urandom > /tmp/xfer-150m.bin   # 150 MiB
SRC_SHA=$(shasum -a 256 /tmp/xfer-150m.bin | awk '{print $1}')

bifrost remote file upload /tmp/xfer-150m.bin xfer-150m.bin \
    --create-parents --overwrite \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox

bifrost remote file download xfer-150m.bin /tmp/xfer-150m.dl.bin \
    --overwrite --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
```
**期望**：
- 传输全程稳定（数百个 chunk），无 Relay `413` / body 超限错误；
- 双端 sha256 一致；
- 峰值内存受控（单块内存占用 ~ chunk size 量级，不整文件驻留）；
- 大于策略 `max_transfer_bytes` 的文件（如把 `max_transfer_bytes` 临时调到 100 MiB 再传 150 MiB）在 `upload_begin` 阶段即被拒绝，错误码 `file.size_too_large`。

---

## 3. 压缩归档

### TC-XFER-3.1 上传 + 下载 tar.gz 归档并解压校验
**步骤**：
```bash
mkdir -p /tmp/xfer-arc/{a,b} && head -c 5242880 /dev/urandom > /tmp/xfer-arc/a/f1.bin && head -c 8388608 /dev/urandom > /tmp/xfer-arc/b/f2.bin
tar -czf /tmp/xfer-arc.tar.gz -C /tmp/xfer-arc .
SRC_SHA=$(shasum -a 256 /tmp/xfer-arc.tar.gz | awk '{print $1}')

bifrost remote file upload /tmp/xfer-arc.tar.gz xfer-arc.tar.gz \
    --create-parents --overwrite --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
bifrost remote file download xfer-arc.tar.gz /tmp/xfer-arc.dl.tar.gz \
    --overwrite --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
```
**期望**：
- 归档往返 sha256 一致；
- host-B 上 `tar -tzf xfer-arc.tar.gz` 可正常列出条目、`tar -xzf` 可无损解压，内部文件 sha 与源目录一致（证明二进制归档在分块传输中未损坏）。

---

## 4. 断点续传（resume）

### TC-XFER-4.1 上传中断后 --resume 续传
**步骤**：
```bash
head -c 104857600 /dev/urandom > /tmp/xfer-resume.bin   # 100 MiB
SRC_SHA=$(shasum -a 256 /tmp/xfer-resume.bin | awk '{print $1}')

# 第一趟：传到中途用 Ctrl-C / kill 打断（或 --chunk-size 调小拉长传输窗口）
bifrost remote file upload /tmp/xfer-resume.bin xfer-resume.bin \
    --create-parents --overwrite --resume --chunk-size 262144 \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
#   ↑ 传输进行到约一半时中断进程

# 第二趟：--resume 从断点续传
bifrost remote file upload /tmp/xfer-resume.bin xfer-resume.bin \
    --overwrite --resume --chunk-size 262144 \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
```
**期望**：
- 第二趟启动时先调用 `upload_status` / `upload_begin`，从 `received_offset`（≈ 中断时已写入字节）继续，而不是从 0 重传（可通过传输字节数/耗时明显小于全量确认）；
- host-B 上留存的 `.bifrost-upload.<id>.part` 临时文件在 `upload_commit` 成功后被原子 rename 为最终文件并清理；
- 最终整文件 sha256 == `SRC_SHA`。

### TC-XFER-4.2 下载中断后 --resume 续传
**步骤**：对已上传的 `xfer-resume.bin` 执行 `download ... --resume`，第一趟中途中断，第二趟 `--resume`。
**期望**：
- 第二趟以本地 `<local>.part` 已有大小为起始 offset 续传；
- 完成后本地 `.part` rename 为最终文件，sha256 与远端 `download_begin` 返回的 `total_sha256` 一致。

---

## 5. 授权与错误路径

### TC-XFER-5.1 只读 grant 拒绝上传
**步骤**：`bifrost remote grant update <grant_id> --file-access read` 后执行 `upload`。
**期望**：`upload_begin` 被拒绝，错误码 `file.permission_denied`（`upload` 属写类）。

### TC-XFER-5.2 只读 grant 允许下载
**步骤**：只读 grant 下执行 `download`。
**期望**：正常成功（`download` 属读类，`file_access >= read` 即可）。

### TC-XFER-5.3 op 被显式收窄时拒绝
**步骤**：策略 `ops` 去掉 `upload`/`download`，分别执行两命令。
**期望**：均返回 `file.op_not_permitted`。

### TC-XFER-5.4 越界 / deny 路径拒绝
**步骤**：上传目标指向 roots 之外、或命中 `denies`（如 `**/*.key`）、或经软链逃逸的路径。
**期望**：分别返回 `file.out_of_scope` / `file.deny_pattern` / `file.symlink_escape`，且不产生任何 `.part` 残留。

### TC-XFER-5.5 目标已存在且未 --overwrite
**步骤**：对已存在的远端文件执行不带 `--overwrite` 的 `upload`。
**期望**：`upload_begin` 返回 `file.precondition_failed`，原文件不被破坏。

### TC-XFER-5.6 完整性校验失败
**步骤**（构造性/白盒）：注入一块被篡改的 chunk（错误 `chunk_sha256`）或整文件 sha 不符。
**期望**：per-chunk 校验失败即返回 `file.sha_mismatch` 并保留 part 供重试；`upload_commit` 阶段整文件 sha 不符同样返回 `file.sha_mismatch`，不会 rename 出损坏文件。

---

## 6. 技能安装源同步

### TC-XFER-6.1 `install-skill` 嵌入的两个 skill 文件包含新命令
**步骤**（仓库根目录）：
```bash
rg -n "remote file upload|remote file download|file\\.upload_\\*|file\\.download_\\*" SKILL.md skill_remote.md
cargo test -p bifrost-cli install_skill_installs_remote_skill_from_embedded_bundle -- --nocapture
```
**期望**：
- `SKILL.md` 和 `skill_remote.md` 都包含 `bifrost remote file upload` / `bifrost remote file download` 的命令说明；
- `skill_remote.md` 明确大文件、压缩包、需要断点续传的二进制优先走 `remote file upload/download --resume`，而不是 `write --from-local` 或 `remote exec + base64`；
- `install_skill_installs_remote_skill_from_embedded_bundle` 通过，证明 `install-skill` 嵌入源会同时安装更新后的 `bifrost` 与 `bifrost-remote` skill。

---

## 7. Phase 4.1 —— 传输吞吐 / 数据包优化

> 关联设计：`design/remote-file-transfer.md` 第 13 节。这些优化在不改变 `.part` append-only 不变量与断点续传语义的前提下，提升吞吐、压缩线上包体、并对幂等重推短路。人工验证以「结果一致 + 行为可观察」为准。

### TC-XFER-7.1 流水线窗口化不破坏顺序与续传
**步骤**：对 TC-XFER-2.1 的 150 MiB 伪随机文件（不可压缩，避免相同块掩盖乱序 bug）执行 upload + download；传输中途 kill 一次再 `--resume` 续传。
**期望**：
- 传输显著快于逐块阻塞的旧行为（窗口 8 并发在途，Relay RTT 被摊薄）；
- 双端 sha256 逐字节一致，证明服务端有界重排缓冲（`MAX_PENDING_CHUNKS = 32`）把乱序到达的块按写前沿落成连续前缀；
- 中断后 host-B 的 `.part` 仍是文件的连续前缀，`--resume` 从 part 大小继续，无需重传已落盘部分；
- 全程无 `file.precondition_failed`（`UPLOAD_WINDOW = 8 < MAX_PENDING_CHUNKS = 32`，突发不会触发收窄窗口）。

### TC-XFER-7.2 自适应 zstd —— 高可压缩载荷线上包体缩小
**步骤**：
```bash
# ~2 MiB 高可压缩内容（长零串 + 重复文本）
head -c 1048576 /dev/zero > /tmp/xfer-zstd.bin
{ yes "the quick brown fox jumps over the lazy dog 0123456789" || true; } | head -c 1048576 >> /tmp/xfer-zstd.bin
SRC_SHA=$(shasum -a 256 /tmp/xfer-zstd.bin | awk '{print $1}')

bifrost remote file upload /tmp/xfer-zstd.bin xfer-zstd.bin \
    --chunk-size 65536 --create-parents --overwrite \
    --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
bifrost remote file download xfer-zstd.bin /tmp/xfer-zstd.dl.bin \
    --chunk-size 65536 --overwrite --cwd <USER_HOME>/work/github/bifrost-xfer-sandbox
```
**期望**：
- 双端 sha256 == `SRC_SHA`（完整性对 **原始字节** 计算，与线上编码解耦）；
- 上传/下载协商 `chunk_encoding = "zstd"`，线上包体明显小于 raw + base64；
- 解码以协商 chunk_size 为上限（解压炸弹防御），单块解压后不超过该值。

### TC-XFER-7.3 已压缩内容不被 zstd 膨胀
**步骤**：对 TC-XFER-3.1 的 `tar.gz`（或任意 jpg/mp4）执行 upload。
**期望**：
- `encode_chunk` 检测到 zstd 压缩后未变小，回落 `chunk_encoding = "none"`，线上包体不膨胀；
- 双端 sha256 一致。

### TC-XFER-7.4 跳过相同文件（skip-if-identical）
**步骤**：对同一文件连续执行两次 `upload`（内容与目标 sha 一致）。
**期望**：
- 第二次 `upload_begin` 直接短路返回 `already_complete: true`（`received_offset = total_size`），caller 跳过整个分块循环；
- human 输出 “already up to date”，`--output json` 附 `skipped: true`；
- host-B 目标文件不被改动（mtime/inode 或内容不变）。

---

## 8. 自动化对照

上述真实场景与 `e2e-tests/tests/test_remote_file_relay_e2e.sh` 中以下用例保持一致；该脚本自建 relay + target + caller，走真实数据路径：
- `TC-FILE-XFER-01`：多块 upload/download 往返 + 双端 sha256；
- `TC-FILE-XFER-02`：`--resume` 续传提交；
- `TC-FILE-XFER-03`：相同内容二次上传短路（`skipped = true` + sha 一致 + 目标不变）；
- `TC-FILE-XFER-04`：高可压缩载荷经自适应 zstd upload + download 往返，两端 sha256 逐字节一致。

人工执行时以本文档的大文件 / 压缩归档 / 中断续传 / 流水线规模场景为准，覆盖自动化脚本因体量受限未覆盖的规模边界。
