# Agent Token Usage 统计口径

## 背景

Bifrost 内置 Agent 面向多路入口（Web Agent Chat、IM Gateway、Admin API 直调）产出 token 统计。真实运行中出现两类问题：

1. **累计 vs. 快照混用**：turn loop 一次响应返回的 `usage.total_tokens` 同时被写入 `session.total_tokens_used`（累计）和 `session.last_response_tokens`（当前 context 快照）。恢复后 `effective_token_count()` 使用 `last_response_tokens`，会把“会话总消耗 90000”当成“当前 context 90000”，触发假阳性自动压缩，同时让 Web/IM 状态面板显示不合理的 200%+ context 使用率。
2. **HUD 缺失**：Web AI Chat 输入框上方没有实时 token/context HUD。用户只能打开 `/status` 弹窗查看，不利于快速判断是否接近压缩阈值。

同时 Chat Completions（`usage.prompt_tokens` / `usage.total_tokens`）与 Responses API（`usage.input_tokens` / `usage.output_tokens` / `usage.total_tokens`）字段命名不同，需要统一 accessor。JSONL 持久化也需要同时记录 `tokens`（累计增量）与 `context_tokens`（快照），并对老事件 fallback。

本设计只调整内部口径与 UI 呈现，不改变外部字段名与 API shape。

## 用户目标验证清单

### 必须实现

- `TokenUsage::context_tokens()`：优先返回 `prompt_tokens`（Chat Completions）或 `input_tokens`（Responses），缺失时回退 `total_tokens`。
- Chat Completions parser 读 `usage.prompt_tokens`；Responses API parser 从 `usage.input_tokens` / `usage.prompt_tokens` 归一到同一 `TokenUsage.prompt_tokens` 字段。
- turn loop 收到响应后调用 `session.track_token_usage(usage.context_tokens(), usage.total_tokens)`：
  - `session.last_response_tokens = context_tokens`
  - `session.total_tokens_used += total_tokens`
- assistant_message JSONL 写入同时包含 `tokens`（增量 total）与 `context_tokens`（快照）。
- `load_session_runtime_state()` 恢复 `last_response_tokens` 时优先 `context_tokens`，回退 `tokens`。
- `scan_session_summary()` 只累计 `tokens`。
- `effective_token_count()` 基于 `last_response_tokens` + 后续追加消息估算，不再使用累计 token。
- Web AI Chat 输入框上方渲染 `AgentChatTokenHud`：Tokens（累计）与 Context（百分比），进度条按 percentage 宽度。
- HUD 使用 Ant Design token 变量（`colorBgElevated`/`colorBorderSecondary`/`colorTextTertiary`），无硬编码色值，明暗主题皆低调不喧宾夺主。
- HUD 数据来源：`RunTelemetry.status.total_tokens_used`（或回退 `context.totalTokensUsed`）与 `context_usage_percent`（或回退 `estimated_context_tokens / context_window_tokens`，默认 250_000）。
- Compression 项不在 HUD 内独立呈现；compaction count/phase 仍由 `/status` 弹窗承载（前端 E2E 显式 `not.toContainText("Compression")`）。

### 必须不破坏

- `TokenUsage` 结构字段命名 `prompt_tokens/completion_tokens/total_tokens` 不变。
- 老 JSONL 中只有 `tokens` 字段的事件仍能恢复。
- `total_tokens_used` 仍按每次响应 `total_tokens` 累加。
- compaction 的 `post_tokens` 仍优先作为压缩后的 context 快照。
- Goal token accounting 基于累计 `total_tokens_used`，不变。
- CLI/API 输出字段名与顺序不变；HUD 只消费已有 telemetry，不改协议。
- AgentChatSection 现有输入、Plan、Queue、Slash runner、Status 弹窗、消息滚动行为不受影响。
- 只有 Web `AgentChatSection` composer 上方新增 HUD；IM/其他入口 UI 不变。

### 必须真实验证

