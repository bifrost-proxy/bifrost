# 飞书群聊 Agent Session 真实场景测试

## 功能模块说明

验证飞书机器人进入群聊后持续接收并保存群消息，但默认只有 `@机器人`、`/g`、`/q` 或既有 slash 命令会触发处理；模型收到的是从上一次触发之后到本次触发为止的结构化多人对话。每个群使用独立 Session Key、上下文游标、Runner 和工作目录绑定。

## 前置条件

- macOS 或 Linux，已安装 Rust、Python 3、curl。
- 在仓库根目录执行。
- 测试使用临时 `BIFROST_DATA_DIR`、mock Feishu provider 和本地 mock Runner，不需要真实飞书凭证。

## 测试用例列表

### TC-FGS-01：普通群消息只保存、不触发模型

1. 启动临时 Bifrost。
2. 通过 `POST /_bifrost/api/im-gateway/debug/mock-inbound` 向 `chat-alpha` 注入 Alice、Bob 的两条 `chatType=group` 普通消息。
3. 等待 1 秒并检查 mock Runner 输入文件。

预期结果：Runner 输入文件不存在或为空；SQLite `im_group_messages` 中保存两条消息；非 owner 的 Bob 消息不会被拒绝。

### TC-FGS-02：艾特机器人触发结构化增量上下文

1. 在 TC-FGS-01 的消息之后注入 `@_user_1 请总结刚才的讨论`，并设置 `mentionBot=true`。
2. 等待 mock Runner 完成。
3. 读取 Runner 捕获的输入。

预期结果：只触发一次；这是该群第一次启动模型 Session，因此输入开头只包含解析后的群名称与群 ID。之后是 Alice/Bob 的两条背景对话和一条当前消息，每行均为 `<at id=飞书 open_id>显示名</at>：消息内容`；触发消息只出现一次。输入不包含 `provider_id`、Session Key、消息 ID、序号、时间戳、sender 类型、mentions 数组等内部字段。显示名不可用时使用 `<at id=飞书 open_id></at>`，不把 open_id 重复到标签正文，但 id 始终可用于后续准确 @ 原发送人。工具能力只由运行环境 Skill 提供，业务 Prompt 不指定具体工具。

### TC-FGS-03：slash 指令与单聊一致且系统指令不调用模型

1. 等待首轮任务结束，向同一群依次发送 `/cwd <仓库绝对路径>`、`/runner group-mock` 和 `/status`。
2. 检查 Runner 捕获条数。
3. 再发送普通补充消息和 `/g 继续给出行动项`。
4. 等待 Guide 对应任务结束，再发送 `/help extra`。

预期结果：三个受支持的系统命令不增加 Runner 调用，`/cwd` 和 `/runner` 只写当前群 binding；`/g` 按单聊的运行中 Guide/Queue 语义触发一次，只携带上一触发之后的有效群聊“补充消息”和本次引导消息，已处理的 `/cwd`、`/runner`、`/status` 不进入模型背景；后续输入不再重复群名称和群 ID。单聊不会识别的 `/help extra` 同样回落到模型，形成只含“谁发送了 `/help extra`”的一轮紧凑输入。

### TC-FGS-04：两次触发的上下文区间不重叠

1. 比较 TC-FGS-02 与 TC-FGS-03 捕获的两个输入。

预期结果：第二次输入不再包含第一次触发前的“发布窗口”等消息，但包含第一次触发后的新消息；无漏消息和重复区间。

### TC-FGS-05：不同群使用独立 Session

1. 向 `chat-beta` 注入一条普通消息和一条 `@机器人` 触发消息。
2. 检查第三次 Runner 输入和 SQLite `im_group_bindings`。

预期结果：第三次输入是 `chat-beta` 的首次模型输入，因此包含 Beta 群名称和群 ID，不含 `chat-alpha` 内容；数据库存在两个不同的群 Session Key 和独立游标，但这些内部 Session Key 不注入给模型。

### TC-FGS-06：重复事件幂等

