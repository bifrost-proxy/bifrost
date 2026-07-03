# Replay 路由进入首屏拉取

## 背景

Bifrost 管理端 Web 端是单页应用（SPA），全局维护一条 push 长连接（`PushCenter` / `useReplayPush`）用来推送 traffic、rules、replay saved requests、replay groups 等实时事件。理想情况下，只要 push 连接建立且订阅了 Replay 频道，前端 store 就会随广播收到 `replay_saved_requests_update`、`replay_groups_update` 全量快照。

历史实现里，Replay 页面首屏拉取只依赖“页面挂载时触发一次 push subscription”作为拉取入口。这在两种场景下都能工作：

- 冷启动直接刷新 `/_bifrost/replay`：push 连接第一次订阅 Replay 频道，服务端会主动 broadcast 一次快照，前端收到后填充列表。

但从 `/_bifrost/traffic` 等已经建立 push 连接的路由切换到 `/_bifrost/replay` 时：

- 全局 push 连接已经存在，切换到 Replay 只是新增订阅 topic。
- 服务端不一定 broadcast“该 topic 的当前快照”给这个已有 client，Replay 前端就得不到首屏数据。
- 表现为左侧 Replay 列表保持空白，直到用户手动刷新页面。

本次修复不改 push 协议，只在 Replay 页面挂载时显式发起一次 HTTP 拉取，让 push 是否 broadcast 都能拿到首屏数据。修复范围仅限“Replay 页首屏数据初始化”，不改动 Replay 保存、执行、历史记录逻辑，也不动 push 协议。

## 用户目标验证清单

### 必须实现

- 已建立全局 push 连接 + 切换到 `/_bifrost/replay` 时，无需刷新页面，左侧 Replay 列表立即出现数据。
- 冷启动直接进入 `/_bifrost/replay` 时行为不变，列表能正常出现。
- 首屏拉取 `loadGroups()` 与 `loadSavedRequests()` 只发生一次（组件挂载时），不与 push 快照产生重复覆盖。
- push 快照后续到达后仍能作为“最终一致”覆盖 store，避免 HTTP 拉取到旧数据。

### 必须不破坏

- Replay 保存、执行、历史查看、tabs、分组管理行为不变。
- push 订阅仍继续接收 Replay 相关频道的后续增量与快照。
- 未登录 / API 报错场景，Replay 页面能显示错误提示，不因初始化拉取失败而崩溃。
- Replay 页面二次进入（tab 切换后再切换回来）不重复触发大量 HTTP 请求。

### 必须真实验证

- Playwright 用例：先进入 `/_bifrost/traffic`，再切到 `/_bifrost/replay`，断言 saved request 立即渲染。
- 手工回归：验证冷启动直接进入 `/_bifrost/replay` 也可用。

## 产品语义

Replay 页面的首屏数据初始化遵循“push + 显式拉取”双通道模型：

1. **push 是长期一致性通道**：负责后续所有 saved requests / groups 的实时同步。
2. **显式 HTTP 拉取是首屏兜底通道**：确保任何路由进入方式下，页面挂载后 store 里至少有一次来自后端的完整状态。

两条通道结果通过 store 的 upsert / 快照覆盖策略自然合并，无先后依赖。这样即便 push 协议未来演进（增量而非快照），首屏依然可用。

## 技术细节

### 前端页面挂载

`web/src/pages/Replay/index.tsx`：

```tsx
import { useEffect } from "react";
// ...
const { loadGroups, loadSavedRequests } = useReplayStore();

useEffect(() => {
  Promise.all([
    loadGroups(),
    loadSavedRequests(),
  ]).catch((err) => {
    // 集中错误日志：不阻塞 push 后续快照
    console.warn("initial replay load failed", err);
  });
}, [loadGroups, loadSavedRequests]);
```

行数参考：`useEffect(() => { ... }, [loadGroups, loadSavedRequests])`（line 74-95）。`useReplayPush` 相关 hook 保持第二个 `useEffect`（line 97）。

### store 方法

`web/src/stores/useReplayStore.ts`：

- `loadGroups()`
  - 调用 `GET /_bifrost/api/replay/groups`。
  - 返回后 `set({ groups: response })`，允许被 push 快照进一步覆盖。
- `loadSavedRequests()`
  - 调用 `GET /_bifrost/api/replay/saved-requests`。
  - 返回后 `set({ savedRequests: response })`。
- 两个方法都是幂等的，重复调用只会得到一致结果。

### 与 push 的合并

