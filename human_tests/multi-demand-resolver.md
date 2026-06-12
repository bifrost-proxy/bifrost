# 多需求 Resolver 两相缓存改造真实场景测试

## 功能模块说明

本用例覆盖 Bifrost 规则解析器在多需求并行开发场景下的 header 分流能力。目标是验证同一个 URL、同一个域名 pattern 下，多条 `includeFilter://h:x-feature-env=...` 规则不会因为 resolver 缓存复用旧请求的最终决策，而是能按当前请求 header 重新执行 filter gate。

覆盖范围包含五类用户可感知代理行为：

- 转发类：`host://` 按 header 路由到不同 upstream；前序规则被 include/exclude filter 跳过时可继续 fallthrough 到后续同协议规则或 fallback。
- 修改类：`reqHeaders://`、`resHeaders://`、`replaceStatus://` 按 header 应用不同修改。
- 其他直接代理决策类：`redirect://` 按 header 返回不同 Location，`statusCode://` 按 header 返回不同状态码。
- 排除类：`excludeFilter://` 按当前请求 header 决定是否排除候选规则。
- 跳过类：`skip://` 自身带 filter 时按当前请求 header 决定是否跳过目标规则。

## 前置条件

- 当前目录为仓库根目录。
- Rust toolchain 可用。
- 测试命令会使用临时端口和测试 runner 自带的 mock upstream，不需要启动系统代理。
- 如果手动启动 Bifrost 服务，必须设置独立 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。本用例使用 `bifrost-e2e` 内置代理实例，不修改系统代理。

## 测试用例列表

### TC-MDR-01：resolver 单元回归验证同 URL 不同 header 重新执行 filter gate

操作步骤：
1. 执行：
   ```bash
   cargo test -p bifrost-core rule::resolver::tests::multi_demand:: -- --nocapture
   ```
2. 确认测试通过。

预期结果：
- `test_cached_candidates_recheck_header_filters_per_request`：`x-feature-env=feature-web` 命中 `127.0.0.1:3000`，同 URL 的 `x-feature-env=feature-mobile` 命中 `127.0.0.1:3001`，未知 header fallthrough 到 `127.0.0.1:3999`。
- `test_cached_candidates_recheck_method_filters_per_request`：POST 请求命中规则，随后同 URL GET 请求不命中 POST-only 规则。
- `test_cached_candidates_recheck_disabled_state_per_request`：候选缓存存在时，rule disabled 状态仍按当前规则状态重新判断；前序 host 规则被禁用后应 fallthrough 到 fallback host。
- `test_cached_candidates_recheck_exclude_filters_per_request_both_orders`：同 URL 下带 `x-block: true` 的请求被前序 `excludeFilter` 排除并 fallthrough 到后续 host，不带 header 的请求命中前序 host，两个请求顺序都不污染缓存。
- `test_cached_candidates_recheck_request_modify_filters_per_request`：同 URL 不同 header 只产生对应 `reqHeaders://` 修改规则。
- `test_cached_candidates_recheck_response_modify_filters_per_request`：同 URL 不同 header 只产生对应 `resHeaders://` 与 `replaceStatus://` 修改规则。
- `test_cached_candidates_recheck_direct_proxy_filters_per_request`：同 URL 不同 header 只产生对应 `redirect://` 与 `statusCode://` 规则。
- `test_cached_candidates_recheck_skip_filters_per_request`：同 URL 下 `skip://` 规则自身的 includeFilter 按当前请求 header 重新判断。
- 所有测试都在缓存开启的默认 resolver 下通过。

### TC-MDR-02：真实代理链路验证同 URL 不同 `x-feature-env` 路由到不同 upstream

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_includeFilter_header_route_split_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- e2e runner 启动三个 mock upstream：web upstream 返回 `feature_web_server`，mobile upstream 返回 `feature_mobile_server`，fallback upstream 返回 `feature_fallback_server`。
- 通过同一个代理地址请求同一个 `http://multi-demand.local/api`。
- 请求头 `x-feature-env: feature-web` 返回体包含 `feature_web_server`。
- 请求头 `x-feature-env: feature-mobile` 返回体包含 `feature_mobile_server`。
- 请求头 `x-feature-env: feature-unknown` 返回体包含 `feature_fallback_server`。
- 执行顺序为 mobile -> fallback -> web -> mobile，缓存命中后不得复用前一次请求的 host 解析结果。

