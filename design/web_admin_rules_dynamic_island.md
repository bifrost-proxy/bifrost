# Web Admin Rules Dynamic Island

## 背景

Rules 页面顶部的 **Dynamic Island** 是「当前所有 enabled 规则的一键预览面板」。它把生效规则数量、分组来源、变量冲突提示、以及展开后的 Merged Rules 完整文本聚在一个组件里，用户不用逐条打开规则文件就能验证「我到底给代理喂了什么」。

历次迭代给 Dynamic Island 加了三批新语义：

1. Merged Rules 展开后需要提供**一键复制**（并解决嵌入式浏览器 Clipboard API 假成功的问题）。
2. active summary 面板不能被远端 group cache 的解析拖慢或清空——远端不可用不能导致 `0 active`。
3. Group 规则消费必须以「登录态」作为门禁——`.group_cache.json` 是本地导航索引，而不是登录 token；退出登录不能删该文件，但没有 active sync session 时 Group 规则不进入代理 resolver / active-summary / Badge。

本文档在此把这三批语义与实现路径完整化，并给出实测覆盖的 E2E 用例清单。

## 用户目标验证清单

### 必须实现

- Dynamic Island 展开面板显示：active rules 数量、分组来源、变量冲突提示（若开启对应设计）。
- Merged Rules 展开后代码框右上角提供复制按钮，按钮点击后：
  - 复制内容 = `mergedContent.trim()`。
  - 成功时按钮图标短暂切换为 CheckOutlined；失败时显示 message error。
  - 在 Web 模式下优先走同步 `execCommand('copy')`（用户点击手势内），失败再回落 async Clipboard API。
- `/api/rules/active-summary` 首屏只依赖本地 `RulesStorage` + Group 子目录，不等待远端 group cache 解析，不删除 orphan group 目录。
- 未缓存 Group 的 `group_id` 回退为本地目录名，Merged Rules 与预览不因为远端不可用变成空。
- 有 sync manager 时，未缓存 Group 通过后台 best-effort 任务补 id/name，成功后持久化 group cache 并刷新 badge cache。
- `/api/group-rules/{group}` 的 `{group}` 支持三种解析：
  1. 按 group id 查 cache；
  2. 按 group name 反查 id；
  3. 回退本地 group 目录名。
- `/api/sync/logout` 只清登录态，不删 `.group_cache.json`。
- 有登录态时启停 Group 规则先补齐 group id/name cache，再触发热更新与 Badge cache 刷新。
- 代理启动与热重载：`resolve_valid_group_dirs` 有 active sync session 时合并 group cache + 本地子目录；无 session 时返回空集合。

### 必须不破坏

- Dynamic Island 触发器、Merged Rules toggle、内容区域、复制按钮都有稳定 `data-testid`，供 UI E2E 使用。
- 复制按钮不改变 merged rules 文本，不做二次序列化或格式化。
- 非 Group 规则（个人 My Rules）的启停、编辑、置顶、拖拽不受影响。
- Rules 深链 `?group=<group_name>` 从 Badge / 消息卡片跳转仍可打开对应 Group 规则列表。
- 未登录时 Web UI 仍能查看/恢复本地 Group 规则文件（只是不进入代理）。

### 必须真实验证

- Playwright：`admin-rules-values.spec.ts` 覆盖 Dynamic Island 展开 → 复制 → 剪贴板比对。
- Rust E2E：`crates/bifrost-e2e/src/tests/group_rules.rs` 覆盖 active summary、深链、快速启停、退出重登、resolver 门禁。
- 手工：human_tests 补 `TC-WRU-39/41/42/43/44`。

## 产品语义

### Merged Rules 复制的两种路径

Web 模式必须优先同步 `execCommand('copy')`，因为部分嵌入式浏览器（客户端 WebView、企业内浏览器扩展环境）在调用 async Clipboard API 时会 resolve `true`，但**实际没写入系统剪贴板**。表现为：Toast 提示成功，用户粘贴时却拿到旧内容。

`web/src/utils/clipboard.ts::copyToClipboard(text)` 的调用序：

1. 检测是否在桌面壳内，若在则走桌面壳提供的桥接写入。
2. Web 模式：优先尝试用户点击事件内的同步 `document.execCommand('copy')`。
3. 若 2 失败或不支持，再退回 `navigator.clipboard.writeText(text)`。

Dynamic Island 组件消费返回值：`const ok = await copyToClipboard(text);` 后决定 UI 反馈。

### active summary 的读写路径

