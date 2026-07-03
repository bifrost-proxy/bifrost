# Bifrost Long-term Memory 长期方案

## 背景

Bifrost Agent 需要跨 session 复用「用户是谁 / 说过什么偏好 / 之前得到什么结论」这类持久信息。行业内主要有两条路径：

1. **在线召回主存储**：SQLite/向量库为一等，模型每轮 turn 前做 topK 召回、把结果直接注入 system prompt。
2. **文件化 read-path**：把记忆当成用户数据目录下的 Markdown 文件，模型按需读，不做在线召回。

Bifrost 早期尝试过路径 1，但存在明显问题：SQLite schema 迁移风险高、召回结果与用户可见文件不一致、跨 session/多机同步困难，且模型对结构化 topK 结果的信任度低于直接读文件。2026-05-02 修订之后，Bifrost 明确**采用文件化 read-path 方案**，废弃在线召回主存储，不做旧版本兼容和迁移，`.memory_state.db` 仅承担 Phase 2 consolidation 的运行状态。

2026-05-03 又追加了 P0/P1/P2 硬化合并：`fs2` 文件锁替代自定义 pid 心跳、稳健 JSON 解析 + 熔断、自动抽取后台化 + 超时、memory skills 与用户 skills 隔离、retention sweep、`/forget` + PATCH 支持 `auto_extract` 条目、本地 telemetry、全量导出/导入。

主要实现位于 `crates/agent/src/memory/`（`layout.rs` / `read_path.rs` / `write.rs` / `extract.rs` / `consolidation.rs` / `lock.rs` / `retention.rs` / `constants.rs` / `pollution.rs` / `citation_consumer.rs` / `state_db.rs` / `sub_agent.rs`）与 `crates/agent/src/memory_prompts.rs` / `memory_extensions.rs` / `memory_citations.rs`；Admin 侧接口在 `crates/bifrost-admin/src/handlers/agent_memories.rs`。

## 用户目标验证清单

### 必须实现

- 记忆根目录固定在 `$BIFROST_DATA_DIR/agent/memory/`；仅测试可覆盖到临时目录。
- 目录布局：`memory_summary.md` / `MEMORY.md` / `raw_memories.md` / `rollout_summaries/` / `skills/` / `.memory_state.db` / `.telemetry.jsonl` / `.phase2.lock`。
- 每 turn 前根据 `memories.use_memories`（默认 `true`，`Some(false)` 关闭）决定是否注入 read-path instructions；`memory_summary.md` 为空则完全不注入。
- 显式写入（`/remember`、Admin append）追加 `MEMORY.md` / `memory_summary.md` / `raw_memories.md`，此时不创建 SQLite。
- 自动写入（`memories.generate_memories != Some(false)`）在 assistant turn 后 fire-and-forget 抽取，写入 `raw_memories.md` + `rollout_summaries/<slug>.md` 并追加 `memory_summary.md`。
- Phase 2 consolidation 使用 `fs2::FileExt::try_lock_exclusive` 抢 `.phase2.lock`；相同 bounded input hash 则跳过；模型返回 JSON 通过 `strip_markdown_fences + extract_balanced_json` 兜底解析。
- Phase 2 输出通过临时文件 + `rename` 原子替换 `memory_summary.md` / `MEMORY.md`，memory skills 写入 `agent/memory/skills/_memory/<name>/SKILL.md`。
- Phase 2 失败连续 5 次同 hash 熔断（`MEMORY_CONSOLIDATION_FAILURE_LIMIT`），bounded input 变化后自动解除。
- Retention sweep 按 `max_unused_days` / `max_rollout_age_days` / `max_rollouts_per_startup` / raw 超限迁入 `raw_memories.archive.md` 收敛。
- `/forget` 支持按 id 或前缀命中显式写入和 `auto_extract` 条目；Admin PATCH `/api/agent/memories/:id` 走 `replace_memory` 原子重写三份 md，sha 不一致自动回滚。
- 本地 `.telemetry.jsonl` 记录 `memory_extract_started/finished/timeout`、`phase2_started/finished/skipped/locked/circuit_open`、`memory_skill_shadow_refused`、`retention.*`、`forget`、`replace_memory`、`export_archive`、`import_archive` 等事件；`fs2::lock_exclusive` 串行化。
- Admin 支持 `GET /api/agent/memories/export`（tar.gz + base64）和 `POST /api/agent/memories/import`（`is_safe_relative` 校验，原子落到 memory root）。
- `disable_on_external_context` 为唯一外部上下文污染开关；`memory/pollution.rs::PollutionDetector` 维护 per-thread `ThreadMemoryMode::Enabled` / `Disabled` / `Polluted { reason }`。
- Citation 解析：`memory_citations.rs` 解析 `<oai-mem-citation>`；`memory/citation_consumer.rs` 通过 `MemoryUsageRecord` 与 `state_db::record_usage` 上报 usage，驱动 retention。
- Rate-limit gating：`memory_guard::rate_limit_ok` 按 `min_rate_limit_remaining_percent` 判断当前 rate-limit 余量，不足时跳过本轮 Phase 1/Phase 2。

