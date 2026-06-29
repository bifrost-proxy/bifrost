# Remote Invoke v5 PoP Hardening — P0 修复方案

> 状态: 草案，待远端 Mac 上 apply
> 分支建议: `feat/remote-invoke-v5-pop-hardening`
> 基线 commit: 29f833e (feat/remote-invoke-v5-pop)
> 范围: P0-1 / P0-2 / P0-3 / P0-4 + 对应测试

本稿是第二轮安全复审 (P0 部分) 的可落地方案。每个 P0 都给出：
- 漏洞回顾 (file:line 证据)
- 修复设计 (含取舍说明)
- 完整代码草案 (直接复制即可)
- 单元/集成测试 (vitest)
- 回归矩阵

---

## 全局原则

1. **不破坏 v5 PoP 主干**：所有 P0 修复都在现有 service / route / ssh-auth 三层内部完成，不引入新对外端点。
2. **schema 兼容**：本轮**不改表结构**，能在内存 map / DAO 现有列上落地的就不动 SQL。唯一例外是 P0-1 在 `bifrost_remote_invoke_clients` 表已存在 `user_id` 列的前提下，将该值反查注入 `RouteEntry` 内存结构。
3. **严格优先用 server 端可信值**，client / caller 自报字段一律降级为"展示性 hint"。
4. **回滚**: 每个 P0 修改独立，若发现回归可独立 revert，无相互依赖。

## 2026-06-29 PPE `/v5` 路由回归修复

PPE TLB 已部署 `/v5/` 路由后，真实双 Bifrost 验证仍显示
`POST /v5/remote-invoke/pairings/start` 返回后端 404，而本地
`dist/cli.js --enable-remote-invoke` 构建产物对同一路径返回 v5 业务错误。
这说明源码和发布产物已注册显式 `/v5/remote-invoke/*`，线上失败更像是
TLB 或服务框架将 `/v5` 前缀剥掉后转发为 `/remote-invoke/*`。

修复策略：

- server 入口将 `/remote-invoke/*` 仅归一化为 v5 caller 协议路径
  `/v5/remote-invoke/*`。
- 保持 `/v4/remote-invoke/pairings/start`、`watch`、`grants/reusable`、
  `calls/open` 等旧 caller 敏感入口返回 `410 protocol_version_not_supported`。
- 不把无版本 `/remote-invoke/client/*` 映射到 v4 client 注册/stream 路由，
  避免扩大 client 面暴露。
- 补充 Vitest 回归：模拟 TLB strip prefix 的
  `POST /remote-invoke/pairings/start` 必须进入 v5 route 并返回 400 业务错误，
  同时 v4 caller 入口仍为 410。

### 发布前 PPE header 验证开关

远端正式域名 `https://bifrost.bytedance.net` 的 PPE 路由需要 TLB 请求头
`x-tt-env=ppe_ticket_system` 与 `x-use-ppe=1`。该能力仅用于发布前真实环境
验证，不进入 UI，也不写入 sync/config 持久化配置。

实现策略：

- 新增进程级环境变量 `BIFROST_REMOTE_RELAY_HEADERS`，格式为逗号分隔的
  `name=value` 列表，例如：
  `BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1'`。
- target 端 Remote Invoke worker 的 relay 注册、心跳、pair-code、SSE stream、
  grant/call 请求均复用同一组 header。
- caller 端 `bifrost remote *` 的 pairing、claim/lookup/open/revoke、SSH 复用、
  SSE watch 与 job polling 均复用同一组 header。
- relay SSE/watch/job polling 使用专用 direct SSE client，禁用 gzip/br/zstd/deflate
  自动解压，并显式发送 `Accept-Encoding: identity` 与
  `Cache-Control: no-transform`，避免 PPE TLB 对长连接做转换后触发 body decode
  错误。
- target SSE 重连只清理本地已过期 pairing，不再每次重连都调用 relay
  `cancel_pending_pairings`。PPE/TLB 可能周期性关闭长连接，重连不能误拒绝仍在
  等待本地审批的活跃 pairing。
- 拒绝通过该变量覆盖 `authorization`、`cookie`、`host`、`x-bifrost-token`
  等敏感或鉴权 header，避免测试开关变成凭据注入通道。
- 环境变量解析失败只记录 warning 并忽略该开关，避免发布构建因测试变量写错而
  阻断普通启动。
