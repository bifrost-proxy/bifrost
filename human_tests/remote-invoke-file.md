# Remote Invoke File API — 人工测试用例（Phase 1）

> 关联设计：[`design/remote-invoke-file-api.md`](../design/remote-invoke-file-api.md)
> 关联分支：`feat/remote-file-api`
> 场景范围：Phase 1（只读文件能力 + FileAccessPolicy）
> 硬性约束：本技能涉及跨设备文件访问，所有用例必须在真实设备上由人工执行；严禁仅依赖单测通过即判定完成。

---

## 0. 前置准备

1. **两台机器**：
   - `host-A`（调用端）：安装 `bifrost` CLI（本 PR 构建产物）。
   - `host-B`（被控端）：运行 `bifrost` daemon，且已通过 SSH key 或配对码授权 host-A。
2. **策略文件**：`host-B` 的 `~/.bifrost/file-access.toml` 预置下述策略：
   ```toml
   [[policy]]
   name = "default-readonly"
   roots = ["/Users/eden/work/github/bifrost-remote-file"]
   denies = ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"]
   ops = ["read", "list", "stat", "glob", "search", "hash"]
   max_read_bytes = 2_097_152    # 2 MiB
   respect_gitignore = true
   ```
3. **Grant**：host-A 本地通过 `bifrost remote grant add --scope remote_file_read` 为目标 grant 追加只读文件作用域。

---

## 1. 正向用例

### TC-1.1 file.read — 读取文本文件
**步骤**：
```bash
bifrost remote file read --path README.md --cwd /Users/eden/work/github/bifrost-remote-file
```
**期望**：
- 返回 200/OK，`content` 字段为 base64 解码后的 README 内容；
- `meta.size`、`meta.sha256`、`meta.mtime` 字段完整；
- `meta.truncated = false`。

### TC-1.2 file.read — 超大文件截断
**步骤**：准备一个 5 MiB 的文件 `crates/bifrost-core/big.txt`，执行：
```bash
bifrost remote file read --path crates/bifrost-core/big.txt --max-bytes 1048576
```
**期望**：
- `content` 长度 = 1 MiB（解码后）；
- `meta.truncated = true`，`meta.total_size = 5242880`。

### TC-1.3 file.list — 列目录（浅层）
```bash
bifrost remote file list --path crates --depth 1
```
**期望**：返回 11 个工作区 crate 目录，每项包含 `name / type / size / mtime`；`.git` 等 deny 目录不出现在结果中。

### TC-1.4 file.stat — 获取元信息
```bash
bifrost remote file stat --path Cargo.toml
```
**期望**：返回 `size / mtime / mode / sha256 / kind=file`。

### TC-1.5 file.glob — 匹配
```bash
bifrost remote file glob --pattern 'crates/*/Cargo.toml'
```
**期望**：返回 11 条匹配。

### TC-1.6 file.search — 内容检索
```bash
bifrost remote file search --pattern 'GrantScope' --path crates/bifrost-admin --max-matches 20
```
**期望**：至少命中 `remote_invoke/types.rs`；返回 `{path, line, column, preview}`；不包含二进制文件命中。

### TC-1.7 file.hash — 校验
```bash
bifrost remote file hash --path Cargo.lock --algo sha256
```
**期望**：返回 64 位 hex；与 host-B 本地 `shasum -a 256 Cargo.lock` 一致。

---

## 2. 安全 / 拒绝用例

### TC-2.1 越界路径被拒（deny list）
```bash
bifrost remote file read --path .git/config
```
**期望**：`error.code = file.permission_denied`，`message` 提示命中 deny 规则；无任何字节返回。

### TC-2.2 越过 roots
```bash
bifrost remote file read --path /etc/passwd
```
**期望**：`error.code = file.out_of_scope`，审计日志记录 grant_id / scope / attempted_path。

### TC-2.3 Symlink 跨越 roots
在 `host-B` 上创建 `ln -s /etc/passwd crates/link-passwd`，执行：
```bash
bifrost remote file read --path crates/link-passwd
```
**期望**：`error.code = file.symlink_escape`。

### TC-2.4 scope 缺失
移除 grant 中的 `remote_file_read` 后执行任一 file.* 命令。
**期望**：`error.code = grant.scope_missing`。

### TC-2.5 二进制文件默认保护
```bash
bifrost remote file read --path target/debug/bifrost  # 或任意 ELF/Mach-O
```
**期望**：在未传 `--allow-binary` 时返回 `error.code = file.binary_not_allowed`。

### TC-2.6 gitignore 生效
在 `respect_gitignore = true` 时，访问 `target/` 下文件。
**期望**：被拒，`error.code = file.ignored_by_gitignore`。

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
执行 TC-1.1 后，在 host-B `~/.bifrost/audit.log` 中检索：
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
**期望**：列出 Phase 1 全部子命令；`read/list/stat/glob/search/hash` 六项具备独立帮助。

### TC-5.2 JSON 输出
所有命令支持 `--output json`，输出为严格 JSON；人类可读输出默认为表格/摘要。

---

## 6. 回归

- 确保原有 `bifrost remote command exec`、`bifrost remote shell` 行为不受影响。
- `bifrost remote grant list` 能正确展示新增的 `remote_file_read` 作用域。

---

## 完成定义（DoD）

- [ ] 以上全部 TC 在 macOS + Linux 被控端至少各执行一轮并记录结果。
- [ ] 每条失败 TC 单独开 issue；全部通过前不得合入 main。
- [ ] 审计日志样本随 PR 附带（脱敏）。
