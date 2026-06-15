# Remote Invoke File API — 人工测试用例

> 关联设计：[`design/remote-invoke-file-api.md`](../design/remote-invoke-file-api.md)
> 关联分支：`feat/remote-file-api`
> 场景范围：读取/写入/编辑/patch 全操作 + coding agent 增强能力
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
   roots = ["<USER_HOME>/work/github/bifrost-remote-file"]
   denies = ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"]
   write_denies = ["**/Cargo.lock", "**/*.lock"]
   ops = ["read", "list", "stat", "glob", "search", "hash", "write", "edit", "mkdir", "move", "delete", "apply_patch"]
   max_read_bytes = 2_097_152    # 2 MiB
   max_write_bytes = 2_097_152
   respect_gitignore = true
   allow_overwrite = true
   allow_recursive_delete = false
   ```
3. **Grant**：host-A 本地通过 `bifrost remote grant update <grant_id> --file-access read_write` 为目标 grant 设置文件读写权限；只读拒绝用例中再降级到 `--file-access read`。注意：`file_access` 是独立于 `grant_scope` 的正交字段。

---

## 1. 正向用例

### TC-1.1 file.read — 读取文本文件
**步骤**：
```bash
bifrost remote file read README.md --cwd <USER_HOME>/work/github/bifrost-remote-file
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
bifrost remote file write agent-write.txt --content-file /tmp/bifrost-file-write.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
bifrost remote file read agent-write.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
```
**期望**：
- `write` 返回 `bytes_written / sha256 / previous_sha256`；
- 后续 `read` 解码内容为 `hello remote file\n`。

### TC-1.9 file.edit — 行范围编辑
**步骤**：
```bash
bifrost remote file edit agent-write.txt --edits '[{"start_line":1,"end_line":1,"replacement":"edited remote file\n"}]' --cwd <USER_HOME>/work/github/bifrost-remote-file
bifrost remote file read agent-write.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
```
**期望**：
- `edit` 返回 `applied_edits = 1` 和新的 `sha256`；
- 后续 `read` 解码内容为 `edited remote file\n`。

### TC-1.10 file.mkdir / file.mv / file.rm — 目录与路径变更
**步骤**：
```bash
bifrost remote file mkdir agent-dir/nested --parents --cwd <USER_HOME>/work/github/bifrost-remote-file
bifrost remote file mv agent-write.txt agent-dir/nested/moved.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
bifrost remote file rm agent-dir/nested/moved.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
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
bifrost remote file apply-patch --patch-file /tmp/bifrost-file.patch --cwd <USER_HOME>/work/github/bifrost-remote-file
bifrost remote file read patch-target.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
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

### TC-6.1 回归：配对批准时 remote_file_write 不依赖 Shell Access policy

**触发 Bug**：CI `scripts/ci/run-e2e-shell.sh` 运行 `test_remote_file_relay_e2e.sh` 时，前置授权未稳定升级为 file scope，后续 `file.read/list/edit` 被 target 端拒绝为 `grant scope RemoteQuery does not allow command kind File`。

**步骤**：
```bash
SKIP_BUILD=true bash e2e-tests/tests/test_remote_file_relay_e2e.sh
```

**期望**：
- 配对批准阶段输出 `grant available as remote_file_write`；
- `TC-FILE-01` `file.read` 返回包含 `content_b64` 的 JSON，不再出现 `RemoteQuery does not allow command kind File`；
- `TC-FILE-19` `file.list --exclude` 返回 `entries` JSON；
- `TC-FILE-20` `file.edit` 空 replacement 删除行返回成功 JSON；
- `TC-FILE-09` 降级到 `remote_file_read` 后写入被拒绝，证明 read/write scope 边界仍生效；
- 总结为 `Total: 56`、`Failed: 0`。

### TC-6.2 回归：SSH Key 默认 File Policy 可配置且 reset 后保留

