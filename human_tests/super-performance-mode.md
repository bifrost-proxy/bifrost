# Super Performance Mode 真实场景测试

## 功能模块说明

验证超级性能模式在真实 CLI/API/WebUI 场景中的行为：代理规则仍执行，但 Network 流量记录、body 缓存、WebSocket/SSE 持久化和 traffic DB 记录均不产生。该模式默认关闭，可通过 `bifrost start --super-performance-mode` 或 Settings > Performance 顶部开关启用。

## 前置条件

- 当前仓库已构建可执行 Bifrost CLI：`target/debug/bifrost` 或 `target/release/bifrost`。
- 本地可用 `curl`、`jq`、`node`、`python3`。
- 测试必须使用临时 `BIFROST_DATA_DIR` 和动态端口，不修改系统代理。
- WebUI 测试使用 Playwright UI 测试环境。

## 测试用例列表

### TC-SPM-01 默认关闭与配置字段

操作步骤：

1. 运行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-storage test_unified_config_default -- --nocapture`。
2. 运行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-storage test_update_traffic_config_accepts_valid_fields -- --nocapture`。

预期结果：

- 默认配置中 `traffic.super_performance_mode=false`。
- 配置更新路径能持久化 `super_performance_mode=true`。

### TC-SPM-02 运行时禁写记录与 body 缓存

操作步骤：

1. 运行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin -p bifrost-proxy -p bifrost-cli super_performance -- --nocapture`。

预期结果：

- `AdminState::record_traffic` 与 `update_traffic_by_id` 在超级性能模式下不创建 DB 记录。
- 请求体和响应体存储 helper 返回 `None`，且不创建 body cache 文件。

### TC-SPM-03 真实代理链路：规则执行但流量记录为空

操作步骤：

1. 运行 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_super_performance_mode.sh`。

预期结果：

- Bifrost 通过 `--super-performance-mode` 启动后，`GET /_bifrost/api/config/performance` 返回 `traffic.super_performance_mode=true`。
- 代理请求到本地 upstream 成功。
- 响应头包含规则注入的 `X-Bifrost-Super-Mode: on`。
- `GET /_bifrost/api/traffic?limit=100` 和 `POST /_bifrost/api/traffic/query` 均返回 0 条记录。
- 临时 `body_cache` 下没有请求/响应 body 文件。

### TC-SPM-04 Network 浮层与 Settings 高亮跳转

操作步骤：

1. 运行 `pnpm --dir web exec playwright test web/tests/ui/admin-settings.spec.ts -g "Network 超级性能模式浮层可跳转并高亮 Performance 开关"`。

预期结果：

- 通过 Admin API 打开超级性能模式后，Network 表格区域显示磨砂浮层。
- 浮层提示说明当前处于超级性能模式，Network 录制不可用。
- 点击浮层按钮跳转到 `/settings?tab=performance&highlight=super-performance-mode`。
- Settings > Performance 顶部 Super Performance Mode 开关处于开启状态并高亮。

### TC-SPM-05 压测对比与零记录门禁

操作步骤：

1. 运行 `node --check scripts/loadtest-super-performance-mode.mjs`。
2. 运行 `SUPER_PERF_LOADTEST_REQUESTS=500 SUPER_PERF_LOADTEST_CONCURRENCY=32 BIFROST_BIN=target/debug/bifrost node scripts/loadtest-super-performance-mode.mjs`。

预期结果：

- 脚本语法检查通过。
- 脚本分别执行 normal 和 super 两轮真实代理压测。
- 报告写入 `.artifacts/loadtest/super-performance-*.json`。
- 报告包含请求数、并发、RPS、p50/p95/p99、错误数、规则响应头命中数和最终 traffic 总数。
- super 模式 `trafficTotal=0`。

## 清理步骤

- E2E 和压测脚本会自动停止 Bifrost 子进程、关闭本地 upstream 并删除临时数据目录。
- 如脚本异常退出，执行：

```bash
pkill -f "bifrost.*super-performance" || true
rm -rf .bifrost-super-perf-* .artifacts/loadtest/super-performance-*.tmp
```

## 执行记录

2026-07-10 本轮执行结果：

- TC-SPM-01：通过。
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-storage test_unified_config_default -- --nocapture`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-storage test_update_traffic_config_accepts_valid_fields -- --nocapture`
- TC-SPM-02：通过。
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin -p bifrost-proxy -p bifrost-cli super_performance -- --nocapture`
- TC-SPM-03：通过。
  - `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_super_performance_mode.sh`
  - 8 条断言全部通过：配置开启、规则响应头注入、upstream 可达、traffic list/query 均为 0、body cache 无文件。
- TC-SPM-04：通过。
  - `pnpm --dir web exec playwright test web/tests/ui/admin-settings.spec.ts -g "Network 超级性能模式浮层可跳转并高亮 Performance 开关"`
  - 1 条 Playwright 用例通过。
- TC-SPM-05：通过。
  - `node --check scripts/loadtest-super-performance-mode.mjs`
  - `SUPER_PERF_LOADTEST_REQUESTS=500 SUPER_PERF_LOADTEST_CONCURRENCY=32 BIFROST_BIN=target/debug/bifrost node scripts/loadtest-super-performance-mode.mjs`
  - 报告：`.artifacts/loadtest/super-performance-2026-07-10T15-06-17-032Z.json`
  - normal：500 ok / 0 error / 602.28 RPS / p95 63.9ms / trafficTotal 500。
  - super：500 ok / 0 error / 568.61 RPS / p95 75.34ms / trafficTotal 0。
