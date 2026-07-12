# 代码覆盖率机制 真实场景测试

> 关联设计：`design/coverage-90.md`
>
> 关联脚本：
>
> - `scripts/ci/coverage.sh`（unit + integration，原有）
> - `scripts/ci/coverage-e2e.sh`（E2E instrumented，原有）
> - `scripts/ci/coverage-all.sh`（unit + integration，传 `--with-e2e` 时合并 E2E，默认 90% 门禁）
> - `scripts/ci/coverage-crate.sh`（按 crate，新增）
> - Makefile target：`coverage` / `coverage-unit` / `coverage-e2e` /
>   `coverage-html` / `coverage-json` / `coverage-crate CRATE=<name>`

## 前置条件

- Rust toolchain 已安装 `cargo`、`rustc`、`cargo-llvm-cov`、`llvm-tools-preview`
- 当前工作目录为仓库根
- 默认在临时分支（如 `codex/coverage-90`）上执行，避免污染 main

## 用例索引

| 用例 ID | 名称 | 类型 |
| ------ | ---- | ---- |
| TC-COV-01 | `make coverage-unit` 顺利产出 unit 覆盖率 | smoke |
| TC-COV-02 | `make coverage-crate CRATE=bifrost-command --fail-under 90` 通过 | regression |
| TC-COV-03 | `bash scripts/ci/coverage-all.sh --json --fail-under 0` 输出 coverage.json | smoke |
| TC-COV-04 | `bash scripts/ci/coverage-all.sh --text --fail-under 9999` 因门禁失败而退出非 0 | gate |
| TC-COV-05 | `make coverage-html` 生成 HTML 报告并可在浏览器查看 | smoke |
| TC-COV-06 | `coverage-e2e.sh` 产出 E2E JSON 且不掩盖失败 | resilience |
| TC-COV-07 | AGENTS.md 已注明 90% CI 门禁且本地 coverage 默认不跑 | doc |
| TC-COV-08 | 设计文档 `design/coverage-90.md` 列出 mechanism + 不适用清单 | doc |
| TC-COV-09 | 覆盖 E2E 与生产数据目录、HOME/XDG、9900 端口硬隔离 | safety |
| TC-COV-10 | 覆盖管线与全仓 Shell 语法契约通过 | regression |
| TC-COV-11 | PR changed-lines 95% 门禁 | gate |
| TC-COV-12 | Shell 静态质量门禁 | regression |
| TC-COV-13 | 代理核心能力矩阵 | contract |
| TC-COV-14 | E2E 机器可读结果 | observability |
| TC-COV-15 | WebSocket 核心与 Playwright 关键矩阵 | e2e |
| TC-COV-16 | 核心代理 production coverage 90% 门禁 | gate |
| TC-COV-17 | Full E2E 门禁与 TLS 动态切换确定性 | e2e |

## 用例细节

### TC-COV-01 `make coverage-unit` smoke

**步骤**：

```bash
cd <repo-root>
make coverage-unit | tee /tmp/coverage-unit.log
```

**预期**：

- 退出码 0
- 日志最后一行包含每个 crate 的覆盖率统计或 LCOV 路径
- `target/coverage/` 目录产生

### TC-COV-02 单 crate 覆盖率 + 90% 门禁

**步骤**：

```bash
make coverage-crate CRATE=bifrost-command
```

**预期**：

- `bifrost-command` 行覆盖率 ≥ 90%
- 退出码 0

如果门禁不通过：

- 修复方式：在 `crates/bifrost-command/src/lib.rs::tests` 中补 case，
  覆盖 `SearchScope::default`、`SearchFilters::has_constraints`、
  `CanonicalQueryCommand::command_id` / `capability` 全部分支。

### TC-COV-03 unit + json 输出

**步骤**：

```bash
bash scripts/ci/coverage-all.sh --json --fail-under 0
```

**预期**：

- 退出码 0
- `target/coverage/coverage.json` 文件存在且为合法 JSON

### TC-COV-04 门禁失败回归

**步骤**：

```bash
bash scripts/ci/coverage-all.sh --text --fail-under 9999 || echo "FAIL_AS_EXPECTED rc=$?"
```

**预期**：

- 输出 `FAIL_AS_EXPECTED rc=<非零>`，证明 `--fail-under` 真实生效

### TC-COV-05 HTML 报告

**步骤**：

```bash
bash scripts/ci/coverage-all.sh --html --fail-under 0
ls target/coverage/html/index.html
```

