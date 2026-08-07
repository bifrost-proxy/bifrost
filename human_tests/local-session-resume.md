# Codex / Traex / Claude Code 本地 Session Resume

## 功能模块说明

验证 Agent Chat 与 IM external runner 的统一 `/resume` 指令：按当前 Runner 列出最近
20 个本地 session（`id / title / datetime`），使用 `/resume <id>` 选择后，下一条普通
消息通过 provider 原生 resume 参数继续本地会话。

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
- Codex/Traex argv 包含 `resume` 和完整 id；Claude argv 包含 `--resume` 和完整 id。
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

### TC-LSR-04：Web Agent Chat Slash 菜单

操作步骤：

1. 执行：

   ```bash
   pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts \
     -g "shows model slash commands for Claude Code runner" --reporter=line
   ```

2. 观察 slash 面板断言。

预期结果：

- Claude Code Runner 下输入 `/` 时出现 `/resume`。
- 说明文字包含“列出最近 20 个本地会话”。
- 原有 `/models` 与 `/model` 仍存在。

## 清理步骤

- E2E trap 自动停止隔离 Bifrost 进程并删除 `.bifrost-e2e-local-resume.*`。
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