`/api/rules/active-summary` 是只读的合并预览，绝不能在此路径内做写操作（例如删除 orphaned group 目录）——否则「查看」会误改磁盘。因此实现约束为：

- 首屏用本地 `RulesStorage::list_summaries()` + Group 子目录扫描直接返回。
- 每个 Group 目录尽量提供 `group_id`：
  - 命中 `.group_cache.json` 时返回真实 id；
  - 未命中时以本地目录名兜底。
- 只有当 sync manager 有 active session 时，才在后台异步尝试解析未缓存 Group 的 id/name。
- 后台解析成功 → 持久化 `.group_cache.json` → 刷新 Badge cache → 下一次读 active summary 已经带真实 id。
- 后台解析失败或超时 → 短时间不重试（避免 hot loop），下次 UI 刷新继续本地兜底。

### Group ref 解析

`/api/group-rules/{group}` 接受三种输入：

1. 远端 group id（例如 UUID 或 numeric id）；
2. 本地 group name（历史 Badge 深链会带这个）；
3. 本地目录名（等价于 name 的规范化形式）。

命中顺序：先查 cache 中的 id；查不到就按 name 反查 id；再查不到就回退到本地目录。所以未登录 / 未缓存时，深链 `?group=next-agent` 也能显示本地 Group 规则列表，不会 502。

### 登录态门禁

Group 规则的**消费方**是代理 resolver 和 active-summary/Badge。门禁语义：

- 有 active sync session → 允许把 Group 规则加入 proxy resolver / active-summary（badge/injection 中的 active rules 列表）。
- 无 active sync session → resolver 不加载任何 Group 目录；active-summary 仍以本地目录兜底展示（供用户查看/恢复），但代理运行时不消费。

Group cache 与登录态解耦：cache 只是「目录名 ↔ 远端 id」的反向索引，退出登录不删，重新登录直接复用，避免用户手动重建 Badge 深链的挫败。

## 技术细节

### 前端

- `web/src/pages/Rules/RulesDynamicIsland.tsx`
  - 复用 `copyToClipboard(text)`。
  - Merged Rules 代码框改为相对定位容器，复制按钮绝对定位到右上角。
  - `mergedCopied` state：成功后短时 CheckOutlined；`useEffect` 在 merged content 变化时清理成功状态。
  - `data-testid`：`rules-dynamic-island-trigger`、`rules-dynamic-island-panel`、`rules-dynamic-island-merged-toggle`、`rules-dynamic-island-merged-panel`、`rules-dynamic-island-copy-merged`、`rules-dynamic-island-merged-content`、`rules-dynamic-island-empty`、`rules-dynamic-island-rule-row`。
- `web/src/utils/clipboard.ts`：Web 模式下优先 `execCommand('copy')`，再回落 async Clipboard API；单测 `web/src/utils/clipboard.test.ts` 覆盖两条分支与失败返回 `false`。

### 后端

- `/api/rules/active-summary`
  - 只用 `RulesStorage` 与本地子目录读取；未缓存 Group 用目录名兜底为 `group_id`。
  - 只读语义：不删除本地目录，不写 group cache。
- `/api/group-rules/{group}`（含 list / detail / enable / disable / create / update / delete）
  - `{group}` 视为 group ref，按 id → name → 本地目录名 三步解析。
  - create / update / delete 中需要远端写入时，最终使用解析出的真实 group id 或规则文件里的 `remote_id`。
- `/api/sync/logout`
  - 只清登录态与 in-memory session，不删 `.group_cache.json`。
- `crates/bifrost-cli/src/commands/start.rs::resolve_valid_group_dirs`
  - 有 active sync session：合并 `.group_cache.json` 中的目录 + 本地子目录，返回给 resolver。
  - 无 active sync session：`WARN "no active sync session, skipping group rule directories for proxy runtime"` 并返回空集合。

## Sync 边界

- Group 规则本身通过 sync backend 与远端同步；本文档只覆盖「消费门禁 + 本地导航索引」。
- `.group_cache.json` 不进入 sync payload；它是本地目录 ↔ 远端 id 的映射快照。
- 退出登录后重新登录会按需重建 cache，而不是完全信任旧 cache。

## Phase 1 – 复制按钮

- 引入 Dynamic Island Merged Rules 展开与复制按钮。
- 修复 clipboard 工具的 execCommand 优先策略。
- 增加 `data-testid` 与 Playwright 覆盖。

## Phase 2 – active summary 本地优先

- `/api/rules/active-summary` 去掉远端 group cache 同步依赖。
- 未缓存 Group 用目录名兜底 `group_id`。
- 后台 best-effort 补齐 cache（仅在有 sync manager 时）。

