# ASR Admin API CSRF 真实场景测试

验证 ASR 管理页面 `/_bifrost/ai?aiSection=tools-asr` 的 Admin API 请求都会携带管理端 CSRF token，不再批量报 `Missing or invalid admin CSRF token`；同时确认后端 CSRF 门禁仍然有效，不能因为前端修复而放松安全校验。

## 前置条件

1. 使用当前源码构建前端和 Bifrost 二进制。
2. 使用隔离数据目录启动 Bifrost，必须设置：
   - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
   - `BIFROST_DISABLE_TRAY=1`
   - `BIFROST_DATA_DIR=<临时目录>`
   - `--no-system-proxy`
3. 使用非 9900 临时端口，避免影响用户正在运行的服务。

## 测试用例列表

### TC-ASR-CSRF-01 ASR API 客户端自动注入 CSRF

操作步骤：

1. 执行 `pnpm --dir web test:unit src/api/asr.test.ts`。
2. 确认 `ASR admin API CSRF headers` 用例通过。

预期结果：

- `createAsrTask`、`streamAsrTranscription`、`updateDailyAgentConfig` 三类 unsafe ASR 请求都带 `X-Bifrost-CSRF`。
- 测试不通过时不能继续交付。

### TC-ASR-CSRF-02 后端 CSRF 门禁仍有效

操作步骤：

1. 执行 `bash e2e-tests/tests/test_asr_admin_csrf.sh`。
2. 脚本会构建当前 Bifrost 二进制并启动隔离数据目录服务。
3. 脚本会复跑 admin cross-site/security 回归。

预期结果：

- 不带 `X-Bifrost-CSRF` 的 unsafe admin 请求返回 403。
- cross-site 请求即使带 token 也返回 403。
- same-origin 且带有效 token 的 unsafe admin 请求通过。
- 脚本最后输出 `[asr-admin-csrf-e2e] passed`。

### TC-ASR-CSRF-03 ASR 管理页真实打开不再批量报缺 CSRF

操作步骤：

1. 使用当前源码启动临时服务：

   ```bash
   WEB_PORT=3000 BACKEND_PORT=<临时端口> pnpm --dir web dev --host 127.0.0.1
   ```

2. 打开：

   ```text
   http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr
   ```

3. 在浏览器 Network 或自动化脚本中观察 ASR 页面发出的 `/_bifrost/api/asr/**` 请求。
4. 触发至少一个 ASR 写动作，例如创建 Directory Task、保存 Daily Agent 配置，或提交 ASR 上传请求。

预期结果：

- ASR 页面加载过程没有弹出 `Missing or invalid admin CSRF token`。
- 触发写动作时，请求 headers 包含 `X-Bifrost-CSRF`。
- 对应接口不因缺 CSRF 返回 403；如因模型、目录或平台条件失败，错误应是业务错误而不是 CSRF 错误。

## 清理步骤

1. 停止临时 Bifrost 服务和 Vite dev server。
2. 删除临时 `BIFROST_DATA_DIR`。
3. 确认没有残留测试进程监听临时端口。

## 本次执行结果

- 2026-06-26：TC-ASR-CSRF-01 通过。执行 `pnpm --dir web test:unit src/api/asr.test.ts`，6/6 passed；新增 `ASR admin API CSRF headers` 覆盖 `createAsrTask`、`streamAsrTranscription`、`updateDailyAgentConfig`，确认 unsafe ASR 请求携带 `X-Bifrost-CSRF`。
- 2026-06-26：TC-ASR-CSRF-02 通过。执行 `SKIP_ADMIN_SECURITY_E2E=true bash e2e-tests/tests/test_asr_admin_csrf.sh` 通过 ASR 前端 CSRF E2E；执行 `BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_admin_cross_site_security.sh` 通过，后端仍拒绝缺 token / cross-site 请求并接受 same-origin + token 请求；执行完整 `bash e2e-tests/tests/test_asr_admin_csrf.sh` 通过，脚本构建当前源码二进制后输出 `[asr-admin-csrf-e2e] passed`。
- 2026-06-26：TC-ASR-CSRF-03 通过可验证部分。启动 `WEB_PORT=3000 BACKEND_PORT=9900 pnpm --dir web dev --host 127.0.0.1`，用浏览器打开 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr`；页面标题为 `Bifrost Proxy`，DOM 包含 ASR 内容，浏览器日志中 `Missing or invalid admin CSRF token` / CSRF 相关错误数量为 0。由于该验证连接用户当前 9900 服务，未创建真实 Directory Task 或修改 Daily Agent 配置，避免污染用户数据。
