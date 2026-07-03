# Replay 执行中取消按钮可达性

## 背景

Bifrost 管理端的 `Replay` 页面允许用户构造并重放 HTTP / SSE / WebSocket 请求。历史实现里，Replay 页面在“执行中”状态会给外层 composer 区域套一层全屏 `Spin` 遮罩，遮罩覆盖了 `RequestPanel` 中已经从 `Send` 切换为 `Cancel` 的按钮，导致：

- 用户看到 `Cancel` 按钮，但点击落到遮罩上，取消不生效。
- 慢请求（例如后端 60s 长响应、hang 住的 SSE 连接）无法在前端及时中断，只能等超时或刷新页面。
- 现场排查困难：用户会以为“Replay 卡住了”，实际是 UI 层拦住了鼠标事件。

本次修复只调整执行态的视觉与可交互性，保留首屏拉取、集合列表加载、历史列表加载等场景下的整体 loading。

## 用户目标验证清单

### 必须实现

- Replay 执行 HTTP 请求过程中，`Cancel` 按钮真实可点击。
- 点击 `Cancel` 后请求立即中止，按钮回到 `Send`，`AbortController` 被触发。
- 响应区在等待首个响应字节期间给出明确“执行中”反馈，避免看起来什么都没发生。
- 首屏 loading、集合列表刷新、历史 loading 等真实需要遮罩的场景保持原来的整页 `Spin`。

### 必须不破坏

- SSE / WebSocket / 流式响应场景下，一旦开始收到内容就退出内联 executing 占位，展示实际流内容。
- 已有 Response 内容或 Traffic 记录时不再出现 executing 占位。
- Send / Cancel 按钮的键盘可达性与快捷键行为保持不变。
- 请求执行完毕（成功、失败、取消）后 executing 占位必须消失。

### 必须真实验证

- Playwright 用例真实点击 `Cancel` 按钮，断言状态回落。
- 慢响应 mock 服务真实存在，覆盖“执行中 → 取消 → 空状态”完整链路。

## 产品语义

Replay 页面的执行态有三个视觉层次：

1. **整页 Spin**：仅在“首屏加载 saved requests / groups”“集合切换 loading”等需要遮蔽整个 composer 的场景使用，由 `showSpinner = loading` 决定。
2. **RequestPanel 按钮态**：请求在执行时按钮从 `Send` 切换为 `Cancel`，绑定当前请求的 `AbortController`。
3. **ResponsePanel 内联 executing 占位**：仅当 `executing && !currentResponse && !currentTrafficRecord && !hasStreamingContent` 时才出现，位于响应区中央，不遮挡 composer。

三层严格分层：整页 Spin 不用于承载“请求执行中”，请求执行态只影响 ResponsePanel 与按钮，composer 区域始终可交互。

## 技术细节

### 前端组件

- `web/src/pages/Replay/index.tsx`
  - 保留 `<Spin spinning={showSpinner}>` 包裹整页，但 `showSpinner` 只映射 `loading`（首屏 / 列表切换），不再映射 `executing`。
- `web/src/pages/Replay/components/ResponsePanel.tsx`
  - 计算 `showExecutingPlaceholder = executing && !currentResponse && !currentTrafficRecord && !hasStreamingContent`。
  - 当 `showExecutingPlaceholder === true` 时渲染：
    ```tsx
    <div style={staticStyles.emptyState} data-testid="replay-response-executing">
      <Spin tip="Executing request..." />
    </div>
    ```
  - 当响应或流式内容到达时立即退出该占位，走原有响应渲染路径。
- `RequestPanel`
  - 保持既有 `Send/Cancel` 切换逻辑；`Cancel` onClick 调用当前请求 `AbortController.abort()`，并把 store 中 `executing` 置回 `false`。

### 状态与执行流

- `useReplayStore.executeRequest`
  - 进入执行态：置 `executing = true`，清空上一条 `currentResponse` / `currentTrafficRecord` / `hasStreamingContent`。
  - 结束执行态（成功 / 失败 / 取消）：`executing = false`。
  - `cancelRequest`：调用 abort，`executing = false`，不主动写入错误响应，保持 UI 干净。

### 关键 selector

