# 规则协议用例补齐与修复方案

## 背景

Bifrost 的规则引擎（`crates/bifrost-core::rules`）实现了 100+ 种规则协议：URL 匹配、请求/响应改写、内容注入、限速、Content-Type/Charset 修正、断点、代理转发、DNS 覆盖、模板 values、include、@disabled、@important 等。真实的用户场景组合非常多，仅靠单元测试覆盖协议解析并不足够，必须通过端到端的规则夹具（`e2e-tests/rules/**.txt` + `e2e-tests/test_rules.sh` + mock echo/proxy 服务）验证真实代理链路里"匹配到并按协议执行到底"。

`e2e-tests/rules/COVERAGE.md` 记录了每个协议的覆盖状态。历史上存在三类缺口：

1. **规则文件缺口**：协议实现存在，但没有 fixture 触发或没有断言。
2. **协议实现缺口**：某些协议在解析器可识别，但在 handler（tunnel 分支、HTTP/1.1 vs HTTP/2、gzip 与流式）里没有真正生效。
3. **runner 缺口**：并行 runner 动态分配端口、共享 mock server 掉线、Windows 平台超时、日志文件命名漂移等基础设施问题，导致 fixture 通过率不稳定。

本文档把上述三类缺口的补齐方案统一起来，覆盖设计意图、修复过的具体事件、后续如何测试、以及 Review/Fix/Test 闭环。

## 用户目标验证清单

### 必须实现

- 每一条 `COVERAGE.md` 里被列为"待验证 / 部分覆盖 / 未覆盖"的协议，都要在 `e2e-tests/rules/<category>/*.txt` 有可运行的 fixture，`test_rules.sh` 能按分类执行并给出通过/失败结论。
- 协议链路（解析 → 转换 → 执行）在 HTTP/HTTPS(MITM tunnel)/HTTP2/WS/SSE 上下文中必须一致生效。
- 并行 runner (`run_all_tests_parallel.sh`) 与 wrapper (`scripts/run_all_e2e.sh`) 必须支持共享 mock server 掉线自动识别与重试。
- Windows 与 macOS 平台上 rules E2E 都能稳定运行；Windows 允许通过 shard + 串行的方式收敛超时预算。
- 规则夹具兼容 legacy `__MOCK_*_PORT__` 与新 `__ECHO_*_PORT__` 占位符，避免历史 replay/tls fixture 无法转发。

### 必须不破坏

- 现有 `test_rules.sh` CLI 兼容参数（`--category`、`--filter`、`--parallel`、`--no-build`、`--retry-failed-once`）行为不变。
- 单条 fixture 的旧文本内容（例如 replay 里 `__MOCK_*` 写法）无需批量重写，preprocess 层负责兼容。
- Mock echo/proxy 服务 API 不改动，rules fixture 仍能直接命中已知路径。
- `values://`、`@rule`、`@disabled`、`@important` 语义、matcher priority 与 specificity 保持一致。
- CI YAML `.github/workflows/ci.yml` 的其他 job 结构不受影响。

### 必须真实验证

- 全量 rules 分类脚本 `bash e2e-tests/test_rules.sh --parallel` 在本机 macOS 上通过；Windows 全量通过由 CI runner 覆盖。
- 至少覆盖 replay 夹具、`content_inject`、`request_modify/url_params`、`advanced/speed`、`combination/multi_rules`、`template/values`、`control/enable_disable` 这些历史高故障文件。
- Windows 共享 mock outage 与 suite watchdog 场景在 CI run 里验证过一次全量通过。

## 产品语义

### 规则夹具是"最小可验证行为"

每个 fixture 文件遵循的原则：

- 只声明触发目标协议所需的最小规则集。
- 断言用 mock echo 服务的 response header/body 里能稳定观察的字段。
- 通过 `echo://` 与 `resBody://(...)` 让 fixture 对上游依赖降到最低。
- 允许通过内联 `values://` 或独立 values 文件参数化端口、host、path。

### 协议实现按核心路径优先

修复优先级：

1. **核心路径不可用**：例如 `trailers`、`reqSpeed`、`urlReplace/pathReplace` 在 HTTPS MITM tunnel 分支缺失 → 高优先级。
2. **过滤器 / 匹配类协议**：需要在 matcher priority 与 specificity 前提下最小复现，再修复。
3. **纯语法/展示类协议**：例如 log tag、metric 字段扩展，作为最低优先级。

### runner 稳定性优先于 fixture 通过率

