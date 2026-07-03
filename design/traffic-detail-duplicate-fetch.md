# Traffic 请求详情重复拉取修复

## 背景

Bifrost Web Admin 的 Traffic 页在选中一条流量记录后需要展示详情（headers、
body、SSE 事件、WebSocket frames、matched rules、脚本结果等）。历史实现
中存在两个独立的拉取触发源：

1. `selectedId` 变化的 `useEffect` 自动拉取。
2. Traffic 列表行的单击 / 双击 handler 中调用 `fetchTrafficDetail(id)`。

两者在同一次“用户点击某行”的交互中都被执行，导致：

- `GET /_bifrost/api/traffic/{id}` 触发两次；
- `GET /_bifrost/api/traffic/{id}/body/request` 触发两次；
- `GET /_bifrost/api/traffic/{id}/body/response` 触发两次；
- 若有 raw body，`?raw=true&encoding=base64` 变体也重复。

体感是打开详情时后端读盘量翻倍，前端 network 面板出现成对相同请求。
对大 body（响应几十 MB）尤其明显。

本文档描述已经落地的修复方案：**详情拉取的唯一入口是 `selectedId` 变化
的 useEffect + ref-guard**，行 handler 只负责 `setSelectedId`。

## 用户目标验证清单

### 必须实现

- 同一次选中操作只触发一次 `fetchTrafficDetail(id)`。
- `fetchTrafficDetail` 内部触发的 request body / response body / raw body
  请求各自只发一次。
- 单击选中、双击展开详情（并展开详情面板）交互不变。
- 已经加载过的 `currentRecord.id === selectedId` 场景不再重复拉取。
- 双开详情面板 / detach 到新窗口场景下，只有前景 store 触发拉取。

### 必须不破坏

- push 事件 (`TrafficUpdated`) 更新 `recordsMap` / `currentRecord` 时
  合并逻辑保持，不算作用户主动选择，不触发详情重拉。
- 主动 refresh 详情（未来若引入按钮）仍能显式调用 `fetchTrafficDetail`。
- 详情 loading state 语义正确：开始拉取时 `detailLoading = true`，
  成功后 `false`，失败时 `false + detailError`。
- SSE 活跃请求的 response body 拉取仍走“open SSE 跳过 response body”
  分支（`isOpenSse`）。

### 必须真实验证

- 打开一次详情：Network 面板中 `/traffic/{id}` 与 `/body/request`、
  `/body/response` 各只出现一次。
- 快速在两行之间来回点击：每次切换只发一组请求，不叠加。
- 已经加载过详情的行再次点击（`selectedId === currentRecord.id`）不发
  网络请求。
- 双击展开详情面板：请求数量与单击相同。

## 产品语义

### 详情拉取的单一入口

`fetchTrafficDetail` 的**唯一调用者**是 `Traffic/index.tsx` 中的
`selectedId` useEffect。任何行为 handler（单击、双击、右键、快捷键、
detach 打开、URL 直达）只允许调用 `setSelectedId(id)`；后续详情拉取由
useEffect 统一驱动。

这个约束把“同一次逻辑选择触发详情拉取”的责任集中到一个地方，方便加
guard 与 memo。

### `lastAutoFetchSelectedIdRef` guard

useEffect 依赖 `[currentRecord?.id, fetchTrafficDetail, selectedId]`。为了
让 `currentRecord?.id` 变化不重复触发 `fetchTrafficDetail`，用一个
`useRef` 记录“上次已经为哪个 selectedId 触发过 fetch”：

- `selectedId === null` → 清 ref，不 fetch。
- `lastAutoFetchSelectedIdRef.current === selectedId` → 已经处理过，跳过。
- `currentRecord?.id === selectedId` → 详情已经加载（可能来自 push /
  cache），更新 ref 但不 fetch。
- 否则更新 ref 并 `fetchTrafficDetail(selectedId)`。

这样可以确保：

