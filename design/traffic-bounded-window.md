# Network 有界流量窗口与历史浏览

## 背景

Network 页当前首屏加载最新 500 条后，会在 `backfillHistory()` 中持续向前分页，直到把数据库中的全部历史记录放入 WebView。数据库保留上限为数万条时，这会同时放大以下成本：

- `records`、`recordsMap`、筛选目录和派生结果长期持有全部 `TrafficSummary` 对象；
- 每个历史批次都会触发 Zustand 更新、React 筛选和虚拟列表重算；
- Toolbar/Filter Panel 条件变化会再次扫描整个历史数组；
- WebKit WebContent 发生内存压力后可能停止响应，最终只剩空白窗口。

虚拟列表只限制 DOM 行数，不限制 JavaScript 中的数据对象，因此不能解决这类问题。

本方案把“数据库可以保留多少历史”和“WebView 同时持有多少记录”拆开：数据库仍按现有配置保留完整历史，Network 页只维护有界窗口，并通过游标继续浏览；筛选按 compact 分页扫描完整历史，但只保留命中结果窗口；Search 继续使用现有服务端分页搜索。

## 用户目标验证清单

### 必须实现

- 首次进入 Network 仍快速加载最新 500 条，不自动把全部历史放入 WebView。
- 普通列表最多常驻 1,000 条；向上滚动可加载更老记录，向下滚动可继续回到更新记录。
- 筛选仍覆盖数据库完整历史，而不是只匹配当前 1,000 条窗口；筛选结果最多常驻 1,000 条，并可继续加载更老命中项。
- Network 左侧 Client IP、Proxy port、Applications、Accounts、Domains 计数来自服务端内存统计快照，不受当前 1,000 条窗口影响。
- Activity 的 Requests、应用数与 Traffic Distribution 复用同一服务端快照；实时速率、活动连接、规则与系统代理继续使用各自既有服务端来源。
- Settings / Metrics 与全局底部状态栏只展示服务端下发字段：进程生命周期请求数、连接数、速率、累计上下行、CPU/内存仍保持原有实时指标口径；`Recorded Traffic` 保持当前落库记录数口径，不能把两者混为同一个计数。
- Metrics 的 Applications / Hosts 汇总卡与列表都由服务端统计；前端不再对返回列表执行 `length` / `reduce` 形成业务统计。
- Search 保持现有服务端搜索、50 条分页和最多 1,000 条结果的有界语义。
- 实时更新在最新窗口中正常追加；用户停留在历史窗口时不强行改变可见位置，并显示新流量提示。
- 服务端达到 `traffic.max_records` 并批量淘汰旧记录后，Push 携带当前最老可用 `sequence` 水位；普通窗口和筛选结果移除水位之前的失效记录。
- 页面休眠/恢复或 WebSocket 重连时，补推流量按最多 500 条一批发送，前端待处理队列最多保留 1,000 条记录，不能因瞬时 backlog 重回无界内存。
- 所有记录容器、Map、筛选结果和临时扫描批次都有明确上限。

### 必须不破坏

- Toolbar、主筛选条件、Filter Panel（Client IP、Proxy port、App、Account、Domain）语义不变。
- 正则、否定、空值等客户端高级筛选条件继续生效。
- 点击详情、双击独立窗口、多选、清空流量、暂停/恢复和 URL 状态同步不变。
- 列表始终按 `sequence` 升序；pending → completed 更新保持原位置。
- WebSocket/SSE 实时状态更新和 hidden/reconnect catch-up 不丢失最新游标。
- hidden 恢复只依赖携带单调游标的 Push 初始 delta，不再并发启动重复 HTTP catch-up 请求放大同一 backlog。
- 明暗主题与现有高密度 Network 布局不变。

### 必须真实验证

- 单元测试验证普通窗口在向前/向后合并后不超过 1,000 条，淘汰方向正确且游标单调。
- Rust 单元测试验证内存统计在插入、应用身份更新、删除、滚动淘汰、清空与数据库重启初始化后保持准确。
- Web 单元测试验证 Network 与 Activity 使用服务端快照，而不是从当前窗口重建计数。
- 单元测试验证筛选扫描会丢弃未命中页、保留旧历史命中项，并在超过上限时正确裁剪。
- Web UI E2E 在隔离后端构造超过窗口上限的历史，验证初始常驻数量、向上加载旧记录、向下返回新记录。
- Web UI E2E 验证只存在于首屏之外的旧记录仍可被筛选和 Search 找到。
- Web UI E2E 验证大量历史存在时 Network、Rules、Settings Tab 仍能切换，实时流量仍可出现。
- Web UI E2E 将服务端 `traffic.max_records` 设为最小值 1,000，在页面隐藏期间持续写入数千条流量，验证服务端滚动淘汰、恢复补推、旧记录失效、前端窗口/队列上限、事件循环延迟和 JS heap 峰值。
- human test 在隔离端口按真实滚动和筛选步骤执行，并覆盖 light/dark。