### 必须不破坏

- 用户目录下已有的 `MEMORY.md` / `memory_summary.md` / `raw_memories.md` 被视为用户内容，不做隐式格式迁移。
- 用户在 `agent/memory/skills/<name>/` 手动放置的 skill 永远不被覆盖；同名命中记 `memory_skill_shadow_refused` telemetry。
- `.telemetry.jsonl` 纯本地，不外发。
- 抽取超时 / consolidation 超时 / 熔断只记 telemetry，不影响主 turn。
- 长摘要 append / 轮转等共享写路径通过 `agent/memory/.phase2.lock` 的 `lock_exclusive` 串行化，`append_line` 显式 `flush + sync_data + unlock`。

### 必须真实验证

- 手动 `/remember` 后重启 CLI，新 session 能在 `memory_summary.md` / `MEMORY.md` 中看到内容。
- 「请记住我是独孤怼怼」自动沉淀，跨独立 session 消费；`.memory_state.db` 中 `processed_input_count` / `total_input_count` / `has_more_inputs` 有值；`raw_memories.md` 保留 `source: auto_extract`，`MEMORY.md` 最终来源 `phase2_consolidated`。
- 并发 session 抢 Phase 2 锁：只有一个进程真正执行 consolidation，另一个跳过并写 `phase2_started/locked` telemetry。
- 用户自建 `skills/foo/SKILL.md` 时，consolidation 尝试写同名 skill 被拒并记 telemetry。

## 产品语义

### 文件即真相（file-is-truth）

- 用户可以直接 `cat` / `git diff` / 手工编辑 memory 文件；Bifrost 不做「文件里内容和 SQLite 内容双写」的一致性苦活。
- 模型看到的记忆等于文件里的内容；调试链路只看文件即可。

### Phase 1（抽取）与 Phase 2（consolidation）分工

- Phase 1（`memory/extract.rs` + `memory_prompts.rs::EXTRACT_SYSTEM_PROMPT`）：每次 assistant turn 后 fire-and-forget，快速产出 `rollout_summary` / `rollout_slug` / `raw_memory` JSON，无副作用地追加原始材料。
- Phase 2（`memory/consolidation.rs` + `memory_prompts.rs::CONSOLIDATION_SYSTEM_PROMPT`）：抢锁后基于「本轮实际进入 prompt 的 bounded input」重写 `memory_summary.md` / `MEMORY.md` 和 memory skills。

### 每一份状态的角色

- `memory_summary.md`：随 read-path 说明注入的轻量摘要；为空则完全不注入。
- `MEMORY.md`：模型按需读取的长期记忆索引，consolidation 定期覆写。
- `raw_memories.md` / `raw_memories.archive.md`：原始材料，超限迁移到 archive。
- `rollout_summaries/<slug>.md`：每次自动抽取的可追溯摘要。
- `skills/`：保留用户手写 skills；`skills/_memory/` 保留 consolidation 生成 skills。
- `.memory_state.db`：Phase 2 状态（bounded input hash / processed count / failure count / pinned hash）、`rollouts`（含 `phase1_status` / `ownership_token` / `usage_count` / `last_usage`）、`leases`（DB-backed lease/heartbeat）。
- `.phase2.lock`：`fs2` 文件锁；文件内容仅供人工调试。
- `.telemetry.jsonl`：本地事件流。

## 技术细节

### 读路径

`memory/read_path.rs` 在 turn 前执行：

1. 如果 `memories.use_memories == Some(false)` 直接短路。
2. 确保 `agent/memory/` 布局存在（layout.rs `ensure_layout`）。
3. 读取 `memory_summary.md`，为空则不注入；非空注入 read-path instructions，指示模型按需读取 `MEMORY.md` / `rollout_summaries/` / `skills/` 并使用 `<oai-mem-citation>` 标签。
4. Bifrost 不做 topK 数据库召回，也不维护 `scope/kind/use_count/FTS` 数据库字段。

### 写路径

