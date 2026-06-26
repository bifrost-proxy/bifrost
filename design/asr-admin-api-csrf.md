# ASR Admin API CSRF 回归方案

## 背景

ASR WebUI 位于 `/_bifrost/ai?aiSection=tools-asr`，所有 ASR 请求都属于本地管理端 Admin API。后端对浏览器发起的 unsafe method 请求执行 admin CSRF 校验，要求请求携带 `X-Bifrost-CSRF`。统一的 `apiFetch` 和 axios client 已经支持自动获取并注入该 token，但 ASR API 层长期存在大量直接 `fetch(buildApiUrl(...))` 调用，只手动补了 `Authorization` 和 `X-Client-Id`，因此写接口会在页面上批量报：

```text
{"error":"Missing or invalid admin CSRF token","status":403}
```

## 目标验证清单

- 必须实现：ASR 模块所有 unsafe Admin API 请求统一经过 `apiFetch`，自动携带 `X-Bifrost-CSRF`。
- 必须实现：ASR JSON、SSE/FormData、Daily Agent 三类请求形态都覆盖。
- 必须不破坏：ASR GET 查询、WebSocket URL token、现有 `get/post` axios client 调用保持原行为。
- 必须真实验证：打开 ASR 管理页或调用 ASR Web API 时不再出现缺 CSRF 的 403。
- 必须交付：单元回归、E2E 回归、`human_tests/` 场景与两轮 Review/Fix/Test 证据。

## 实现逻辑

- 在 `web/src/api/asr.ts` 内新增局部 `asrFetch(path, init)`，内部调用 `apiFetch(buildApiUrl(path), init)`。
- 保留 `buildStreamHeaders()` / `buildJsonHeaders()` 对 `Accept` 与 `Content-Type` 的表达，移除手写 `Authorization` 与 `X-Client-Id`。
- 所有原生 ASR `fetch` 调用迁移为 `asrFetch`；`GET` 请求不触发 CSRF，`POST` / `PUT` / `PATCH` / `DELETE` 由 `apiFetch` 自动获取并注入 token。
- `buildVoiceRealtimeUrl()` 继续把 admin token 放入 WebSocket query，不纳入 CSRF header 逻辑。

## 测试方案

### 单元测试

- `web/src/api/asr.test.ts` 新增 ASR admin API CSRF 回归：
  - `createAsrTask` 验证 JSON 写请求带 `X-Bifrost-CSRF`。
  - `streamAsrTranscription` 验证 FormData/SSE 写请求带 `X-Bifrost-CSRF`。
  - `updateDailyAgentConfig` 验证 Daily Agent 写请求带 `X-Bifrost-CSRF`。

### E2E 测试

- 新增 `e2e-tests/tests/test_asr_admin_csrf.sh`：
  - 执行 ASR 前端 API 回归，确认 unsafe ASR 请求通过统一 CSRF fetch。
  - 执行后端 admin cross-site security 回归，确认 CSRF 门禁仍然生效，未因前端修复放松安全策略。

### human_tests

- 新增 `human_tests/asr-admin-csrf.md`：
  - 真实打开 ASR 管理页。
  - 验证页面加载和典型 ASR 写接口不再出现 `Missing or invalid admin CSRF token`。
  - 验证后端无 token 的 unsafe 请求仍返回 403，带 token 的请求通过 CSRF 层。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：整个 ASR 模块大面积 CSRF 403，而不是单一页面按钮。
- 执行 `git status --short`、`git diff`，检查所有直接 ASR `fetch` 是否收敛。
- Review 风险点：FormData 不应误加 `Content-Type`，SSE `Accept` 不丢失，WebSocket token 不受影响。
- 运行最小测试：`pnpm --dir web test:unit src/api/asr.test.ts` 和 ASR CSRF E2E。

### 第 2 轮

- 再次复核第 1 轮后的 diff，确认 Daily Agent、Speaker Profile、Directory Task、Workbench、Offline Job 全部覆盖。
- 再次执行 `git status --short`、`git diff`。
- 复跑相关测试与 human_tests，确认没有只修测试或只修局部路径。

## 校验要求

- `pnpm --dir web test:unit src/api/asr.test.ts`
- `bash e2e-tests/tests/test_asr_admin_csrf.sh`
- `human_tests/asr-admin-csrf.md` 逐条执行
- `cargo test --workspace --all-features`
- `make coverage`；如完整 E2E 覆盖环境不可用，退化为 `make coverage-unit` 并记录原因。
