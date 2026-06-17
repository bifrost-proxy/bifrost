# Bifrost Long-term Memory 长期方案

## 2026-05-02 方案修订：对齐文件化记忆标准

本方案废弃旧版在线召回主存储，不做旧版本兼容和迁移。Bifrost Agent 的长期记忆采用文件化 read-path 方案，并将所有记忆存放在用户自定义数据目录的 `agent/memory/` 下；Phase 2 运行状态仅保存在 `.memory_state.db`。

## 目录布局

记忆根目录：

- 正常运行：`$BIFROST_DATA_DIR/agent/memory/`
- 测试显式覆盖：临时 `$BIFROST_DATA_DIR/agent/memory/`

文件布局：

- `memory_summary.md`：随请求注入的轻量摘要。为空时不注入 memory instructions。
- `MEMORY.md`：可搜索的长期记忆索引。
- `raw_memories.md`：原始记忆汇总，用于后续 consolidation。
- `rollout_summaries/`：每次自动抽取产生的可追溯摘要。
- `skills/`：保留 memory skill 目录，用于后续 consolidation 生成/更新 skills。
- `.memory_state.db`：Phase 2 bounded input hash、处理计数、失败熔断状态。

禁止创建或依赖 `.phase2_state.json`。

## 读路径

turn 前执行：

1. 如果 `memories.use_memories == Some(false)`，直接短路。
2. 确保 `agent/memory/` 布局存在。
3. 读取 `memory_summary.md`，为空则不注入。
4. 非空时注入 read-path instructions，模型按需决定是否读取 `MEMORY.md`、`rollout_summaries/` 或 `skills/`。

Bifrost 不再在 turn 前做 topK 数据库召回，也不维护 `scope/kind/use_count/FTS` 等数据库字段。

## 写路径

### 显式写入

`/remember` 和 Admin append：

- 追加 `MEMORY.md`
- 追加 `memory_summary.md`
- 追加 `raw_memories.md`
- 显式写入不触发 Phase 2 时不创建 SQLite；自动写入完成 Phase 2 后创建 `.memory_state.db`

### 自动写入

当 `memories.generate_memories != Some(false)` 时，assistant turn 完成后用 `extract_model` 或主模型执行轻量抽取：

1. 输入当前 user message 与 assistant response。
2. 模型返回当前结构化 JSON：`rollout_summary`、`rollout_slug`、`raw_memory`。
3. `raw_memory` 非空时写入 `raw_memories.md`，并写入对应 `rollout_summaries/<slug>.md`。
4. 追加 `memory_summary.md` 的简短摘要。
5. 如果模型没有返回 `raw_memory`，本轮自动写入不落盘，也不走旧的单行 `memories` fallback。

自动写入完成后触发 Phase 2 consolidation。

## Phase 2 Consolidation

Bifrost 使用 `.memory_state.db` 存储 Phase 2 任务状态：

1. 获取 `agent/memory/.phase2.lock` 文件锁；锁存在且未过期时跳过本轮，避免多个 session 同时 consolidation 覆盖彼此结果。
2. 按 `memories.max_raw_memories_for_consolidation` 选择最近 N 条 raw/rollout 输入；未配置时使用内置默认值，且最小为 1。
3. 只基于“本轮实际进入 prompt 的 bounded input”计算稳定 hash。SQLite 只记录这批输入的 hash/count，不把被 retention 排除或截断之外的内容标记为已 consolidated。
4. 读取 `.memory_state.db` 中的 Phase 2 状态，如果 `last_input_hash` 相同则跳过。
5. 构造 consolidation prompt，输入包括：
   - 当前 `memory_summary.md`
   - 当前 `MEMORY.md`
   - selected `raw_memories.md` sections
   - selected `rollout_summaries/*.md`
6. 使用 `memories.consolidation_model`；未配置时复用主模型。
7. 模型返回 JSON：

```json
{
  "memory_summary": "- compact summary",
  "memory": "# Memory\n\n- durable fact",
  "skills": [
    {"name": "optional-skill", "skill_md": "---\nname: optional-skill\n---\n# Skill\n..."}
  ]
}
```

8. 成功后通过临时文件 + rename 原子替换 `memory_summary.md`、`MEMORY.md`，写入 `skills/<name>/SKILL.md`，并更新 SQLite Phase 2 状态。
9. SQLite Phase 2 状态记录 `last_input_hash`、`processed_input_count`、`total_input_count`、`has_more_inputs` 和 `updated_at_unix`。

