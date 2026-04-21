# Remote Invoke 状态区布局优化

> 状态：已实现 | 更新时间：2026-04-21

## 背景

Remote Invoke 设置页当前将 `Connection Status` 与 `Discovery Mode` 拆成两张独立卡片展示，而 `SSH Key` 单独占据右侧列。桌面宽屏下会出现左下空白区，导致首屏空间利用率偏低，信息扫描效率也不够好。

## 目标

1. 将 `Connection Status` 与 `Discovery Mode` 合并到同一张左侧卡片中。
2. 左右两列卡片在 `md` 及以上宽度下保持等高，避免出现明显空白。
3. 不改变现有状态展示、发现模式操作、SSH Key 管理能力与交互语义。

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

## 测试方案

### 单元测试

- 本次改动为纯布局重组，不新增独立业务函数；不单独新增单元测试。

### E2E 测试

- 更新 `web/tests/ui/admin-settings.spec.ts`
- 验证点：
  - Remote Invoke Tab 可正常打开
  - 合并后的状态卡片同时包含 `Connection Status` 与 `Discovery Mode`
  - `SSH Key` 卡片仍独立存在
  - `Enter Discovery Mode` 按钮仍可见

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`
- 新增布局回归用例，验证：
  - 左侧状态卡片已合并
  - 宽屏下左右卡片首屏占满，无明显孤立空白块
  - Discovery Mode 操作区仍可正常识别与点击

## 校验要求

- 执行相关 Web UI E2E 测试
- 按 `human_tests/remote-invoke.md` 新增用例进行真实场景核对

## 文档更新要求

- 同步更新 `human_tests/readme.md` 中 `remote-invoke.md` 的用例数与说明

## 补充说明

- `Create SSH key` 弹窗中的说明文案改为单条合并提示，集中表达两个约束：
  - 同一时间只支持一个 active SSH key
  - SSH grant 不受时间型 TTL 控制，只有 rotate 或 revoke 才会失效