- 显式：`memory/write.rs::remember`。
- 自动：`memory_extensions.rs::auto_extract_after_turn` -> `tokio::spawn` -> `tokio::time::timeout(MEMORY_EXTRACT_TIMEOUT_SECS = 30s)`。若 `PollutionDetector` 已置为 `Polluted { reason }`，`auto_extract_after_turn_with_pollution_check` 直接短路。
- 触发 Phase 2 时套 `tokio::time::timeout(MEMORY_CONSOLIDATION_TIMEOUT_SECS = 120s)`，超时只记 telemetry。

### Phase 2 consolidation 流程

1. `fs2::FileExt::try_lock_exclusive` 抢 `.phase2.lock`；进程崩溃后 OS 自动释放。
2. 按 `memories.max_raw_memories_for_consolidation` 选择最近 N 条 raw/rollout 输入；未配置时使用内置默认值，最小为 1。
3. 只基于「本轮实际进入 prompt 的 bounded input」计算稳定 hash；SQLite 只记录这批输入的 hash/count。
4. 读取 `.memory_state.db`，如果 `last_input_hash` 相同则跳过。
5. 构造 consolidation prompt：`memory_summary.md` + `MEMORY.md` + selected `raw_memories.md` sections + selected `rollout_summaries/*.md`。
6. 使用 `memories.consolidation_model`；未配置时复用主模型。
7. 模型返回：

   ```json
   {
     "memory_summary": "- compact summary",
     "memory": "# Memory\n\n- durable fact",
     "skills": [
       {"name": "optional-skill", "skill_md": "---\nname: optional-skill\n---\n# Skill\n..."}
     ]
   }
   ```

8. 成功后临时文件 + `rename` 原子替换 `memory_summary.md` / `MEMORY.md`；memory skills 写入 `skills/_memory/<name>/SKILL.md`（预先检查同名 `skills/<name>/` 是否用户创建，命中拒写并记 `memory_skill_shadow_refused`）。
9. SQLite Phase 2 状态记录 `last_input_hash` / `processed_input_count` / `total_input_count` / `has_more_inputs` / `updated_at_unix` / `failure_count` / `pinned_failure_hash`。
10. 非法 JSON 时保留 turn-end append，不更新 Phase 2 状态；连续 5 次同 hash 失败进入熔断，输入变化后解除。
11. Phase 2 成功后运行 `prune_memory_artifacts`，按 `max_unused_days` / `max_rollout_age_days` / `max_rollouts_per_startup` 收敛，raw 超限尾部段落迁入 `raw_memories.archive.md`；操作前先抢 `.phase2.lock`。

### 关键常量（`crates/agent/src/memory/constants.rs`）

```rust
pub const MEMORY_EXTRACT_TIMEOUT_SECS: u64 = 30;
pub const MEMORY_CONSOLIDATION_TIMEOUT_SECS: u64 = 120;
const MEMORY_CONSOLIDATION_FAILURE_LIMIT: usize = 5;
const MEMORY_SKILLS_SUBDIR: &str = "_memory";
```

### Admin API

- `POST /api/agent/memories/remember`：显式追加。
- `DELETE /api/agent/memories/:id`：`/forget`，按 id 或前缀命中显式写入和 `auto_extract` 条目；命中唯一项才删。
- `PATCH /api/agent/memories/:id`：`memory_runtime::replace_memory` 按行定位目标，原子重写三份 md；sha 不一致回滚。
- `GET /api/agent/memories/export`：返回 `{archive_b64, bytes, file_count, memory_root}`；跳过 `.phase2.lock`。
- `POST /api/agent/memories/import`：`{archive_b64}` -> gunzip -> `tar::Archive` -> `is_safe_relative` 校验（拒绝绝对路径、`..`、盘符前缀）-> 原子落到 memory root；返回 `ImportReport { imported_files, imported_bytes, memory_root }`。
- Admin PATCH 接收 memories 配置字段，包括 `min_rate_limit_remaining_percent` / `use_memories` / `generate_memories` / `disable_on_external_context` / `consolidation_model` / `extract_model` / `max_raw_memories_for_consolidation` / `max_unused_days` / `max_rollout_age_days` / `max_rollouts_per_startup`。

## CLI + Web + Admin API

- CLI：`bifrost` agent 侧 `/remember <text>`、`/forget <id_prefix>` 交互命令；`memories.use_memories=false` 可通过 `--memories-off` 或配置文件关闭。
- Web：Admin UI 提供 memory 文件视图（`MEMORY.md` / `memory_summary.md` / `raw_memories.md` / `rollout_summaries/`）与 export / import 入口。
- Admin API：见上表。

