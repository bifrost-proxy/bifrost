# Sync Settings Remote URL 草稿保留方案

## Background

Bifrost Web UI 的 `Settings → Sync` 页展示当前 Sync 状态（连接、登录、上次同步），并允许用户编辑 Sync 远端服务 URL。为了让状态卡片、按钮和全局 Sync store 保持实时，Settings 组件在 Sync tab 或 Remote Invoke tab 打开时会以 2 秒周期轮询 `/_bifrost/api/sync/status`。

修复前的路径存在一个严重可用性 Bug：状态轮询在每次刷新时把 `status.remote_base_url` 直接写回 URL 输入框的 draft state。当用户正在敲新的远端地址时，下一次轮询会用旧的服务器值覆盖掉本地未保存的编辑，导致输入框看上去“打不了字”。

本方案通过引入 dirty 追踪与显式“何时把服务器值写回 draft”的开关，让被动轮询不再吃掉用户草稿，同时保留 Save 成功后写回权威值的行为。

## Product Objectives Verification Checklist

### Must implement

- `syncRemoteBaseUrlDraft` 作为 controlled input value，被 URL 输入组件绑定。
- `syncRemoteBaseUrlDirtyRef` 追踪未保存的本地编辑；输入 onChange 立即置 dirty=true。
- `applySyncStatus(status, options)` 是所有 Sync 状态更新的唯一入口。
- 被动状态刷新（周期轮询、tab 切换、静默恢复）：更新状态卡片、按钮、全局 Sync store，但只有 draft 未 dirty 时才写回 URL draft。
- 成功的 Save 动作调用 `applySyncStatus(..., { syncRemoteBaseUrlDraft: true })`：服务器返回的新 URL 覆盖 draft，dirty 标记清零。
- 其他成功的 Sync 操作（Enable Sync、Auto Sync、Sign In、Sign Out、Sync Now）都刷新状态，但不强制覆盖 URL draft；draft 干净时反映服务器最新 URL，draft 脏时保留本地编辑。

### Must not break

- Sync tab 上其它字段（会话状态、上次同步时间、错误提示、按钮 loading 态）依旧随轮询更新。
- 全局 Sync store（订阅其它组件用于展示登录状态）继续被 `applySyncStatus` 同步。
- 不改变 `/sync/status` / `/sync/config` / `/sync/enable` 等后端 API 契约。
- Remote Invoke tab 打开时的联动轮询行为不变。

### Must be really verified

- 真实 Playwright 用例：mock `/sync/status` 轮询、开输入框敲字、把 mocked status 从 unauthorized 切到 ready，assert 输入框仍是用户 draft。
- 同一用例继续点击 Save，assert `PUT /sync/config` 收到用户 draft，assert 响应 URL 写回输入。
- 真实手测 `TC-WST-39` 覆盖 draft 保护流程。

## Product Semantics

### draft 与 status 谁是权威

- URL 输入框的显示值 = `syncRemoteBaseUrlDraft`。
- 服务器返回的 URL 只在两种情况下能覆盖 draft：
  1. 用户主动点 Save 成功。
  2. draft 从未被用户改过（dirty=false），此时保持“UI 显示与后端一致”的默认体验。
- 其他任何情况（周期轮询、tab 切换、Sign In / Sign Out 后台刷新等）都不能踩草稿。

### dirty 标记生命周期

- 初始 dirty=false。
- 用户输入 onChange → dirty=true。
- Save 成功 → dirty=false。
- 手动“重置为服务器值”按钮（若存在）→ dirty=false 并同步 draft。
- 页面 unmount → 组件级 state 自然销毁，无跨会话遗留。

## 技术细节

### 关键实现（`web/src/pages/Settings/index.tsx`）

- 组件 state 与 ref：
  ```ts
  const [syncRemoteBaseUrlDraft, setSyncRemoteBaseUrlDraft] = useState("");
  const syncRemoteBaseUrlDirtyRef = useRef(false);
  ```
- 唯一状态入口 `applySyncStatus`（line 214）：
  ```ts
  const applySyncStatus = useCallback(
    (status: SyncStatus, options?: { syncRemoteBaseUrlDraft?: boolean }) => {
      // 更新其它 UI state 与全局 Sync store。
      if (options?.syncRemoteBaseUrlDraft || !syncRemoteBaseUrlDirtyRef.current) {
        setSyncRemoteBaseUrlDraft(status.remote_base_url ?? "");
        syncRemoteBaseUrlDirtyRef.current = false;
      }
    },
    [/* deps */]
  );
  ```
- 输入 onChange（line 230 附近）：
  ```ts
  syncRemoteBaseUrlDirtyRef.current = true;
  ```
- Save 成功后（line 932 附近）：
  ```ts
  const remoteBaseUrl = syncRemoteBaseUrlDraft.trim();
  // PUT /sync/config ...
  applySyncStatus(status, { syncRemoteBaseUrlDraft: true });
  ```
- 其它成功动作（Sign In 909、Sign Out 922、Enable Sync 976、Auto Sync 963、Sync Now 952）都调用 `applySyncStatus(status)`，不带 `syncRemoteBaseUrlDraft: true`，保留 draft。

