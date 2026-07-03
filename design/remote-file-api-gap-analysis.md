# Remote File API — Gap Analysis (vs Coding Agent 需求)

> **基线**：`feat/remote-file-api` @ `a7a5115b`（2026-04-26）
> **上次更新**：2026-07-03 — 基于 `bifrost-remote-fixes-on-tray` 当前 HEAD，对照 `crates/bifrost-admin/src/remote_invoke/file_ops.rs`（约 5600 行）复核所有条目。

## 背景

Bifrost Remote File API 为 coding agent（Codex / Claude / 自研 agent）提供跨设备编辑源码仓库的能力。本文档不是新方案，而是**能力矩阵与实现现状差距分析**：对照 coding agent 真实工作流需求（read / edit / patch / search 等 14 op），扫描当前实现是否达到可用标准，并明确剩余 backlog。

扫描范围：
- `crates/bifrost-core/src/file_access/*`
- `crates/bifrost-admin/src/remote_invoke/{executor, file_ops, file_policy_store, types, worker, file_transfer, file_access_roots}.rs`
- `design/remote-file-*.md`
- E2E：`e2e-tests/tests/test_remote_invoke_*.sh`
- Human tests：`human_tests/remote-invoke-file.md`

## 用户目标验证清单

### 必须实现

- 14 个核心 op（read / read_many / list / stat / glob / search / hash / outline / write / edit / mkdir / move / delete / apply_patch）能够覆盖 coding agent 完整闭环。
- 正交权限模型：per-grant `FileAccessPolicy` + roots + write/deny 白/黑名单。
- 路径 canonicalize 后再做 policy 匹配，避免 `..` / symlink 绕过。
- 写操作全部走原子 tmp→rename（apply_patch 两阶段 + rollback）。
- 乐观锁 `base_sha256` / `if_match_sha256` 覆盖 write / edit / move / delete。
- CRLF 与 mode（可执行位）在写入后保留。
- `.gitignore` 遵守 + `DEFAULT_EXCLUDE_DIRS` 默认排除。
- Executor 级 `audit.file` tracing 覆盖每个 op 的入口/出口。
- Policy store 有 `(size, mtime)` 缓存，避免每请求读盘。

### 必须不破坏

- Remote invoke 现有非 file op 命令（`shell.exec` / `traffic.*` / `search.stream` / `status`）不受影响。
- 现有 `admin/remote_invoke_grants.json` / `admin/remote_invoke_call_history/` 兼容。
- CLI `bifrost remote file *` 与 Web `remote-invoke` 面板的既有交互。

### 必须真实验证

- 单元测试覆盖 `file_ops.rs` 中的 write / edit / apply_patch / move / delete / search / glob / outline 分支。
- E2E 覆盖 grant 授权后 CLI 通过 remote file 修改真实仓库。
- Human tests 记录真实 macOS 上 coding agent 借助 remote file API 完成一次 rebase / refactor 的证据。

## 0. 结论

能力矩阵覆盖了 coding agent 的核心闭环（read / read_many / list / stat / glob / search / hash / outline / write / edit / mkdir / move / delete / apply_patch — 14 个 op），**正交权限模型 + per-grant policy + path canonicalize + 乐观锁（含 move/delete）+ 原子写 + write_denies + .gitignore 感知 + mode 保留 + CRLF 归一 + 两阶段 apply_patch + policy store mtime 缓存 + executor `audit.file` tracing** 全部到位。

综合打分 **9.0 / 10**：可稳定用于 coding agent 真实 repo 场景。剩余缺口为 glob 单 pattern / 无 mtime、search 不合并同文件命中、默认 exclude 目录仍为 6 项，均为体验/性能优化，不阻塞核心工作流。

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

## 2. P1 — 修复状态汇总

| ID | 项 | 原始状态 | 当前状态 | 说明 |
|---|---|---|---|---|
| P1-1 | search 缺 case_insensitive + glob 文件过滤 | ❌ | ✅ 已实现 | `case_insensitive: bool`；glob 通过 gitignore + deny 过滤 |
| P1-2 | search 不合并同文件命中 | ❌ | ❌ **仍缺** (planned, not yet shipped as of 2026-07-03) | `matches` 仍是扁平 per-line 数组；agent 需自行 group-by |
| P1-3a | search 多 pattern (OR) | ❌ | ✅ 已实现 | `patterns: Vec<String>` + `fixed_strings` + `word`，与 `pattern` 向后兼容 |
| P1-3b | glob 多 pattern & 返回 mtime | ❌ | ❌ **仍缺** (planned, not yet shipped as of 2026-07-03) | `handle_file_glob`（`file_ops.rs:804`）仍接收 `pattern: &str`；`matches` 仅是字符串列表 |
| P1-4 | list 静默截断 | ❌ | ✅ 已实现 | `truncated` 字段 |
| P1-5 | `file.move` 无 `base_sha256` | ❌ | ✅ 已实现 | `handle_file_move`（`file_ops.rs:1932`）接受 `base_sha256: Option<&str>`；不匹配时返回 `[file.sha_mismatch]` |
| P1-6 | `file.delete` 无 `if_match_sha256` | ❌ | ✅ 已实现 | `handle_file_delete`（`file_ops.rs:2043`）接受 `if_match_sha256: Option<&str>`；目录跳过乐观锁、依赖 `allow_recursive_delete` |
| P1-7 | `FileAccessPolicyStore::load_default()` 每请求读盘 | ❌ | ✅ 已实现 | `OnceLock<RwLock<CacheEntry>>` + `(size, mtime)` 快照失效；`save_raw_config` 主动 invalidate |
| P1-8 | `file.read` truncate 时 sha256 = 片段哈希 | ❌ | ✅ 已实现 | `file_sha256` 字段区分 |