- 发布前全量 PPE 回归脚本落在
  `e2e-tests/tests/test_remote_invoke_ppe_full_e2e.sh`。脚本默认构建当前分支
  `target/debug/bifrost`，从默认 Bifrost 数据目录读取登录 token，连接
  `https://bifrost.bytedance.net`，并覆盖 Code 授权、SSH key 授权、
  remote traffic、remote file、remote exec/run/job 与连接清理矩阵。可通过
  `SKIP_BUILD=true` 跳过构建，通过 `BIFROST_REMOTE_RELAY_URL`、
  `BIFROST_REMOTE_RELAY_HEADERS`、`BIFROST_SYNC_STATE_FILE` 或
  `BIFROST_SYNC_TOKEN` 覆盖默认环境。

---

## P0-1 / P0-3 合并: SSH 路由必须强绑定 user 与 client，且 SSH 审批不再绕过 v5 PoP

### 1.1 漏洞证据

- `ssh-auth.ts:74-120`：`routeByDeviceCode` / `deviceCodeByClientInstanceId` 全是全局命名空间，无 `userId` 维度。
- `service.ts:1046-1051` (heartbeat) 和 `service.ts:228-233` (registerClient)：直接把 client 上报的 `ssh_device_route` 转发给 `syncSshRoute`，无任何 owner 校验。
- `service.ts:1084-1146` (`submitSshConnectResult`)：approved 分支直接落库 `permanent` + `max_calls=999999` + `user_id:''`，完全绕开 v5 的 claim_token / grant_session_token 链路。

### 1.2 修复设计

**RouteEntry 必须携带 userId**

- `SshAuthService.syncSshRoute(...)` 增加 `userId` 参数。
- 若 `previousRoute.userId !== userId`，拒绝并抛 `device_code_owned_by_other_user`。
- `issueChallenge(deviceCode)` 内部继续不需要 userId（device_code 对外公开），但下游 `verifyAndPrepareConnect` 不变。
- 调用点：`service.registerClient` 与 `service.clientHeartbeat` 处必须传入当前请求的 `userId`（不再传 `''`）。

**SSH approved 分支降级为 v5 claim**

- `submitSshConnectResult` 在 `approved` 时不直接 INSERT grant，而是：
  1. 在 SSH 通道上派生一次性 `claim_token`（同 v5 pairing 路径）；
  2. 通过 SSE 推回 caller：caller 拿 claim_token 走 `POST /v5/remote-invoke/grants/claim`；
  3. caller 必须签 PoP envelope (`requirePoP`) 才能把 claim 兑成 `grant_session_token`。
- 同时强制 `grant_mode` 上限为 `1d`（不再 `permanent`），`max_calls` 上限按 policy（默认 1000），`user_id` 必须非空。

> 兼容: 旧 caller 仍能收到 `grant_session_token`，但它现在走 v5 路径产生。v4 `grant_id` / `relay_token` 字段保留为占位 (空串)。

### 1.3 完整代码草案

**`packages/bifrost-sync-server/src/remote-invoke/ssh-auth.ts`**