一次 CI 出现红灯时，先判断是"业务规则回归"还是"共享 mock outage / 超时 / 日志漂移"。如果是基础设施掉线，必须先修 runner，避免同一红灯误判为业务回归。

## 技术细节

### 规则文件

- 目录：`e2e-tests/rules/<category>/*.txt`。
- 分类：`advanced`、`admin_api`、`combination`、`content_inject`、`control`、`devtools`、`forwarding`、`hot_reload`、`http3`、`logging`、`pattern`、`priority`、`redirect`、`references`、`regression`、`replay`、`request_modify`、`response_modify`、`template`、`tls`、`values` 等。
- 覆盖清单：`e2e-tests/rules/COVERAGE.md`；每条协议对应一条 fixture 或明确标记覆盖状态。

### `test_rules.sh` 关键路径

- 环境变量：`ECHO_HTTP_PORT`（默认 3000）、`ECHO_HTTPS_PORT`、`ECHO_WS_PORT`、`ECHO_SSE_PORT`、`ECHO_PROXY_PORT`；并行 runner 会动态分配这些端口并传入。
- `preprocess_rules_file`：把 `__ECHO_*_PORT__`、`__MOCK_*_PORT__` 占位符替换成真实端口，兼容历史 replay/tls fixture。
- 每条 fixture 独立启动 Bifrost（临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`），发起 curl/websocat/自定义 client，然后 diff 期望值。

### 并行 runner 与共享 mock 掉线恢复

- `run_all_tests_parallel.sh` 在 Windows 强制串行执行 rules，避免共享 mock 服务在并发下掉线。
- 失败后扫描日志 `log_<idx>.txt`（历史 `test_<idx>.log` 保留 fallback），若所有失败都指向 `Mock 服务器未运行` / `REQUEST_CONNECT_REFUSED` / `os error 10061`，判定为基础设施 outage，跳过 `BIFROST_E2E_MAX_RETRY_SUITES` 上限，重启 mock server 后串行重试全部失败套件；outage 重试预算至少 900 秒。
- `scripts/run_all_e2e.sh::run_and_capture` 在 Windows 分支后台运行真实命令，`command_pid` 指向真实 suite runner，watchdog 用 `kill_process_tree` 终止整条命令树；命令结束后同样 `kill_process_tree` 收拾日志流子 shell，避免 `tail -f` 残留。

### macOS rules 语义回归

- HTTPS MITM tunnel 分支在构建上游 URI 前调用 `apply_url_rules`（`urlParams` / `urlReplace` / `pathReplace`），在复制上游请求头前调用 `apply_req_rules`（`reqType` / `reqCharset` / `reqHeaders` / `reqCookies` / `ua` / `referer` / `auth`），与普通 HTTP handler 对齐。
- `test_rules.sh` 对 html/js/css 注入期望值调用 `resolve_code_block_var`，用 fenced code block 内容做真实响应体断言。
- `lineProps://disabled` 断言禁用规则的响应头值没有出现在最终响应；若存在同名 fallback 规则，允许最终值来自 fallback。
- `urlReplace/pathReplace` 使用 `from=to` 语法；删除参数在请求 URL 中携带待删参数；参数合并用 `urlParams://`（不是 body merge 的 `params://`）。`urlParams` 解析补齐 `&` 分隔多参数语义。
- speed 断言按 `ThrottledBoxBody` 设计：首个限速窗口容量可立即发送，只有剩余字节需要等待后续窗口。

### Windows x86 rules 分片预算收敛

- Windows x86 拆为 4 个 shard：`1/4`、`2/4`、`3/4`、`4/4`，通过 `BIFROST_E2E_RULE_SHARD_INDEX` / `BIFROST_E2E_RULE_SHARD_TOTAL` 复用分片逻辑。
- 每个 shard 在 Windows 内部仍串行；跨 shard 由 GitHub Actions 并行调度。
- `BIFROST_E2E_SUITE_TIMEOUT=1260` 秒；`BIFROST_E2E_RULE_RUNNER_TIMEOUT=1200` 秒；job `timeout-minutes` 使用 60 分钟，容纳 Windows runner 固定 setup/cleanup。
- 移除 Windows rules job 的 pnpm setup 与 Node setup，rules fixture 不构建/运行 Web UI。
- 失败 artifact 名称包含 shard index。
- Windows ARM rules 由独立 CLI build + runner E2E + desktop sidecar/check 覆盖，不作为合入门禁。

## Sync 边界

