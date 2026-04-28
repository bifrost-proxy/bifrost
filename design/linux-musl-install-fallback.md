# Linux musl 安装回退

## 功能模块说明

Bifrost Linux x64 / ARM64 同时发布 glibc 目标和 musl 目标。glibc 目标由 GitHub Actions 的较新 Ubuntu runner 构建，会携带构建机 glibc 符号版本要求。Debian 10 等旧环境只有 glibc 2.28，运行 `x86_64-unknown-linux-gnu` 产物时会在动态加载阶段失败，例如缺少 `GLIBC_2.39`。

本模块的目标是在官方安装入口中优先保证兼容性：当 Linux glibc 版本低于 GNU 产物最低要求，或无法可靠识别 glibc 版本时，自动选择对应的 musl/static 产物。

## 实现逻辑

- `install-binary.sh` 保持 shell 安装入口的目标选择逻辑：Linux x86_64 / aarch64 在 glibc 版本低于 `2.39` 时选择 musl target。
- npm 主包新增共享平台选择模块，`install.js` 和 `lib/index.js` 共用同一套逻辑，避免安装时和运行时选择不同包。
- `bifrost upgrade` 的手动 release 下载路径使用相同的最低 glibc 要求，glibc 低于 `2.39` 时直接选择 musl target；若 GNU 下载或安装后验证失败，也继续保留二次 musl fallback。
- npm Linux x64 / ARM64 在以下情况下选择 musl 包：
  - `ldd --version` 显示 musl；
  - `ldd --version` 显示 glibc 且版本小于 `2.39`；
  - Linux libc 状态无法可靠识别。
- Linux ARMv7 当前没有 musl artifact，仍保持 `linux-arm-glibc`。

## 依赖项

- 不新增 Rust 依赖。
- npm 侧仅使用 Node 内置模块：`fs`、`child_process`。
- shell 安装入口不依赖 `sudo`、`apt-get`、`patchelf`。

## 测试方案

### 单元测试

- npm 平台选择纯函数：
  - Debian glibc 2.28 输出 `linux-x64-musl`；
  - glibc 2.39 输出 `linux-x64-glibc`；
  - musl ldd 输出 `linux-x64-musl`；
  - ARMv7 glibc 2.28 仍输出 `linux-arm-glibc`。
- `bifrost upgrade` target 选择：
  - glibc 2.38 需要 musl fallback；
  - glibc 2.39 保持 GNU target；
  - 无法识别 glibc 版本时需要 musl fallback。

### E2E 测试

- 新增 `e2e-tests/tests/test_install_musl_fallback.sh`：
  - source `install-binary.sh` 的函数并模拟 `ldd --version` 输出，验证 glibc 2.28 自动选择 `x86_64-unknown-linux-musl`；
  - 验证 npm 平台选择模块在 Debian 10 样例下选择 `@bifrost-proxy/bifrost-linux-x64-musl`；
  - 验证新 glibc 和 musl 场景不会回归。
- 执行 `cargo test -p bifrost-cli glibc --all-features`，验证 `bifrost upgrade` 对 2.38 / 2.39 / unknown 三种 glibc 状态的 fallback 判断。

### 真实场景测试

- 新增 `human_tests/linux-install-musl-fallback.md`：
- 覆盖 Debian 10 / glibc 2.28 沙箱安装路径；
- 覆盖 npm/npx 运行时选择路径；
- 覆盖 `bifrost upgrade` 旧 glibc release 下载路径；
- 覆盖显式 `--target x86_64-unknown-linux-musl` 手动绕过路径。

## 校验要求

- 执行新增 E2E 脚本。
- 执行 `cargo fmt --all -- --check`。
- 执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 执行 `cargo test --workspace --all-features`。
- 按修改范围执行本地 CI：脚本 / npm 安装入口变更不修改 Rust 逻辑，至少执行 `bash scripts/ci/local-ci.sh --skip-e2e`，如环境阻塞需记录原因。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- README 当前已展示 `install-binary.sh` 安装入口，不需要新增用户命令；若后续发布说明中描述兼容矩阵，应补充“旧 glibc 自动回退 musl”。
