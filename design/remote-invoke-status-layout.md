# Remote Invoke 状态区布局优化

> 状态：已实现 | 更新时间：2026-04-28

## 背景

Remote Invoke 设置页当前将 `Connection Status` 与 `Discovery Mode` 拆成两张独立卡片展示，而 `SSH Key` 单独占据右侧列。桌面宽屏下会出现左下空白区，导致首屏空间利用率偏低，信息扫描效率也不够好。

## 目标

1. 将 `Connection Status` 与 `Discovery Mode` 合并到同一张左侧卡片中。
2. 左右两列卡片在 `md` 及以上宽度下保持等高，避免出现明显空白。
3. 不改变现有状态展示、发现模式操作、SSH Key 管理能力与交互语义。
4. 从 `Remote Status` 卡片移除 `Active Calls` 计数，避免与底部 `Recent Calls` 中更明确的 caller/执行记录信息重复。
5. 未登录 Sync 服务时，`Remote Status` 卡片内的登录提示必须兼容暗色主题，不再使用固定浅色警告背景。
6. 在底部 `Grants` 列表中展示每条授权的连接方式，区分 `SSH key` 与 `Pair code`。

## 实现方案

### 布局调整

- 保留外层 `Row` 双列结构。
- 左列改为单张 `Remote Status` 卡片，内部按纵向两段展示：
  - 上半区：`Connection Status`
  - 下半区：`Discovery Mode`
- 右列 `SSH Key` 卡片保持独立，但通过列容器与卡片 `height: 100%` 拉伸到与左列同高。

### 交互与可测试性

- 为合并后的卡片和两个内部区块补充稳定的 `data-testid`：
  - `settings-remote-invoke-status-card`
  - `settings-remote-invoke-connection-section`
  - `settings-remote-invoke-discovery-section`
  - `settings-remote-invoke-ssh-card`
- 内部保留原有按钮与状态文案，避免影响已有操作流。
- `Connection Status` 仅保留 relay 连接、实例、设备与 Sync Account 等静态状态；正在执行/已执行调用继续由底部 `Recent Calls` 负责展示，其中 caller 信息以 `by <caller>` 和详情弹窗中的 `Caller` 字段呈现。
- 未登录 Sync session 时显示的锁定提示使用 Ant Design 主题 token（`colorWarningBg` / `colorWarningBorder` / `colorWarningText` / `colorWarning`），随全局 light/dark algorithm 自动切换，避免暗色主题下出现硬编码浅黄色块。
- `Grants` 列表读取 grant API 已返回的 `auth_method` 字段：
  - `ssh_publickey` 显示为 `SSH key`
  - `pair_code` 显示为 `Pair code`
  - 旧数据缺失 `auth_method` 但存在 `ssh_key_fingerprint` 或 `ssh_key_id` 时按 `SSH key` 兜底
  - 其他未知值按原始值展示，方便排查协议漂移
- 为 `Grants` 卡片补充 `settings-remote-invoke-grants-card` 测试定位点。

## 测试方案

### 单元测试

- 本次改动为纯布局重组，不新增独立业务函数；不单独新增单元测试。

### E2E 测试

- 更新 `web/tests/ui/admin-settings.spec.ts`
- 验证点：
  - Remote Invoke Tab 可正常打开
  - 合并后的状态卡片同时包含 `Connection Status` 与 `Discovery Mode`
  - `Connection Status` 不再包含 `Active Calls`
  - `SSH Key` 卡片仍独立存在
  - `Enter Discovery Mode` 按钮仍可见
  - `Grants` 列表同时能展示 `SSH key` 与 `Pair code` 两类连接方式标签
- 新增未登录 Sync + dark theme 回归：
  - 预置 `bifrost-theme=dark`
  - mock `/_bifrost/api/sync/status` 返回 `has_session=false`
  - 断言 `Remote Status` 登录提示可见，且提示块背景/边框不再等于浅色主题硬编码色值

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增布局回归用例，验证：
  - 左侧状态卡片已合并
  - `Remote Status` 卡片不再展示 `Active Calls`
  - `Recent Calls` 中仍保留 caller 信息
  - `Grants` 列表能看出每条 grant 是 SSH key 连接还是 pair code 连接
  - 宽屏下左右卡片首屏占满，无明显孤立空白块
  - Discovery Mode 操作区仍可正常识别与点击
  - 未登录 Sync 服务时，暗色主题下登录提示仍与卡片背景协调，提示文案和 Sync Server 地址可读

## 校验要求

- 执行相关 Web UI E2E 测试
- 按 `human_tests/remote-invoke.md` 新增用例进行真实场景核对

## 文档更新要求

- 同步更新 `human_tests/readme.md` 中 `remote-invoke.md` 的用例数与说明

## 补充说明

- `Create SSH key` 弹窗中的说明文案改为单条合并提示，集中表达两个约束：
  - 同一时间只支持一个 active SSH key
  - SSH grant 不受时间型 TTL 控制，只有 rotate 或 revoke 才会失效