- `data-testid="replay-response-executing"` 是唯一稳定的 UI 契约点，E2E 通过它断言执行中占位可见 / 消失。

## CLI / Admin API 边界

本项修复完全位于 Web UI，不新增 CLI 子命令，不改动 Admin API。Replay 执行仍走：

- `POST /_bifrost/api/replay/execute`（unified replay）
- `POST /_bifrost/api/replay/execute/sse`
- WebSocket replay 走 `/_bifrost/api/replay/ws` 握手

取消行为完全通过前端 `AbortController` 中断 fetch / EventSource / WebSocket，不需要新增服务端取消 API。

## Sync 边界

不涉及规则 Sync、Group Sync、跨端同步；纯前端可交互性修复。

## 实现切分

### Phase 1：拆分整页 loading 与执行态

- `index.tsx` 移除 `executing` 对 `showSpinner` 的贡献。
- 保留 `loading` 驱动的整页 Spin。

### Phase 2：ResponsePanel 内联 executing 占位

- 新增 `showExecutingPlaceholder` 计算与渲染分支。
- 复用 `staticStyles.emptyState` 保持视觉一致。
- 添加 `data-testid="replay-response-executing"`。

### Phase 3：RequestPanel 取消可达性

- 确认 `Send/Cancel` 切换在没有遮罩后可正常点击。
- 复核 `AbortController` 生命周期，避免残留监听。

### Phase 4：回归与文档

- 新增 Playwright 用例 “Replay 执行中可以点击 Cancel 中止请求”。
- 更新 human_tests Replay 章节相关描述（如已有 TC-WRP-XX 覆盖，则补一句“执行中 Cancel 可点击”）。

## 测试方案

### Playwright UI

- `web/tests/ui/admin-replay.spec.ts`
  - `test("Replay 执行中可以点击 Cancel 中止请求", ...)`（当前 line 181）
    - 起一个人为延迟的 mock endpoint。
    - 填入 URL，点击 `Send`。
    - 断言 `page.getByTestId("replay-response-executing")` 可见。
    - 定位 `page.getByRole("button", { name: "Cancel" })` 并 click。
    - 断言按钮回到 `Send`，`replay-response-executing` count 为 0。
- 需保持既有用例通过，尤其是完整响应回来后 executing 占位消失的用例。

### 单元 / 组件测试

- 无需新增 Rust 单元测试，前端组件测试可选：
  - `ResponsePanel` 在给定 `executing` prop 与不同 response state 下渲染分支的快照。

### human_tests

- `human_tests/webui-replay.md`
  - 在 Replay 执行链路的用例（如 TC-WRP-01 等）中补一句“执行中 Cancel 按钮可点击并能中止”。
- 若新增独立用例，命名遵循 `TC-WRP-XX` 递增，同时更新 `human_tests/readme.md` 计数。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核目标：Cancel 可点、executing 占位可见、不影响 SSE / WS 流式渲染。
- 复核 diff：`index.tsx` / `ResponsePanel.tsx` / `RequestPanel.tsx` / `useReplayStore.ts` / Playwright 用例是否都覆盖。
- 重点 review：executing 占位在 SSE 已经开始 streaming 时是否正确退出；cancelRequest 是否残留 dangling promise。
- 复测：Playwright Replay 相关用例 + 手工点一次 Cancel 验证。

### 第 2 轮

- 复核修复；再跑一次 `pnpm --filter web test:ui` 中 Replay 用例。
- 检查是否引入新的整页遮罩残留。

## 风险与决策点

- **风险**：如果 executing 占位在 SSE first-byte 到达后没有及时消失，可能出现内容闪烁。缓解：`hasStreamingContent` 一旦为 true 立刻退出占位。
- **风险**：Cancel 后未清 `AbortController`，下一次 Send 复用旧 controller 会立即失败。缓解：`executeRequest` 每次进入执行态都 new 一个 controller。
- **决策**：不在服务端引入 replay-cancel API，纯客户端 abort 已足够，减少后端复杂度。
- **决策**：不给 executing 占位加进度条，占位仅用 `Spin tip="Executing request..."`；进度条需要真实上游反馈，Replay 不掌握。