1. 使用相同 `providerId + messageId` 重放群消息和触发消息。
2. 检查 `im_group_messages` 和 `im_group_turns` 唯一约束对应的记录数。

预期结果：消息只保存一份；同一触发消息只生成一个 Turn，不重复调用模型。

### TC-FGS-07：同一群内多个机器人按 Provider 独立

1. 在临时 Bifrost 管理端配置两个不同飞书应用的 Provider，并确认两个长连接都为 connected。
2. 把两个应用机器人加入同一个飞书群，先发送一条不 @ 的公共背景消息。
3. 只 @ 第一个机器人并让它复述背景，再只 @ 第二个机器人并执行不同任务。
4. 分别对两个机器人执行 `/status`，并检查 `admin/im_group_context.db` 中该 `chat_id` 的 binding。

预期结果：每次只有被 @ 且 `open_id` 匹配当前 Provider 的机器人响应；两个 Session Key 分别为 `im:<provider-a>:group:<chat-id>` 和 `im:<provider-b>:group:<chat-id>`，各自第一次启动模型时的 Prompt 均包含相同群的群 ID、群名称以及可复用的发送者 `<at>`；后续 Prompt 只保留有效背景和当前消息，不重复基本信息。上下文游标、对话历史、Runner 与工作目录互不串用；机器人重名时仍按 `open_id` 精确区分。

### TC-FGS-08：飞书卡片无顶部标题并原生引用触发消息

1. 通过本地 mock Feishu OpenAPI 调用普通回复卡片和 CardKit 进度卡片发送链路，记录每次请求的路径和请求体。
2. 检查普通回复、进度卡片首次发送和后续 Turn rollover 的卡片 JSON。
3. 在真实飞书群中 @ 机器人并执行 slash 指令，观察返回卡片的视觉样式和引用关系。
4. 确认发送回复不依赖、也不测试飞书客户端自动打开悬浮窗、话题窗或侧边回复面板；需要定位时可由用户手动点击原生引用条。

预期结果：所有 Bifrost 生成的飞书卡片都不包含根级 `header`，不显示大块彩色顶部标题；任务计划和工具记录等内部折叠面板标题仍可见。存在来源消息时，请求路径为 `/im/v1/messages/{触发 message_id}/reply`，请求体不携带 `receive_id`，飞书客户端显示轻量原生引用；不要求飞书自动打开任何悬浮或话题面板。后续 Turn 的进度卡片引用本轮最新触发消息。上线通知、定时任务等无来源消息场景可直接发送，但也没有顶部标题。

### TC-FGS-09：菜单撤销后群会话不依赖菜单能力

1. 不为飞书应用申请机器人菜单写权限，也不订阅机器人菜单事件。
2. 执行群会话真实服务脚本，依次验证普通消息入账、`@机器人`、`/g`、`/cwd`、`/runner`、`/status` 和 `/help` 路径。
3. 检查 Bifrost IM Gateway 源码与运行日志，确认没有机器人菜单 API 请求、菜单事件订阅或菜单事件键解析。

预期结果：群会话和现有 slash 命令全部正常工作；Bifrost 不请求飞书机器人菜单 API，不要求菜单权限，不处理机器人菜单事件，也不向用户展示由 Bifrost 同步的自定义菜单。

### TC-FGS-10：忙时排队保留各自回复目标且拒绝的清理不推进上下文

1. 启动群会话真实服务脚本，让 `chat-beta` 的首轮 Runner 保持执行中。
2. 依次注入一条普通背景消息、`/clear` 和 `/q 排队后的结论`。
3. 等待首轮结束并检查自动执行的排队 Turn、SQLite Turn 状态以及队列上下文单元测试。

预期结果：忙时 `/clear` 被拒绝且不会提前推进群上下文基线；排队 Turn 包含 `/clear` 前的普通背景，不把 `/clear` 注入模型，并最终标记为 `completed`。每个排队项保留自己的触发 `message_id`、发送人和群 `turn_id`，出队回复不会错误引用第一轮消息；队列满或取消排队时释放对应群 Turn。

