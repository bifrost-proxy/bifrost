# 请求详情重复拉取修复

## 背景

前端在打开请求详情时会触发重复的详情、请求体、响应体接口请求，影响性能与后端负载。

## 现状

请求详情的拉取由选中项变化触发的自动拉取与行点击/双击处理器中的手动拉取共同触发，导致同一请求 ID 被拉取两次。

## 目标

- 同一请求 ID 在一次选中操作中只触发一次详情拉取
- 保持交互行为不变（单击选中、双击展开详情）

## 方案

- 移除行点击与双击处理器中的手动详情拉取，handlers 仅调用 setSelectedId
- 仅保留 selectedId 变化的自动拉取逻辑作为唯一入口
- 自动拉取 useEffect 内通过 lastAutoFetchSelectedIdRef 守卫，避免同一 selectedId 在依赖（如 currentRecord?.id）刷新时重复触发
- 若 currentRecord.id 已与 selectedId 一致（详情已加载），则只更新 ref 不再请求

## 实现位置

- web/src/pages/Traffic/index.tsx：lastAutoFetchSelectedIdRef + selectedId useEffect，handleDoubleClick 仅 setSelectedId

## 影响范围

- 前端流量列表页的行选择与详情获取逻辑

## 验证

- 打开请求详情时网络请求不再重复
- 请求体与响应体接口只请求一次
- 自动化回归测试 (planned, not yet shipped as of 2026-06-17)

## 状态（2026-06-17）

- 已实现并与代码对齐：web/src/pages/Traffic/index.tsx 中的 ref-guarded selectedId useEffect 与无手动拉取的 handleDoubleClick
