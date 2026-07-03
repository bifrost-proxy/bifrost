# ASR Admin API CSRF 回归方案

## 背景

ASR WebUI 位于 `/_bifrost/ai?aiSection=tools-asr`,所有 ASR 请求都属于本地管理端 Admin API。后端对浏览器发起的 unsafe method 请求(POST/PUT/PATCH/DELETE)执行 admin CSRF 校验,要求请求头携带 `X-Bifrost-CSRF`。该 token 由 `GET /_bifrost/api/security/csrf` 派发,前端统一入口 `apiFetch`(`web/src/api/apiFetch.ts`)会在 unsafe method 时自动读取缓存 token 并注入 header;axios 客户端和其他模块(Traffic、Rules、Agent、Video、Power)也通过 `apiFetch` 或 axios interceptor 完成相同注入。

ASR API 层长期存在大量直接 `fetch(buildApiUrl(...))` 调用,只手动补了 `Authorization` 和 `X-Client-Id`,因此写接口在页面上批量报:

```text
{"error":"Missing or invalid admin CSRF token","status":403}
```

具体表现是 ASR Directory Task 保存、Daily Agent 配置更新、Speaker Profile 注册、Task run/pause/resume 等 unsafe 请求全部 403;GET 查询与 WebSocket 语音输入不受影响。修复目标是让 ASR 模块所有 unsafe Admin API 请求走 `apiFetch`,复用现有 CSRF token 机制,不放松后端门禁。

## 用户目标验证清单

### 必须实现

- ASR 模块所有 unsafe Admin API 请求统一经过 `apiFetch`,由 `apiFetch` 自动附加 `X-Bifrost-CSRF`;不允许在 `web/src/api/asr.ts` 中直接调用 `fetch(buildApiUrl(...))` 做写操作。
- ASR 三类请求形态全部覆盖:
  - JSON 写请求(Directory Task 增删改、Speaker Profile 增删改、Daily Agent 配置更新)。
  - SSE / stream 写请求(`streamAsrTranscription`、`/asr/service/start|stop`、`/asr/tasks/run|pause|resume`)。
  - FormData 写请求(音频上传、外部设备导入)。
- `web/src/api/asr.ts` 内新增局部封装 `asrFetch(path, init)`,内部 `return apiFetch(buildApiUrl(path), init)`;`buildStreamHeaders()` / `buildJsonHeaders()` 只表达 `Accept` 与 `Content-Type`,不再手写 `Authorization` / `X-Client-Id`(这些由 `apiFetch` 内部按需处理)。
- WebSocket 语音链路 `buildVoiceRealtimeUrl()` 保持原样:token 走 query,不走 header,不进入 CSRF 分支。
- 后端 admin CSRF 校验不放松:缺 token / cross-site 请求继续返回 403。

### 必须不破坏

- ASR GET 查询、WebSocket URL token、现有 axios `get/post` 通道保持行为一致。
- 其他模块(Traffic、Rules、Agent、Video、Power、Editor 校验)对 `apiFetch` 的调用不受本次改动影响。
- `web/src/api/csrf.ts` 单飞 (in-flight dedupe) 与缓存策略不变;不新增 CSRF endpoint。
- 后端 `handle_admin_csrf` / `admin_csrf_middleware` 逻辑不改;`test_admin_cross_site_security.sh` 保持全绿。
- ASR 页面加载性能:token 请求仍为一次全局命中,不因迁移触发额外 CSRF fetch。

### 必须真实验证

- 打开 `http://127.0.0.1:<web>/_bifrost/ai?aiSection=tools-asr`,加载与典型写动作(建 Directory Task、保存 Daily Agent 配置、上传/提交音频)不再出现 `Missing or invalid admin CSRF token`。
- 浏览器 Network 面板中所有 unsafe `/_bifrost/api/asr/**` 请求 headers 都含 `X-Bifrost-CSRF`。
- Vitest `pnpm --dir web test:unit src/api/asr.test.ts` 中的 `ASR admin API CSRF headers` 用例全绿,覆盖 JSON、SSE/FormData、Daily Agent 三类形态。
- E2E `bash e2e-tests/tests/test_asr_admin_csrf.sh` 输出 `[asr-admin-csrf-e2e] passed`;内部会跑 `test_admin_cross_site_security.sh` 验证后端门禁未被放松。
- human_tests `human_tests/asr-admin-csrf.md` 逐条执行并回填结果。