如果模型输出非法 JSON，保留 turn-end append 产生的原始文件，不更新 Phase 2 状态，后续输入变化时可重试。

## 功能对齐情况

已对齐：

- 用户数据目录下的文件化 memory root。
- `memory_summary.md` 非空才注入 read-path instructions。
- `MEMORY.md`、`raw_memories.md`、`rollout_summaries/`、`skills/` 布局。
- `generate_memories` 与 `use_memories` 默认开启，显式 `false` 关闭。
- 自动抽取后跨独立 session 可消费记忆。
- `disable_on_external_context` 为唯一外部上下文污染开关字段。
- Admin PATCH 接收 memories 配置字段，包括 `min_rate_limit_remaining_percent`。
- Phase 2 consolidation：基于 bounded `raw_memories.md`/`rollout_summaries/` 输入重写 `MEMORY.md`、`memory_summary.md`，使用文件锁、SQLite 状态和原子替换，并支持生成 memory skills。

不实现：

- 数据库中的 stage1/stage2 job、lease、watermark、retry/backoff 表。Bifrost 只用 SQLite 保存 Phase 2 状态，不把记忆正文作为在线召回数据库。

仍未完全对齐，后续可继续补：

- memory root git baseline：Bifrost 当前没有在 `agent/memory/` 下初始化 git，也没有 `phase2_workspace_diff.md`（planned, not yet shipped as of 2026-06-16）。
- citation 解析的 UI 消费：`memory_citations.rs` + `memory/citation_consumer.rs` 已经把 `<oai-mem-citation>` 解析并喂给 `memory_guard`/`usage_tracking` 驱动 retention；面向终端用户隐藏 citation 标签的前端 UI 链路尚未落地（planned, not yet shipped as of 2026-06-16）。

## 测试方案

单元测试：

- `memory_read_instructions_use_agent_memory_root`：验证注入内容包含 `memory_summary.md`、`MEMORY.md`、`rollout_summaries/`、`skills/` 与 citation 要求。
- `empty_memory_summary_does_not_inject`：验证空 summary 不注入。
- `use_memories_disabled_short_circuits`：验证 `use_memories=false` 不读取文件。
- `remember_writes_codex_files_without_sqlite`：验证显式写入更新 `MEMORY.md`、`memory_summary.md`、`raw_memories.md`，且不生成 SQLite。
- `remember_auto_writes_codex_files_without_sqlite`：验证自动抽取写入 `MEMORY.md`、`memory_summary.md`、`raw_memories.md` 和 `rollout_summaries/`。
- `phase2_prompt_is_dirty_when_raw_memory_changes`：验证 raw/rollout 输入变化会改变 Phase 2 hash，并构造 consolidation prompt。
- `phase2_input_selection_uses_recent_bounded_inputs`：验证 Phase 2 只选择最近 N 条输入，不会把截断外旧输入误标为已处理。
- `phase2_lock_prevents_concurrent_consolidation`：验证 `.phase2.lock` 能阻止并发 consolidation。
- `apply_consolidated_memory_rewrites_summary_memory_and_skills`：验证 consolidation 输出重写 `memory_summary.md`、`MEMORY.md`，并写入 `skills/<name>/SKILL.md`。

E2E 测试：

- `e2e-tests/tests/test_long_term_memory_remember_recall.sh`：预置文件记忆，断言模型请求包含 read-path instructions。
  - 自动记忆用例的 mock Chat Completions 服务必须用实际 `EXTRACT_SYSTEM_PROMPT` / `CONSOLIDATION_SYSTEM_PROMPT` 常量识别 Phase 1/Phase 2 请求，避免 prompt 文案演进后落入普通对话兜底响应。
- `e2e-tests/tests/test_long_term_memory_human_api.sh`：启动真实 Bifrost 和 OpenAI-compatible mock，使用对话接口创建多个独立 session，验证“请记住我是独孤怼怼”自动沉淀并在新 session 消费。
  - mock 同时验证自动抽取请求和 Phase 2 consolidation 请求；请求识别必须匹配当前 `Memory Writing Agent: Phase 1` / `Memory Writing Agent: Phase 2` prompt 文案，并返回当前 Phase 1 的 `rollout_summary` / `rollout_slug` / `raw_memory` JSON 结构。
  - 真实对话路径会构造系统 prompt 和技能摘要，技能摘要裁剪必须保持 UTF-8 字符边界，避免中文 skill 描述触发 `String::truncate` panic。
  - 断言 `MEMORY.md` 中最终来源为 `phase2_consolidated`，`raw_memories.md` 保留 `source: auto_extract` 追溯材料。
  - 断言 `.memory_state.db` 的 Phase 2 状态记录 bounded input 元数据：`processed_input_count`、`total_input_count`、`has_more_inputs`。

