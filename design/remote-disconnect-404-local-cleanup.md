# Remote Disconnect 404 本地清理修复

## 背景

`bifrost remote disconnect --all` 当前会先调用 relay `DELETE /v4/remote-invoke/grants/:grant_id`，只有远端删除成功时才移除本地 `remote-connections.json` 里的记录。

当 grant 已经在 relay 侧被清理、过期或被其他入口删除时，CLI 会收到 `404 grant_not_found`。历史行为下这会中断遍历并保留本地连接记录，形成“幽灵连接”。本方案通过在 caller 侧显式识别 `AlreadyMissing` 归因，让本地清理与 relay 清理彻底解耦，做到幂等收敛。

## 用户目标验证清单

### 必须实现

- `bifrost remote disconnect --all` 在 relay 返回 `404` 时仍然清理本地连接记录，遍历不中断。
- 单个 `bifrost remote disconnect` 与 `--grant-id` 路径保持相同的幂等语义。
- CLI 在 `--grant-id` 路径下可以明确告知用户「remote grant 已经不存在，本地记录同步清理」。
- 非 `404` 的失败原样上抛，让用户能看到真实网络或服务端异常。
- `remote-connections.json` 中不留下无对应 grant 的孤儿条目。

### 必须不破坏

- 正常 `disconnect` 流程（remote grant 存在 → 删除 → 本地清理）行为完全兼容。
- 不引入静默吞错：非 404 错误仍然打印失败并保留连接。
- `remote connect` / `remote status` / `remote grant list` 等其它命令行为不变。

### 必须真实验证

- Cargo 单元测试覆盖 `classify_delete_grant_failure`。
- E2E `TC-RI-08A` 覆盖“预先 DELETE relay grant 制造 404 → 再 disconnect --all → 本地记录归零”的路径。
- Human tests `TC-RI-回归-120` 记录真实 CLI 输出与 `remote-connections.json` 归零证据。

## 产品语义

### `AlreadyMissing` 是成功的一种

CLI 把 `DeleteGrantOutcome` 显式建模为两种成功：

- `Deleted`：relay 侧确实删除了 grant，本次调用产生了实际副作用。
- `AlreadyMissing`：relay 侧已经不存在该 grant（`404` 或 body 含 `grant_not_found`），本次调用只做本地收敛。

两者都触发本地 `connections.retain(...) + save_connections`，避免幽灵连接。

### 非 404 错误保留旧语义

`classify_delete_grant_failure(status, body)` 在 `status != NOT_FOUND` 且 body 不含 `grant_not_found` 时返回 `None`，由调用方按原错误向上抛出，用户可以看到具体 HTTP 状态或网络失败。

## 技术细节

### 结果分类

`crates/bifrost-cli/src/commands/remote.rs`：

- `enum DeleteGrantOutcome { Deleted, AlreadyMissing }`（第 5369 行）。
- `fn classify_delete_grant_failure(status, body) -> Option<DeleteGrantOutcome>`（第 4811-4817 行）：
  - `status == NOT_FOUND` → `AlreadyMissing`。
  - `body.contains("grant_not_found")` → `AlreadyMissing`。
  - 其它 → `None`（错误上抛）。
- `caller.delete_grant()`（第 5673-5698 行）：
  - 2xx → `Deleted`。
  - 非 2xx 且被 `classify_delete_grant_failure` 归类为 `AlreadyMissing` → `AlreadyMissing`。
  - 其它 → `Err`。

### `handle_disconnect` 三条路径

`handle_disconnect()`（第 2750 行起）：

- **`--grant-id`**：直接 `caller.delete_grant(gid, fingerprint)`。命中 `Deleted` 或 `AlreadyMissing` 都执行 `connections.retain(|c| c.grant_id != gid)` + `save_connections`，然后打印文案。
  - `Deleted` → `✓ Grant <short_id> revoked.`
  - `AlreadyMissing` → `✓ Grant <short_id> was already missing on relay; local record removed.`（第 2772 行）
