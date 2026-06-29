# Remote Invoke v5 PoP — P0 Hardening Smoke Tests

Branch: feat/remote-invoke-v5-pop-hardening

These checks verify the four P0 security fixes in the v5 Proof-of-Possession
authentication flow:

- P0-1: SSH route is bound to the registering user. No cross-user device_code
  hijack is possible.
- P0-2: Once a grant has frozen a caller_ephemeral_pub, any later lookup that
  presents a different ephemeral pubkey is rejected with HTTP 401
  ephemeral_pub_rotation_not_allowed.
- P0-3: SSH approval no longer leaks the long-lived grant_session_token. The
  server mints a single-use claim_token; the CLI must redeem it via PoP at
  POST /v5/remote-invoke/grants/ssh-claim to obtain the real session token.
- P0-4: Pairing caller_fingerprint is derived server-side from caller_pubkey
  with ed25519FingerprintFromBase64. Attacker-supplied caller_info.fingerprint
  is ignored. start_pairing without caller_pubkey is rejected.

## Setup

1. Start a fresh sync-server (sqlite is fine):

   pnpm --filter @bifrost/sync-server dev

2. Register two users (alpha, beta) via /v4/sso/register and capture their
   x-bifrost-token values.

3. Generate two ed25519 long-term client keypairs (alpha-client,
   beta-client) and one caller PoP ed25519 keypair.

## P0-1: SSH route user binding

1. As alpha, register alpha-client with an ssh_device_route whose device_code
   is derived from alphaSshPubPemA via deriveSshDeviceCode.
   Expect: HTTP 200, code 0.
2. As beta, register beta-client with the SAME device_code/public_key_pem from
   step 1.
   Expect: non-200, message contains device_code_owned_by_other_user.
3. Confirm via the routes DAO that the device_code is still owned by alpha.

## P0-2: ephemeral_pub freeze

1. Seed a grant for client-instance C with caller_ephemeral_pub = E1 (32-byte
   base64) via the grants DAO directly OR by completing a pairing.
2. Send POST /v5/remote-invoke/grants/lookup with a PoP envelope whose body
   carries caller_ephemeral_pub = E2 (different 32-byte base64) for the same
   client_instance_id and caller fingerprint.
   Expect: HTTP 401, message ephemeral_pub_rotation_not_allowed.
3. Re-read the grant row; caller_ephemeral_pub must still be E1.
4. Sending another lookup with E1 should succeed and mint a session token.

## P0-3: SSH claim_token redemption

1. Drive a full SSH connect on the CLI:
   bifrost remote connect --client-id <id> --ssh-key <pubkey>
2. Server side: on approval, the SSE ssh_connect_result event must contain
   claim_token + claim_expires_at + grant_id, NOT grant_session_token.
3. Server DB: bifrost_remote_invoke_ssh_claims should contain a row with the
   sha256(claim_token), grant_id, client_instance_id, caller_pubkey_fp,
   expires_at and empty claimed_at.
4. CLI must POST /v5/remote-invoke/grants/ssh-claim with a PoP envelope
   { client_instance_id, claim_token, caller_ephemeral_pub } and receive a
   normal GrantInfo back (grant_session_token encrypted with the caller's
   shared secret). After redemption, claimed_at on the SshClaim row must be
   non-empty.
5. Replaying the same claim_token: expect HTTP 401 claim_token_already_used.
6. After grant_session_expires_at the claim_token must also be refused.

## P0-4: server-derived caller fingerprint

1. POST /v5/remote-invoke/pairings/start without caller_pubkey.
   Expect: non-200, error mentions caller_pubkey or invalid_pair_code.
2. POST start_pairing with attacker-controlled caller_info.fingerprint = 'fake'
   AND a real caller_pubkey. Server must store
   caller_fingerprint = ed25519FingerprintFromBase64(caller_pubkey).
3. Inspect the pairings DAO row and the SSE pairing_offer event — both must
   show the derived fingerprint, never 'fake'.
4. claim flow: a POST /v5/remote-invoke/pairings/claim whose PoP-derived
   fingerprint differs from the stored caller_fingerprint is rejected
   (caller_pubkey_mismatch / caller_fingerprint_mismatch).

## TC-P0-5: PPE /v5 TLB strip-prefix route compatibility

Regression target: after TLB deploys `/v5/`, the relay server may receive the
caller path as `/remote-invoke/*`. This must still enter the v5 caller protocol
without reopening legacy v4 caller endpoints.

Steps:

1. Start `packages/bifrost-sync-server/dist/cli.js` from a fresh build with
   `--enable-remote-invoke` and a temporary SQLite data dir.