### TC-FGS-11：图片艾特和重叠 mention 占位符

1. 构造只包含 `@机器人` 和一张图片的群消息，不提供额外文本。
2. 构造同时包含 `@_user_1` 与 `@_user_10` 的群消息，并让短占位符先出现在 mentions 数组中。
3. 执行群上下文分类与 mention 渲染测试。

预期结果：图片消息触发 Agent 图片理解，不回落到 `/help`；渲染时先替换较长占位符，`@_user_10` 不会被短占位符破坏，两个发送者都生成正确的 `<at>`。

### TC-FGS-12：群级 Runner、模型命令、工作目录和群名失败退避保持隔离

1. 为群 binding 设置与 Provider 默认值不同的 Runner 和工作目录。
2. 验证 `/model`、`/models`、`/effort` 的 Runner 解析，以及连续 Runner 请求的工作目录。
3. 模拟群名查询失败并在 60 秒退避窗口内重复准备群消息，再模拟查询成功。

预期结果：模型命令优先使用当前群选择的 Runner；每次 Runner 调用都使用群 binding 的工作目录，不能回退串用 Provider/Runner 默认目录；群名首次查询失败后相同群在退避窗口内不重复发起慢请求，成功写入群名后清除退避。

### TC-FGS-13：升级时兼容旧版 mention 字符串历史

1. 在临时数据目录写入旧版 `admin/im_gateway_events.json`，其中 `mentions` 为字符串数组 `['@_user_1']`。
2. 初始化 `ImEventStore` 并读取事件列表。
3. 检查旧历史文件仍然存在，mention 已转换为结构化对象的 `key`。

预期结果：旧历史能正常加载，`key` 保留为 `@_user_1`，其余新字段使用安全默认值；事件历史文件不会因格式升级被删除。

### TC-FGS-14：群工作目录解析 Runner 回复中的相对附件

1. 为群 binding 执行 `/cwd <群项目绝对路径>`，同时保留一个不同的 Provider 默认工作目录。
2. 让 Runner 在群项目目录生成 `report.txt`，并返回 Markdown 链接 `[报告附件](./report.txt)`。
3. 检查回复资产解析结果和飞书附件上传路径。

预期结果：相对附件解析到群 binding 的工作目录，不读取 Provider 默认目录中的同名文件；回复卡片发送后附件可正常上传。没有群工作目录时仍回退到 Provider 默认目录。

### TC-FGS-15：富文本 Post 的 At 节点保留稳定 Mention

1. 构造包含机器人和普通成员 `at` 节点的飞书 `post` 事件，并同时提供对应 `mentions[].key/open_id/name`。
2. 执行飞书事件归一化和群 Prompt mention 渲染。
3. 检查机器人 placeholder 可被分类器移除，普通成员 placeholder 可渲染为稳定 `<at id=...>`。

预期结果：归一化正文使用 `@_user_N` 稳定 placeholder，不直接保留易变显示名；机器人 mention 不污染 active request，普通成员身份不丢失。

### TC-FGS-16：Live Guide Turn 等待 Runner 最终结果

1. 创建并标记一个群 Guide Turn 为 `dispatched`，模拟外部 Runner 接受 live guide。
2. 在 Runner 尚未结束时检查 SQLite Turn 状态。
3. 分别模拟 Runner 成功与失败并执行收尾。

预期结果：控制通道返回 accepted 后 Turn 仍为 `dispatched`；Runner 成功后变为 `completed`，Runner 失败后变为 `failed`，不会提前记录成功。

### TC-FGS-17：进程重启后恢复非终态群 Turn

1. 准备并 dispatch 一个群触发 Turn，不执行 Runner，关闭并重新打开 `ImGroupContextStore` 模拟进程重启。
2. 重新投递同一飞书触发事件，并声明对应 Session 当前空闲。
3. 对比恢复前后的 Turn ID、冻结 Prompt 和状态。

