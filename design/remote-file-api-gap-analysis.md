# Remote File API — Gap Analysis (vs Coding Agent 需求)

> 基线：`feat/remote-file-api` @ `128ba7cf`
> 扫描范围：`crates/bifrost-core/src/file_access/*` + `crates/bifrost-admin/src/remote_invoke/{executor, file_ops, file_policy_store, types, worker}.rs` + `design/{remote-invoke-file-api, grant-file-access}.md` + `crates/bifrost-e2e/src/tests/remote_file_api.rs` + `e2e-tests/tests/test_remote_file_*.sh`

本文盘点 Remote File API 相对 Claude Code / Cursor / Codex 等主流 coding agent 在真实工作流中的**已交付能力**与**已知差距**，作为后续 PR 的 backlog。

---

## 0. 结论一句话

能力矩阵覆盖了 coding agent 的核心闭环（read / list / stat / glob / search / hash / write / edit / mkdir / move / delete / apply_patch），**正交权限模型 + per-grant policy + path canonicalize + 乐观锁 + 原子写 + write_denies** 都到位。但对标稳态 coding agent 仍有 ~10 个会在真实工作流被踩到的 P0/P1 缺口，其中 `apply_patch` 多文件原子性、解析器健壮性、文件 mode 保留、CRLF 行尾处理、`.gitignore` 未接线 是最关键的五项。

综合打分 **7.2 / 10**：能上生产，但 agent 在 apply_patch / CRLF / gitignore / mode 上会踩坑。

---

## 1. 能力覆盖矩阵（vs 主流 Coding Agent）

| Coding Agent 常用原语 | Bifrost 当前实现 | 覆盖度 | 关键差距 |
|---|---|---|---|
| read file (+line range) | `file.read` + offset/limit | ✅ | offset/limit 为行号，与 Claude Code `Read` 一致 |
| list dir (+depth) | `file.list` | ✅ | 缺 `symlink_target`；`MAX_ENTRIES_PER_DIR=10_000` 静默截断且无 `truncated` 标志 |
| stat | `file.stat` | ✅ | 缺 `symlink_target` / `inode` |
| glob | `file.glob` | ✅ | 仅单 pattern；不支持 `!negate`；不感知 `.gitignore` |
| grep / search | `file.search` | ✅ | 缺 `case_insensitive`；缺 `glob` 文件过滤；二进制文件静默跳过不计数 |
| hash | `file.hash` | ✅ | 仅 sha256 |
| write (+precondition) | `file.write` | ⚠️ | tmp + fsync + rename；**不 fsync 父目录、不保留 mode、不支持 `create_parents`** |
| line edit | `file.edit` | ✅ | 基于行号 range；无 find / replace；无 dry-run |
| unified diff apply | `file.apply_patch` | ⚠️ | 关键缺陷集中项，见 §3 |
| mkdir / mv / rm | 全齐 | ✅ | `file.move` 无 `base_sha256`；跨 fs rename 会 EXDEV 失败 |
| watch | — | ❌ | 设计文档显式推迟 |
| 正交权限（shell + file） | `FileAccessScope` + `GrantScope` | ✅ | 新模型干净；`scope_allows_command()` 单入口 |
| per-grant policy | `file-access.toml` | ⚠️ | 每次请求 `load_default()` 读盘 + 解析 TOML |

---

## 2. P0 — 会直接让 Coding Agent 出错

### P0-1 `apply_patch` 不是"全或无"原子

- **现象**：`file_ops.rs::handle_file_apply_patch` 对每个文件 `rename(tmp, target)` 后继续处理下一个；若第 N 个文件 hunk 失败，前 N-1 个已落地，无法回滚。
- **与设计的偏差**：`design/remote-invoke-file-api.md` §错误码 明写 `file.patch_rejected`（所有 hunk 均未应用）与 `file.partial_applied`（部分 hunk 应用）作为对称错误码。当前实现只会抛 `precondition_failed`，并留下半应用状态。
- **修复方向**：
  1. 遍历所有文件写出 `<parent>/.bifrost-patch.<pid>.<nanos>.<i>.tmp`（不 rename）。
  2. 所有 hunk 校验通过后，再做两阶段 rename；任意失败即 `remove_file` 清理全部 tmp 并返回 `file.patch_rejected`。
  3. 对 `/dev/null`（新建 / 删除）单独记录待执行队列，最后统一执行。