真实场景测试：

- `human_tests/long-term-memory.md` 覆盖目录初始化、按需加载说明注入、显式写入、自动写入、Admin 文件 API、WebUI 文件视图、SQLite Phase 2 状态、跨独立 session 消费。
- 每次修改后必须按文档逐条执行并记录实际结果。

## 2026-05-03 硬化修订（P0/P1/P2 合并落地）

本次修订在不改变上节整体 read/write 语义的前提下，一次性收敛了 Phase 2 的并发与可用性、记忆管理的运维面、以及观测能力。要点如下。

### Phase 2 锁改用 `fs2` 文件锁

- `.phase2.lock` 内仍 best-effort 写入一行 `pid=<n> acquired_at=<ts>` 文本仅供人肉调试，互斥不再依赖该内容，也不再做 staleness 心跳判断。
- 运行态使用 `fs2::FileExt::try_lock_exclusive`；进程崩溃后 OS 会自动释放，避免旧版“pid 号复用”误判。
- 长摘要 append/轮转等共享写路径通过同一把 `agent/memory/.phase2.lock` 的 `lock_exclusive` 串行化，`append_line` 显式 `flush + sync_data + unlock`，保证崩溃后内容落盘且锁被释放。

### 稳健的 JSON 解析与熔断

- 抽取与 consolidation 的模型输出都通过 `strip_markdown_fences + extract_balanced_json` 兜底解析：
  - 优先剥离 ``` / ```json 栅栏。
  - 按括号配对扫描首个平衡 JSON 对象，字符串字面量与转义字符不会误断。
- SQLite Phase 2 状态记录 `failure_count` 与 `pinned_failure_hash`；同一 bounded input hash 连续失败 **5 次**后熔断，直到输入变化才解除。避免模型死循环浪费 token。

### 自动抽取改为后台 fire-and-forget

- `auto_extract_after_turn` 使用 `tokio::spawn`，assistant turn 不再被抽取阻塞。
- 抽取本体套 `tokio::time::timeout(MEMORY_EXTRACT_TIMEOUT_SECS = 30s)`。
- 触发的 Phase 2 consolidation 内部套 `MEMORY_CONSOLIDATION_TIMEOUT_SECS = 120s`。
- 任一超时只记录 telemetry，不影响主 turn。

### Memory Skills 与用户 skills 隔离

- 所有 consolidation 生成的 skills 写在 `agent/memory/skills/_memory/<name>/SKILL.md`。
- 写入前显式检查 `agent/memory/skills/<name>/` 是否由用户创建：命中则拒绝写入并记 `memory_skill_shadow_refused` telemetry，永不覆盖用户手写内容。

### Retention Sweep

- `prune_memory_artifacts` 在每轮 Phase 2 成功后运行，按以下配置收敛：
  - `max_unused_days`：超期未访问的 `rollout_summaries/*.md` 直接删除。
  - `max_rollout_age_days`：按 mtime 截断 rollout summaries。
  - `max_rollouts_per_startup`：每次启动最多保留最近 N 条 rollout。
  - `raw_memories.md` 超限时尾部段落迁入 `raw_memories.archive.md`。
- 所有删除/迁移前先抓 `.phase2.lock` 避免和并发 consolidation 冲突。

### `/forget` 与 PATCH 语义

- `forget_memory` 现在可命中显式写入和 `auto_extract` 条目；支持按 id 或 id 前缀匹配，命中唯一项才删除。
- Admin PATCH `/api/agent/memories/:id` 通过 `memory_runtime::replace_memory` 实现：按行定位目标条目，原子重写 `MEMORY.md` / `memory_summary.md` / `raw_memories.md` 中同一条的副本；sha 不一致自动回滚。

### Telemetry (`.telemetry.jsonl`)

- 事件以 JSON Lines 形式落盘到 `agent/memory/.telemetry.jsonl`，单行格式 `{ts,event,value,success,detail}`（`ts` 为 unix 秒）。
- 关键事件：`memory_extract_started/finished/timeout`、`phase2_started/finished/skipped/locked/circuit_open`、`memory_skill_shadow_refused`、`retention.*`、`forget`、`replace_memory`、`export_archive`、`import_archive` 等。
- 写入时对 `.telemetry.jsonl` 自身加 `fs2::lock_exclusive` 串行化，确保并发 session 的事件不会交叉撕裂；不依赖 `.phase2.lock`。
- 纯本地文件，不外发。