### TC-MDR-03：真实代理链路验证同 URL 不同 `x-feature-env` 应用不同修改规则

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_includeFilter_header_modify_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- e2e runner 启动一个 mock upstream。
- 通过同一个代理地址请求同一个 `http://multi-modify.local/api`。
- 请求头 `x-feature-env: feature-web` 时，上游收到 `X-Selected-Env: web`，响应包含 `X-Selected-Env: web`，状态码为 201。
- 请求头 `x-feature-env: feature-mobile` 时，上游收到 `X-Selected-Env: mobile`，响应包含 `X-Selected-Env: mobile`，状态码为 202。
- 执行顺序为 mobile -> web -> mobile，缓存命中后不得复用前一次请求的修改类解析结果。

### TC-MDR-04：真实代理链路验证同 URL 不同 `x-feature-env` 返回不同直接代理决策

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_includeFilter_header_redirect_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- 通过同一个代理地址请求同一个 `http://multi-direct.local/api`。
- 请求头 `x-feature-env: feature-web` 返回 302，`Location` 为 `https://web.example.test/landing`。
- 请求头 `x-feature-env: feature-mobile` 返回 302，`Location` 为 `https://mobile.example.test/landing`。
- 通过同一个代理地址请求同一个 `http://multi-status.local/api`。
- 请求头 `x-feature-env: feature-web` 返回 201。
- 请求头 `x-feature-env: feature-mobile` 返回 202。
- 执行顺序为 mobile -> web -> mobile，缓存命中后不得复用前一次请求的 redirect/statusCode 解析结果。

### TC-MDR-05：真实代理链路验证同 URL `excludeFilter` 按当前 header 重新判断并 fallthrough

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_excludeFilter_header_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- e2e runner 启动两个 mock upstream。
- 通过同一个代理地址请求同一个 `http://exclude-cache.local/api`。
- 请求头 `x-block: true` 时，`excludeFilter://h:x-block=true` 排除前序 host 规则，响应体包含 `exclude_fallback_server`。
- 不带 `x-block` header 时，前序 host 规则生效，响应体包含 `exclude_primary_server`。
- 执行顺序为 blocked -> allowed -> blocked，缓存命中后不得复用前一次请求的 excludeFilter 判断结果。

### TC-MDR-06：真实代理链路验证同 URL `skip://` filter 按当前 header 重新判断

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_skip_header_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- e2e runner 启动一个 mock upstream。
- 通过同一个代理地址请求同一个 `http://skip-cache.local/api`。
- 请求头 `x-skip: true` 时，`skip://operation=resHeaders://X-Skip-Target=selected` 生效，响应不包含 `X-Skip-Target`。
- 不带 `x-skip` header 时，skip 规则不生效，响应包含 `X-Skip-Target: selected`。
- 执行顺序为 skipped -> selected -> skipped，缓存命中后不得复用前一次请求的 skip 判断结果。

## 实际执行记录

- 2026-06-11：TC-MDR-01 执行 `cargo test -p bifrost-core rule::resolver::tests::multi_demand:: -- --nocapture`，结果 8 passed。
- 2026-06-11：TC-MDR-02 执行 `cargo run -p bifrost-e2e -- --test filters_includeFilter_header_route_split_cache_regression --test-timeout 120`，结果 1 passed。
- 2026-06-11：TC-MDR-03 执行 `cargo run -p bifrost-e2e -- --test filters_includeFilter_header_modify_cache_regression --test-timeout 120`，结果 1 passed。
- 2026-06-11：TC-MDR-04 执行 `cargo run -p bifrost-e2e -- --test filters_includeFilter_header_redirect_cache_regression --test-timeout 120`，结果 1 passed。
- 2026-06-11：TC-MDR-05 执行 `cargo run -p bifrost-e2e -- --test filters_excludeFilter_header_cache_regression --test-timeout 120`，结果 1 passed。
- 2026-06-11：TC-MDR-06 执行 `cargo run -p bifrost-e2e -- --test filters_skip_header_cache_regression --test-timeout 120`，结果 1 passed。
- 2026-06-11：二次 CR 补充 fallback host 与 excludeFilter host fallthrough 覆盖后，复跑 TC-MDR-01、TC-MDR-02、TC-MDR-05 和 `cargo run -p bifrost-e2e -- --test cache_regression --test-timeout 120`，结果分别为 8 passed、1 passed、1 passed、5 passed。
- 2026-06-12：CR 追问 disabled 缓存证明力后，将 `test_cached_candidates_recheck_disabled_state_per_request` 增强为禁用前序 host 后 fallthrough 到 fallback host，并执行 `cargo test -p bifrost-core rule::resolver::tests::multi_demand::test_cached_candidates_recheck_disabled_state_per_request -- --nocapture`，结果 1 passed。

## 清理步骤

- `bifrost-e2e` 测试结束后自动停止代理实例和 mock upstream。
- 如命令被中断，执行 `ps -ef | rg 'bifrost-e2e|target/.*/bifrost'` 检查残留进程，只清理本用例启动的测试进程。
- 不保留临时数据目录。