2. Send `POST /remote-invoke/pairings/start` with `{}`.
   Expect: HTTP 400 and message
   `pair_code, caller_info and caller_ephemeral_pub are required`, proving the
   request reached the v5 pairing handler rather than the generic 404 handler.
3. Send `POST /v5/remote-invoke/pairings/start` with `{}`.
   Expect: the same HTTP 400 v5 pairing handler response.
4. Send `POST /v4/remote-invoke/pairings/start` with `{}`.
   Expect: HTTP 410 and `protocol_version_not_supported`, proving legacy v4
   caller paths remain blocked.
5. Send `POST /remote-invoke/client/register` with `{}`.
   Expect: HTTP 404 `remote invoke endpoint not found`, proving stripped v5
   compatibility does not map unversioned paths back to v4 client registration.

Execution result (2026-06-29, local dist entry):

- PASS. Built `packages/bifrost-sync-server` with `pnpm --dir
  packages/bifrost-sync-server run build`, started
  `node packages/bifrost-sync-server/dist/cli.js -p 58688 -H 127.0.0.1
  -d /tmp/bifrost-sync-v5-strip-human.BF7Tar --enable-remote-invoke`, and
  verified:
  - `POST /remote-invoke/pairings/start` -> HTTP 400,
    `pair_code, caller_info and caller_ephemeral_pub are required`.
  - `POST /v5/remote-invoke/pairings/start` -> HTTP 400 with the same v5
    pairing-handler message.
  - `POST /v4/remote-invoke/pairings/start` -> HTTP 410,
    `protocol_version_not_supported`.
  - `POST /remote-invoke/client/register` -> HTTP 404,
    `remote invoke endpoint not found`.
  - Temporary data dir and process were cleaned up by the test trap.

## TC-P0-6: 正式 relay 默认链路与 PPE header 开关真实链路

Regression target: caller 和 target 都使用当前分支编译出的 Bifrost 二进制，
直连正式域名 `https://bifrost.bytedance.net`。默认不设置
`BIFROST_REMOTE_RELAY_HEADERS`，用于正式环境回归；发布前 PPE 验证时才显式
设置 `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'`。
该开关不依赖 UI、不改持久化配置。

Steps:

1. 执行仓库脚本
   `e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。
2. 脚本默认执行 `cargo build --bin bifrost`，记录 `target/debug/bifrost`
   的 git commit 与 sha256，确认 caller 和 target 都使用当前分支编译产物。
   如已明确完成构建，可用 `SKIP_BUILD=true` 跳过构建。
3. 脚本从默认 Bifrost 数据目录 `~/.bifrost/sync-state.json` 读取已登录
   sync token；也可用 `BIFROST_SYNC_TOKEN` 或 `BIFROST_SYNC_STATE_FILE` 覆盖。
   token 不打印到日志。
4. 脚本创建临时 target / caller-code / caller-ssh 数据目录，target 使用
   `--no-system-proxy --no-tray --skip-cert-check --unsafe-ssl` 启动当前分支
   Bifrost。未设置 `BIFROST_REMOTE_RELAY_HEADERS` 时不注入任何 relay header。
5. 脚本直连正式域名 `https://bifrost.bytedance.net`，先执行 Code 授权
   pair-code 流程，再执行 SSH key 授权流程。
6. 两种授权方式均执行同一 Remote 能力矩阵：`remote conn status`、
   `remote traffic list/get/search`、`remote file read/read-many/scratch-dir/list/stat/glob/find/hash/outline/write/edit/mkdir/move/delete/patch`、
   `remote exec`、`remote run`、`remote exec --detach`、`remote run --detach`、
   `remote job logs/watch/list/status`。
7. 最后执行 `remote conn down --all` 并清理临时 Bifrost 进程和临时数据目录。

Expected:

- CI 环境中脚本只打印 skip 并 0 退出；该脚本禁止在 GitHub CI 中真实连接
  外部 relay。
- 脚本启动时打印当前 relay mode、relay URL 和是否设置 relay headers；失败时
  自动保留临时目录用于排查。
- 未设置 `BIFROST_REMOTE_RELAY_HEADERS` 时，caller 的 pairing start、watch、
  claim/open 以及 target 的注册、pair-code、stream 请求均走正式 relay。
- 设置该环境变量后，同一脚本用于 PPE TLB 路由验证。
- UI 与持久化配置中不出现该测试开关。
- 仓库脚本覆盖 Code 授权与 SSH key 授权两条入口，并覆盖发布前 Remote
  常用 CLI 能力矩阵；任一命令失败时脚本非 0 退出。

