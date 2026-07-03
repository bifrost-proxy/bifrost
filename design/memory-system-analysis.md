# Bifrost Memory 系统现状分析

> 首次生成 2026-05-02，最近对齐 2026-07-03。
> 目的：把 Agent Memory 子系统当前**实现事实**用设计文档语言完整收敛，供后续 review / 修复 / 测试。
> 范围：`crates/agent/src/memory/`、`crates/agent/src/session/turn_loop.rs`、`crates/agent/src/persistence.rs`、`crates/bifrost-admin/src/handlers/im_gateway.rs`、`web/src/pages/Settings/tabs/agent/MemoriesSection.tsx` 与 human_tests。

## 背景

Bifrost Agent 需要跨 session 记住用户偏好、任务结论、长期事实。过去只有 in-turn 上下文和会话事件 JSONL 两层，无法在新 session 中主动召回历史。现在仓库落地了一套**文件式长期记忆子系统**：

- 抽取：`memory::extract` 每 turn 结束后把新的原子事实写入 `raw_memories.md`。
- 巩固：`memory::consolidation` 把 raw 合并/去重/裁剪进 `MEMORY.md`。
- 摘要：`memory_summary.md` 供 developer message 注入。
- 召回：`memory::recall_system_message()` 注入摘要 + MCP 工具 `memory/list|read|search` 让模型按需读取。
- 显式入口：`/remember`、`/forget` slash 命令 + `MemoriesConfig` Settings UI。

没有向量库 / embedding / topK rank；`state_db.rs` 只是租约与心跳协调，不承担 memory 内容存储。本文档以设计模板重整该子系统的产品语义、技术细节、CLI/Admin/Web、Sync 边界、测试与风险。

## 用户目标验证清单

### 必须实现（已落地）

- Agent 启动或首次访问 memory 时调用 `ensure_memory_layout()` 初始化 `$agent_home/memory/`：`MEMORY.md`、`memory_summary.md`、`raw_memories.md`、`rollout_summaries/`、`skills/`、`extensions/`（`crates/agent/src/memory/layout.rs:9-31`）。
- `MemoriesConfig` 字段全部被消费：`use_memories`、`generate_memories`、`extract_model`、`consolidation_model`、`max_raw_memories`、`max_unused_days`、`max_rollout_age_days`、`disable_on_external_context` 等。
- 每轮 turn 结束触发 `auto_extract_after_turn_with_pollution_check_blocking()` 做 phase-1 抽取（`turn_loop.rs:2394`），后台异步 phase-2 巩固（`memory::consolidation`）。
- 每轮 turn 开始注入 memory developer message：`recall_system_message()` 读取 `memory_summary.md`、按 token 截断、拼 `READ_PATH_TEMPLATE`（`turn_loop.rs:1725`）。
- Compaction（pre-turn / mid-turn / 手动 `/compact`）都会通过 `record_compaction_event()` 写入 JSONL 事件流。
- 用户显式入口：`/remember <text>` / `/forget <id|last>`（`crates/agent/src/slash.rs:78-194`）；MCP 工具 `memory/list`、`memory/read`、`memory/search`（`crates/agent/src/memory/mcp_tools.rs`）。
- Settings UI 提供 Memories 子区域（`web/src/pages/Settings/tabs/agent/MemoriesSection.tsx`），Admin PATCH 支持 `memories` 字段。
- 保留策略 `retention.rs`：`max_unused_days` / `max_rollout_age_days` 到期清理。
- JSONL history `max_bytes` 强制裁剪（`persistence.rs:426-450`），配合 90 天清理（`im_gateway.rs:543-556`）。

### 必须不破坏

- 关闭 `use_memories`：不注入 developer message，模型行为退回原状。
- 关闭 `generate_memories`：不做 phase-1 抽取和 phase-2 巩固，只保留 JSONL。
- `disable_on_external_context = true` 且 session 携带外部大上下文：跳过抽取，避免污染。
- 用户 `/forget` 后，下一 turn `recall_system_message()` 内容不再包含该条。
- Admin `/agent/sessions*` 路由与 90 天清理保持不变。
- 无向量库依赖；离线可用；不引入外部服务。

### 必须真实验证