### 依赖文件

- `web/src/pages/Settings/index.tsx` — draft state、dirty ref、`applySyncStatus`、Save/Sign In/Sign Out 等 handler。
- `web/src/pages/Settings/tabs/SyncTab.tsx` — URL 输入受控组件，读取 `remoteBaseUrlDraft` prop（line 1339 附近传入）。
- `web/src/api/sync.ts` — `getSyncStatus/setSyncConfig` 类型。
- 后端 API 契约无变化。

## CLI / Web / Admin API 边界

- CLI：不涉及；Sync CLI 命令读写的是后端存储，不与 Web draft 有关。
- Web：只在 Settings 页组件级 state 中处理 draft 保护。
- Admin API：无契约变更；仍然使用 `/sync/status` 只读、`/sync/config` 写入。

## Sync 边界

本方案本身就在处理 Sync 设置页。dirty 保护只作用于本地 UI state，不影响：

- 多设备 Sync 状态同步（依然由后端 `/sync/status` 主导）。
- Sync 后端 URL 的持久化（依然通过 `/sync/config` 写入）。
- Sync store 全局订阅者的更新时机。

## Phase 1 - 4

### Phase 1：draft 与 dirty 状态

- 引入 `syncRemoteBaseUrlDraft` 与 `syncRemoteBaseUrlDirtyRef`。
- 输入 onChange 置 dirty=true。

### Phase 2：applySyncStatus 收口

- 抽出唯一入口 `applySyncStatus(status, options)`。
- 将 Settings 组件内所有 `setSyncStatus / setSyncRemoteBaseUrlDraft` 更新点改为 `applySyncStatus`。
- 只在 `options.syncRemoteBaseUrlDraft` 为 true 或 draft 干净时覆盖 draft。

### Phase 3：动作路径接入

- Save 成功走 `{ syncRemoteBaseUrlDraft: true }`。
- Enable / Auto / Sign In / Sign Out / Sync Now 走默认（不带 `syncRemoteBaseUrlDraft`）。
- 轮询 effect（useEffect 内 setTimeout / setInterval）也走默认，不覆盖 draft。

### Phase 4：测试与手测

- 新增 Playwright 用例 `web/tests/ui/admin-settings.spec.ts`。
- `human_tests/webui-settings.md` 新增 `TC-WST-39` 并真实执行。

## 测试方案

### UI / Unit Test

`web/tests/ui/admin-settings.spec.ts`：

- Mock `/sync/status` 周期轮询：初始 `unauthorized`；用户键入 draft；切换 mock 到 `ready`；assert 输入框仍是用户 draft。
- 点击 Save；assert `PUT /sync/config` payload 中 `remote_base_url` 是用户 draft。
- 断言 Save 响应的 URL 写回输入框，dirty 标记清零后下一次轮询再次可自动覆盖。

### E2E Test

- 跑 Settings Sync draft preservation 的 focused Playwright 用例；确认页面不再回滚 draft。

### human_tests

- `human_tests/webui-settings.md` `TC-WST-39`（line 537 附近，已落地）：
  1. 打开 Settings → Sync。
  2. 在 Remote URL 输入 `https://sync.example.test/staging`。
  3. 等待 3~5 次轮询周期（≈6~10 秒），期间不点 Save。
  4. 预期：输入框仍为 `https://sync.example.test/staging`，不被回滚。
  5. 点击 Save，assert `/sync/config` 收到该值，随后 draft 与服务器一致。

### 收尾校验

- 前端 lint / typecheck。
- `rust-project-validate`（若涉及后端变更；本方案纯前端可跳过）。
- 覆盖率遵循仓库约定；本地 no-local-coverage 时说明豁免。

## Review / Fix / Test Loop

### Round 1

- 复核用户症状：轮询覆盖 draft。
- Review：dirty ref lifetime、被动轮询 vs 显式动作、`applySyncStatus` 是否为唯一入口。
- 复测：跑 focused Playwright 用例；修任何 failing assertion。

### Round 2

- Review 第 1 轮修复后的 diff。
- 验证 `human_tests/webui-settings.md` 索引与本设计一致。
- 复跑 focused Playwright 用例与相关 web check。

## 风险与决策

- 若未来引入 “多用户会话同时编辑” 的场景，需要在 dirty 之外增加冲突检测（例如服务器版本号比对）。本方案 v1 仅解决“自己轮询覆盖自己 draft”问题。
- Save 失败时 dirty 保持 true 是合理默认；用户可以继续编辑并重试，而不是回滚到旧值。
- 如果引入 Server-Sent Events 推 status，仍应通过 `applySyncStatus` 收口，保持 dirty 语义一致。
- 回滚方案：Revert `applySyncStatus` 改动，恢复直接 `setSyncStatus + setSyncRemoteBaseUrlDraft` 分散更新；不推荐，会重现原 Bug。

## Documentation

- `design/sync-settings-draft-preservation.md`（本文件）。
- `human_tests/webui-settings.md`（`TC-WST-39`）。
- `human_tests/readme.md`（用例索引）。