- rules E2E 不涉及 Sync；启动 Bifrost 时统一使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- Group 规则同步逻辑独立测试，rules fixture 只关心本地规则。
- CI 上 rules job 只依赖 `build-cli-windows/build-cli-macos` 产出的 CLI artifact，不依赖 desktop/frontend。

## 实现切分

### Phase 1：规则文件补齐

- 逐条比对 `COVERAGE.md` 与 fixture 目录，补齐缺失协议。
- 每新增一条 fixture，`test_rules.sh --category=<cat> --filter=<name>` 单独运行一次。

### Phase 2：协议实现补齐

- HTTPS MITM tunnel 分支复用 `apply_url_rules` / `apply_req_rules`。
- `trailers`、`reqSpeed`、`urlReplace`、`pathReplace` 等核心路径缺口按优先级修复。
- `test_rules.sh` 断言语义与协议实现同步收敛。

### Phase 3：runner 与 CI 稳定性

- 并行 runner 支持 mock outage 识别与重试。
- Windows `run_and_capture` 后台运行 + watchdog `kill_process_tree`。
- Windows x86 rules 拆 shard、收敛超时预算。

### Phase 4：文档与真实回归

- 更新 `COVERAGE.md`、`e2e-tests/rules/README.md`、`human_tests/rules-e2e-fixtures.md`。
- Windows / macOS 平台上分别执行一次完整 rules E2E，记录 wall time。

## 测试方案

### 单元测试

- `cargo test -p bifrost-proxy utils::url` 验证 URL 规则工具函数。
- `cargo test -p bifrost-core rules::parser` 验证协议解析器。

### E2E 测试（rules fixture）

- 每条 fixture 单独运行：`bash e2e-tests/test_rules.sh --category=<cat> --filter=<name>`。
- 并行全量：`bash e2e-tests/run_all_tests_parallel.sh --no-build --retry-failed-once`。
- Windows 分片：`BIFROST_E2E_RULE_SHARD_INDEX=<n> BIFROST_E2E_RULE_SHARD_TOTAL=4 bash e2e-tests/test_rules.sh --parallel`。

### E2E 测试（关键脚本）

- `bash e2e-tests/test_pattern.sh`：pattern 匹配。
- `bash e2e-tests/test_values_cli.sh` / `test_values_e2e.sh`：values 模板。
- macOS Rules CI 回归夹具（`content_inject/html.txt`、`content_inject/js.txt`、`content_inject/css.txt`、`control/enable_disable.txt`、`advanced/content_type.txt`、`request_modify/url_params.txt`、`advanced/speed.txt`、`template/values.txt`、`combination/multi_rules.txt`）逐条执行。
- 静态检查：`bash -n e2e-tests/run_all_tests_parallel.sh`、`bash -n scripts/run_all_e2e.sh`、`bash -n e2e-tests/test_rules.sh`。

### CI 结构回归

- 静态解析 `.github/workflows/ci.yml`：`e2e-windows-rules` 有 4 个 x86_64 shard、每 shard total 为 4、`timeout-minutes` 为 60、`BIFROST_E2E_SUITE_TIMEOUT=1260`、`BIFROST_E2E_RULE_RUNNER_TIMEOUT=1200`；`e2e-windows-rules.needs == ["build-cli-windows"]`；`build-cli-windows` 上传 CLI artifact；`build-desktop-windows` 下载但不阻塞 rules。

### 真实场景测试 human_tests

- 更新 `human_tests/rules-e2e-fixtures.md`：
  - TC-RUC-01：legacy `__MOCK_*_PORT__` 兼容与 replay 夹具通过。
  - TC-RUC-02：Windows 共享 mock outage 判定 + outage 预算 900 秒。
  - TC-RUC-03：Windows `run_and_capture` watchdog `kill_process_tree`。
  - TC-RUC-04：Windows x86 rules 4 shard、60 分钟 job envelope。
  - TC-RUC-05：Windows ARM rules 收敛（CLI build / runner / sidecar）。
  - TC-RUC-06：macOS Rules E2E 语义回归（HTTPS MITM tunnel + `urlParams`/`urlReplace`/`reqType` 等）。
