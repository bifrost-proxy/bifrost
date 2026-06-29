# Remote Invoke v5 Proof-of-Possession 真实场景测试

## 功能模块说明

Remote Invoke v5 将调用方敏感路径从 v4 的 caller fingerprint / grant id 明文信任模型，升级为 Ed25519 Proof-of-Possession（PoP）签名、一次性 `claim_token`、短期 `grant_session_token` 和调用方公钥指纹绑定。本文档覆盖 v5 配对、授权领取、授权查询、会话刷新、调用打开、撤销、旧 v4 caller 路径拒绝、nonce 重放防护、SSE 多订阅者与日志脱敏验证。

## 前置条件

- 当前仓库分支：`feat/remote-invoke-v5-pop`。
- 每条命令执行前先运行 `source ~/.zshrc`。
- 启动真实 Bifrost 或 E2E 时设置：
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - `BIFROST_DISABLE_TRAY=1`
- 所有真实服务使用 E2E 随机端口与临时数据目录，禁止使用固定 `9900`，禁止修改系统代理。

## 测试用例列表

| 用例编号 | 用例名称 | 操作步骤 | 预期结果 | 实际结果 |
| --- | --- | --- | --- | --- |
| TC-V5-01 | v5 PoP 配对、claim、lookup、open、revoke 完整链路 | 执行 `source ~/.zshrc; cd /Users/eden_studio/work/github/bifrost; BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 cargo run -p bifrost-e2e -- --test remote_invoke --test-timeout 180 --timeout 300`。 | `remote_invoke_pop_pair_claim_lookup_open_revoke` 通过；v5 caller 使用 PoP 签名完成 pairing start、watch、grant claim、grant lookup、call open、grant revoke；pairing start 必须携带 caller public key，lookup 必须复用已冻结 caller ephemeral pub；revoke 后再次 open 被拒绝。 | PASS。2026-06-29 CI 先发现旧 fixture 未携带 `caller_pubkey` 且 lookup 旋转 ephemeral，与 v5 P0 约束不一致；修复后本地复跑通过，E2E 汇总 `1/1` passed，耗时约 684ms。 |
| TC-V5-02 | v5 后 remote shell 执行链路保持可用 | 执行 `source ~/.zshrc; cd /Users/eden/work/github/bifrost; BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 cargo run -p bifrost-e2e -- --test remote_shell --test-timeout 180 --timeout 600`。 | `remote_shell_exec_policy_guard`、`remote_shell_exec_streams_stdout`、`remote_shell_exec_unix_shell_path_fallback`、`remote_shell_policy_update_preserves_execution` 全部通过。 | PASS。2026-06-29 本地执行通过，E2E 汇总 `4/4` passed，耗时约 0.48s。 |
| TC-V5-03 | 敏感令牌不进入运行日志或 console 输出 | 执行 `source ~/.zshrc; cd /Users/eden/work/github/bifrost; export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1; export BIFROST_DISABLE_TRAY=1; LOG=/tmp/bifrost-ri-v5-pop-human-test.log; cargo run -p bifrost-e2e -- --test remote_invoke --test-timeout 180 --timeout 300 > "$LOG" 2>&1; rg -n "claim_token|grant_session_token|watch_token|relay_token|ri-pop-command|ri-pop-tag|client_auth_token" "$LOG" || true; rg -n "console\\.(log|debug|info|warn|error).*token|claim_token|grant_session_token|watch_token|relay_token" packages/bifrost-sync-server/src/remote-invoke packages/bifrost-sync-server/src/routes/remote-invoke.ts || true`。 | E2E 日志中不出现 `claim_token`、`grant_session_token`、`watch_token`、`relay_token`、测试密文或 client auth token；源码中不出现 `console.*token` 形式的敏感日志输出。 | PASS。2026-06-29 真实执行，`/tmp/bifrost-ri-v5-pop-human-test.log` 对敏感关键字无命中；源码扫描仅命中协议字段名和业务读写位置，未命中 `console.*token` 日志输出。 |
| TC-V5-04 | 旧 v4 caller 敏感路径全部返回 410 | 执行 `source ~/.zshrc; cd /Users/eden/work/github/bifrost/packages/bifrost-sync-server; pnpm exec vitest run src/__tests__/remote-invoke-security.test.ts`，重点覆盖 `returns 410 for legacy v4 caller-sensitive endpoints` 与 `rejects legacy v4 caller openCall with protocol_version_not_supported`。 | `/v4/remote-invoke/pairings/start`、`/v4/remote-invoke/pairings/:id/watch`、`/v4/remote-invoke/grants/reusable`、`DELETE /v4/remote-invoke/grants/:id`、`/v4/remote-invoke/calls/open` 均返回 410 `protocol_version_not_supported`。 | PASS。2026-06-29 定向执行通过，`1` 个 test file、`11` 个 tests passed。 |
| TC-V5-05 | PoP canonical JSON、签名、nonce 重放和 timestamp 窗口 | 执行 `source ~/.zshrc; cd /Users/eden/work/github/bifrost/packages/bifrost-sync-server; pnpm exec vitest run src/__tests__/pop.test.ts`。 | canonical JSON 稳定排序并忽略 signature；有效 Ed25519 PoP 通过；同 nonce 重放返回 `replay_detected`；篡改字段返回 `signature_invalid`；超出窗口时间戳返回 `timestamp_out_of_window`；非 Ed25519 key 返回 `invalid_caller_pubkey`。 | PASS。2026-06-29 定向执行通过，`1` 个 test file、`5` 个 tests passed。 |
| TC-V5-06 | SSE 多订阅者并发、watch_token 校验与 nonce GC | 执行 `source ~/.zshrc; cd /Users/eden/work/github/bifrost/packages/bifrost-sync-server; pnpm exec vitest run src/__tests__/sse-multi-watcher.test.ts src/__tests__/grants-claim.test.ts src/__tests__/grants-lookup.test.ts src/__tests__/grants-revoke.test.ts`。 | 同一 pairing 的多个 watcher 都收到 approved 事件；approved 事件只携带一次性 claim 信息，不泄漏 grant id 或双方 ephemeral key；错误 watch_token 被拒绝；claim、lookup、revoke 对 caller_pubkey_fp 与 grant session token 约束生效；v5 PoP 请求会清理 60 秒前的旧 nonce。 | PASS。2026-06-29 定向执行通过，`4` 个 test files、`13` 个 tests passed。 |
| TC-V5-07 | 过期 grant session 自动刷新并保持 PoP canonical 兼容 | 执行 `source ~/.zshrc; cd /Users/eden_studio/work/github/bifrost; BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_remote_invoke_v5_session_refresh_e2e.sh`。该脚本启动一个 Relay 服务、一个本地 Bifrost 调用端和一个远端 Bifrost 目标端，完成配对授权后强制把 relay DB 与本地连接缓存中的 `grant_session_token` 过期，再发起包含分解 Unicode 字符的第二次 `remote exec`。 | 第一次 `remote exec` 返回 `FIRST_OK`；过期 session 不直接用于 open call，而是复用已冻结 caller/client ephemeral pubkey 走 v5 lookup 刷新短期 session；第二次 `remote exec` 返回 `_REFRESH_OK`；PoP canonical JSON 对分解 Unicode 输入保持签名兼容；脚本清理 relay/local/target 三个临时进程和数据目录。 | PASS。2026-06-29 真实执行通过，脚本输出 `Summary: 17 passed, 0 failed`，确认 relay + local Bifrost + target Bifrost 三进程链路通过。 |

## 清理步骤

- E2E runner 使用的临时数据目录由测试框架清理。
- 如保留日志用于排查，检查完成后可删除 `/tmp/bifrost-ri-v5-pop-human-test.log`；TC-V5-07 使用的临时 relay/local/target 数据目录由脚本清理。
- 确认无测试服务残留：`source ~/.zshrc; cd /Users/eden/work/github/bifrost; ps aux | rg "bifrost-e2e|bifrost-sync-server|remote_invoke_pop" | rg -v rg || true`。
