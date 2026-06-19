# Node.js 安全依赖修复真实场景测试

## 功能模块说明

验证所有 Node.js 安全依赖升级后，GitHub Dependabot npm 告警对应的依赖均已升级到安全版本，并确认管理端前端、文档站点、Sync Server 与 Puppeteer 脚本目录没有残留本地 audit 漏洞或用户可感知前端回归。

## 前置条件

1. 工作目录为 `<REPO_ROOT>`。
2. 当前分支为 `codex/node-security-fixes`。
3. 已执行对应目录的依赖安装：
   - `pnpm --dir web install`
   - `pnpm --dir site install`
   - `pnpm --dir packages/bifrost-sync-server install`
   - `npm install --package-lock-only --ignore-scripts` 已用于两个 `.agents/skills/*/scripts` 目录刷新 lockfile
4. 不启动系统代理；如需启动 Bifrost 服务，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 并传入 `--no-system-proxy`。

## 测试用例列表

### TC-NSF-01 Node 安全 audit 清零

操作步骤：
1. 执行 `pnpm --dir web audit --audit-level=low`。
2. 执行 `pnpm --dir site audit --audit-level=low`。
3. 执行 `pnpm --dir packages/bifrost-sync-server audit --audit-level=low`。
4. 执行 `cd .agents/skills/e2e-verify/scripts && npm audit --audit-level=low`。
5. 执行 `cd .agents/skills/site-cookie-login/scripts && npm audit --audit-level=low`。

预期结果：
- 五个 audit 命令退出码均为 0。
- 输出均显示无已知漏洞或 `found 0 vulnerabilities`。

实际结果：
- 已执行，全部通过。
- `pnpm --dir web audit --audit-level=low` 输出 `No known vulnerabilities found`。
- `pnpm --dir site audit --audit-level=low` 输出 `No known vulnerabilities found`。
- `pnpm --dir packages/bifrost-sync-server audit --audit-level=low` 输出 `No known vulnerabilities found`。
- `.agents/skills/e2e-verify/scripts` 下 `npm audit --audit-level=low` 输出 `found 0 vulnerabilities`。
- `.agents/skills/site-cookie-login/scripts` 下 `npm audit --audit-level=low` 输出 `found 0 vulnerabilities`。

### TC-NSF-02 Web 前端构建与单元测试未退化

操作步骤：
1. 执行 `pnpm --dir web run lint`。
2. 执行 `pnpm --dir web run build`。
3. 执行 `pnpm --dir web run test:unit`。

预期结果：
- 三个命令退出码均为 0。
- build 输出包含 Vite 成功构建信息。
- unit test 全部通过。

实际结果：
- 已执行，全部通过。
- `pnpm --dir web run lint` 最终退出码 0；验证过程中发现 build 产物 `dist-gzip/` 会被 lint 扫描，已将该生成目录加入 eslint ignore 后复测通过，剩余为既有 warning。
- `pnpm --dir web run build` 退出码 0，Vite 6.4.3 构建成功。
- `pnpm --dir web run test:unit` 退出码 0，21 个测试文件、81 个测试全部通过。

### TC-NSF-03 Web UI 真实浏览器 E2E 未退化

操作步骤：
1. 执行 `pnpm --dir web run test:ui`。
2. 检查 Playwright 结果。

预期结果：
- 命令退出码为 0。
- 管理端核心页面、流量、规则、设置、通知、Replay、Scripts 等 UI E2E 全部通过。

实际结果：
- 已执行，最终全部通过。
- 先对失败点执行 targeted 回归：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/traffic.spec.ts --grep "切换页面后保留"`，1 个用例通过。
- 针对 ASR Daily Agent 两个超时失败修复测试等待与可见性断言后，执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts --grep "simple Runner|full-page Markdown"`，2 个用例通过。
- 首次最终执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web run test:ui`，174 个 Playwright UI 用例全部通过，用时约 6.4 分钟。
- 追加 UI 覆盖用例与 ASR 测试稳定性修复后，再次执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web run test:ui`，175 个 Playwright UI 用例全部通过，用时约 7.1 分钟。

### TC-NSF-04 本次 UI 修改点逐项覆盖

