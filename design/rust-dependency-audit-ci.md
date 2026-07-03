# Rust Dependency Audit CI

## 背景

Rust workspace 里 crate 数量长期上涨，很容易出现两类隐蔽膨胀：

- **未使用的直接依赖**：`[dependencies]`/`[dev-dependencies]` 声明了但源码里没人真正 `use`，`cargo check` 不会报警，长年不清理会拖慢编译、放大 Dependabot/`cargo audit` 攻击面。
- **重复版本**：多个上游 crate 各自锁在不同 minor version 上（`rand 0.8` / `0.9` / `0.10`、`glib 0.18/0.20`），锁文件里同时存在多份，编译时间和二进制体积一起放大。

同时，GitHub Security 面板会持续下发 Dependabot Rust `security` 与 `quality` 告警。历史修复通常靠一次性手工升级，缺少每 PR 都跑一次的 CI gate；一旦上游放出新的漏洞或 unmaintained 通告，只能等下一个人手动巡检。

本方案把三件事统一到一条 CI job：
1. `cargo-udeps` 基于真实编译图检查未使用直接依赖。
2. `cargo-deny bans` 持续把重复版本作为 warning 曝光（第一版不强制 fail，避免大批传递依赖被强行拉齐引发运行时行为变化）。
3. 通过 `human_tests/rust-dependency-audit-ci.md` 记录 Dependabot 修复的历次审计，把“暂时无法安全关闭”的项也留痕。

## 用户目标验证清单

### 必须实现

- 提供统一入口脚本 `scripts/ci/rust-dependency-audit.sh`，本地和 CI 用同一份逻辑。
- 脚本必须显式检查 `cargo`、`cargo-deny`、`cargo-udeps`、`rustup` 与 nightly toolchain；缺任何一个立刻报错并给 hint。
- `cargo-deny bans` 使用 `deny.toml` 的 `all-features = true` graph；`multiple-versions = "warn"` 保持可见但不 fail。
- `cargo-udeps` 使用 nightly toolchain + `--workspace --all-targets --all-features --locked`，并设置 `SKIP_FRONTEND_BUILD=1` 跳过前端构建。
- GitHub Actions `.github/workflows/ci.yml` 新增独立 `dependency-audit` job，装固定版本的 `cargo-deny 0.19.6` 与 `cargo-udeps 0.1.61` 后调用脚本。
- `scripts/ci/local-ci.sh` 复用同一脚本，暴露 `--skip-deps-audit` 供本地快速通道。
- 每一轮 Dependabot 修复的成功/未能安全关闭项都要在设计文档和 `human_tests/rust-dependency-audit-ci.md` 中留痕。

### 必须不破坏

- workspace `cargo test --workspace --all-features`、`cargo clippy` 现有语义不变。
- CI 其他 job（fmt、clippy、test、e2e、coverage）继续独立运行；`dependency-audit` 失败不阻塞其他 job 的可见度。
- Tauri 桌面锁文件 (`desktop/src-tauri/Cargo.lock`) 与根锁文件分别审计，不合并。

### 必须真实验证

- 本地 `bash scripts/ci/rust-dependency-audit.sh` 全流程通过。
- 裁剪 `PATH` 后重跑，工具缺失路径必须报出显式错误。
- `bash scripts/ci/local-ci.sh --skip-e2e --skip-deps-audit` 能真正跳过审计并 `SKIP` 打点。
- 根与 desktop 双 lock 的 `cargo audit --no-fetch` 输出 `vulnerabilities=0`（在有 advisory DB 缓存的机器上）。

## 产品语义

### 三种审计的语义边界

| 工具 | 意图 | 失败策略 |
|---|---|---|
| `cargo-udeps` | 未使用直接依赖 | fail |
| `cargo-deny bans` | 重复版本 / 通配符版本 | warn |
| `cargo audit`（人工/Dependabot） | RustSec advisories | 人工每期收敛，记录在 human_tests |

### 脚本失败即 CI 失败

`rust-dependency-audit.sh` 使用 `set -euo pipefail`：任何依赖工具缺失、`cargo deny check bans` non-zero、`cargo udeps` 检出未使用直接依赖，都会退出非零。CI 侧 job 直接 fail。

