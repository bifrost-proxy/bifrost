# CI Windows E2E Runner

## 功能模块说明

验证 GitHub Actions Windows `E2E Runner` job 在运行 `cargo run -p bifrost-e2e` 前已经预安装 `rust-src`，且 E2E 入口显式绑定当前工具链的 `rustc`，避免并行编译早期触发 rustup component 安装竞争，或 Cargo 1.95 混用 Rustc 1.65 导致 `--check-cfg` 失败。

## 前置条件

- 工作目录：项目根目录 `<REPO_ROOT>`。
- 本用例只做 workflow 静态验证和远端 CI 观察，不启动本地 Bifrost，不修改系统代理。
- 远端 CI 观察需要 `GITHUB_TOKEN` 环境变量可用，并将 `GH_REPO` 设为 `bifrost-proxy/bifrost`。

## 测试用例

### TC-CWER-01: Windows runner toolchain 预安装 rust-src

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 定位 `jobs.e2e-windows-runner.steps`。
3. 找到 `uses: dtolnay/rust-toolchain@stable` 的 step。
4. 检查该 step 的 `with.components`。

**预期结果**：
- `.github/workflows/ci.yml` 可被 YAML 解析器读取。
- `e2e-windows-runner` job 存在。
- Windows E2E runner 的 `dtolnay/rust-toolchain@stable` step 包含 `components: rust-src`。

### TC-CWER-02: x86_64 Windows E2E runner 不再卡在 rust-src conflict

**操作步骤**：
1. 推送当前分支。
2. 查询当前分支最新 `CI` workflow run。
3. 使用 fail-fast watcher 观察该 run。
4. 若 `E2E Runner (x86_64-pc-windows-msvc)` 失败，拉取 job log 并检查是否仍出现 `failed to install component: 'rust-src', detected conflict`。

**预期结果**：
- 最新 `CI` run 不再因 `rust-src` component conflict 失败。
- 如果 CI 出现其他失败，失败日志应指向新的独立根因，而不是本用例覆盖的 rustup component conflict。

### TC-CWER-03: E2E 入口绑定当前 rustc

**操作步骤**：
1. 检查 `scripts/run_all_e2e.sh`。
2. 确认脚本在 `RUSTC` 未显式设置时调用 `rustup which rustc`。
3. 确认 Runtime Context 输出 `Rustc bin`。
4. 运行无 suite 模式：
   ```bash
   bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build
   ```

**预期结果**：
- `scripts/run_all_e2e.sh` 默认通过 `rustup which rustc` 设置 `RUSTC`。
- Runtime Context 包含 `Rustc bin`，便于 CI 日志诊断 Cargo/Rustc 是否匹配。
- 命令可正常退出，不启动 Bifrost 服务、不运行 runner。

## 清理步骤

- 无本地清理需求；本测试不创建临时服务实例、不写入数据目录、不修改系统代理。

## 执行记录

- 2026-05-19：TC-CWER-01 通过 `ruby -e 'require "yaml"; ...'` 静态检查；TC-CWER-03 通过 `bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-runner.sh`、`rg -n 'rustup which rustc|Rustc bin|export RUSTC' scripts/run_all_e2e.sh` 和 `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build` 验证，Runtime Context 输出当前 Cargo/Rustc 真实路径且未启动任何 suite；TC-CWER-02 首次推送已越过 `rust-src` component conflict，但暴露 Cargo 1.95 / Rustc 1.65 混用导致的 `--check-cfg` 失败，最终结果由后续 GitHub Actions `CI` run 观察确认。
