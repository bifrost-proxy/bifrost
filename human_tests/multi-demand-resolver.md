# 多需求 Resolver 两相缓存改造真实场景测试

## 功能模块说明

本用例覆盖 Bifrost 规则解析器在多需求并行开发场景下的 header 分流能力。目标是验证同一个 URL、同一个域名 pattern、同一个转发协议下，多条 `includeFilter://h:x-tt-env-fe=...` 规则不会因为 resolver 缓存复用旧请求的最终决策，而是能按当前请求 header 路由到对应 dev server。

## 前置条件

- 当前目录为仓库根目录 `/Users/bytedance/project/bifrost`。
- Rust toolchain 可用。
- 测试命令会使用临时端口和测试 runner 自带的 mock upstream，不需要启动系统代理。
- 如果手动启动 Bifrost 服务，必须设置独立 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。本用例使用 `bifrost-e2e` 内置代理实例，不修改系统代理。

## 测试用例列表

### TC-MDR-01：resolver 单元回归验证同 URL 不同 header 重新执行 filter gate

操作步骤：
1. 执行：
   ```bash
   cargo test -p bifrost-core rule::resolver::tests::test_cached_candidates_recheck_header_filters_per_request -- --exact
   ```
2. 确认测试通过。
3. 执行：
   ```bash
   cargo test -p bifrost-core rule::resolver::tests::test_cached_candidates_recheck_method_filters_per_request -- --exact
   ```
4. 确认测试通过。

预期结果：
- 第一个测试中，`x-tt-env-fe=featA-web` 命中 `127.0.0.1:3000`，随后同 URL 的 `x-tt-env-fe=featA-mobile` 命中 `127.0.0.1:3001`。
- 第二个测试中，POST 请求命中规则，随后同 URL GET 请求不命中 POST-only 规则。
- 两个测试都在缓存开启的默认 resolver 下通过。

### TC-MDR-02：真实代理链路验证同 URL 不同 `x-tt-env-fe` 路由到不同 upstream

操作步骤：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test filters_includeFilter_header_route_split_cache_regression --test-timeout 120
   ```
2. 确认 e2e runner 输出 1 个测试通过。

预期结果：
- e2e runner 启动两个 mock upstream：web upstream 返回 `feature_web_server`，mobile upstream 返回 `feature_mobile_server`。
- 通过同一个代理地址请求同一个 `http://multi-demand.local/api`。
- 请求头 `x-tt-env-fe: featA-web` 返回体包含 `feature_web_server`。
- 请求头 `x-tt-env-fe: featA-mobile` 返回体包含 `feature_mobile_server`。
- 第二次请求不得复用第一次请求的 host 解析结果。

## 清理步骤

- `bifrost-e2e` 测试结束后自动停止代理实例和 mock upstream。
- 如命令被中断，执行 `ps -ef | rg 'bifrost-e2e|target/.*/bifrost'` 检查残留进程，只清理本用例启动的测试进程。
- 不保留临时数据目录。
