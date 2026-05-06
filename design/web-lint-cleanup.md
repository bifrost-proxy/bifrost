# Web ESLint Cleanup

## 功能模块说明

本模块用于约束 `web/` 目录 ESLint 清理任务。目标是让 `pnpm run lint` 在 Web 前端项目内全量零错误、零警告通过，同时不降级 lint 配置、不引入新功能、不改动 Rust 代码。

## 实现逻辑

- 将 DevTools 组件文件中导出的非组件 helper 拆分到同目录工具模块，避免触发 `react-refresh/only-export-components`。
- 对 React Hooks 依赖和 `set-state-in-effect` 报告做最小语义修复，优先保持原有用户可见行为。
- 删除空 `catch` 块和未使用变量。
- 对不兼容 React Compiler 的 Storage 虚拟列表 hook，保留表格交互行为并改为普通滚动列表渲染，确保 lint 零 warning。

## 依赖项

- `web/package.json` 中现有 ESLint、TypeScript、Vite、React 依赖。
- 不新增运行时依赖，不修改 `pnpm-lock.yaml`。

## 测试方案

### 单元测试

- 不新增单元测试。此任务不修改可单测的业务算法，核心验证由 lint/build 和现有快速回归覆盖。

### E2E 测试

- 不新增 E2E 脚本。此任务不引入新的用户流程，使用 `pnpm run build` 验证前端类型检查和打包路径。

### 真实场景测试

- 在 `human_tests/web-lint-cleanup.md` 中新增 `TC-WLF-01`，执行 `cd web && pnpm run lint`，验证零 error、零 warning。
- 在 `human_tests/web-lint-cleanup.md` 中新增 `TC-WLF-02`，执行 `cd web && pnpm run build`，验证 TypeScript 与 Vite build 成功。

## 校验要求

- `cd web && pnpm run lint`
- `cd web && pnpm run build`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check --workspace`
- `cargo test -p memory -p bifrost-agent`

## 文档更新要求

- 更新 `human_tests/web-lint-cleanup.md`。
- 更新 `human_tests/readme.md` 索引表与总计。