**触发 Bug**：SSH Key 授权没有 pair-code 弹窗，caller 连接后虽然 shell 可用，但 file scope / file policy 需要用户手动按短 grant_id 补授；key reset 后还会回退到默认 `$HOME + 全部 ops`，导致已收窄的 roots/ops 丢失。

**步骤**：
```bash
# 使用隔离数据目录和非 9900 端口启动管理端，禁止修改系统代理
TEST_DIR="$(mktemp -d)"
CARGO_TARGET_DIR=./.codex-target/ssh-file-policy cargo build --bin bifrost
BIFROST_DATA_DIR="$TEST_DIR" ./.codex-target/ssh-file-policy/debug/bifrost start -p 18890 --unsafe-ssl --no-system-proxy

# 创建 SSH Key，并通过 API 配置当前 SSH fingerprint 的默认 file policy
curl -sS http://127.0.0.1:18890/_bifrost/api/remote-invoke/ssh-key \
  -H 'content-type: application/json' \
  -d '{"label":"agent-default","grant_mode":"permanent","seed_policy":{"roots":["/tmp/agent-a"],"ops":["read","list"],"allow_overwrite":false,"allow_recursive_delete":false}}'
FP="$(curl -sS http://127.0.0.1:18890/_bifrost/api/remote-invoke/ssh-key | jq -r '.ssh_key_fingerprint')"
curl -sS http://127.0.0.1:18890/_bifrost/api/remote-invoke/file-access-config \
  -H 'content-type: application/json' \
  -X PUT \
  -d "{\"grant\":[{\"match\":{\"ssh_fingerprint\":\"$FP\"},\"name\":\"ssh-key:agent-default\",\"roots\":[\"<USER_HOME>/work/code/nextoncall/next_agent\"],\"ops\":[\"read\",\"list\",\"stat\",\"glob\",\"search\",\"hash\",\"write\",\"edit\",\"mkdir\",\"move\",\"delete\",\"apply_patch\"],\"allow_overwrite\":true,\"allow_recursive_delete\":false}]}"

# reset 后确认策略迁移到新 fingerprint，roots/ops 不回退
curl -sS -X POST http://127.0.0.1:18890/_bifrost/api/remote-invoke/ssh-key/reset
NEW_FP="$(curl -sS http://127.0.0.1:18890/_bifrost/api/remote-invoke/ssh-key | jq -r '.ssh_key_fingerprint')"
curl -sS http://127.0.0.1:18890/_bifrost/api/remote-invoke/file-access-config | jq .
```

**期望**：
- SSH Key 卡片展示 File Access 状态，并可通过 Configure 保存 `match.ssh_fingerprint` 策略；
- `file-access.toml` 中只有新 fingerprint 的 SSH Key 策略，不保留旧 fingerprint 重复项；
- reset 后策略 roots 仍包含 `<USER_HOME>/work/code/nextoncall/next_agent`；
- reset 后策略 ops 仍包含 `write` / `edit` / `apply_patch` 等写操作；
- Bifrost 测试实例启动命令包含 `--no-system-proxy`，且端口不是 9900。

### TC-6.3 回归：SSH Key 默认 File Policy 被误删后自动恢复落盘

**触发 Bug**：用户在 File Access 策略编辑器或直接编辑 `file-access.toml` 时，可能误删当前 active SSH Key 的 `match.ssh_fingerprint` 策略。缺失后远端 SSH grant 无法自动获得 file policy，需要后端在读取/保存策略时自动恢复默认策略并写回配置文件。