- 单元：`crates/agent/src/memory/tests.rs` 覆盖 layout、read_path、extract、consolidation、write、search、pollution、retention、mcp_tools。
- 单元：`crates/agent/src/persistence.rs::record_compaction_event_round_trip`。
- 单元：`crates/agent/src/session/tests.rs::test_record_compaction_event_includes_emergency_and_total_tokens`。
- E2E：`crates/bifrost-e2e/src/tests/im_gateway_agent.rs` 覆盖 Feishu → Agent → memory 全链路。
- 真实场景：`human_tests/im-gateway-agent.md`、`human_tests/agent-session-persistence.md`（`/compact`、`/remember`、`/forget`、resume）。

## 产品语义

### 三层存储

| 层 | 介质 | 生命周期 | 说明 |
| --- | --- | --- | --- |
| Session history | 进程内 `AgentSession.history` | 单 session；`/compact` 时被摘要替换 | 模型输入的直接上下文，`max_bytes` / auto-compact 兜底 |
| Conversation JSONL | `$agent_home/sessions/YYYY/MM/DD/session-{key}-{ts}.jsonl` | 90 天窗口 | 事件流可 resume；含 compaction 事件；startup 清 90 天前文件 |
| Long-term memory | `$agent_home/memory/*` 文件树 | 长期，按 `max_unused_days` / `max_rollout_age_days` 淘汰 | phase-1 抽取 raw、phase-2 巩固进 `MEMORY.md`、摘要注入 developer message |

### 两阶段抽取 / 巩固

- **Phase-1（抽取）**：每 turn 结束后 `auto_extract_after_turn_with_pollution_check_blocking()` 触发。`extract.rs` 用 `extract_model` 对最新 user/assistant/tool 事件抽出原子事实，追加到 `raw_memories.md`。含 `pollution.rs` 门槛：外部大上下文 / 明显噪声 turn 会跳过。
- **Phase-2（巩固）**：`consolidation.rs` 用 `consolidation_model` 把 `raw_memories.md` 中的事实与 `MEMORY.md` 合并去重，并同步 `memory_summary.md`（供下 turn read-path 注入）。触发条件：raw 达到 `max_raw_memories` 或 rollout 结束。

### 召回路径

- Read-path：`recall_system_message()` → 读取 `memory_summary.md` → 按 token 预算截断 → 拼 `READ_PATH_TEMPLATE` → 作为 developer message 与 system prompt / history 一起送入模型。
- 按需读取：MCP 工具 `memory/list`（枚举）、`memory/read`（取正文）、`memory/search`（关键词检索，`search.rs`）。模型可在 tool loop 中主动调用。
- 无 embedding / 向量库。设计上假设文件树体量 <= 数千行，关键词 + 摘要注入足以覆盖 P0 场景。

### 显式写入 / 删除

- `/remember <text>`：直接写入 `MEMORY.md` 并同步 summary。
- `/forget <id|last>`：删除指定条目或最近一条。
- `MemoriesConfig` UI：编辑 `use_memories` / `generate_memories` / models / 上限 / disable_on_external_context 等。

## 技术细节

### 关键文件索引

| 文件 | 角色 |
| --- | --- |
| `crates/agent/src/memory/mod.rs` | 子模块入口、`ensure_memory_layout`、公共 API |
| `crates/agent/src/memory/layout.rs` | agent home / memory 目录初始化 |
| `crates/agent/src/memory/read_path.rs` | `recall_system_message()`、`READ_PATH_TEMPLATE` |
| `crates/agent/src/memory/extract.rs` | phase-1 抽取，消费 `extract_model` |
| `crates/agent/src/memory/consolidation.rs` | phase-2 巩固，消费 `consolidation_model` |
| `crates/agent/src/memory/write.rs` | `remember_explicit` / `replace_memory` / `forget_memory` |
| `crates/agent/src/memory/search.rs` | 关键词检索（供 MCP 工具） |
| `crates/agent/src/memory/mcp_tools.rs` | `memory/list` / `memory/read` / `memory/search` MCP 工具定义 |
| `crates/agent/src/memory/pollution.rs` | 外部大上下文识别，跳过抽取 |
| `crates/agent/src/memory/retention.rs` | `max_unused_days` / `max_rollout_age_days` 淘汰 |
| `crates/agent/src/memory/rollout_tracker.rs` | rollout 生命周期、`rollout_summaries/` |
| `crates/agent/src/memory/state_db.rs` | 进程间租约/心跳（不是内容存储） |
| `crates/agent/src/memory/telemetry.rs` / `usage_tracking.rs` | 指标 / 用量 |
| `crates/agent/src/persistence.rs` | JSONL 会话持久化 |
| `crates/agent/src/session/turn_loop.rs` | 消费 memory：注入 developer message、触发抽取、写 compaction 事件 |
| `crates/bifrost-admin/src/handlers/im_gateway.rs` | Feishu / API 入口、`/agent/sessions*` 路由、PATCH `memories` 透传 |
| `web/src/pages/Settings/tabs/agent/MemoriesSection.tsx` | UI 卡片 |