## 产品语义

### CSRF token 的注入应当由框架统一负责,而不是分散在每个模块

Bifrost 前端已经存在唯一的 CSRF 入口 `apiFetch`:GET/HEAD/OPTIONS 直接透传;unsafe method 通过 `getAdminCsrfToken()` 获取或复用缓存的 token,并注入 `X-Bifrost-CSRF` header。任何直接使用 `fetch(buildApiUrl(...))` 的模块,都会绕过这一层。

ASR 模块的问题就是长期在 `web/src/api/asr.ts` 内写 `fetch(buildApiUrl(...))`;修复策略是把所有 ASR 请求集中经过 `asrFetch = (path, init) => apiFetch(buildApiUrl(path), init)`,让 CSRF 注入回到框架层,而不是每个函数手动去补 header。这样后续新增 ASR 接口也不会遗漏 CSRF。

### `buildStreamHeaders` / `buildJsonHeaders` 只表达内容协商

原有 `buildStreamHeaders` 混杂了 `Authorization`、`X-Client-Id`、`Accept: text/event-stream` 等多重语义。迁移后:

- `buildStreamHeaders()` 只返回 `{ Accept: "text/event-stream" }`。
- `buildJsonHeaders()` 在其之上叠加 `Accept: application/json` 与 `Content-Type: application/json`。

`Authorization` 与 `X-Client-Id` 由 `apiFetch` 内部按需附加(如果后端要求)。FormData 请求不再显式设 `Content-Type`,让浏览器自动生成带 boundary 的值。

### WebSocket 语音输入走 URL token,不参与 CSRF

`buildVoiceRealtimeUrl()` 用于建立浏览器与 ASR 语音后端的 WebSocket 连接;WebSocket 握手不支持自定义 header,认证 token 必须放在 query。该路径不属于 unsafe HTTP method,不涉及 CSRF,本方案不改。

### 后端门禁保持严格

前端修复不放松后端策略。`admin_csrf_middleware` 继续要求:same-origin + 有效 token 才通过;缺 token 或 cross-site 直接 403。`test_admin_cross_site_security.sh` 作为 E2E 中枢在 `test_asr_admin_csrf.sh` 内部被调用,保证前后端修复一致。

## 技术细节

### 关键文件

- `web/src/api/asr.ts`:新增 `asrFetch`;重写 `buildStreamHeaders` / `buildJsonHeaders`;把所有原生 `fetch(buildApiUrl(...))` 迁移为 `asrFetch(path, ...)`。
- `web/src/api/apiFetch.ts`:提供 `apiFetch(input, init)`,unsafe method 时通过 `getAdminCsrfToken()` 注入 `X-Bifrost-CSRF`。
- `web/src/api/csrf.ts`:提供 `getAdminCsrfToken()`、`ADMIN_CSRF_HEADER = "X-Bifrost-CSRF"`、`isUnsafeHttpMethod()`;单飞 + 缓存。
- `web/src/api/asr.test.ts`:vitest 用例,`ASR admin API CSRF headers` 断言 `createAsrTask` / `streamAsrTranscription` / `updateDailyAgentConfig` 都带 `X-Bifrost-CSRF`。
- `e2e-tests/tests/test_asr_admin_csrf.sh`:构建 bifrost 二进制,跑 ASR vitest + `test_admin_cross_site_security.sh`。
- `e2e-tests/tests/test_admin_cross_site_security.sh`:后端 CSRF 门禁回归。
- `human_tests/asr-admin-csrf.md`:真实场景三条用例 + 本次执行结果记录。

### `asrFetch` 定义

```ts
function asrFetch(path: string, init: RequestInit = {}): Promise<Response> {
  return apiFetch(buildApiUrl(path), init);
}
```

- 输入是 ASR 内部相对路径(如 `/asr/tasks`),内部拼成绝对 URL。
- 所有 ASR API 函数只依赖 `asrFetch`,不再直接调用 `fetch`。

### 迁移点清单(节选)

已经迁移到 `asrFetch` 的调用点:

- `createAsrTask`、`listAsrTasks`、`updateAsrTask`、`runAsrTask`、`pauseAsrTask`、`resumeAsrTask`、`deleteAsrTask`。
- `streamAsrTranscription`、`/asr/service/start`、`/asr/service/stop`。
- `identifySpeakerProfile`、`createSpeakerEnrollmentSession`、`updateSpeakerProfile`、`deleteSpeakerProfile`。
- `updateDailyAgentConfig`、`listDailyAgentTasks`、`runDailyAgentTask`。

迁移完成后 `web/src/api/asr.ts` 中不再出现 `fetch(buildApiUrl(...))` 调用(GET 只读接口也走 `asrFetch`,保持一致性)。

### FormData 与 SSE 边界

- FormData: 不显式设 `Content-Type`,让浏览器写 `multipart/form-data; boundary=...`。`asrFetch` 里的 `body` 直接传 `FormData`。
- SSE: `buildStreamHeaders()` 返回 `Accept: text/event-stream`;`readSseResponse` 读取 body reader,行为不变。
- CSRF 注入对 body 类型透明,`apiFetch` 只操作 headers。

### CSRF token 生命周期

- 页面初次 unsafe 请求时 `getAdminCsrfToken()` 触发一次 `GET /_bifrost/api/security/csrf`。
- 后续 unsafe 请求命中缓存;并发时 `inFlight` promise 去重。
- token 失效(401/403)时由 `apiFetch` 抛错,由调用方决定是否 refresh(本方案不新增自动 refresh 逻辑;若后续需要,应在 `apiFetch` 层加一次性重试)。

## CLI 与 Admin API

- CLI:不新增子命令。
- Admin API:不新增 endpoint;`GET /_bifrost/api/security/csrf` 与 admin CSRF middleware 保持原样。
- ASR API 契约:`POST/PUT/PATCH/DELETE /_bifrost/api/asr/**` 继续要求 `X-Bifrost-CSRF`;GET 不要求。

## Sync / 导入导出 / 分享边界

CSRF token 属于本机浏览器会话,不参与规则 sync、Group sync、rule share。ASR 任务、Speaker Profile、Daily Agent 配置的同步/导入导出边界不受本方案影响;本次仅收敛前端 header 注入路径。

## 实现切分

### Phase 1:抽取 `asrFetch` + 重写 headers 工具

- `web/src/api/asr.ts` 引入 `asrFetch`。
- `buildStreamHeaders` 只保留 `Accept: text/event-stream`;`buildJsonHeaders` 在其上叠加 JSON headers。
- 移除所有手写 `Authorization` / `X-Client-Id`。

### Phase 2:迁移全部 ASR fetch 调用

- 把 `fetch(buildApiUrl(...))` 全量替换为 `asrFetch(...)`;GET 只读接口一并迁移,避免遗漏。
- 检查 FormData、SSE、Daily Agent 三类形态覆盖。
- 保持 `buildVoiceRealtimeUrl()` 独立(WebSocket 路径)。

### Phase 3:测试补齐

- `web/src/api/asr.test.ts` 新增 `ASR admin API CSRF headers` 用例,覆盖三类请求。
- 新增 `e2e-tests/tests/test_asr_admin_csrf.sh`,内部串联 vitest + 后端 cross-site security 回归。

### Phase 4:真实场景验证与文档

- 更新 `human_tests/asr-admin-csrf.md`,逐条执行 TC-ASR-CSRF-01 / 02 / 03 并回填时间与结果。
- 更新 `human_tests/readme.md` 索引与总数。

## 测试方案

### 单元测试

`web/src/api/asr.test.ts` 内 `ASR admin API CSRF headers`:

- Mock `getAdminCsrfToken` 返回固定 token。
- 覆盖 `createAsrTask` (JSON POST) → headers 中含 `X-Bifrost-CSRF` 且值正确;`Content-Type: application/json`。
- 覆盖 `streamAsrTranscription` (FormData + SSE) → headers 含 `X-Bifrost-CSRF` 与 `Accept: text/event-stream`;不显式设 `Content-Type`(留给浏览器)。
- 覆盖 `updateDailyAgentConfig` (JSON PUT/POST) → headers 含 `X-Bifrost-CSRF`。
- 断言 GET 请求(如 `listAsrTasks`)不携带 `X-Bifrost-CSRF`,避免污染。