- 单元测试：`TokenUsage::context_tokens()` 分别覆盖 prompt/input/回退 total 三条路径。
- 单元测试：`track_token_usage(context,total)` 分别写入不同字段。
- 单元测试：JSONL `context_tokens` 恢复与旧事件兼容。
- E2E：mock provider 返回 `prompt_tokens != total_tokens`，`/status` context 用 prompt，session summary 用 total。
- WebUI E2E：Playwright 断言 `agent-chat-token-hud` 展示 `Tokens 1.2K` + `Context 45%`，`not.toContainText("Compression")`，HUD 不遮挡 composer。
- human_tests：`agent-token-usage.md` 覆盖设计、代码、单元命令与 E2E，含亮暗主题截图。

## 产品语义

### 两类 token 的语义边界

| 名称 | 语义 | 使用位置 |
| --- | --- | --- |
| `total_tokens_used` | 会话生命周期累计 API 消耗 | 成本、Goal accounting、Sessions 列表 Total 列 |
| `last_response_tokens` | 最近一次模型响应的 context 快照 | `effective_token_count()`、`/status` Context、IM 卡片 Context、HUD Context% |
| `context_window_tokens` | 模型上下文窗口大小（默认 250_000） | HUD Context% 分母、hover title 中的窗口大小说明 |

### HUD 定位

- Web AI Chat 用户在打字过程中就应能看到当前会话 token 与 context 占用。
- HUD 是次要视觉信息，不能与 composer 争夺注意力；不做动画、不做颜色告警（告警仍在 Status 弹窗内）。
- 数据完全来自已有 SSE `RunTelemetry` 事件，不发起额外请求，不影响 SSE 帧频。

## 技术细节

### 1. `TokenUsage::context_tokens()`

`crates/agent/src/types.rs`：

```rust
impl TokenUsage {
    pub fn context_tokens(&self) -> u64 {
        self.prompt_tokens.unwrap_or(self.total_tokens.unwrap_or(0))
    }
}
```

- Chat Completions parser 已经把 `usage.prompt_tokens` 写入 `self.prompt_tokens`，无需改动。
- Responses parser（`crates/agent/src/responses.rs`）把 `usage.input_tokens` 归一到同一 `prompt_tokens` 字段：`prompt_tokens = input_tokens.or(prompt_tokens)`。

### 2. `AgentSession::track_token_usage(context, total)`

`crates/agent/src/session.rs`：

```rust
pub(crate) fn track_token_usage(&mut self, context_tokens: u64, total_tokens: u64) {
    self.last_response_tokens = Some(context_tokens);
    self.last_response_history_len = Some(self.history.len());
    self.total_tokens_used = self.total_tokens_used.saturating_add(total_tokens);
}
```

turn loop 调用：

```rust
let context = usage.context_tokens();
let total = usage.total_tokens.unwrap_or(0);
session.track_token_usage(context, total);
```

### 3. JSONL 持久化

`crates/agent/src/persistence.rs::record_assistant_message()`：

```rust
serde_json::json!({
    "message": text,
    "tokens": total_tokens,
    "context_tokens": context_tokens,
})
```

`load_session_runtime_state()` 恢复：

```rust
let last_response_tokens = event
    .content
    .get("context_tokens")
    .and_then(|v| v.as_u64())
    .or_else(|| event.content.get("tokens").and_then(|v| v.as_u64()));
```

`scan_session_summary()` 累计 `tokens`（历史增量之和），不重复计入 `context_tokens`。

### 4. Web AI Chat Token HUD

新增 `web/src/pages/AI/AgentChatSection.tokenHud.tsx`：

```tsx
export function AgentChatTokenHud({ status, context }: Props) {
  const totalTokens = status?.total_tokens_used ?? context?.totalTokensUsed;
  const contextPct = resolveContextPct(status, context);
  if (totalTokens == null && contextPct == null) return null;
  return (
    <div data-testid="agent-chat-token-hud" className="agent-chat-token-hud">
      <span>Tokens {formatTokens(totalTokens)}</span>
      <span>Context {formatPct(contextPct)}</span>
      <div className="agent-chat-token-hud-progress" style={{ width: `${contextPct ?? 0}%` }} />
    </div>
  );
}
```