## 产品语义

### 三类有界数据集

| 模式 | 数据来源 | 常驻上限 | 历史范围 |
| --- | --- | ---: | --- |
| 普通 Network | `/traffic/updates` + `/traffic` 双向分页 + Push | 1,000 | 通过前后游标连续浏览全部历史 |
| Network 筛选 | `/traffic` compact 分页扫描，浏览器只保留命中项 | 1,000 | 每次筛选从最新向最老扫描全部历史；之后继续加载旧命中项 |
| Search | `/search/stream` 服务端扫描与分页 | 1,000 | 保持现有全历史搜索语义 |

普通窗口和筛选窗口可以同时存在，但总量仍是常数级；扫描筛选时额外只存在一个 500 条 compact 临时批次。

### 普通窗口的双向滑动

- 首屏：最新 500 条，`hasOlder` 由响应 `has_more` 决定，`hasNewer=false`。
- 顶部接近阈值：以当前最老 `sequence` backward 拉 500 条，前插后从最新侧裁剪到 1,000 条。
- 底部接近阈值且 `hasNewer=true`：以当前窗口最新 `sequence` forward 拉 500 条，后插后从最老侧裁剪到 1,000 条。
- 每次前插或裁剪都用原可见记录 ID 做滚动锚定，避免内容跳跃。
- “New Traffic”操作回到最新 500 条并清零提示，不要求逐页追赶全部中间窗口。

`lastSequence` 表示实时订阅已经消费到的服务端游标，不能因浏览旧窗口而回退；当前显示窗口的最新游标从 `records` 边界独立计算。

### 历史窗口中的实时流量

当 `hasNewer=false` 时，Push/Delta 追加到普通窗口并从最老侧裁剪，自动滚动语义保持不变。

当 `hasNewer=true` 时：

- 已在窗口中的记录状态更新仍原位合并；
- 新记录不插入历史窗口，避免用户正在阅读的行被淘汰或跳动；
- 实时订阅游标继续前进，`newRecordsCount` 累加；
- 用户点击新流量提示时重新加载最新窗口。

### 服务端滚动保留水位

前端窗口上限与服务端 `traffic.max_records` 是两个不同边界。服务端达到清理阈值后会批量删除最老记录；只依赖 `server_total` 无法判断哪些序号已失效，尤其是在用户停留于历史窗口、记录序号存在删除缺口时。

现有存储为避免每条写入都清理，采用软上限：记录数超过配置值的 115%（额外量最多 2,000）后，批量回收到配置值的 80%，并每 100 次写入检查一次。因此测试不能把瞬时 `server_total === max_records` 当作不变量；在 `max_records=1_000` 时，确定性门禁是稳定快照不超过 1,150，且最老水位已经推进。

按条数滚动淘汰处于持续写入热路径，只执行批量 `DELETE` 并交由 SQLite 的常规 auto-checkpoint 回收 WAL；不能在每轮淘汰时持有写锁执行 `wal_checkpoint(TRUNCATE)`，否则休眠期数千条写入会被多轮 checkpoint 串行放大。按数据库字节上限清理和显式 compact 仍保留 TRUNCATE/VACUUM 语义，因为那些路径要求立即释放磁盘空间。

因此 `traffic_delta` 增加可选的 `oldest_sequence`：

- 服务端在每个增量批次和初始补推中查询当前最老可用序号；空库时为 `null`；
- 前端收到水位后，立即从普通窗口、`recordsMap`、pending 集合和筛选结果中移除 `sequence < oldest_sequence` 的记录；
- 水位只允许向前推进，乱序或旧连接发来的较小水位不能让已淘汰记录复活；
- 用户继续向旧方向滚动时，以服务端分页结果为准；游标已经落在淘汰区间时，允许直接返回当前最老可用页或封底，不做无限重试；
- 手工删除仍使用精确的 `traffic_deleted.ids`，与滚动保留水位互补。

最新窗口最终上限为 `min(1_000, server_total)`；历史窗口不插入新记录，但会按 `oldest_sequence` 清除已经被服务端淘汰的行，并保留 `hasNewer` 以便向新方向恢复。

### 休眠恢复与爆发补推