### 事件类型清单（persistence.rs:29-48）

`user_message`、`assistant_message`、`assistant_delta`、`tool_call`、`tool_result`、`compaction`、`session_start`、`session_end`、`mcp_tools_loaded`、`skills_loaded`、`title_updated`、`goal_updated` / `goal_cleared`、`plan_updated` / `plan_cleared`、`proposed_plan`、`run_state_changed`。

### 数据流

```text
Feishu WS / POST /agent/chat
  → im_gateway::process_agent_chat
  → AgentSession + optional ConversationRecorder
  → session::turn_loop::run_turn_with_mcp
      1. slash 命令拦截 (/clear /undo /compact /status /resume /remember /forget)
      2. pre-turn compact_session（含 record_compaction_event）
      3. recall_system_message() 注入 memory developer message
      4. build_messages(system + memory dev + session.history)
      5. model chat completion + tool loop (含 memory/*)
      6. recorder 写事件
      7. auto_extract_after_turn_with_pollution_check_blocking() phase-1
      8. 后台 consolidation.rs phase-2
  → persistence.rs 写 JSONL
  → Admin/WebUI 通过 /agent/sessions* 拉取
```

## CLI / Web / Admin API

### Admin API

- `GET /_bifrost/api/im-gateway/agent/config`：包含 `memories` 字段。
- `PATCH /_bifrost/api/im-gateway/agent/config`：透传 `memories` 到 `MemoriesConfig`（`im_gateway.rs:2253-2305` 附近）。
- `GET /_bifrost/api/im-gateway/agent/sessions` / `/sessions/all` / `/sessions/history` / `/sessions/history/{path}`：会话 JSONL 列表/详情。
- `POST /_bifrost/api/im-gateway/agent/chat`：内部测试入口，与 IM 入口共享 turn loop。

### CLI

- 内置 slash（在 chat 中）：`/remember`、`/forget`、`/compact`、`/status`、`/resume`、`/clear`、`/undo`。
- `bifrost-cli config memory` 是进程内存诊断（`crates/bifrost-cli/src/cli.rs:2161-2162`），**不是**长期记忆 CLI；本次不新增外部 CLI 子命令。

### Web

- Settings → Agent → Memories section：编辑 `MemoriesConfig` 全部字段，保存后 PATCH 透传。
- Settings → Agent → History section：查看/管理 JSONL 列表与 90 天清理策略。

## Sync 边界

- Memory 文件树 (`$agent_home/memory/`) 与 JSONL (`$agent_home/sessions/`) 都属**本机长期数据**，不参与 Bifrost 规则/设置的 sync。
- `MemoriesConfig` 走 Agent 配置 store；未来若接入 unified_config sync，需要单独评估隐私（记忆内容属于用户私域，不适合跨设备自动同步）。
- MCP 工具 `memory/*` 只在本机 Agent runtime 生效。

## Phase 1-4（历史里程碑）

### Phase 1：JSONL 会话持久化 + compaction 事件

- `persistence.rs` JSONL 事件流、`record_compaction()`、`scan_session_summary`、90 天清理。

### Phase 2：文件式长期记忆骨架

- `memory/layout.rs` + `mod.rs` + `MEMORY.md` / `memory_summary.md` / `raw_memories.md`。
- `read_path.rs` + `recall_system_message()` 注入 developer message。

### Phase 3：抽取 + 巩固闭环

- `extract.rs`（phase-1）、`consolidation.rs`（phase-2）、`pollution.rs` 门槛、`retention.rs` 淘汰、`rollout_tracker.rs`。
- MCP 工具 `memory/list|read|search`。
- Slash `/remember` `/forget`。

### Phase 4：Settings UI + 可观测（当前）

- `MemoriesSection.tsx` 配置卡片。
- `telemetry.rs` / `usage_tracking.rs` 指标。
- Admin PATCH 透传 `memories`。

### 后续（planned）