### P0-2 `apply_patch` 解析器对真实 diff 过于脆弱

- 按 `\n--- ` 切分：文件本体若含以 `\n--- ` 开头的行（markdown / changelog / patch-in-patch）会误切。
- 不识别 `diff --git` / `index` / `new file mode 100644` / `rename from|to` / `similarity index`，全部作为 `_meta` 丢弃 —— **新建文件场景丢 mode**。
- `\ No newline at end of file` 被 "ignore"，但不据此决定是否省略输出末尾 `\n`，会**给本无 trailing newline 的文件强行补一个**。
- `--- /dev/null` 作为新建文件正规写法未被正面处理（executor 里仅跳过 `+++ /dev/null` 的 key 构建）。
- 不支持 `GIT binary patch`，当前走 `context mismatch`，应返回明确的 `file.binary_patch_unsupported`。

### P0-3 行尾 / EOL 未归一

- `file.edit` 对 CRLF 源文件：`split_inclusive('\n')` 会保留 `\r`，但 `replacement` 若不带 `\r` 就产生**混合行尾**。
- `file.apply_patch` 用 `trim_end_matches('\n')` 做 context 比对，对行尾 `\r` 不处理 → CRLF 源文件**整体 context mismatch**。
- 建议：在 handler 入口探测源文件 EOL 风格（优先看第一行），进入归一通道后再写回时还原。

### P0-4 文件 mode / 可执行位丢失

- `file.write` / `file.edit` / `file.apply_patch` 都是 `fs::File::create(tmp)` → `rename`，**原文件 0755 会丢**，对 `*.sh` / CI entrypoint 是灾难。
- 修复：写前 `fs::metadata(path).permissions()` 记下 mode，写 tmp 后 `set_permissions(tmp, mode)`；新文件应用合理默认（0644 / 0755 基于 shebang）。

### P0-5 `.gitignore` 未接线

- `policy.rs` 第 243 行注释仍为 `// For now we pass through.`
- 设计文档承诺 `file.search` / `file.glob` 默认尊重 `.gitignore`。
- 当前只有 `DEFAULT_EXCLUDE_DIRS` 硬编码 6 项（`.git` / `node_modules` / `target` / `__pycache__` / `.svn` / `.hg`），对 `dist/ build/ .venv/ coverage/ .next/ .nuxt/ .pytest_cache/` 等真实仓库噪音零防御。
- 修复：接入 `ignore` crate；`respect_gitignore=true` 时把文件级 ignore 判断下沉到 handler 遍历入口。

### P0-6 `file.write` 不支持 `create_parents`

- policy 层允许"写入父目录不存在的路径"（`check` 里 `NotFound` 分支重建 parent），但 handler 层 `fs::File::create(tmp)` 直接 IO error。
- agent 常态场景："写 `a/b/c/new.rs`" 必须先 `file.mkdir --parents` → 多 1 次 RTT。
- 修复：`file.write` 新增 `create_parents: Option<bool>` 参数；默认 `false` 保向后兼容。

---

## 3. P1 — 会拖慢 / 限制 Agent

| ID | 项 | 影响 |
|---|---|---|
| P1-1 | `file.search` 缺 `glob` 文件过滤 & `case_insensitive` | 设计文档已定义，实现未落 → agent 需多次 glob + read |
| P1-2 | `file.search` 不合并同文件命中 | 100 hits 分 5 file 时，agent 需自行 group-by |
| P1-3 | `file.glob` 不支持多 pattern & 不返回 mtime | 查"近期修改的 .rs/.toml 各 20 个"需要 2× glob + N× stat |
| P1-4 | `file.list` 静默截断 | `MAX_ENTRIES_PER_DIR=10_000` 命中时 JSON 无 `truncated:true` |
| P1-5 | `file.move` 无 `base_sha256` | 双 agent race 时丢写 |
| P1-6 | `file.delete` 无 `if_match_sha256` | 删错版本风险 |
| P1-7 | `FileAccessPolicyStore::load_default()` 每请求读盘 | 高频 agent + 百级 grant 场景的热点 |
| P1-8 | `file.read` truncate 时 sha256 = 前 N 字节哈希 | 作为 `base_sha256` 回传给 `file.write` 会**永远不匹配**；应区分 `content_sha256` 与 `file_sha256` |

---

## 4. P2 — 易忽略但会放大事故

