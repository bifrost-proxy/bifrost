# Codex / Traex / Claude Code 本地 Session Resume

## 功能模块说明

验证 Agent Chat 与 IM external runner 的统一 `/resume` 指令：按当前 Runner 列出最近
20 个本地 session（`id / title / datetime`），使用 `/resume <id>` 选择后，下一条普通
消息通过 provider 原生 resume 参数继续本地会话；同时验证飞书中的 `/resume`、
`/model`、`/effort` 返回 Card 2.0 下拉选择卡，用户选中后直接执行选择，不需要复制粘贴参数。

## 前置条件

- macOS/Linux 本机可读取当前用户的 Codex、Traex、Claude Code session 目录；任一客户端
  没有历史时允许返回空列表。
- 当前 worktree 已构建 `target/debug/bifrost`。
- 测试服务必须使用临时 `BIFROST_DATA_DIR`、动态端口、`--no-system-proxy`；禁止操作正式
  9900 服务。

## 测试用例列表

### TC-LSR-01：三 Provider 确定性列表、选择与原生 Resume

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_local_session_resume.sh
   ```

2. 观察脚本输出和退出码。

预期结果：

- 脚本输出 `[im-local-session-resume] PASS`，退出码为 0。
- Codex、Traex、Claude Code 的 `/resume` 都返回对应 fixture 的 id/title/datetime。
- `/resume <唯一前缀>` 不执行 mock runner；下一条消息才执行。
- Codex argv 包含 `resume` 和完整 id；Traex app-server 收到包含完整 `threadId` 与
  `excludeTurns: true` 的 `thread/resume`，随后收到同 thread 的 `turn/start`；Claude argv
  包含 `--resume` 和完整 id。
- Traex 使用 Codex session id 时返回 400，证明 provider 目录隔离。

### TC-LSR-02：Parser、排序、20 条上限和状态持久化

操作步骤：

1. 执行：

   ```bash
   cargo test -p bifrost-admin local_sessions --lib -- --nocapture
   ```

2. 检查测试名称与结果。

预期结果：

- 合成 22 条 Codex session 后只返回最新 20 条，标题换行被压成单行。
- Traex 与 Claude Code 从各自 home/index 读取，不互相串用。
- 完整 id、唯一前缀成功；歧义前缀、非法字符和跨 provider id 失败。
- pick 替换 `externalThreadId`、清除旧 `conversationId`，保留 model override。

### TC-LSR-03：真实本地 Session 只读 Smoke

操作步骤：

1. 记录三类 session JSONL 的路径、大小、mtime 和 SHA-256：

   ```bash
   SNAPSHOT_DIR="$(mktemp -d)"
   find "$HOME/.codex/sessions" "$HOME/.trae/cli/sessions" "$HOME/.claude/projects" \
     -type f -name '*.jsonl' -mmin +5 -print0 2>/dev/null | sort -z | \
     xargs -0 shasum -a 256 >"$SNAPSHOT_DIR/before.sha256"
   ```

2. 执行真实只读 smoke：

   ```bash
   cargo test -p bifrost-admin \
     real_local_session_stores_are_readable_without_exposing_message_bodies \
     --lib -- --ignored --nocapture
   ```

3. 再次生成 `after.sha256` 并比较：

   ```bash
   find "$HOME/.codex/sessions" "$HOME/.trae/cli/sessions" "$HOME/.claude/projects" \
     -type f -name '*.jsonl' -mmin +5 -print0 2>/dev/null | sort -z | \
     xargs -0 shasum -a 256 >"$SNAPSHOT_DIR/after.sha256"
   cmp "$SNAPSHOT_DIR/before.sha256" "$SNAPSHOT_DIR/after.sha256"
   rm -rf "$SNAPSHOT_DIR"
   ```

预期结果：

- 测试通过；每个结果 id 非空、title 无换行且有长度上限、datetime 为 RFC3339 或 unknown。
- `cmp` 退出码为 0，证明扫描未改写 provider session 文件。
- 输出和断言不打印 session 正文。

### TC-LSR-04：IM External Runner 帮助文案

操作步骤：

1. 执行：

   ```bash
   cargo test -p bifrost-admin \
     im_help_for_external_cli_runner_only_lists_supported_commands \
     --lib -- --nocapture
   ```

2. 观察帮助文案断言。

预期结果：

- Traex External Runner 帮助中出现 `/model`、`/resume` 和 `/effort`。
- `/resume` 说明包含“查看最近 20 个本地会话（含新建会话）”和“选择一个会话在下一条消息恢复”。
- 旧的帮助行 `/models` 与 `/efforts` 不再出现；带参数文本命令兼容由 TC-LSR-07 覆盖。

### TC-LSR-05：飞书三类 Slash 选择卡片与单聊点击闭环

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_feishu_slash_choice_cards.sh
   ```

