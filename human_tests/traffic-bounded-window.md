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
- 隐藏恢复或自动重连后服务端已为空、没有任何 traffic delta 时，权威零统计快照仍能清除失效窗口、选中详情和 body，普通总数与统计总数都归零。中断前未提交的 backlog 不阻止收敛；核对最多两次 500-ID 查询，并保留后续新流量、筛选条件和历史游标。

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

### TC-TBW-06：有界重连最后一包仍有更老历史

操作步骤：

1. 在隔离端口运行当前源码，保留超过 2,000 条请求；可复用 `web/tests/ui/traffic.spec.ts` 的 `startIsolatedBackend`、`startMockServer` 与 `seedTrafficBatch` fixture。不要操作共享服务。
2. 在 Chrome 打开隔离实例的 `/_bifrost/traffic`，向上浏览历史窗口，保留一个 path 筛选条件；记下窗口中间一条记录的 ID。
3. 隐藏该页面使其取消 Traffic 订阅；在另一个连接删除所记 ID，并新增至少 1,000 条流量，保留原有更老历史。
4. 恢复页面，在 DevTools Network 的 `/api/push` 消息中确认最后一个初始 delta 仍带 `has_more=true`，随后有当前 `traffic_statistics`。
5. 检查被删除的中间行已经消失、未删除的旧行仍在、筛选条件和历史浏览位置保持；向上继续翻页仍能找到更老记录。

预期结果：

- 无需等待后续新请求，被删行就能被核对移除。
- 当前 1,000 条窗口最多发出两次 500-ID 查询；不通过无条件历史扫描来核对删除。
- `has_more` 的历史分页语义保持不变，新增期间不会把用户从历史窗口拉回最新位置。
- 自动化等价路径：`pnpm --dir web test:unit -- useTrafficStore.test.ts -t "bounded reconnect"`。

### TC-TBW-07：数据库身份变化后的窗口与共享订阅恢复

操作步骤：

1. 在隔离实例生成至少 1,000 条流量，浏览 Traffic 并打开一条详情；保持 Overview/Metrics 等共享订阅。
2. 停止此测试实例，使用另一个临时数据目录在相同测试端口启动当前源码，显式保留 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`--no-system-proxy` 和 `--skip-cert-check` 护栏。分别覆盖空库，以及恢复浏览器连接前已写入 600 条请求的新库；另外覆盖新库 `server_sequence` 等于和高于旧库的情况。新旧记录须保留不同进程前缀的真实 `REQ-...-...` ID；不要删除真实用户目录。
3. 在 DevTools 确认新旧统计的 `database_epoch` 不同，当前统计 HTTP 与初始 delta/updates 的身份一致。检查前端读取当前统计、清理旧详情/窗口及保留水位并重新建立共享 Push 连接；即使新序号相等/更高，或初始空 delta 与成员核对先完成，仍须恢复游标。重复相同快照不得产生重连循环。
4. 发送新数据库的下一条请求；确认它显示在 Traffic，且订阅游标按新库 sequence 前进。再断开并恢复页面连接，确认这条记录仍在且未恢复旧库游标。
5. 额外覆盖确认响应延迟期间生成新请求、旧统计晚到、第二次断线发生于确认请求期间，以及活动筛选在新数据库最新 500 条之外仍能找到匹配记录。让重置前发出的“回到实时窗口”读取延迟到重置后返回，确认它不能恢复旧窗口或旧高序号游标。

预期结果：

- 旧窗口先原子清理；新的低序号记录不会被旧保留水位过滤或被 1,000 条旧记录裁剪掉。
- 仅在新统计读取确认身份变化后重置游标；不带身份的旧后端保留原序号回退兼容路径。读取失败保留现有窗口，并允许后续统计或轮询触发重试。
- Overview/Metrics/Values/Settings 订阅保留；旧 socket 的迟到事件不会关闭或重复重建新 socket。
- 确认响应已经包含新请求时仍能从无游标初始补推恢复；旧连接的查询响应不能覆盖新连接状态。
- 自动化等价路径：`pnpm --dir web test:unit -- useTrafficStore.test.ts pushService.test.ts`。