**预期**：

- `target/coverage/html/index.html` 存在
- 浏览器打开后可看到每个 crate 的覆盖率热点视图

### TC-COV-06 E2E 报告与失败传播

**步骤（仅在 E2E 环境完整时执行；CI sandbox 可跳过并写明）**：

```bash
bash scripts/ci/coverage-e2e.sh --json --suite rules
```

**预期**：

- `target/coverage-e2e/coverage.json` 存在且为合法 JSON。
- rules 套件通过时命令退出 0。
- 任一 E2E 套件失败时可保留诊断覆盖报告，但命令最终退出非 0，禁止把失败掩盖成绿灯。

### TC-COV-07 AGENTS.md 覆盖率默认执行边界

**步骤**：

```bash
grep -n "coverage 90% CI 门禁" AGENTS.md
grep -n "默认情况下不要在本机运行" AGENTS.md
grep -n "coverage-all.sh --json --gate" AGENTS.md
```

**预期**：

- 能命中 90% 覆盖率门禁段落。
- AGENTS.md 明确覆盖率默认由远端 CI 门禁兜底。
- AGENTS.md 明确默认不在本机运行 `make coverage` / `coverage-all.sh --gate`，除非用户明确要求、专项排查覆盖率失败或需要提前确认高风险覆盖率缺口。

### TC-COV-08 设计文档对齐

**步骤**：

```bash
test -f design/coverage-90.md
grep -n "不适用清单" design/coverage-90.md
```

**预期**：文档存在且包含「不适用清单」章节

### TC-COV-09 生产服务与数据目录隔离

**步骤**：

```bash
bifrost status --format json
shasum -a 256 ~/.bifrost/config.toml ~/.bifrost/runtime.json \
  ~/.bifrost/admin/im_gateway_providers.json
grep -n 'prepare_isolated_e2e_environment\|BIFROST_E2E_PROTECTED_PORTS\|REFUSING:' \
  scripts/ci/coverage-all.sh scripts/ci/coverage-e2e.sh
bash e2e-tests/tests/test_coverage_pipeline_contract.sh
bifrost status --format json
shasum -a 256 ~/.bifrost/config.toml ~/.bifrost/runtime.json \
  ~/.bifrost/admin/im_gateway_providers.json
```

**预期**：

- 测试前后主服务 PID 与 9900 listener 不变。
- 三个生产配置文件哈希不变。
- 覆盖 E2E 使用 worktree `target/` 下的数据目录和 HOME/XDG，不使用 `~/.bifrost`。
- cleanup helper 将 9900 作为 protected port。

### TC-COV-10 覆盖管线与 Shell 语法契约

**步骤**：

```bash
bash scripts/ci/check-shell-syntax.sh
bash e2e-tests/tests/test_coverage_pipeline_contract.sh
bash scripts/ci/check-e2e-shell-ci-coverage.sh
BIFROST_E2E_CAPABILITY_SHARDS=1 BIFROST_E2E_SHELL_JOBS=2 \
  bash scripts/run_all_e2e.sh \
  --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
  --shard 1/3 --check-shell-shard-balance
```

**预期**：四条命令均退出 0；新增 `test_*.sh` 自动进入 CI 或有显式跳过原因；
macOS Shell 按 `proxy-core`、`remote`、`agent-extensions` 三个能力 job 拆分；167 个
测试无重复、无遗漏，按历史耗时估算的最大 job 偏差不超过平均值 15%。

### TC-COV-11 PR changed-lines 95% 门禁

**步骤**：

```bash
python3 -m unittest discover -s scripts/ci/tests -p 'test_*.py' -v
grep -n 'changed_lines_min = 95.0' scripts/ci/coverage-thresholds.toml
grep -n 'coverage-diff.py' .github/workflows/ci.yml
```

**预期**：工具单测全部通过；PR coverage job 在 LCOV 生成后执行 changed-lines 95%
门禁；`#[cfg(test)] mod tests` 中的测试代码不计入生产增量覆盖率。

### TC-COV-12 Shell 静态质量门禁

**步骤**：

```bash
PATH="$(go env GOBIN):$PATH" BIFROST_REQUIRE_SHELL_TOOLS=1 \
  bash scripts/ci/check-shell-quality.sh origin/main
```

