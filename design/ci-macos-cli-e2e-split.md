# CI macOS CLI/E2E 构建拆分设计方案

## 背景

macOS CI 需要同时产出两个 CLI target 与一个 desktop bundle：

- `aarch64-apple-darwin` CLI（release binary，供 macOS 平台 E2E 使用）
- `x86_64-apple-darwin` CLI（release binary，Tauri desktop sidecar 使用）
- macOS desktop bundle（`bundle-desktop-macos` matrix 覆盖 x86_64 + aarch64）

历史上 macOS E2E 三个 job（`e2e-macos-rules`、`e2e-macos-shell`、`e2e-macos-runner`）通过 `needs: [build-desktop-macos]` 依赖整个 desktop matrix，导致：

1. E2E 必须等 x86_64 与 aarch64 两次 Tauri 打包完成才启动，反馈时间被拉到 30+ 分钟。
2. desktop 阶段偶发红灯（signing / notarization / DMG 打包）会直接阻断 E2E，即便 rules/shell 只依赖 CLI binary。

同时观察到两类 macOS 稳态失败：

- `static.rust-lang.org` 偶发 DNS 抖动，让 macOS CLI / desktop bundle job 在 rustup 阶段直接失败，未进入 Cargo 编译。
- `sherpa-onnx-sys` build script 直接下载 `github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-osx-arm64-static-lib.tar.bz2`，GitHub Release asset 偶发 504，让 aarch64 CLI build 红灯。

本设计把 CLI 与 desktop 构建拆开、把 E2E 依赖只钉在 aarch64 CLI 上，并为 rustup / sherpa-onnx 两类外网抖动加上重试与本地缓存。

## 用户目标验证清单

### 必须实现

- 新增 `build-cli-macos-x86_64` job，`runs-on: macos-15`，只跑 `cargo build -p bifrost-cli --release --target x86_64-apple-darwin`，通过 `SKIP_FRONTEND_BUILD=1` 跳过 web 资产，产物 upload artifact 名 `bifrost-release-x86_64-apple-darwin`。
- 现有 `build-cli-macos-aarch64` 保持产物 `bifrost-release-aarch64-apple-darwin`。
- `e2e-macos-rules`、`e2e-macos-shell`（2-shard matrix）、`e2e-macos-runner` 的 `needs` 只包含 `build-cli-macos-aarch64`。
- `bundle-desktop-macos` 保持 `needs: [build-cli-macos-aarch64, build-cli-macos-x86_64]`，把两个 CLI artifact 用作 Tauri sidecar。
- macOS CLI 与 desktop bundle 的 Rust toolchain 安装：rustup 缺失时先 bootstrap（`curl --retry 10 ...`），随后 `rustup toolchain install stable --target <target> --profile minimal --no-self-update`，包裹在 `for attempt in 1 2 3` 循环里，失败之间 `sleep $((attempt * 20))`。
- release workflow 的 macOS desktop bundle 路径复用同一重试片段；其它平台 release bundle 继续用 `dtolnay/rust-toolchain@stable`。
- aarch64 CLI 与 `e2e-macos-runner` 接入 `actions/cache@v4` 缓存 `.ci-cache/sherpa-onnx`，key 为 `sherpa-onnx-${{ hashFiles('Cargo.lock') }}-aarch64-apple-darwin`，miss 时 restore-keys 兜底。
- 新增 `scripts/ci/prepare-sherpa-onnx-archive.sh`，从 `Cargo.lock` 解析 `sherpa-onnx-sys` 版本，用 `curl --retry --retry-all-errors` 下载官方归档，用 `tar -tjf` 验证，写入 `.ci-cache/sherpa-onnx`，并 export `SHERPA_ONNX_ARCHIVE_DIR`。

### 必须不破坏

