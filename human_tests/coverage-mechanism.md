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
| TC-COV-06 | `coverage-e2e.sh` 在禁用网络下跳过失败用例但仍产出 profraw | resilience |
| TC-COV-07 | AGENTS.md 已注明 90% 门禁规则 | doc |
| TC-COV-08 | 设计文档 `design/coverage-90.md` 列出 mechanism + 不适用清单 | doc |

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

### TC-COV-06 E2E 弹性

**步骤（仅在 E2E 环境完整时执行；CI sandbox 可跳过并写明）**：

```bash
bash scripts/ci/coverage-e2e.sh --json
```

**预期**：

- 即使部分 E2E 套件失败，也能在 `target/coverage-e2e` 下产出 `.profraw`
- 输出末尾包含 `"covered"` 字段

### TC-COV-07 AGENTS.md 强制规则

**步骤**：

```bash
grep -n "coverage" AGENTS.md
grep -n "90%" AGENTS.md
```

**预期**：能命中 90% 门禁段落

### TC-COV-08 设计文档对齐

**步骤**：

```bash
test -f design/coverage-90.md
grep -n "不适用清单" design/coverage-90.md
```

**预期**：文档存在且包含「不适用清单」章节

## 执行记录

| 用例 ID | 执行人 | 结果 | 备注 |
| ------ | ----- | ---- | ---- |
| TC-COV-01 | Codex | ⚠️ | 本机先因 Homebrew Rust 与 rustup llvm-tools 不一致失败；改用 `PATH="$HOME/.cargo/bin:$PATH"` 后进入真实编译，但 workspace coverage 在 `target/llvm-cov-target` 写入阶段因磁盘仅剩约 11GiB 而报 `No space left on device` |
| TC-COV-02 | Codex | ✅ | `make coverage-crate CRATE=bifrost-command` 通过，bifrost-command 行覆盖率 98.28% |
| TC-COV-03 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --json --fail-under 0 --output-dir target/coverage-command-all`；`coverage.json` 已生成且可被 `python3 -m json.tool` 解析 |
| TC-COV-04 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --text --fail-under 9999`；返回 rc=1，证明门禁失败路径生效 |
| TC-COV-05 | Codex | ✅ | 受磁盘限制改跑 `coverage-all.sh -p bifrost-command --html --fail-under 0 --output-dir target/coverage-command-html`；`target/coverage-command-html/html/index.html` 已生成 |
| TC-COV-06 | Codex | ⚠️ | 未跑全量 E2E coverage；本机磁盘不足已阻塞 workspace coverage，继续跑 E2E coverage 风险同类失败 |
| TC-COV-07 | Codex | ✅ | AGENTS.md 已加入 90% 门禁段落 |
| TC-COV-08 | Codex | ✅ | design/coverage-90.md 已落地 |
