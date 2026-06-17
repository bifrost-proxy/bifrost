# Web Admin Rules / Values 同步策略

## 现状结论

旧文档中的“页面级首次加载 + 手动刷新”只说对了一半。当前实现已经继续演进为：

- `Rules`：规则列表仍按需 HTTP 拉取；全局值通过 push 订阅。
- `Values`：首次进入仍调用一次 `fetchValues()` GET 拉取，并同时通过 push 订阅 `values_update` 持续接收快照（混合模式）。

## 当前实现

- 全局初始化阶段不再轮询 `rules` / `values`。
- [`web/src/pages/Rules/index.tsx`](../web/src/pages/Rules/index.tsx)
  - 首次进入时如果规则列表为空，调用 `fetchRules()`。
  - 同时订阅 `need_values`，让规则编辑器拿到最新 values 快照。
- [`web/src/pages/Values/index.tsx`](../web/src/pages/Values/index.tsx)
  - 首次进入若本地 `values` 为空，调用一次 `fetchValues()` 做 HTTP 初始化拉取。
  - 同时通过 `pushService.connect({ need_values: true })` 订阅 `values_update`，由 `applyValuesSnapshot` 合并后续推送。
  - 卸载时取消订阅并 `disconnectIfIdle()`。
  - 纯 push-first（移除首次 GET）方案：planned, not yet shipped as of 2026-06-17。

## 与旧方案的差异

- `Values` 已从“首次 GET + 手动刷新”演进为“首次 GET + push 订阅”的混合模式，手动刷新按钮已下线。
- `Rules` 也不是完全手动刷新模型，因为 values 依赖已经走 push 通道。

## 结论

这份文档不应再把 Rules / Values 视作同一种同步模式；当前是“Rules 混合模式（按需 GET + values push），Values 混合模式（首次 GET + values push）”。