预期结果：复用同一个 `prepared`/`dispatched` Turn 并重新进入执行路径；若 Session 仍繁忙则重复投递继续被抑制，已完成或失败 Turn 不会重跑。

### TC-FGS-18：群并发分发模块保持单文件上限

1. 检查 `agent_chat.rs`、并发事件模块、进度事件模块和 event loop 相关文件行数。
2. 执行格式与 clippy 检查，确认拆分后的 sibling module 可见性和调用路径。

预期结果：每个生产 Rust 文件不超过 1500 行；群 Session 并发分发不再继续堆入 `agent_chat.rs`，行为测试保持通过。

### TC-FGS-19：群聊与私聊使用独立进程且跨 Session 不等待

1. 为同一个飞书 Provider 配置会休眠 1 秒并记录 PID、开始时间和结束时间的本地 mock Runner。
2. 先在 `chat-alpha` 启动长任务；在它仍为 active 时，连续向 `chat-beta` 和 Provider owner 私聊发送触发消息。
3. 检查三个 Session 的 active 状态、mock Runner PID 和开始/结束事件顺序。
4. 在单元场景中再同时建立两个群 Session 与两个私聊 Session，并向第一个群发送普通背景、Guide、`/cwd`、Queue 和取消排队命令。
5. 模拟同一 Session 的 receiver 已关闭但 completion 尚未处理，此时再到达新消息；随后模拟旧任务延迟完成后已建立新 mailbox，处理旧 generation 完成通知。
6. 发送触发消息后正常关闭 Provider 输入通道，并等待已启动 Runner 收尾。

预期结果：两个群和私聊在第一个任务完成前都启动，三个 Runner PID 各不相同；群聊按 `provider_id + chat_id`、私聊按 `provider_id + user_id` 使用独立 mailbox、异步任务、外部进程和会话历史，任一长任务不会阻塞其他会话接收、启动或回复。同一群的后续消息仍只进入该群 Guide/Queue 通道并保持顺序；receiver 关闭前后的消息按原顺序回放，旧 generation 完成通知不能移除替代 mailbox。Provider 输入正常关闭会排空已启动任务，不会截断结果；只有 event-loop 被显式取消时才终止任务。

### TC-FGS-20：活跃 Session 重投去重并保留触发确认与审计

1. 在同一个飞书 Provider 下同时启动两个群 Session 和两个私聊 Session，使四个外部 Runner 都保持 active。
2. 向其中一个群发送 Guide、`/cwd`、`/q`、`/rq`，并以相同 `message_id`、不同 `event_id` 重投 Guide；向一个活跃私聊发送 Guide 并以相同方式重投。
3. 在同一活跃群发送一条不触发 Agent 的普通背景消息。
4. 检查群上下文账本、event history、inbound message log 和 `reaction_added`。
5. 模拟 mailbox receiver 已关闭但 completion 尚未处理，确认该消息回放到正常路径后才写入 dedup window。

预期结果：群聊和私聊的每个重投消息都只处理、入账和审计一次；关闭中 mailbox 的消息不会因过早记入 dedup 而在回放时丢失。活跃 Session 中被接受的触发消息仍尝试添加 `OK` reaction，并生成一条 inbound audit log；普通背景消息只进入群上下文，不产生触发 reaction 或 inbound trigger audit。

### TC-FGS-21：默认 Runner 的忙时 Guide 保留可恢复 Turn

1. 配置一个支持 live Guide 的默认外部 Runner，但不在 Provider Agent 配置中显式填写 `runner`。
2. 验证 busy mode 根据默认 Runner 的真实 adapter 选择 `ExternalGuide`，而不是退回仅存在于主进程内存中的旧 `Guide` 通道。
3. 构造一个已标记为 `dispatched` 的群 Guide Turn，模拟无法在当前活跃进程内消费、需要延后执行的路径。
4. 检查 SQLite Turn 状态与排队项上下文，并检查 event-loop 测试模块行数。

