# TLS Interception Priority

## 背景

Settings 同时支持 Rules、Domain、App、Client IP 与 Global 五层 TLS 解包策略。旧实现先检查 App，再检查 Domain，导致浏览器命中 App Force Intercept 后，即使目标域名已经加入 Domain Passthrough，仍会被解包并可能注入 Bifrost Badge。

用户要求把域名作为高于应用的安全边界：`Rules > Domain > App`。因此 `chatgpt.com` 一旦命中 Domain Passthrough，就不能再被 Edge/Chrome 的 App Force Intercept 重新开启解包。

## 用户目标验证清单

### 必须实现

- TLS 配置判定优先级固定为 `Rules > Domain > App > Client IP > Global`。
- 同一层同时命中 Passthrough 与 Force Intercept 时，Passthrough 优先，避免显式“不解包”被同层白名单覆盖。
- Domain Passthrough 命中后立即返回不解包，不再评估 App Force Intercept。
- Domain Force Intercept 命中后立即解包，不再评估 App Passthrough。
- Settings 页面显示与运行时完全一致的优先级说明。

### 必须不破坏

- 规则级 `tlsPassthrough://` / `tlsIntercept://` 仍是最高优先级。
- HTTPS 到 HTTP/WS 的显式 host rewrite 仍按现有规则级自动解包语义执行。
- 非本机客户端继续跳过 App 策略，只按 Rules、Domain、Client IP、Global 判定。
- 未识别本机应用且配置了 App 策略时，保持现有安全默认：没有更高层 Domain/Rule 决定时不解包。
- 正式 `127.0.0.1:9900` 服务不用于开发测试，不重启、不改配置。

### 必须真实验证

- 单元测试覆盖 Rules、Domain、App、Client IP 和 Global 的关键冲突矩阵。
- 隔离 E2E 实例配置 Domain Passthrough 与 curl App Force Intercept，请求本地 HTTPS HTML；响应保持上游原文且不含 `__bifrost_badge__`。
- E2E 流量记录只出现 CONNECT tunnel，不出现对应内层 HTTPS 明文记录。
- Settings Playwright 用例确认亮色和暗色主题都展示新优先级文案。
- `human_tests/tls-interception-priority.md` 中的用例创建后立即逐条执行。

## 产品语义

从高到低：

1. Rules：规则解析结果 `tlsPassthrough://` / `tlsIntercept://`。
2. Domain：Domain Passthrough，再到 Domain Force Intercept。
3. App：App Passthrough，再到 App Force Intercept；只适用于本机客户端。
4. Client IP：Client IP Passthrough，再到 Client IP Force Intercept。
5. Global：全局 HTTPS Interception 开关。

同一层采用 Passthrough-first，是因为“不解包”是更保守的安全决定。不同层严格遵守 scope 顺序，因此 Domain Force Intercept 仍高于 App Passthrough；需要覆盖 Domain 决定时必须使用 Rules。

## 实现方案

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - `should_intercept_tls_for_client` 在规则和 host rewrite 后，先计算 Domain exclude/include，再计算 App exclude/include，随后计算 Client IP exclude/include，最后回退 Global。
  - 各层 exclude 在 include 之前检查。
  - 保留未识别应用的 fail-closed 分支，但把 Domain 判定放在该分支之前，避免重复并保证 Domain 优先。
- `web/src/pages/Settings/tabs/ProxyTab.tsx`
  - 更新静态说明为运行时真实顺序。
  - 保持现有 Ant Design token、布局和亮暗主题，不新增视觉 token。
- `docs/cli-quick-start.md`、`docs/cli.md`
  - 同步优先级与参数说明，删除 App Include “最高优先级”的旧表述。

## 测试方案

### 单元测试

- Domain Passthrough > App Force Intercept。
- Domain Force Intercept > App Passthrough。
- Domain Passthrough > Domain Force Intercept。
- App Passthrough > App Force Intercept。
- App Passthrough/Force Intercept > Client IP Force Intercept/Passthrough。
- Client IP Passthrough > Client IP Force Intercept。
- Rules override > Domain/App 配置。
- 未识别本机应用与非本机客户端边界保持不变。

### E2E

扩展 `e2e-tests/tests/test_tls_intercept_e2e.sh`：

1. 使用临时 `BIFROST_DATA_DIR` 和可通过环境变量配置的非 9900 隔离端口启动当前源码二进制。
2. 本地 HTTPS mock 返回 `text/html`，且不包含 Badge。
3. 配置 `intercept_exclude=["127.0.0.1"]` 与 `app_intercept_include=["*curl*"]`，保持 Badge 开启。
4. curl 经代理访问 mock。
5. 断言响应不含 `__bifrost_badge__`，并通过 Traffic API 断言该请求只有 CONNECT tunnel、没有内层 HTTPS 明文记录。

### Web UI

在现有 Settings Playwright 用例中断言优先级文案；切换到 dark theme 后再次断言可见，证明文本在双主题下保持可读。

### 真实场景

新增 `human_tests/tls-interception-priority.md`，覆盖优先级静态契约、定向单元测试、隔离 E2E、Settings 文案双主题和正式 9900 进程保持不变。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户原始目标和本优先级表。
- 检查 `git status --short`、`git diff` 与新增文件。
- Review 决策链是否存在旧的 App-first 分支或 include-first 测试。
- 执行定向 Rust unit、TLS shell E2E、Settings Playwright。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff 再次复核 Rules/Domain/App/IP/Global 全矩阵。
- 检查 docs、WebUI、E2E、human_tests 与实现是否同序。
- 复跑失败路径和受影响测试，确认不需要第 3 轮。

## 覆盖率门禁

本地不运行高成本 `make coverage` 或 `coverage-all.sh --gate`。推送后由 CI 的 `bash scripts/ci/coverage-all.sh --json --gate`、各 crate 棘轮阈值及 changed-lines 95% 门禁兜底。