页面隐藏时全局同步会断开 Traffic Push；恢复时 WebSocket 使用单调 `lastSequence` 重新订阅，服务端初始 delta 即承担 catch-up。不得同时再发起 HTTP `catchUpUpdates()`，否则同一批历史会被 WebSocket 和 HTTP 重复反序列化、预处理和合并。

爆发链路有三层硬门禁：

1. Traffic DB 事件广播通道容量保持 1,024；消费者每次最多聚合 500 条事件，超过部分留给下一批，lagged 时也用 500 条分页恢复；
2. WebSocket 单条 `traffic_delta` 的 `inserts + updates <= 500`；连接级发送队列继续保持 64 条容量。重连时只读取最新 1,000 条服务端窗口中晚于客户端游标的记录，再拆成最多 500 条一包连续发送；这样即使游标已经落在淘汰区间，也能收敛到当前尾部而不会回放全部历史；
3. Web 端把同一帧内收到的 delta 合并到有界待处理批次：new/update 各自 ID 去重并最多 1,000 条，只调度一个 timer/RAF；提交后普通窗口和 Map 再执行 1,000 条硬裁剪。

待处理队列即使丢弃较老的中间 summary，也必须保留最新 `server_total`、`server_sequence`、`oldest_sequence` 和源记录计数；详情可按需重新查询，最终列表由最新服务端窗口收敛。

### 全历史筛选的有界扫描

筛选条件变化时，启动带 generation 的 backward 扫描：

1. 每次从 `/traffic?direction=backward&limit=500` 取一个 compact 批次；
2. 复用 `matchesTrafficFilters`，保证 Toolbar、高级条件和 Filter Panel 语义与原实现一致；
3. 未命中记录在当前循环后立即释放，只把命中项加入结果窗口；
4. 收集到 500 个首批命中项或扫描封底后提交结果；
5. 用户向上滚动时从已扫描游标继续，累计结果超过 1,000 时从较新侧裁剪；
6. 条件切换、退出页面或清空筛选后，旧 generation 的响应不再写入状态。

扫描过程中每页主动让出事件循环，确保 Tab、滚动和筛选交互可响应。这里不使用全量后台 backfill，也不为筛选复制全部历史。

### 服务端权威统计快照

`GET /_bifrost/api/traffic/statistics` 返回当前服务端保留记录的完整统计：`total_requests`、`server_sequence`，以及 `client_ips`、`proxy_ports`、`applications`、`account_names`、`domains` 计数表。

- `TrafficDbStore` 启动时从现存 SQLite 记录初始化一次内存计数器；API 请求只读取内存快照，不执行全表聚合。
- 新记录提交成功后递增计数；进程识别或账号识别更新字段时，只从旧分桶减一并向新分桶加一，不重新扫描记录。
- 单条/批量删除、按条数或字节滚动淘汰、retention 清理和 Clear all 都同步扣减或重建内存计数。
- Network 和 Activity 共用该快照；浏览器窗口合并、裁剪、历史换页不再修改统计 Map。
- 前端首次进入时可由 HTTP 接口并行获取兜底快照；WebSocket 建连或重新订阅 Traffic 时立即下发一份 `traffic_statistics` 快照。
- 运行期新增、更新、删除或清理先按受影响维度增量更新内存计数，再设置统计脏标记；服务端以 1 秒周期合并变化，只有脏标记存在且有 Traffic 订阅者时才读取内存快照并推送。因此突发流量最多每秒产生一帧统计消息，空闲期不推送，也不轮询数据库。
- WebSocket 重连会重新下发当前完整快照；瞬时断连时前端保留上一份权威数据，不退回使用 1,000 条窗口样本计算。

### Metrics 与底部状态栏的服务端指标

Metrics 页和底部状态栏包含两类刻意不同的统计口径，改造时必须分别保留：

| 字段 | 服务端来源 | 语义 |
| --- | --- | --- |
| `total_requests`、`active_connections`、QPS、速率、累计上下行、协议指标 | `MetricsCollector` 原子计数与 1 秒滑动速率窗口 | 当前进程生命周期实时指标，服务重启后重新累计 |
| `recorded_traffic` / `overview.traffic.recorded` | `TrafficDbStore` 内存统计总数 | 当前 SQLite 中实际保留的记录数，受清空、retention 和滚动淘汰影响 |
| `total_traffic_bytes`、`memory_usage_percent` | 服务端基于同一指标快照派生 | 供状态栏与 Metrics 直接展示，前端不得再次求和或相除 |

WebSocket 的 `metrics_update` 同时携带实时指标和 `recorded_traffic`。客户端订阅间隔下限保持 1 秒；服务端内部可用更细粒度 tick 合并不同客户端间隔，但同一客户端最快每秒收到一次。Metrics store 收到该帧后同时更新 `current`、`overview.metrics` 与 `overview.traffic.recorded`，确保 Settings 页不会只依赖 5 秒 overview 帧，底部状态栏也不会因为前端流量窗口裁剪而失真。

