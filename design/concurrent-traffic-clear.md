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
最终页面入口 JS `web/dist/assets/index-BkwPNBvS.js` 的 SHA-256 为
`95eb3fc894426ab9033ea125fc517e782a63e24cf5f968c52e92ee5d6f008263`。
