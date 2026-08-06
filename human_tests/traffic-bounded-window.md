# Network 有界窗口、权威统计与实时 Search

## 功能模块说明

验证 Network 前端最多常驻 1,000 条记录后，无筛选、历史筛选、组合筛选和 Search 仍以服务端完整数据为准；验证服务端内存统计、1 秒合并推送、实时新增、断线恢复、滚动淘汰和 Search 定向增量重算不会丢失或重复数据。

## 前置条件

1. 在仓库根目录执行；不得连接或停止共享 9900 服务。
2. Playwright 用例使用动态端口、临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy` 启动隔离后端。
3. 准备当前源码的 UI 测试二进制：

   ```bash
   CARGO_TARGET_DIR=.bifrost-ui-target cargo build --bin bifrost
   ```

## 测试用例

### TC-TBW-01：服务端定向 Search 与输入边界

操作步骤：

1. 执行：

   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin targeted_record_ids --all-features
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin target_record_ids --all-features
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin web_search_conversion_preserves --all-features
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin build_where_clause_supports_record_id_filter --all-features
   ```

预期结果：

- 定向查询只返回请求 ID 集合内同时满足 keyword、scope、method 等条件的记录。
- `record_ids` 超过 500、空 ID 或超长 ID 返回 400。
- WebUI 的 Account 条件和 `record_ids` 在 API → command → SearchEngine 转换中不丢失。
- SQL 使用参数占位符，并与其他条件按 AND 组合。

### TC-TBW-02：前端实时 Search 合并与有界内存

操作步骤：

1. 执行：

   ```bash
   pnpm --dir web exec tsc -b --pretty false
   pnpm --dir web test:unit -- useSearchStore.test.ts useTrafficStore.test.ts boundedTrafficFilter.test.ts trafficWindow.test.ts
   ```

预期结果：

- 新命中记录晋级、更新后不再命中的记录降级、删除记录移除、retention 水位前记录移除。
- pending → completed 替换不产生重复 ID，结果按 sequence 降序。
- Search、普通窗口、Map 和筛选结果均保持 1,000 条上限。
- 现有无筛选、历史分页和筛选增量回归单元测试全部通过。

### TC-TBW-03：无条件、历史筛选、统计与实时 Search 完整矩阵

操作步骤：

1. 执行：

   ```bash
   pnpm --dir web exec playwright test tests/ui/traffic.spec.ts --grep "有界筛选扫描|Network 大历史|Traffic 统计通过|Search 在组合条件下|服务端 3000 条|服务端滚动淘汰后" --workers=1
   ```

预期结果：

- 2,300 条历史下首屏为 500、双向窗口最多 1,000，最老/最新记录均可按滚动方向找回。
- 只存在于首屏之外的历史记录仍可被筛选找到。
- Client IP、Domain 等左侧计数等于服务端完整存量，不等于当前窗口样本。
- 统计仅在变化时推送，突发流量每秒最多一帧。
- Search 同时应用 URL 关键字、protocol、status、content type、method、path、Client IP、Domain 条件；WebSocket 新增命中无需再次点击 Search 即出现，非命中不出现。
- 3,000 条真实请求触发滚动淘汰后存量和最老水位符合软边界；600+600 休眠恢复时单帧不超过 500、窗口不超过 1,000、无淘汰记录复活，Tab 切换和事件循环仍可响应。

### TC-TBW-04：实时链路三轮稳定性

操作步骤：

1. 执行：

   ```bash
   pnpm --dir web exec playwright test tests/ui/traffic.spec.ts --grep "Search 在组合条件下" --repeat-each=3 --workers=1
   ```

预期结果：

- 三轮独立后端全部通过。
- 每轮的初始命中、新增命中、非命中排除、定向组合搜索和 501 ID 拒绝结果一致。
- 无超时、重复 ID、缺失新增记录或跨轮状态污染。

### TC-TBW-05：Shell E2E 自动收集门禁

操作步骤：

1. 执行：

   ```bash
   bash scripts/ci/check-e2e-shell-ci-coverage.sh
   ```

预期结果：

- 所有 `e2e-tests/tests/test_*.sh` 都被 CI shell E2E 统一入口覆盖，没有只在本地手工执行的关键脚本。

## 清理步骤

1. Playwright 用例在 `finally` 中停止各自动态端口后端和 mock server，并删除临时目录。
2. 确认没有本任务启动的 Bifrost 或 mock server 残留。
3. 不删除 `.bifrost-ui-target` 构建缓存；它是仓库既有 UI 测试缓存，不含运行数据。

## 执行记录

2026-08-06 按 TC-TBW-01 → TC-TBW-05 顺序在 macOS 隔离环境真实执行，全部通过：

- TC-TBW-01：4 组 Rust 定向测试通过；keyword/filter/record ID 交集、500 ID/非法 ID 门禁、Account 转换和参数化 SQL 均符合预期。
- TC-TBW-02：TypeScript 构建通过；Vitest 共 45 个文件、222 个用例通过，包含实时 Search 晋级/降级/删除/水位/去重/1,000 条边界。
- TC-TBW-03：Playwright 6/6 通过（23.2s）；覆盖首屏外历史筛选、2,300 条双向窗口、完整统计/1 秒推送、实时 Search 组合矩阵、3,000 条滚动淘汰和 600+600 休眠恢复洪峰。
- TC-TBW-04：实时 Search 独立后端连续 3/3 轮通过（13.7s），单轮约 2.5–2.6s，无丢失、重复、超时或状态污染。
- TC-TBW-05：CI shell 覆盖门禁通过；发现 207 个脚本，179 个被 CI 选择，28 个明确按平台/条件跳过，无遗漏脚本。
- 所有 Playwright 隔离后端和 mock server 均由用例 `finally` 清理；未操作共享 9900 服务和系统代理。