- push handler 收到 `replay_groups_update` / `replay_saved_requests_update` 后同样 `set` 到 store。
- 若 HTTP 拉取先到，push 后到，push 覆盖为最终值；反之 HTTP 覆盖 push（差距应仅为几十 ms 的临时窗口）。
- 分组 upsert 逻辑（见 `replay-group-create-dedup.md`）保护本地创建路径不被 push 覆盖丢失。

### 整页 loading

- `showSpinner = loading` 覆盖 `loadGroups + loadSavedRequests` 期间；此时 composer 不可交互，避免用户在空列表上误操作。
- 与 `executing` 分离（见 `replay-executing-cancel-overlay.md`）。

## CLI / Web / Admin API

### Admin API

- `GET /_bifrost/api/replay/groups`
- `GET /_bifrost/api/replay/saved-requests`
- Push channel：`replay_groups_update`、`replay_saved_requests_update`

三者共享同一份 store 状态；本次修复不改 API 契约。

### Web UI

Replay 页面挂载时额外触发一次拉取；用户不可见新按钮 / 新交互。

### CLI

无。

## Sync 边界

- Replay saved requests / groups 是本地管理端状态，不参与规则 Sync，不跨设备同步。
- push 广播只在同一 Bifrost 实例的多个客户端之间生效。

## 实现切分

### Phase 1：Replay 首屏 useEffect

- 在 `index.tsx` 顶层新增或更新 `useEffect`，挂载时 `Promise.all([loadGroups(), loadSavedRequests()])`。

### Phase 2：store 健壮性

- 确认 `loadGroups` / `loadSavedRequests` 处理并发调用、错误时不清空既有 store。
- 报错走 antd `notification.error`，不 crash。

### Phase 3：与 push handler 幂等

- 复核 push handler 与 HTTP 拉取的合并顺序，确保 upsert 语义生效。
- 覆盖“push 快照包含更多字段”场景。

### Phase 4：回归

- 新增 Playwright 用例覆盖“先进 traffic 再进 replay”。
- 更新 human_tests Replay 相关章节，注明“路由切换后首屏立即出现”。

## 测试方案

### Playwright UI

- `web/tests/ui/admin-replay.spec.ts`
  - 新增或复用用例：
    1. 通过管理端 API 预创建一个 saved request 与一个 group。
    2. `page.goto("/_bifrost/traffic")`，等待页面稳定确保 push 已建立。
    3. 通过 side nav 或 `page.click("[data-testid=side-nav-replay]")` 切到 `/_bifrost/replay`。
    4. 断言无需 `page.reload()`，saved request 与 group 立即出现（等待 `page.getByText(name)` visible）。
  - 覆盖 fallback：模拟 push disconnect 情况下页面仍能填充。

### store 单测

- 若有 vitest / jest 覆盖 store：
  - `loadGroups_populates_groups_from_api`
  - `loadSavedRequests_populates_saved_from_api`
  - `useEffect_calls_load_functions_once_on_mount`

### human_tests

- `human_tests/webui-replay.md` Replay 首屏章节补一句：从 traffic 切换到 replay 后左侧列表立即出现。
- 无需更新用例数量。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核目标：切换路由后首屏立即可见 / 冷启动不变 / push 后续仍生效。
- 复核 diff：`Replay/index.tsx`、`useReplayStore.ts`、Playwright 用例。
- 重点 review：
  - 是否会在 tab 频繁切换时触发大量重复请求；React `useEffect` 依赖数组是否稳定。
  - 组件卸载时是否需要 abort in-flight fetch，避免 stale set。
- 复测：
  - `pnpm --filter web test:ui -- admin-replay`
  - 手工：traffic → replay 切换 5 次，观察 Network 面板请求次数与结果。

### 第 2 轮

- 复核修复。
- 覆盖 push 失联场景：断开 push，切换到 replay 仍能拿数据。
- 覆盖 API 报错场景：`loadSavedRequests` 500 时页面显示错误提示且不 crash。

## 风险与决策点

- **风险**：Replay 首屏拉取增加了每次进入页面的 HTTP 请求次数（`loadGroups` + `loadSavedRequests`），冷路径 tab 切换会多两次网络请求。缓解：两个 API 都很轻量；未来可加 stale-while-revalidate 缓存。
- **风险**：拉取失败但 push 已建立时，用户看到空列表但 push 稍后会补齐。可接受，因为拉取错误会记录 warning，且 push 会最终一致。
- **决策**：不改 push 协议要求“每次订阅触发 broadcast”；那需要跨端约定与更多测试，权衡下前端多一次拉取更简单。
- **决策**：不引入 SWR / React Query 只为这两个 API；当前 store 直接管理已经足够，保持依赖最小。