**预期**：全仓 Shell `bash -n` 与 ShellCheck error gate 通过；变更的
`scripts/ci/*.sh` 通过 shfmt；缺少检查工具时 CI 必须失败而不是静默跳过。

### TC-COV-13 代理核心能力矩阵

**步骤**：

```bash
python3 scripts/ci/check-e2e-capabilities.py
```

**预期**：P0 能力均具备 unit/integration/E2E、Linux/macOS/Windows、失败模式和真实
证据文件；重复 ID、缺失证据或缺少维度时校验失败。

### TC-COV-14 E2E 机器可读结果

**步骤**：

```bash
rm -rf .e2e-reports/summary-human
BIFROST_E2E_REPORT_DIR="$PWD/.e2e-reports/summary-human" \
  bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner \
  --skip-ui --skip-build
python3 -m json.tool .e2e-reports/summary-human/summary.json
```

**预期**：生成 schema version 1 的 JSON，包含 metadata、counts、suites；即使 selected
suite 为 0 也生成合法空报告，真实执行时逐项记录 passed/failed/skipped 与原因。

### TC-COV-15 WebSocket 核心与 Playwright 关键矩阵

**步骤**：

```bash
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy \
  websocket::capture::tests --lib
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy \
  proxy::http::websocket::tests --lib
BIFROST_DATA_DIR="$PWD/.bifrost-ui-test-human" \
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 \
  bash scripts/ci/run-ui-critical.sh
```

**预期**：WebSocket 双向帧、mask 方向、leftover、握手 header 过滤均通过；Playwright
关键矩阵 7/7 通过，覆盖 Rules、Values、Scripts、Traffic、Breakpoint 与 Agent 新会话，
使用隔离数据目录和动态端口，不连接或停止生产 9900 服务。完整 211 条历史套件由
`.github/workflows/ui-e2e-full.yml` 每周审计并上传失败证据，PR 合入门禁不隐瞒其存量债务。

### TC-COV-16 核心代理 production coverage 90% 门禁

**步骤**：

```bash
python3 scripts/ci/coverage-production.py \
  target/coverage-current-merge/coverage-wave15.lcov \
  --crate bifrost-proxy \
  --thresholds scripts/ci/coverage-thresholds.toml \
  --json-output target/coverage-current-merge/production-human.json --top 5
grep -A8 '^\[crates.bifrost-proxy\]' scripts/ci/coverage-thresholds.toml
grep -n 'production-coverage.json\|coverage-all.sh --with-e2e' \
  .github/workflows/coverage-e2e.yml
```

**预期**：production reporter 输出 `PASS` 且覆盖率不少于 90%；阈值文件中
`bifrost-proxy min = 90.0`、`metric = "production"`；PR layered workflow 执行完整
`--with-e2e` 并上传 `production-coverage.json`。

### TC-COV-17 Full E2E 门禁与 TLS 动态切换确定性

**步骤**：

```bash
bash e2e-tests/tests/test_coverage_pipeline_contract.sh
HOME="$HOME" CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" \
  BIFROST_BIN="$PWD/target/release/bifrost" \
  target/release/bifrost-e2e --test tls_switch_intercept_to_tunnel \
  --jobs 1 --test-timeout 30
BIFROST_E2E=1 BIFROST_CHATGPT_WEB_E2E_MOCK=1 \
  target/release/bifrost-e2e \
  --test im_gateway_chatgpt_web_restores_conversation_after_service_restart \
  --jobs 1 --test-timeout 120
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy \
  intercepted_non_http_tls_connects_and_relays_through_tls_upstream \
  --all-features -- --nocapture
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-proxy \
  limited_blocking_resolution_covers_success_closed_and_join_error \
  --all-features -- --nocapture
SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
  bash e2e-tests/tests/test_group_sync_no_logstorm_e2e.sh
cd web && pnpm exec playwright test tests/ui/admin-rules-values.spec.ts \
  --grep "Values 页面完成 CRUD" --workers=1
SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
  bash e2e-tests/tests/test_rule_share_confirm_browser.sh
BIFROST_HTTP3_CARGO_TEST_TIMEOUT=600 SKIP_BUILD=true \
  BIFROST_BIN="$PWD/target/release/bifrost" \
  bash e2e-tests/tests/test_http3_e2e.sh
CI=true SKIP_CARGO_TEST=true SKIP_BUILD=true \
  BIFROST_BIN="$PWD/target/release/bifrost" \
  bash e2e-tests/tests/test_http3_e2e.sh
```

