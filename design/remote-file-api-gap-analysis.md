# Remote File API — Gap Analysis (vs Coding Agent 需求)

> **基线**：`feat/remote-file-api` @ `a7a5115b`（2026-04-26）
> **上次更新**：2026-06-16 — 基于 `bifrost-remote-fixes-on-tray` 当前 HEAD，对照实现复核所有条目。

扫描范围：`crates/bifrost-core/src/file_access/*` + `crates/bifrost-admin/src/remote_invoke/{executor, file_ops, file_policy_store, types, worker}.rs` + `design/*` + E2E 测试。

---

## 0. 结论

能力矩阵覆盖了 coding agent 的核心闭环（read / read_many / list / stat / glob / search / hash / outline / write / edit / mkdir / move / delete / apply_patch — 14 个 op），**正交权限模型 + per-grant policy + path canonicalize + 乐观锁（含 move/delete）+ 原子写 + write_denies + .gitignore 感知 + mode 保留 + CRLF 归一 + 两阶段 apply_patch + policy store mtime 缓存 + executor `audit.file` tracing** 全部到位。

综合打分 **9.0 / 10**：可稳定用于 coding agent 真实 repo 场景。剩余缺口为 glob 单 pattern / 无 mtime、search 不合并同文件命中、默认 exclude 目录仍为 6 项，均为体验/性能优化，不阻塞核心工作流。

---

## 1. P0 — 修复状态汇总

| ID | 项 | 原始状态 | 当前状态 | 修复提交 |
|---|---|---|---|---|
| P0-1 | apply_patch 多文件不原子 | ❌ 有半应用风险 | ✅ 两阶段 tmp→rename + rollback | `51059840` / `13ec624a` |
| P0-2 | apply_patch 解析器对 git-diff 扩展头过于脆弱 | ❌ 不识别 diff --git / rename / copy / binary | ✅ `parse_patch` + `PatchKind` 完整解析 | `884bb47b` |
| P0-3 | CRLF / EOL 未归一 | ❌ 混合行尾 | ✅ `Eol` 侦测 + 保持 | `51059840` / `deec3cd1` |
| P0-4 | 文件 mode 可执行位丢失 | ❌ 强制 0644 | ✅ prior_mode 快照 + chmod 还原 | `503aff04` / `5d48f14d` |
| P0-5 | `.gitignore` 未接线 | ❌ 仅硬编码 6 项 | ✅ `ignore` crate 接入 | `51059840` |
| P0-6 | `file.write` 不支持 `create_parents` | ❌ 需额外 mkdir | ✅ `create_parents: bool` 参数 | `51059840` |
| Bug#1 | apply_patch 删除文件时 policy key 用了 new_path=/dev/null | ❌ | ✅ key 改为 old_path | `6fada6a9` |
| Bug#2 | file.search 未应用 policy.denies | ❌ | ✅ DenyMatcher 过滤 | `19677fd2` |
| Bug#3 | file.edit 替换最后一行时丢失尾换行 | ❌ | ✅ is_last_chunk + original_ended_with_newline 判断 | `deec3cd1` |
| Bug#4 | apply_patch 忽略 `\ No newline at end of file` 标记 | ❌ | ✅ LastEmit 状态机 | `5a6da23d` |

**P0 全绿** ✅

---

## 2. P1 — 修复状态汇总

| ID | 项 | 原始状态 | 当前状态 | 说明 |
|---|---|---|---|---|
| P1-1 | search 缺 case_insensitive + glob 文件过滤 | ❌ | ✅ 已实现 | `case_insensitive: bool`；glob 通过 gitignore + deny 过滤 |
| P1-2 | search 不合并同文件命中 | ❌ | ❌ **仍缺** (planned, not yet shipped as of 2026-06-16) | `matches` 仍是扁平 per-line 数组；agent 需自行 group-by |
| P1-3a | search 多 pattern (OR) | ❌ | ✅ 已实现 | `patterns: Vec<String>` + `fixed_strings` + `word`，与 `pattern` 向后兼容 |
| P1-3b | glob 多 pattern & 返回 mtime | ❌ | ❌ **仍缺** (planned, not yet shipped as of 2026-06-16) | `handle_file_glob` 仍接收 `pattern: &str`；`matches` 仅是字符串列表 |
| P1-4 | list 静默截断 | ❌ | ✅ 已实现 | `truncated` 字段 |
| P1-5 | `file.move` 无 `base_sha256` | ❌ | ✅ 已实现 | `handle_file_move(..., base_sha256: Option<&str>, ...)`；不匹配时返回 `[file.sha_mismatch]` |
| P1-6 | `file.delete` 无 `if_match_sha256` | ❌ | ✅ 已实现 | `handle_file_delete(..., if_match_sha256: Option<&str>)`；目录跳过乐观锁、依赖 `allow_recursive_delete` |
| P1-7 | `FileAccessPolicyStore::load_default()` 每请求读盘 | ❌ | ✅ 已实现 | `OnceLock<RwLock<CacheEntry>>` + `(size, mtime)` 快照失效；`save_raw_config` 主动 invalidate |
| P1-8 | `file.read` truncate 时 sha256 = 片段哈希 | ❌ | ✅ 已实现 | `file_sha256` 字段区分 |

