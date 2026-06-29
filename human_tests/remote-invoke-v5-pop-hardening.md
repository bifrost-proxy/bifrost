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

## TC-P0-6: 发布前 PPE relay header 环境变量真实链路

Regression target: caller 和 target 都使用当前分支编译出的 Bifrost 二进制，
直连正式域名 `https://bifrost.bytedance.net`，仅通过发布前测试环境变量
`BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'`
启用 PPE TLB header，不依赖 UI、不改持久化配置。

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
   Bifrost，并通过 `BIFROST_REMOTE_RELAY_HEADERS` 默认注入 PPE header。
5. 脚本直连正式域名 `https://bifrost.bytedance.net`，先执行 Code 授权
   pair-code 流程，再执行 SSH key 授权流程。
6. 两种授权方式均执行同一 Remote 能力矩阵：`remote conn status`、
   `remote traffic list/get/search`、`remote file read/read-many/scratch-dir/list/stat/glob/find/hash/outline/write/edit/mkdir/move/delete/patch`、
   `remote exec`、`remote run`、`remote exec --detach`、`remote run --detach`、
   `remote job logs/watch/list/status`。
7. 最后执行 `remote conn down --all` 并清理临时 Bifrost 进程和临时数据目录。

Expected:

- 未设置 `BIFROST_REMOTE_RELAY_HEADERS` 时，caller 访问
  `https://bifrost.bytedance.net/v5/remote-invoke/pairings/start` 会命中非
  PPE 路由并返回后端 404。
- 设置该环境变量后，caller 的 pairing start、watch、claim/open 以及 target
  的注册、pair-code、stream 请求均通过 PPE 路由，完整远程调用链路成功。
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
- `remote conn down --all` 清理通过；验证完成后未发现当前分支 Bifrost 残留进程。
