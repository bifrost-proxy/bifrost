# WebSocket Messages 面板滚动与虚拟化

## 背景

WebSocket Traffic Detail 中的 Messages 面板需要展示一次连接上的所有 frame（可能上千条），这个面板对 UX 有几个硬约束：

- 长列表必须能顺畅滚动，不能因帧数量增长导致 DOM 崩塌。
- 上下方向都需要 "跳到端点" 的兜底按钮，让用户在长会话里快速回到最新或最早。
- 帧持续推送时，用户如果没有主动滚动，希望留在最新位置；一旦用户主动往上翻，就不要被后续帧强行拉走。
- 面板要与 TrafficDetail 的整高布局配合，不能撑破外层容器。

早期设计文档提到过 `Ant Table + virtual + scroll.y` 方案，但真正落地的实现是自定义滚动容器 + `@tanstack/react-virtual`。本 doc 把落地事实、当前差异与后续 follow-tail 计划固定下来，避免下一波刷新再走弯路。

## 用户目标验证清单

### 必须实现

- WebSocket Messages 面板对任意帧数量都能正常渲染，浏览器主线程不阻塞。
- 面板具备独立滚动容器；容器高度由父级 `flex: 1 / minHeight: 0` 决定，滚动条永远在容器内部而不是页面上。
- 悬浮 "回到顶部"、"回到底部" 按钮在真实需要时才出现（`isAtTop === false` / `isAtBottom === false`），点击后能精准跳到端点。
- Frame 数据支持 Text / Binary / Ping / Pong / Close / Continuation / SSE 等类型，行高稳定。
- 面板与 SSE 面板复用同一个 UX 语言（悬浮按钮位置、快捷键、样式）。

### 必须不破坏

- Traffic Detail 其它面板（Overview / Headers / Body / Timeline / Frames Search）布局与响应式行为。
- TrafficDetail 的三栏 SplitPane 收缩与拉伸。
- 虚拟化列表在大量重渲染时不能触发 React 警告或抖动。
- WebSocket 连接停止后帧仍能被回看，滚动位置不因新数据缺失而重置到顶部。

### 必须真实验证

- Playwright 或 human_tests 能真实看到 1k+ 帧顺畅滚动、悬浮按钮出现/消失、点击生效。
- 帧持续增长时确认当前 UI 行为（识别位置 + 悬浮按钮）与文档一致，不需要文档改口。
- 后续正式上线 follow-tail 前，本 doc 需要更新为完整状态机；不能只在代码里悄悄改。

## 产品语义

### 当前实现事实

- 组件位置：`web/src/components/TrafficDetail/panes/Messages/index.tsx`。
- 关键导出：`WsMessageList`（同文件第 102–347 行左右），`SseMessageList`（`SseMessageList.tsx`，与 WS 面板并列）。
- 虚拟化：`@tanstack/react-virtual`。
- 滚动容器：独立 `div`，`overflow: auto`；行高 `estimateSize: () => 36`。
- 边界识别：`onScroll` 里同时计算 `distanceToBottom` 与 `distanceToTop`，用来切换 `isAtTop` / `isAtBottom` 状态。
- 交互：
  - `handleScrollToTop`：调用 `rowVirtualizer.scrollToIndex(0, { align: "start" })`。
  - `handleScrollToBottom`：调用 `rowVirtualizer.scrollToIndex(frames.length - 1, { align: "end" })`。
- 悬浮按钮：`data-testid="ws-scroll-top"` / `data-testid="ws-scroll-bottom"`。
- 已具备能力：长列表滚动、虚拟化渲染、手动跳端点。

### 与旧设计的差异

- 未使用 Ant Design Table，也没有 `ResizeObserver + scroll.y` 的表格高度方案。
- 没有 "新帧到达时自动贴底" 的完整 follow-tail 状态机。当前 UX 更接近 "识别位置 + 提供快捷按钮"。
- SSE 面板 (`SseMessageList`) 走类似的虚拟化实现，同样没有 follow-tail。

### 后续 follow-tail（未上线）

若后续要真正做 follow-tail，需要显式设计并写入本 doc：

- 明确用户 mental model：用户滚到底部 → 自动贴底；用户主动向上翻 → 停止贴底，直到再次手动回到底部或点击 "回到底部"。
- 增加显式 `followTail` 状态，替代目前依赖 `isAtBottom` 的隐式行为。
- 处理 `frames` 变化触发 rerender 时的锚点保持，避免抖动或误判。