- **`--all`**：`revoke_all_matching_grants()` 循环遍历本地 `remote-connections.json`，逐条 `find_reusable_grant` + `delete_grant`，将 `Deleted` 与 `AlreadyMissing` 都计为成功 revoke。第 2862 行 `DeleteGrantOutcome::Deleted | DeleteGrantOutcome::AlreadyMissing => Ok(1)` 是关键。
- **默认单连接**：与 `--all` 共用同一收敛路径，最后输出 `✓ Disconnected from <device_name> (grant: <short_id>)`。

### `--all` 汇总文案

- 每条：`✓ <short_id> (<device_name>)` 或 `✗ ... — <err>`。
- 汇总：`Revoked <deleted>/<total> connection(s).`
- `AlreadyMissing` 在 `--all` 路径下与 `Deleted` 共享 `✓` 文案，不再单独提示「already missing on relay」（这一优化 planned, not yet shipped as of 2026-07-03；当前只有 `--grant-id` 路径显式区分）。

## CLI

### 命令面

- `bifrost remote disconnect` — 断开默认（唯一）连接。
- `bifrost remote disconnect --all` — 遍历所有本地记录逐条断开。
- `bifrost remote disconnect --grant-id <gid>` — 断开指定 grant。
- `bifrost remote disconnect --relay-url <url>` — 覆盖 relay 地址（在 E2E 与 human tests 中广泛使用）。

### 输出规范

- `Deleted` / `AlreadyMissing` 均视为成功，均触发本地 `remote-connections.json` 清理。
- 输出文案遵循上一节「`handle_disconnect` 三条路径」中的规范。

## Web

不适用。本方案不改动 Web UI。

## Admin API

- 无新增端点。
- 依赖 relay 侧 `DELETE /v4/remote-invoke/grants/:grant_id`：
  - `2xx` — 成功删除。
  - `404` / body `grant_not_found` — 视为已删除。
- Legacy `v4` 路由已在 e2e 中被验证会返回 `404`（`TC-RI-08A`）。

## Sync 边界

- 本方案不涉及 sync。
- `remote-connections.json` 与 grant 状态均为本地文件，不参与账号同步。

## Phase 1：结果分类落地（已完成）

- 新增 `DeleteGrantOutcome` 与 `classify_delete_grant_failure`。
- `caller.delete_grant` 与 `handle_disconnect` 全面接入新分类。
- 输出文案区分 `Deleted` 与 `AlreadyMissing`（`--grant-id` 路径）。

## Phase 2：E2E 与 human tests 覆盖（已完成）

- `e2e-tests/tests/test_remote_invoke_e2e.sh` 追加 `TC-RI-08A`（第 1275/1277/1283 行附近）：
  - 手动 DELETE relay grant 制造 404。
  - 再执行 `bifrost remote disconnect --all --relay-url ...`。
  - 断言 `DISCONNECT_OUTPUT` 匹配 `already missing on relay|revoked|disconnected|✓` 其一。
  - 断言 `remote-connections.json` 连接数归零。
- `human_tests/remote-invoke.md`：
  - `TC-RI-回归-68`（第 2468 行）——底层路由 404 行为。
  - `TC-RI-回归-120`（第 3788 行）——`disconnect --all` 幂等清理场景与真实证据（第 4210 行执行记录）。

## Phase 3：`--all` 路径下的 AlreadyMissing 文案（规划中）

- 目标：让 `--all` 汇总中的每条记录能区分「relay 真删」与「本地补偿」，方便审计。
- 状态：(planned, not yet shipped as of 2026-07-03)。

## 测试方案

### 单元测试

