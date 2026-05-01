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

## 2026-05-01：Windows 并行 rules 共享 mock 掉线恢复

### 背景
- Windows rules CI 使用共享 mock servers 承载 HTTP/HTTPS/WS/SSE/Proxy 夹具；当共享 HTTP echo 进程在并行套件中途掉线时，大量套件会同时报 `Mock 服务器未运行`、`REQUEST_CONNECT_REFUSED` 或 `os error 10061`。
- 旧 runner 会把这类基础设施掉线当作普通测试失败处理，并受 `BIFROST_E2E_MAX_RETRY_SUITES` 限制只重试前若干套件。CI 中出现 59 个套件失败、仅前 6 个重试通过，其余仍保持失败，导致概率性红灯。
- CI 复跑中又出现 11 个套件均因 mock 掉线失败但只重试前 6 个的情况。原因是失败统计逻辑读取 `test_<idx>.log`，而实际 suite 日志写入 `log_<idx>.txt`，导致全量 mock outage 判定始终漏检。
- 后续稳定性复跑中，Windows x86 rules 可在 `E2E rules tests` 步骤长时间无结果后失败，且 GitHub 未上传 failure artifact。原因是 Windows 分支通过管道后台运行 `command | tee | sed`，记录到的 PID 是日志管道尾部而不是真正的 suite runner，`BIFROST_E2E_SUITE_TIMEOUT` 无法可靠终止真实命令并进入后续日志上传步骤。

### 实现逻辑
- `run_all_tests_parallel.sh` 在 Windows 平台强制 rules E2E 串行执行，避免多 worker 同时共享同一组 mock servers 时放大 fixture 掉线风险。
- 重试前扫描失败套件日志；若所有失败都指向共享 mock 掉线或连接拒绝，则判定为基础设施 outage，跳过普通失败数量上限，重启 mock servers 后串行重试全部失败套件。
- 基础设施 outage 重试预算至少提升到 900 秒，避免大量套件在共享 mock 恢复后因默认预算过短被截断。
- 失败统计优先读取 runner 实际生成的 `log_<idx>.txt`，同时保留历史 `test_<idx>.log` fallback，避免日志文件名漂移再次让 mock outage 识别失效。
- `scripts/run_all_e2e.sh::run_and_capture` 在 Windows 下直接后台运行真实命令并把输出写入日志文件，再用独立子 shell 包裹 `tail -f | sed` 流式打印日志。这样 `command_pid` 指向真实 suite runner，watchdog 可使用 `kill_process_tree` 终止整个命令树；命令结束后也会对日志流子 shell 调用 `kill_process_tree`，避免 `tail -f` 残留导致 wrapper 自身挂住。

### 测试方案
- 脚本语法检查：`bash -n e2e-tests/run_all_tests_parallel.sh`。
- 包装器语法检查：`bash -n scripts/run_all_e2e.sh`。
- E2E 测试：执行缩小分类的 rules runner，验证共享 mock servers 启停、并行参数解析、失败重试入口不回归。
- 日志路径回归：检查 `result_failure_mentions_mock_outage` 会读取 `log_<idx>.txt`，确保 `count_mock_outage_failures` 能正确识别 suite 日志中的 mock outage。
- Windows 超时回归：检查 `run_and_capture` 的 Windows 分支后台运行真实命令后再 `tail -f` 日志，watchdog 对真实命令 PID 调用 `kill_process_tree`。
- 真实场景测试：更新 `human_tests/rules-e2e-fixtures.md`，新增 Windows 共享 mock outage、日志路径回归和 suite timeout 诊断用例；本地执行可用的 runner 场景，Windows 串行 cap、全量 outage 重试与超时 artifact 由 GitHub Actions Windows rules job 继续验证。

### 校验要求
- rules E2E 需优先于 rust-project-validate 执行。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