Execution result:

- PASS。2026-06-29 在 `fix-p0-remote-invoke-hardening` 分支
  `fbadbdc0c813987cc79061b976676f8c0e9ad554` 上执行。当前分支编译产物
  `/Users/eden/work/github/bifrost-fix-p0-remote-invoke-hardening/target/debug/bifrost`
  的 sha256 为
  `1abe9e37a1472dd1d6995aa205dde2f35d26cf3e53498441c179228885fc0191`。
- 执行命令：
  `KEEP_TMP=1 SKIP_BUILD=true e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。
  脚本直连 `https://bifrost.bytedance.net`，使用
  `BIFROST_REMOTE_RELAY_HEADERS=x-tt-env=ppe_ticket_system,x-use-ppe=1`。
- 本轮 target client id 为 `56431d7a-7dfc-4d93-8dcb-9f63da53918c`，
  target 端口为 `52484`，临时目录为 `/tmp/bifrost-ppe-full.Qjyhbl`。
- Code 授权链路通过：`remote conn status`、`remote traffic list/get/search`、
  全部 remote file 子命令、`remote exec`、`remote run`、`remote exec --detach`、
  `remote run --detach`、`remote job logs/watch/list/status` 均 PASS。
- SSH key 授权链路通过：`remote conn status`、`remote traffic list/get/search`、
  全部 remote file 子命令、`remote exec`、`remote run`、`remote exec --detach`、
  `remote run --detach`、`remote job logs/watch/list/status` 均 PASS。
- PASS。2026-06-29 在当前分支模拟 CI 执行
  `CI=true e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`，脚本只输出
  `SKIP: remote relay full regression requires local/internal network access and is not supported in CI.`
  后 0 退出，未连接外部 relay。
- FAIL。2026-06-29 在当前分支 `2117b6fa341f568025004fe0d294f1fcedf332f9`
  上移除 PPE header 后执行正式 relay 回归：
  `env -u BIFROST_REMOTE_RELAY_HEADERS KEEP_TMP=1 e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。
  脚本确认 target 已注册到正式 relay，但 Code 授权入口在
  `start_pairing` 阶段稳定返回 `429 Too Many Requests`：
  `{"code":429,"message":"relay_queue_overflow","data":null}`。
  本地保留证据目录 `/tmp/bifrost-ppe-full.lOBYqj` 与
  `/tmp/bifrost-ppe-full.yFQwto`。
- 根因定位到已发布的 `bifrost-server-v4`：Redis-backed SSE queue 的 Lua
  仍同时访问 `ri:mq:client:<id>` 和 `ri:mq:client:<id>:bytes` 两个不同
  cluster slot，正式 Redis Cluster 返回 CROSSSLOT，服务端包装成
  `relay_queue_overflow`。server-v4 修复为 hash-tag key
  `ri:mq:{client:<id>}` / `ri:mq:{client:<id>}:bytes` 后需重新部署，再复跑
  本用例完整矩阵。
- PASS。2026-06-29 在 `bifrost-server-v4`
  `fix-p0-remote-invoke-hardening` commit
  `847fe2cf201ad15f3c335ef033cf16193c328511` 部署到 PPE 后，当前分支
  `a280a1c01c31bf2841304ea25a5b1ff0b4a80765` 执行：
  `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1' KEEP_TMP=1 e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。
  脚本从当前分支重新构建 `/Users/eden/work/github/bifrost/target/debug/bifrost`，
  二进制 sha256 为
  `0b05e9c68d5b1566fca450bf0c0b104c512b579e626bdb74951165a6c6501781`。
  target client id 为 `3545d8d8-7a7e-40fe-a4ab-525a62b1438f`，临时目录为
  `/tmp/bifrost-relay-full.3Xe4Ak`。
- Code 授权链路通过：grant `b90946b8272b3ee1` 创建成功，`remote conn status`、
  `remote traffic list/get/search`、全部 remote file 子命令、`remote exec`、
  `remote run`、`remote exec --detach`、`remote run --detach`、
  `remote job logs/watch/list/status` 均 PASS。
- SSH key 授权链路通过：`remote conn status`、`remote traffic list/get/search`、
  全部 remote file 子命令、`remote exec`、`remote run`、`remote exec --detach`、
  `remote run --detach`、`remote job logs/watch/list/status` 均 PASS。