预期结果：隐式默认 Codex/Traex/Claude Code Runner 使用 live Guide，ChatGPT Web 使用 Queue；延后执行的群 Guide Turn 仍为 `dispatched`，排队项保留相同 `group_turn_id`，只有延后 Runner 成功或失败后才进入终态。event-loop 主测试模块与拆分出的恢复测试模块均不超过 1500 行。

### TC-FGS-22：引用消息并只艾特机器人仍是有效主输入

1. 在 `chat-alpha` 发送一条普通群消息并完成至少一次后续 Agent Turn，使该普通消息早于当前群上下文游标。
2. 在飞书中回复引用该普通消息，只发送 `@机器人`，不附加任何文字；debug 场景使用相同 `parentId` 和 `text=@_user_1` 注入。
3. 等待 mock Runner 完成并检查捕获的 Prompt、Runner 调用次数和 SQLite 消息账本。
4. 构造引用目标位于当前增量区间的场景，确认它不同时作为普通背景重复出现。
5. 构造 `parentId` 只存在于另一个群或本地账本完全不存在的场景。

预期结果：可见消息的纯引用 + @ 会创建 Agent Turn 并实际启动 Runner，不会被视为空消息或返回 `/help`；同一 Provider、同一群中的被引用消息作为“本轮主要处理对象”进入 Prompt，即使它早于当前游标也能读取，且全文只出现一次。当前用户没有附加文字时，Prompt 明确要求直接理解并回应被引用消息。跨群或缺失引用不泄露消息内容、不隐式扩大权限补拉；网关直接回复“我无法看到你引用的这条消息内容，请重新发送这条消息，或把内容补充到 @ 后面”，不启动 Runner、不让模型猜测，并把该 Turn 标记为 `completed` 以保持游标和重投幂等。

### TC-FGS-23：并发审计验证不依赖固定 Runner 时间窗口

1. 启动群会话真实服务脚本，并让包含“并发检查”的三个 mock Runner 等待显式 release 文件。
2. 确认三个 Runner 已启动后，向仍活跃的 `chat-alpha` 重投两次相同 `/status`，再发送普通背景消息 `a12`。
3. 等待 `a12` 已进入 SQLite 群消息账本后释放三个 Runner，再检查 inbound 审计日志。

预期结果：测试在任意 CI 调度速度下都先验证活跃态再释放 Runner；重复 `/status` 只有一条 inbound 审计且不添加 reaction，普通背景消息只进入群上下文、不产生 inbound trigger 审计。断言不靠增加固定 sleep 或放宽日志数量来通过。

## 执行方式

TC-FGS-01 至 TC-FGS-05 由以下真实服务脚本逐条执行：

```bash
bash e2e-tests/tests/test_feishu_group_session_context.sh
```

TC-FGS-06 的账本和 Turn 幂等边界执行：

```bash
cargo test -p bifrost-admin group_store_deduplicates_messages_and_triggers -- --nocapture
```

TC-FGS-07 使用临时数据目录启动真实管理端，由测试者绑定两个真实飞书 Provider 后在同一群逐项执行，并结合 SQLite binding 记录核验。

TC-FGS-08 的普通卡片、CardKit 进度回复路径和 Provider 根级标题兜底执行：

```bash
cargo test -p bifrost-admin feishu_message_cards_use_native_reply_paths_and_strip_root_headers -- --nocapture
```

TC-FGS-09 复用群会话真实服务脚本，并执行菜单能力残留检查：

```bash
bash e2e-tests/tests/test_feishu_group_session_context.sh
if rg -n -i 'feishu_menu|bot_menu|menu_v6|application/v7/applications/.*/ability|bf_(status|help|runner|models)' crates/bifrost-admin/src; then
  echo '发现飞书机器人菜单能力残留' >&2
  exit 1
fi
```

TC-FGS-10 至 TC-FGS-12 的排队、图片 mention、Runner/workdir 隔离和群名退避边界执行：