## 3. P2 — 修复状态汇总

| ID | 项 | 当前状态 | 说明 |
|---|---|---|---|
| P2-TOCTOU | canonicalize 与 open 之间 race | ❌ 未修 | 依赖 roots 在受控目录 |
| P2-hardlink | 硬链接绕过 write_denies | ❌ 未修 | 需 inode 对照，显式推迟 |
| P2-exclude | 默认排除目录不够 | ⚠️ 部分 (planned expansion, not yet shipped as of 2026-07-03) | `DEFAULT_EXCLUDE_DIRS` 仍是 6 项 (`.git / node_modules / target / __pycache__ / .svn / .hg`)，见 `file_ops.rs:41-48`；`.venv/dist/build/.next` 等未覆盖；调用方可通过 `exclude_patterns` 自助补齐 |
| P2-errcode | 错误码漂移 | ✅ 已对齐 | 设计文档 `remote-file-transfer.md` / `bifrost-file-protocol.md` 已按实现重写 |
| P2-symlink | list/stat 缺 symlink_target | ✅ 已实现 | |
| P2-prev_sha | write/edit 缺 previous_sha256 | ✅ 已实现 | |
| P2-audit | handler 零 tracing | ✅ 已实现 | `executor.rs` 在每个 `file.*` 调用入口/出口发 `info!(target="audit.file", grant_id, method, path_hash, duration_ms, result, bytes, sha256, …)`；写类调用额外打 `file write audit` info 行 |
| P2-concurrent | 并发写同路径无 per-path 锁 | ❌ 未修 (planned, not yet shipped as of 2026-07-03) | 依赖 base_sha256 保底 |

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

## 5. 剩余 Backlog

### 已收尾（自上次刷新起合并到 HEAD）

- ✅ `file.move` + `file.delete` 乐观锁（`base_sha256` / `if_match_sha256`）
- ✅ `FileAccessPolicyStore` `OnceLock<RwLock<CacheEntry>>` + `(size, mtime)` 缓存
- ✅ Executor 级 `audit.file` tracing（per-op info!/warn! 出入对）
- ✅ `file.outline` 新 op（heuristic symbol outline，多语言 regex 表；`file_ops.rs:1417-1500`）
- ✅ `file.search` 多 pattern (`patterns` 数组) + `fixed_strings` + `word`

### 下一 PR 建议（planned, not yet shipped as of 2026-07-03）

1. `file.glob` 多 pattern + 返回 `mtime`（与 search OR-逻辑对齐；改 `handle_file_glob` 签名接受 `patterns: &[String]`）。
2. `file.search` 同文件命中聚合（返回 `{path, matches: [...]}` 形态以省 token；改 `handle_file_search` 输出模型）。
3. `DEFAULT_EXCLUDE_DIRS` 扩充：`.venv / dist / build / .next / .turbo / out / coverage`（`file_ops.rs:41`）。
4. 并发写同路径 per-path 锁（在 `base_sha256` 之上加一层 fast-fail，位置：`handle_file_write` / `handle_file_edit`）。

### 不做

- `file.watch`（单独 feature）
- `GIT binary patch`（下一大版本）
- 硬链接 inode 对照（需跨平台统一方案）

## 技术细节

### 主要源码入口

- `crates/bifrost-admin/src/remote_invoke/file_ops.rs`
  - `should_skip_dir`（第 51 行）
  - `handle_file_read_many`（第 514 行）
  - `handle_file_glob`（第 804 行）
  - `handle_file_search`（第 923 行）
  - `handle_file_outline`（第 1422 行）
  - `handle_file_move`（第 1932 行）
  - `handle_file_delete`（第 2043 行）
- `crates/bifrost-admin/src/remote_invoke/file_policy_store.rs`（policy `OnceLock` 缓存）
- `crates/bifrost-admin/src/remote_invoke/file_access_roots.rs`（roots 校验）
- `crates/bifrost-admin/src/remote_invoke/file_transfer.rs`（分块 upload/download 与 checksum）
- `crates/bifrost-admin/src/remote_invoke/executor.rs`（`audit.file` tracing 中心）

## CLI