```ts
type RouteEntry = SshDeviceRoute & {
  clientInstanceId: string;
  userId: string;             // NEW
  expiresAt: number;
};

export class SshAuthService {
  // ...
  syncSshRoute(
    clientInstanceId: string,
    userId: string,            // NEW
    route: SshDeviceRoute | null | undefined,
  ): { routeCleared: boolean; routeChanged: boolean } {
    this.cleanupExpiredState();
    const previousDeviceCode = this.deviceCodeByClientInstanceId.get(clientInstanceId);
    const previousRoute = previousDeviceCode
      ? this.routeByDeviceCode.get(previousDeviceCode)
      : undefined;

    if (!route) {
      if (previousDeviceCode) {
        this.routeByDeviceCode.delete(previousDeviceCode);
        this.deviceCodeByClientInstanceId.delete(clientInstanceId);
      }
      return {
        routeCleared: !!previousDeviceCode,
        routeChanged: !!previousDeviceCode,
      };
    }

    this.assertRouteMatchesPublicKey(route);

    // NEW: prevent cross-user device_code hijack.
    const candidate = this.routeByDeviceCode.get(route.device_code);
    if (candidate && candidate.userId !== userId) {
      throw new Error('device_code_owned_by_other_user');
    }
    if (previousRoute && previousRoute.userId !== userId) {
      throw new Error('device_code_owned_by_other_user');
    }
    if (previousDeviceCode && previousDeviceCode !== route.device_code) {
      this.routeByDeviceCode.delete(previousDeviceCode);
    }

    const expiresAt = Date.now() + SSH_ROUTE_TTL_MS;
    this.routeByDeviceCode.set(route.device_code, {
      ...route,
      clientInstanceId,
      userId,
      expiresAt,
    });
    this.deviceCodeByClientInstanceId.set(clientInstanceId, route.device_code);
    return {
      routeCleared: false,
      routeChanged:
        !previousRoute ||
        previousRoute.device_code !== route.device_code ||
        previousRoute.public_key_pem !== route.public_key_pem,
    };
  }

  // verifyAndPrepareConnect: in PendingConnectEntry now carry userId
  // so submitSshConnectResult can use it without re-querying.
}

type PendingConnectEntry = {
  connectId: string;
  clientInstanceId: string;
  userId: string;             // NEW (copied from route.userId)
  deviceCode: string;
  relayToken: string;
  expiresAt: number;
  callerInfo?: SshConnectRequest['caller_info'];
  sshKeyFingerprint: string;
};

// ... inside verifyAndPrepareConnect, after fetching route:
this.pendingConnects.set(connectId, {
  connectId,
  clientInstanceId: route.clientInstanceId,
  userId: route.userId,        // NEW
  deviceCode: route.device_code,
  relayToken,
  expiresAt,
  callerInfo: body.caller_info,
  sshKeyFingerprint: fingerprint,
});

// completeConnect: also return userId
return {
  connect_id: body.connect_id,
  status: body.status,
  user_id: pending.userId,     // NEW
  grant_id: body.grant_id,
  expires_at: body.expires_at ?? null,
  reason: body.reason,
  caller_fingerprint: body.caller_fingerprint,
  grant_mode: body.grant_mode,
  caller_info: pending.callerInfo,
  ssh_key_fingerprint: pending.sshKeyFingerprint,
};
```

**`packages/bifrost-sync-server/src/remote-invoke/service.ts`**

