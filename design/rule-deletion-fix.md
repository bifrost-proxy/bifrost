# 规则删除阻塞修复

## 背景

Bifrost 支持把本地 `.bifrost` 规则和远端 sync 后台的规则合流展示。当一条规则已经和 sync 后端建立映射（`RuleFile.sync.remote_id.is_some()`），如果本地想删除它，`delete_rule` handler 需要先向 sync 后端登记一条“已删除”的 tombstone，避免下次同步时被远端再拉回。

历史实现在 `crates/bifrost-admin/src/handlers/rules.rs::delete_rule` 里，如果 `state.sync_manager` 为 `None`（例如用户未登录 sync，或者临时把 sync 关掉），会直接返回 `503 SERVICE_UNAVAILABLE` 并中止后续 `state.rules_storage.delete(name)`。表现出来的现象是：

1. Rules 列表上点删除按钮，前端看似“操作失败”但没有明确提示。
2. 磁盘上 `.bifrost` 文件仍然存在，`bifrost rule list` 依然能看到。
3. 因为文件还在，代理进程刷 rules 后继续匹配这条规则，最典型的表现是“我明明删掉了，怎么还在拦截”。
4. Enable/Disable 切换会因缓存与磁盘状态不一致出现视觉抖动。

这是一个把“可选远端记账”当成“删除前置条件”的顺序错误。正确语义应该是：本地规则的生命周期由本地 storage 拥有，sync tombstone 是尽力而为的辅助路径。

## 用户目标验证清单

### 必须实现

- `DELETE /api/rules/:name` 在 `sync_manager` 未配置时仍能成功删除有 `remote_id` 的规则，返回 200。
- `sync_manager` 存在但 `record_deleted_rule` 失败时，也不能阻断本地 delete；错误必须写入 warn 日志。
- 本地 storage delete 之后必须触发 `notify_rules_changed` 与 `invalidate_overview_cache`，让代理侧和 Web overview 立即感知。
- 前端 `useRulesStore.deleteRule` 收到 API 错误时必须调用 `normalizeApiErrorMessage` 并通过 `antd` 的 `message.error` 展示给用户，不再静默失败。
- CLI `bifrost rule delete <name>` 与 Admin API 语义一致：本地 delete 是主路径，sync 状态是次要路径。
- Default rule 保护逻辑保留：`RulesStorage::is_default_rule_name(name)` 命中直接返回 400，不进入 sync/local 删除分支。

### 必须不破坏

- 有 sync 且 sync 正常时，`record_deleted_rule` 仍然被调用，tombstone 依旧上报。
- Delete 完成后 `sync_manager.trigger_sync()` 仍会被调用，把本地删除动作快速推给远端。
- 未同步的普通规则删除路径不受影响。
- Group rules、Default rule、临时端口绑定规则的 delete 语义不变。
- Rules 列表、编辑器、traffic matched-rules 展示仍然实时刷新。

### 必须真实验证

- 关闭 sync（不登录 / 显式禁用）后，创建带 `remote_id` 的规则（可通过 sync 后再离线复现）→ 删除 → 列表和磁盘同时消失。
- 打开 sync 但 sync API 主动返回错误时，本地删除仍成功，日志能看到 warn。
- Web UI 前端在 API 失败时看到 `message.error` toast，不再是无反馈。

## 产品语义

**本地 storage 是规则的 source of truth。** sync 只是本地状态的复制通道。删除动作的成败必须由本地 storage 决定，任何远端依赖都必须是 best-effort。

新语义可以概括为下面的伪代码：

```rust
async fn delete_rule(state, name, push) -> Response {
    if is_default_rule_name(name) {
        return 400 "Default rule cannot be deleted";
    }
    let rule = match load(name) { Ok(r) => r, Err(_) => return 404 };

    if rule.sync.remote_id.is_some() {
        if let Some(sm) = state.sync_manager.clone() {
            if let Err(e) = sm.record_deleted_rule(&rule).await {
                warn!("record_deleted_rule failed, proceeding with local delete");
            }
        } else {
            warn!("sync manager not available, skipping remote deletion record");
        }
    }

    match state.rules_storage.delete(name) {
        Ok(_) => {
            if let Some(sm) = state.sync_manager.clone() { sm.trigger_sync(); }
            notify_rules_changed(&state);
            invalidate_overview_cache(&push);
            200
        }
        Err(e) => storage_error_status(&e),
    }
}
```

