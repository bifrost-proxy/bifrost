# 并发清空后的抓包恢复

## 问题与修复

`TrafficDbStore::clear_with_active_ids` 按数据库锁 → recent cache 写锁顺序清空。
旧实现仍持有 cache 写锁时释放数据库锁，再通过 `compact_db` 重新取锁。
另一个 clear 或 record 可以持有数据库锁等待 cache，形成 AB-BA 死锁；
代理转发仍成功，但写入和 traffic push 停止。

修复在 cache 更新后立即释放 cache，使用原数据库连接执行 `compact_with_conn`。
清空、统计更新和压缩在同一个数据库临界区完成，所有写入仍遵循数据库 → cache 顺序。
保持现有删除谓词、活跃 ID 保留、序列高水位、database epoch 和压缩错误处理语义。
不改 TLS 依赖或系统代理行为。

前端清空状态在 API 返回前保持 pending，并拒绝重叠调用。失败返回 false、保留列表
和选择状态、显示错误，并解除 pending 以便重试。成功后更新列表；全量清空重置 push
游标并重新读数据库窗口，恢复清空后到响应返回间产生的记录。重新读窗口时保留
这次读取期间到达的 live 记录及较新的 server sequence，避免用旧快照覆盖新推送。

## 验证路由与目标

本次同时影响 Rust 后端和前端 API/状态链路，验证包括：

- 在原锁交接点检查数据库锁仍持有、cache 已释放，同时让第二个 clear 重叠执行。
- 32 个线程交错 clear 与 record，保留指定 active ID；最终 clear 后新增 5 条，
  检查事件、统计、epoch、序列和重新打开数据库后的持久化。
- 前端延迟 API、失败与重试、指定 ID 删除失败、响应期间新流量恢复。
- 真实二进制和浏览器：32 个 HTTP clear 与 8 路持续代理流量；后续 5 个请求的
  转发、API 查询、列表更新及 WebSocket push frame；按钮连点和 500 恢复。
- 受影响 crate 测试、前端单元/类型/lint、Rust fmt/clippy、coverage-changed 90%
  预检、适用仓库门禁及远端 CI。

Review/Fix/Test 第一轮重点复核锁顺序和失败恢复，第二轮重点复核持续新流量、
游标与测试隔离。每轮检查 diff 并复跑相关回归；发现问题继续修复、复测。
真实场景用例见 `human_tests/traffic-clear-concurrency.md`。

## 本次云环境验证记录

基线为 `4c056b22056c50e7dc7a75a71bfa03dd06bce45d`，使用官方 Rust 1.95.0。
初始工作区干净，在独立任务分支开发；旧代码对照放在独立 worktree。

| 检查 | 结果 |
| --- | --- |
| 旧代码确定性子进程回归 | 强制 AB-BA 顺序后 15 秒超时，符合预期失败 |
| 修复代码并发回归 | 2 项通过；含指定 ID 保留、连续写入、重开数据库、序列/epoch 与事件 |
| 前端单元测试 | 64 个文件，426 项通过 |
| 真实 Chromium UI/API/push | 开发模式和最终二进制内嵌前端各 3 项通过；32 个并发 clear、96 个持续请求、5 个后续请求以及连点/500/重试 |
| workspace 和 desktop Rust fmt | 通过 |
| workspace all-targets/all-features Clippy | 通过，使用 `-D warnings` |
| 前端强制类型检查、全量 ESLint | 通过；ESLint 有 7 个既有 warning、0 error |
| E2E capability contract、shell CI coverage guard | 通过；新增 UI 回归已注册到 critical matrix |
| 完整 coverage-changed | 3862 项通过、4 个 IM 模块失败、1 个既有 ignore，退出 101 |
| 失败套件产生的变更行覆盖率 | 官方同源 coverage-diff 算法为 100%（8/8），无未测文件；不等同于完整 preflight 通过 |
| workspace all-features 测试 | 在 admin crate 的 3 个 IM 失败处停止：3863 项通过、1 个既有 ignore；后续 crate 未执行 |
| 前端格式门禁 | 仓库未提供 Prettier；独立 Prettier 检查时新旧基线同三个文件均不合格 |
| 依赖审计 | 缺少 cargo-deny，入口返回失败；未改依赖或 rustls |
| 严格 human_tests 工具门禁 | 当前没有 Chrome DevTools MCP；真实 Chromium E2E 通过，但该 MCP 门禁未验证 |
| 远端完整 CI、全量覆盖率与 UI audit | 尚未验证；完整 UI workflow 为 `ui-e2e-full.yml`，可手动触发 |

完整 coverage-changed 的 4 个失败均在未修改的 IM 代码：

- `mock_inbound_validates_and_injects_group_message`：5 秒内事件未被消费；独立旧基线和新版本分别复跑均失败。
- `group_event_loop_busy_session_falls_back_to_agent_workdir`：仅全套插桩运行超时；非插桩 workspace 运行通过，旧基线单独运行 10.22 秒通过，修复版插桩二进制单独运行 10.23 秒通过。原因未确认，不能判定为已证实的基线失败。
- `group_event_loop_routes_concurrent_context_to_the_active_external_session`：group runner 未进入 active；旧基线单独运行也在 10.23 秒超时失败。
- `group_event_loop_uses_group_workdir_for_busy_default_and_route_paths`：busy group 事件循环超时；旧基线单独运行也在 20.27 秒超时失败。

三个失败已逐例在旧基线复现；fallback 用例未在旧基线单独运行复现。
所有这些用例所在的 IM 生产代码未变更；没有修复无关 IM 问题或弱化测试。
唯一既有 ignored 用例读取当前用户真实 CLI session stores；本任务没有启用它，
也没有新增 ignore、删除测试或弱化断言。

第一轮 Review/Fix/Test 修复了 API `success:false`、错误消息提取和测试初始化未等待的问题。
第二轮修复了同步读窗口期间 live push 被旧快照覆盖的问题，并把锁状态回归加强为
子进程中的真实死锁顺序；相关单元和 UI 用例复跑通过。最终复查再次检查
status/diff、范围、锁序、失败状态、push 游标、语义和门禁记录。

