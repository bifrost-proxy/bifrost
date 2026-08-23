# 飞书 Bot 指令菜单真实场景测试

## 功能模块说明

验证 Bifrost 将飞书 Bot 两级菜单作为现有 IM slash command 的输入入口，并覆盖菜单预览、Application v7 同步与发布、幂等、历史 Provider 启动恢复、私聊回复、Runner/Fast 选择卡、未知事件拒绝和 Owner 权限边界。

## 前置条件

1. 在仓库根目录构建 debug 二进制：`SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost`。
2. 自动场景使用动态端口、隔离临时数据目录与本地 fake Feishu OpenAPI，不访问生产飞书应用。
3. 启动 Bifrost 时设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并携带 `--no-system-proxy`。
4. 自动场景只写入 fake Feishu；若补充真实平台可见性验收，必须事先获得对应应用和正式服务重启授权。

## 测试用例

### TC-FBM-01：菜单预览、同步、发布与幂等

操作步骤：

1. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_bot_menu.sh`。
2. 脚本通过 CLI 依次执行 `menu preview`、draft `menu sync`、`menu sync --publish` 和第二次相同 publish。
3. 检查 fake Feishu 捕获的 token、ability、config 和 publish 请求。

预期结果：

- Preview 包含“会话 / Agent / 工具”三个一级菜单和十个白名单动作。
- ability PATCH 只更新 `bot_menu_enable` 与 `bot_menus`，config PATCH 只增加 `application.bot.menu_v6`。
- 导入应用的普通 sync 不发布；显式 `--publish` 才发布，PC/移动端默认能力均为 `bot`。
- 第二次相同 publish 返回 `skipped=true`，不产生远端请求。

### TC-FBM-02：菜单事件复用现有命令和 P2P 选择卡

操作步骤：

1. 继续执行 TC-FBM-01 的同一脚本。
2. 脚本通过 `/debug/mock-feishu-menu` 注入 `bifrost.status`、`bifrost.runner.select` 和 `bifrost.fast.manage`。
3. 检查 Feishu dry-run 卡片记录。

预期结果：

- 三个事件分别规范化为 `/status`、`/runner` 和 `/fast status`，进入现有事件循环。
- 状态卡、Runner 选择卡和 Fast 开关选择卡都发送给操作者 `open_id`。
- 菜单事件没有初始 `chat_id`，卡片绑定保持 P2P 且不猜测群聊。
- Runner 卡继续执行 `/runner <id>`，Fast 卡继续执行 `/fast on|off`，没有独立菜单业务处理器。

### TC-FBM-03：未知事件和非 Owner 安全边界

操作步骤：

1. 继续执行 TC-FBM-01 的同一脚本。
2. 注入未知 `event_key=unknown.external.command`。
3. 以 `ou_intruder` 注入合法 `bifrost.help`，查询 provider 消息日志。

预期结果：

- 未知 event key 返回 HTTP 400，不会成为任意 slash command。
- 非 Owner 的合法菜单点击可以进入统一事件入口，但被现有 Owner 校验拒绝；消息日志状态为 `rejected`，内容为 `/help`，且不产生发卡记录。

### TC-FBM-04：历史 Provider 启动恢复与普通重连边界

操作步骤：

1. 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_bot_menu.sh`。
2. 脚本先把一个已启用的历史 Feishu provider 持久化到隔离数据目录，再停止并重新启动 Bifrost。
3. 检查 fake Feishu 请求，确认服务启动恢复时自动 PATCH ability/config，但不 publish。
4. 再次停止并启动 Bifrost，确认相同 desired digest 不产生第二次 Application v7 写请求。
5. 检查 provider transport 状态，并检查重连 supervisor 不包含菜单 reconcile 调用。

预期结果：

- 历史上已经启用、已建联的 Feishu provider 在服务启动恢复时自动执行一次 draft reconcile，不要求用户手动点击重新连接。
- 第一次启动恢复只更新菜单 ability 与 `application.bot.menu_v6` 订阅，不自动发布历史导入应用。
- desired digest 相同时，后续服务启动恢复跳过远端写入。
- 普通 WebSocket 断线重连只恢复 transport，不 PATCH、不 publish，菜单同步失败也不阻断连接。

## 清理步骤

1. 自动脚本按 PID 停止本次 Bifrost 与 fake Feishu 进程，并删除隔离临时目录。
2. TC-FBM-04 完成后删除专用测试 provider；是否撤销测试应用版本由测试应用负责人决定。
3. 确认正式端口 9900、生产数据目录和当前生产飞书 Bot 均未被修改。

## 执行记录

- 2026-08-23：TC-FBM-01 至 TC-FBM-03 PASS — 使用最新 debug 二进制执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_bot_menu.sh`，输出 `[feishu-bot-menu] PASS`。Fake Feishu 捕获 1 次 token、1 次 ability PATCH、1 次 config PATCH、1 次 publish；第二次 publish 被 digest 幂等跳过。真实事件规范化链路产生状态、Runner 与 Fast P2P 卡，未知 key 返回 400，非 Owner 事件记为 `rejected`。脚本使用随机端口、隔离数据目录、代理 fail-closed、双启动护栏和 PID 定向清理，未触碰正式端口 9900 或生产数据。
- 2026-08-24：TC-FBM-04 PASS — 在隔离数据目录预置已启用的历史 Feishu provider 后重启测试服务，首次启动恢复捕获到 ability/config 各一次 PATCH 且无 publish；再次重启被 digest 幂等跳过，没有新增 Application v7 写请求。Provider 仍进入 connecting/connected/reconnecting 之一，证明菜单同步未阻断 transport；定向单元测试同时约束普通 WebSocket reconnect supervisor 不调用 reconcile。
- 真实飞书客户端菜单可见性仍需在明确授权的应用上验收；历史导入应用的启动恢复只更新 draft，平台要求发布时仍需显式执行 `menu sync --publish`。