关键点：

- Missing sync_manager 从“错误”降级为“warn 日志”。
- `record_deleted_rule` 返回 Err 也不阻断，仅记录 warn。
- 只有 `state.rules_storage.delete(name)` 的 `Err` 才映射到 HTTP 错误码。

## 技术细节

### 后端 handler

- 位置：`crates/bifrost-admin/src/handlers/rules.rs::delete_rule`（约 1488-1541 行）。
- 变化点：
  - `if rule.sync.remote_id.is_some()` 分支不再返回 503，改为 `tracing::warn!(target: "bifrost_admin::rules", rule = %name, "sync manager not available, skipping remote deletion record for synced rule")`。
  - `record_deleted_rule` 的 `Err` 分支也从早退改为 warn。
  - `state.rules_storage.delete(name)` 的错误仍走 `storage_error_status(&e)` 转 4xx / 5xx。
- notify / cache：`notify_rules_changed(&state)` 广播 SSE `rules_changed` 事件；`invalidate_overview_cache(&push_manager)` 让 overview 立即重算规则数量。
- Sync trigger：`sync_manager.trigger_sync()` 在删除成功后仍然被调用，异步把 tombstone 推给远端（如果 sync 存在）。

### 前端 store

- 位置：`web/src/stores/useRulesStore.ts`（`deleteRule` 大约 441-510 行）。
- 变化：
  - `catch (e)` 分支里调用 `normalizeApiErrorMessage(e, 'Failed to delete rule')`，返回的字符串通过 `antd` 的 `message.error(msg)` 弹出。
  - 保留原本的 `isNotFoundError` / `isConnectionIssueError` 判断，避免 not-found 场景给出误导 toast。
  - 删除成功仍触发 `refreshRules()` 与 SSE 触发的 store 更新。
- 兼容性：`normalizeApiErrorMessage` 已经处理了 fetch/axios 两种客户端错误格式，本改动不新增依赖。

### CLI 路径

- `bifrost rule delete <name>` 底层复用 Admin API，因此后端修复自动覆盖 CLI。
- 需要保留 Default rule 保护：CLI 前也应该在本地做一次 `is_default_rule_name` 拦截，避免多余的 HTTP round-trip。

### Sync 边界

- Sync 未启用：走 warn 日志，不阻塞本地删除。
- Sync 已启用但远端不可达：`record_deleted_rule` 返 Err → warn → 本地仍删除 → `trigger_sync` 后台重试。
- Sync 已启用且成功：正常上报 tombstone，删除后再一次 `trigger_sync` 确保上下游一致。
- Group rules 复用同一 handler 语义；Group 与 sync 关系目前仍走 sync_manager 的通用流程，不做特殊处理。

## CLI / Web / Admin API

### CLI

```bash
# 直接删除本地/已同步规则，无需 --force
bifrost rule delete some-rule

# 删除 Default 会被拦截
bifrost rule delete Default
# → Default rule cannot be deleted
```

退出码：0 成功、非 0 表示 storage 层错误或 not-found。

### Web UI

- Rules 列表右键 / 更多菜单里的 Delete。
- 删除失败时右上角弹出 `antd` `message.error(...)`；成功时无 toast，靠列表自动刷新。
- Default 行的删除入口保持禁用（`can_delete=false`）。

### Admin API

- `DELETE /api/rules/:name`
  - 200 `{ "message": "Rule '<name>' deleted successfully" }`
  - 400 `Default rule cannot be deleted`
  - 404 `Rule '<name>' not found`
  - 4xx/5xx 来自 `storage_error_status`
- 已废弃：503 `sync manager not available`（本修复后不再出现）

## 实现切分

### Phase 1：后端解耦

- 修改 `delete_rule` 中的 sync 分支为 warn。
- 保留 default 保护 / not-found 分支。
- 补一个 handler 级单元测试 `test_delete_synced_rule_without_sync_manager`（在 `crates/bifrost-admin` 内构造一个不含 `sync_manager` 的 `AdminState`，创建带 `remote_id` 的 rule，断言 200 且磁盘文件消失）。
  - 状态：**planned, not yet shipped as of 2026-06-17**。当前仓库唯一命中该测试名的文件仍是本设计文档本身。

