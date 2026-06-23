# Release CI Resilience

## 功能模块说明

Release workflow 负责手动或 tag 触发发版，包含 CLI 多平台构建、Desktop 多平台构建、GitHub Release 创建、Homebrew tap 更新和 npm 发布。2026-06-23 的手动 Release run `28038797261` 在两个与代码无关但会中断发版的基础设施点失败：

- `Build Desktop (aarch64-pc-windows-msvc)` 在 `Upload artifact` 阶段收到 artifact blob storage `The server is busy`。
- `Build Desktop (x86_64-apple-darwin)` 在 `Build macOS desktop app bundle` 阶段由 cargo 访问 `index.crates.io` 时 DNS 解析失败。

PR CI 回归 run `28043895224` 还暴露了 macOS ARM shell E2E 的 60 分钟 job 预算不足：`E2E Shell (aarch64-apple-darwin, shard 2/2)` 上传的 73 个 shell 日志均为通过或跳过，但 job 在最后收尾阶段达到 60 分钟超时而失败。

## 实现逻辑

- 新增本地 composite action `.github/actions/upload-artifact-with-retry/action.yml`，封装 `actions/upload-artifact@v4`，最多尝试 3 次，失败后按 20 秒、40 秒退避等待。每次设置 `overwrite: true`，避免前一次失败留下同名 artifact 时阻断后续重试。
- Release workflow 的 CLI artifact 与 Desktop artifact 上传统一改用本地 retry action。
- 在 CLI build job 的 Rust cache 后、实际编译前执行 `cargo fetch --target "${{ matrix.target }}"`，最多 3 次重试。
- 在 Desktop build job 的 Rust cache 后、前端/CLI/Tauri 构建前执行 root workspace 与 `desktop/src-tauri/Cargo.toml` 的 `cargo fetch`，最多 3 次重试。这样 Tauri build 进入编译阶段前已经对 crates.io 瞬断做过显式重试，并且保持与现有 release build 未加 `--locked` 的行为一致。
- 将 PR CI 的 `E2E Shell (aarch64-apple-darwin, shard */2)` job timeout 从 60 分钟提高到 90 分钟，给 macOS ARM shell shard 的真实测试与清理阶段留出余量，避免所有用例已通过但 job 级超时导致红灯。

## 依赖项

- `.github/workflows/release.yml`
- `.github/workflows/ci.yml`
- `.github/actions/upload-artifact-with-retry/action.yml`
- `Cargo.lock`
- `desktop/src-tauri/Cargo.lock`
- GitHub hosted runner 的 bash、cargo 与 actions artifact service

## 测试方案

### 单元测试

本次修改为 GitHub Actions YAML 与 composite action 配置，不涉及 Rust 公共函数，不新增 Rust 单元测试。

### E2E 测试

本次修复不启动 Bifrost 代理服务，不涉及规则、Admin API、Web UI 或 CLI 运行时行为；本地代理 E2E 不适用。以 workflow 静态校验、human_tests 场景验证和推送后的 GitHub Actions CI run 作为发布链路回归。

### 真实场景测试

- 新增 `human_tests/release-ci-resilience.md`，覆盖 artifact 上传重试、CLI cargo fetch 重试、Desktop cargo fetch 重试，以及失败日志与修复点映射。
- 按用例逐条执行 `rg` / YAML 解析 / GitHub Actions 日志检查。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核失败日志与目标，检查 release workflow diff、本地 action 语义、YAML 解析结果和 human_tests 索引，修复发现的问题后复跑最小静态验证。
- 第 2 轮：基于最新 diff 再次检查上传重试 action 的条件表达式、cargo fetch 插入位置、文档与 human_tests 是否一致，复跑相同验证命令。

## 校验要求

- `git diff --check -- .github/workflows/ci.yml .github/workflows/release.yml .github/actions/upload-artifact-with-retry/action.yml design/release-ci-resilience.md human_tests/release-ci-resilience.md human_tests/readme.md`
- `python3 - <<'PY' ... yaml.safe_load(...) ... PY`
- `rg` 静态检查 retry action、`cargo fetch` 和 release workflow 引用。
- `rg` 静态检查 macOS ARM shell E2E timeout 为 90 分钟。
- `cargo fetch --target x86_64-apple-darwin && cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-apple-darwin`
- 推送后使用 `github-actions-pat` 查询并看护 PR CI；Release workflow 需要合入后由维护者重新触发。

## 文档更新要求

- 新增本设计文档。
- 新增 `human_tests/release-ci-resilience.md`。
- 更新 `human_tests/readme.md` 的 CI / 发布相关索引行。