**状态**：`planned, not yet shipped as of 2026-06-17`。上线前本文档必须补齐状态机与交互规则。

## 技术细节

### 组件结构

```
<TrafficDetail>
  <PaneTabs>
    <MessagesTab>
      <WsMessageList frames={...}>
        <div ref={containerRef} style={{ height: '100%', overflow: 'auto' }}>
          <div style={{ height: `${rowVirtualizer.getTotalSize()}px` }}>
            {rowVirtualizer.getVirtualItems().map(...)}
          </div>
        </div>
        {!isAtTop && <FloatingButton data-testid="ws-scroll-top" />}
        {!isAtBottom && <FloatingButton data-testid="ws-scroll-bottom" />}
      </WsMessageList>
    </MessagesTab>
  </PaneTabs>
</TrafficDetail>
```

### 虚拟化参数

- `useVirtualizer({ count: frames.length, getScrollElement: () => containerRef.current, estimateSize: () => 36, overscan: 8 })`（overscan 可按需调整，避免超快滚动时闪白）。
- `estimateSize` 固定 36px；如果未来支持多行展开预览，需要改用 `measureElement` 并接受布局 shift，同步更新本 doc。

### 滚动状态

- `handleScroll`：`distanceToBottom = scrollHeight - scrollTop - clientHeight`；阈值默认 `threshold = 8`。
- `setIsAtTop(scrollTop <= threshold)`；`setIsAtBottom(distanceToBottom <= threshold)`。
- `useEffect` 里绑定 `passive: true` 的 scroll listener，卸载时清理。

### 与 SSE 面板的关系

- `SseMessageList` 与 `WsMessageList` 结构镜像；两者共享相同悬浮按钮样式与 API。
- 未来 follow-tail 若上线，需同时改造两处，或抽出通用 `VirtualMessageList`（已有 `web/src/components/VirtualMessageViewer/VirtualMessageList.tsx` 作为基础可参考）。

## CLI + Web + Admin API 边界

### CLI

- 不涉及 CLI 参数。

### Web UI

- Traffic Detail → Messages Tab：唯一入口。
- data-testid 用于 Playwright：
  - `ws-scroll-top`
  - `ws-scroll-bottom`
- 面板尊重 TrafficDetail 三栏分割高度，`flex: 1 / minHeight: 0`。

### Admin API

- 底层帧数据由 `GET /_bifrost/api/traffic/:id/frames`（`crates/bifrost-admin/src/handlers/frames.rs`）返回。
- 前端订阅走 `crates/bifrost-admin/src/connection_monitor.rs` push 通道；虚拟化列表按追加顺序渲染，不修改 API。

## Sync 边界

- Frame 数据不参与 Sync。本 doc 完全不涉及 Sync/Group Sync/规则共享。

## Phase 拆分

### Phase 1（已完成）

- 用 `@tanstack/react-virtual` 替换早期 Ant Table 方案。
- 独立滚动容器 + `overflow: auto`。
- 添加 `data-testid="ws-scroll-top"` / `ws-scroll-bottom` 悬浮按钮。
- 输出 `isAtTop` / `isAtBottom` 状态供未来 follow-tail 使用。

### Phase 2（已完成）

- 与 SSE 面板 UX 对齐，样式与位置一致。
- Traffic Detail 布局在小尺寸屏幕下保持滚动容器不撑破外层。

### Phase 3（Planned）

- 引入 `followTail` 显式状态机：
  - 初始 `followTail = true`。
  - 用户向上滚 → `followTail = false`。
  - 用户手动点 "回到底部" 或滚到底部 → `followTail = true`。
  - `frames` 增长时，`followTail === true` 才自动 `scrollToIndex(last)`。
- 补 Playwright：`web/tests/ui/*` 新增 `ws-message-follow-tail.spec.ts`。
- 上线时必须同步更新本 doc（去掉 "planned"）。

### Phase 4（Planned，与 Phase 3 同批）

- 处理多行展开预览时的 `measureElement`。
- 与 SSE 面板共用状态机。

## 测试方案

### 单元/组件测试

- `WsMessageList` 目前无独立 unit test。建议在 Phase 3 前补：
  - 空 frames：`isAtTop === true && isAtBottom === true`，两个悬浮按钮均不渲染。
  - 大量 frames + 起始滚动位置：`isAtBottom === true`，`ws-scroll-bottom` 不渲染。
  - 滚到中间：两个按钮同时渲染。

### E2E / Playwright