Applications / Hosts 的持久化统计在 `TrafficStatistics` 中按桶维护请求数、上下行字节和协议计数：启动时随既有统计初始化扫描构建一次，之后在记录插入、完成更新、应用归因更新、删除、清空和滚动淘汰时按旧值/新值做增减。API 读取内存快照并附带服务端汇总，不再执行请求时全表 `GROUP BY`；活动连接仍按原有 `connection_registry` 叠加，因此保留“落库历史 + 当前活动连接”的既有语义。

这套 Metrics 推送不替代 Network / Activity 的 `traffic_statistics` 变化驱动推送：后者仍只在维度统计变脏时最多每秒推送一次；实时速率和资源使用必须周期采样，所以仅在存在 Metrics 订阅者时按客户端间隔推送，空闲时不序列化或广播。

## 数据不变量

- `records.length <= 1_000`。
- `recordsMap.size <= 1_000`，且 key 集合与 `records` 完全一致。
- `pendingBatch.newRecords.length <= 1_000`、`pendingBatch.updatedRecords.length <= 1_000`；服务端单条 delta 最多 500 条。
- 普通 `records` 与筛选结果均严格按 `sequence` 升序且 ID 去重。
- 单次筛选扫描临时批次 `<= 500`，筛选结果 `<= 1_000`。
- Search 结果继续由 `max_results=1_000` 约束。
- `lastSequence` 只向前移动；历史分页不修改它。
- `serverOldestSequence` 只向前移动；所有常驻记录和筛选结果的序号不得小于该水位。
- 切换筛选 generation 后，旧请求最多完成当前页但不能提交结果。

## UI 与交互

不改变现有 Network 视觉层级、表格列、面板宽度或主题 token。虚拟表增加两类内部能力：

- 顶部阈值触发 `onLoadOlder`；
- 底部阈值触发 `onLoadNewer`；
- 数据前插时恢复原可见行锚点；
- 根节点暴露测试用 `data-loaded-count`，不新增用户可见噪音。

加载历史时保留当前列表，避免整页 Spinner 或白屏。筛选首次扫描可以使用表格内轻量 loading 状态，但 Toolbar、Tab 和左右面板始终可操作。

## 失败与恢复

- 普通历史分页失败：保留当前窗口和游标，展示现有 error，不自动无限重试；下一次滚动允许重试。
- 筛选扫描单页失败：保留已完成结果并结束本轮 loading；条件不变时用户可再次触发。
- Push 重连/窗口恢复：以单调 `lastSequence` 由 WebSocket 初始 delta catch-up，不并发重复 HTTP catch-up；若当前是历史窗口，只更新游标、新流量计数并按服务端水位删除失效旧行。
- Clear all：递增所有分页/筛选 generation，清空普通/筛选窗口及游标，避免旧响应复活已删除数据。

## 验证与资源边界

- 所有真实验证使用独立数据目录、动态端口和 `--no-system-proxy`，不得停止或重启共享 9900 服务。
- E2E 以 DOM 暴露的 loaded count 和记录 ID 边界验证窗口，不仅依赖进程 RSS 瞬时值。
- Chromium E2E 可补充 JS heap 采样作为趋势证据，但 record/map 上限才是确定性门禁。
- 服务端滚动长压测写入 3,000 条真实本地代理请求，断言全部 sequence 落库、最终存量位于 80%–115% 软边界、最老水位前移且完整链路小于 480 秒。UI 休眠恢复用 600 条初始 + 600 条 hidden 写入，既触发服务端滚动又跨越 500 单包边界；断言恢复后单批 Push `<= 500`、DOM loaded count `<= min(1_000, server_total)`、旧淘汰记录不可见、Tab 切换在 3 秒内完成、事件循环最大停顿小于 1.5 秒；若 Chromium 暴露 `usedJSHeapSize`，恢复后必须低于 512 MiB。另以 5,000 条同帧 delta 单元测试覆盖更大的前端合并洪峰。
- 业务代码改动由远端 `bash scripts/ci/coverage-all.sh --json --gate` 执行 coverage 90% 棘轮门禁。

## 与旧设计的关系

本设计替代 `traffic-refresh-bootstrap-window.md` 中“后台自动 backfill 直到数据库最老一条”和“约 20,000 条后才裁剪”的前端策略。首屏最新 500、Push/HTTP 最新窗口一致、sequence 单调、hidden/reconnect catch-up 等既有约束继续保留。