**步骤**：
```bash
# 使用隔离数据目录和非 9900 端口启动管理端，禁止修改系统代理
TEST_DIR="$(mktemp -d)"
CARGO_TARGET_DIR=./.codex-target/ssh-file-policy-restore cargo build --bin bifrost
BIFROST_DATA_DIR="$TEST_DIR" ./.codex-target/ssh-file-policy-restore/debug/bifrost start -p 18891 --unsafe-ssl --no-system-proxy

# 创建 SSH Key 后，模拟用户误删所有 file-access grant policies
curl -sS http://127.0.0.1:18891/_bifrost/api/remote-invoke/ssh-key \
  -H 'content-type: application/json' \
  -d '{"label":"agent-restore","grant_mode":"permanent","seed_policy":{"roots":["/tmp/agent-restore"],"ops":["read","list"],"allow_overwrite":false,"allow_recursive_delete":false}}'
FP="$(curl -sS http://127.0.0.1:18891/_bifrost/api/remote-invoke/ssh-key | jq -r '.ssh_key_fingerprint')"
curl -sS http://127.0.0.1:18891/_bifrost/api/remote-invoke/file-access-config \
  -H 'content-type: application/json' \
  -X PUT \
  -d '{"grant":[]}'

# GET 应返回自动恢复后的 fingerprint 策略；磁盘 file-access.toml 也必须包含它
curl -sS http://127.0.0.1:18891/_bifrost/api/remote-invoke/file-access-config | jq .
grep "$FP" "$TEST_DIR/file-access.toml"
```

**期望**：
- PUT 空策略后返回的配置不为空，包含当前 `match.ssh_fingerprint`；
- 随后的 GET 仍包含当前 `match.ssh_fingerprint`；
- `$TEST_DIR/file-access.toml` 已落盘恢复该 fingerprint 策略；
- 恢复策略使用默认 roots（`$HOME`）和 12 个 file ops，确保 SSH Key 连接不会卡在无 file policy 状态；
- Bifrost 测试实例启动命令包含 `--no-system-proxy`，且端口不是 9900。

### TC-6.4 回归：旧 SSH grant 使用 caller fingerprint 时 file.write 仍命中 active SSH Key 默认策略

**触发 Bug**：旧版 SSH grant 可能把 `ssh_key_fingerprint` 错存成 caller fingerprint。此时 grant 列表看起来是 SSH key 连接，`file_access=read_write`，SSH Key 默认 File Policy 也已配置为 `roots=["/"]`，但执行 `bifrost remote file write hello.txt --cwd "$HOME"` 时执行端拿错误 fingerprint 查 policy，退回 readonly fallback，报 `readonly policy does not allow write operations`。

**步骤**：
```bash
# 前置：target 当前 active SSH Key 已配置 match.ssh_fingerprint 默认策略，roots=["/"]，ops 包含 write/edit/apply_patch。
# 构造或保留一条旧 SSH grant：auth_method=ssh_publickey，file_access=read_write，
# 但 ssh_key_fingerprint 缺失或等于 caller_fingerprint。
printf 'hello\n' > /tmp/bifrost-remote-hello.txt
HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 \
  bifrost remote file write hello.txt --cwd "$HOME" \
  --content-file /tmp/bifrost-remote-hello.txt --output json
HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 \
  bifrost remote file read hello.txt --cwd "$HOME" --output json
```

**期望**：
- `file.write hello.txt` 成功返回 JSON，不再出现 `[file.permission_denied] readonly policy does not allow write operations`。
- 后续 `file.read hello.txt` 能读回 `hello\n`。
- target 侧该旧 grant 的 `ssh_key_fingerprint` 被修正为当前 active SSH Key fingerprint，后续调用继续命中 `match.ssh_fingerprint` 默认策略。

---

## 完成定义（DoD）

- [ ] 以上全部 TC 在 macOS + Linux 被控端至少各执行一轮并记录结果。
- [ ] 每条失败 TC 单独开 issue；全部通过前不得合入 main。
- [ ] 审计日志样本随 PR 附带（脱敏）。


---

## 3. Coding Agent 增强能力用例