- 现状：`web/tests/ui/` 下暂无专门的 `ws-message-*.spec.ts`；Traffic 相关的 UI 用例主要覆盖 traffic 列表和详情主要 Tab。
- Phase 3 上线前需要新增：
  - `web/tests/ui/ws-message-scroll.spec.ts`：
    - 构造 1k+ 帧的 mock 会话（可用 `test_websocket_frames.sh` 生成的 fixture）。
    - 打开 Traffic Detail → Messages Tab；断言容器 `scrollHeight > clientHeight`。
    - 向上滚断言 `ws-scroll-bottom` 出现；点击后落到底部。
    - 向下滚回底部断言按钮消失。
  - Phase 3 follow-tail 上线时同文件补 `followTail` 状态。

### human_tests

- `human_tests/proxy-websocket-sse.md::TC-PWS-05`（"管理端 UI WebSocket 消息面板"）已包含 Messages 面板打开与帧展示回归。刷新时需明确：
  - 目前的滚动 UX 是 "手动跳端点"，不是 "自动贴底"。
  - `ws-scroll-top` / `ws-scroll-bottom` 悬浮按钮的可见性判定。
  - 若后续上线 follow-tail，需要新增 `TC-PWS-XX` 用例并同步 `human_tests/readme.md`。

### E2E 后端支撑

- `e2e-tests/tests/test_websocket_frames.sh`：真实建立 WS 连接并推送多类型帧（text/binary/ping/pong/close），可用作 Messages 面板前端手工验证的数据源。
- `e2e-tests/tests/test_frames_admin_api.sh`：验证 `/api/traffic/:id/frames` 返回结构，前端虚拟化依赖该接口。

## Review / Fix / Test 闭环

### 第 1 轮

- 复查用户目标：长列表可滚、悬浮按钮存在、面板不撑破布局。
- 复查 diff：确保只改 `WsMessageList` 与必要的样式；不误改 SSE 面板；不引入 follow-tail 半成品。
- 复测：
  - 手动打开一条长 WS 会话，验证 `ws-scroll-top` / `ws-scroll-bottom` 显示/隐藏一致。
  - `pnpm --filter web lint` 与 `pnpm --filter web build` 确认没有类型/构建错误。

### 第 2 轮

- 复查第 1 轮修复；确认按钮的 `data-testid` 未改名（Playwright 依赖）。
- 复跑 SSE 面板：确认 `SseMessageList` UX 与 WS 面板一致，未被误伤。
- 如果修改牵涉 `connection_monitor.rs`，追加运行 `cargo test -p bifrost-admin frames`。

## 校验要求

- `pnpm --filter web lint`
- `pnpm --filter web test`（若新增 unit test）
- `pnpm --filter web build`
- 手动打开真实 WS 长会话验证滚动 UX。
- 收尾按 `rust-project-validate` 技能兜底：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`；如环境阻塞，需要记录风险。

## 文档更新要求

- 更新本 doc：始终反映真实实现与 planned 项，避免旧的 Ant Table 方案幽灵指导。
- 更新 `human_tests/proxy-websocket-sse.md::TC-PWS-05` 中滚动 UX 描述与悬浮按钮 `data-testid`。
- Phase 3 follow-tail 落地时同步补 `TC-PWS-XX` 与 `human_tests/readme.md` 用例数量。

## 风险与决策点

- **follow-tail 半成品风险**：`isAtBottom` 状态已经在 `WsMessageList` 中输出，容易被误当成 "自动贴底" 触发条件。上线 Phase 3 前不应在代码中偷偷把 `frames` 变化 → `scrollToIndex(last)` 的逻辑加上；如果被加上，需要文档同步。
- **虚拟化行高假设**：`estimateSize: () => 36` 假定每帧一行小卡片。若后续支持行内展开或多行 payload 预览，必须切到 `measureElement` 并接受 layout shift，同时验证 `overscan` 是否够。
- **SSE 面板一致性**：任何 WS 面板 UX 改动都应同步评估 `SseMessageList`；两个面板 mental model 必须一致，否则用户会困惑。
- **性能极限**：目前 `overscan` 默认较小；如果单次连接帧数超过 10k，可能需要提高 `overscan` 或延迟渲染 payload preview；上线前需要构造压测数据 (`e2e-tests/mock_servers/http_ws_echo_server.py`) 验证。
- **Ant Table 遗留**：项目中仍有 Ant Table + virtual 的其它用法（如 `web/src/components/TrafficTable/VirtualTrafficTable.tsx`）。不要把它们混为一谈；本 doc 只覆盖 Traffic Detail Messages 面板。