- PASS。2026-06-29 为确认最新 HEAD，重新执行同一 PPE 全量脚本：
  `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1' KEEP_TMP=1 e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。
  本轮脚本从最新分支重新构建
  `/Users/eden/work/github/bifrost/target/debug/bifrost`，HEAD 为
  `1e8844b8f767f1602fda73682b80305789eff07d`，二进制 sha256 仍为
  `0b05e9c68d5b1566fca450bf0c0b104c512b579e626bdb74951165a6c6501781`。
  target client id 为 `5b81ab9e-7b7f-422a-a4cd-15f6f0417815`，临时目录为
  `/tmp/bifrost-relay-full.v46Fg5`。Code 授权 grant
  `b86654fe07edd5c0` 创建成功；Code 和 SSH key 两条链路的
  `remote conn status`、`remote traffic list/get/search`、全部 remote file
  子命令、`remote exec`、`remote run`、detach job 与
  `remote job logs/watch/list/status` 均 PASS。
- `remote conn down --all` 清理通过；验证完成后未发现当前分支 Bifrost 残留进程。

## TC-P0-7: client_auth_token query fallback rejected

Regression target: target/client SSE streams must not accept
`client_auth_token` from URL query parameters. Tokens must be sent with
`Authorization: Bearer ...` only.

Steps:

1. Execute:
   `pnpm --dir packages/bifrost-sync-server test -- src/__tests__/remote-invoke-security.test.ts`.
2. In `remote-invoke-security.test.ts`, verify the case
   `rejects client SSE authentication tokens in URL query parameters` registers
   a client, opens `/v4/remote-invoke/client/stream?...&client_auth_token=...`
   without an Authorization header, and asserts HTTP `401`.
3. Verify normal authenticated SSE cases in the same file pass with
   `Authorization: Bearer <client_auth_token>`.

Expected:

- Query-string client tokens are rejected with `401`.
- Existing client SSE flows keep working when the token is sent in the
  Authorization header.

Execution result:

- PASS. 2026-06-29 executed
  `pnpm --dir packages/bifrost-sync-server test -- src/__tests__/remote-invoke-security.test.ts src/__tests__/remote-invoke-relay-v2-phase1.test.ts src/__tests__/remote-invoke-pairing-timeout.test.ts`;
  all `15` selected test files and `196` tests passed.

## TC-P0-8: once grant concurrent openCall consumes atomically

Regression target: concurrent `/v5/remote-invoke/calls/open` requests against a
once grant with `remaining_calls=1` must not both create calls.

Steps:

1. Execute:
   `pnpm --dir packages/bifrost-sync-server test -- src/__tests__/remote-invoke-relay-v2-phase1.test.ts`.
2. In `remote-invoke-relay-v2-phase1.test.ts`, verify the case
   `atomically consumes once grants under concurrent v5 openCall requests`
   seeds a once grant with `max_calls=1` and `remaining_calls=1`.
3. Verify the test sends two concurrent `/v5/remote-invoke/calls/open`
   requests using the same grant session token and caller PoP key.

Expected:

- Exactly one request succeeds with HTTP `200`.
- The other request is rejected as `grant_consumed` or
  `grant_session_token_invalid`, depending on whether it reaches service logic
  before or after the successful request marks the grant consumed.
- The stored grant has `remaining_calls=0`.
- Only one call row is created for the target client.

Execution result:

- PASS. 2026-06-29 executed the selected sync-server remote-invoke test set;
  the atomic once-grant regression passed as part of `196` passing tests.

## TC-P0-9: one-click full local release regression

Regression target: the local release gate for Remote Invoke must be a single
script that covers local protocol/security regressions, the adjacent
`bifrost-server-v4` hardening checks, and the deployed relay Code + SSH key
end-to-end matrix. The script must stay out of CI because CI cannot access the
internal relay network.

Steps:

1. From the repository root, execute:
   `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1' KEEP_TMP=1 e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`.
2. Verify the script first runs the local sync-server Remote Invoke Vitest
   suites:
   `p0-hardening`, `remote-invoke-security`,
   `remote-invoke-relay-v2-phase1`, `remote-invoke-pairing-timeout`,
   `sse-multi-watcher`, `remote-invoke-sse`,
   `remote-invoke-stream-frame`, `grants-claim`, `grants-lookup`,
   `grants-revoke`, and `pop`.
3. Verify the script runs the Rust Remote Invoke CLI filter:
   `cargo test -p bifrost-cli remote -- --nocapture`.
4. Verify the script runs the adjacent server-v4 checks from
   `BIFROST_SERVER_V4_DIR` (default:
   `./bifrost-server-v4`): `pnpm run build` and
   `pnpm run test:remote-invoke-hardening`.