```ts
// registerClient — pass userId into syncSshRoute
if (Object.prototype.hasOwnProperty.call(req, 'ssh_device_route')) {
  const routeState = this.sshAuth.syncSshRoute(
    req.client_instance_id,
    userId,                            // CHANGED
    req.ssh_device_route ?? null,
  );
  if (routeState.routeChanged) {
    await this.storage.remoteInvoke.revokeSshGrantsForClient(req.client_instance_id);
  }
}

// clientHeartbeat — look up user_id from clients table
async clientHeartbeat(req: ClientHeartbeatRequest): Promise<void> {
  const stream = getClientStream(req.client_instance_id);
  if (stream) {
    stream.lastHeartbeat = Date.now();
  }
  await this.storage.remoteInvoke.updateClientRecord(req.client_instance_id, {
    last_heartbeat_at: new Date().toISOString(),
  });
  if (Object.prototype.hasOwnProperty.call(req, 'ssh_device_route')) {
    const record = await this.storage.remoteInvoke.getClientRecord(req.client_instance_id);
    if (!record) throw new Error('client_not_registered');
    const routeState = this.sshAuth.syncSshRoute(
      req.client_instance_id,
      record.user_id,                  // FROM DB
      req.ssh_device_route ?? null,
    );
    if (routeState.routeChanged) {
      await this.storage.remoteInvoke.revokeSshGrantsForClient(req.client_instance_id);
    }
  }
}

// submitSshConnectResult — route through v5 claim_token instead of writing
// a permanent 999999-call grant.
async submitSshConnectResult(
  clientInstanceId: string,
  req: SshConnectResultRequest,
): Promise<void> {
  const result = this.sshAuth.completeConnect(clientInstanceId, req);
  if (result.status !== 'approved' || !result.user_id) {
    // rejected: nothing to mint; only push event.
    pushToClient(clientInstanceId, 'ssh_connect_complete', {
      connect_id: result.connect_id,
      status: result.status,
      reason: result.reason ?? '',
    });
    return;
  }

  const callerPubkey = result.caller_info?.caller_pubkey || '';
  if (!callerPubkey) throw new Error('caller_pubkey_required');

  // Server-derived fingerprint, not caller-claimed.
  const callerFp = ed25519FingerprintFromBase64(callerPubkey);
  const callerDisplayName =
    result.caller_info?.hostname ||
    result.caller_info?.username ||
    '';

  await this.storage.remoteInvoke.revokeActiveGrantsForCaller(clientInstanceId, callerFp);

  // Clamp grant_mode and max_calls; SSH path cannot bypass server policy.
  const grantMode: GrantMode = clampSshGrantMode(req.grant_mode);
  const maxCalls = this.config.ssh_grant_max_calls ?? 1000;
  const ttlMs = grantModeTtlMs(grantMode);
  const expiresAt = ttlMs ? new Date(Date.now() + ttlMs).toISOString() : '';
  const now = new Date().toISOString();

  const grant: RemoteInvokeGrant = {
    id: result.grant_id || nanoid(),
    user_id: result.user_id,                     // FROM RouteEntry
    client_instance_id: clientInstanceId,
    caller_fingerprint: callerFp,
    caller_display_name: callerDisplayName,
    caller_pubkey: callerPubkey,
    caller_pubkey_fp: callerFp,
    caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
    client_ephemeral_pub: req.client_ephemeral_pub ?? '',
    grant_mode: grantMode,
    grant_scope: normalizeGrantScope(req.grant_scope),
    file_access: normalizeFileAccess(req.file_access),
    ssh_key_id: '',
    ssh_key_fingerprint: result.ssh_key_fingerprint,
    status: 'active',
    first_authorized_at: now,
    expires_at: expiresAt,
    last_used_at: now,
    max_calls: maxCalls,
    remaining_calls: maxCalls,
    created_by: 'ssh_publickey',
    revoked_at: '',
    session_token_hash: '',
    session_token_expires_at: '',
    last_nonce_seen: '',
    create_time: now,
    update_time: now,
  };
  await this.storage.remoteInvoke.upsertGrant(grant);

  // Mint single-use claim_token; caller MUST exchange via v5 PoP.
  const claimToken = randomToken();
  const claimExpiresAt = new Date(Date.now() + CLAIM_TOKEN_TTL_MS).toISOString();
  await this.storage.remoteInvoke.createSshClaim({
    claim_token_hash: sha256Hex(claimToken),
    grant_id: grant.id,
    client_instance_id: clientInstanceId,
    caller_pubkey_fp: callerFp,
    expires_at: claimExpiresAt,
  });

  pushToClient(clientInstanceId, 'ssh_connect_complete', {
    connect_id: result.connect_id,
    status: 'approved',
    grant_id: grant.id,
    claim_token: claimToken,
    claim_expires_at: claimExpiresAt,
  });
}

function clampSshGrantMode(mode: GrantMode | undefined): GrantMode {
  switch (mode) {
    case '30m':
    case '1h':
      return mode;
    case 'once':
      return 'once';
    case '1d':
    case 'permanent':
    default:
      return '1d';
  }
}
```

> 备注：`createSshClaim` 需要新 DAO 方法或直接复用 pairing 表的 `claim_token_hash` 列（推荐建一张轻量 `bifrost_remote_invoke_ssh_claims (token_hash PK, grant_id, client_instance_id, caller_pubkey_fp, expires_at)`，避免与 pairing 表 join 复杂化）。

### 1.4 测试矩阵

| 测试 | 类型 | 预期 |
|---|---|---|
| `syncSshRoute` user A 已占 → user B 同 device_code | unit | 抛 `device_code_owned_by_other_user` |
| heartbeat 自报 user A 的 device_code → server 反查 client.user_id=B | unit | 同上 |
| `submitSshConnectResult` approved 但 `caller_info.caller_pubkey` 为空 | unit | 抛 `caller_pubkey_required` |
| approved 后 caller_fingerprint 取值来自 server 派生，与 caller_info.fingerprint 不一致 | unit | DB grant.caller_fingerprint == server-derived |
| `grant_mode='permanent'` 请求 → 实际写库 `1d` | unit | `clampSshGrantMode` |
| SSE 推回 `ssh_connect_complete` 包含 `claim_token` 而非直接的 grant_session_token | unit | 事件 schema 校验 |
| caller 拿 claim_token 调 `POST /v5/.../grants/claim` 带 PoP envelope → 200 | e2e | 返 `grant_session_token` |
| 同一 claim_token 再次 claim | e2e | 401 `claim_token_invalid` |