最终 `cargo build --bin bifrost --all-features` 通过；构建产物 SHA-256 为
`e7e3ff5e042edca344bfb59080011b75a891cadbc896953221b1fd0874868e73`。
本次提交未包含构建自动产生的 Cargo.lock workspace 版本同步。

远端草稿 PR 为 `https://github.com/bifrost-proxy/bifrost/pull/547`。
按父会明确的分工，本任务保持仓库既有 PAT 路径限制；远端 CI 由父会通过其会话
中的已连接 GitHub 只读工具独立核验，本任务未绕过 403 或改用其他凭据读取 CI。

## P2 复查：同步读取与成员删除重叠

对应评论：`https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4269700698`。
基线 head 为 `76835aac6e137bf9dbc183a6a0f7da43de5feae0`。评论成立：
读取得到旧快照后，删除通知中的 ID 尚不在本地窗口，旧处理器忽略它；随后 additive merge
将这个已删除快照行和旧总数重新带回。删除、另一客户端清空、保留期 floor、插入/更新
交错的四个确定性回归，在修改前全部失败。

本次只修改前端状态逻辑、相关测试和验证文档：

- 每次清空后的同步读建立临时 journal。即使本地没有相应 ID，删除也会作废该读；清空
  统计与保留期 floor 同样参与失效检测。受影响的快照重新读取，结束后释放 journal。
- 接收清空统计时，即使更晚的插入令它不适合更新全局总数，它仍能作废更早的读取。
- 重读取消旧待刷批次，但在有界 journal 中保留已接收的新插入/更新；不把零总数的
  缓存旧更新当成新的数据库记录，也不因持续的空库统计令空读永远无法结束。
- 有限 latest window 中缺少 ID 不等于删除。对隐式清理下窗口外的较早 live 候选，
  只使用完整 ID 查询确认成员；保留成员查询期间的新写入，不从不完整结果推断删除。
- 保留既有普通 append/retention 路径的查询策略；对失败后的重试标记只作用于已知隐式
  删除且仍有可见记录的情形。未修改无关 IM 或 rustls，也未新增 ignore 或弱化断言。

| 本次验证 | 结果 |
| --- | --- |
| 原四个确定性 P2 回归 | 修改前 4 个失败；修改后通过 |
| 新增 store 回归 | 14 项；覆盖删除/清空/floor、未刷批次、过期水位、再次删除、失败释放、窗口外成员、成员查询期间写入、空库收敛和零总数旧行 |
| 前端全套单元测试 | 64 个文件，440 项通过；其中 store 文件 90 项 |
| 强制 TypeScript 检查、全量 ESLint | 通过；ESLint 保留 7 个既有 warning、0 error |
| 格式与脚本检查 | 新增/修改片段使用独立 Prettier 格式化，git diff --check、bash -n、E2E capability contract 均通过；仓库全量 Prettier 基线阻塞不变 |
| 最终 Rust 1.95.0 二进制构建 | cargo build --bin bifrost --all-features 通过，内嵌本次前端 |
| 最终内嵌前端真实 Chromium UI/API/push | 4 项通过，含跨客户端删除与暂挂 GET 旧快照，并保留其后新请求；该新增用例已进入 critical matrix |
| Rust/IM/覆盖率门禁 | 本次没有生产 Rust 变更，未重复运行无关 Rust/IM/覆盖率；保留此前准确结果，远端 CI 由父会核验 |

两轮 Review/Fix/Test 及后续复查关闭了旧快照、待刷批次、过期统计、窗口外成员和空库
收敛问题。全套检测还发现过宽的错误重试标记会给普通 append/retention 增加成员查询；
已收窄条件并保留原断言，复跑完整 440 项通过。严格 Chrome DevTools MCP 门禁仍未验证，
本节的浏览器证据是实际 Chromium 自动测试，不把它声明为该 MCP 门禁通过。

## P2 复查：异步事件、统计与待刷批次顺序

本轮基线为 `da9ed88512579b79496bf69af9eebba39a467e78`，沿用干净的
`fix/concurrent-traffic-clear` worktree。对应评论：

- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4270946607`
- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4270946622`
- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4270946628`

三条评论均独立验证成立。数据库的 update 进入订阅异步任务，删除直接广播，
因此更新先提交但后送达是合法时序；事件未提供能证明更新晚于删除的行版本。
确定性测试使用受控 Promise 和 fake timer/animation frame，无并发压力碰运气。
三个核心用例在旧代码上全部失败：已删行恢复、701 总数退回 700、GET 的 702
总数被旧批次退回 701。扩展的七个用例中六个在旧代码失败，另一个保护迟到零总数
清空信号，不把它误判为应忽略的普通旧统计。

修复内容：

- 当前 reconciliation 的删除标记保持权威；迟到 insert/update 不能移除标记，
  也不会进入 UI 待刷批次。保护只作用于当前 history/resume 代际，不阻挡已经取消
  的旧 GET 尚未返回期间，新代际中恢复的行和新更新。
- journal 的总数与 sequence 一起保持水位，迟到正数统计不能覆盖更新的 metadata。
  零总数独立标记 clear，即使游标过期仍作废旧成员快照；保留期 floor 同样保持语义。
- 批次总数不再与更早的事件游标混配。GET 合并完 journal 后消费批次，取消旧 timer/RAF，
  防止旧总数在下一帧回退，包括删除不推进 allocation cursor 的相同游标场景。
- 读取前已排队的 insert/update 先进入有界 journal；消费时同步保留已选详情的摘要更新，
  不丢正文。成员查询、保留期、空读收敛及新流量保护原有断言保留。

两轮 Review/Fix/Test 后继续复查：确认三个核心失败和组合顺序；发现取消批次会丢失
详情更新，补保正文断言并修复；随后用新失败回归证明旧读的 tombstone 会污染新代际，
加入代际约束并复测。八个本轮用例涵盖三项组合、迟到 clear 与正数统计、读取前批次、
相同游标 metadata、详情、取消旧读的新代际更新。没有弱化既有断言或新增 skip/ignore。

| 本轮最终验证 | 结果 |
| --- | --- |
| 新时序回归 | 8 项通过；store 文件共 98 项 |
| 前端全套单元 | 64 文件、448 项通过 |
| 强制 TypeScript、全量 ESLint | 通过；0 error、7 个既有 warning |
| 格式与能力注册 | 修改片段使用独立 Prettier；git diff --check、shell 语法、10 项 E2E capability contract 通过；既有全量格式基线问题未改变 |
| 官方 Rust 1.95.0 最终构建与静态前端 | cargo build --offline --bin bifrost --all-features 通过；最终确认构建 2 分 50 秒 |
| 实际 Chromium、真实隔离代理/API/push | 4 项通过，48.5 秒；原 critical 用例加强了删除后的迟到 update 时序 |
| 新 head CI | 由父会只读核验；本轮未通过其他凭据读取 Actions |

浏览器在真实删除 frame 被消费之后，用真实旧行和旧 metadata 构造 update，经
Playwright WebSocket route 受控投递，并暂挂 GET 检查下一 UI 批次是否短暂恢复旧行。
该证据证明浏览器完整接收/状态/渲染路径，不声称此轮后端恰好产生同一随机队列乱序。
严格 Chrome DevTools MCP 门禁仍未验证；本轮无生产 Rust、IM、依赖或 rustls 修改，
不重复无关 Rust 单元/覆盖率。构建生成的 Cargo.lock workspace 版本同步不纳入提交。

本轮进一步校正“嵌入前端”的验证边界：`rust-embed` 在本次 debug 构建下默认从
生成的 `web/dist-gzip` 读取资源。Chromium 连到 Admin 提供的最终构建静态前端，
没有使用 Vite；验证包含真实接收、暂挂 GET、删除后受控 update、UI 批次和最终计数。
不能把 debug 二进制未变化的 SHA-256 当作前端内容证明，也未验证 release 模式的
物理资源嵌入。此前“内嵌前端”的本地证据应按这个边界理解。最终二进制 SHA-256 为
`e7e3ff5e042edca344bfb59080011b75a891cadbc896953221b1fd0874868e73`。
上一轮页面入口 JS `web/dist/assets/index-BkwPNBvS.js` 的 SHA-256 为
`95eb3fc894426ab9033ea125fc517e782a63e24cf5f968c52e92ee5d6f008263`。

## P2 复查：清空完成、有限同步与失败收尾

本轮基线为 `b3f5e3f8b3e9ed22d7abd8b006fe3be48f30066b`，任务开始时
`fix/concurrent-traffic-clear` 工作区干净，继续现有分支，无并行代码任务。
对应评论：

- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4271355414`
- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4271355423`
- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4271355431`

