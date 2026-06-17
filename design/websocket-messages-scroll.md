# WebSocket Messages 滚动与贴底

## 现状结论

滚动问题已经修复，但实现方式与原设计不同；当前用的是自定义滚动容器 + `@tanstack/react-virtual`，不是 Ant Table 的 `virtual + scroll.y`。

## 当前实现

- WebSocket 消息列表位于 [`web/src/components/TrafficDetail/panes/Messages/index.tsx`](../web/src/components/TrafficDetail/panes/Messages/index.tsx) 的 `WsMessageList`。
- 列表使用 `useVirtualizer()` 做虚拟渲染，滚动容器是独立的 `div`，`overflow: auto`。
- 组件会实时计算 `isAtTop` / `isAtBottom`，并在界面中显示“滚动到顶部 / 底部”悬浮按钮。
- 目前已具备：
  - 长列表可滚动浏览；
  - 虚拟化渲染；
  - 手动快速回到底部/顶部。

## 与旧设计的差异

- 未使用 Ant Table。
- 没有 `ResizeObserver + scroll.y` 的表格高度方案。
- 目前没有“新帧到达时自动贴底”的完整跟随逻辑；更接近“识别当前位置 + 提供快捷按钮”。

## 文档结论

- “滚动条缺失”问题已解决。
- “自动贴底”仍不是当前实现的主行为；如果后续需要真正的 follow-tail，需要单独补充状态机与交互规则 (planned, not yet shipped as of 2026-06-17)。
