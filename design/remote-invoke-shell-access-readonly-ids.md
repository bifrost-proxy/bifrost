# Remote Invoke Shell Access 只读 ID 方案

> 状态：已实现 | 更新时间：2026-06-16
>
> 当前实现校验（2026-06-16）：
> - `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx` 中已有 profile 与 policy 的 `id` 输入框均带 `readOnly`（profile 见 L3513、policy 见 L3788）。
> - 新建条目走顶层 `nextShellItemId(prefix, existingIds)`（L343-L354），按 `profile-N` / `policy-N` 递增并跳过已存在 ID。
> - 文件中已不存在 `slug` / `slugify` 相关逻辑，确认旧的边输入边规范化已删除。
> - E2E 回归 `web/tests/ui/admin-settings.spec.ts`（用例名「Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读」，L1623 起）验证 `Manage Shell Access` 弹窗里预置的 `readonly-profile` / `readonly-policy` 输入框带 `readonly`，且仅 name 字段可改并保存。
> - 真实场景用例 `human_tests/remote-invoke.md` 中的 TC-RI-回归-127 已记录 ✅ PASS（详见同文件 TC-RI-回归-127 执行结果章节）。

## 背景

Remote Invoke 的 `Shell Access` 编辑器当前允许在 Web UI 中直接修改 `Policy ID` 与 `Profile ID`。虽然前端会把输入即时规范化成 slug，但这仍会让用户误以为这些字段是普通可编辑名称，容易引发两类问题：

- 用户试图把 `Policy ID` 当展示名称修改，结果被自动改写，产生“无法修改”的困惑
- 现有 grant / policy binding 依赖稳定的 policy/profile 标识，误改 ID 会导致已有授权与本地配置脱节

因此本轮将 Web UI 调整为：**已有 policy/profile 的 ID 只读，仅允许修改 name/description 等展示与行为字段。**

## 目标

1. 已存在的 `Policy ID` 与 `Profile ID` 在编辑器中只读展示，不再允许手工改写。
2. `Policy name` 与 `Profile name` 继续可编辑，满足日常命名维护需求。
3. 新增 policy/profile 时仍自动分配稳定初始 ID，避免因为只读约束造成新增条目无法保存。
4. 不改变后端 `remote_shell.json` 数据结构与保存协议，保持向后兼容。

## 实现方案

### Web UI 交互约束

- 在 `Manage Shell Access` 对话框中：
  - 已有 `Policy ID` 输入框改为 `readOnly`
  - 已有 `Profile ID` 输入框改为 `readOnly`
- `Policy name` / `Profile name` 输入框保持可编辑。
- 其它行为字段（enabled、exec_mode、profile 关联、allowlist、timeout 等）保持原有编辑能力。

### 新增条目 ID 生成

- 删除原先“边输入边 slugify”的 ID 编辑逻辑。
- 新增 policy/profile 时，前端使用本地辅助函数生成唯一 ID：
  - profile: `profile-N`
  - policy: `policy-N`
- 生成逻辑会跳过已存在的 ID，避免删除后再新增时发生重复。

### 为什么不在后端强制只读

- 后端目前接受完整 `RemoteShellSet` 覆盖保存，职责是做结构校验，而不是识别“本次是否改了某个旧 ID”。
- 本轮目标是修复 Web UI 误导性交互，而不是收紧底层存储协议。
- CLI / 手工文件编辑仍可作为高级运维手段保留，Web UI 则收敛到更安全的日常操作模型。

## 影响范围

- 前端文件：
  - `web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`
- UI E2E：
  - `web/tests/ui/admin-settings.spec.ts`
- 真实场景测试：
  - `human_tests/remote-invoke.md`
  - `human_tests/readme.md`

## 测试方案

### 单元测试

- 本次主要是 React 交互层调整，不新增独立单元测试。
- 通过现有类型检查与诊断确保前端改动无编译错误。

### E2E 测试

- 更新 `web/tests/ui/admin-settings.spec.ts`
- 验证点：
  - Remote Invoke -> Manage Shell Access 可正常打开
  - 预置 policy/profile 的 ID 输入框带有 `readonly`
  - 可编辑并保存 `Policy name` / `Profile name`
  - 保存后后端返回的新配置中，ID 保持不变，仅 name 更新

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增回归用例验证：
  - Web UI 中 `Policy ID` / `Profile ID` 只读展示
  - 名称字段仍然可编辑并成功保存
  - 保存后页面与接口都显示原 ID 未变化

## 校验要求

按顺序执行：

1. 运行针对性的 Playwright UI E2E
2. 按 `human_tests/remote-invoke.md` 新增用例逐条执行真实场景验证
3. `cargo fmt --all -- --check`
4. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
5. `cargo test --workspace --all-features`
6. `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 更新 `human_tests/remote-invoke.md`
- 更新 `human_tests/readme.md` 中 `remote-invoke.md` 的说明与用例数