2. 检查脚本退出码、最终输出和隔离服务断言。

预期结果：

- 脚本输出 `[feishu-slash-choice] PASS`，退出码为 0。
- 飞书单聊发送无参数 `/resume`、`/model`、`/effort` 后分别产生 Card 2.0 卡片，卡片正文
  只含一个 `select_static` 下拉，不再平铺按钮。
- `/resume` 卡片摘要明确提示“从下方下拉列表选择”，并包含“🆕 新建会话”入口，不再要求
  复制 `/resume <id>`。
- 下拉每个 option 的 `value` 是可反序列化的绑定 JSON 字符串；`/model`、`/effort` 下拉
  均含“恢复 Runner 默认值”对应的 `clear` option。
- 从下拉选择三个候选项后，当前单聊 session 分别写入完整本地 session id、`gpt-unit`
  model override 和 `high` reasoning effort override。
- 选择 `/model clear`、`/effort clear` option 后两个 override 被清除。

### TC-LSR-06：飞书群聊绑定与越权回调安全回归

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_feishu_slash_choice_cards.sh
   ```

2. 检查群聊 session 与拒绝回调断言。

预期结果：

- 群聊 `/model` 下拉 option 的绑定 `value` 含 `chatType=group` 和原群 `chatId`。
- 群聊选择 `/model gpt-unit` option 后只更新
  `im:<provider-id>:group:<chat-id>` 对应 session，不串到单聊 session。
- 其他用户、其他 chat、过期 callback 和伪造 `/stop now` 命令均返回 HTTP 400。
- 所有被拒绝的 callback 都不修改单聊或群聊 session 状态。

### TC-LSR-07：带参数文本命令兼容与测试环境隔离

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_feishu_slash_choice_cards.sh
   ```

2. 检查最后的文本命令恢复状态、启动参数和清理结果。

预期结果：

- 卡片 clear 后继续发送文本 `/model gpt-unit` 和 `/effort high`，原文本命令链路仍能
  恢复两个 override。
- 飞书发送 `/models`、`/efforts` 仍返回原文本目录内容，不生成 `select_static` 下拉。
- 测试只启动一个当前构建的 Bifrost，使用动态端口、临时 `BIFROST_DATA_DIR`、
  `--no-system-proxy`、托盘禁用、Sync 登录弹窗禁用和 system proxy lifecycle helper
  禁用护栏。
- 飞书发送走显式 dry-run 文件，不外呼正式飞书 API，不修改正式 9900 服务。
- 退出后脚本只按本次记录的 PID 清理服务并删除临时目录。