## Sync 边界

- 记忆是**每个用户本机数据目录**的一等文件，**不参与** Bifrost 规则 / Group 的远端 sync。
- 用户想跨机迁移，走 `GET /api/agent/memories/export` -> 目标机 `POST /api/agent/memories/import` 的显式路径；解压时 `is_safe_relative` 校验拒绝逃逸。
- 已废弃的旧 SQLite 主存储不做迁移；老 `.phase2.lock` 内容（pid/ts 文本）首次启动会被 `fs2` 重建为 0 字节占位，无需手工清理。
- 旧 `skills/<name>/SKILL.md` 中由旧版本 consolidation 自动生成的记忆 skills 不会被自动迁入 `_memory/`；如需清理请手工 `rm -r skills/<name>`，新一轮 Phase 2 在 `_memory/` 下重建。

## Phase 1-4

### Phase 1：文件化底座

- 目录 layout / 读路径 / 显式写入 / 自动 fire-and-forget 抽取；`use_memories` / `generate_memories` 默认开启，`Some(false)` 关闭。
- `disable_on_external_context` + `ThreadMemoryMode`。

### Phase 2：Consolidation + 状态硬化

- `fs2::try_lock_exclusive`；bounded input hash 跳过；失败熔断 5 次；`strip_markdown_fences + extract_balanced_json`；memory skills 与用户 skills 隔离。
- SQLite Phase 2 状态与 DB-backed lease/heartbeat；rollouts usage_count / last_usage 驱动 retention。

### Phase 3：Retention + Telemetry + 管理面

- `prune_memory_artifacts` 按配置收敛；raw 超限迁 archive。
- `.telemetry.jsonl` `fs2::lock_exclusive` 串行化；关键事件覆盖 extract / phase2 / retention / forget / replace_memory / export / import。
- `/forget`、Admin PATCH `replace_memory`、Admin GET/POST export/import。

### Phase 4：Citation 与 UI 消费

- `memory_citations.rs` + `memory/citation_consumer.rs` 已完成 `<oai-mem-citation>` 解析并喂给 `memory_guard` / `usage_tracking` 驱动 retention。
- 面向终端用户隐藏 citation 标签的前端 UI 链路尚未落地（planned, not yet shipped as of 2026-06-16）。
- memory root git baseline（`agent/memory/` 下初始化 git、`phase2_workspace_diff.md`）尚未落地（planned, not yet shipped as of 2026-06-16）。

## 测试方案

### 单元测试（`crates/agent/src/memory/tests.rs` + `pollution.rs` + `sub_agent.rs`）

真实存在测试，可用 `cargo test -p bifrost-agent memory::` 覆盖：

- `memory_read_instructions_use_agent_memory_root`（读路径注入包含 summary / MEMORY / rollout / skills / citation）
- `empty_memory_summary_does_not_inject`
- `use_memories_disabled_short_circuits`
- `remember_writes_codex_files_without_sqlite`
- `remember_auto_writes_codex_files_without_sqlite`
- `phase2_prompt_is_dirty_when_raw_memory_changes`
- `phase2_input_selection_uses_recent_bounded_inputs`
- `phase2_lock_prevents_concurrent_consolidation`
- `phase2_lock_blocks_concurrent_append_line`
- `phase2_failure_counter_opens_breaker`
- `apply_consolidated_memory_rewrites_summary_memory_and_skills`
- `apply_consolidated_memory_preserves_user_authored_skill`
- `prune_memory_artifacts_caps_raw_memories`
- `telemetry_event_writes_jsonl`
- `replace_memory_rewrites_content_line`
- `extract_balanced_json_handles_nested_braces_in_strings`
- `phase2_agent_tools_have_correct_definitions`
- `pollution` 模块：`ThreadMemoryMode` 判定、`PollutionDetector` 由 MCP/web/search 触发 `Polluted` 的多组用例。

### E2E 测试