**预期**：coverage pipeline contract 全部通过；TLS ON -> OFF 后第二次请求使用新连接，
流量统计精确为 `HTTPS=1` 且 `TUNNEL>=1`；release Runner 只有在 E2E 上下文与 mock
请求双开关同时存在时走 ChatGPT Web mock，服务重启会话恢复用例 1/1 通过；
`bifrost-e2e` 显式显示 `EXEMPT`，且不进入 crate/workspace 自覆盖率百分比门禁。
非 HTTP TLS 拦截成功连接真实本地 TLS upstream，并双向转发初始载荷、客户端载荷和
upstream 响应，取消信号能够正常结束 relay。
进程解析限流测试用已取消 task 确定性地产生 `JoinError`，不依赖 panic 回溯速度或
平台调度时序。
Group Rules 真实远端变更会传播到 API 与规则文件；当 info 日志可观测时产生 runtime
reload，随后重复未变更同步前后日志计数与规则文件指纹均稳定，不形成 log storm。
Values CRUD/sort/push 用例在切换排序前移开 Refresh tooltip 的 hover，排序选择器可点击，
完整目标用例通过且不再因 tooltip 截获 pointer events 超时。
Rule Share 浏览器确认页在导航切换期间允许 `document.body` 暂时为空，CDP 轮询不会因
读取空 body 的 `innerText` 提前失败，并最终完成规则应用与目标页跳转。
HTTP/3 E2E 冷缓存预编译使用与实际测试相同的 release profile，600 秒预算内完成，
避免 debug/release 重复编译和 macOS runner 180 秒误超时。
macOS CI 显式跳过重复的 Rust integration 编译，但仍执行 HTTP/3 脚本的全部真实代理
场景；Linux Unit/Integration 与 Linux Shell 继续双重强制执行 Rust integration test。

## 执行记录