- `bundle-desktop-macos` 继续下载两个 CLI artifact，签名 / notarization / DMG 打包链路保持。
- macOS 以外目标不接入 sherpa-onnx 缓存或 archive 脚本（Windows / Linux CI 不构建 ASR native runtime）。
- 已有 `Swatinem/rust-cache@v2` key（`cli-build-aarch64-apple-darwin` 等）保持不变。
- macOS E2E `--rules` / `--shell` / `--runner` 的运行行为保持一致；本设计只改依赖顺序，不改测试内容。
- release 非 macOS bundle（Linux/Windows）继续使用 `dtolnay/rust-toolchain@stable`，不引入 rustup bootstrap。

### 必须真实验证

- 静态检查 `.github/workflows/ci.yml`：`e2e-macos-rules` / `e2e-macos-shell` / `e2e-macos-runner` 的 `needs` 仅指向 `build-cli-macos-aarch64`。
- 静态检查 artifact 名字：`bifrost-release-aarch64-apple-darwin` / `bifrost-release-x86_64-apple-darwin` 两个 upload/download 名匹配。
- 静态检查 rustup retry：`Install Rust toolchain with retry` step 在 `build-cli-macos-aarch64`、`build-cli-macos-x86_64`、`bundle-desktop-macos` 及 release macOS bundle 均出现。
- 静态检查 sherpa 缓存：`Cache sherpa-onnx archive` 与 `Prepare sherpa-onnx archive` 步骤同时出现在 `build-cli-macos-aarch64` 与 `e2e-macos-runner`。
- 推送后 GitHub Actions `CI` 中 `Bundle macOS (x86_64-apple-darwin)` 不再因单次 rustup DNS 抖动直接失败；`Build macOS CLI (aarch64-apple-darwin)` 不再因 sherpa 504 失败。

## 产品语义

- 本设计只影响 CI 编排与构建可用性；不改变用户下载的 CLI / desktop bundle 内容。
- 对开发者，PR 反馈时间缩短：rules/shell 在 aarch64 CLI 构建完（约 15min）后立即启动，不再等 x86_64 CLI（~30min）与 desktop bundle（~20min）。

## 技术细节

### macOS CLI jobs

`build-cli-macos-aarch64`（`ci.yml` L529–594）：

- `runs-on: macos-15`，`timeout-minutes: 60`。
- steps 顺序：checkout → pnpm → Node 22 → `pnpm install --frozen-lockfile`（web/）→ `Install Rust toolchain with retry` → `Swatinem/rust-cache@v2 (key: cli-build-aarch64-apple-darwin, save-if: always())` → `Cache sherpa-onnx archive` → `Prepare sherpa-onnx archive` → `cargo build -p bifrost-cli --release --target aarch64-apple-darwin` → upload `bifrost-release-aarch64-apple-darwin`（retention-days: 1）。

`build-cli-macos-x86_64`（`ci.yml` L596–641）：

- `runs-on: macos-15`，`timeout-minutes: 90`（Rosetta / cross-compile 更慢）。
- steps：checkout → `Install Rust toolchain with retry` → `Swatinem/rust-cache@v2 (key: cli-build-x86_64-apple-darwin, save-if: always())` → `cargo build -p bifrost-cli --release --target x86_64-apple-darwin`（env `SKIP_FRONTEND_BUILD: "1"`）→ upload `bifrost-release-x86_64-apple-darwin`。
- 不接入 sherpa 缓存（`bifrost-asr/full-local-asr` 仅 aarch64 启用）。

### macOS E2E jobs

- `e2e-macos-rules`：`needs: [build-cli-macos-aarch64]`，`runs-on: macos-15`，`timeout-minutes: 60`；下载 `bifrost-release-aarch64-apple-darwin`，跑 `scripts/ci/run-e2e-rules.sh`。
- `e2e-macos-shell`：`needs: [build-cli-macos-aarch64]`，matrix `shard: [1, 2]`，`runs-on: macos-15`，`timeout-minutes: 60`，env `BIFROST_E2E_SHELL_JOBS: "2"`；下载 aarch64 CLI，跑 `scripts/ci/run-e2e-shell.sh`。
- `e2e-macos-runner`：`needs: [build-cli-macos-aarch64]`，`runs-on: macos-15`，`timeout-minutes: 60`；同时接入 sherpa-onnx 缓存 + prepare 脚本（因 runner E2E 会启动完整 tray + ASR runtime smoke）。