- `bifrost remote file read <path> [--range]`
- `bifrost remote file list <dir>` / `stat` / `hash`
- `bifrost remote file glob <pattern>` / `search <pattern>`
- `bifrost remote file outline <path>`
- `bifrost remote file write <path> --from-local <local> [--base-sha256 <sha>] [--allow-overwrite]`
- `bifrost remote file edit <path> --patch <patch-file>`
- `bifrost remote file mkdir <path>` / `move <from> <to>` / `delete <path>`
- `bifrost remote file apply-patch <patch-file>`

## Web

`remote-invoke` 面板中的「File Access」子页展示：
- Grant 授权中的 roots / write_allows / write_denies 列表。
- Policy `(size, mtime)` 缓存命中/失效指标。
- 最近 `audit.file` tracing 抽样（可跳转到 grant 详情）。

## Admin API

- `POST /_bifrost/api/remote-invoke/exec`（`RemoteCommand.method = file.*`）。
- `GET /_bifrost/api/remote-invoke/file-access-config`（读取当前 policy 全景）。
- `PUT /_bifrost/api/remote-invoke/file-access-config`（写盘 + `save_raw_config` 主动 invalidate 缓存）。
- `GET /_bifrost/api/remote-invoke/calls?limit=N&before=<cursor>`（历史列表，包含 `file.*` 调用）。

## Sync 边界

- File policy 与 grant 均属本地状态，不参与账号 sync。
- `admin/remote_invoke_call_history/*.jsonl` 属本地历史，不参与 sync。

## Phase 1：核心 op 与安全模型（已完成）

见 P0 表。

## Phase 2：乐观锁与性能优化（已完成）

见 P1 表。

## Phase 3：可观测性与错误码统一（已完成）

见 P2 表已完成项。

## Phase 4：Coding agent 体验优化（进行中）

见「下一 PR 建议」四项：glob 多 pattern + mtime、search 同文件聚合、`DEFAULT_EXCLUDE_DIRS` 扩充、per-path 锁。

## 测试方案

### 单元测试

`file_ops.rs` 内内嵌 test module（第 3000+ 行起），覆盖：

- `handle_file_outline`：至少 1 例（第 3132 行）。
- `handle_file_glob`：多例，涵盖 exclude、denies、cursor 分页（第 3279 / 3874 / 5279 / 5296 / 5326 / 5393 / 5405 行）。
- `handle_file_move` 乐观锁：sha 不匹配、目录 fallback、跨盘处理（第 4751 / 4772 / 5111 / 5136 / 5166 行）。
- `handle_file_delete` 乐观锁：sha 匹配 / 不匹配 / recursive（第 5190 / 5207 / 5237 行）。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_file_e2e.sh` — 覆盖 write / edit / apply_patch / move / delete 幂等与乐观锁失败路径。
- `e2e-tests/tests/test_remote_invoke_e2e.sh` — 覆盖 grant 授权后 file op 允许/拒绝路径。

### human_tests

- `human_tests/remote-invoke-file.md` — 记录 macOS 上真实 coding agent 借助 `bifrost remote file` 修改仓库、rebase、apply-patch 的证据链。
- `human_tests/ci-windows-unit-tests.md` TC-CWUT-05 — 覆盖 `file_access_roots` 单元测试的 Windows 分支。

## Review / Fix / Test 闭环

- 每次改动 `file_ops.rs`：
  1. 补齐单元测试到 `mod tests`。
  2. 若涉及协议字段变化，同步 `design/bifrost-file-protocol.md`。
  3. `human_tests/remote-invoke-file.md` 追加真实执行记录。
- 每次改动 policy store 或缓存策略：
  1. 覆盖 `save_raw_config` 后主动 invalidate 的测试。
  2. 记录 `audit.file` 中新增字段。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin --lib remote_invoke::`
- `bash e2e-tests/tests/test_remote_invoke_file_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`

## 风险与决策

### 1. TOCTOU race

- 风险：canonicalize 与 open 之间路径可能被替换。
- 决策：暂不修，依赖 roots 位于受控目录；后续视需要引入 `openat` 系列 syscall。

### 2. 硬链接绕过 deny

- 风险：写方向 hardlink 至 root 内目标可能绕过 write_denies。
- 决策：不做跨平台 inode 对照，属于 policy 侧文档化限制。

### 3. Search / glob 单 pattern

- 风险：agent 每次多关键字需要多轮调用，token 成本增加。
- 决策：Phase 4 计划一并升级为多 pattern + mtime；短期 agent 可拼多轮请求。

### 4. 默认 exclude 目录不足

- 风险：`.venv / dist / build / .next / .turbo / out / coverage` 等常见目录会被 walker 遍历。
- 决策：Phase 4 扩充；短期通过 `exclude_patterns` 自助补齐。

### 5. Per-path 写并发

- 风险：多进程同时对同一路径写入可能出现 base_sha 竞争抖动。
- 决策：Phase 4 加 per-path fast-fail 锁；短期依赖 base_sha256 保底。