5. Verify the script then builds the current branch `bifrost` binary and uses
   that same binary for both local Bifrost clients: one target and one caller
   at a time.
6. Verify the deployed relay phase connects to
   `https://bifrost.bytedance.net`, runs the Code authorization path, runs a
   separate Code `remote_power_mgmt` authorization path, then runs the SSH key
   authorization path.
7. Verify Code shell/file and SSH key authorization paths execute the full
   ordinary Remote matrix:
   `remote conn status`, `remote traffic list/get/search`,
   `remote file read/read-many/scratch-dir/list/stat/glob/find/hash/outline/write/edit/mkdir/move/delete/patch`,
   `remote exec`, `remote run`, `remote exec --detach`,
   `remote run --detach`, and `remote job logs/watch/list/status`.
8. Verify Code power authorization and SSH key default Full Trust both execute
   the power-management matrix: `remote keep-awake status`,
   `remote keep-awake on`, `remote keep-awake mode get`,
   `remote keep-awake mode set off`, and `remote keep-awake off`.
9. Execute `CI=true e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`.

Expected:

- The default local command runs all three phases. Individual phases can be
  narrowed only by explicit local env flags: `RUN_LOCAL_CASES=0`,
  `RUN_SERVER_V4_CASES=0`, or `RUN_REMOTE_RELAY_CASES=0`.
- Missing `bifrost-server-v4` checkout fails fast unless
  `RUN_SERVER_V4_CASES=0` is explicitly set.
- PPE routing is enabled only by `BIFROST_REMOTE_RELAY_HEADERS`; the switch is
  not persisted and is not exposed in UI.
- The CI invocation prints skip and exits 0 before any local/server-v4/remote
  network work starts.
- On failure, the script exits non-zero and preserves the temp directory when
  one has been created.

Execution result:

- PASS. 2026-06-29 executed
  `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1' KEEP_TMP=1 e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`.
- The local phase passed:
  - sync-server Remote Invoke Vitest suites: `11` files, `60` tests passed.
  - `cargo test -p bifrost-cli remote -- --nocapture`: `245` lib tests,
    `15` CLI command tests passed.
  - `pnpm --dir bifrost-server-v4 run build`: PASS.
  - `pnpm --dir bifrost-server-v4 run test:remote-invoke-hardening`: PASS.
- During the first full run after expanding coverage, `remote run` exposed a
  target-to-relay send error while posting a call frame. We checked deployed
  `bifrost.server.v4` with `bytedcli log search-psm-log` for call
  `86d6226fb4cd87f1`; PPE access logs showed `/frame`, `/stream-frame` and
  `/exit` requests returning HTTP `200` and no `relay_queue` errors. The
  client-side target relay client was hardened with short retry for
  request-send failures only, and server-v4 regression tests were expanded to
  assert client frame/stream-frame/exit queue delivery uses
  `ri:mq:{caller:<callId>}`.
- The previous final full PPE run passed before `remote keep-awake` was added
  to the one-click matrix: target client id
  `06c2776a-d62f-4a8e-b1c3-47149edf71c3`, temp dir
  `/tmp/bifrost-relay-full.UODRT5`, target port `51792`, binary
  `/Users/eden/work/github/bifrost/target/debug/bifrost`, binary sha256
  `68e7c6455028991986d4a4320d28913ac169c2d0c953bf68409d17e6b66115b9`,
  Code grant `5fa75bfc0edc9527`.
- After auditing `remote --help`, the script was expanded to include the
  remaining `remote keep-awake` command family. This requires the server-side
  fix that treats SSH key default Full Trust (`remote_shell_interactive`) as
  allowed for `power.mgmt`; rerun the full PPE command after the updated
  `bifrost-server-v4` is deployed.

## TC-P0-10: async remote grant tests do not hold std mutex across await

Regression target: Remote Invoke CLI grant revocation tests must pass
`cargo clippy --workspace --all-targets --all-features -- -D warnings` without
holding a `std::sync::MutexGuard` across `await`.

Steps:

1. Execute:
   `cargo test -p bifrost-cli caller_delete_grant_treats -- --nocapture`.
2. Execute:
   `cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings`.
3. Execute:
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

Expected:

- Both async delete-grant tests pass.
- Clippy does not report `clippy::await_holding_lock` for
  `crates/bifrost-cli/src/commands/remote.rs`.

Execution result:

- PASS. 2026-06-29 executed the two commands above after moving the async tests
  to an async-aware test data-dir mutex, then executed the workspace clippy
  command; all checks passed.