### TC-3.1 file.read — offset/limit 行范围读取
**步骤**：
```bash
# 先读取整个文件确认行数
bifrost remote file read Cargo.toml --cwd <USER_HOME>/work/github/bifrost-remote-file --output json | jq .total_size

# 读取第 5-10 行
bifrost remote file read Cargo.toml --offset 5 --limit 6 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 返回 `start_line: 5`, `end_line: 10`, `total_lines` 为文件实际行数
- `content_b64` 解码后只包含第 5-10 行内容
- `truncated: true`（因为未读到文件末尾）

### TC-3.2 file.read — offset 超出文件末尾
**步骤**：
```bash
bifrost remote file read Cargo.toml --offset 99999 --limit 10 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 返回 `start_line` 等于 `total_lines + 1`（或 clamped），`end_line` 等于 `start_line - 1`
- `content_b64` 解码后为空
- `size: 0`

### TC-3.3 file.read — 仅指定 offset 读到文件末尾
**步骤**：
```bash
bifrost remote file read Cargo.toml --offset 3 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `start_line: 3`, `end_line` 等于 `total_lines`
- `truncated: false`
- `content_b64` 解码后从第 3 行开始到文件末尾

### TC-3.4 file.search — 带上下文行
**步骤**：
```bash
bifrost remote file search "name" -B 2 -A 2 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 每个 match 对象包含 `context` 数组
- `context` 中每个元素有 `line`（行号）和 `content`（内容）
- 上下文范围覆盖匹配行前 2 行和后 2 行

### TC-3.5 file.search — 不带上下文（默认行为不变）
**步骤**：
```bash
bifrost remote file search "name" --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 每个 match 对象只有 `path`/`line`/`column`/`preview`，**没有** `context` 字段

### TC-3.6 file.glob — 默认排除 .git / node_modules / target
**步骤**：
```bash
# 确保目标目录下有 .git 和 target 目录
bifrost remote file glob "**/*" --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `matches` 中不包含任何以 `.git/` 或 `target/` 或 `node_modules/` 开头的路径
- 正常文件（如 `src/main.rs`, `Cargo.toml`）正常返回

### TC-3.7 file.glob — 自定义 --exclude
**步骤**：
```bash
bifrost remote file glob "**/*" --exclude "src" --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `matches` 中不包含 `src/` 开头的路径
- 其他目录下的文件正常返回

### TC-3.8 file.search — 默认排除 .git / node_modules / target
**步骤**：
```bash
bifrost remote file search "fn" --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `matches` 中无来自 `.git/`、`target/`、`node_modules/` 的结果

### TC-3.9 file.list — 默认排除 .git / node_modules / target
**步骤**：
```bash
bifrost remote file list --depth 3 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `entries` 中不递归进入 `.git/`、`target/`、`node_modules/` 目录
- 但这些目录本身作为条目仍然可见（仅不递归遍历其内部）

### TC-3.10 file.read — offset 超出文件末尾
**步骤**：
```bash
bifrost remote file read Cargo.toml --offset 99999 --limit 10 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `size: 0`，`content_b64` 解码后为空
- `total_lines` 为文件实际行数
- 无错误返回（优雅降级为空）

### TC-3.11 file.read — limit=0 返回空
**步骤**：
```bash
bifrost remote file read Cargo.toml --offset 1 --limit 0 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `size: 0`，`content_b64` 解码后为空
- `truncated: true`（还有更多行可读）

### TC-3.12 file.read — 非 offset 模式返回 total_lines
**步骤**：
```bash
bifrost remote file read Cargo.toml --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 响应中包含 `total_lines` 字段，值为文件实际行数
- coding agent 可据此规划分段读取

### TC-3.13 file.edit — 空 replacement 删除行（回归）
**步骤**：
```bash
# 准备三行文件
printf 'line1\nline2\nline3\n' | bifrost remote file write edit-test.txt --content-file - --cwd <USER_HOME>/work/github/bifrost-remote-file
# 删除第 2 行（空 replacement）
bifrost remote file edit edit-test.txt --edits '[{"start_line":2,"end_line":2,"replacement":""}]' --cwd <USER_HOME>/work/github/bifrost-remote-file
# 读取验证
bifrost remote file read edit-test.txt --cwd <USER_HOME>/work/github/bifrost-remote-file
```
**预期**：
- 结果为 `line1\nline3\n`，**不应有多余空白行**（修复前 bug 会产生 `line1\n\nline3\n`）