- `crates/bifrost-cli/src/commands/remote.rs`：
  - `test_classify_delete_grant_failure_treats_404_as_already_missing`（第 8295 行）——验证 `404 grant_not_found` 被识别为 `AlreadyMissing`。
  - `test_classify_delete_grant_failure_rejects_other_errors`——验证 `500` 等错误不会被误判为可吞掉的本地成功。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh` `TC-RI-08A`：
  - Setup：本地起 relay + client + caller，完成一轮 `remote connect` 与审批。
  - Trigger：直接 `curl -X DELETE http://127.0.0.1:<relay>/v4/remote-invoke/grants/<gid>` 制造 `404 grant_not_found`。
  - Action：`bifrost remote disconnect --all --relay-url ...`。
  - Assert：`DISCONNECT_OUTPUT` 含 `already missing on relay|revoked|disconnected|✓`；`remote-connections.json` 中 `connections.len() == 0`。

### human_tests 真实场景

- `TC-RI-回归-68`（`human_tests/remote-invoke.md:2468`）——客户端侧 DELETE grant 不存在返回 404。
- `TC-RI-回归-120`（`human_tests/remote-invoke.md:3788`）——`disconnect --all` 幂等清理，真实执行记录见第 4210 行「TC-RI-回归-120 执行结果」，包含 2026-04-21 与 2026-04-23 两次证据。
- `human_tests/readme.md` 索引已同步。

## Review / Fix / Test 闭环

- 每次改动 `caller.delete_grant` 或 `handle_disconnect`：
  1. 补齐单元测试覆盖新分支。
  2. 更新 `TC-RI-08A` 断言集合（若新增了成功归因文案）。
  3. 在 `human_tests/remote-invoke.md` 追加实际执行记录并写明日期。
- 每次改 relay 路由：
  1. 保证 `404 grant_not_found` 语义不变（body 含关键字或状态码为 `404`）。
  2. 若 body 改动，需要同步更新 `classify_delete_grant_failure` 的 body 关键字匹配逻辑。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --e2e-only shell`

## 风险与决策

### 1. 吞掉真正的错误

- 风险：过于宽泛的 404 归因会掩盖真实的 relay 异常。
- 决策：`classify_delete_grant_failure` 只在 `status == 404` 或 body 含 `grant_not_found` 时归为 `AlreadyMissing`，其它错误必须原样上抛。

### 2. 幽灵连接残留

- 风险：本地记录与 relay 状态不一致，`remote status` 显示可用连接但实际无法调用。
- 决策：`Deleted` 与 `AlreadyMissing` 均触发本地 `retain + save`，形成幂等收敛。

### 3. `--all` 遍历中途失败

- 风险：其中一条非 404 失败会导致后续记录不被处理。
- 决策：`revoke_all_matching_grants()` 内部对每条独立错误处理，一条失败不阻塞其它条目；末尾汇总实际成功数。

### 4. 输出歧义

- 风险：用户无法分辨「relay 真的删了」还是「relay 已经不存在」。
- 决策：`--grant-id` 路径显式区分文案；`--all` 路径当前统一 `✓`，后续 Phase 3 可细化。

## 影响范围

### 必须修改的模块

- `crates/bifrost-cli/src/commands/remote.rs`（`DeleteGrantOutcome` / `classify_delete_grant_failure` / `handle_disconnect` / `revoke_all_matching_grants` / `caller.delete_grant`）。
- `e2e-tests/tests/test_remote_invoke_e2e.sh`（`TC-RI-08A`）。
- `human_tests/remote-invoke.md`（`TC-RI-回归-68`、`TC-RI-回归-120`）。
- `human_tests/readme.md`（索引）。

### 明确不改的范围

- Relay 端 `DELETE /v4/remote-invoke/grants/:grant_id` 路由与响应契约。
- `remote-connections.json` 文件格式。
- `remote connect` / `remote status` / `remote grant list` 命令行为。

## 依赖项

- 本地连接持久化：`remote-connections.json`。
- Relay grant 删除接口：`DELETE /v4/remote-invoke/grants/:grant_id`。
- 依赖 `bifrost_core::Result` 与 CLI 侧 `reqwest` 响应 body 解码。
