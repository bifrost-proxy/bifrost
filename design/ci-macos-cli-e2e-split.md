# CI macOS CLI/E2E 构建拆分

## 功能模块描述

macOS CI 的 rules/shell E2E 只依赖 `bifrost` CLI release binary，不依赖 Tauri desktop bundle。原 workflow 中 macOS E2E 依赖 `build-desktop-macos` matrix job，会等待 x86_64 与 aarch64 两个 macOS workspace 构建全部完成后才开始，拉长反馈时间。

## 实现逻辑

- 拆出 `build-cli-macos-aarch64`，只在 `macos-15` 上执行 `cargo build -p bifrost-cli --release --target aarch64-apple-darwin`，上传 `bifrost-release-aarch64-apple-darwin`。
- 拆出 `build-cli-macos-x86_64`，只在 `macos-15-intel` 上执行 `cargo build -p bifrost-cli --release --target x86_64-apple-darwin`，上传 `bifrost-release-x86_64-apple-darwin`。
- `e2e-macos-rules` 与 `e2e-macos-shell` 只依赖 `build-cli-macos-aarch64`，避免等待 x86_64 或 desktop bundle。
- `bundle-desktop-macos` 继续依赖两个 CLI artifact，用作 Tauri sidecar，再执行 frontend build 与 desktop bundle 验证。
- 避免让 E2E 依赖 matrix job，因为 GitHub Actions 的 `needs` 会等待 matrix job 的全部 child 完成。
- `bundle-desktop-macos` 的 Rust toolchain 安装保留 rustup 缺失时的 bootstrap，并改为显式 `rustup toolchain install stable --target <target>`，最多重试 3 次，避免 `static.rust-lang.org` DNS 或短暂网络抖动让 macOS desktop bundle 在进入构建前失败。
- release workflow 的 macOS desktop bundle 路径同步使用同一重试策略；非 macOS release bundle 继续使用 `dtolnay/rust-toolchain@stable`。

## 依赖项

- GitHub Actions artifact:
  - `bifrost-release-aarch64-apple-darwin`
  - `bifrost-release-x86_64-apple-darwin`
- Rust toolchain target:
  - `aarch64-apple-darwin`
  - `x86_64-apple-darwin`
- 外部下载依赖：
  - `https://static.rust-lang.org/dist/channel-rust-stable.toml.sha256`

## 测试方案

### 单元测试

- 本次修改为 CI workflow 编排变更，不新增 Rust 函数；通过 workflow YAML 解析与 dependency 断言验证。

### E2E 测试

- 不新增产品 E2E 脚本；macOS CI 自身会在 GitHub Actions 中执行：
  - `e2e-macos-rules`
  - `e2e-macos-shell`
- 本地验证重点是确认这两个 job 的 `needs` 仅指向 aarch64 CLI 构建，并且下载 artifact 名称仍匹配。
- 静态验证 `bundle-desktop-macos` 和 release macOS desktop bundle 的 toolchain 安装步骤包含 rustup bootstrap、3 次重试、`--profile minimal`、`--no-self-update` 和递增等待。
- 推送后通过 GitHub Actions `CI` workflow 验证 `Bundle macOS (x86_64-apple-darwin)` 不再因单次 rustup DNS 抖动直接失败。

### 真实场景测试

- 新增 `human_tests/ci-macos-cli-e2e-split.md`。
- 覆盖 macOS E2E 依赖、artifact 名称、desktop bundle 依赖与 workflow YAML 可解析性。
- 新增 macOS desktop bundle toolchain retry 回归用例，覆盖 PR CI 和 release workflow 的配置一致性。

## 校验要求

- 执行 workflow YAML 静态解析。
- 执行 macOS desktop bundle Rust toolchain retry 静态检查。
- 执行 `cargo fmt --all -- --check`。
- 执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 修改范围为 CI workflow 与文档，`cargo test --workspace --all-features` 如因耗时未执行，必须在结果中明确说明。

## 文档更新要求

- 更新 `human_tests/readme.md` 的 CI/DevOps 索引与总计。