- 同一 selectedId 只 fetch 一次；
- 详情 push 更新后 useEffect 因 `currentRecord?.id` 变化重跑时不会重复
  fetch；
- 用户切换到别的行时正常 fetch。

## 技术细节

### 关键代码入口

- `web/src/pages/Traffic/index.tsx`
  - `const lastAutoFetchSelectedIdRef = useRef<string | null>(null);`
    （line ~462）
  - useEffect 逻辑（line ~465-479）：
    ```tsx
    useEffect(() => {
      if (!selectedId) {
        lastAutoFetchSelectedIdRef.current = null;
        return;
      }
      if (lastAutoFetchSelectedIdRef.current === selectedId) {
        return;
      }
      if (currentRecord?.id === selectedId) {
        lastAutoFetchSelectedIdRef.current = selectedId;
        return;
      }
      lastAutoFetchSelectedIdRef.current = selectedId;
      fetchTrafficDetail(selectedId);
    }, [currentRecord?.id, fetchTrafficDetail, selectedId]);
    ```
  - `handleSelect(record)` (line ~511)：只 `setSelectedId(record.id)`。
  - `handleDoubleClick(record)` (line ~541)：只
    `setSelectedId(record.id)`；若详情面板折叠则展开。**不再**调用
    `fetchTrafficDetail`。
  - `handleOpenDetailInNewWindow(record)` (line ~551)：同样只
    `setSelectedId` + 打开 popup。
- `web/src/stores/useTrafficStore.ts`
  - `fetchTrafficDetail(id)` (line ~1860)：
    1. `preserveBodies = currentRecord?.id === id`，避免同 id 拉取时先
       清空已有 body。
    2. `set({ detailLoading: true, detailError: null, ... })`。
    3. `api.getTrafficDetail(id)` → merge summary → `set({ currentRecord })`。
    4. 触发 `api.getRequestBody(id)` / `api.getResponseBody(id)`（若非
       open SSE）/ raw body 变体，各自并发但每种只一次。
    5. 失败时 `set({ currentRecord: null, detailError: ... })`。

### handler 与 useEffect 的边界

| 触发                    | 行为                                    |
| ----------------------- | --------------------------------------- |
| 单击行                  | `setSelectedId(id)`                     |
| 双击行                  | `setSelectedId(id)` + 展开详情面板     |
| 打开新窗口详情           | `setSelectedId(id)` + `window.open(...)` |
| URL 参数 `?id=...`      | `setSelectedId(id)` on mount           |
| push `TrafficUpdated`   | 不 setSelectedId；仅更新 recordsMap    |
| `handleTrafficDeleted`  | 若 selectedId 命中，`setSelectedId(undefined)` |

useEffect 只关心 `selectedId` 与 `currentRecord?.id` 的关系，不感知
handler 触发源。

### body 拉取并行 + 去重

`fetchTrafficDetail` 内部同时发起 request body / response body / raw body
请求。这些请求各自只发一次，因为：

- `fetchTrafficDetail` 本身只被 useEffect 调用一次（受 ref guard 保护）；
- 内部 `.then` / `.catch` 不做重试；
- SSE 打开状态下自动跳过 response body 请求（`isOpenSse = is_sse &&
  socket_status.is_open`）。

## CLI + Web + Admin API

### CLI

- 无影响。

### Web

- Traffic 页详情面板：单击 / 双击 / URL 直达 / 新窗口 detach 都只触发
  一次详情拉取。
- 详情面板顶部若显示 loading spinner，短暂闪现一次即完成。

### Admin API

- 无接口变更。相关端点：
  - `GET /_bifrost/api/traffic/{id}`
  - `GET /_bifrost/api/traffic/{id}/body/request` （含 `?raw=&encoding=` 变体）
  - `GET /_bifrost/api/traffic/{id}/body/response`
- 后端未新增去重逻辑；前端 guard 是唯一防御层。

## Sync 边界

- 无。

## Phase 1-4

