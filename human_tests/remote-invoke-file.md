# Remote Invoke File API — 人工测试用例（Phase 1/2/3）

> 关联设计：[`design/remote-invoke-file-api.md`](../design/remote-invoke-file-api.md)
> 关联分支：`feat/remote-file-api`
> 场景范围：Phase 1 只读能力 + Phase 2 写/编辑能力 + Phase 3 apply-patch
> 硬性约束：本技能涉及跨设备文件访问，所有用例必须在真实设备上由人工执行；严禁仅依赖单测通过即判定完成。

---

## 0. 前置准备

1. **两台机器**：
   - `host-A`（调用端）：安装 `bifrost` CLI（本 PR 构建产物）。
   - `host-B`（被控端）：运行 `bifrost` daemon，且已通过 SSH key 或配对码授权 host-A。
2. **策略文件**：`host-B` 的数据目录下预置 `file-access.toml`（默认路径为 `~/.bifrost/file-access.toml`；测试时如设置了 `BIFROST_DATA_DIR`，则写入 `$BIFROST_DATA_DIR/file-access.toml`）：
   ```toml
   [[grant]]
   grant_id = "<目标 grant_id>"
   name = "workspace-read-write"
   roots = ["/Users/eden/work/github/bifrost-remote-file"]
   denies = ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"]
   write_denies = ["**/Cargo.lock", "**/*.lock"]
   ops = ["read", "list", "stat", "glob", "search", "hash", "write", "edit", "mkdir", "move", "delete", "apply_patch"]
   max_read_bytes = 2_097_152    # 2 MiB
   max_write_bytes = 2_097_152
   respect_gitignore = true
   allow_overwrite = true
   allow_recursive_delete = false
   ```
3. **Grant**：host-A 本地通过 `bifrost remote grant update <grant_id> --scope remote_file_write` 为目标 grant 设置文件读写作用域；只读拒绝用例中再降级到 `remote_file_read`。

---

## 1. 正向用例

### TC-1.1 file.read — 读取文本文件
**步骤**：
```bash
bifrost remote file read README.md --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- CLI 输出为 JSON，包含 `content_b64 / size / total_size / sha256 / mtime_unix / truncated`；
- `content_b64` 解码后与 host-B 上 README 内容一致；
- `truncated = false`。

### TC-1.2 file.read — 超大文件截断
**步骤**：准备一个 5 MiB 的文件 `crates/bifrost-core/big.txt`，执行：
```bash
bifrost remote file read crates/bifrost-core/big.txt --max-bytes 1048576
```
**期望**：
- `content` 长度 = 1 MiB（解码后）；
- `meta.truncated = true`，`meta.total_size = 5242880`。

### TC-1.3 file.list — 列目录（浅层）
```bash
bifrost remote file list crates --depth 1
```
**期望**：返回 11 个工作区 crate 目录，每项包含 `name / type / size / mtime`；`.git` 等 deny 目录不出现在结果中。

### TC-1.4 file.stat — 获取元信息
```bash
bifrost remote file stat Cargo.toml
```
**期望**：返回 `size / mtime / mode / sha256 / kind=file`。

### TC-1.5 file.glob — 匹配
```bash
bifrost remote file glob 'crates/*/Cargo.toml'
```
**期望**：返回 11 条匹配。

### TC-1.6 file.search — 内容检索
```bash
bifrost remote file search 'GrantScope' --path crates/bifrost-admin --max-matches 20
```
**期望**：至少命中 `remote_invoke/types.rs`；返回 `{path, line, column, preview}`；不包含二进制文件命中。

### TC-1.7 file.hash — 校验
```bash
bifrost remote file hash Cargo.lock --algo sha256
```
**期望**：返回 64 位 hex；与 host-B 本地 `shasum -a 256 Cargo.lock` 一致。

### TC-1.8 file.write — 从本地文件写入远端文件
**步骤**：
```bash
printf 'hello remote file\n' > /tmp/bifrost-file-write.txt
bifrost remote file write agent-write.txt --content-file /tmp/bifrost-file-write.txt --cwd /Users/eden/work/github/bifrost-remote-file
bifrost remote file read agent-write.txt --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- `write` 返回 `bytes_written / sha256 / previous_sha256`；
- 后续 `read` 解码内容为 `hello remote file\n`。