### Rust toolchain retry snippet

```bash
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 --retry 10 --retry-connrefused \
       --location --silent --show-error --fail \
       https://sh.rustup.rs | sh -s -- --default-toolchain none -y
  echo "${cargo_home}/bin" >> "${GITHUB_PATH}"
  export PATH="${cargo_home}/bin:${PATH}"
fi

target="aarch64-apple-darwin"  # 或 x86_64-apple-darwin
for attempt in 1 2 3; do
  echo "Installing stable Rust toolchain for ${target} (attempt ${attempt}/3)"
  if rustup toolchain install stable --target "${target}" \
       --profile minimal --no-self-update; then
    rustup default stable
    exit 0
  fi
  if [[ "${attempt}" == "3" ]]; then
    echo "Failed to install stable Rust toolchain for ${target} after 3 attempts"
    exit 1
  fi
  sleep "$((attempt * 20))"
done
```

### sherpa-onnx 预下载

`scripts/ci/prepare-sherpa-onnx-archive.sh`：

- 从 `Cargo.lock` 解析 `sherpa-onnx-sys` 版本。
- 按传入 target 拼出官方归档名（例如 `sherpa-onnx-v1.13.2-osx-arm64-static-lib.tar.bz2`）。
- 用 `curl --retry <n> --retry-all-errors -L -o <path>` 下载到 `.ci-cache/sherpa-onnx/`，成功后 `tar -tjf <path>` 验证归档可读。
- 在 GitHub Actions 环境下 `echo "SHERPA_ONNX_ARCHIVE_DIR=$(pwd)/.ci-cache/sherpa-onnx" >> "$GITHUB_ENV"`。

`Cache sherpa-onnx archive` step:

```yaml
- uses: actions/cache@v4
  with:
    path: .ci-cache/sherpa-onnx
    key: sherpa-onnx-${{ hashFiles('Cargo.lock') }}-aarch64-apple-darwin
    restore-keys: |
      sherpa-onnx-${{ hashFiles('Cargo.lock') }}-
      sherpa-onnx-
```

### release workflow

- macOS desktop bundle job 的 `Install Rust toolchain with retry` 复用同一片段。
- `aarch64-apple-darwin` CLI build step 同步接入 `Cache sherpa-onnx archive` + `Prepare sherpa-onnx archive`，key 一致。

## CLI / Web / Admin API / Sync 边界

- 无 CLI 命令、Web UI、Admin API、Sync payload 变化。
- `scripts/ci/prepare-sherpa-onnx-archive.sh` 属于 CI 专用脚本，不进入 release 的 CLI 安装内容。

## 实现切分

### Phase 1 — CLI job 拆分

- 在 `ci.yml` 新增 `build-cli-macos-x86_64`，删除 `build-desktop-macos` 对 E2E 的依赖入口。
- 保留 `build-cli-macos-aarch64`。

### Phase 2 — E2E 依赖收敛

- 把 `e2e-macos-rules`、`e2e-macos-shell`（shard 1/2 与 2/2）、`e2e-macos-runner` 的 `needs` 改为 `[build-cli-macos-aarch64]`。
- 保证 download-artifact 名字仍匹配 aarch64 upload。

### Phase 3 — Rust toolchain retry

- 在两个 macOS CLI job 与 `bundle-desktop-macos` 加入 `Install Rust toolchain with retry`，替代原 `dtolnay/rust-toolchain@stable`（macOS）。
- release.yml macOS bundle 同步。

### Phase 4 — sherpa-onnx 预下载 & 缓存

- 提交 `scripts/ci/prepare-sherpa-onnx-archive.sh`。
- `build-cli-macos-aarch64`、`e2e-macos-runner`、release aarch64 CLI 接入 `actions/cache@v4` + prepare 脚本。

### Phase 5 — human_tests 与索引

