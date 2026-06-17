# 管理端 Performance 设置

## 现状结论

“延迟提交草稿”方案已经落地，而且范围比旧文档更大。

## 当前实现

- `Settings` 页维护本地 `trafficDraft`，滑动时先更新本地显示。
- 各字段通过独立的 600ms 防抖计时器（`schedulePerformanceUpdate` 中 `window.setTimeout(..., 600)`）提交到 `PUT /api/config/performance`。
- 每个字段在 `perfUpdateTimers` 中持有独立 timer key，连续调整只保留最后一次提交。
- 提交失败时会回退到最近一次服务端配置（`setPerfDraft(performanceConfig.traffic)` + `setBreakpointPerfDraft`）。
- 当前 Performance 面板包含（验证于 2026-06-17，`web/src/pages/Settings/tabs/PerformanceTab.tsx` 与 `web/src/pages/Settings/index.tsx`）：
  - `Max Records`（UI 上限 100000，对应 `handleMaxRecordsChange`）
  - `Max DB Size`（上限 10 GiB）
  - `Max Body Inline Size (DB)`（上限 10 MiB）
  - `Max Body Buffer Size`（上限 64 MiB）
  - `Max Body Probe Size`（上限 1 MiB）
  - `File Retention Days`（上限 7 天）
  - `Breakpoint Timeout`（带 min/max 区间，`handleBreakpointTimeoutChange`）
  - `Enable Binary Traffic Capture` 开关（对应 `binary_traffic_performance_mode`）
  - `Inject Bifrost Badge` 开关（`handleInjectBifrostBadgeChange`）
  - 存储统计面板（body / frame / ws_payload store stats）与 `Clear Cache` 按钮（`DELETE /api/config/performance/clear-cache`）

## 与旧文档的差异

- `Max Records` 上限已经不是旧文档中的 50000；当前 UI 文案与实现以 100000 为上限。
- 设计范围已经从单纯“滑条防抖”扩展为一组流量/存储参数的统一草稿编辑体验。

## 结论

- 该方案已实现，文档不应再停留在单一交互优化层面。
- 当前更准确的定位是“Performance 配置草稿态 + 分字段防抖提交”。