- 集成点：`AgentChatSection.tsx` composer 容器上方。
- 样式：`AgentChatSection.styles.ts` 使用 Ant Design token（`colorBgElevated`, `colorBorderSecondary`, `colorTextTertiary`）。
- HUD 内不展示 Compression 项。
- 计算 fallback：
  ```ts
  contextPct = status?.context_usage_percent
    ?? context?.contextUsagePercent
    ?? (context?.estimatedContextTokens && windowTokens
        ? (context.estimatedContextTokens / (context.contextWindowTokens ?? 250_000)) * 100
        : undefined);
  ```

### 5. 数据流

```
Model Response
  └─ TokenUsage { prompt_tokens, completion_tokens, total_tokens }
       ├─ context = usage.context_tokens() // prompt or fallback total
       └─ total = usage.total_tokens.unwrap_or(0)
              └─ session.track_token_usage(context, total)
                    ├─ session.last_response_tokens = Some(context)
                    └─ session.total_tokens_used += total
                          └─ persist assistant_message { tokens: total, context_tokens: context }
                                └─ RunTelemetry.status broadcast
                                      └─ Web HUD / IM card / /status
```

## CLI / Admin API / Web

### CLI

- `bifrost agent status --session <key>` 输出：
  - `context_tokens: <last_response_tokens>`
  - `total_tokens_used: <cumulative>`
  - `context_window_tokens: <window>`

### Admin API

- `GET /_bifrost/api/im-gateway/agent/sessions/<key>` 返回 status 字段包含 `total_tokens_used`、`last_response_tokens`、`context_usage_percent`。字段名不变。
- SSE 事件 `status` payload 同样携带上述字段，供 HUD 消费。

### Web

- AI Chat composer 上方渲染 HUD。
- Status 弹窗保留原有 Compression、Compaction Count 等详细信息。

## Sync 边界

- Token 统计属于本机 session 数据，不跨设备 sync。
- HUD 只是本机 UI 展示，无 sync 影响。

## 实现切分

### Phase 1：TokenUsage accessor 与 turn loop

- `TokenUsage::context_tokens()`。
- Responses parser 归一 input_tokens 到 prompt_tokens。
- `AgentSession::track_token_usage()` 与 turn loop 调用点。
- 单元测试。

### Phase 2：JSONL 持久化与恢复

- `record_assistant_message` 写 `context_tokens`。
- `load_session_runtime_state` 恢复 `last_response_tokens`。
- 老事件兼容 fallback。
- 单元测试。

### Phase 3：`effective_token_count` 与 compaction

- 确认 `effective_token_count()` 基于 `last_response_tokens`。
- 确认 compaction post_tokens 覆盖 `last_response_tokens` 语义不变。
- 单元测试。

### Phase 4：Web HUD

- 新增 `AgentChatSection.tokenHud.tsx`。
- 集成到 `AgentChatSection.tsx` composer 上方。
- 样式与主题变量。
- Playwright 用例。

### Phase 5：human_tests 与文档

- 新增 `human_tests/agent-token-usage.md`。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `token_usage_context_prefers_prompt_tokens`：`prompt=90,total=100` → `context_tokens()==90`。
- `token_usage_context_falls_back_to_total_tokens`：无 prompt/input，total=100 → `context_tokens()==100`。
- `responses_parser_maps_input_tokens_to_prompt_tokens`：Responses API `input_tokens=80` → `TokenUsage.prompt_tokens=Some(80)`。
- `test_track_token_usage`：`context=35,total=42` → `last_response_tokens=Some(35)`、`total_tokens_used=42`。
- `test_record_assistant_message_with_tokens_updates_runtime_summary`：JSONL 记录 `tokens=42,context_tokens=35` 后累计 42、快照 35。
- `test_load_session_runtime_state_keeps_context_snapshot_separate_from_cumulative_tokens`：多条事件恢复后累计=180 与最近 context=150 不混。
- `test_load_session_runtime_state_falls_back_to_total_tokens_for_old_events`：老事件只有 `tokens=1200`，`last_response_tokens=Some(1200)`。
- `test_effective_token_count_uses_last_response_snapshot`：追加 3 条 user message 后估算 = 快照 + estimate。

