# CI Windows Unit Tests Stability

## 功能模块说明

本设计记录 Windows 本地与 GitHub Actions 单元测试稳定性修复。目标是在 Parallels Windows VM 的真实仓库 `C:\Users\eden\github\bifrost` 中完成 `cargo test --workspace --all-features`，并消除只在 Windows shell、路径、进程生命周期、TLS body fallback、文件 watcher remove 事件中暴露的失败。

## 实现逻辑

- Agent `exec_command` 测试允许 Windows 高负载下短命命令先返回 running session，再通过 `write_stdin`/poll 累积输出到最终 exit code；Windows 不稳定的交互式 TTY stdin 路径不再作为 P1 E2E 的硬性断言。
- IM Gateway external CLI 测试使用平台化 delayed command；Windows `taskkill` 在目标 PID 已消失时视为停止成功。
- HTTPS interception 的 H2 body reset fallback 到 HTTP/1.1 后，先按 Content-Length 有界读取响应体；需要跳过 body processing 时仍规范化响应头，避免客户端把 fallback body 判定为 decode error。
- Skills registry watcher 同时保存 raw root 与 canonical root。Windows remove event 中已删除路径无法 canonicalize 时，仍可从 raw root 计算 slug 并删除缓存项。
- 其他 Windows 编译或 clippy 差异通过 `cfg` 缩小到平台相关分支，不改变非 Windows 行为；平台专用测试 helper 必须随平台 cfg 一起收敛，Unix-only fixture import 不得在 Windows `--all-targets -D warnings` 下产生 unused warning。
- Windows Unit Tests job（`.github/workflows/ci.yml` 中的 `test-windows-tray`，job name `Windows Unit Tests (x86_64)`，`runs-on: windows-latest`，仅运行 `x86_64-pc-windows-msvc` target）的 `Swatinem/rust-cache` 通过 `save-if: ${{ false }}` 仅用于 restore，不在该 job 的 post-step 保存 cache，避免测试主体通过后被 Windows tar/zstd cache 打包失败或超时拖红。该 job 直接执行 `cargo test --workspace --all-features --target x86_64-pc-windows-msvc`（不带 `-j1`）；`aarch64-pc-windows-msvc` 当前仅在 `build-cli-windows` / `build-desktop-windows` 编译类 job 中覆盖，没有专门的 Windows aarch64 单元测试 job。

## 依赖项

- Windows VM 需要 Visual Studio MSVC environment、rustup stable、Git Bash、LLVM `lld-link`。
- 本地 full workspace 验证使用 `SKIP_FRONTEND_BUILD=1`，避免前端构建掩盖 Rust Windows 单测问题。
- `bifrost-tests` HTTPS fallback 用例依赖本地 mock TLS server，不访问外网，不启动正式 Bifrost 服务，不修改系统代理。

## 测试方案

- 单元测试：覆盖 `bifrost-agent` exec_command、`bifrost-admin` IM Gateway external CLI、`skills` registry watcher、`bifrost-core` launchd parser、`bifrost-cli` upgrade/main 相关 Windows 编译路径，以及 `bifrost-device` iOS cfgutil/Android CA status 平台专用测试 helper 的 Windows `--all-targets` 编译路径。
- E2E/集成测试：执行 `bifrost-agent --test p1_tools_e2e` 与 `bifrost-tests --test https_proxy_test`，验证工具链路和 HTTPS H2 body fallback 真实路径。
- 真实场景测试：更新并执行 `human_tests/ci-windows-unit-tests.md`，记录 Windows VM 中完整 `cargo test --workspace --all-features -j1` 结果。
- CI 稳定性：通过 PR run 确认 Windows Unit Tests 不再因为 `Post Run Swatinem/rust-cache@v2` 保存缓存失败而在测试主体通过后失败。
- Workspace 兜底：至少一次完整执行 `cargo test --workspace --all-features -j1`。

## Review/Fix/Test 闭环方案

1. 第 1 轮复核当前 diff、Windows 失败归因和 targeted tests；发现新失败后先补最小修复，再跑对应过滤用例。
2. 第 2 轮基于修复后 diff 复查跨平台 `cfg`、HTTP header/body 语义和 watcher path 匹配，再复跑 targeted tests 与 workspace full test。
3. 若 clippy、fmt、CI 或 Windows full test 继续失败，按失败日志追加新轮次，不削弱断言。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -j1 -- -D warnings`
- `cargo test --workspace --all-features -j1`
- 受影响 targeted tests 必须在 Windows VM 仓库内执行。

## 文档更新要求

- 更新 `human_tests/ci-windows-unit-tests.md` 和 `human_tests/readme.md`。
- 本修复不新增 CLI/API 配置项，不需要更新 README。