三项独立成立：无关删除可不断作废 GET，使 DELETE 已成功的 UI 仍等待同步；
选中删除响应对已含删除结果的总数再次扣减；失效读取取消待刷批次后，GET 或成员
查询失败会丢 journal，而 idle 连接不会重放已推进游标的写入。六个定向回归在旧
`b3f5e3f8` 上全部失败，覆盖这三项及删除广播重复扣数、选中清空保留 active 行。

本轮围绕收尾不变量重构，而非增加按钮拦截：

1. `clearTraffic` 的结果与 `clearing` 只等待 DELETE；重叠 API 请求仍拒绝，失败仍保留
   原记录并恢复按钮。成功后的窗口同步独立运行，最多三次 GET，整个读取/成员查询
   过程有五秒 deadline，不因持续删除或永不返回的 GET 锁住清空按钮。
2. 快照提交与 journal-only 提交使用同一条路径。失败、deadline、重试超限、暂停/恢复
   取消不应用旧快照，但提交当前数据库代际的有效已接收写入，保留后续字段更新和
   详情正文；被更新的清空/数据库代际 supersede 的旧读取不能恢复旧数据。
3. 只有完整成员查询的已完成 chunk 能证明缺失；失败/未完成 chunk 不推断删除。
   已确认缺失进入读取的删除标记，并跨后续重试保留。查询快照后的新写入继续由
   journal 引用变化保护。明确删除与真实 retention floor 仍过滤数据及待刷批次。
4. 删除 ID 改变成员，不从 snapshot total 再推导一次减法。选中响应与删除广播使用
   相同规则，独立请求删除后的计数；该请求替换删除前在途的统计请求，旧结果不能
   覆盖它。保留 active 的选中清空由权威 GET 恢复成员。总数不以本地保留行数补高。
5. 保留最新 metadata 供失败恢复，但读取前已有的旧 metadata 不反复作废在它之后
   发起的 GET。同一 allocation cursor 下计数不同不构成时间顺序证明，先确认窗口外
   live 候选的成员，再发起两次观察之后的计数读取，解决 idle 下的计数分歧。
6. 无快照且无数据变化的收尾不重置过滤缓存；有恢复数据时发布增量 mutation，保留
   当前历史窗口、filterVersion 与新记录计数。原 filter/epoch/pagination 断言未改。

源码复核发现 `record()` 在获得写锁前分配 sequence，因此不能把清空统计的 allocation
cursor 当成删除 floor。新增低序号晚提交回归先证明这会丢真实 post-clear 写入，再撤销
该推断；最终只使用真实 retention floor、明确删除和完整成员证据。另两个回归证明
同 cursor 的旧 metadata 可反复作废新 GET、并令正确 live 行的总数停在零，已修正。

新增十五个确定性回归涵盖有限结束、GET/成员失败、deadline、取消、未完成 chunk、
跨重试缺失证据、API/广播计数、active 保留、旧统计请求、低序号晚提交与同游标分歧。
已有成员/空库/新插入与更新保护断言保留。清空成功与同步断开后，既有清空测试明确
等待单独同步 Promise 才断言窗口内容；直接 API 结果的新回归验证它不等待同步。

### Critical UI 删除 frame 失败复核

父会提供的旧 head 失败为 run `38570618070`、job `115875338975`：critical UI
63 项通过、1 项失败，失败在等待 `traffic_deleted` frame，尚未投递迟到 update。
本容器用明确暂挂替换 WebSocket 注册的方式，移除等待注册/订阅的屏障，在旧 store
上复现同一点的 15000ms `Expected true / Received false`。该运行还确认按 ID DELETE
实际清除一条、后续数据库 GET 为 404，故本地重现不是“active 连接未清除”。

修复用例继续检查真实广播，并等待替换 socket 的 `connected` 与 `need_traffic` 订阅，
而不把 HTTP 快照就绪等同于 push 注册。保留原 frame、迟到更新及新流量断言，新增
实际清除数量与 404 断言。另一真实 Chromium 场景冻结 RAF，使写入确定性滞留批次，
在删除令 GET 重试后注入替代 GET 500，确认清空成功/按钮已恢复、有效新行不丢失；
该场景加入原 critical matrix。

