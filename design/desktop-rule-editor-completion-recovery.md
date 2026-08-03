# 桌面端 Rules 编辑器智能提示恢复方案

## 背景

Bifrost Desktop 的 Rules 编辑器仍能正常显示 Monaco 语法高亮，但在规则 matcher 后输入 `req` 等协议前缀时不再出现智能提示；手动执行 `Ctrl+Space` 也没有候选。当前 `/api/syntax` 能返回完整协议词表，因此故障不在后端语法数据，而在桌面启动阶段的前端加载时序。

`web/src/components/BifrostEditor/snippet/operator.ts` 在模块导入时立即调用一次 `loadDynamicData()`。Desktop 的真实 Core 端口和启动状态则在 React 首次渲染后的运行时初始化阶段才稳定。若首次 `/api/syntax` 请求发生在 Core ready 之前，异常被记录后 `dynamicOperators` 保持为空；普通输入和手动触发补全都只读取空数组，不会再次加载，导致本次 Desktop 会话中补全永久失效。

针对 Desktop 跨源边界的实测结果：已安装 `0.0.169` Core 对 `Origin: tauri://localhost` 的 syntax GET 响应返回 `Access-Control-Allow-Origin: tauri://localhost`，带 `X-Client-Id` 的 OPTIONS 预检返回 204，并允许该请求头。当前 CORS 配置不是直接根因，但需要增加 syntax 路由专项回归，避免以后出现“同源 Web 正常、Desktop 被 CORS 拦截”的真实退化。

## 用户目标验证清单

### 必须实现

- Rules 编辑器在 Desktop 启动阶段首次 syntax 请求失败后能够自动恢复。
- 在 matcher 后输入协议前缀（例如 `a.com req`）时出现协议智能提示。
- `Ctrl+Space` 手动触发也能显示同一候选集合。
- 选择 `reqHeaders` 候选后插入完整 snippet。

### 必须不破坏

- Web 管理端现有补全、语法高亮、Hover、变量/脚本/规则引用补全保持不变。
- 不把后端动态协议清单复制成前端静态清单；`/api/syntax` 仍是唯一词表来源。
- 后端暂时不可用时不阻塞编辑器输入；后续触发可继续重试。
- Rules 保存、Default 保护、桌面编辑快捷键和明暗主题保持不变。
- 不停止或重启用户正在使用的共享 9900 Desktop Core。

### 必须真实验证

- 单元测试模拟模块预加载失败、后端随后恢复，确认下一次补全请求重新拉取并返回 `reqHeaders`。
- Playwright 模拟首个 `/api/syntax` 请求失败，确认普通键入或 `Ctrl+Space` 能恢复建议、选择候选并插入 snippet。
- Admin 安全 E2E 使用 `Origin: tauri://localhost` 请求真实 `/api/syntax`，确认允许 Origin 且响应包含 `reqHeaders`。
- human test 在隔离 Desktop/测试服务上验证自动提示、插入结果和亮暗主题；配套 Playwright 验证 `Ctrl+Space` 手动提示。

### 必须交付

- 修复实现、单元测试、Playwright E2E、`human_tests/webui-rules.md` 与索引同步提交。
- 完成两轮 Review/Fix/Test、推送、Draft PR 和远端 CI 看护。
- 远端 CI 执行 `bash scripts/ci/coverage-all.sh --json --gate` 覆盖率门禁。

## 技术方案

### 失败后按需恢复

保留模块导入时的预加载以维持 Web 管理端和正常 Desktop 启动时的低延迟体验，但把补全 provider 改为：

1. disposed model 立即返回空列表，不发起网络请求。
2. 若动态协议清单尚未就绪，在计算候选前 `await loadDynamicData()`。
3. 加载成功后使用新词表生成候选。
4. 加载失败时仍返回空列表；因为状态保持未就绪，下一次普通键入或 `Ctrl+Space` 会再次尝试。

`syntaxApi.ts` 已用共享 `fetchPromise` 合并并发请求，所以快速连续触发不会制造重复请求风暴。

### 交互约束

- 不新增弹窗、Toast 或占位 loading，避免补全失败干扰规则编辑。
- 使用 Monaco 原生 suggest widget，保持现有密度、键盘导航和明暗主题。
- 明确启用 `quickSuggestions` 与 `suggestOnTriggerCharacters`，让 Desktop WebView 与浏览器使用同一交互配置。

## 测试方案

### 单元测试

新增 `web/src/components/BifrostEditor/snippet/operator.test.ts`：

- 模块预加载失败后，模拟 syntax API 恢复。
- 输入 `a.com req` 请求候选。
- 断言 provider 重新请求 syntax，并返回 `reqHeaders` snippet。
- disposed model 不发起恢复请求。

### E2E

更新 `web/tests/ui/admin-rules-values.spec.ts`：

- 首次 syntax 请求失败后再放行真实 API。
- 打开 Rules 编辑器并输入 matcher + `req`。
- 验证 suggest widget 出现 `reqHeaders`。
- 选择候选后验证编辑器内容包含 `reqHeaders://`。

### human_tests

在 `human_tests/webui-rules.md` 新增 TC-WRU-49，覆盖真实 Desktop 的自动恢复、候选插入和亮暗主题，并通过同一用例内的 Playwright 步骤覆盖 `Ctrl+Space`。用例创建后立即按文档执行，只更新 `human_tests/readme.md` 中 Web UI Rules 对应索引行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照截图、真实 Desktop 现象和本设计逐项复核。
- 执行 `git status --short`、`git diff`，review 异步 provider、失败重试、重复请求和 disposed model。
- 运行定向 Vitest、Playwright 与 TC-WRU-49。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff，再次复核 Desktop/Web 一致性、明暗主题和文档索引。
- 复跑定向 Vitest、Playwright、Web build、human test，并按 `rust-project-validate` 完成仓库终检。

## 覆盖率与交付门禁

- 本地执行定向前端单元测试和 E2E，不默认执行高成本全量 coverage。
- 推送后由远端 CI 的 `bash scripts/ci/coverage-all.sh --json --gate` 执行 90% 棘轮门禁。
- CI 失败时按日志进入 fix → push → watch，直到全绿或确认外部阻塞。
- 若 macOS shell E2E 已下载 release artifact 并设置 `SKIP_BUILD=true`，相关 suite 必须复用注入的 `BIFROST_BIN`；禁止覆盖为 debug 路径或再次执行 `cargo build`，避免冷 runner 在 600 秒 suite watchdog 前尚未进入业务断言。