### 全量导出/导入

- GET `/api/agent/memories/export` 返回 `{archive_b64, bytes, file_count, memory_root}`，tar.gz 打包整个 `agent/memory/`（跳过 `.phase2.lock`）。
- POST `/api/agent/memories/import` 接 `{archive_b64}`：gunzip → `tar::Archive` → 逐项做 `is_safe_relative` 校验（拒绝绝对路径、`..`、盘符前缀），再原子落到 memory root 下。返回 `ImportReport { imported_files, imported_bytes, memory_root }`。
- 作为用户自备份/迁移机器的一等方案，无需 SQLite dump。

### 新增 / 更新的关键常量

```rust
pub const MEMORY_EXTRACT_TIMEOUT_SECS: u64 = 30;
pub const MEMORY_CONSOLIDATION_TIMEOUT_SECS: u64 = 120;
const MEMORY_CONSOLIDATION_FAILURE_LIMIT: usize = 5;
const MEMORY_SKILLS_SUBDIR: &str = "_memory";
```

### 新增单元测试（节选）

- `phase2_failure_counter_opens_breaker`：连续 5 次同 hash 失败进入熔断，输入变化后解除。
- `apply_consolidated_memory_preserves_user_authored_skill`：user skill 同名时拒绝覆盖，memory skill 落在 `_memory/` 子目录。
- `prune_memory_artifacts_caps_raw_memories`：raw 超限段落迁入 archive；超期 rollout 被清理。
- `telemetry_event_writes_jsonl`：并发写入不互相撕裂。
- `replace_memory_rewrites_content_line`：同一条目在三个 md 中同步替换；sha 变化则回滚。
- `extract_balanced_json_handles_nested_braces_in_strings`：字符串内的 `}` 不会错误终止解析。
- `phase2_lock_blocks_concurrent_append_line`：append 在 consolidation 持锁期间会排队，不会切碎行。

### 对齐情况更新

已补齐的标准行为：

- usage-based / age-based retention 已落地（`max_unused_days`、`max_rollout_age_days`、`max_rollouts_per_startup`），`memory/usage_tracking.rs` + state DB 的 `rollouts.usage_count`/`last_usage` 字段驱动 pruning。
- memory skills 写路径（与用户 skills 隔离）。
- 观测维度（通过本地 `.telemetry.jsonl`，不依赖数据库 telemetry）。
- 稳健 JSON 解析与抽取熔断。
- per-thread `ThreadMemoryMode`（`Enabled`/`Disabled`/`Polluted { reason }`）由 `memory/pollution.rs::PollutionDetector` 维护，受 `disable_on_external_context` 控制：当 MCP/web/search 等外部上下文进入时自动把该 session 置为 `Polluted`，extract 入口 `auto_extract_after_turn_with_pollution_check` 直接短路。
- citation 解析与 usage 上报：`memory_citations.rs` 解析 `<oai-mem-citation>` 块，`memory/citation_consumer.rs` 把命中的 rollout/skill 喂给 `MemoryUsageRecord` 与 `state_db::record_usage`。
- rate-limit gating：`memory_guard::rate_limit_ok` 按 `min_rate_limit_remaining_percent` 判断当前 rate-limit 余量，不足时跳过本轮 Phase 1/Phase 2 任务。
- state DB 扩展：`.memory_state.db` 除 `phase2_state` 外，已新增 `rollouts`（含 `phase1_status`/`ownership_token`/`usage_count`/`last_usage`）和 `leases`（DB-backed lease/heartbeat）；DB-backed `LeaseManager` 与文件锁 `Phase2LockGuard` 并存。

仍保持不实现（故意选择）：

- 不引入 SQLite，不使用 stage1/stage2 job/lease/watermark。
- 不实现 citation parser / UI，Bifrost 只注入 citation 要求。

### 迁移说明

- 老 `.phase2.lock` 内容（pid/ts 文本）首次启动会被 `fs2` 重建为 0 字节占位，无需手工清理。
- 旧 `skills/<name>/SKILL.md` 中由旧版本 consolidation 自动生成的记忆 skills 不会被自动迁入 `_memory/`；如需清理请手工 `rm -r skills/<name>`，新一轮 Phase 2 会在 `_memory/` 下重建。
- `rollout_summaries/` 历史文件不受影响，下一次 Phase 2 会按新的 retention 策略裁剪。
