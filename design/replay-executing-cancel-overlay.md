# Replay 执行中取消按钮可达性

## 功能模块详细描述

修复 Replay 页面执行请求时，composer 区域被全屏 loading 遮罩覆盖，导致用户无法点击 `Cancel` 手动取消执行的问题。

当前表现是：

- 发送普通 HTTP Replay 请求后，页面进入执行态
- `RequestPanel` 已经把 `Send` 按钮切换成 `Cancel`
- 但外层全屏 `Spin` 会拦住点击，导致取消按钮视觉上存在、交互上不可用

## 实现逻辑

1. 保留 Replay 页面在首屏拉取、列表加载等场景下的整体 loading。
2. 普通 HTTP 执行过程中不再对整个 composer 区域加全屏遮罩。
3. 在 `ResponsePanel` 内增加局部执行态：
   - 当 `executing && !currentResponse && !currentTrafficRecord && !hasStreamingContent` 时，展示内联 `Spin tip="Executing request..."`（带 `data-testid="replay-response-executing"`）
4. 这样请求执行时：
   - 用户仍能在 `RequestPanel` 里点击 `Cancel`
   - 响应区仍然给出明确的执行中反馈
5. 增加 UI 回归用例，覆盖“慢请求执行中点击 Cancel”场景。

## 依赖项

- `web/src/pages/Replay/index.tsx`
- `web/src/pages/Replay/components/ResponsePanel.tsx`
- `web/tests/ui/admin-replay.spec.ts`

## 测试方案（含 e2e）

1. 启动一个延迟响应的 mock HTTP 服务。
2. 在 Replay 中填入该地址并发起请求。
3. 断言 `Cancel` 按钮可见且可点击。
4. 点击 `Cancel` 后断言按钮恢复为 `Send`，响应区不再停留在执行态。
5. 按任务要求执行 Replay UI E2E 回归后，再执行 rust-project-validate。

## 校验要求（含 rust-project-validate）

- 先执行本次变更相关的 Replay UI E2E 回归
- 再依次执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`（按修改范围执行）
  - `cargo build --all-targets --all-features`

## 文档更新要求

- 本次为管理端交互修复，不涉及 README、API 或配置项变更