真实失败 ZIP 的消费仍有明确限制：当前 cloud skill catalog 为空，Library 的文档化
`prepare_materialize` 工具可调用且验证了指定文件，返回签名下载地址而非本地文件。
默认和一次授权网络下载均报代理 `Tunnel connection failed: 403 Forbidden`，本容器
检查 ZIP 不存在，未声称已读、已校验完整性或见到当次 trace。未改代理、凭据或改走
GitHub CI 读取。已请父会直接提供清空到 frame 超时之间各 socket 创建/关闭、注册、
订阅时间线及 DELETE 响应正文。因此本地证明的是该失败机制，尚不能认定远端当次
必然是相同连接间隙。没有重复上传或创建 Library 副本。

| 本轮最终验证 | 结果 |
| --- | --- |
| 旧 head 六项定向回归 | 六项失败；核心四项涵盖三条评论 |
| 最终前端单元 | 64 文件、463 项通过；store 113 项、原 filter/epoch 10 项通过 |
| TypeScript、ESLint | 强制类型检查通过；0 error、7 个既有 warning |
| 格式、注册、脚本 | 修改片段独立 Prettier，UI 文件 check、git diff --check、shell 语法和 10 项 capability contract 通过；既有全量格式债未扩大 |
| 官方 Rust 1.95.0 最终构建 | all-features offline debug 构建通过，2 分 51 秒 |
| 完整 critical UI matrix | 最终代码 65 项通过，8.7 分钟；包含四项清空并发/失败恢复流程 |
| 严格 MCP / release 物理嵌入 | 未验证；实际 UI 使用 Admin 提供的 debug 构建静态资源，不使用 Vite |
| 最终 head CI / 当次远端 trace | CI 由父会核验；当次 trace 的 Library 下载受代理 403 阻断 |

Review/Fix/Test 不止两轮：先复现并统一提交路径，再验证失败/取消和分块查询；全套
发现过滤缓存无效重置后用生产修复保留原断言；最后核查序号分配位置和同 cursor
时间顺序，补失败回归后修正。没有新 ignore/skip、删用例或削弱门禁。当前 follow-up
不含生产 Rust、IM、依赖或 rustls 改动，未重复无关 Rust coverage/workspace 测试。

本轮 Admin 提供的页面入口 `web/dist/assets/index-CzuQD-SC.js` 的 SHA-256 为
`c7b7c1e510689ed7e87cfd45236ef05623f32bff5f60a1dbc29d7f40d85eecca`；
debug 二进制 SHA-256 与上一轮相同，不作为前端物理嵌入证明。

## P2 复查：选中删除在 journal 建立前的证据交接

基线为 `e7a5b991149e10250397c821324a72af580cc1e2`。评论
`https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4273339945`
成立：删除帧和成功响应均可在同步 journal 建立前到达，旧代码没有将删除证据
交给新读，迟到的已排队 update 可重新插入被删行。

选中 DELETE 在途期间用临时集合保存实际删除帧，并过滤这些 ID 的迟到 live 事件。
响应成功后把这些证据和本次请求 ID 交给当前读的 live tombstone 集合，随后释放
临时集合；失败时释放集合，不推断请求 ID 已删除。该集合不作为权威 GET 的删除
过滤器：服务端保留的 active 行仍可从 GET 恢复，journal 结束后接受正常新更新。
没有长期全局 tombstone，也没有改变后端或无关生产代码。

新增五项测试全部在旧生产代码上失败：删除帧在响应前到达/未先到达各配 GET 成功/
失败的四项，以及 active 行的权威恢复和后续更新。修复后全量前端 64 文件、468 项
通过（store 118 项）；强制 TypeScript 通过，ESLint 0 error、7 个既有 warning。

八条 review 在该 head 的回归对应关系如下；当时测试位于
`web/src/stores/useTrafficStore.test.ts`。后续按领域迁移见下文，没有删除断言或新增 skip。

| 评论 discussion ID | 回归覆盖 |
| --- | --- |
| 4269700698 | `does not resurrect an unseen snapshot row deleted by another client`；`does not restore snapshot or live rows after another client clears`；retention floor、post-delete inserts/updates、窗口外成员确认 |
| 4270946607 | `keeps deletion authoritative over a delayed pre-delete update`；`combines deletion, delayed updates/statistics and a newer GET without resurrection or rollback`；取消后新代际写入 |
| 4270946622 | `retains newer positive metadata when older statistics arrive during a bounded read`；迟到零清空信号；同 cursor 新计数 |
| 4270946628 | `does not let a queued batch roll back metadata committed by a newer GET`；`consumes a pre-read batch while retaining its insert and updated fields`；equal-cursor stale totals |
| 4271355414 | `finishes the successful clear and bounds reads under unrelated continuous deletions`；stalled GET deadline；retry budget |
| 4271355423 | `counts a selected deletion once when a post-delete delta total arrives before its response`；广播不重复减数；旧统计在途被新读取替换；active 保留 |
| 4271355431 | replacement GET/member 查询失败的 journal 恢复；暂停/恢复取消；完整 chunk 与未完成 chunk；低序号晚提交；已确认缺失跨重试保留 |
| 4273339945 | 四项 `keeps selected deletion tombstones before journal creation`；`restores a preserved active selected row from GET and accepts its later live update` |

完整 UI audit 使用 workflow 同一入口：
`bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner`，
Playwright 总期限 50 分钟、外层阶段期限 60 分钟。首轮结果为前端构建通过、
官方 Rust debug 构建通过（3 分 58 秒）、UI 189 通过/30 失败/4 未运行（28 分钟）。
规则、shell、自定义 runner 按此命令不执行，不能把完整 audit 声称为全绿。

重启失败的日志显示：入口预构建了 `target/debug/bifrost`，但环境分配器把目标目录
设回 `.bifrost-ui-target`；重启助手不采用 prebuilt 参数，转而冷编译并超时，引起
后续 `ECONNREFUSED` 级联失败。相关 helper 本轮未改。把真实预构建二进制准备到
测试默认目录后，重启测试 10.7 秒通过，串行组及普通 Traffic 流程恢复。没有改测试
代码、增大期限、跳过用例或弱化断言。