### Phase 1（历史，已完成）

- 移除行 handler 中的 `fetchTrafficDetail` 调用；handler 仅 `setSelectedId`。
- 在 `selectedId` useEffect 中加入 `lastAutoFetchSelectedIdRef` guard。

### Phase 2（历史，已完成）

- `fetchTrafficDetail` 中加入 `preserveBodies` 逻辑，避免同 id 重拉时闪
  空 body。

### Phase 3（当前维护）

- 保持 handler 与 useEffect 的分工不变；新增详情入口（例如快捷键
  Ctrl+Enter）都必须遵循 `setSelectedId only` 约定。

### Phase 4（可选未来）

- 若需要“强制 refresh 详情”按钮，可显式清 `lastAutoFetchSelectedIdRef.current`
  并再调用 `fetchTrafficDetail`。当前无产品需求。

## 测试方案

### 单元测试

- `web/src/stores/useTrafficStore.test.ts` 现有 fetch 相关用例保持。
- 建议新增：
  - `fetchTrafficDetail invoked once per selectedId change`
  - `fetchTrafficDetail skipped when currentRecord id already matches`
  - `fetchTrafficDetail preserves existing bodies when re-selecting same id`

### 组件 / E2E

- Playwright `web/tests/ui/traffic.spec.ts`：可加断言 —
  监听 route `/api/traffic/*/body/request`，选中一行后回放 network
  记录，验证 request body 请求只发生一次。
- E2E-verify 场景：`select traffic row -> assert exactly one detail fetch`。

### 手工

- 打开 Traffic 页，选中一行，DevTools Network 过滤 `/api/traffic/`：
  - `/api/traffic/{id}` 计数 = 1
  - `/api/traffic/{id}/body/request` 计数 = 1
  - `/api/traffic/{id}/body/response` 计数 = 1（非 open SSE）
- 双击同一行：不新增请求。
- 单击其他行：新增另一组请求，各一次。

### human_tests

- `human_tests/webui-traffic.md`：Traffic 详情面板用例应包含 “Network
  面板不重复出现同一详情请求” 断言。

## Review / Fix / Test 闭环

### 第 1 轮

- Grep：`git grep -n "fetchTrafficDetail(" web/src` 检查所有调用点，
  预期只有 `Traffic/index.tsx` 中的 useEffect 一处 + `useTrafficStore.ts`
  的定义。
- 复核所有行 handler（`handleSelect` / `handleDoubleClick` /
  `handleOpenDetailInNewWindow` / 快捷键）确认只 `setSelectedId`。
- 复核 URL 直达路径：mount 时 `setSelectedId(urlParams.id)` 是否走同一
  useEffect（是）。

### 第 2 轮

- 手工验证 Network 面板不重复。
- 复核 `preserveBodies` 分支：切换到已选行时不清空 body，避免 UI 闪空。
- 复核 push 更新 currentRecord 后 useEffect 不重拉：`currentRecord?.id
  === selectedId` 分支覆盖。

## 风险与决策

- **决策**：guard 用 ref 而不是新 state。原因：ref 变化不触发 rerender，
  语义上是“对上一次触发的记录”，与 UI 状态解耦。
- **决策**：不做基于 request key 的 dedupe 层（例如 SWR / react-query）。
  原因：详情拉取本质是主动交互驱动，单一入口 + ref guard 已经足够；
  引入 SWR 会带来更大的重构面。
- **风险**：未来新增详情拉取入口（例如快捷键、右键菜单“刷新”）时可能
  再次绕过 useEffect。缓解：文档 + code review + 建议加 eslint rule 禁用
  `useTrafficStore.getState().fetchTrafficDetail` 直接调用（放行位于
  `Traffic/index.tsx` 的白名单）。
- **风险**：如果 `fetchTrafficDetail` 引用发生变化（例如 store rebinding），
  useEffect 会重跑。当前 Zustand store action 引用稳定，无此风险。