### TC-LSR-08：飞书 `/resume` 下拉「新建会话」清空 thread 回归

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_feishu_slash_choice_cards.sh
   ```

2. 关注脚本中先选择本地 session 再选择「🆕 新建会话」的状态变化。

预期结果：

- `/resume` 下拉首个 option 固定为 `/resume new`（“🆕 新建会话”）。
- 先选择本地 session option 后单聊 session 写入 `externalThreadId`；随后选择
  「🆕 新建会话」option 后该字段被移除（`__ABSENT__`），下一条普通消息将开全新会话。
- 越权点击（他人 / 错误 chat / 过期 / `/stop now`）在下拉回调路径下同样返回 HTTP 400。

### TC-LSR-09：Traex Legacy Thread History 恢复兼容

操作步骤：

1. 选择一个可由 `traex resume <session-id>` 正常恢复、但 app-server 默认
   `thread/resume` 会触发 `thread_turns` 唯一键冲突的旧 session。
2. 将 Traex provider home 和相关 thread-store 完整复制到临时目录；记录正式 rollout 的
   SHA-256，以及目标 thread 的 turns/items 数量和 projection state。所有后续命令仅对临时
   副本设置 `TRAE_HOME`。
3. 对临时副本发送不含 `excludeTurns` 的 app-server `thread/resume`，确认复现
   `failed to materialize legacy thread history`。
4. 使用当前构建的 Bifrost 和 Traex Runner 绑定同一临时 session，发送下一条普通消息。
5. 对比正式 rollout 的 SHA-256 和目标 thread 数据快照，并清理临时目录和测试进程。

预期结果：

- 默认 app-server resume 在隔离副本稳定复现唯一键错误，证明 fixture 命中原故障。
- Bifrost 发出的 Traex resume 含 `excludeTurns: true`，响应后继续进入 `turn/start`，任务
  不再在历史列表阶段失败。
- 仍使用原 `threadId`，Traex 能读取旧上下文并续写；不创建替代 session。
- 正式 Traex rollout SHA-256、目标 thread 的 turns/items 数量与 projection state 前后
  相同。运行中的其他 Traex session 可以正常更新共享 SQLite 文件。

## 清理步骤

- E2E trap 自动停止隔离 Bifrost 进程并删除 `.bifrost-e2e-local-resume.*`。
- 飞书选择卡片 E2E trap 自动停止隔离 Bifrost 进程并删除
  `.bifrost-e2e-feishu-choice.*`。
- 删除 TC-LSR-03 的临时 snapshot 目录。
- 确认没有测试端口监听和 mock runner 残留。

## 执行记录

- 2026-08-07，TC-LSR-01：PASS。执行隔离 E2E，输出
  `[im-local-session-resume] PASS`；三 provider list/pick/next-turn resume 与跨 provider
  拒绝全部通过。
- 2026-08-07，TC-LSR-02：PASS。`cargo test -p bifrost-admin local_sessions --lib --
  --nocapture` 结果为 5 passed、1 ignored（真实本机 smoke 在 TC-LSR-03 单独执行）。
- 2026-08-07，TC-LSR-03：PASS。对 349 个超过 5 分钟未修改的真实 session JSONL
  记录 SHA-256；ignored read-only smoke 结果 1 passed；前后 `cmp` 退出码为 0。
- 2026-08-07，TC-LSR-04：PASS。第一次执行暴露测试 fixture 同时配置 Codex 时产品默认
  选择 Codex、且 `/model` substring locator 同时匹配 `/models` 的测试缺陷；修正 fixture 为
  仅 Claude Code、locator 取精确目标后复跑，结果 1 passed（7.6s）。
- 2026-08-17，TC-LSR-05：PASS。下拉改造后复跑，隔离 Bifrost 依次生成 `/resume`、
  `/model`、`/effort` Card 2.0，正文均为单个 `select_static` 下拉；从下拉选择三个
  候选 option 后状态正确，`/model clear`、`/effort clear` option 实选后对应 override 被移除。
- 2026-08-17，TC-LSR-06：PASS。群聊 `/model gpt-unit` 下拉 option 实选只更新群级
  session；其他用户、错误 chat、过期 callback 和伪造 `/stop now` 均返回 HTTP 400，
  未篡改单聊或群聊状态。
- 2026-08-17，TC-LSR-07：PASS。clear 后发送带参数文本 `/model gpt-unit` 与
  `/effort high` 能恢复 override；`/models`、`/efforts` 仍返回不含 `select_static`
  下拉的文本目录；测试使用动态端口、临时数据目录、双启动护栏和 `--no-system-proxy`。
- 2026-08-17，TC-LSR-08：PASS。`/resume` 下拉首项为 `/resume new`；先选本地 session
  写入 `externalThreadId`，再选「🆕 新建会话」后该字段变为缺失；下拉回调路径下越权
  点击仍返回 HTTP 400。脚本输出 `[feishu-slash-choice] PASS`。
- 2026-08-17，TC-LSR-04：PASS。将失效的 Web Playwright 路径替换为实际存在的 IM
  External Runner 帮助回归；`im_help_for_external_cli_runner_only_lists_supported_commands`
  断言 `/model`、`/resume`、`/effort` 与旧复数帮助行的边界。
- 2026-08-20，TC-LSR-01：PASS。Traex 场景切换为 app-server mock；从 Chat Gateway
  完成 list → pick → next turn，捕获的 `thread/resume` 含原 `threadId` 和
  `excludeTurns: true`，随后同 thread 的 `turn/start` 完成并返回 `TRAEX_RESUME_OK`。
- 2026-08-20，TC-LSR-09：PASS。Traex 0.201.4 对目标 session 的隔离副本执行默认
  `thread/resume`，稳定返回 legacy materializer 的 `(thread_id, turn_id)` 唯一键错误；
  对同一副本添加 `excludeTurns: true` 后 `resume=ok`、`same_thread=true`、
  `turn_start=ok`、`turn_completed=true`。正式 rollout SHA-256 未变，目标 thread 的
  2 turns、29 items 与 projection state 前后相同；临时副本已删除。