独立失败还包括大历史用例的 120 秒超时（trace 中没有 page.goto，前端尚未加载）
以及统计推送用例收到 0→44→50 三帧但期望两帧。不能把全部失败直接认定为
历史债；受影响文件及旧 head 对照验证的后续结果记录在本轮交付报告。

本轮 Admin 前端入口为 `web/dist/assets/index-DB5J4awL.js`，SHA-256 为
`a27584f496de1db4514875cf1f23740542ce9ad5051b75fc9e1956d98d097ecd`。
严格 DevTools MCP、真实 WebKit 运行和 release 物理嵌入未验证。
父会已独立确认基线提交的 CI run `38595495623` 成功，该结果不代表本次新提交。

## P2 复查：存活连接完成与领域拆分

本轮基线为 `237263c07b3859f2a7c9ba67291c2e915e21436e`。两条评论：

- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4275333575`
- `https://github.com/bifrost-proxy/bifrost/pull/547#discussion_r4275333566`

第一条成立。将所有请求 ID 当作已确认删除，会丢弃后台实际保留的 active 行的
合法完成事件。HTTP、SSE、CONNECT 三项受控 stale GET 回归在旧代码全部失败。
扩展六项中五项在旧代码失败，一项确认删除保护原本通过；修复后六项全部通过。

现在分别保存 confirmed deletion 与 unconfirmed selected-clear membership。实际
删除帧仍过滤迟到事件；仅请求清空的 ID 保留其 live 完成字段，暂不发布未确认行。
DELETE 在途和同步读期间收到的完成都进入 journal。GET 中的存活行或完整 ID 查询
确认其成员后合入完成字段；窗口外缺失不直接推断删除，不完整查询不作为成员证据，明确删除仍不可恢复。API 成功、有限重试、deadline 与失败恢复的外部行为保留。

原测试把没有确认删除的 status 299 更新断言为丢弃，与本条修复目标冲突；现在
明确断言保留该完成更新及后续更新。新增断言覆盖 status 201、8192 字节、543ms、
结束时间及 CONNECT 关闭标记。保留原确认删除、失败恢复和代际取消的断言。

第二条的精确规则位于 `AGENTS.md` 的“单文件行数限制”：
“禁止让单个代码文件超过 1500 行。超过此限制必须进行模块化拆分”。
main/tag `4c056b22056c50e7dc7a75a71bfa03dd06bce45d` 已有 2391 行，
本轮基线 store 为 2952 行，不能以历史超限为由继续扩张。格式化后的主 store 第一版为 1425 行，随后删除协调集中到 reconciliation 模块，最终为 1320 行。本轮 14 个 store/领域/测试 TypeScript 文件均不超过 1500 行，最长 1426 行。

| 领域文件 | 职责 |
| --- | --- |
| `trafficCaptureCoordinator.ts` | 抓包代际、读取 journal、批次和订阅协调状态 |
| `trafficReconciliation.ts` | 窗口与成员证据收尾、清空 API 协调 |
| `trafficLiveCapture.ts` | 实时 delta 接收、journal 字段保留与 UI 批次发布 |
| `trafficRecordModel.ts` | 记录转换、合并、展示字段与 mutation 构造 |
| `trafficFilters.ts` | 记录筛选与增量筛选 |
| `trafficStoreTypes.ts` | store 数据及协调器访问契约 |

主 store 保留状态装配、现有轮询/历史/详情和选择逻辑；公开 helper 导出继续从原
入口提供。提取保持原算法和调用行为，抓包相关代际统一使用同一个协调状态。
原 3573 行测试文件按既有 describe 领域拆为六个测试文件，共用 fixture，保留全部
118 个原回归；新加六项，合计 124 个 store 回归。Vitest mock 必须先于 store 加载，
共享 fixture 显式先导入。最终前端全套为 69 文件、474 项通过；强制类型和 lint
通过（0 error、7 个既有 warning），修改文件独立 Prettier 和行数检查通过。

| 评论 ID | 当前测试文件及代表回归 |
| --- | --- |
| 4269700698 | `trafficClearOverlap.test.ts`：unseen snapshot deletion、另一客户端 clear、retention floor |
| 4270946607 | `trafficDelayedDelivery.test.ts`：delayed pre-delete update、取消后新代际 |
| 4270946622 | `trafficDelayedDelivery.test.ts`：newer positive metadata、delayed zero clear |
| 4270946628 | `trafficDelayedDelivery.test.ts`：queued batch rollback、equal-cursor totals |
| 4271355414 | `trafficCompletionRecovery.test.ts`：unrelated continuous deletions、deadline、retry budget |
| 4271355423 | `trafficCompletionRecovery.test.ts`：selected deletion counted once、fresh count、active rows |
| 4271355431 | `trafficCompletionRecovery.test.ts`：GET/member failure、取消、完整 chunk、低序号晚提交 |
| 4273339945 | `trafficCompletionRecovery.test.ts`：四种 before-journal deletion 时序 |
| 4275333575 | `trafficCompletionRecovery.test.ts`：HTTP/SSE/CONNECT stale GET 完成、API 响应前完成、窗口外确认、confirmed deletion |
| 4275333566 | 生产/测试均按领域提取；124 个原/新 store 回归和 474 个前端测试验证行为；行数逐文件检查 |

当前 UI 入口调用的是清空全部；selected-ID 精确时序由 store 回归覆盖，现有清空
链路由真实 Chromium 场景及完整 audit 验证。完整 audit 使用同一 workflow 命令和
默认 `.bifrost-ui-target` 预构建目录，最终结果另记录。

此前云端 CONNECT 应用识别为空和大历史两分钟超时，仅在旧 PR head `e7a5b991`
上做过对照，不能推广为 main 已有问题；main 的 2391 行只是行数证据。
未使用用户电脑、用户数据库或系统代理；测试采用独立临时数据。没有生产 Rust、
IM、rustls、依赖或门禁放宽。新 head CI 由父会独立读取，缺少 PAT 不阻塞本地工作。

