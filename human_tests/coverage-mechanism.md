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
grep -A8 -F 'e2e-macos-shell:' .github/workflows/ci.yml | grep -F 'timeout-minutes: 75'
```

**预期**：四条命令均退出 0；新增 `test_*.sh` 自动进入 CI 或有显式跳过原因；
macOS Shell shard 的总 job 预算为 75 分钟，单用例 timeout 与断言保持不变。

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
  job log 证明期间持续有新测试结果，并非单用例静默死锁。总 job 预算调整为 75 分钟，
  单用例 `BIFROST_E2E_SHELL_TEST_TIMEOUT=1260` 与所有断言不变；契约检查已覆盖该值。
