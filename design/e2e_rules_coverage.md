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
- Windows rules job 只运行 `x86_64-pc-windows-msvc` 全量 rules fixture，并把 suite watchdog 收敛到 1200 秒、job timeout 收敛到 30 分钟。Windows ARM runner 当前会把 rules fixture 强制串行执行；CI 实测 shard 1/2/3 在 ARM 上连续触发 90 秒请求超时重试，20 分钟内只能完成少量失败 fixture，无法作为合入门禁。Windows ARM 仍由独立 CLI build、runner E2E 与 desktop sidecar/check 覆盖平台二进制可用性。
- Windows rules job 只依赖 `build-cli-windows` 上传的 `bifrost-release-x86_64-pc-windows-msvc` CLI artifact。Windows desktop bundle/check 继续作为独立验证 job 运行，但不阻塞 rules E2E，因为 rules 只需要 CLI 二进制。

### 测试方案
- 脚本语法检查：`bash -n e2e-tests/run_all_tests_parallel.sh`。
- 包装器语法检查：`bash -n scripts/run_all_e2e.sh`。
- E2E 测试：执行缩小分类的 rules runner，验证共享 mock servers 启停、并行参数解析、失败重试入口不回归。
- 日志路径回归：检查 `result_failure_mentions_mock_outage` 会读取 `log_<idx>.txt`，确保 `count_mock_outage_failures` 能正确识别 suite 日志中的 mock outage。
- Windows 超时回归：检查 `run_and_capture` 的 Windows 分支后台运行真实命令后再 `tail -f` 日志，watchdog 对真实命令 PID 调用 `kill_process_tree`。
- Windows 预算回归：检查 `.github/workflows/ci.yml` 中 Windows rules job 只保留 x86_64 矩阵项，timeout 为 30 分钟，`BIFROST_E2E_SUITE_TIMEOUT` 为 1200 秒。
- Windows CI 依赖回归：静态解析 `.github/workflows/ci.yml`，确认 `e2e-windows-rules.needs == ["build-cli-windows"]`，`build-cli-windows` 上传 CLI artifact，`build-desktop-windows` 下载同一 artifact 但不再被 rules 依赖。
- 真实场景测试：更新 `human_tests/rules-e2e-fixtures.md`，新增 Windows 共享 mock outage、日志路径回归、suite timeout 诊断、CI 预算、rules job CLI-only 依赖与 Windows ARM rules 收敛用例；本地执行可用的 runner 场景，Windows x86 完整 rules job 与 ARM 平台 build/runner 由 GitHub Actions `CI` run 继续验证。

### 校验要求
- rules E2E 需优先于 rust-project-validate 执行。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。

## 2026-05-08：Windows x86 Rules E2E 20 分钟内完成

### 背景
- Windows rules runner 为避免共享 mock servers 在并发 fixture 下掉线，会在 Windows 平台强制串行执行。
- 单个 `x86_64-pc-windows-msvc` `1/1` job 需要串行跑完整 rules fixture 集，实际 wall time 容易贴近或超过 20 分钟预算，并在慢 runner 上触发 job timeout。
- rules job 只使用下载好的 CLI release binary、bash/Python/curl/jq 与 mock servers；Node/pnpm 依赖安装不参与 rules fixture 执行。

### 实现逻辑
- 将 Windows x86 rules CI 拆为 4 个矩阵分片：`1/4`、`2/4`、`3/4`、`4/4`，继续通过 `BIFROST_E2E_RULE_SHARD_INDEX` / `BIFROST_E2E_RULE_SHARD_TOTAL` 复用已有 runner 分片逻辑。
- 每个 shard 仍在 Windows 内部串行执行，避免重新引入共享 mock server 并发掉线风险；跨 shard 由 GitHub Actions 并行调度，把 wall time 降到约四分之一。
- 单个 shard 的 `BIFROST_E2E_SUITE_TIMEOUT` 收敛为 1080 秒，job `timeout-minutes` 收敛为 20 分钟，给失败日志 dump/upload 留出约 2 分钟余量。
- 移除 Windows rules job 的 pnpm setup、Node setup 与 `pnpm install` 步骤；rules fixture 不构建或运行 Web UI，减少每个 shard 的固定启动耗时。
- 失败 artifact 名称加入 shard index，避免多个 shard 同时失败时上传 artifact 名称冲突。