本轮完整 audit 首次尝试中，前端构建通过，Rust 链接时云盘耗尽（100%，linker
Bus error），UI 阶段未执行。保留该失败日志后，仅清理本任务旧 `target/debug/deps`
和 `incremental` 可重建缓存，保留有效二进制、源码和测试证据，恢复约 15GB 空间。
随后以同一完整入口、默认预构建目录重跑；最终 audit 结果记录于本轮交付与 PR。

对实际订阅契约的补充检查发现，待确认 ID 若提前退出 `pending_ids`，完成事件可能
无法送达。三项 HTTP/SSE/CONNECT 回归增加“GET 在途仍订阅该 ID”的断言后，在
第一版提取代码上全部失败；保留未确认订阅至明确删除或成员收尾后全部通过。
删除协调同步移到 `trafficReconciliation.ts`。全套仍为 69 文件、474 项通过，类型与
lint 再次通过。该修复前的 audit 在 112 项通过时主动停止并保留日志，不能作为最终
代码完整 audit；随后重新构建并从头运行完整套件。没有修改测试期限或删除断言。

最后的请求代际检查复现了新增字段缓冲的跨库风险：选中 DELETE 在途时数据库身份
改变，旧 collector 的 201/8192 字段可覆盖新库同 ID 的 204/16 行。新增回归在修复前
失败；现在旧请求返回只结束本次操作，不再移交字段、删除新库行或修改新订阅，
并在 finally 释放旧上下文。该回归及全部 125 个 store 测试通过；最终前端为 69
文件、475 项通过，类型与 lint 通过。语法树复核确认模型/筛选的 38 个变量初始化
与提取前等价，算法保持不变。第二轮 audit 在 131 项通过时为此修复主动停止；
保存诊断后重新在最终代码构建并完整执行，最终结果以 PR 和交付报告为准。


## 新云任务续接：失败恢复与筛选 reset

本轮从 `94838f6c4dfc754cd4b55b61fa4c850a82024498` 的干净云端 checkout 续接，
没有使用用户设备、数据库或系统代理。最初阶段修改前端状态、组件和对应回归；后续追加评审要求的 Rust store 领域拆分见末节。
远端 CI 由父会独立核验，本任务按明确分工不查询 CI、不寻找或替换凭据。

| 问题 / 评审 | 修复和确定性证据 |
| --- | --- |
| selected clear 的 GET / membership 失败丢失 active completion（补充 4275333575、4271355431） | DELETE journal 保留请求前的行；失败回退不把未确认 ID 当作已删除，保留 completion 并标记后续 membership 重试。`retains a selected active completion when snapshot/membership fails` 两项在原生产代码失败；没有新 push 时保留 active 行与 pending 订阅的两项回归也在原代码失败。已确认删除和完整 membership miss 仍具有优先权。 |
| 4276350645：DELETE 响应失败丢失 tombstone | 在释放 DELETE collector 前同步把删除证据与字段移交 reconciliation；`retains confirmed tombstones after a failed DELETE response` 在原生产代码发生实际行复活，修复后保持删除。 |
| 4276350665：过期 full clear 擦除新数据库 | 对 full / selected clear 统一捕获并检查发起时的 history generation；跨库后不执行旧清理或订阅编辑。扩展既有跨库回归为两种 clear，原 full-clear 分支失败。 |
| 4276350672：非空 reset 未更新 active filters | store 通知同步保留 reset 版本并触发服务端筛选重扫，取消过期扫描。已完成扫描、在途扫描两项在原代码失败；第二轮发现 reset 紧接增量可被同一 React render 合并，另补先失败再通过的专门回归。普通增量不改变该 reset 版本。 |

原有删除时序测试补齐实际 `traffic_deleted` 证据；统计请求时序测试提供完整 ID
membership miss。成功 DELETE 本身不再被错误当作 active 行已删除的证据。
所有删除、字段、成员与总数断言均保留，没有删除用例、ignore 或放宽期限。

本轮第一轮 Review/Fix/Test 覆盖四类缺陷与失败回退；第二轮发现并修复 React
合并渲染遗漏 reset，第三轮复查同步 store 订阅的释放、普通增量路径、跨库代际、
confirmed tombstone 与未完成成员查询的边界。针对 store / Traffic 组件的 46 项通过。
同一测试组在隔离副本中使用原生产代码执行：9 失败、37 通过；合并渲染新增用例
单独在第一版修复上失败，最终通过。测试副本不参与产品构建。

本轮完整验证结果和剩余门禁以本任务最终交付记录为准，不把旧运行结果作为新代码
已通过的证据，也不把此前 UI 统计帧数不稳定直接归类为既有问题。

### 统计节流测试与 UI fixture 收尾

同一最终二进制上，原统计测试连续三次因“总帧数恰好增加一帧”失败。
50 路真实请求分别准备了约 2163、1941、1976ms；同一连接的总数依次为
`0→28→50`、`0→24→50`、`0→32→50`，跨越多个一秒统计节拍。
补充记录订阅后也确认计数帧均在同一连接；失败清理阶段的重连不产生这些计数帧。
不能把多秒输入等价为单节拍突发，也没有据此宣称 main 既有失败或修改生产节流。

按补充授权，将该用例改为验证每对连续统计帧的节流间隔：沿用原 900ms
浏览器投递容差，但应用到每一对帧，并断言连接一致。API 和 push 最终总数由
“至少 50”收紧到精确 50，保留前后各 1100ms 空闲不增帧、客户端计数展示断言。
每次执行均附加 seed 起止时间、逐帧 receivedAt / 总数 / 连接 ID，以及连接和
订阅事件。修改后连续三次通过；准备分别耗时 1968、1945、1991ms，帧间隔分别为
3024.8/1924.5、2927.4/1897.1、2514.8/1100.1 ms。
这些运行没有显示生产统计超频；原失败是测试对输入持续时间的假定错误。

原 traffic.spec.ts 为 2761 行。为满足 1500 行限制，将共享 fixture、基础场景、
窗口/统计和流式场景拆开；机械拆分时全部 33 个测试正文逐字保留，完成格式整理
后 32/33 个测试语法树与原版一致，唯一行为调整是上述统计契约。完整 Playwright
仍收集 223 项，critical 入口的基础用例名称与文件不变。拆分模块均少于 1000 行。