### TC-3.14 file.search — 无匹配时带 context 不崩溃
**步骤**：
```bash
bifrost remote file search "NONEXISTENT_PATTERN_XYZ_12345" -B 3 -A 3 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- `matches` 为空数组
- `truncated: false`
- 无错误

### TC-3.15 file.search — 首行匹配 + context_before 不越界
**步骤**：
```bash
bifrost remote file search "^\[package\]" -B 5 -A 2 --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 匹配 Cargo.toml 第 1 行 `[package]`
- `context` 数组中 `line` 最小值为 1（不为负数或 0）

### TC-3.16 file.search — 非法正则返回清晰错误
**步骤**：
```bash
bifrost remote file search "[invalid_regex" --cwd <USER_HOME>/work/github/bifrost-remote-file --output json
```
**预期**：
- 返回错误，错误码包含 `file.invalid_regex`

### TC-3.17 file.write — sha256 前置条件不匹配拒绝
**步骤**：
```bash
printf 'data' | bifrost remote file write precond.txt --content-file - --cwd <USER_HOME>/work/github/bifrost-remote-file
printf 'new' | bifrost remote file write precond.txt --content-file - --base-sha256 wrong_sha --cwd <USER_HOME>/work/github/bifrost-remote-file
```
**预期**：
- 第二次写入失败，错误码包含 `file.precondition_failed`
- 原有内容未被覆盖

