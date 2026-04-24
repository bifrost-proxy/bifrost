# 规则协议用例补齐与修复方案

## 目标
- 覆盖规则协议中待验证、部分覆盖、未覆盖的用例
- 修复协议实现链路中未接通或缺失的行为
- 通过端到端规则测试与相关脚本验证

## 范围
- 规则文件：e2e-tests/rules/**.txt
- 规则执行：e2e-tests/test_rules.sh
- 协议解析与执行：crates/bifrost-core、crates/bifrost-cli、crates/bifrost-proxy

## 设计要点
- 规则文件补齐以“最小可验证行为”为原则，避免引入额外依赖
- 协议实现补齐优先顺序：核心路径不可用的协议（如 trailers、reqSpeed、urlReplace/pathReplace）优先
- 过滤器与匹配类协议以最小复现用例定位问题并修复

## 变更清单
- 新增或扩展规则用例文件，补齐待测协议
- 补齐协议链路：解析 → 转换 → 执行
- 更新 COVERAGE.md 与相关说明

## 验证策略
- 规则用例：test_rules.sh 按分类运行
- 专项脚本：test_pattern.sh / test_values_*.sh
- 失败用例定位后回归，确保全量通过

## 2026-04-24：并行规则夹具端口占位符兼容

### 背景
- `run_all_tests_parallel.sh` 会为共享 mock 服务器动态分配 HTTP/HTTPS/WS/SSE/Proxy 端口，并通过 `ECHO_*_PORT` 环境变量传入 `test_rules.sh`。
- 部分 replay/tls 历史夹具仍使用 `__MOCK_HTTP_PORT__`、`__MOCK_HTTPS_PORT__` 这类旧占位符。
- 若预处理只替换 `__ECHO_*_PORT__`，规则会原样加载 `http://127.0.0.1:__MOCK_HTTP_PORT__/`，转发目标无效，`replay/forward_localhost_api.txt` 会表现为 HTTP 502。

### 实现逻辑
- 在 `test_rules.sh::preprocess_rules_file` 中将 legacy `__MOCK_*_PORT__` 映射到当前 runner 分配的 `ECHO_*_PORT`。
- 保留 `__ECHO_*_PORT__` 作为推荐新命名，避免修改已有 replay 夹具语义。
- 不改变 mock server 启停、动态端口分配、代理启动参数或系统代理行为。

### 测试方案
- 单元级检查：对 `preprocess_rules_file` 的 sed 替换链路做语法和文本检查，确认 `__MOCK_HTTP_PORT__` 不会残留到处理后的规则文件。
- E2E 测试：执行 `e2e-tests/test_rules.sh` 的 `replay/forward_localhost_api.txt`，断言 HTTP->HTTP 转发返回 2xx 且后端协议为 HTTP。
- 并行回归：执行 `e2e-tests/run_all_tests_parallel.sh --no-build --retry-failed-once` 或缩小范围的 replay 夹具，确认并行 runner 注入的动态端口可被 legacy 占位符消费。
- 真实场景测试：更新 `human_tests/rules-e2e-fixtures.md`，按用例执行 CLI 场景，确认临时数据目录、非 9900 端口和 `--no-system-proxy` 均满足要求。

### 校验要求
- 规则 E2E 需优先于 rust-project-validate 执行。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
