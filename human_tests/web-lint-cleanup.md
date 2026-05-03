# Web ESLint 清理真实场景测试

## 功能模块说明

验证 `web/` 前端项目的 ESLint 清理结果，确保全量 lint 零错误、零警告通过，并确认 TypeScript 与 Vite build 未因清理改动退化。

## 前置条件

1. 工作目录为 `<REPO_ROOT>`。
2. 当前分支为 `feat/agent`。
3. 已执行 `cd web && pnpm install --frozen-lockfile`。
4. 不需要启动 Bifrost 服务，不使用 9900 端口，不修改系统代理。

## 测试用例列表

### TC-WLF-01 Web ESLint 全量零错误零警告

操作步骤：
1. 执行 `cd <REPO_ROOT>/web && pnpm run lint 2>&1 | tee /tmp/human-web-lint-cleanup-lint.log`。
2. 检查命令退出码。
3. 检查 `/tmp/human-web-lint-cleanup-lint.log` 中不存在 `error`、`warning`、`✖` 或 `ELIFECYCLE`。

预期结果：
- 命令退出码为 0。
- ESLint 输出只包含脚本启动信息，无任何文件级 error 或 warning。

实际结果：
- 2026-05-02 执行通过。命令退出码为 0，输出仅包含 `eslint .` 脚本启动信息，无 error、warning、`✖` 或 `ELIFECYCLE`。

### TC-WLF-02 Web Build 未退化

操作步骤：
1. 执行 `cd <REPO_ROOT>/web && pnpm run build 2>&1 | tee /tmp/human-web-lint-cleanup-build.log`。
2. 检查命令退出码。
3. 检查输出包含 `✓ built in`。

预期结果：
- 命令退出码为 0。
- TypeScript build 与 Vite build 成功完成。
- 允许保留本分支已有的 Vite chunk size 或 module directive 提示，但不得出现 TypeScript error 或 build failure。

实际结果：
- 2026-05-02 执行通过。命令退出码为 0，输出包含 `✓ built in`；保留基线已有的 Vite module directive、dynamic import 与 chunk size 提示，无 TypeScript error 或 build failure。

## 清理步骤

1. 无需停止服务。
2. 保留 `/tmp/human-web-lint-cleanup-lint.log` 与 `/tmp/human-web-lint-cleanup-build.log` 作为本轮验证证据。