---

## P0-2: `lookupGrantSession` 不再静默轮换 caller_ephemeral_pub

### 2.1 漏洞证据

`service.ts:617-630`：caller 一次合法 PoP 即可写入任意新 ephemeral_pub，等于免 client 二次同意接管 ECDH 会话。

### 2.2 修复设计

- 默认行为：`req.caller_ephemeral_pub !== grant.caller_ephemeral_pub` 时**抛 `ephemeral_pub_rotation_not_allowed`**，不再静默 update。
- 显式轮换路径：caller 想换 ephemeral_pub → 走 `POST /v5/remote-invoke/grants/ephemeral-rotate`：
  - 校验 PoP；
  - 必须 client 通过 SSE 收到 `ephemeral_rotation_request`，并由用户在 UI 上点 approve，client 回 `POST /v4/.../grants/:id/ephemeral-rotate/approve`；
  - 双方确认后才 update `caller_ephemeral_pub` 并写审计事件。
- 兼容: 现有 caller CLI 不会主动换 ephemeral，受影响为 0；遇到老 caller 复用旧 grant 自动重连场景，CLI 已经在 `merge_transport_context` 里做了"saved 与 grant 一致"校验，本身就 abort，不存在隐藏破坏。

### 2.3 完整代码草案

```ts
async lookupGrantSession(
  req: GrantLookupRequest,
  callerPubkeyFp: string,
): Promise<GrantSessionResponse> {
  const grant = await this.storage.remoteInvoke.getGrantByCallerFp(
    callerPubkeyFp,
    req.client_instance_id,
  );
  if (!grant || grant.revoked_at || grant.status !== 'active') {
    throw new Error('grant_not_found');
  }
  if (grant.expires_at && new Date(grant.expires_at) < new Date()) {
    await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'expired' });
    throw new Error('grant_not_found');
  }
  // SECURITY FIX (P0-2): never silently rotate caller_ephemeral_pub.
  if (
    grant.caller_ephemeral_pub &&
    req.caller_ephemeral_pub &&
    grant.caller_ephemeral_pub !== req.caller_ephemeral_pub
  ) {
    throw new Error('ephemeral_pub_rotation_not_allowed');
  }
  // First-time bind only: caller_ephemeral_pub empty (legacy migration) ok to set.
  if (!grant.caller_ephemeral_pub && req.caller_ephemeral_pub) {
    await this.storage.remoteInvoke.updateGrantCallerEphemeralPub(
      grant.id,
      req.caller_ephemeral_pub,
    );
  }
  return this.mintGrantSessionToken(grant.id);
}
```

### 2.4 测试矩阵

| 测试 | 类型 | 预期 |
|---|---|---|
| grant.caller_ephemeral_pub=X，lookup 提交 X | unit | 200 |
| grant.caller_ephemeral_pub=X，lookup 提交 Y | unit | 抛 `ephemeral_pub_rotation_not_allowed` |
| grant.caller_ephemeral_pub='' (legacy)，lookup 提交 X | unit | 写入 X 并 200 |
| lookup 路由 401 → caller CLI 是否能合理报错并提示 reconnect | e2e | CLI `reconnect required` |

---

## P0-4: pairing 中 caller_fingerprint 必须 server 端派生

### 3.1 漏洞证据

- `service.ts:377-396`：`pairing.caller_fingerprint = req.caller_info.fingerprint`，攻击者可控。
- pairing UI 弹窗、SSE `pairing_request` 事件、审计日志都用这个值。

### 3.2 修复设计

- `startPairing` 强制 caller 在 envelope 里附 `caller_pubkey`（base64 SPKI），server 端调 `ed25519FingerprintFromBase64` 派生，**不再读** `req.caller_info.fingerprint`。
- 老 caller 没传 caller_pubkey → 拒绝 (`caller_pubkey_required_for_pairing`)。
- 派生出的 fingerprint 写入 pairing.caller_fingerprint + pushToClient 事件，且必须与后续 `redeemClaim` 时 PoP envelope 派生出的 fp **完全一致**，否则 `claim_token_invalid`。

### 3.3 完整代码草案