同步修正隔离后端清理顺序：等待 child close、排空输出后再结束日志流，并清理
强制停止 timer。原来的“先 logStream.end、后停止 child”在失败时产生
ERR_STREAM_WRITE_AFTER_END 并遗留隔离进程；统计三次复测未再出现该错误。
这只改变测试 fixture 生命周期，没有生产 Rust、IM、rustls 或依赖变更。
新增 UI 模块的严格 TypeScript、ESLint 和格式检查通过。

正式 critical UI 为 65/65 通过，包含四项并发清空真实链路。
完整单测与完整 UI 的最终结果以本任务交付记录为准，未通过项不得归为已完成。

### 追加评审 4277858363 / 4277858368

`discussion_r4277858363` 成立：PR 修改的 Rust store 文件为 3641 行，超过仓库 1500 行限制。
保留 store 根部的类型、连接池、初始化及 metadata 协调，按写入、读取、清理领域提取
impl 方法；测试按 database/schema/reads/statistics/retention/read_pool 分组。根文件
466 行，最大领域文件约 626 行。66 个方法体在忽略格式后相同，40 个内联测试名
无遗漏，外部 clear concurrency 回归模块不变。仅将移入子模块的原私有项恢复到
父 store 模块可见，并调整一处测试 schema 常量路径；锁顺序、事务、SQL、cache、
active-ID 保留、epoch、序列及所有公开接口保持不变。首轮编译发现的清理常量可见性
已修正，Rust 1.95.0 的受影响 traffic_db 回归 54/54 通过。

`discussion_r4277858368` 成立：reconciliation 的已确认 membership miss 以及 retention
floor 可移除列表行，却保留已选详情及正文。现在只有确认删除/成员缺失、或权威 floor
证据才清除 selectedId、详情、四类正文和 loading；延迟 detail/body 响应由既有 ID
校验拒绝。仅被 1000 行有界窗口裁剪、仍可能存在于数据库的详情继续保留。
另补复查发现的 no-op 路径：列表已空且替代 GET 失败时，也必须提交 pending-ID
变化并更新订阅，不能继续订阅已确认不存在的 ID。

新增 trafficDetailReconciliation 回归覆盖成员缺失、后续读取失败、迟到详情响应、
retention floor、有界窗口保留，以及空列表 pending 清退。前五项在旧独立 worktree
为 4 失败/1 通过；pending 清退在首版详情修复上另复现失败后通过。与原清空和筛选
回归合计 52 项通过。类型、相关 ESLint、格式检查与最新前端生产构建通过。

追加改动使本轮验证路由升级为 Rust + 前端；必须重新执行适用 Rust fmt/clippy、
coverage-changed、真实构建和最终 UI，不沿用此前 SHA 的门禁结果。此前完整 UI
在处理新增评审时主动停止，实际报告为 64 通过/159 未运行，不是完整通过证据。

### 追加评审 4277948726 与 Windows 读压力

`discussion_r4277948726` 成立：full-clear 没有 selected-clear evidence，DELETE 已提交但
响应丢失时原 catch 不启动恢复。现在相同数据库 generation 下以强制 membership
的 journal 读取服务端实际状态，并重置推送 cursor；仍返回原失败，不先假定删除成功。
暂停/断开推送也能恢复，未提交的 DELETE 保留原行，恢复中的新写入保留；独立选中、
位于列表窗口外的详情也加入 membership。新增 5 项回归；旧代码前四项为 2 失败/2 通过，
修复后前四项及详情 11 项全部通过，第五项另确认窗口外详情及正文清退。

父会提供 Windows CI 的 reader=0 / written=1 失败。旧测试用 50ms sleep 猜测写入已提交，
存在全部读取先于首个提交结束的合法调度。新增 channel 控制的反例确定性复现该次序；
压力测试等待首条实际持久化后开始读，写线程直到读完才退出，保留原 8x20 次压力读取，
并将要求加强为全部 160 次成功。Linux 本地和 Windows CI 结果分别报告，不能相互替代。

CONNECT client_app 独立探针使用相同 netstat2 0.11.2：建立并保持本地 TCP 连接后，
/proc/net/tcp 显示已建立两端，但 get_sockets_info(IPV4|IPV6,TCP) 返回 0 条。
证据位于云端 /tmp/bifrost-netstat-probe.log，属于该云端 socket 枚举能力限制；
原 UI 应用识别断言保留，不混入进程识别改造。历史 2300 条准备超时仍须最终复测。

第二轮详情检查补充：当前选中 ID 的独立 detail GET 尚未返回、列表也没有该 ID 时，
完整 replacement snapshot 同样能证明缺失。新增合法选中状态的测试在首版条件下
1 失败/11 通过；将读取开始时的 selectedId 纳入证据后，迟到 GET 不能恢复旧详情。
最终协调三模块 50/50、filterEpoch 13/13，共 63 项通过。最终冻结前端生产构建通过。
并行 Rust 编译曾触及 16 GiB cgroup 上限（oom_kill=2，SIGKILL），该轮不是断言结果；
后续按 jobs=1 顺序重跑，最终结果另记，不能把已开始执行当作通过。

最终无 Rust 编译竞争的前端全套：71 文件、501/501，--maxWorkers=1，158.66s；
此前三个 ASR 超时均通过，未修改 ASR 代码、断言或超时。最终前端生产构建 exit 0，
最终 cargo build --bin bifrost --all-features 单编译 worker 构建 exit 0（4m00s）。
完整 UI 随后使用该新二进制执行，结果必须以最终报告为准。

### 16 条评审的当前证据索引

下表供逐条回复引用，不代表线程已经 resolve，也不代替最终 Rust/完整 UI/CI 门禁。
前端相关项包含在最终 501 项完整单测中；Rust 拆分及压力测试的最终检查单独报告。