## Phase 3 – Group ref 解析

- `/api/group-rules/{group}` 支持 id / name / 目录名 三种解析。
- Badge 深链 `?group=<group_name>` 打得开对应 Group 规则详情。

## Phase 4 – 登录态门禁与 cache 保留

- `/api/sync/logout` 不删 `.group_cache.json`。
- Group 规则启停在有登录态时补齐 cache 再刷新。
- `resolve_valid_group_dirs` 以 active sync session 作为门禁。

## 测试方案

### 单元测试

- `web/src/utils/clipboard.test.ts::copyToClipboard`
  - `resolves(true)` on execCommand success。
  - `resolves(false)` when both paths fail。
  - `resolves(true)` on async Clipboard fallback。
- `crates/bifrost-cli/src/commands/start.rs`
  - `resolve_valid_group_dirs_skips_local_dirs_without_sync_session`
  - `resolve_valid_group_dirs_skips_cached_and_uncached_local_dirs_without_sync_session`

### E2E 测试

- `web/tests/ui/admin-rules-values.spec.ts`
  - 展开 Dynamic Island → 展开 Merged Rules → 点击复制 → 读取剪贴板比对。
  - 使用 `.group_cache.json` fixture 走 Group 分支。
- `crates/bifrost-e2e/src/tests/group_rules.rs`
  - `group_rules_active_summary_includes_group_rules`（基线）
  - `group_rules_active_summary_after_toggle`
  - `group_rules_active_summary_uses_local_group_fallback_without_login`
  - `group_rules_active_summary_survives_remote_cache_resolution_failure`
  - `group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent`
  - `group_rules_deeplink_accepts_group_name`
  - `group_rules_logout_keeps_group_id_navigation_cache`
- CI 约束：`E2E Runner` 必须启动仓库内置 `packages/bifrost-sync-server`，通过 `BIFROST_E2E_SYNC_BASE_URL` / `BIFROST_E2E_SYNC_TOKEN` 注入 Rust E2E，禁止回落到默认生产远端地址。
- 备注：`group_rules_active_summary_skips_group_rules_without_login`（未登录时 active summary 也跳过本地 Group 规则）planned, not yet shipped as of 2026-06-17——当前实现中 active summary 仍通过本地目录名兜底返回未登录态下的本地 Group 规则，登录态门禁只作用在代理 resolver 层。

### 真实场景 human_tests

- `human_tests/webui-rules.md`
  - `TC-WRU-39`：Dynamic Island Merged Rules 一键复制。
  - `TC-WRU-41`：Group 规则在远端 group 信息不可用时仍显示 active rules。
  - `TC-WRU-42`：远端 cache 解析失败 / 登录态短暂失效 / 快速本地启停时收敛。
  - `TC-WRU-43`：`?group=<group_name>` 深链可用，不返回 502。
  - `TC-WRU-44`：退出登录再重新登录后，Group 规则启用状态与 Dynamic Island / Badge 跳转仍使用真实 group id。
- 更新 `human_tests/readme.md` 中 WebUI Rules 用例数量。

启动约束：临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核复制路径的两条分支覆盖率。
- 复核 active summary 是否引用了任何写操作。
- 复核 Group ref 解析在无登录/无 cache 时是否走本地目录 fallback。
- 复核 logout 后 `.group_cache.json` 仍存在。

### 第 2 轮

- 复测：`group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent` 在多轮启停下是否稳定。
- 复测：退出重登后 Badge 深链 URL 使用真实 group id。
- 复测：代理侧 `resolve_valid_group_dirs` 在多种 sync 状态迁移中收敛。

## 校验要求

- `pnpm --dir web test:unit`
- `pnpm --dir web test:ui -- admin-rules-values.spec.ts`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 风险与决策

- **Clipboard API 假成功**：即使标准返回 true，仍以同步 execCommand 为首选，不再单独 whitelist 浏览器。
- **`.group_cache.json` 的清理时机**：只在用户主动 reset 或 rebuild 时清理；logout 不清理。
- **未登录时 active summary 是否隐藏 Group 规则**：当前保留本地兜底以便用户查看/恢复；如未来产品决定统一门禁到 active summary 层，再实现 `group_rules_active_summary_skips_group_rules_without_login`。
- **Badge cache 与 active summary 一致性**：以 active summary 为主写，Badge cache 是缓存副本；两者在启停后短窗口内可能不同步，靠 rapid_toggle E2E 保证收敛。
- **深链兼容**：历史 Badge 生成的 `?group=<name>` URL 永久保留兼容路径；新链接优先使用 group id。
