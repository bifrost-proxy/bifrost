# Remote Invoke Shell Access 只读 ID 方案

## 背景

Remote Invoke 的 Shell Access 能力允许用户在 Bifrost 上定义 shell profile（`profile-N`）与 shell policy（`policy-N`），并把 grant 与 profile / policy 绑定，形成「谁可以在什么 profile 上跑什么命令」的白名单模型。这些 ID 由 grant / policy binding / call history 直接引用，是稳定的外部标识。

早期 WebUI 允许用户在编辑器里直接改写 `Policy ID` / `Profile ID` 字段，并即时 slugify。这带来两个可用性和安全问题：

1. 用户以为 ID 是普通显示名，改完发现 slug 被自动转小写、去空格，产生「无法修改」的困惑。
2. 一旦真的把 ID 改掉，已有 grant / policy binding / call history 里对旧 ID 的引用立即失效，需要重建授权。

本方案把 WebUI 收敛到「Policy/Profile ID 只读 + Name 可改」的日常操作模型，避免误改；后端 `remote_shell.json` 数据结构与保存协议不变，CLI / 手工文件仍可作为高级运维手段。

> 实现校准（2026-06-16）：该方案已上线。`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx` 中 profile / policy 的 `id` 输入框均带 `readOnly`（profile 见 L3514，policy 见 L3789）；新建条目走顶层 `nextShellItemId(prefix, existingIds)`（L343），按 `profile-N` / `policy-N` 递增并跳过已存在 ID；旧的 slug/slugify 边输入边规范化逻辑已删除。E2E 回归 `web/tests/ui/admin-settings.spec.ts` L2135「Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读」已通过。真实场景用例 `human_tests/remote-invoke.md` 中的 TC-RI-回归-127 已记录 ✅ PASS。

## 用户目标验证清单

### 必须实现

- `Manage Shell Access` 对话框中已有 `Policy ID` / `Profile ID` 输入框 `readOnly`，用户无法通过键盘或粘贴修改。
- `Policy name` / `Profile name` 输入框保持可编辑并可保存。
- 新增 profile / policy 时前端自动分配 `profile-N` / `policy-N`，N 跳过已存在 ID。
- 后端 `remote_shell.json` 数据结构与保存协议不变，CLI / 手工编辑不受影响。
- Shell Access 相关行为字段（`enabled` / `exec_mode` / profile 关联 / allowlist / timeout）保持可编辑。

### 必须不破坏

- 已有 policy / profile / grant binding 引用的 ID 稳定。
- 保存后接口返回的 `RemoteShellSet` 中 ID 保持不变。
- `bifrost remote shell` CLI 及 `crates/bifrost-storage/src/remote_shell.rs` 结构不修改。
- 老 UI 使用者手工编辑过的 slug 型 ID 继续可用。

### 必须真实验证

- Playwright UI E2E 打开 Manage Shell Access → 预置 policy / profile 输入框 `readonly`。
- 只改 name 保存 → 后端返回的 ID 与保存前一致，只有 name 更新。
- CLI 侧仍可执行 `bifrost remote shell list` / `bifrost remote shell allow ...`。
- `human_tests/remote-invoke.md` TC-RI-回归-127 复跑通过。

## 产品语义

### ID 是外部稳定引用，不是显示名

Shell Access 中的 `Policy ID` / `Profile ID` 被以下位置引用：

- Grant → policy binding：grant metadata 记录了 `policy_id`。
- Call history / audit：`call_history_store.rs` 落盘 `policy_id / profile_id`。
- CLI 命令：`bifrost remote shell allow --policy <id>` / `bifrost remote shell profile --id <id>`。
- 后端 `remote_shell.json` schema 的 `id` 字段。

一旦 ID 变化，上述引用要么失效要么指向错误对象。因此 WebUI 一定要把 ID 定位成「只读外部键」，把改名需求引导到 `name` 字段。

### 新增 ID 生成策略

- profile 前缀 `profile-`，policy 前缀 `policy-`。
- 递增 `N` 跳过 `existingIds`，即使中间删除过条目也不复用旧编号，避免 grant 引用错乱。
- 生成后立即写到只读 input，用户无需手动填写。

### 不在后端强制只读

后端保存协议是 `RemoteShellSet` 全量覆盖，职责是结构校验；识别「本次是否改了某个旧 ID」超出后端语义范围。此外 CLI / 手工文件编辑仍应保留改 ID 的能力（作为运维手段），因此本轮只收窄 WebUI 的日常路径。

## 技术细节

### Web UI 交互（`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`）

- L343 `function nextShellItemId(prefix, existingIds)` 顶层辅助函数：遍历 `existingIds`，从 `1` 起递增到未占用编号返回 `${prefix}-${N}`。
- L3434 profile 新增按钮回调：`const nextId = nextShellItemId('profile', profiles.map(p => p.id))`。
- L3514 profile ID 输入框 `readOnly` prop 绑定。
- L3681 policy 新增按钮回调。
- L3690 policy 新增时同时用 `nextShellItemId('profile', ...)` 生成一个默认 profile id 并绑定。
- L3789 policy ID 输入框 `readOnly` prop 绑定。
- L4719 额外区块的 readOnly 标注（`Manage Shell Access` 外部 grant 面板）。

原先的 slug 边输入边规范化代码已被移除，防止再次误改。

### 后端契约（`crates/bifrost-storage/src/remote_shell.rs`）