- 新增 `human_tests/ci-macos-cli-e2e-split.md`。
- 新增 `human_tests/ci-sherpa-onnx-prebuilt.md`。
- 更新 `human_tests/readme.md` 的 CI / DevOps 索引行。

## 测试方案

### 单元测试

CI workflow 编排变更，不新增 Rust 单元测试。`scripts/ci/prepare-sherpa-onnx-archive.sh` 通过 `bash -n` 语法检查。

### 集成 & E2E 测试

- macOS CI 由 GitHub Actions 真实运行 `e2e-macos-rules`、`e2e-macos-shell (shard 1/2, 2/2)`、`e2e-macos-runner`。
- 本地验证：
  - `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'` 解析成功。
  - `grep -A1 '^  e2e-macos-rules:' .github/workflows/ci.yml | grep needs` 只含 `build-cli-macos-aarch64`。
  - `grep -c 'Install Rust toolchain with retry' .github/workflows/ci.yml` >= 3（两个 CLI job + bundle-desktop-macos matrix）。
  - `grep -c 'Cache sherpa-onnx archive' .github/workflows/ci.yml` == 2。
- sherpa 脚本本地验证：`bash -n scripts/ci/prepare-sherpa-onnx-archive.sh`；用 `file://` mock release 目录执行脚本，断言 `.ci-cache/sherpa-onnx` 生成归档。

### human_tests

- `human_tests/ci-macos-cli-e2e-split.md`：覆盖 CLI 拆分、E2E 依赖收敛、artifact 名匹配、rustup retry 一致性。
- `human_tests/ci-sherpa-onnx-prebuilt.md`：覆盖脚本本地归档准备与 workflow 接线。
- `human_tests/readme.md`：只更新对应索引行。

## Review / Fix / Test 闭环

1. 第 1 轮：静态解析 workflow YAML；本地 grep 断言 needs / retry / cache 全部命中；`cargo fmt` / `cargo clippy` 全绿。
2. 第 2 轮：PR 推送后观察 macOS 三段 E2E 与两段 CLI 构建；如果 x86_64 依然偶发失败，追加 sleep 或 mirror。
3. 第 3 轮：release tag 触发一次 dry-run，确认 macOS aarch64 CLI 走本地 sherpa 归档。

## 风险与决策

- 决策：只对 aarch64 CLI 接入 sherpa 缓存——x86_64 CLI 不启用 `full-local-asr`，无必要引入下载依赖。
- 决策：`e2e-macos-shell` 拆 2 shard 而不是 4，因单 shard 内部 `BIFROST_E2E_SHELL_JOBS=2` 已充分利用 macos-15 runner；再拆会摊薄 checkout / node_modules 缓存。
- 风险：sherpa release asset 若整体不可用，缓存与重试都无法救；届时需要在 `Cargo.toml` 或 vendor path 里改用 mirror。
- 风险：`bundle-desktop-macos` 仍依赖两个 CLI；如果 x86_64 CLI 长时间挂掉，desktop bundle 会红，但 E2E 不再被拖累。

## 依赖项

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `scripts/ci/prepare-sherpa-onnx-archive.sh`
- `Cargo.lock`（sherpa 版本源）
- `bifrost-asr/full-local-asr` feature（aarch64 only）
- Artifact 名：`bifrost-release-aarch64-apple-darwin` / `bifrost-release-x86_64-apple-darwin`
- 外部下载：`https://static.rust-lang.org/dist/*`、`https://github.com/k2-fsa/sherpa-onnx/releases/download/*`

## 校验要求

- `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'`
- `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml")'`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `bash -n scripts/ci/prepare-sherpa-onnx-archive.sh`
- `cargo test --workspace --all-features`（如因耗时未执行必须在报告中明说）

## 文档更新要求

- 更新 `human_tests/ci-macos-cli-e2e-split.md`
- 更新 `human_tests/ci-sherpa-onnx-prebuilt.md`
- 更新 `human_tests/readme.md` 的 CI / DevOps 索引行；禁止维护全局测试用例总计