- **TOCTOU**：`canonicalize_within_roots` 与 `File::open` 之间可被替换成 symlink；最终防线是 `roots` 列在受控目录。
- **硬链接规避**：设计文档 §安全 明确 Phase 2 做 inode 对照，**未实现**。`write_denies: **/*.lock` 可被硬链接绕过。
- **默认 exclude 缺失**：无 `.venv / dist / build / .next / .nuxt / .parcel-cache / .pytest_cache / .ruff_cache / .mypy_cache`。
- **错误码漂移**：设计文档 `file.sha_mismatch` / `file.too_large` 在实现为 `file.precondition_failed` / `file.size_too_large`。CLI / Relay 按设计文档 switch 会 fallthrough。
- **symlink 字段缺失**：`file.list` / `file.stat` 把 symlink 归类为 `kind="symlink"` 但**不返回目标路径**。
- **apply_patch 返回值不对称**：缺 `previous_sha256`，无法事后校验。
- **handler 无 tracing / audit**：设计文档 §安全 承诺审计 `grant_id + method + path_hash + size + sha256`，`file_ops.rs` 零 tracing；`file_policy_store.rs` 仅一条 debug。
- **并发写同路径**：tmp 后缀 `pid.nanos` 碰撞罕见但非 0；无 per-path 锁 → 后者胜出（需 `base_sha256` 保底）。

---

## 5. 整体评分

| 维度 | 分数 | 说明 |
|---|---|---|
| 能力覆盖 | 8.5 / 10 | 12 个 op 齐，watch 可延后 |
| 安全 / 权限 | 8 / 10 | 正交模型 + canonicalize + deny 到位；gitignore / 硬链接未做 |
| 原子性 / 一致性 | 6.5 / 10 | 单文件 ok；apply_patch 多文件不原子；mode 丢失 |
| 错误模型 | 7 / 10 | 实现与设计小范围漂移 |
| 性能 | 7 / 10 | per-request load TOML 隐患 |
| 可观测性 | 5 / 10 | handler 无 tracing / audit |
| **综合** | **7.2 / 10** | 可用；P0 未修前不建议让 agent 做真实 repo 修改 |

---

## 6. 推进路线

### Round 1 — 把差距暴露（E2E 红灯）
- 追加 `test_remote_file_relay_e2e.sh` 用例：apply_patch 部分失败、CRLF edit、executable 保留、`.gitignore` 感知、`create_parents`、sha on truncate、symlink target 字段、multi-file glob。
- 期望：每个 P0/P1 项至少 1 条红灯。

### Round 2 — P0 修复（单 PR）
涵盖 P0-1 / P0-3 / P0-4 / P0-5 / P0-6 + P1-1 / P1-8 + 错误码对齐。预计 ~600–900 行 Rust + 对应单测。
- 多文件 apply_patch 两阶段 rename
- EOL 归一（CRLF 侦测 + 保持）
- mode 保留 + 新文件默认
- `ignore` crate 接入；`.gitignore` 生效于 `file.search` / `file.glob`
- `file.write` + `create_parents`
- `file.search` + `case_insensitive` / `glob`
- `file.read` 新增 `file_sha256` 字段
- 错误码重命名 / 兼容别名

### Round 3 — P1 / P2 收尾
- per-path in-memory lock + store 缓存 + mtime 失效
- symlink_target 回填 list / stat
- list 截断标志 + 默认 exclude 扩展
- handler tracing + audit 行（`info!(target="audit.file", ...)`)
- apply_patch 返回 `previous_sha256`

### 非目标 / 显式不做
- `file.watch`（实时推送，单独 PR）
- `GIT binary patch`（下一个大版本）
- 硬链接 inode 对照（需要跨平台统一方案，P2）

---

## 7. 风险与迁移

- P0-5（`.gitignore` 接线）会改变 `file.search` / `file.glob` 的默认返回集合；CLI / WebUI 需要同步放出 `--no-gitignore` 逃生口。
- P0-4（mode 保留）对既有调用方**兼容**（之前是无条件 0644，之后变"保留原 mode"）。
- 错误码重命名需保留旧码别名 ≥ 1 个 minor 版本，避免打破外部 CLI。
- 多文件 apply_patch 两阶段 rename 会占用 2× 临时 inode，需在文档注明 `max_write_bytes` 仍按单文件上限评估。