### TC-TBW-08：同库重启、首屏并发与旧请求隔离

1. 保留隔离实例数据目录，重启同一数据库；确认 `database_epoch` 不变，历史浏览位置、筛选条件、真实记录游标与保留水位保持。随后新增一个请求，确认其新进程前缀 ID 可见且订阅游标前进。
2. 同一数据库删除当前窗口全部 ID、保留窗口外历史；确认只移除已删行，不把删除当作数据库替换，也不重置历史位置或全历史筛选。
3. 暂停首屏 updates 后的统计 HTTP，在这两次读取之间切换隔离数据库；确认旧 HTTP 记录的身份没有被新统计覆盖。重复空首屏与已排队、尚未提交的初始 delta。
4. 延迟旧库历史、reload、普通统计、详情和正文请求，先完成新库确认后再返回旧响应；确认任何旧数据都不能回填新库状态。确认过程中插入新请求，并快速切换第三个隔离数据库，检查候选身份和连接请求不会互相确认。
5. HTTP 轮询模式下让一次确认失败，再恢复网络；检查继续轮询并恢复空游标读取。随后同库重连不得产生 reset 循环。
6. 对隔离数据库制作一致的早期 SQLite 备份，再生成更高 sequence 的请求并保留历史筛选。停止隔离实例，恢复早期备份并在原测试端口启动；分别覆盖空备份和含 600 条记录的备份。确认 UUID 保持相同，而当前 `server_sequence` 小于或等于浏览器已消费的实际记录 sequence；额外统计确认后必须清理旧游标/保留水位、重新扫描筛选、展示备份中的低序号历史和随后的新请求。重复相同快照不应循环重置。
7. 延迟同 UUID 备份恢复的确认响应，在其返回前收到更高实际记录；旧确认不得覆盖较新记录，随后新的统计确认与初始重放应自动完成恢复。普通同库重启仅丢失未使用的分配序号时，不得重置窗口或筛选。

自动化等价路径：`pnpm --dir web test:unit -- trafficDatabaseEpoch.test.ts filterEpoch.test.tsx useTrafficStore.test.ts pushService.test.ts`。真实浏览器需要分别验证亮、暗主题；状态层测试不能代替浏览器操作证据。

## 清理步骤

1. Playwright 用例在 `finally` 中停止各自动态端口后端和 mock server，并删除临时目录。
2. 确认没有本任务启动的 Bifrost 或 mock server 残留。
3. 不删除 `.bifrost-ui-target` 构建缓存；它是仓库既有 UI 测试缓存，不含运行数据。

## 执行记录

2026-10-05 同 UUID 备份恢复 review 回归：

- 在被审查基线 `2ed22f58` 新增 11 个恢复/保护回归，先复现 10 个失败；另行复现确认期间暂缓合并的备份新行未参与新鲜度判断，再修复。
- 覆盖空/非空备份、保留相同身份、下一分配序号等于已消费游标、历史窗口实际最大 sequence、未提交真实记录、仅有分配高水位的排队元数据、旧轮询/catch-up 响应、确认失败/过期、活动筛选全历史重扫和下一次重连。
- 最终定向验证 4 个文件共 127/127 通过，TypeScript、修改文件 ESLint 和差异检查通过；独立 review 复跑身份 store/挂载筛选 60/60 通过。普通同库失败分配高水位回退、真实下一条记录与筛选可见性保护用例全部保留。
- 真实 Chrome 流程继续受已确认的 `AF_UNIX EPERM` 环境限制，未执行；未绕过沙箱，也未运行 Cargo 或启动额外后端。

2026-10-05 持久化数据库身份 review 回归：

- 使用不同真实形状的 `REQ-进程前缀-计数器` ID 新增身份回归；基线先复现 12/14 失败，包括相等/更高序号、历史窗口、旧保留水位、首屏身份和迟到轮询响应。
- 最终定向验证 4 个文件共 102/102 通过，包含旧后端兼容、同库失败写入预留序号回退、历史筛选低序号新行、首屏筛选身份竞态、确认失败重试、过期请求和共享 Push 重建；TypeScript、修改文件 ESLint 与差异空白检查通过。已安装依赖未提供 Prettier，因此未执行格式化脚本。
- TC-TBW-07/08 的真实 Chrome 亮/暗主题流程仍因已确认的 `AF_UNIX EPERM` 浏览器启动限制未执行；不绕过沙箱或启动额外后端，不能把组件/store 验证记作真实浏览器通过。