```ts
// service.ts -- startPairing
async startPairing(
  _userId: string,
  req: StartPairingRequest,
  sourceIp?: string,
): Promise<{ pairing_id: string; watch_token: string; expires_at: string }> {
  // ... existing pair_code resolution ...

  // SECURITY FIX (P0-4): server-derived fingerprint only.
  const callerPubkey = (req as any).caller_pubkey
    ?? req.caller_info?.caller_pubkey;
  if (!callerPubkey || typeof callerPubkey !== 'string') {
    throw new Error('caller_pubkey_required_for_pairing');
  }
  const callerFingerprint = ed25519FingerprintFromBase64(callerPubkey);

  const pairing: RemoteInvokePairing = {
    id: pairingId,
    user_id: clientStream.userId,
    client_instance_id: resolvedClientId,
    caller_fingerprint: callerFingerprint,        // SERVER-DERIVED
    pair_code: req.pair_code,
    status: 'pending_approval',
    caller_pubkey: callerPubkey,                  // STORED for redeemClaim
    caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
    client_ephemeral_pub: '',
    caller_info_json: JSON.stringify({
      ...req.caller_info,
      fingerprint: callerFingerprint,             // overwrite untrusted
    }),
    // ... unchanged ...
  };

  // ... unchanged storage + pushToClient ...

  pushToClient(resolvedClientId, 'pairing_request', {
    pairing_id: pairingId,
    caller_fingerprint: callerFingerprint,        // SERVER-DERIVED
    caller_display_name: req.caller_info.display_name ?? '',
    caller_info: { ...req.caller_info, fingerprint: callerFingerprint },
    caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
    source_ip: sourceIp,
    user_agent: req.caller_info.user_agent ?? '',
    expires_at: expiresAt,
  });
  // ...
}

// redeemClaim -- assert PoP fp matches pairing.caller_pubkey
async redeemClaim(req: GrantClaimRequest, callerPubkeyFp: string): Promise<GrantSessionResponse> {
  const pairing = await this.storage.remoteInvoke.getPairingByClaimTokenHash(sha256Hex(req.claim_token));
  if (!pairing) throw new Error('claim_token_invalid');
  // ... existing checks ...

  // SECURITY: PoP fingerprint must match what UI consented to.
  if (pairing.caller_pubkey) {
    const expectedFp = ed25519FingerprintFromBase64(pairing.caller_pubkey);
    if (expectedFp !== callerPubkeyFp) throw new Error('caller_pubkey_mismatch');
  } else {
    // Legacy rows without stored pubkey: deny the claim outright.
    throw new Error('caller_pubkey_mismatch');
  }
  // ... rest unchanged ...
}
```

> CLI 侧（`crates/bifrost-cli/src/commands/remote.rs`）：现有 `start_pairing` 已经在 `caller_info` 里塞 caller_pubkey；server 现在改读这个字段，CLI **不需要改**。但建议加一行明确的 `body["caller_pubkey"] = caller_pubkey_b64` 字段，使 envelope schema 在 server / CLI 两端解耦。

### 3.4 测试矩阵

| 测试 | 类型 | 预期 |
|---|---|---|
| startPairing 不带 caller_pubkey | unit | 抛 `caller_pubkey_required_for_pairing` |
| startPairing caller_info.fingerprint='SPOOFED'，caller_pubkey 正常 | unit | DB pairing.caller_fingerprint == sha256(spki) ≠ 'SPOOFED' |
| pushToClient 事件载荷里的 caller_fingerprint 必须等于 server-derived | unit | event payload schema |
| redeemClaim PoP envelope 的 fp 与 pairing.caller_pubkey 不一致 | unit | 抛 `caller_pubkey_mismatch` |
| 老 row pairing.caller_pubkey 为空 → redeem | unit | 抛 `caller_pubkey_mismatch` (强迫重新 pair) |

---

## 端到端测试计划

### A. vitest 套件

新增文件 `packages/bifrost-sync-server/test/security-hardening.spec.ts`：

```ts
describe('P0-1 SSH route owner binding', () => {
  it('rejects cross-user device_code claim', /* ... */);
  it('heartbeat with mismatched user clears nothing', /* ... */);
});

describe('P0-2 ephemeral pub immutability', () => {
  it('rejects silent rotation', /* ... */);
  it('allows first-time bind only', /* ... */);
});

describe('P0-3 SSH approval routes through claim_token', () => {
  it('clamps grant_mode to 1d', /* ... */);
  it('writes server-derived caller_fingerprint', /* ... */);
  it('mints claim_token instead of grant_session_token', /* ... */);
});

describe('P0-4 server-derived pairing fingerprint', () => {
  it('uses ed25519FingerprintFromBase64', /* ... */);
  it('overrides untrusted caller_info.fingerprint', /* ... */);
  it('rejects redeemClaim with mismatched PoP fp', /* ... */);
});
```