### TC-1.9 file.edit — 行范围编辑
**步骤**：
```bash
bifrost remote file edit agent-write.txt --edits '[{"start_line":1,"end_line":1,"replacement":"edited remote file\n"}]' --cwd /Users/eden/work/github/bifrost-remote-file
bifrost remote file read agent-write.txt --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- `edit` 返回 `applied_edits = 1` 和新的 `sha256`；
- 后续 `read` 解码内容为 `edited remote file\n`。

### TC-1.10 file.mkdir / file.mv / file.rm — 目录与路径变更
**步骤**：
```bash
bifrost remote file mkdir agent-dir/nested --parents --cwd /Users/eden/work/github/bifrost-remote-file
bifrost remote file mv agent-write.txt agent-dir/nested/moved.txt --cwd /Users/eden/work/github/bifrost-remote-file
bifrost remote file rm agent-dir/nested/moved.txt --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- `mkdir` 后目录存在；
- `mv` 后源文件不存在，目标文件存在；
- `rm` 后目标文件不存在。

### TC-1.11 file.apply-patch — 应用 unified diff
**步骤**：
```bash
cat >/tmp/bifrost-file.patch <<'PATCH'
--- a/patch-target.txt
+++ b/patch-target.txt
@@ -0,0 +1 @@
+patched by remote file
PATCH
bifrost remote file apply-patch --patch-file /tmp/bifrost-file.patch --cwd /Users/eden/work/github/bifrost-remote-file
bifrost remote file read patch-target.txt --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- `apply-patch` 返回 `applied` 列表；
- `read` 解码内容包含 `patched by remote file`。

---

## 2. 安全 / 拒绝用例

### TC-2.1 越界路径被拒（deny list）
```bash
bifrost remote file read .git/config
```
**期望**：`error.code = file.permission_denied`，`message` 提示命中 deny 规则；无任何字节返回。

### TC-2.2 越过 roots
```bash
bifrost remote file read /etc/passwd
```
**期望**：`error.code = file.out_of_scope`，审计日志记录 grant_id / scope / attempted_path。

### TC-2.3 Symlink 跨越 roots
在 `host-B` 上创建 `ln -s /etc/passwd crates/link-passwd`，执行：
```bash
bifrost remote file read crates/link-passwd
```
**期望**：`error.code = file.symlink_escape`。

### TC-2.4 scope 缺失
将 grant 降级为 `remote_shell_exec` 后执行任一 `file.*` 命令。
**期望**：调用被拒绝，错误信息包含 `grant_scope_mismatch` 或 `does not allow command kind`；shell scope 不自动授予 file API。

### TC-2.5 只读 policy 拒绝写操作（回归）
**步骤**：
```bash
CARGO_TARGET_DIR=./.codex-target/remote-file-readonly-policy cargo run -p bifrost-e2e -- --test remote_file_readonly_policy_rejects_write_op
```
**期望**：
- 用例 `remote_file_readonly_policy_rejects_write_op` 通过；
- 只读 `FileAccessPolicy` 对 `FileOp::Write` 返回 `error.code = file.permission_denied`；
- 该错误码表示只读 policy 明确拒绝写操作，和一般非只读策略的 op allowlist 缺失错误 `file.op_not_permitted` 区分。

### TC-2.6 二进制文件默认保护
```bash
bifrost remote file read target/debug/bifrost  # 或任意 ELF/Mach-O
```
**期望**：在未传 `--allow-binary` 时返回 `error.code = file.binary_not_allowed`。

### TC-2.7 gitignore 生效
在 `respect_gitignore = true` 时，访问 `target/` 下文件。
**期望**：当前实现至少被默认 deny 中的 `**/target/**` 拒绝；如果后续接入 gitignore，则错误码可收敛为 `file.ignored_by_gitignore`。

### TC-2.7 回归：只读 policy 拒绝写操作错误码
**步骤**：
```bash
cargo test -p bifrost-e2e remote_file_readonly_policy_rejects_write_op -- --nocapture
```
**期望**：
- 用例通过；
- `FileAccessPolicy::new_readonly` 收到 `FileOp::Write` 时返回 `error.code = file.permission_denied`；
- 不得返回 `file.op_not_permitted`，避免 caller 将 policy 拒绝误判为未知/未启用操作。

**2026-04-25 执行记录**：
- 触发原因：Windows runner E2E 失败，`remote_file_readonly_policy_rejects_write_op` 实际返回 `file.op_not_permitted`。
- 验证命令：`cargo test -p bifrost-e2e remote_file_readonly_policy_rejects_write_op -- --nocapture`。
- 通过标准：命令退出码为 0，且用例不再输出 `expected file.permission_denied, got file.op_not_permitted`。

---

## 3. 并发 / 稳定性

### TC-3.1 并发读取
host-A 上开 10 个并发 `file.read` 同一个 1 MiB 文件。
**期望**：全部成功，内容 hash 一致；host-B 日志无 panic / 重入错误。

### TC-3.2 长 search 可中断
执行一个 `file.search` 在大仓库上，host-A 上 `Ctrl+C`。
**期望**：host-B 端 worker 在 ≤ 2s 内感知取消、释放 IO；无僵尸进程。

### TC-3.3 Grant 过期后调用
让 grant 过期后再调用。
**期望**：`error.code = grant.expired`；CLI 给出 `bifrost remote connect` 重连提示。

---

## 4. 审计与日志

### TC-4.1 审计记录完整
执行 TC-1.1 后，在 host-B 数据目录的 `audit.log` 中检索：
**期望**：存在一条 `kind=file.read`，包含 `grant_id / client_id / path / size / result=ok`。

### TC-4.2 拒绝事件留痕
TC-2.1 失败后：
**期望**：audit 记录 `result=denied, reason=deny_pattern`，且不记录文件内容。

---

## 5. CLI UX 验收

### TC-5.1 帮助文案
```bash
bifrost remote file --help
bifrost remote file read --help
```
**期望**：列出 `read/list/stat/glob/search/hash/write/edit/mkdir/mv/rm/apply-patch` 十二项；每项具备独立帮助。

### TC-5.2 JSON 输出
所有命令支持 `--output json`，输出为严格 JSON；人类可读输出默认为表格/摘要。

---

## 6. 回归

- 确保原有 `bifrost remote command exec`、`bifrost remote shell` 行为不受影响。
- `bifrost remote grant list` 能正确展示 `remote_file_read` / `remote_file_write` 作用域。

---

## 完成定义（DoD）

- [ ] 以上全部 TC 在 macOS + Linux 被控端至少各执行一轮并记录结果。
- [ ] 每条失败 TC 单独开 issue；全部通过前不得合入 main。
- [ ] 审计日志样本随 PR 附带（脱敏）。


---

## 自动化覆盖

| 来源 | 覆盖点 |
|------|--------|
| `e2e-tests/tests/test_remote_file_api_e2e.sh` | CLI 子命令 help + 表面契约 |
| `e2e-tests/tests/test_remote_file_relay_e2e.sh` | caller → relay → target 的 read/write/mkdir/mv/rm 与 scope 拒绝 |
| `crates/bifrost-e2e/src/tests/remote_file_api.rs` | FileAccessPolicy / DenyMatcher / PolicyDecision 正负向用例 |

以上自动化用例在 CI 必过，手动用例覆盖真机网络/双端/审计等自动化不便验证的场景。

## 本次回归执行记录

| 日期 | 用例 | 执行命令 | 结果 |
|------|------|----------|------|
| 2026-04-25 | TC-2.5 只读 policy 拒绝写操作（回归） | `CARGO_TARGET_DIR=./.codex-target/remote-file-readonly-policy cargo run -p bifrost-e2e -- --test remote_file_readonly_policy_rejects_write_op` | PASS：1/1 passed，确认只读 policy 写操作返回 `file.permission_denied` |