| 用例 ID | 执行人 | 结果 | 备注 |
| ------ | ----- | ---- | ---- |
| TC-COV-01 | Codex | ⚠️ | 本机先因 Homebrew Rust 与 rustup llvm-tools 不一致失败；改用 `PATH="$HOME/.cargo/bin:$PATH"` 后进入真实编译，但 workspace coverage 在 `target/llvm-cov-target` 写入阶段因磁盘仅剩约 11GiB 而报 `No space left on device` |
| TC-COV-02 | Codex | ✅ | `make coverage-crate CRATE=bifrost-command` 通过，bifrost-command 行覆盖率 98.28% |
| TC-COV-03 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --json --fail-under 0 --output-dir target/coverage-command-all`；`coverage.json` 已生成且可被 `python3 -m json.tool` 解析 |
| TC-COV-04 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --text --fail-under 9999`；返回 rc=1，证明门禁失败路径生效 |
| TC-COV-05 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --html --fail-under 0 --output-dir target/coverage-command-html`；`target/coverage-command-html/html/index.html` 已生成 |
| TC-COV-06 | Codex | ⚠️ | 未跑全量 E2E coverage；本机磁盘不足已阻塞 workspace coverage，继续跑 E2E coverage 风险同类失败 |
| TC-COV-07 | Codex | ✅ | AGENTS.md 已加入 90% CI 门禁段落，并明确本地默认不跑全量 coverage |
| TC-COV-08 | Codex | ✅ | design/coverage-90.md 已落地 |

2026-07-09 执行补充：

- TC-COV-07：PASS。执行 `grep -n "coverage 90% CI 门禁" AGENTS.md`、`grep -n "默认情况下不要在本机运行" AGENTS.md`、`grep -n "coverage-all.sh --json --gate" AGENTS.md` 均命中；执行固定字符串检索确认“收尾/提交前必须在本机跑 make coverage”这类旧强制本地 coverage 表述不再存在于 AGENTS.md / design/coverage-90.md / human_tests/coverage-mechanism.md。

2026-07-11 执行补充（本次覆盖机制专项验证）：

- TC-COV-03：PASS。执行
  `COVERAGE_JOBS=4 bash scripts/ci/coverage-all.sh --with-e2e --e2e-suite rules --json --output-dir target/coverage-layered-debug`
  退出 0；三份 JSON 均可解析。unit + integration 为
  `218743/324449 = 67.42%`，E2E-only 为 `20896/324449 = 6.44%`，联合覆盖为
  `221038/324449 = 68.13%`；联合覆盖比 unit + integration 多覆盖 2295 行，证明
  E2E profile 被真实合并，而不是仅执行测试后仍输出单测报告。
- TC-COV-06：PASS。独立执行
  `COVERAGE_JOBS=4 bash scripts/ci/coverage-e2e.sh --json --suite rules --skip-build --output-dir target/coverage-e2e-rules`
  退出 0，67/67 fixture、582/582 assertion 通过，生成合法 E2E-only JSON；另由
  `test_coverage_pipeline_contract.sh` 断言 E2E 非零退出码不会被吞掉。
- TC-COV-09：PASS。覆盖测试前后线上服务均为 PID `12141`、listener `9900`、数据目录
  `~/.bifrost`；`config.toml`、`runtime.json`、`admin/im_gateway_providers.json`
  的 SHA-256 分别始终为 `42676e4a...7235235`、`8bf03442...b5b0c5a`、
  `eca06315...3f0eb0`。测试数据实际位于 worktree 的
  `target/coverage-layered-debug/.bifrost-data`，测试 HOME/XDG 同样位于该 target 目录；
  未执行 upgrade/start/stop，未修改系统代理。
- TC-COV-10：PASS。`check-shell-syntax.sh` 检查 249 个 tracked Shell 文件，失败 0；
  `test_coverage_pipeline_contract.sh` 通过；`check-e2e-shell-ci-coverage.sh` 发现 197 个
  Shell 测试，169 个进入 CI、28 个有显式跳过原因，检查通过。
- TC-COV-10（CI 回归补测）：PASS。首轮 CI run `29148890471` 的 macOS Shell shard
  1/2 在持续完成用例约 58 分钟后，于最后串行 fixture 被 60 分钟 job 上限取消；完整
  job log 证明期间持续有新测试结果，并非单用例静默死锁。按用户要求改为三个能力
  job：`proxy-core` 84 个 / 1068s、`remote` 35 个 / 1215s、
  `agent-extensions` 50 个 / 1079s；169 个用例 0 重复、0 遗漏，最大估算偏差
  13.1%，通过 capability 模式 15% 平衡门禁。单用例 timeout 与所有断言不变。
- TC-COV-10（环境继承回归）：PASS。CI run `29151557395` 暴露 Linux Shell 外层
  `BIFROST_E2E_SHELL_JOBS=4` 会污染契约中的 macOS 平衡估算；契约现显式固定 macOS
  的 2 lanes，并在外层 `BIFROST_E2E_SHELL_JOBS=4` 环境中复跑通过。
- TC-COV-10（跨平台 Shell 断言回归）：CI run `29152804806` 暴露
  `test_upgrade_cli.sh` 在 `pipefail` 下使用 `echo | grep -q` 累计 help 检查不稳定；改为
  Bash 原生字符串匹配，四项语义不变并能报告具体缺失项。使用 worktree binary 与隔离
  `BIFROST_DATA_DIR` 真实复跑通过。
- TC-COV-10（macOS 能力组耗时优化）：CI run `29154062293` 三个能力 job 均通过，实际
  耗时分别为 `proxy-core` 21m07s、`remote` 39m17s、`agent-extensions` 12m39s。下载
  `remote` 完整 job log 后确认，`test_desktop_sidecar_launchd_env_contract.sh` 与
  `test_desktop_open_requests_contract.sh` 分别耗时 1153s、1079s，主要成本是 Shell job
  内重新编译 Tauri/Rust 图，不是远程调用本身。按用户决定将这两个本地桌面发布契约
  加入 CI 显式跳过清单；其余真实远程、同步、长期记忆用例保留。用本轮真实日志重校
  权重并调整能力归组后，167 个 CI Shell 用例分布为 `proxy-core` 79 个 / 816s、
  `remote` 37 个 / 790s、`agent-extensions` 51 个 / 874s，0 重复、0 遗漏，最大估算
  偏差 10.2%，通过 15% 门禁；CI 显式跳过项由 28 个增至 30 个。
- TC-COV-10（HTTPS fixture readiness 回归）：CI run `29156752284` 的 `proxy-core`
  暴露 `test_trustworthy_traffic_metrics.sh` 用 80 次固定轮询等待临时 RSA 证书与 HTTPS
  fixture，在 macOS 负载下误报未就绪。改为 90 秒真实墙钟 deadline；正常启动不增加
  等待，慢 runner 仍会在明确上限内失败并打印 fixture 日志。使用 worktree release
  binary、动态端口、临时数据目录并保护 9900 真实复跑该用例。
- TC-COV-10（优化后 CI 实测）：同一 run 中 `agent-extensions` 成功耗时 14m05s，
  `remote` 成功耗时 16m57s；`remote` 相比优化前 39m17s 缩短 22m20s（约 57%）。
  `proxy-core` 在 19m44s 因上述 HTTPS readiness 假失败结束，非能力分组超时。
- TC-COV-10（全绿后慢 job 审计）：CI run `29157919216` 35/35 全绿；唯一超过 30 分钟
  的 job 是 Windows x86_64 CLI build（32m12s），其中 `cargo build --release` 独占
  30m46s。日志显示 `No cache found` 且 job 结束未保存 Rust cache，Actions cache API
  也查不到对应 key。根因是 7 处 `save-if: always()` 不是布尔输入；首次修为
  `${{ always() }}` 后 run `29159544439` 在 workflow 解析期 0 job 失败，进一步确认状态
  函数不能用于 action input。最终修为 `save-if: ${{ true }}`，并由 coverage pipeline
  contract 同时阻止两种错误写法回归。

2026-07-12 执行补充（Phase 6 完备性门禁）：

- TC-COV-11：PASS。`scripts/ci/tests` 共 12 条工具单测通过，其中 changed-lines 6 条；
  阈值固定为 95%，并验证内联测试模块不会抬高生产代码增量覆盖率。
- TC-COV-12：PASS。253 个 tracked Shell 文件 `bash -n` 与 ShellCheck error gate 全部
  通过；变更的 `scripts/ci/*.sh` 通过 shfmt。门禁真实发现并修复 7 处 stdin 重定向、
  命令替换和条件表达式错误，未使用全局 disable。首次远端 CI 还发现本地检查未纳入
  尚未提交的新 Shell 文件；现已把 untracked `scripts/ci/*.sh` 一并纳入本地 shfmt 选择。
- TC-COV-13：PASS。能力矩阵校验 10 个代理核心能力，P0 的测试层、三平台、失败模式与
  证据文件完整。
- TC-COV-14：PASS。隔离运行生成 `.e2e-reports/summary-human/summary.json`，schema v1
  可解析，空选择也包含 metadata、counts 与 suites。
- TC-COV-15：PASS。WebSocket capture 2 条、upgrade 3 条专项 Rust 测试通过；Playwright
  关键能力矩阵 7/7 通过。完整历史套件首次审计在 211 条中运行至第 73 条时已暴露 11 条
  旧页面契约失败，验证了新增每周完整审计的必要性；该债务如实保留，不计为关键矩阵通过。

2026-07-12 执行补充（核心代理 90%）：

- TC-COV-16：PASS。production reporter 对合并 LCOV 统计为
  `19304/21446 = 90.01%`，90% 门禁通过；阈值文件确认 `min=90.0`、
  `metric="production"`，PR layered workflow 执行完整 `--with-e2e` 并上传
  `production-coverage.json`。
- TC-COV-17：PASS。coverage pipeline contract 检查 253 个 Shell 文件、23 个工具单测、
  10 个能力契约及 3 个能力分片均通过；真实运行 TLS ON -> OFF 用例 1/1 通过，切换后
  流量为 `HTTPS=1`、`TUNNEL=1`；release Runner 双开关 ChatGPT Web 会话恢复用例
  1/1 通过，耗时 1.11 秒；测试框架自身已按产品边界标记为 `metric="exempt"`，
  其 Rules/Shell/Runner 可执行门禁保持强制；非 HTTP TLS 拦截真实本地 TLS upstream
  双向 relay 用例 1/1 通过；进程解析限流的 success/closed/cancelled join-error 用例
  1/1 通过；Group Rules no-logstorm 真实脚本 20/20 通过，远端内容成功传播并在 15 次
  重复同步前后保持文件指纹和 reload 计数稳定；Values CRUD/sort/push Playwright 目标
  用例 1/1 通过，耗时 8.1 秒，不再被 Refresh tooltip 截获点击；Rule Share 浏览器确认
  脚本通过，规则成功应用并跳转目标页，导航瞬间空 body 不再中断 CDP 轮询；HTTP/3
  release 预编译 1 分 33 秒完成，全量脚本 34/34 通过；macOS CI 等价配置下 Rust
  integration 标记为平台重复跳过，其余真实代理场景仍为 34/34、失败 0。
