# Remote File API — Gap Analysis (vs Coding Agent 需求)

> **基线**：`feat/remote-file-api` @ `a7a5115b`（2026-04-26）
> **上次更新**：原文基于 `128ba7cf`，本次刷新至当前 HEAD，标注所有已修复项。

扫描范围：`crates/bifrost-core/src/file_access/*` + `crates/bifrost-admin/src/remote_invoke/{executor, file_ops, file_policy_store, types, worker}.rs` + `design/*` + E2E 测试。

---

## 0. 结论

能力矩阵覆盖了 coding agent 的核心闭环（read / list / stat / glob / search / hash / write / edit / mkdir / move / delete / apply_patch），**正交权限模型 + per-grant policy + path canonicalize + 乐观锁 + 原子写 + write_denies + .gitignore 感知 + mode 保留 + CRLF 归一 + 两阶段 apply_patch** 全部到位。

综合打分 **8.5 / 10**：可稳定用于 coding agent 真实 repo 场景。剩余缺口为 move/delete 无乐观锁、store 无缓存、handler 无审计，均不阻塞核心工作流。

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
| P1-2 | search 不合并同文件命中 | ❌ | ❌ **仍缺** | agent 需自行 group-by |
| P1-3 | glob 不支持多 pattern & 不返回 mtime | ❌ | ❌ **仍缺** | 单 pattern；无 mtime |
| P1-4 | list 静默截断 | ❌ | ✅ 已实现 | `truncated` 字段 |
| P1-5 | `file.move` 无 `base_sha256` | ❌ | ❌ **仍缺** | 多 agent race 丢写风险 |
| P1-6 | `file.delete` 无 `if_match_sha256` | ❌ | ❌ **仍缺** | 误删风险 |
| P1-7 | `FileAccessPolicyStore::load_default()` 每请求读盘 | ❌ | ❌ **仍缺** | 无缓存 |
| P1-8 | `file.read` truncate 时 sha256 = 片段哈希 | ❌ | ✅ 已实现 | `file_sha256` 字段区分 |

---

## 3. P2 — 修复状态汇总

| ID | 项 | 当前状态 | 说明 |
|---|---|---|---|
| P2-TOCTOU | canonicalize 与 open 之间 race | ❌ 未修 | 依赖 roots 在受控目录 |
| P2-hardlink | 硬链接绕过 write_denies | ❌ 未修 | 需 inode 对照，显式推迟 |
| P2-exclude | 默认排除目录不够 | ⚠️ 部分 | 仅 6 项；`.venv/dist/build/.next` 等未覆盖 |
| P2-errcode | 错误码漂移 | ✅ 已对齐 | 设计文档本次更新已按实现重写 |
| P2-symlink | list/stat 缺 symlink_target | ✅ 已实现 | |
| P2-prev_sha | write/edit 缺 previous_sha256 | ✅ 已实现 | |
| P2-audit | handler 零 tracing | ❌ **仍缺** | file_ops.rs 无任何 tracing 调用 |
| P2-concurrent | 并发写同路径无 per-path 锁 | ❌ 未修 | 依赖 base_sha256 保底 |

---

## 4. 评分

| 维度 | 分数 | 说明 |
|---|---|---|
| 能力覆盖 | 9 / 10 | 12 个 op 齐；watch / chmod 可延后 |
| 安全 / 权限 | 8.5 / 10 | 正交模型 + canonicalize + deny + gitignore 到位；硬链接未做 |
| 原子性 / 一致性 | 8.5 / 10 | 单文件 write/edit 原子；多文件 apply_patch 两阶段 + rollback |
| 错误模型 | 8 / 10 | 实现与设计已对齐 |
| EOL / Mode | 9 / 10 | CRLF 侦测 + 保持；mode 快照还原；NNAEOF 正确处理 |
| 性能 | 7 / 10 | store 每请求读盘仍是隐患 |
| 可观测性 | 5 / 10 | handler 无 tracing / audit |
| **综合** | **8.5 / 10** | 可稳定用于 coding agent 真实场景 |

---

## 5. 剩余 Backlog

### 下一 PR 建议（P1 收尾）

1. `file.move` + `file.delete` 加 `base_sha256` / `if_match_sha256`（~200 行）
2. `FileAccessPolicyStore` 加 `OnceLock<RwLock>` + mtime 缓存（~100 行）

### 后续 PR

3. handler tracing + audit 行（每个 handler 入口 `info!(target="audit.file", …)`）
4. 默认 exclude 目录扩展
5. search 合并同文件命中 / glob 多 pattern + mtime — 体验优化

### 不做

- `file.watch`（单独 feature）
- `GIT binary patch`（下一大版本）
- 硬链接 inode 对照（需跨平台统一方案）
