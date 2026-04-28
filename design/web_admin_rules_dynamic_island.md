# Web Admin Rules Dynamic Island

## 功能模块说明

Rules 页面顶部的 Dynamic Island 用于展示当前生效规则数量、规则来源分组、变量冲突提示，以及展开后的 Merged Rules 完整文本。

## 本次变更

展开 Merged Rules 后，在代码框右上角提供一键复制按钮。按钮复制当前合并规则文本的 `trim()` 结果，成功后显示短暂的成功反馈，失败时显示错误提示。

后续回归发现，仅凭 async Clipboard API 返回成功不足以证明文本已经进入可粘贴剪贴板；部分嵌入浏览器环境会出现 Toast 成功但实际粘贴失败。因此 Web 模式复制路径必须优先使用用户点击手势内的同步 `execCommand('copy')`，再退回 async Clipboard API。

## 实现逻辑

- 在 `web/src/pages/Rules/RulesDynamicIsland.tsx` 中复用现有 `copyToClipboard()` 工具，兼容桌面壳写入、Web 同步复制路径与 async Clipboard API fallback。
- `web/src/utils/clipboard.ts` 在 Web 模式下优先使用 `execCommand('copy')`，避免 async Clipboard API 在嵌入浏览器里假成功。
- Merged Rules 代码框改为相对定位容器，复制按钮绝对定位到右上角，不改变合并规则文本内容。
- 复制成功后按钮图标短暂切换为成功状态；当 merged content 更新时清理成功状态。
- 为 Dynamic Island 触发器、Merged Rules toggle、内容区域和复制按钮增加稳定 `data-testid`，支撑 UI E2E 与真实场景验证。

## 依赖项

- `antd` Button / Tooltip / message
- `@ant-design/icons` CopyOutlined / CheckOutlined
- `web/src/utils/clipboard.ts`

## 测试方案

### 单元测试

本次没有新增独立纯函数；复制逻辑依赖浏览器剪贴板与 UI 事件，覆盖重点放在 Playwright UI E2E。

### E2E 测试

- `web/tests/ui/admin-rules-values.spec.ts`
  - 创建一个启用规则。
  - 打开 Rules 页面并展开 Dynamic Island。
  - 展开 Merged Rules，点击右上角复制按钮。
  - 读取浏览器剪贴板，断言内容等于页面展示的 merged rules 文本。

### 真实场景测试

- 更新 `human_tests/webui-rules.md`
  - 新增 `TC-WRU-39`：Dynamic Island Merged Rules 一键复制。
  - 按用例真实执行：创建/启用规则，展开 Dynamic Island，展开 Merged Rules，点击复制按钮，粘贴或读取剪贴板验证内容一致。
- 更新 `human_tests/readme.md` 中 WebUI Rules 用例数量与说明。

## 校验要求

- `pnpm --dir web test:unit`
- `pnpm --dir web test:ui -- admin-rules-values.spec.ts`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 文档更新要求

本次仅修改 WebUI 交互，不涉及 README、规则协议、Hook、CLI 参数或配置项更新。