### 本地开发豁免

工具链不齐（例如没装 nightly、没装 `cargo-udeps`）的开发机可以显式 `--skip-deps-audit`，仍能跑其他 static/e2e 步骤。这条豁免只对本地生效，CI 里无对应开关。

## 技术细节

### `scripts/ci/rust-dependency-audit.sh`

关键结构（`crates/../scripts/ci/rust-dependency-audit.sh:1`）：

```bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
require_command cargo
require_command cargo-deny
require_command cargo-udeps
require_command rustup
rustup toolchain list | grep -q '^nightly' || {
  echo "error: rust nightly toolchain is required for cargo-udeps" >&2
  echo "hint: rustup toolchain install nightly --profile minimal" >&2
  exit 1
}
cargo deny check bans --hide-inclusion-graph
nightly_rustc="$(rustup which --toolchain nightly rustc)"
RUSTUP_TOOLCHAIN=nightly \
  RUSTC="$nightly_rustc" \
  SKIP_FRONTEND_BUILD=1 \
  cargo udeps --workspace --all-targets --all-features --locked
```

要点：
- 显式解析 `rustup which --toolchain nightly rustc`，避免 `rustup` shim 在 CI 上因缓存 miss 触发额外网络下载。
- `SKIP_FRONTEND_BUILD=1` 是 workspace build.rs 已经识别的开关，跳过 web 前端构建以缩短 udeps 时间。

### `deny.toml`

```toml
[graph]
all-features = true

[bans]
multiple-versions = "warn"
wildcards = "allow"
highlight = "all"
```

- `all-features=true` 让 bans 检查覆盖真实特性组合（默认只算 default features 会漏 admin/desktop 场景）。
- `multiple-versions="warn"` 保留信号但不强制统一。真的要收敛某一路（例如 `rand 0.9`）时，可以在具体 crate 侧独立升级并让 warning 消失。

### GitHub Actions job

`.github/workflows/ci.yml:81` 的 `dependency-audit`：

```yaml
dependency-audit:
  name: Rust Dependency Audit
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: dtolnay/rust-toolchain@nightly
    - uses: Swatinem/rust-cache@v2
    - name: Install dependency audit tools
      run: |
        cargo install --locked cargo-deny --version 0.19.6
        cargo install --locked cargo-udeps --version 0.1.61
    - name: Rust dependency audit
      shell: bash
      run: bash scripts/ci/rust-dependency-audit.sh
```

- 版本锁死避免上游语义静默变化。
- 与 clippy/test job 并行；rust-cache 复用 stable + nightly 的 `~/.cargo` / target 缓存。

### `scripts/ci/local-ci.sh` 集成

`local-ci.sh` 命令行接受 `--skip-deps-audit`；`run_step "Rust dependency audit" bash scripts/ci/rust-dependency-audit.sh` 与 fmt/clippy/test 相同的 `register_result` 机制打点，SKIP 分支也会显式记录。

## CLI / Web / Admin API 边界

依赖审计模块不引入运行时代码，因此不新增：
- CLI 子命令
- Admin API endpoint
- Web UI 面板

它只影响 CI 报告与本地 `local-ci.sh` 的输出章节。

## Sync 边界

依赖审计与 Bifrost sync 协议无关。

## Phase 拆分

- **Phase 1**：新增 `rust-dependency-audit.sh`、`deny.toml`、CI job；本地/CI 双通道跑通。
- **Phase 2**：清理已确认低风险的未使用直接依赖（`tokio-test`、`env_logger`、`is-terminal`、`async-compression`、`bifrost-core in bifrost-power`）。
- **Phase 3**：整期性 Dependabot 修复（2026-06-17、2026-06-19 已完成）；每期在 human_tests 中新增章节。
- **Phase 4**：把无法安全关闭的项（`glib`、`rand 0.7.3`、`unic-*`、`fxhash`、`paste`、`proc-macro-error2`）作为“上游追踪”条目，等待上游迁移或单独设计专项。

## 测试方案

### 单元测试

无 Rust 运行时代码；通过 workspace 聚合命令覆盖：

- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

### 脚本级验证