### E2E 测试

`e2e-tests/tests/test_asr_admin_csrf.sh`:

1. `pnpm --dir web test:unit src/api/asr.test.ts` 快速回归前端注入。
2. 构建 `target/debug/bifrost`(`SKIP_FRONTEND_BUILD=1`)。
3. 调用 `test_admin_cross_site_security.sh`,验证:
   - 缺 token unsafe → 403。
   - cross-site + token → 403。
   - same-origin + token → 通过。
4. 输出 `[asr-admin-csrf-e2e] passed`。

支持 `SKIP_ADMIN_SECURITY_E2E=true` 快速路径,仅跑前端 vitest,便于本地反复回归。

### 真实场景测试 human_tests

`human_tests/asr-admin-csrf.md` 保持三条用例并在 `本次执行结果` 段落追加日期与结论:

- TC-ASR-CSRF-01:前端 vitest 覆盖 `ASR admin API CSRF headers`。
- TC-ASR-CSRF-02:后端 CSRF 门禁不放松;完整脚本 + `SKIP_ADMIN_SECURITY_E2E=true` 分别验证。
- TC-ASR-CSRF-03:真实浏览器打开 ASR 管理页,Network 面板确认 unsafe 请求含 `X-Bifrost-CSRF`,页面日志中 `Missing or invalid admin CSRF token` 数量为 0。

真实执行使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_DATA_DIR=$(mktemp -d)`、`--no-system-proxy`、非 9900 端口。

`human_tests/readme.md` 同步索引与总数。

### 覆盖率与项目校验

- `pnpm --dir web test:unit src/api/asr.test.ts`
- `bash e2e-tests/tests/test_asr_admin_csrf.sh`
- `bash e2e-tests/tests/test_admin_cross_site_security.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 时不跑 `make coverage`;交付说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标:整个 ASR 模块大面积 CSRF 403,而不是单一页面按钮;检查是否存在遗漏的 `fetch(buildApiUrl(...))`。
- 复核 diff:`git status --short` / `git diff` 是否覆盖 `web/src/api/asr.ts`、`asr.test.ts`、`e2e-tests`、`human_tests`。
- 重点 review:FormData 不误加 `Content-Type`;SSE `Accept` 不丢失;WebSocket token 不受影响;GET 请求不多余附加 CSRF。
- 复测:`pnpm --dir web test:unit src/api/asr.test.ts` + `bash e2e-tests/tests/test_asr_admin_csrf.sh`。

### 第 2 轮

- 复查第 1 轮后 diff,确认 Daily Agent、Speaker Profile、Directory Task、Workbench、Offline Job 全部覆盖。
- 再次 `git status --short` / `git diff`,确认没有只修测试或只修局部路径的情况。
- 复跑相关测试与 human_tests,回填执行结果;出现新问题追加轮次。

## 风险与决策点

- 隐藏的直接 `fetch` 调用:仓库其他模块也可能存在 `fetch(buildApiUrl(...))` 遗留;本方案只处理 ASR,发现其他模块问题时应单开方案统一治理。
- FormData boundary:显式设 `Content-Type: multipart/form-data` 会破坏 boundary;测试需要显式断言 FormData 请求不带 `Content-Type`。
- CSRF token 失效自动重试:当前 `apiFetch` 不做失效重试,后端换 token 时首个请求会 403 一次;可接受(用户刷新页面即可)。若需要更强健壮性,应在 `apiFetch` 层加一次性重试,不由 ASR 单独处理。
- WebSocket 认证:query token 暴露在 URL 中,受浏览器 history / 日志采集影响;由更上层 WebSocket 认证方案约束,不在本方案范围。
- 迁移风险:GET 只读接口也走 `asrFetch` 主要为一致性;若某些接口不希望经过 `apiFetch` 的通用 error 处理,应在 `apiFetch` 内提供 opt-out,而不是回退到裸 `fetch`。
- 后端策略保持严格:不能因为前端修复方便而放宽 `admin_csrf_middleware`;E2E 用 `test_admin_cross_site_security.sh` 作为 tripwire。
