# Bifrost Long-term Memory 长期方案

## 2026-05-02 方案修订：对齐 Codex 文件记忆

本方案废弃原 SQLite/`crates/memory` 在线主存储，不做旧版本兼容和迁移。Bifrost Agent 的长期记忆复用 Codex 的文件化 read-path，并将所有记忆存放在用户自定义数据目录的 `agent/memory/` 下。

## 目录布局

记忆根目录：

- 正常运行：`$BIFROST_DATA_DIR/agent/memory/`
- 测试显式覆盖：`$BIFROST_AGENT_HOME/memory/`

文件布局：

- `memory_summary.md`：随请求注入的轻量摘要。为空时不注入 memory instructions。
- `MEMORY.md`：可搜索的长期记忆索引。
- `raw_memories.md`：Codex-style 原始记忆汇总，用于后续 consolidation。
- `rollout_summaries/`：每次自动抽取产生的可追溯摘要。
- `skills/`：保留 Codex memory skill 目录，用于后续 consolidation 生成/更新 skills。
- `.phase2_state.json`：无数据库 Phase 2 的 bounded input hash 与处理计数状态，用于判断是否需要再次 consolidation。

禁止创建或依赖 `memories.sqlite`。

## 读路径

turn 前执行：

1. 如果 `memories.use_memories == Some(false)`，直接短路。
2. 确保 `agent/memory/` 布局存在。
3. 读取 `memory_summary.md`，为空则不注入。
4. 非空时注入 Codex `read_path.md` 同构 instructions，模型按需决定是否读取 `MEMORY.md`、`rollout_summaries/` 或 `skills/`。

Bifrost 不再在 turn 前做 topK 数据库召回，也不维护 `scope/kind/use_count/FTS` 等数据库字段。

## 写路径

### 显式写入

`/remember` 和 Admin append：

- 追加 `MEMORY.md`
- 追加 `memory_summary.md`
- 追加 `raw_memories.md`
- 不创建 SQLite

### 自动写入

当 `memories.generate_memories != Some(false)` 时，assistant turn 完成后用 `extract_model` 或主模型执行轻量抽取：

1. 输入当前 user message 与 assistant response。
2. 模型返回 `{"memories":["..."]}`。
3. 过滤空项和同批重复项。
4. 写入 `MEMORY.md`、`memory_summary.md`、`raw_memories.md`。
5. 为每条自动记忆写入 `rollout_summaries/<timestamp>-<hash>-<session>.md`。

自动写入完成后触发无数据库 Phase 2 consolidation。

## Phase 2 Consolidation

Bifrost 不复制 Codex 的 state DB job/lease/watermark 表，而是在 `agent/memory/` 内使用文件状态实现 Phase 2：

1. 获取 `agent/memory/.phase2.lock` 文件锁；锁存在且未过期时跳过本轮，避免多个 session 同时 consolidation 覆盖彼此结果。
2. 按 `memories.max_raw_memories_for_consolidation` 选择最近 N 条 raw/rollout 输入；未配置时使用内置默认值，且最小为 1。
3. 只基于“本轮实际进入 prompt 的 bounded input”计算稳定 hash。`.phase2_state.json` 只记录这批输入的 hash/count，不把被 retention 排除或截断之外的内容标记为已 consolidated。
4. 读取 `.phase2_state.json`，如果 `last_input_hash` 相同则跳过。
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

8. 成功后通过临时文件 + rename 原子替换 `memory_summary.md`、`MEMORY.md`、`.phase2_state.json`，并写入 `skills/<name>/SKILL.md`。
9. `.phase2_state.json` 记录 `last_input_hash`、`processed_input_count`、`total_input_count`、`has_more_inputs` 和 `updated_at_unix`。

如果模型输出非法 JSON，保留 turn-end append 产生的原始文件，不更新 Phase 2 状态，后续输入变化时可重试。

## Codex 对齐情况

已对齐：

- 用户数据目录下的文件化 memory root。
- `memory_summary.md` 非空才注入 Codex-style read-path instructions。
- `MEMORY.md`、`raw_memories.md`、`rollout_summaries/`、`skills/` 布局。
- `generate_memories` 与 `use_memories` 默认开启，显式 `false` 关闭。
- 自动抽取后跨独立 session 可消费记忆。
- `disable_on_external_context` 与旧 alias `no_memories_if_mcp_or_web_search` 的配置字段保留。
- Admin PATCH 接收 Codex memories 配置字段，包括 `min_rate_limit_remaining_percent`。
- 无数据库 Phase 2 consolidation：基于 bounded `raw_memories.md`/`rollout_summaries/` 输入重写 `MEMORY.md`、`memory_summary.md`，使用文件锁和原子替换，并支持生成 memory skills。

不复制：

- Codex state DB 中的 stage1/stage2 job、lease、watermark、retry/backoff 表。用户要求 Bifrost 不使用数据库存储记忆。

仍未完全对齐，后续可继续补：

- memory root git baseline：Bifrost 当前没有在 `agent/memory/` 下初始化 git，也没有 `phase2_workspace_diff.md`。
- per-thread `ThreadMemoryMode`：Bifrost 当前是配置级开关，没有 Codex 的 thread metadata `enabled/disabled/polluted`。
- `disable_on_external_context` 的污染语义：字段存在，但尚未在 MCP/web/search 等外部上下文进入时自动禁用该 session 的 memory generation。
- memory citation 解析与隐藏：Bifrost 注入 `<oai-mem-citation>` 要求，但还没有 Codex 的 citation parser/telemetry/UI 消费链路。
- usage-based retention：Bifrost 当前不维护 `last_usage/use_count`，`max_unused_days` 等字段尚未驱动 pruning；Phase 2 已支持基于最近 N 条 raw/rollout 输入的 bounded selection。
- rate-limit gating：字段可配置，但自动抽取尚未按 `min_rate_limit_remaining_percent` 跳过低余量窗口。

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

- `e2e-tests/tests/test_long_term_memory_remember_recall.sh`：预置文件记忆，断言模型请求包含 Codex-style read-path instructions。
- `e2e-tests/tests/test_long_term_memory_human_api.sh`：启动真实 Bifrost 和 OpenAI-compatible mock，使用对话接口创建多个独立 session，验证“请记住我是独孤怼怼”自动沉淀并在新 session 消费。
  - mock 同时验证自动抽取请求和 Phase 2 consolidation 请求。
  - 断言 `MEMORY.md` 中最终来源为 `phase2_consolidated`，`raw_memories.md` 保留 `source: auto_extract` 追溯材料。
  - 断言 `.phase2_state.json` 记录 bounded input 元数据：`processed_input_count`、`total_input_count`、`has_more_inputs`。

真实场景测试：

- `human_tests/long-term-memory.md` 覆盖目录初始化、按需加载说明注入、显式写入、自动写入、Admin 文件 API、WebUI 文件视图、不创建 SQLite、跨独立 session 消费。
- 每次修改后必须按文档逐条执行并记录实际结果。