### 测试方案
- 静态解析 `.github/workflows/ci.yml`，确认 `e2e-windows-rules` 有 4 个 x86_64 shard、每个 shard total 为 4、job timeout 为 20 分钟、suite timeout 为 1080 秒。
- 语法检查 `e2e-tests/run_all_tests_parallel.sh`、`e2e-tests/test_rules.sh` 与 `scripts/run_all_e2e.sh`，确认现有 rules runner 和 wrapper 入口未被破坏。
- 分片覆盖检查：按当前 workflow 矩阵参数验证 4 个 shard 索引为 `1 2 3 4` 且没有重复/缺失，artifact 名称包含 shard 维度。
- 真实场景测试：更新并执行 `human_tests/rules-e2e-fixtures.md` 中 Windows x86 rules 20 分钟分片预算用例；完整 Windows runtime 耗时由推送后的 GitHub Actions `CI` run 继续验证。

### 校验要求
- rules E2E 需优先于 rust-project-validate 执行。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。

## 2026-05-04：macOS Rules E2E 语义回归收敛

### 背景
- `feat/agent` 最新 CI 的 macOS aarch64 Rules E2E 失败集中在内容注入、URL 修改、请求 Content-Type、lineProps disabled 与 speed 限速。
- 排查后将问题分为两类：
  - 功能缺口：HTTPS MITM tunnel 分支没有复用普通 HTTP handler 的 URL 规则与请求头规则变换，导致 `urlParams`、`urlReplace/pathReplace`、`reqType`、`reqCharset` 在 tunnel 转发到 HTTP 上游时只匹配不生效。
  - 测试缺口：`test_rules.sh` 对 html/js/css 注入仍断言 `{value}` 占位符而非代码块真实内容；`lineProps://disabled` 仍走“验证未实现”失败分支；speed 断言未考虑 throttle 初始窗口会立即释放首个窗口容量。

### 实现逻辑
- tunnel 分支在构建上游 URI 前调用 `apply_url_rules`，确保 URL 参数追加、删除与路径替换对 HTTPS MITM 请求同样生效。
- tunnel 分支在复制上游请求头前调用 `apply_req_rules`，复用 `reqType`、`reqCharset`、`reqHeaders`、`reqCookies`、`ua`、`referer`、`auth` 等请求侧协议的既有实现。
- `test_rules.sh` 对 html/js/css 注入期望值调用 `resolve_code_block_var`，用 fixture 中的 fenced code block 内容做真实响应体断言。
- `lineProps://disabled` 断言禁用规则中的响应头值没有出现在最终响应中；若存在同名 fallback 规则，则允许最终值来自 fallback。
- URL 修改夹具按当前协议语法收敛：`urlReplace/pathReplace` 使用 `from=to`，删除参数用例在请求 URL 中先携带待删除参数，URL 参数合并用 `urlParams://` 而不是 body merge 语义的 `params://`。
- `urlParams` 解析补齐 `&` 分隔多参数语义，与历史 `urlParams://(key=value&key2=value2)` 规则和 template fixture 中的内联参数写法保持一致；继续保留逗号和换行分隔。
- speed 断言按当前 `ThrottledBoxBody` 设计计算最小耗时：首个限速窗口容量可立即发送，剩余字节才需要等待后续窗口。

### 测试方案
- 单元测试：执行 `cargo test -p bifrost-proxy --utils::url -- --nocapture` 验证 URL 规则工具函数仍通过。
- E2E 测试：逐个执行 CI 失败 rules fixture：
  - `e2e-tests/rules/content_inject/html.txt`
  - `e2e-tests/rules/content_inject/js.txt`
  - `e2e-tests/rules/content_inject/css.txt`
  - `e2e-tests/rules/control/enable_disable.txt`
  - `e2e-tests/rules/advanced/content_type.txt`
  - `e2e-tests/rules/request_modify/url_params.txt`
  - `e2e-tests/rules/advanced/speed.txt`
  - `e2e-tests/rules/template/values.txt`
  - `e2e-tests/rules/combination/multi_rules.txt`
- 真实场景测试：更新并执行 `human_tests/rules-e2e-fixtures.md` 中的 macOS Rules CI 回归用例，所有命令使用临时数据目录、非 9900 端口且保持 `--no-system-proxy`。

### 校验要求
- rules E2E 需优先于 rust-project-validate 执行。
- 收尾阶段执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