- 引入可选 embedding / 向量库或外部 memory 后端（默认关闭，保持零依赖）。
- 记忆内容的隐私管控（脱敏 / 标签）。
- 跨设备可选同步（含用户显式确认）。

## 测试方案

### 单元测试

| 位置 | 断言 |
| --- | --- |
| `crates/agent/src/memory/tests.rs` | layout / read_path / extract / consolidation / write / search / pollution / retention / mcp_tools 全 |
| `crates/agent/src/persistence.rs::record_compaction_event_round_trip` | compaction 事件写入 → 读回一致 |
| `crates/agent/src/session/tests.rs::test_record_compaction_event_includes_emergency_and_total_tokens` | pre-turn / mid-turn / 手动 compact 都记录 emergency + total tokens |
| `crates/agent/src/persistence.rs::enforce_max_bytes` | history JSONL 超阈值裁剪 |

### 集成 / E2E

| 位置 | 断言 |
| --- | --- |
| `crates/bifrost-e2e/src/tests/im_gateway_agent.rs` | Feishu → Agent → memory 抽取/召回全链路 |
| `human_tests/im-gateway-agent.md:152-163` | `/compact` 真实场景 |
| `human_tests/im-gateway-agent.md:863-895` | Agent Loop tool history resume 回归 |
| `human_tests/agent-session-persistence.md` | JSONL resume / 90 天清理 |

### 真实场景（human_tests）

- `TC-MEMORY-EXTRACT-01`：多轮对话后 `raw_memories.md` / `MEMORY.md` 出现新事实。
- `TC-MEMORY-RECALL-01`：新 session `recall_system_message()` 含上轮总结。
- `TC-MEMORY-FORGET-01`：`/forget last` 后下 turn 摘要不再包含该条。
- `TC-MEMORY-DISABLE-01`：`use_memories = false` 时不注入 developer message。

## Review / Fix / Test 闭环

- **第 1 轮**：核对本文与代码：`ensure_memory_layout`、`recall_system_message` 调用点、phase-1/phase-2 触发点、compaction 事件持久化；跑 `cargo test -p bifrost-agent memory::` 与 `persistence::`。
- **第 2 轮**：基于最新 diff 复查 human_tests 索引；跑 E2E `im_gateway_agent.rs`；review MemoriesSection UI 与 PATCH 透传字段是否遗漏。
- **第 3 轮（按需）**：若 planned 项（向量库 / 隐私 / 跨设备 sync）启动，追加独立设计文档与测试轮次。

## 校验要求

- 优先执行：
  - `cargo test -p bifrost-agent memory:: persistence:: session::`
  - `cargo test -p bifrost-admin im_gateway::`
  - `cargo test -p bifrost-e2e im_gateway_agent`
- 再执行 `rust-project-validate`：fmt / clippy / `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在最终范围需要完整本地 CI 时执行。

## 风险与决策

| 风险 | 决策 |
| --- | --- |
| 无向量库，长期记忆规模变大后 read-path 摘要注入不再够 | 保持 P0 只用摘要 + MCP 关键词检索；条目超阈值走 `retention` 淘汰；后续再评估向量库 |
| Phase-1 抽取可能被外部大上下文污染 | `pollution.rs` 前置门槛；`disable_on_external_context` 配置化 |
| Phase-2 巩固调用模型有成本 | `consolidation_model` 独立配置；触发条件受 `max_raw_memories` 与 rollout 结束限制 |
| JSONL 无限增长 | `HistoryConfig.max_bytes` 强制裁剪 + startup 90 天清理 |
| 长期记忆属用户私域 | 不做默认跨设备同步；未来若做需显式确认与脱敏 |
| `state_db.rs` 名字容易误解 | 明确它只做租约/心跳协调，不承担 memory 内容存储；文档反复澄清 |
| compaction 事件类型扩展导致老 JSONL 读取失败 | `scan_session_summary` 与 `load_conversation` 对未知事件类型 skip 并告警 |

## 文档更新要求

- 更新 `human_tests/im-gateway-agent.md`、`human_tests/agent-session-persistence.md`（如新增 memory TC）。
- 更新 `human_tests/readme.md` 索引。
- README / docs 侧继续保留 memory 章节链接到本文与 `long-term-memory.md`、`web_admin_resume_memory_spike.md`；避免设计事实分裂。
- 本次不新增外部 CLI 子命令；Web / Admin API 字段以现有 `memories` PATCH 为准。
