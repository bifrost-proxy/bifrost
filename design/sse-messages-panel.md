# SSE Messages 面板展开/折叠丢失修复方案

## 背景
SSE Messages 面板在 request/response 展开与折叠切换时会丢失实时流数据。问题表现为 Response 面板重新渲染后消息列表被清空。

## 现状分析
- 展开/折叠会切换两套不同的布局树，导致 Response Panel 被卸载重建
- Messages 组件内维护 SSE 列表状态，组件卸载即清空
- Messages Tab 在非折叠模式下未做保活，切换 Tab 也会卸载

## 方案
1. 保持面板树稳定
   - 始终使用 Splitter 布局渲染 Request/Response
   - 通过 Splitter.Panel 的 size 控制折叠高度
2. 折叠模式不允许拖拽
   - 折叠态下禁用 resizable，避免拖拽干扰布局
3. 保活 Messages Tab
   - Response Panel 始终设置 keepAliveTabs 为 Messages，避免切换 Tab 时卸载
4. 折叠时不卸载内容区域
   - Panel 折叠仅隐藏内容，不卸载 Messages 组件

## 影响范围
- TrafficDetail 面板布局渲染
- Response Panel 中 Messages Tab 生命周期
- Panel 折叠显示逻辑

## 回滚方案
如出现布局异常，回退到原本的两套布局分支渲染逻辑。

## 验证方式
- 展开/折叠 request/response 面板时，Messages 流数据不丢失
- 切换 Response 的 Tab 后返回 Messages，流数据仍在持续追加

## 现状对照 (2026-06-17)
方案已落地，主要实现位置：
- `web/src/components/TrafficDetail/index.tsx`：统一使用垂直 `Splitter` 渲染 Request/Response，折叠态通过 `requestPanelProps`/`responsePanelProps` 设置 `size` 并 `resizable: false`；Response Panel `keepAliveTabs` 包含 `Messages`。
- `web/src/components/TrafficDetail/Panel/index.tsx`：基于 `keepAliveTabs` 集合保活对应 Tab 内容，未激活时仅隐藏不卸载。
- `web/src/pages/Replay/components/MessagesPanel.tsx` 与 `ResponsePanel.tsx`：Replay 流程下 `MessagesPanel` 从 `useReplayStore` 读取 `sseEvents`/`wsMessages`，状态由 store 持有，组件复挂载不会清空。