---

## 3. P2 — 修复状态汇总

| ID | 项 | 当前状态 | 说明 |
|---|---|---|---|
| P2-TOCTOU | canonicalize 与 open 之间 race | ❌ 未修 | 依赖 roots 在受控目录 |
| P2-hardlink | 硬链接绕过 write_denies | ❌ 未修 | 需 inode 对照，显式推迟 |
| P2-exclude | 默认排除目录不够 | ⚠️ 部分 (planned expansion, not yet shipped as of 2026-06-16) | `DEFAULT_EXCLUDE_DIRS` 仍是 6 项 (`.git / node_modules / target / __pycache__ / .svn / .hg`)；`.venv/dist/build/.next` 等未覆盖；调用方可通过 `exclude_patterns` 自助补齐 |
| P2-errcode | 错误码漂移 | ✅ 已对齐 | 设计文档本次更新已按实现重写 |
| P2-symlink | list/stat 缺 symlink_target | ✅ 已实现 | |
| P2-prev_sha | write/edit 缺 previous_sha256 | ✅ 已实现 | |
| P2-audit | handler 零 tracing | ✅ 已实现 | `executor.rs` 在每个 `file.*` 调用入口/出口发 `info!(target="audit.file", grant_id, method, path_hash, duration_ms, result, bytes, sha256, …)`；写类调用额外打 `file write audit` info 行；handler 内仍保持薄，符合“decision 已检查、handler 只翻译”分层 |
| P2-concurrent | 并发写同路径无 per-path 锁 | ❌ 未修 (planned, not yet shipped as of 2026-06-16) | 依赖 base_sha256 保底 |

---

## 4. 评分

| 维度 | 分数 | 说明 |
|---|---|---|
| 能力覆盖 | 9 / 10 | 14 个 op 齐（含 read_many、outline）；watch / chmod 可延后 |
| 安全 / 权限 | 9 / 10 | 正交模型 + canonicalize + deny + gitignore + move/delete 乐观锁到位；硬链接未做 |
| 原子性 / 一致性 | 9 / 10 | 单文件 write/edit 原子；多文件 apply_patch 两阶段 + rollback；move 通过 hard-link create-if-absent 避免 rename race |
| 错误模型 | 8 / 10 | 实现与设计已对齐 |
| EOL / Mode | 9 / 10 | CRLF 侦测 + 保持；mode 快照还原；NNAEOF 正确处理 |
| 性能 | 8 / 10 | policy store 已加 mtime 缓存；search/glob 仍单线程 walker |
| 可观测性 | 8 / 10 | executor 入口/出口 `audit.file` info!/warn!，包含 grant_id / path_hash / duration_ms / result / bytes / sha256 / error_code |
| **综合** | **9.0 / 10** | 可稳定用于 coding agent 真实场景 |

---

## 5. 剩余 Backlog

### 已收尾（自上次刷新起合并到 HEAD）

- ✅ `file.move` + `file.delete` 乐观锁（`base_sha256` / `if_match_sha256`）
- ✅ `FileAccessPolicyStore` `OnceLock<RwLock<CacheEntry>>` + `(size, mtime)` 缓存
- ✅ Executor 级 `audit.file` tracing（per-op info!/warn! 出入对）
- ✅ `file.outline` 新 op（heuristic symbol outline，多语言 regex 表）
- ✅ `file.search` 多 pattern (`patterns` 数组) + `fixed_strings` + `word`

### 下一 PR 建议（planned, not yet shipped as of 2026-06-16）

1. `file.glob` 多 pattern + 返回 `mtime`（与 search OR-逻辑对齐）
2. `file.search` 同文件命中聚合（返回 `{path, matches: [...]}` 形态以省 token）
3. `DEFAULT_EXCLUDE_DIRS` 扩充：`.venv / dist / build / .next / .turbo / out / coverage`
4. 并发写同路径 per-path 锁（在 base_sha256 之上加一层 fast-fail）

### 不做

- `file.watch`（单独 feature）
- `GIT binary patch`（下一大版本）
- 硬链接 inode 对照（需跨平台统一方案）