- 所有命令使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- rules E2E 需优先于 `rust-project-validate` 执行。
- 收尾阶段：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`
  - `rust-project-validate`

本机 no-local-coverage 约定下不运行 `make coverage`；交付时说明豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 COVERAGE.md 是否与 fixture 目录一致，新增 fixture 是否分类正确。
- 复核 HTTPS MITM tunnel 是否复用 `apply_url_rules` / `apply_req_rules`。
- 复核并行 runner 的 mock outage 识别逻辑与 shard 参数。
- 复测：至少运行 macOS Rules CI 回归夹具 + 一次并行全量。

### 第 2 轮

- 复核第 1 轮修复的 diff。
- 复核 CI YAML shard 数、超时预算、依赖 job。
- 复核 Windows `run_and_capture` 与 `kill_process_tree` 的顺序。
- 复测：静态解析 CI YAML、`bash -n` 检查 runner 脚本、抽查一条 fixture。

## 风险与决策点

- **共享 mock server 掉线**：Windows 上采用串行 + outage 识别的组合方案；后续若单机 mock server 仍不稳定，可考虑改为每 shard 独立 mock。
- **Windows ARM 门禁**：ARM 上 rules fixture 90 秒请求超时高发，20 分钟内难以稳定完成，因此不作为合入门禁；平台可用性由 build + runner + sidecar 覆盖。
- **legacy 占位符**：短期通过 `preprocess_rules_file` 兼容；长期建议逐步迁移 fixture 到 `__ECHO_*_PORT__`，preprocess 层保持兼容窗口。
- **speed 断言波动**：`ThrottledBoxBody` 首窗口一次性发送首个 chunk，断言最小耗时需明确扣除首窗口容量；不能沿用 fixed rate 计算方式。
- **CI runner 变更**：GitHub Actions Windows runner 版本或 cache post 步骤变慢时，60 分钟 job envelope 仍可能超时；届时优先提高外层 envelope 或再拆 shard，而不是压 suite timeout。

## 历史事件回顾

### 2026-04-24：并行规则夹具端口占位符兼容

- 现象：`replay/forward_localhost_api.txt` 走并行 runner 后返回 HTTP 502，规则里保留原始 `__MOCK_HTTP_PORT__`。
- 根因：预处理只替换 `__ECHO_*_PORT__`，legacy 占位符未替换。
- 修复：`test_rules.sh::preprocess_rules_file` 把 legacy `__MOCK_*_PORT__` 映射到当前 runner 分配的 `ECHO_*_PORT`；保留 `__ECHO_*_PORT__` 作为推荐新命名。
- 校验：sed 替换链路语法检查 + 真实运行 replay 夹具。

### 2026-05-01：Windows 并行 rules 共享 mock 掉线恢复

- 现象：Windows rules CI 59 个 suite 同时报 mock outage，仅前 6 个被重试；后续 CI 上有 job 长时间无结果被杀。
- 根因：失败统计读的是 `test_<idx>.log`，实际写入 `log_<idx>.txt`；Windows 分支通过管道后台 `command | tee | sed`，`command_pid` 指向管道尾部而非真实 runner。
- 修复：Windows rules 强制串行；识别 mock outage 后跳过重试上限并串行重试全部失败；`run_and_capture` 后台运行真实命令并单独包裹日志流；预算收敛 `BIFROST_E2E_SUITE_TIMEOUT=1260` 秒、`BIFROST_E2E_RULE_RUNNER_TIMEOUT=1200` 秒、job 60 分钟；rules job 只依赖 `build-cli-windows`。
- 校验：脚本语法 + CI YAML 静态解析 + human_tests。

### 2026-05-04：macOS Rules E2E 语义回归收敛

- 现象：macOS aarch64 rules 失败集中在内容注入、URL 修改、Content-Type、`lineProps://disabled`、speed 限速。
- 根因：HTTPS MITM tunnel 分支没有复用普通 HTTP handler 的 URL 规则与请求头规则变换；`test_rules.sh` 对注入断言仍用 `{value}` 占位符；`lineProps://disabled` 走"验证未实现"分支；speed 断言未考虑首窗口。
- 修复：见"技术细节 → macOS rules 语义回归"。
- 校验：逐条运行 CI 失败 fixture + `cargo test -p bifrost-proxy utils::url`。

### 2026-05-08：Windows x86 Rules E2E 分片预算收敛

- 现象：Windows x86 rules 单 job wall time 贴近 20 分钟；CI 有 shard 在 fixture 通过后卡在 post cleanup 阶段超时。
- 根因：单 job 串行 wall time 过长；20 分钟 job envelope 不覆盖 Windows runner 固定 setup、artifact 下载、cache post cleanup、orphan process cleanup。
- 修复：Windows x86 拆 4 shard；job envelope 60 分钟；suite timeout 1260 秒；移除 pnpm/Node setup；artifact 命名带 shard index。
- 校验：CI YAML 静态解析 + 语法检查 + 分片索引 `1/2/3/4` 覆盖检查。