| discussion ID | 当前代码/确定性回归证据 |
| --- | --- |
| 4269700698 | `trafficMembership.test.ts`：跨客户端删除、隐式清空、floor、有界窗口外 membership 与更新时序 |
| 4270946607 | `trafficDelayedDelivery.test.ts`：删除优先于迟到更新；取消后新代际不受旧 tombstone 污染 |
| 4270946622 | `trafficDelayedDelivery.test.ts`：较新正数 metadata 与迟到零清空信号分别协调 |
| 4270946628 | `trafficDelayedDelivery.test.ts`：GET 消费旧批次，包括 equal-cursor，防总数回退 |
| 4271355414 | `trafficCompletionRecovery.test.ts`：持续无关删除、deadline、重试预算，确保读取有界结束 |
| 4271355423 | `trafficCompletionRecovery.test.ts`：selected 删除计数只应用一次，保留 active 行及最新计数 |
| 4271355431 | `trafficCompletionRecovery.test.ts`：GET/member 失败保留 journal、完整 chunk 证据及后续重试 |
| 4273339945 | `trafficCompletionRecovery.test.ts`：DELETE collector 创建 reconciliation 前的四种已确认删除时序 |
| 4275333575 | `trafficCompletionRecovery.test.ts`：HTTP/SSE/CONNECT completion，窗口外 active 与读取失败期间字段保护 |
| 4275333566 | 生产/测试按领域提取；全部 36 个改动代码模块均不超过 1500 行；原断言保留 |
| 4276350645 | `retains confirmed tombstones after a failed DELETE response`：失败返回前交接 tombstone，迟到更新不复活 |
| 4276350665 | `does not apply an old clear response to a replacement database`：full 和 selected 两个分支 |
| 4276350672 | `filterEpoch.test.tsx`：非空 reset、在途重扫、同 render 的 reset+增量；13/13 |
| 4277858363 | `traffic_db/store/`：根 466 行；66 个方法体保持；writes/reads/maintenance 与测试领域拆分 |
| 4277858368 | `trafficDetailReconciliation.test.ts`：12 项，确认删除清详情/四类正文，拒绝迟到 GET，保留有界窗口裁剪及较新选择 |
| 4277948726 | `trafficAmbiguousClear.test.ts`：5 项，full DELETE 响应丢失后暂停/断开恢复、未提交保留、新写入、窗口外选中详情 |

Windows 压力测试另见 `traffic_db/store/tests/read_pool.rs`：确定性首次提交前空读反例，
以及等待真实首提交后全部 160 次读取成功的原压力测试。Windows 结果由父会 CI 确认。

### 最终完整 UI 与历史准备对照

冻结代码、新二进制（SHA-256 `308acc50ca9726a04746b375b0cb3bdb523d5a5f14c6898e2194942fbd34a985`）
默认配置完整 223 项：220 通过、2 失败、1 未运行，38.3 分钟，exit 1。
按 `run-ui-critical.sh` 相同文件集合和 grep 从该报告精确提取的 65 项均通过，
包含四项并发清空真实链路。统计测试同连接 0→28→50，50 条准备耗时 1992ms，
帧间隔 2796.9/1989.3ms，空闲段及最终精确计数断言通过。

两项失败及实际阶段：

- CONNECT `client_app=null`，15 秒轮询失败；第三方相同 netstat2 的独立 TCP 探针为
  0 sockets，/proc/net/tcp 则显示真实已建立连接。没有改进程识别生产代码。
- 2300 条历史在 `seedTrafficBatch` 准备阶段 120 秒超时，落库 1833 条；平均已记录
  转发耗时 1.44ms，最大 25ms。页面 `goto`、1000 行窗口与统计断言尚未执行。
  03:02:10 为 683 条，03:03:18 为 1648 条。未改用例断言或 timeout。
- 1 个未运行项是 CONNECT 失败后同一串行组的 backlog 用例，不能记为通过。

为区分独立运行与配置变化，追加同一最终代码的两次单项对照：只使用项目已有
`BIFROST_PROCESS_RESOLUTION_CONCURRENCY=40` 时原历史用例完整通过（1.6 分钟）；
恢复默认配置独立运行再次 120 秒超时，最终 1913/2300。默认解析槽为 4，云端
socket 枚举缺失导致重试，这些证据支持准备阶段受解析等待影响，但不是完整根因
证明，也不能把默认完整 UI 或基线状态改记为通过。没有改动无关 resolver 或弱化断言。

报告分别归档于 `.e2e-reports/cloud-clear/full-final-default/`、`history-concurrency40/`、
`history-default-control/`，包含 JSON、trace、截图、视频和只读采样；超时残留后端已清理。

### 最终 Rust 与提交前门禁

- workspace/desktop fmt、强制 TypeScript、全量 ESLint 均通过；ESLint 0 error、7 条原有 warning。
- 最终 workspace all-targets/all-features Clippy 使用 `-D warnings`，exit 0，10m18s。
- 正常 workspace all-features `--no-fail-fast` 全部目标执行完成，exit 101。按每个 Cargo
  目标的最后主 harness 汇总（排除嵌套子进程重复输出）为 10495 通过、6 失败、9 项既有
  ignore；没有新增 ignore。三项 admin IM 超时、两项 CLI daemon 停止时只读 shell 代理
  配置清理失败、一项 external_runner_worker_stop 脚本失败（stdout/stderr 为空，未归因）。
- 正常 admin 为 3864 通过、3 项 IM 失败、1 项既有 ignore；受影响 `traffic_db` 55/55，
  包含首提交前零读反例与首提交后全部 160 次成功读取。Windows 仍需跨平台 CI 复验。
- 不带 test filter 的默认 `make coverage-changed` 已完整执行：插桩 admin 同样
  3864 通过、上述同三项 IM 失败、1 项既有 ignore，419.74s；cargo 退出 101，make 退出 2。
  因此完整 preflight 未通过，没有以忽略失败的参数重跑或修改阈值。
- 从本轮实际 profile 单独生成 LCOV，再运行官方 `coverage-diff.py --worktree --threshold 90`
  得到 changed-lines **100%（17/17）**、无 unmeasured file。官方算法自动识别并排除
  1619 行未变更的搬迁代码；root 2/2，maintenance 15/15。该结果仅证明变更行门禁，
  不把含三项 IM 失败的完整 preflight 改记为通过。

日志、同源 JSON 和完整主 harness 汇总保存在 `.e2e-reports/cloud-clear/final-review-evidence/`。
保持 IM、CLI、rustls 与依赖代码不变。按父会要求，在保存当前修复到同一 PR 后，再以
已获取的 main `4c056b22056c50e7dc7a75a71bfa03dd06bce45d` 做同环境 CONNECT/默认历史
原用例对照；结果由 PR 最终检查点及归档报告补充，不盲目 resolve 评审线程。