- `RemoteShellSet { profiles: Vec<Profile>, policies: Vec<Policy> }` 结构不变。
- Profile / Policy 的 `id` 字段仍是 `String`，允许任意 slug。
- CLI 侧 `crates/bifrost-cli/src/commands/remote_shell.rs` 与 `cli/remote.rs` 不动。
- `crates/bifrost-admin/src/test_support/mod.rs` 中默认样本继续用 slug 型 id。

## CLI + Web + Admin API

### CLI（不变）

```
bifrost remote shell list
bifrost remote shell profile --id profile-1 --name "sandbox"
bifrost remote shell policy --id policy-1 --profile profile-1 --allow "ls,pwd"
```

### Web UI（本轮变更）

- 打开 `Settings → Remote Invoke → Shell Access → Manage Shell Access`。
- 已有 Profile / Policy 卡片的 ID 输入框灰底 + `readOnly`；hover 时提示「ID 不可修改，如需重建请新建后手工迁移」。
- Name / Description / enabled / exec_mode / allowlist / timeout 保持交互式编辑。
- 新增按钮点击后弹出的空白卡片自动填入 `profile-N` / `policy-N`。

### Admin API（不变）

- `GET /_bifrost/api/remote-invoke/shell-config` 返回全量 `RemoteShellSet`。
- `PUT /_bifrost/api/remote-invoke/shell-config` 覆盖保存，后端做结构校验。

## Sync 边界

- Shell Access 配置属于本机策略，`remote_shell.json` 保存在 `BIFROST_DATA_DIR`，不参与跨设备 sync。
- Grant 绑定的 `policy_id` 只在本机 Bifrost 内解析，不会被 Relay 感知。
- CLI 覆盖 ID 会影响本机所有引用者，需要用户自行迁移；WebUI 只读约束就是为了减少这种维护成本。

## Phase 1 – WebUI 只读约束

- 删除旧 slugify 编辑逻辑。
- profile / policy ID input 加 `readOnly`。
- 顶层 `nextShellItemId` 辅助函数产出稳定 ID。

## Phase 2 – 回归验证

- 新增 Playwright 用例覆盖只读约束。
- 更新 `human_tests/remote-invoke.md` 与 `human_tests/readme.md`。

## Phase 3 – 文档同步

- 更新 human_tests 索引与用例数。
- 无 API / 后端变更，无需迁移脚本。

## Phase 4 – 后续演进（不在本轮）

- 若未来需要「重建 grant 时批量替换 policy_id」，可在 Admin API 追加 `PATCH /shell-config/rename-id` 端点；本轮不做。

## 测试方案

### 单元测试

- 本轮改动集中在 React 交互层，不新增独立单元测试；依赖现有 TypeScript 类型检查与 lint。

### E2E 测试

- `web/tests/ui/admin-settings.spec.ts` L2135 用例「Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读」：
  - L2148 预置 `readonly-profile`、L2157 预置 `readonly-policy`。
  - L2217 打开 Manage Shell Access。
  - L2233 定位 profile ID input，L2234 定位 policy ID input。
  - L2236/L2239 断言 `HTMLInputElement.readOnly === true`。
  - L2254 保存后接口返回 `id === "readonly-profile"` 保持不变，只有 name 更新。

### 真实场景测试 `human_tests/remote-invoke.md`

- TC-RI-回归-127：
  - 在 Manage Shell Access 中确认 Policy / Profile ID 只读展示。
  - 修改 name 并保存，页面与 API 返回都显示 ID 未变化。
  - 新增 profile / policy 时 ID 自动生成为 `profile-N` / `policy-N`。
- `human_tests/readme.md` 索引更新 `remote-invoke.md` 用例数与说明。

## Review/Fix/Test 闭环

### 第 1 轮

- Review：确认 `RemoteInvokeTab.tsx` 中所有 profile/policy ID input 都加了 `readOnly`（grep `readOnly` 应命中 L3514、L3789、L4719）。
- Review：`nextShellItemId` 是否处理 `existingIds` 中含非数字后缀的旧 slug（跳过并从最大数字续增）。
- Test：`npx playwright test admin-settings.spec.ts -g "Shell Access 仅允许修改名称"`。

### 第 2 轮

- Review：CLI / 后端未意外加入 ID 校验拒绝老 slug。
- Review：`Manage Shell Access` 外部 grant 面板同一交互无回退（L4719）。
- Test：按 `human_tests/remote-invoke.md` TC-RI-回归-127 手工复跑。

## 风险与决策

- **为什么不让后端强制只读**：后端做全量覆盖保存，无法低成本识别「本次是否改了旧 ID」；CLI / 手工文件仍应保留能力，本轮只处理 WebUI 误导交互。若未来出现恶意 API 客户端强改 ID，需要在专门的 audit / diff 层拦截。
- **删除条目后编号不复用**：`nextShellItemId` 只查最大编号 + 1，避免历史 grant 引用错乱；副作用是长期使用后编号会「跳号」，但对可读性无实质影响。
- **重命名 vs 迁移**：若用户真的需要「换 ID」，推荐流程是：新增目标 ID → 手动把 grant 绑定切到新 ID → 删除旧 policy/profile。WebUI 不主动帮做迁移，避免误伤。
- **老 slug 型 ID 兼容**：文件里已有 `slug-style-id` 的老数据继续按 `readOnly` 展示，不做批量重命名；新条目才用 `profile-N` / `policy-N` 格式。
