# Web UI CSRF 传输层 ESLint 守卫

## 功能模块说明

Web UI 所有浏览器来源的写接口（POST/PUT/PATCH/DELETE）必须携带 `X-Bifrost-CSRF`，否则被后端 `admin_csrf_middleware` 拒绝返回 403 `Missing or invalid admin CSRF token`。前端注入该 header 只有两条合法通道：

- `apiFetch`（`web/src/api/apiFetch.ts`）：裸 `fetch` 的封装，unsafe method 自动注入 CSRF token 并做失效重试。
- 共享 axios 客户端 `client`（`web/src/api/client.ts`）：请求拦截器统一注入 token。

历史上 `web/src/api/bifrost-file.ts` 直接 `import axios from 'axios'` 用**默认实例**发请求，绕过了拦截器，导致 `.bifrost` 文件导入/导出接口全部 403。为防止今后再次出现"用默认 axios 绕过 CSRF"的回归，新增 ESLint `no-restricted-imports` 守卫：`web/src/**` 下禁止 `import ... from 'axios'`，仅 `web/src/api/client.ts` 白名单放行。

本用例验证守卫规则真实生效、白名单正确、既有代码不再违规，且 `pnpm lint` 保持全绿以便守卫可被 CI/本地强制执行。

## 前置条件

- 当前仓库位于本次修复分支。
- 已安装 `web/` 依赖（`pnpm --dir web install`）。
- 执行命令前先运行 `source ~/.zshrc`。

## 测试用例列表

### TC-CSRF-GUARD-01：`pnpm lint` 全绿

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cd web && pnpm lint; echo "exit=$?"
   ```

**预期结果**：

- `exit=0`。
- 汇总行为 `0 errors`（允许既有 `react-hooks` warnings）。
- `web/src/api/asr.test.ts` 不再报 `_init is defined but never used`（`no-unused-vars` 已按 `^_` 忽略）。

### TC-CSRF-GUARD-02：非白名单文件 `import axios` 触发 error

**操作步骤**：

1. 在任意非白名单文件（如 `web/src/api/videos.ts`）临时首行插入 `import axios from 'axios';`。
2. 执行：
   ```bash
   source ~/.zshrc && cd web && npx eslint src/api/videos.ts
   ```
3. 还原该文件（`git checkout -- src/api/videos.ts`）。

**预期结果**：

- 报 `no-restricted-imports` error，提示信息包含 `Use apiFetch ... or the shared client ... so X-Bifrost-CSRF is injected`。
- 还原后 `git status --short src/api/videos.ts` 为空。

### TC-CSRF-GUARD-03：白名单文件 `client.ts` 不被误伤

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cd web && npx eslint src/api/client.ts 2>&1 | grep -c "no-restricted-imports"
   ```

**预期结果**：

- 输出 `0`（`client.ts` 是唯一 sanctioned 的 axios 入口，不报 restricted-imports）。

### TC-CSRF-GUARD-04：`bifrost-file.ts` 已移除默认 axios 且行为不变

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cd web && grep -n "from 'axios'" src/api/bifrost-file.ts; echo "grep-exit=$?"
   ```
2. 执行单元测试：
   ```bash
   source ~/.zshrc && cd web && pnpm test:unit src/api/bifrost-file.test.ts
   ```

**预期结果**：

- 步骤 1 `grep-exit=1`（无任何 axios import）。
- 步骤 2 全部用例通过，含 `imports files through the CSRF-aware API client`、`exports network files through the CSRF-aware API client`、`extracts backend error messages from axios responses`（duck-type 错误判定不依赖 axios 运行时）。

### TC-CSRF-GUARD-05：全量前端单测与类型构建通过

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cd web && pnpm test:unit && npx tsc -b
   ```

**预期结果**：

- 全部单测通过。
- `tsc -b` 退出码为 0。

## 清理步骤

- TC-CSRF-GUARD-02 修改的文件必须还原（`git checkout -- web/src/api/videos.ts`）。
- 本用例不启动服务，无需清理数据目录。

## 本次执行结果

- 执行日期：2026-07-06
- TC-CSRF-GUARD-01：PASS。`pnpm lint` `exit=0`，`14 problems (0 errors, 14 warnings)`。
- TC-CSRF-GUARD-02：PASS。videos.ts 注入 axios 后报 `no-restricted-imports` error 并含引导文案；还原后工作区干净。
- TC-CSRF-GUARD-03：PASS。`client.ts` 输出 `0`。
- TC-CSRF-GUARD-04：PASS。`grep-exit=1`；`bifrost-file.test.ts` 全绿。
- TC-CSRF-GUARD-05：PASS。全量单测 `23 files / 99 tests passed`；`tsc -b` `exit=0`。