- `bash -n scripts/ci/rust-dependency-audit.sh`
- `bash -n scripts/ci/local-ci.sh`
- `bash scripts/ci/rust-dependency-audit.sh`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- `bash scripts/ci/local-ci.sh --skip-e2e --skip-deps-audit`

### 真实场景测试 human_tests

`human_tests/rust-dependency-audit-ci.md` 覆盖：

- TC-RDA-01：无参直接执行脚本，观察 `cargo-deny` warn 与 `cargo-udeps` OK。
- TC-RDA-02：`local-ci.sh --skip-e2e` 全流程通过并含 audit 步骤。
- TC-RDA-03：`local-ci.sh --skip-e2e --skip-deps-audit` 打点 SKIP。
- TC-RDA-04：`PATH=` 裁剪掉 `cargo-udeps` 后重跑，脚本立即失败且提示明确。
- TC-RDA-05：Dependabot 复核章节记录每期 open alerts 的处置结果与剩余上游路径。

### CI 侧真实执行

- `.github/workflows/ci.yml` PR 触发即执行 `dependency-audit` job。
- 失败输出必须包含 `cargo deny` 的重复版本表和 `cargo udeps` 的未使用依赖行。

## Dependabot 修复留痕

历史修复在 human_tests 里以日期分节维护，本设计文档保留“方法与门槛”，具体 CVE/RUSTSEC ID 走 human_tests：

- 2026-06-17：`hickory-proto` DoS、`rustls-webpki` CRL/URI/wildcard、`rand` unsound、`lru` unsound、`serial` unmaintained、`rustls-pemfile` unmaintained、`bincode` unmaintained（Traffic DB detail blob 改 postcard）。
- 2026-06-19：`jsonwebtoken` 升级到 10.4 + `aws_lc_rs` provider、`tar 0.4.46`、`openssl 0.10.80`、`tauri 2.11.1+`、desktop `rand 0.7.3` 通过 Tauri 升级消除。
- 未能安全关闭：`glib 0.18.5`（GTK/Tauri Linux stack）、`fxhash 0.2.1`（`bm25`）、`paste 1.0.15`（ASR/tokenizers/netstat2）、`proc-macro-error2 2.0.1`（`local-ip-address`）、桌面 `rand 0.7.3`（`tauri-utils/phf_codegen 0.8`）、桌面 `unic-*`（Tauri Linux stack）。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核 diff：`scripts/ci/rust-dependency-audit.sh`、`scripts/ci/local-ci.sh`、`.github/workflows/ci.yml`、`deny.toml`、`Cargo.toml` / `Cargo.lock`、`human_tests/rust-dependency-audit-ci.md`。
- 执行 `bash scripts/ci/rust-dependency-audit.sh`、`bash scripts/ci/local-ci.sh --skip-e2e`。
- 检查 GitHub Actions log 中 `dependency-audit` 是否输出 bans warning 和 udeps clean。

### 第 2 轮

- 复核 target-specific dependency 修改（`netstat2` 非 macOS、`sha1` Windows）没有影响其他平台。
- 复跑 `cargo audit` 根与 desktop lockfile：均需 `vulnerabilities=0`。
- 确认 CI 上 `dependency-audit` 缓存命中率，如果每次都重装工具则调整 `rust-cache` key。

## 风险与决策

- **不强制统一重复版本**：短期收益低、回归风险高；用 warn 维持可见度，等上游主要 crate 升级窗口再单独处置。
- **不加 `cargo audit` 到 CI job**：advisory DB 需要联网、下游误报概率高，改由 Dependabot Security Alerts + 定期 human-review 覆盖。
- **`cargo-udeps` 需要 nightly**：nightly 频繁变化，选择安装固定 `cargo-udeps 0.1.61` 而不是 `cargo install cargo-udeps --git`；上游发新版本时再统一升级。
- **Tauri Linux stack 无法快速去 `glib 0.18`**：跨 gtk-rs major，短期只能等待上游；文档中显式列出并每期 Dependabot 复核时回顾。
- **本地不跑 `make coverage`**：遵循 workspace no-local-coverage 规则，coverage 门禁交给远端 CI。
