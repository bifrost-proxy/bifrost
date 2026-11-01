# Playwright 隔离后端语言初始化

## 目标与范围

完整 UI audit 的四项 Traffic 用例导航到 `startIsolatedBackend()` 分配的新端口。
`playwright.config.ts` 的 `storageState` 仅覆盖配置中的 baseURL origin，
因此这些新 origin 使用产品默认的 `zh-CN`，与英文定位器不一致。
修复仅修改测试初始化，不修改产品默认语言、普通用户存储或已有业务断言。

## 方案

`helpers/isolated-locale.ts` 提供 `gotoIsolatedTraffic(page, baseUrl)`：

- 在导航前注册 `page.addInitScript`，让语言值在应用 hydration 前就可读取。
- 仅在目标 origin 的 `bifrost-locale` 不存在时写入 English。
- 使用当前 Zustand persist 的 `{ state: { locale: "en" }, version: 0 }` 格式。
- 保留已有语言值，避免刷新或后续导航覆盖测试选择。
- 两个 Traffic spec 的隔离页面入口统一使用该 helper，普通 baseURL 导航保持原样。
- 专用搜索套件仍位于 `web/tests/search/`，不改变默认 UI 发现范围。

## 用户目标验证清单

- 四项原失败用例先真实复现，再在修复后重跑，保留全部原业务断言。
- 新增回归验证新 origin 仍默认中文、helper 初始化英文、刷新保持英文、
  已有中文选择不被覆盖、其他 origin 不被 seed。
- 运行完整 UI audit，依据真实退出码及 Playwright 结果判断，不能依据
  workflow 的 `continue-on-error` 步骤颜色判断通过。
- 执行格式、类型、相关 lint 和启动护栏检查；不修改生产 Rust，因此
  Rust coverage、workspace 测试不适用；自动浏览器回归足够覆盖本次测试修复，
  不新增 `human_tests/` 产品行为用例。
- 提交独立草稿 MR 并跟进准确 head 的 CI/评审，不合并或发布。

## Review/Fix/Test 计划

### 第 1 轮

复读用户范围，检查 helper 的 origin 限制、首次加载时序、已有值处理和
两个 spec 的调用位置；检查 `git status --short` / `git diff` / 新增文件。
修复发现后运行四项用例、新增语言回归和格式/type/lint。

### 第 2 轮

基于最新 diff 重新检查断言没有弱化、没有产品变更、没有扩大搜索套件发现范围，
核对本文与实际实现一致；重跑受影响用例及完整 UI audit。
若有残余失败，先归因，超出语言初始化范围时单独报告，不扩展产品修复。

## 验证记录

执行环境为云端 Linux，浏览器使用系统 Chromium/FFmpeg；环境没有 Rust 工具链，
后端使用官方 npm 发布的 `@bifrost-proxy/bifrost-linux-x64@0.0.202`，
与任务基线 main 的版本一致。此处不宣称执行过源码 Rust 构建。

修复前四项全部失败：英文 Fuzzy Search / Add Filter 定位器超时，
以及 `Port 1` 与实际 `端口 1` 不一致。修复后这四项均越过原语言失败位置，
三项筛选用例及新增语言边界回归通过。

搜索用例暴露后续残余失败：它要求新流量在 5 秒内自动进入已完成的搜索结果。
这与 [搜索会话稳定性](search-session-stability.md) 的显式提交契约冲突，
不是语言初始化问题。本次保留该断言，也不修改产品行为；完整 audit 的真实结果
和该残余问题应在 MR 验证记录中报告，不能把本次语言修复称为全量通过。

第 1 轮复查修正了新增回归中图标参与 accessible name 的定位器；
格式、产品类型检查、受影响测试类型检查、相关 ESLint 和启动护栏均通过。
第 2 轮复查确认已有业务断言完全保留，改动只有隔离页面入口与新增语言回归；
复测结果及完整 audit 以 MR 的最终验证记录为准。