2026-10-05 非空数据库重建 review 回归：

- 四个实际 store 回归在旧实现复现游标 2000 无法恢复到已含 600 条记录的新库；延迟“回到实时窗口”响应另行复现旧窗口/游标覆盖新状态。
- 修复后针对性 77/77 通过，包括当前 generation 的正常 reload、过期非零统计、确认失败重试、并发新流量和真实挂载的筛选 effect；TypeScript、修改文件 ESLint、独立 review 与差异检查通过。
- TC-TBW-07 的真实 Chrome 操作仍受下述 `AF_UNIX EPERM` 限制，未执行；组件和 store 回归不能记作真实浏览器通过。

2026-10-05 有界重连与序号重启 review 回归：

- TC-TBW-06/07 的 store/service 自动化先复现旧实现失败，再运行修复后的对应回归；包含完整旧窗口、新旧请求交错、查询失败/重试、共享订阅、下一次重连游标和迟到 socket 回调。
- 最终针对性验证：6 个文件共 75 个测试通过，包括真实挂载的 Traffic 筛选 effect、全历史筛选、窗口分页和 StrictMode 共享订阅；TypeScript、修改文件的 ESLint 与 `git diff --check` 通过。独立 review 也运行相同 75 个测试通过，并确认选取的 8 个新回归在旧运行时代码上失败。
- TC-TBW-06/07 的真实 Chrome 流程在本地未执行：本任务沿用已确认的浏览器启动 `AF_UNIX EPERM` 环境阻塞，并明确不绕过浏览器沙箱、不额外启动 Cargo 后端。不能把自动化 store/service 结果当作真实浏览器通过。

2026-10-05 空库重连回归复测：

- TC-TBW-02 的前端自动化范围通过：TypeScript 检查、51 个文件共 253 个单元测试、ESLint（0 errors，7 个既有 warnings）与 Vite 生产构建通过。使用现有 `node_modules` 中的 Node 入口执行，未重新安装依赖。
- 新增回归覆盖自动重连、共享连接重新订阅、未提交 backlog、空库重启后序号回到 1、旧统计晚到、仅有待提交记录、查询期间新流量、详情清理与两次 500-ID 查询上限；中断和乱序用例先复现失败再修复通过。
- 本次未重新执行 TC-TBW-03/04 的真实浏览器验证：当前环境存在 `AF_UNIX EPERM` 启动阻塞，不能将单元测试结果视为浏览器通过。

2026-08-06 按 TC-TBW-01 → TC-TBW-05 顺序在 macOS 隔离环境真实执行，全部通过：

- TC-TBW-01：4 组 Rust 定向测试通过；keyword/filter/record ID 交集、500 ID/非法 ID 门禁、Account 转换和参数化 SQL 均符合预期。
- TC-TBW-02：TypeScript 构建通过；Vitest 共 45 个文件、222 个用例通过，包含实时 Search 晋级/降级/删除/水位/去重/1,000 条边界。
- TC-TBW-03：Playwright 6/6 通过（23.2s）；覆盖首屏外历史筛选、2,300 条双向窗口、完整统计/1 秒推送、实时 Search 组合矩阵、3,000 条滚动淘汰和 600+600 休眠恢复洪峰。
- TC-TBW-04：实时 Search 独立后端连续 3/3 轮通过（13.7s），单轮约 2.5–2.6s，无丢失、重复、超时或状态污染。
- TC-TBW-05：CI shell 覆盖门禁通过；发现 207 个脚本，179 个被 CI 选择，28 个明确按平台/条件跳过，无遗漏脚本。
- 所有 Playwright 隔离后端和 mock server 均由用例 `finally` 清理；未操作共享 9900 服务和系统代理。