操作步骤：
1. 对照本次修改的 UI 相关文件，确认每个用户可感知修改都有对应单元测试或 Playwright UI 测试。
2. 执行 `pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts`。
3. 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/pending-auth-modal.spec.ts tests/ui/admin-rules-values.spec.ts tests/ui/admin-settings.spec.ts tests/ui/asr-daily-agent-runner.spec.ts tests/ui/asr-home-tabs.spec.ts tests/ui/asr-microphone-meter.spec.ts tests/ui/traffic.spec.ts tests/ui/traffic-push.spec.ts tests/ui/breakpoint-ui.spec.ts`。

预期结果：
- UI 覆盖矩阵中每个修改点都有精确测试。
- 单元测试与 Playwright UI 子集均通过。

UI 覆盖矩阵：

| 修改点 | 覆盖测试 |
| --- | --- |
| Pending Auth 弹窗新增 Settings 跳转按钮 | `web/tests/ui/pending-auth-modal.spec.ts` 中 `Pending auth modal settings button opens Access Control settings` |
| AI 对话历史显式 idle 不再显示 running 占位 | `web/src/pages/AI/AgentChatSection.timeline.test.ts` 中 `does not append a running placeholder when detail run_state is explicit idle` |
| ASR Daily Agent report sync dir 保存按钮可稳定定位并保存 | `web/tests/ui/asr-daily-agent-runner.spec.ts` 中 Daily Agent sync dir 保存流程 |
| Diarization setup 录音计时器 lint 修复后页面流程不退化 | `web/tests/ui/asr-home-tabs.spec.ts`、`web/tests/ui/asr-microphone-meter.spec.ts` 与全量 UI E2E ASR 用例 |
| Rules 编辑器真实变更后 Save 启用，保存后禁用且清理 dirty state | `web/tests/ui/admin-rules-values.spec.ts` 中 `Rules 页面真实变更后可保存且保存后禁用按钮` |
| Settings Sync/Remote Invoke active tab 周期刷新并同步底部状态栏 | `web/tests/ui/admin-settings.spec.ts` 中 `Settings Sync 打开时会轮询刷新页面与底部状态栏` |
| Remote Invoke Shell Access 管理入口可打开且 ID 只读 | `web/tests/ui/admin-settings.spec.ts` 中 `Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读` |
| Values item menu 在导出、导入、删除流程中可稳定操作 | `web/tests/ui/admin-rules-values.spec.ts` Values export/import/delete 用例 |
| Traffic 清空、新请求提示、Header diff、SSE 详情、OpenAI SSE response tab 未退化 | `web/tests/ui/traffic.spec.ts` 对应清空、订阅提示、Header、SSE、OpenAI SSE 用例 |
| 页面切换后 traffic push 与 reload 后历史记录保持 | `web/tests/ui/traffic.spec.ts` 中 `切换页面后保留已加载流量并持续接收 push` 与 `web/tests/ui/traffic-push.spec.ts` |
| Breakpoint/Rules Monaco 补全在升级后仍可触发 | `web/tests/ui/breakpoint-ui.spec.ts` 与 `web/tests/ui/admin-rules-values.spec.ts` 规则编辑器补全用例 |

实际结果：
- 已完成覆盖矩阵设计。
- 已执行 `pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts`，1 个测试文件、14 个单元测试通过。
- 新增 `Settings Sync 打开时会轮询刷新页面与底部状态栏` 用例首次暴露断言缺口，修正为真实 `ready` 状态后复跑通过。
- UI 覆盖子集首次执行时发现 2 个 ASR 测试稳定性问题：Daily Agent sync dir 输入被前一次 config reload 覆盖、Directory Task route handler 断言会把请求校验错误伪装成弹窗不关闭。已修正等待与断言位置。
- 已执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/pending-auth-modal.spec.ts tests/ui/admin-rules-values.spec.ts tests/ui/admin-settings.spec.ts tests/ui/asr-daily-agent-runner.spec.ts tests/ui/asr-home-tabs.spec.ts tests/ui/asr-microphone-meter.spec.ts tests/ui/traffic.spec.ts tests/ui/traffic-push.spec.ts tests/ui/breakpoint-ui.spec.ts`，90 个 Playwright UI 用例全部通过。

### TC-NSF-05 Docs site 与 Sync Server Node 包未退化

操作步骤：
1. 执行 `pnpm --dir site run docs:test`。
2. 执行 `pnpm --dir site run build`。
3. 执行 `pnpm --dir packages/bifrost-sync-server run build`。
4. 执行 `pnpm --dir packages/bifrost-sync-server test`。

预期结果：
- 四个命令退出码均为 0。
- Astro/Starlight 文档站点构建成功。
- Sync Server TypeScript 编译和 Vitest 测试通过。

实际结果：
- 已执行，全部通过。
- `pnpm --dir site run docs:test` 退出码 0，6 个 Node docs sync 测试全部通过。
- `pnpm --dir site run build` 退出码 0，Astro/Starlight 构建成功并通过站点链接验证。
- `pnpm --dir packages/bifrost-sync-server run build` 退出码 0。
- `pnpm --dir packages/bifrost-sync-server test` 退出码 0，9 个测试文件、160 个 Vitest 用例全部通过。

## 清理步骤

1. 不需要停止 Bifrost 服务，因为本测试默认不启动服务。
2. 若 Playwright 或构建产生 `web/test-results/`、`web/playwright-report/`、`site/dist/`、`packages/bifrost-sync-server/dist/` 等可再生成目录，验证后按需清理，避免提交生成产物。