### Phase 2：前端错误暴露

- 更新 `useRulesStore.deleteRule` 的 catch 分支使用 `normalizeApiErrorMessage` + `message.error`。
- 保留 not-found 静默处理，避免用户看到无意义 toast。

### Phase 3：E2E 与人工场景

- 新增 `e2e-tests/tests/test_rule_delete_without_sync.sh`（planned）：
  - 启动 bifrost 使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、非 9900 端口。
  - 通过 Admin API 创建规则、手工写入 `sync.remote_id = "fake"`（或复用 `state.rules_storage` fixture），DELETE → 断言 200、magasin 文件消失。
- 更新 `human_tests/rule-deletion-fix.md`（planned）：TC-RDF-01～04 覆盖 sync-off、sync-error、UI toast、Default 保护。

### Phase 4：文档与日志

- 后端日志加 `target = "bifrost_admin::rules"`，便于 `bifrost log grep rules` 定位。
- 更新 `docs/rule.md` 中删除章节，说明本地删除不再依赖 sync。

## 测试方案

### 单元测试

- `crates/bifrost-admin`：
  - `test_delete_synced_rule_without_sync_manager` — planned, not yet shipped as of 2026-06-17（正是本次要补的）。
  - `test_delete_synced_rule_with_failing_sync` — planned；用 mock `SyncManager` 让 `record_deleted_rule` 返回 Err，断言 200。
  - `test_delete_default_rule_returns_400`（已有的 default 保护回归，验证保护路径未被本次修复破坏）。
- `crates/bifrost-storage`：`RulesStorage::delete` 现有的 not-found / IO 错误测试保持不变。
- 前端：`web/src/stores/useRulesStore.test.ts` 中补一个 `deleteRule` catch 分支的单元测试（planned）。

### E2E

- `e2e-tests/tests/test_rule_delete_without_sync.sh`（planned）。
- 现有 `e2e-tests/test_rules.sh` 的 delete 场景保持通过，作为回归基线。

### 真实场景 human_tests

`human_tests/rule-deletion-fix.md`（planned, not yet shipped as of 2026-06-17）：

- TC-RDF-01：sync 未登录，创建规则并同步（离线复现），断开 sync → CLI delete → 200 + list 消失 + 磁盘文件消失。
- TC-RDF-02：sync 登录中但远端 500，CLI delete → 200 + warn 日志中包含 `record_deleted_rule failed`。
- TC-RDF-03：Web UI 删除失败时看到 `message.error` toast。
- TC-RDF-04：Default 规则删除返回 400，错误消息稳定。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin rules`
- 本机 no-local-coverage 约定生效，不跑 `make coverage`；交付时说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- Review：`delete_rule` handler 三种 sync 分支覆盖是否完备（no sync manager / sync manager Err / sync manager Ok）；notify + cache invalidate 是否位于所有成功分支之后；前端 catch 分支是否兼容 not-found。
- 复测：单元测试 + 手工 CLI delete。
- 检查：`grep -R "SERVICE_UNAVAILABLE" crates/bifrost-admin/src/handlers/rules.rs` 应该不再命中 delete_rule 分支。

### 第 2 轮

- Review：warn 日志是否包含足够上下文（rule 名、error）；前端 toast 是否会在 not-found 场景误弹；trigger_sync 是否仍在 sync 存在分支被调用。
- 复测：`bifrost-admin` 单元测试重跑；关闭 sync 的 E2E 场景重跑。
- 项目级：`cargo clippy` + `cargo test --workspace --all-features`。

## 风险与决策

- **静默丢失 tombstone 风险**：当 sync 不可用时删除会导致远端仍保留旧规则；下次 sync 恢复时，远端会把它拉回本地。缓解：`trigger_sync` 成功后由 sync 后台补账；同时在 warn 日志里保留 rule 名，便于诊断。
- **前端 toast 冗余风险**：`message.error` 与列表刷新可能同时出现给出双重反馈。已通过 not-found 静默兜住主要噪音场景。
- **权限与租户**：Admin API 复用 CSRF/session 中间件，本次改动不涉及授权路径。
- **兼容性**：老客户端可能仍在处理 503。因为 503 只是被替换成 200，老客户端只会看到删除“突然成功了”，不会破坏协议。
