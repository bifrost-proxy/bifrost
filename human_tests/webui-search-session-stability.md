# 搜索会话稳定性回归

本用例验证前端生产组件和真实 Chromium 布局，使用确定性搜索 API 响应与抓包 store 通知。它不替代真实代理持续流量集成测试；后端搜索算法不在本次改动范围内。

## 执行

在 `web/` 安装锁定依赖后运行：

```bash
pnpm exec playwright test -c playwright.search.config.ts
```

如使用系统 Chromium，可通过 `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` 指定云环境内的浏览器。测试使用隔离 Vite 端口 3047，后端目标端口 8847，不连接正式服务。

## TC-SS-01 显式提交

输入 match，确认没有搜索请求；点击 Search 后出现结果。修改关键词和域名筛选不发请求，再点击 Search 才请求。Exit 清空搜索会话并回到普通模式。

## TC-SS-02 分页增量与滚动

返回 60 条记录，选中 r0 并确认详情选择；滚动至 700，记录可见 r10 的 DOM 节点。点击 Load More，返回包含重叠 r59 的后续 31 条记录。确认最终 90 条唯一记录、r10 节点相同、滚动位置 700、详情仍选择 r0。

## TC-SS-03 完成后持续流量

全部分页完成后连续注入五次抓包 reset/incomplete 通知，跨越原自动刷新间隔。确认没有新增搜索请求、没有加载态重启、可见行内容与滚动位置保持不变、详情仍选择 r0。

## 配套状态回归

`src/stores/useSearchStore.test.ts` 覆盖多批流式结果、重复 ID、未改变行引用、全任务完成、显式再次搜索、旧响应晚到、旧分页晚到、取消、清空、快速改查询/范围、未提交筛选的分页保护。

`src/components/SearchMode/index.test.tsx` 以假时间覆盖完成后连续 reset/overflow 通知及显式再次提交。普通流量筛选与清空选择使用原有 `useTrafficStore`、`filterEpoch` 和 `clearSelection` 用例验证。