```bash
cargo test -p bifrost-admin queue_manager -- --nocapture
cargo test -p bifrost-admin group_context -- --nocapture
cargo test -p bifrost-admin queued_event_restores_the_triggering_reply_target -- --nocapture
cargo test -p bifrost-admin group_session_work_dir_overrides_runner_and_provider_defaults -- --nocapture
cargo test -p bifrost-admin model_commands_resolve_the_group_selected_runner_first -- --nocapture
cargo test -p bifrost-admin prepare_group_dispatch -- --nocapture
cargo test -p bifrost-admin loads_legacy_string_mentions_without_deleting_history -- --nocapture
```

TC-FGS-13 与 TC-FGS-14 的升级兼容、禁用 Agent Turn 清理和群目录相对附件执行：

```bash
cargo test -p bifrost-admin loads_legacy_string_mentions_without_deleting_history -- --nocapture
cargo test -p bifrost-admin group_event_loop_records_ambient_and_releases_turn_when_agent_is_disabled -- --nocapture
cargo test -p bifrost-admin group_reply_assets_use_the_session_work_dir_before_provider_default -- --nocapture
```

TC-FGS-15 至 TC-FGS-18 的富文本 mention、live Guide 生命周期、重启恢复和模块边界执行：

```bash
cargo test -p bifrost-admin test_normalize_feishu_post_restores_stable_mention_placeholders -- --nocapture
cargo test -p bifrost-admin live_guide_group_turns_follow_the_external_run_outcome -- --nocapture
cargo test -p bifrost-admin prepare_group_dispatch_recovers_nonterminal_turn_after_restart -- --nocapture
test "$(wc -l < crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs)" -le 1500
test "$(wc -l < crates/bifrost-admin/src/handlers/im_gateway/agent_chat_concurrent.rs)" -le 1500
test "$(wc -l < crates/bifrost-admin/src/handlers/im_gateway/agent_chat_progress.rs)" -le 1500
cargo clippy -p bifrost-admin --all-targets --all-features -- -D warnings
```

TC-FGS-19 的多群、私聊并发进程隔离与 mailbox 竞态执行：

```bash
cargo test -p bifrost-admin session_dispatch --all-features -- --nocapture
cargo test -p bifrost-admin group_event_loop_routes_concurrent_context_to_the_active_external_session --all-features -- --nocapture
bash e2e-tests/tests/test_feishu_group_session_context.sh
```

TC-FGS-20 的活跃 Session 去重、确认和审计回归执行：

```bash
cargo test -p bifrost-admin session_dispatch --all-features -- --nocapture
cargo test -p bifrost-admin group_event_loop_routes_concurrent_context_to_the_active_external_session --all-features -- --nocapture
bash e2e-tests/tests/test_feishu_group_session_context.sh
```

TC-FGS-21 的默认 Runner Guide 恢复语义和测试模块边界执行：

```bash
cargo test -p bifrost-admin busy_default_mode_guides_external_runners_except_chatgpt_web --all-features -- --nocapture
cargo test -p bifrost-admin busy_group_turns_complete_or_release_with_their_queue_outcome --all-features -- --nocapture
test "$(wc -l < crates/bifrost-admin/src/handlers/im_gateway/event_loop/tests.rs)" -le 1500
test "$(wc -l < crates/bifrost-admin/src/handlers/im_gateway/event_loop/tests/recovery_tests.rs)" -le 1500
```

TC-FGS-22 的纯引用触发、旧游标查询、区间去重与跨群隔离执行：

```bash
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin mention_only_reply_uses_quoted_message_instead_of_help --all-features -- --nocapture
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin quoted_message --all-features -- --nocapture
SKIP_BUILD=true bash e2e-tests/tests/test_feishu_group_session_context.sh
```

TC-FGS-23 的确定性并发审计时序执行：

```bash
CONCURRENT_INJECT_DELAY_SECONDS=3 SKIP_BUILD=true \
  bash e2e-tests/tests/test_feishu_group_session_context.sh
```

## 清理步骤

脚本退出时停止临时 Bifrost 进程并删除 `mktemp` 数据目录；设置 `KEEP_TEST_DIR=true` 可保留现场用于检查 SQLite 与日志。