- `e2e-tests/tests/test_long_term_memory_remember_recall.sh`：预置文件记忆，断言模型请求包含 read-path instructions。自动记忆用例的 mock Chat Completions 服务必须用 `EXTRACT_SYSTEM_PROMPT` / `CONSOLIDATION_SYSTEM_PROMPT` 常量识别 Phase 1/Phase 2 请求，避免 prompt 文案演进后落入普通对话兜底响应。
- `e2e-tests/tests/test_long_term_memory_human_api.sh`：启动真实 Bifrost + OpenAI-compatible mock，通过对话接口创建多个独立 session，验证「请记住我是独孤怼怼」自动沉淀并在新 session 消费。
  - mock 识别 Phase 1（`Memory Writing Agent: Phase 1`）与 Phase 2（`Memory Writing Agent: Phase 2`）prompt 文案，返回当前 Phase 1 `rollout_summary` / `rollout_slug` / `raw_memory` JSON 结构。
  - 真实对话路径会构造 system prompt 和技能摘要，技能摘要裁剪必须保持 UTF-8 字符边界，避免中文 skill 描述触发 `String::truncate` panic。
  - 断言 `MEMORY.md` 最终来源为 `phase2_consolidated`；`raw_memories.md` 保留 `source: auto_extract`；`.memory_state.db` 的 Phase 2 状态记录 `processed_input_count` / `total_input_count` / `has_more_inputs`。
- `crates/bifrost-e2e/src/tests/im_gateway_agent.rs`：IM 网关侧长任务下 memory 抽取不阻塞主 turn 的回归。

### 真实场景测试

- `human_tests/long-term-memory.md`：覆盖目录初始化、按需加载说明注入、显式写入、自动写入、Admin 文件 API、WebUI 文件视图、SQLite Phase 2 状态、跨独立 session 消费、`/forget`、Admin PATCH `replace_memory`、`export` / `import` 双向回归、并发 session 抢 Phase 2 锁、用户手写 skill 命中拒写。
- 每次修改后必须按文档逐条执行并记录实际结果。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 用户目标：文件即真相 / read-path 按需注入 / Phase 1 + Phase 2 分工 / 用户 skills 保护 / retention / telemetry / export & import / pollution / rate-limit gating。
- 变更范围：`git status --short` 覆盖 `crates/agent/src/memory/`、`memory_prompts.rs`、`memory_extensions.rs`、`memory_citations.rs`、`crates/bifrost-admin/src/handlers/agent_memories.rs`、`e2e-tests/tests/test_long_term_memory_*`、`human_tests/long-term-memory.md`。
- 重点：`fs2` 锁 vs 旧 pid/心跳锁；`extract_balanced_json` 是否覆盖字符串内 `}`；Phase 2 熔断解锁条件；memory skills `_memory/` 隔离；`is_safe_relative` 是否拒绝所有逃逸形态。
- 复测：`cargo test -p bifrost-agent memory::`、`bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`、`bash e2e-tests/tests/test_long_term_memory_human_api.sh`。

### 第 2 轮

- 再次核对 telemetry 事件名（`phase2_started` / `phase2_finished` / `phase2_skipped` / `phase2_locked` / `circuit_open` / `memory_skill_shadow_refused` / `retention.*` / `forget` / `replace_memory` / `export_archive` / `import_archive`）稳定可测试。
- 并发场景：两个 session 抢锁、快速连续 turn 触发 Phase 2、长 skill 摘要 UTF-8 边界。
- 失败场景：非法 JSON、rate-limit 不足、consolidation 超时。

## 校验要求

- `cargo test -p bifrost-agent memory::`
- `bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`
- `bash e2e-tests/tests/test_long_term_memory_human_api.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 按仓库规则执行 `cargo test --workspace --all-features`。

## 风险与决策

- **不引入在线召回主存储**是刻意选择：文件方案对用户可见、可 diff、可备份；性能损失换来的调试性与信任度提升值得。
- **不使用 SQLite 作为记忆正文**：`.memory_state.db` 仅保存 Phase 2 状态、rollouts usage、DB-backed lease；正文永远在文件。
- **Phase 2 锁改用 `fs2`**：旧版自定义 pid/心跳锁在进程崩溃 + pid 复用时会误判互斥失败，`fs2` 依赖 OS 自动释放，正确性明显更好。
- **memory skills 与用户 skills 隔离**：`_memory/` 子目录避免 consolidation 覆盖用户手写内容；命中拒写并 telemetry。
- **本地 telemetry 不外发**：所有事件仅落 `.telemetry.jsonl`，禁止走网络；`fs2::lock_exclusive` 串行化避免并发撕裂。
- **旧数据不迁移**：老 `.phase2.lock` / 旧版 SQLite 记录 / 旧 `skills/<name>/SKILL.md` 中的 auto-generated 记忆 skills 都不自动迁移；用户可手工 `rm` 后新一轮 Phase 2 会在 `_memory/` 下重建。
- **Citation UI 与 git baseline 尚未落地**：明确标注 planned, not yet shipped as of 2026-06-16；不影响当前 read-path 与 consolidation 正确性。