### TC-3.18 file.apply_patch — context mismatch 拒绝
**步骤**：
```bash
cat >/tmp/bifrost-bad.patch <<'PATCH'
--- a/precond.txt
+++ b/precond.txt
@@ -1,1 +1,1 @@
-this_line_does_not_exist
+replacement
PATCH
bifrost remote file apply-patch --patch-file /tmp/bifrost-bad.patch --cwd <USER_HOME>/work/github/bifrost-remote-file
```
**预期**：
- 返回错误，包含 `mismatch`
- 原文件内容未被修改

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
| 2026-04-25 | TC-3.1 file.read offset/limit | `cargo test -p bifrost-admin -- read_with_offset_limit_returns_line_range` | PASS：start_line=2, end_line=3, total_lines=5, truncated=true, 内容只含 line2/line3 |
| 2026-04-25 | TC-3.2/3.3 file.read offset 边界 | `cargo test -p bifrost-admin -- read_with_offset_only_returns_from_offset_to_end` | PASS：start_line=2, end_line=3, truncated=false, 从 offset 读到末尾 |
| 2026-04-25 | TC-3.4 file.search context | `cargo test -p bifrost-admin -- search_with_context_lines` | PASS：context 数组含 line 2/3/4，匹配行前后各 1 行 |
| 2026-04-25 | TC-3.6 file.glob 默认排除 | `cargo test -p bifrost-admin -- glob_excludes_default_dirs` | PASS：src/main.rs 存在，.git/config 和 node_modules/pkg.js 被排除 |
| 2026-04-25 | TC-3.9 file.list 默认排除 | `cargo test -p bifrost-admin -- list_excludes_default_dirs` | PASS：src 可见，.git/node_modules 不被递归遍历 |
| 2026-04-25 | CLI help 验证 | `./target/release/bifrost remote file read/search/glob/list --help` | PASS：20/20 CLI 合约测试通过，--offset/--limit/-B/-A/--exclude 全部出现 |
| 2026-04-25 | TC-3.10 offset 超出 EOF | `cargo test -p bifrost-admin -- read_offset_beyond_eof_returns_empty` | PASS：size=0, total_lines=2, 内容为空 |
| 2026-04-25 | TC-3.11 limit=0 | `cargo test -p bifrost-admin -- read_limit_zero_returns_empty` | PASS：size=0, truncated=true |
| 2026-04-25 | TC-3.12 非 offset 返回 total_lines | `cargo test -p bifrost-admin -- read_non_offset_includes_total_lines` | PASS：total_lines=3 |
| 2026-04-25 | TC-3.13 空 replacement 删除行 | `cargo test -p bifrost-admin -- edit_empty_replacement_deletes_line_without_blank` | PASS：结果为 "line1\nline3\n"，无多余空白行 |
| 2026-04-25 | TC-3.14 search 无匹配 | `cargo test -p bifrost-admin -- search_no_matches_returns_empty` | PASS：matches 为空数组 |
| 2026-04-25 | TC-3.15 首行 context_before | `cargo test -p bifrost-admin -- search_match_at_first_line_with_context_before` | PASS：context 行号最小为 1 |
| 2026-04-25 | TC-3.16 非法正则 | `cargo test -p bifrost-admin -- search_invalid_regex_returns_error` | PASS：file.invalid_regex |
| 2026-04-25 | TC-3.17 sha256 前置条件 | `cargo test -p bifrost-admin -- write_sha256_precondition_mismatch` | PASS：file.precondition_failed |
| 2026-04-25 | TC-3.18 patch context mismatch | `cargo test -p bifrost-admin -- apply_patch_context_mismatch_rejected` | PASS：mismatch 拒绝 |
| 2026-04-25 | 多区间编辑 | `cargo test -p bifrost-admin -- edit_multiple_ranges` | PASS："AA\nbb\nCC_DD\nee\n" |
| 2026-04-25 | 重叠编辑拒绝 | `cargo test -p bifrost-admin -- edit_overlapping_ranges_rejected` | PASS：overlap 错误 |
| 2026-04-25 | write+read 往返 | `cargo test -p bifrost-admin -- write_and_read_roundtrip` | PASS：写入 16 字节，读回一致 |
| 2026-04-25 | patch 创建新文件 | `cargo test -p bifrost-admin -- apply_patch_creates_new_file` | PASS："hello\nworld\n" |
| 2026-04-25 | glob 自定义 exclude | `cargo test -p bifrost-admin -- glob_custom_exclude` | PASS：build/ 被排除 |
| 2026-04-25 | 空文件 offset | `cargo test -p bifrost-admin -- read_empty_file_with_offset_returns_empty` | PASS：total_lines=0 |
| 2026-04-25 | TC-6.1 配对批准时 remote_file_write 不依赖 Shell Access policy | `SKIP_BUILD=true bash e2e-tests/tests/test_remote_file_relay_e2e.sh` | PASS：输出 `grant available as remote_file_write`，TC-FILE-01/19/20/09 通过，Summary 56/56 passed |
| 2026-04-27 | TC-6.2 SSH Key 默认 File Policy 可配置且 reset 后保留 | `TMPDIR=$PWD/.codex-tmp CARGO_TARGET_DIR=./.codex-target/ssh-file-policy cargo build --bin bifrost && BIFROST_DATA_DIR=<tmp> ./.codex-target/ssh-file-policy/debug/bifrost start -p 18890 --unsafe-ssl --no-system-proxy` + SSH key/file-access/reset API 断言 | PASS：旧 fingerprint `55a9ccae` reset 到新 fingerprint `2a018c79` 后，`file-access-config` 仅保留新 `match.ssh_fingerprint`，roots 仍为 `<USER_HOME>/work/code/nextoncall/next_agent`，ops 包含 `write/edit/apply_patch` |
| 2026-04-27 | TC-6.3 SSH Key 默认 File Policy 被误删后自动恢复落盘 | `TMPDIR=$PWD/.codex-tmp CARGO_TARGET_DIR=./.codex-target/ssh-file-policy-restore cargo build --bin bifrost && BIFROST_DATA_DIR=<tmp> ./.codex-target/ssh-file-policy-restore/debug/bifrost start -p 18891 --unsafe-ssl --no-system-proxy` + `PUT {"grant":[]}` / GET / grep file-access.toml 断言 | PASS：PUT 空策略后自动恢复 fingerprint `fed9b02c`；API 返回 12 个 file ops；`file-access.toml` 已写入 `match.ssh_fingerprint` |
| 2026-04-28 | TC-6.4 旧 SSH grant 使用 caller fingerprint 时 file.write 仍命中 active SSH Key 默认策略 | `cargo test -p bifrost-admin legacy_ssh_grant -- --nocapture` | PASS：旧 SSH grant 的 `ssh_key_fingerprint=caller_fingerprint` 被修正为 active SSH key fingerprint；随后 `FileAccessPolicyStore::resolve()` 命中 `match.ssh_fingerprint` 的 `roots=["/"]` 写策略，`FileOp::Write` 检查通过，不再走 readonly fallback |
| 2026-04-25 | workspace 全量测试 | `cargo test --workspace --all-features` | PASS：全部通过 |
| 2026-04-25 | clippy + fmt | `cargo clippy -p bifrost-admin -p bifrost-cli -- -D warnings && cargo fmt --all -- --check` | PASS：无警告无格式问题 |


