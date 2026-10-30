# 搜索会话稳定性

## 问题与边界

SearchMode 原先每秒汇总抓包变动并刷新搜索；reset、超过 500 个变动 ID 的积压和刷新失败会重新执行完整搜索，清空结果并切换加载态。持续流量由此不断重启已完成搜索。

本次只修改 WebUI 搜索会话，不修改普通实时抓包、后端搜索协议、搜索范围及筛选条件的既有含义。保留打开共享搜索 URL 时的一次显式恢复搜索。

## 行为

- Search/Enter 提交新会话；后台抓包变化不发起搜索，也不替换已完成结果。
- 提交时固定高亮关键词，草稿关键词变化不改变既有结果的高亮；编辑关键词/范围取消旧请求，下一次搜索由用户提交。
- 分页仅能使用与已提交会话一致的关键词、范围和筛选，避免把不同查询的结果混到旧列表。
- 同一会话的流式批次及分页以 record.id 合并；重复 ID 原位更新，新 ID 追加，保留未改动行对象与已有顺序。
- virtualizer 使用 record.id 作稳定标识，总高度跟随实际尺寸更新，分页不卸载列表容器。
- generation 隔离旧响应；在启动非流式 fallback 前也检查 generation，防止旧响应晚到后借用新请求的控制器。
- 搜索完成由 SSE done/响应结束及分页完成确定。后续流量只更新普通实时抓包 store。

## 验证

状态测试覆盖流式多批、分页去重、过期响应、取消/清空、草稿与已提交查询隔离。组件测试证明抓包 reset/overflow 不触发自动搜索。真实 Chromium 回归覆盖选中记录、滚动位置、可见 DOM 节点、重复分页记录、完成后连续抓包通知和再次手动提交；另有独立真实代理套件：新编译二进制直接提供内嵌页面，1100 条 loopback 请求触发真实分页，再持续产生 24 条新流量；验证实际 push、完整详情面板和响应 Body 稳定，重复同一查询可以看到新数据。未覆盖后端扫描压力。

执行方式见 [搜索会话稳定性回归](../human_tests/webui-search-session-stability.md)。

## Review/Fix/Test 闭环计划

以下是后续搜索行为变更应执行的计划，不是本次文档补充的执行记录。变更路由为前端行为/状态/API 集成；依赖既有 React、Zustand、虚拟列表及搜索 API。仅补充文档时，只检查文档格式、路径、链接和内容一致性，不重新运行下列运行时测试，也不触发 Rust 校验或覆盖率。

命令假定已安装项目要求的 Rust/pnpm、`web/` 依赖及 Playwright 浏览器，并已按真实场景用例准备任务专用数据目录；云环境可用 `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` 指定浏览器。

### 第一轮：搜索生命周期与状态一致性

- **Review 范围**：`web/src/stores/useSearchStore.ts`、`web/src/components/SearchMode/index.tsx` 及对应测试。对照用户目标检查显式提交、搜索完成后的稳定性、草稿与已提交条件隔离、generation、取消/清空、旧响应晚到，以及普通实时抓包不受影响。
- **修复策略**：针对发现的触发链或并发缺陷补充可复现的回归断言，再做最小实现修复；不以削弱断言或恢复后台完整搜索来消除失败。
- **复测命令**：在仓库根目录执行类型检查及受影响的状态/组件回归；失败先区分实现、测试或环境问题，修复后重跑失败路径。

```bash
pnpm --dir web exec tsc -b
pnpm --dir web exec vitest run \
  src/stores/useSearchStore.test.ts \
  src/components/SearchMode/index.test.tsx \
  src/components/SearchMode/SearchResultsList.test.tsx \
  src/pages/Traffic/searchUrl.test.tsx \
  src/stores/useTrafficStore.test.ts \
  src/pages/Traffic/filterEpoch.test.tsx \
  src/pages/Traffic/clearSelection.test.tsx
```

### 第二轮：增量列表与真实链路

- **Review 范围**：基于第一轮修复后的最新 diff，复查 `web/src/components/SearchMode/SearchResultsList.tsx`、`web/tests/search/`、两个专用 Playwright 配置及本方案、真实场景用例。重点检查稳定行标识、去重与顺序、虚拟总高度、分页时滚动/选择/详情保持，以及测试发现范围隔离。
- **修复策略**：修复身份重建、尺寸缓存、分页混入旧条件或验证缺口，补上对应断言；环境失败记录实际原因，不能记为通过。若本轮仍有修复，追加下一轮 Review/Fix/Test，直到目标、实现、测试与文档一致。
- **复测命令**：在仓库根目录复跑核心状态/组件测试和夹具浏览器回归；真实链路先编译最新代码，再按 [TC-SS-04](../human_tests/webui-search-session-stability.md) 执行，使用隔离数据与 loopback，不修改系统代理。

```bash
pnpm --dir web exec vitest run \
  src/stores/useSearchStore.test.ts \
  src/components/SearchMode/index.test.tsx \
  src/components/SearchMode/SearchResultsList.test.tsx
pnpm --dir web exec playwright test -c playwright.search.config.ts
CARGO_TARGET_DIR=./.bifrost-ui-target cargo build --bin bifrost
pnpm --dir web exec playwright test -c playwright.search-live.config.ts
```

校验要求：前端变更执行适用的格式、类型与测试检查；只有同时修改 Rust/后端时才追加 `rust-project-validate` 及对应 Rust 检查。文档更新要求：同步本方案、`human_tests/webui-search-session-stability.md` 与其在 `human_tests/readme.md` 中的索引，保持入口和预期一致。

## 已有验证记录

搜索实现已在提交 `8595a6ab2e3e446f1b913f98bee902274ea2ede8` 上完成 52 项相关单元/组件测试、夹具浏览器与真实代理回归；[CI #3620](https://github.com/bifrost-proxy/bifrost/actions/runs/41051589089) 的最终结果为 `completed / success`。这是此前实现验证的记录，不代表本次文档补充重新执行了这些测试。