### B. 集成测试 (`packages/bifrost-sync-server/test/e2e-v5-pop.spec.ts` 扩展)

| 场景 | 步骤 | 预期 |
|---|---|---|
| 完整 SSH 授权链路 (修复后) | ssh_challenge → ssh_connect → user approve → caller 拿 claim_token → POST /v5/.../grants/claim with PoP → 200 | grant_session_token issued |
| SSH 跨 user 抢占 | user A client 注册 device_code → user B client heartbeat 自报同 device_code | route 拒绝并日志 |
| ephemeral 接管尝试 | 合法 caller A 拿到 grant → 攻击者用 A 的 long-term key + 自己 ephemeral_pub 调 lookup | 401 `ephemeral_pub_rotation_not_allowed` |
| caller_info.fingerprint 欺骗 | startPairing caller_info.fingerprint='evil' 但 caller_pubkey 是合法 caller 的 | UI 弹窗 + DB 记录 == 合法 caller fp，'evil' 被丢弃 |

### C. 远端 CLI 端到端 (在 Mac 上跑)

```bash
# 1. 主干 + 修复后 sync-server 部署到 bifrost.bytedance.net staging
# 2. caller A 拿 SSH key 连接：
bifrost remote conn up --ssh-key ~/.bifrost/test-keyA.key
bifrost remote exec --shell-text "uname -a"

# 3. 验证 grant_mode 被 clamp:
sqlite3 ... "SELECT grant_mode, max_calls FROM bifrost_remote_invoke_grants ORDER BY create_time DESC LIMIT 1;"
# 期望: 1d, 1000

# 4. 验证 fingerprint server-derived:
# 用恶意 caller_info.fingerprint 调 /v5/.../pairings/start, 然后比对 DB
```

### D. 回归矩阵

| 场景 | 修复前 | 修复后 | 备注 |
|---|---|---|---|
| 正常 pair_code 流程 | ✅ | ✅ | 主路径不变 |
| 正常 SSH key 流程 | ✅ (直接 grant) | ✅ (claim → grant) | caller CLI 需要支持 claim 兑换；当前 CLI 已支持（v5 pair_code 路径同 endpoint） |
| 老 v4 CLI 试图调 v5 端点 | 410 | 410 | 不变 |
| 老 v4 CLI 走 SSH 通路 | grant 直接落 | 收到 `ssh_connect_complete` 含 claim_token，需升级 CLI | 文档标注 BREAKING |

### E. CI 全量覆盖清单

- `npm --workspace bifrost-sync-server run lint`
- `npm --workspace bifrost-sync-server run test`
- `npm --workspace bifrost-sync-server run test:e2e`
- `cargo test -p bifrost-cli --features remote`
- `cargo test -p bifrost-admin --features remote_invoke`
- GitHub Actions: `.github/workflows/ci.yml` 全平台 (linux/mac/win) 全绿

---

## 落地步骤 (Mac 上执行)

1. `git checkout -b feat/remote-invoke-v5-pop-hardening`
2. 按 §1/§2/§3 改 `ssh-auth.ts` / `service.ts` / DAO `createSshClaim`
3. 新增 SQL migration: `bifrost_remote_invoke_ssh_claims` 表
4. 写测试 (§A/§B)
5. `npm test && cargo test` 全绿
6. `git push origin feat/...` 开 MR，跟 CI 直到全绿
7. Merge 前在 staging 跑一次 D 段远端 e2e

---

## 风险与回滚

- **BREAKING**: 老 caller CLI 走 SSH 通路会收到 `ssh_connect_complete` 含 `claim_token`，需要 CLI ≥ 0.0.103 才能解析。建议同步出 CLI release。
- **回滚**: 单独 revert 任一 P0 即可，互相无依赖。
- **监控**: 上线后 1 周关注 sync-server 日志中 `device_code_owned_by_other_user` / `ephemeral_pub_rotation_not_allowed` / `caller_pubkey_required_for_pairing` / `caller_pubkey_mismatch` 出现频次。突增可能说明真实滥用或老 caller 未升级。