### TC-P0-READMANY-01 file.read_many — 请求级 ReadMany capability 与逐文件 Read 双层校验
**步骤**：
1. 在 host-B policy 中为测试 grant 配置 `ops = ["read"]`，保留测试根目录内 `ok.txt`。
2. 执行：
   ```bash
   bifrost remote file read-many --path ok.txt --cwd <测试根目录>
   ```
3. 将 policy 改为 `ops = ["read_many", "read"]` 后再次执行同一命令。
4. 再准备一个被 `denies` 命中的文件和一个正常文件，执行同一批量读取。

**期望**：
- 第 2 步请求整体失败，错误码包含 `file.op_not_permitted`，证明 `read_many` 需要请求级 capability。
- 第 3 步成功读取 `ok.txt`。
- 第 4 步正常文件成功，被拒绝文件以 per-item error 返回，批量请求不因单个文件拒绝而整体中断。

### TC-P0-MOVE-01 file.move — source base sha 与 overwrite 安全参数
**步骤**：
1. 在测试根目录准备 `from.txt` 和 `to.txt`，记录 `from.txt` 的 sha256。
2. 使用错误 sha 执行：
   ```bash
   bifrost remote file move from.txt moved.txt --base-sha256 deadbeef --cwd <测试根目录>
   ```
3. 使用已存在的 `to.txt` 执行：
   ```bash
   bifrost remote file move from.txt to.txt --allow-overwrite false --cwd <测试根目录>
   ```
4. 使用正确 sha 和显式允许覆盖执行：
   ```bash
   bifrost remote file move from.txt to.txt --base-sha256 <正确sha256> --allow-overwrite true --cwd <测试根目录>
   ```

**期望**：
- 第 2 步失败，错误码包含 `file.sha_mismatch`，`from.txt` 仍存在。
- 第 3 步失败，错误码包含 `file.precondition_failed`，`from.txt` 与 `to.txt` 均保持原内容。
- 第 4 步成功，返回 `from` / `to` / `overwritten` / `source_sha256`，目标内容等于原 source 内容。

### TC-P0-CLI-01 remote file CLI contract — read-many / outline / move safety flags
**步骤**：
```bash
cargo build --release -p bifrost-cli
bash e2e-tests/tests/test_remote_file_api_e2e.sh
```

**期望**：
- root help 列出 fourteen subcommands，包括 `read-many` 和 `outline`。
- `read-many --help` 包含 `--path` / `--max-bytes` / `--allow-binary`。
- `outline --help` 包含 `--max-symbols` / `--max-bytes`。
- `move --help` 包含 `--base-sha256` / `--allow-overwrite`。
- 缺少 required 参数时 `read-many` 与 `outline` 均被 CLI 拒绝。
