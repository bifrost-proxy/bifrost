# Web Admin Rules Dynamic Island

## 功能模块说明

Rules 页面顶部的 Dynamic Island 用于展示当前生效规则数量、规则来源分组、变量冲突提示，以及展开后的 Merged Rules 完整文本。

## 本次变更

展开 Merged Rules 后，在代码框右上角提供一键复制按钮。按钮复制当前合并规则文本的 `trim()` 结果，成功后显示短暂的成功反馈，失败时显示错误提示。

后续回归发现，仅凭 async Clipboard API 返回成功不足以证明文本已经进入可粘贴剪贴板；部分嵌入浏览器环境会出现 Toast 成功但实际粘贴失败。因此 Web 模式复制路径必须优先使用用户点击手势内的同步 `execCommand('copy')`，再退回 async Clipboard API。

2026-05-20 回归发现：用户已经切到 Group 或个人规则页面并启用规则时，Dynamic Island 仍可能显示 `0 active`。根因是 `/_bifrost/api/rules/active-summary` 在首次遇到未缓存的 Group 目录时会同步请求远端 group/peer 信息；远端不可用或超时时，前端 catch 后把 active rules 清空。该链路必须改成本地规则文件优先，远端 group cache 只作为显示增强，不能阻塞 active 规则预览。

2026-05-20 追加回归发现：Badge/Rules 深链可能生成 `/_bifrost/rules?group={group_name}&rule=...`，例如 `group=next-agent`。Web UI 会把该值作为 active group 传给 `/api/group-rules/{group}`，因此后端 Group Rules API 必须把 path segment 视为 group ref：既可能是远端 group id，也可能是本地 group name/目录名。不能把 group name 当成远端 id 直接请求，否则会在已登录但路径使用名字时返回 502。

## 实现逻辑

- 在 `web/src/pages/Rules/RulesDynamicIsland.tsx` 中复用现有 `copyToClipboard()` 工具，兼容桌面壳写入、Web 同步复制路径与 async Clipboard API fallback。
- `web/src/utils/clipboard.ts` 在 Web 模式下优先使用 `execCommand('copy')`，避免 async Clipboard API 在嵌入浏览器里假成功。
- Merged Rules 代码框改为相对定位容器，复制按钮绝对定位到右上角，不改变合并规则文本内容。
- 复制成功后按钮图标短暂切换为成功状态；当 merged content 更新时清理成功状态。
- 为 Dynamic Island 触发器、Merged Rules toggle、内容区域和复制按钮增加稳定 `data-testid`，支撑 UI E2E 与真实场景验证。
- `/_bifrost/api/rules/active-summary` 首屏只读取本地 `RulesStorage` 和 Group 子目录，不再等待远端 group cache 解析，也不在只读 summary 请求中删除所谓 orphaned group 目录。
- 对已缓存 Group，active summary 返回真实 `group_id` 和目录名/组名；对未缓存 Group，`group_id` 先回退为本地目录名，确保预览和 merged content 不会因为远端不可用变成空。
- 如果 sync manager 可用，未缓存 Group 的 ID/名称解析改为后台 best-effort 任务；解析成功后持久化 group cache 并刷新 badge 缓存。这样保留远端映射补全能力，但不阻塞 Web UI active 规则展示。
- `/api/group-rules/{group}` 的 `{group}` 采用 group ref 解析：先按 group id 查 cache，再按 group name 反查 id，最后回退本地 group 目录。list/get/enable/disable 必须支持 group name 深链；create/update/delete 在需要远端写入时继续使用解析出的真实 group id 或规则文件中的 remote id。
- 代理启动与热重载加载规则时同样以本地 Group 规则目录为准：`resolve_valid_group_dirs` 合并 group cache 与本地子目录，即使没有 active sync session，也不会把本地已启用的 Group 规则从代理处理链路剔除。

## 依赖项

- `antd` Button / Tooltip / message
- `@ant-design/icons` CopyOutlined / CheckOutlined
- `web/src/utils/clipboard.ts`

## 测试方案

### 单元测试

本次没有新增独立纯函数；复制逻辑依赖浏览器剪贴板与 UI 事件，覆盖重点放在 Playwright UI E2E。

### E2E 测试

- `web/tests/ui/admin-rules-values.spec.ts`
  - 创建一个启用规则。
  - 打开 Rules 页面并展开 Dynamic Island。
  - 展开 Merged Rules，点击右上角复制按钮。
  - 读取浏览器剪贴板，断言内容等于页面展示的 merged rules 文本。
- `crates/bifrost-e2e/src/tests/group_rules.rs`
  - `group_rules_active_summary_uses_uncached_local_group_dirs`：未缓存 group id 且没有 sync manager 时，active summary 仍返回本地启用的 Group 规则。
  - `group_rules_active_summary_survives_remote_cache_resolution_failure`：有 sync manager 但登录态/远端 group cache 解析失败时，active summary 立即返回本地 Group 规则、不会删除本地目录，并在失败后清理 retry 标记，避免临时网络抖动后永久不再重试。
  - `group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent`：连续启停同一 Group 规则时，active summary 与 Badge cache 必须在限定时间内收敛到相同状态。
  - `group_rules_deeplink_accepts_group_name`：`/api/group-rules/{group_name}` list/detail/enable 均可用，覆盖 Badge/Rules 深链传 group name 的路径。
  - `resolve_valid_group_dirs_uses_local_dirs_without_sync_session` / `resolve_valid_group_dirs_keeps_cached_and_uncached_local_dirs`：代理启动与 hot reload 使用本地 Group 目录，不依赖远端刷新或登录态。

### 真实场景测试

- 更新 `human_tests/webui-rules.md`
  - 新增 `TC-WRU-39`：Dynamic Island Merged Rules 一键复制。
  - 新增 `TC-WRU-41`：Group 规则在远端 group 信息不可用时仍显示 active rules。
  - 新增 `TC-WRU-42`：远端 cache 解析失败、登录态短暂失效和快速本地启停时，Rules active summary 与 Badge cache 在限定时间内收敛，不出现长期 `0 active` 或旧规则残留。
  - 新增 `TC-WRU-43`：`group` URL 参数为本地 group name 时，Rules 页面能打开 Group 规则列表和规则详情，不返回 502。
  - 按用例真实执行：创建/启用规则，展开 Dynamic Island，展开 Merged Rules，点击复制按钮，粘贴或读取剪贴板验证内容一致。
- 更新 `human_tests/readme.md` 中 WebUI Rules 用例数量与说明。

## 校验要求

- `pnpm --dir web test:unit`
- `pnpm --dir web test:ui -- admin-rules-values.spec.ts`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 文档更新要求

本次仅修改 WebUI 交互，不涉及 README、规则协议、Hook、CLI 参数或配置项更新。