### E2E 测试

- 复用/更新 bifrost-e2e mock model API 脚本：mock 返回 `prompt_tokens=8000,total_tokens=8500`。
  - `/status` 返回 `last_response_tokens=8000` 且 `total_tokens_used=8500`（第一次响应）。
  - 第二次响应 `prompt=8200,total=8600` 后 `total_tokens_used=17100`。
- Responses API 路径同上，验证 `input_tokens` 归一。
- `web/tests/ui/agent-chat.spec.ts`：mock AI Chat SSE 注入 status/context 事件，断言：
  - `data-testid="agent-chat-token-hud"` 可见。
  - `toContainText("Tokens 1.2K")`、`toContainText("Context 45%")`。
  - `not.toContainText("Compression")`。
  - 进度条 `agent-chat-token-hud-progress` 宽度按 percentage。
  - HUD 不遮挡 composer（截图交付）。
- Playwright 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，注入真实 SSE mock，覆盖亮暗主题。

### 真实场景测试（human_tests）

`human_tests/agent-token-usage.md`：

- TC-ATU-01：静态复核 `TokenUsage::context_tokens()` 与 turn loop 调用点。
- TC-ATU-02：执行相关单元命令，记录实际输出。
- TC-ATU-03：真实启动 bifrost + mock model，验证 `/status` 与 session summary token 口径。
- TC-ATU-04：Web AI Chat 打开 mock 环境，HUD 亮/暗主题截图与文案。
- TC-ATU-05：Compaction 场景下 HUD Context 按 post_tokens 更新。

真实服务启动必须使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent --all-features token_usage context_snapshot runtime_state`
- `cargo test -p bifrost-agent --all-features plan_update_empty`
- `cd web && pnpm exec playwright test tests/ui/agent-chat.spec.ts -g "token HUD"`
- `cd web && pnpm build`
- 对应 human_tests 逐条执行
- 相关 E2E/API 脚本
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 生效时不跑 `make coverage`；交付时说明。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `TokenUsage` 字段解析、turn loop 写入、持久化读取、summary 扫描是否严格区分累计与快照。
- 复核 HUD 数据源顺序：`status` 优先，`context` fallback。
- 复核 HUD 是否遮挡 composer、亮暗主题是否统一。
- 运行 token usage 相关单元 + WebUI Agent Chat targeted test，修复混用或 UI 遮挡。

### 第 2 轮

- 复查 Responses API 与 Chat Completions 双路径。
- 复查旧 JSONL 兼容 fallback。
- 复查 Playwright 断言的 `not.toContainText("Compression")`。
- 再跑 targeted 单元、前端 Agent Chat test、human_tests、相关 E2E。
- 若第 2 轮仍发现统计口径不一致，继续追加轮次。

## 风险与决策点

- **Provider 只返回 `total_tokens`**：`context_tokens()` 回退 total，仍会略估高，但不会污染累计口径；比“把累计当 context”更接近真实。
- **Responses `input_tokens` 语义**：不同 provider 对 input_tokens 的定义略有差异（含/不含 tool call 结果）。第一版仍以 provider usage 为准，后续可 provider-specific 归一。
- **HUD 数据缺失**：`total_tokens_used` 与 `contextPct` 都缺失时 HUD 不渲染，避免误导用户；只缺一项时显示 `-`。
- **Compaction 与 HUD**：HUD 不展示 Compression 是有意选择——避免打扰用户；`/status` 弹窗仍有详细信息。
- **多主题**：使用 Ant Design token 变量而非硬编码色值；暗色主题下 HUD 保持低对比。
- **兼容 sync**：Token 数据不跨设备 sync，未来若做多设备统一，需要单独设计。
