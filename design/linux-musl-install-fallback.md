# Linux musl 安装回退

## 背景

Bifrost 的 Linux 发布产物分两类：

- **GNU 目标**：`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`，由 GitHub Actions 较新的 Ubuntu runner 构建，携带 runner 上 glibc 的最低符号版本要求。
- **musl / static 目标**：`x86_64-unknown-linux-musl`、`aarch64-unknown-linux-musl`，静态链接，不依赖 host glibc。

Debian 10 / RHEL 8 / 部分企业内网机器只有 glibc 2.28。这些机器直接运行 GNU 产物时会在动态加载阶段失败，例如缺少 `GLIBC_2.39`。历史上安装脚本和 npm 主包都默认下 GNU 产物，导致这些用户体验极差。

本方案在**所有 Linux x86_64 / aarch64 安装入口**中统一做「glibc 版本探测 -> 自动回退 musl」：

1. `install-binary.sh`：Linux glibc < 2.39 或无法可靠识别时选择 musl。
2. `install-binary.ps1`：Windows 无关，跳过。
3. `npm` 主包：`install.js`（安装阶段）与 `lib/platform.js`（运行时）共用同一份纯函数，避免安装期和运行期选择不同包；`ldd` 输出或 `/lib/ld-musl-*.so.1` loader 存在也可判定 musl。
4. `bifrost upgrade`：手动 release 下载路径使用相同 glibc 阈值，GNU 下载或校验失败时保留二次 musl fallback。

Linux ARMv7 当前没有 musl artifact，继续走 `linux-arm-glibc`，不参与回退。

## 用户目标验证清单

### 必须实现

- 全部 Linux x86_64 / aarch64 安装入口在 glibc < 2.39 时自动选择 musl 产物，不需要用户手工指定 `--target`。
- glibc 无法可靠识别（`ldd` 缺失、输出格式异常）时也走 musl，取「兼容优先」的默认。
- musl loader (`/lib/ld-musl-x86_64.so.1`、`/lib/ld-musl-aarch64.so.1`、`/lib/ld-musl-armhf.so.1`) 存在时直接走 musl，不再依赖 `ldd`。
- `bifrost upgrade` 使用与安装脚本一致的最低 glibc 阈值 `2.39`；GNU 产物下载后 `--version` 校验失败时继续退回 musl。
- npm Linux x64 / ARM64 **musl** 平台包 `package.json` 不声明 `libc` 字段；GNU 包保留 `libc: ["glibc"]`。否则 npm 在 glibc 主机上会用 `libc` 过滤把 musl 包滤掉，静态 fallback 就永远装不上。
- `install.js` 与 `lib/platform.js` 复用同一个 `lib/platform.js` 纯函数模块，安装期与运行期选中的包必须一致。

### 必须不破坏

- glibc >= 2.39 的 Linux 机器仍选 GNU 产物。
- musl-native 主机（Alpine 等）仍选 musl 包。
- Linux ARMv7 (`arm-unknown-linux-gnueabihf`) 无 musl artifact，继续选择 `linux-arm-glibc`。
- 用户显式 `--target x86_64-unknown-linux-musl` 或设置强制环境变量时优先生效。
- 不新增 Rust 依赖；不使用 `sudo` / `apt-get` / `patchelf`。
- npm 侧仅依赖 Node 内置模块 `fs` / `child_process`。

### 必须真实验证

- Debian 10 沙箱下 `curl ... | bash` 自动选择 `x86_64-unknown-linux-musl`，安装完成后 `bifrost --version` 立即可运行，不出现 `GLIBC_2.39 not found`。
- Debian 10 沙箱下 `npm install @bifrost-proxy/bifrost` 自动装 `@bifrost-proxy/bifrost-linux-x64-musl`。
- Debian 10 上运行 `bifrost upgrade` 从 GNU 起点也能收敛到 musl 二进制。
- Alpine 主机走 musl 分支且 `libc` 字段不阻挡安装。

## 产品语义

回退核心是一个「glibc 状态 -> target 名」的纯函数：

- `musl` -> `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` / npm 包 `linux-x64-musl` / `linux-arm64-musl`
- `glibc` -> `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu` / npm 包 `linux-x64` / `linux-arm64`
- ARMv7 恒定 `linux-arm-glibc`

glibc 状态判定规则（`npm/bifrost/lib/platform.js` 是唯一权威实现）：

1. `ldd --version` 输出包含 `musl` -> `musl`
2. `ldd --version` 输出包含 glibc 版本且 `<2.39` -> `musl`
3. `ldd --version` 输出包含 glibc 版本且 `>=2.39` -> `glibc`
4. `ldd` 缺失或输出无法解析且存在 musl loader -> `musl`
5. 全部无法识别 -> `musl`（兼容优先默认）

`bifrost upgrade` 中 Rust 侧复用 `MIN_GLIBC_VERSION = (2, 39)` 常量做等价判断（`crates/bifrost-cli/src/commands/upgrade.rs:258`）。

## 技术细节

### `install-binary.sh`

- 增加 `detect_linux_libc_target()` 函数：读取 `ldd --version`，解析主次版本；`glibc < 2.39` 时把 Linux target 从 `gnu` 改成 `musl`。
- 探测顺序与 `npm/bifrost/lib/platform.js` 完全一致，规避「安装脚本用 GNU、npm 用 musl」的分裂。

### npm 主包

- `npm/bifrost/lib/platform.js`：常量 `MIN_GLIBC_VERSION = "2.39"`，导出 `selectPlatform(...)` / `versionLt(...)` / `parseGlibcVersion(...)` / `hasMuslLoader(...)`。
- `npm/bifrost/install.js`：`postinstall` 阶段调用同一 `selectPlatform`，安装匹配平台包。
- `npm/bifrost/lib/index.js`：运行时 `spawn` 找二进制之前调用同一 `selectPlatform`。
- 平台子包：
  - `npm/bifrost-linux-x64/package.json`：`"libc": ["glibc"]`
  - `npm/bifrost-linux-x64-musl/package.json`：**不声明** `libc`
  - `npm/bifrost-linux-arm64/package.json`：`"libc": ["glibc"]`
  - `npm/bifrost-linux-arm64-musl/package.json`：**不声明** `libc`
  - `npm/bifrost-linux-arm/package.json`：`"libc": ["glibc"]`
- `scripts/npm-publish.mjs`：发布顺序与 manifest 校验，防止 musl 包缺失即发布。

### `bifrost upgrade`

- `crates/bifrost-cli/src/commands/upgrade.rs`：
  - `MIN_GLIBC_VERSION: (u32, u32) = (2, 39)` 常量。
  - `requires_musl(glibc_version: Option<(u32, u32)>) -> bool`：`glibc_version < MIN_GLIBC_VERSION` 或 `None` 时返回 `true`。
  - manual release 下载路径先按 `requires_musl` 选择 target；若 GNU 下载或 `--version` 校验失败，保留二次 musl fallback。

### 依赖项

- 不新增 Rust 依赖。
- npm 侧仅使用 Node 内置模块 `fs`、`child_process`。
- shell 安装入口不依赖 `sudo`、`apt-get`、`patchelf`。

### 相关文件

- `install-binary.sh`
- `npm/bifrost/install.js`
- `npm/bifrost/lib/platform.js`
- `npm/bifrost/lib/index.js`
- `npm/bifrost/package.json`
- `npm/bifrost-linux-x64/package.json`
- `npm/bifrost-linux-x64-musl/package.json`
- `npm/bifrost-linux-arm64/package.json`
- `npm/bifrost-linux-arm64-musl/package.json`
- `npm/bifrost-linux-arm/package.json`
- `scripts/npm-publish.mjs`
- `crates/bifrost-cli/src/commands/upgrade.rs`
- `e2e-tests/tests/test_install_musl_fallback.sh`
- `human_tests/linux-install-musl-fallback.md`
- `human_tests/readme.md`

## CLI + Web + Admin API

- CLI：`curl ... | bash`、`npm install @bifrost-proxy/bifrost`、`bifrost upgrade` 均自动选择正确 target；用户显式 `--target x86_64-unknown-linux-musl` 保留覆盖能力。
- Web / Admin API：无相关面。

## Sync 边界

- 与 Bifrost sync 服务无耦合。musl / GNU 选择结果只写入本地文件系统与 npm 平台包缓存，不涉及远端配置。

## Phase 1-4

### Phase 1：npm 平台选择模块

- 抽出 `lib/platform.js`，`install.js` 与 `lib/index.js` 共用。
- 平台子包 manifest 校准：musl 不声明 `libc`。
- 单元测试覆盖 Debian glibc 2.28 / glibc 2.39 / musl / ARMv7 / 未知 glibc 状态。

### Phase 2：shell 安装入口

- `install-binary.sh` 增加 `detect_linux_libc_target()`，行为与 npm 模块一致。
- 覆盖 `ldd` 缺失、`ldd` 格式异常、`/lib/ld-musl-*.so.1` 存在等场景。

### Phase 3：`bifrost upgrade` glibc 判断

- Rust `MIN_GLIBC_VERSION` + `requires_musl`。
- 手动 release 下载路径优先选 musl；GNU 校验失败保留二次 fallback。

### Phase 4：文档与真实沙箱验证

- `human_tests/linux-install-musl-fallback.md` 覆盖 Debian 10 / npm / upgrade / 显式 `--target` 四条路径。
- `human_tests/readme.md` 索引更新。
- README / release notes 若描述兼容矩阵，补充「旧 glibc 自动回退 musl」。

## 测试方案

### 单元测试

- **npm 平台选择（纯函数，`npm/bifrost/lib/platform.js`）**：
  - Debian glibc 2.28 -> `linux-x64-musl` -> `@bifrost-proxy/bifrost-linux-x64-musl`
  - glibc 2.39 -> `linux-x64-glibc` -> `@bifrost-proxy/bifrost-linux-x64`
  - musl ldd -> `linux-x64-musl`
  - ARMv7 glibc 2.28 仍输出 `linux-arm-glibc`
  - 未知 glibc 状态 -> `linux-x64-musl`
- **npm 平台包 manifest**：
  - GNU Linux x64 保留 `libc: ["glibc"]`
  - Linux x64 / ARM64 musl 不声明 `libc`
- **`bifrost upgrade` target 选择（`crates/bifrost-cli/src/commands/upgrade.rs`）**：真实存在的测试 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib glibc`：
  - `test_glibc_2_38_requires_musl_for_upgrade`
  - `test_glibc_2_39_keeps_gnu_for_upgrade`
  - `test_unknown_glibc_requires_musl_for_upgrade`

### E2E 测试

- `e2e-tests/tests/test_install_musl_fallback.sh`：
  - `source install-binary.sh` 的函数并 stub `ldd --version` 输出，验证 glibc 2.28 自动选择 `x86_64-unknown-linux-musl`。
  - 在 Node 中 require `npm/bifrost/lib/platform.js`，用 Debian 10 样例（`ldd (Debian GLIBC 2.28-10) 2.28`）验证选中 `@bifrost-proxy/bifrost-linux-x64-musl`。
  - 校验 `npm/bifrost-linux-x64-musl/package.json`、`npm/bifrost-linux-arm64-musl/package.json` 不再声明 `libc` 字段。
  - glibc 2.39 与 musl-native 场景不回归。
- `cargo test -p bifrost-cli glibc --all-features`：覆盖 `bifrost upgrade` 三种 glibc 状态判断。

### 真实场景测试

- `human_tests/linux-install-musl-fallback.md`：
  - 覆盖 Debian 10 / glibc 2.28 沙箱安装路径（`curl -fsSL install-binary.sh | bash` 自动落 musl；`bifrost --version` 立即可跑）。
  - 覆盖 npm/npx 运行时选择路径（`npm install -g @bifrost-proxy/bifrost` 后 `npx bifrost --version`）。
  - 覆盖 `bifrost upgrade` 旧 glibc release 下载路径（旧 daemon + 本地 archive + upgrade）。
  - 覆盖显式 `--target x86_64-unknown-linux-musl` 手动绕过路径。
- 每次修改后必须按文档逐条执行并记录实际结果，禁止仅跑单元测试就交付。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 用户目标：Linux x64 / aarch64 在旧 glibc 主机自动落 musl；npm `libc` 过滤不再挡 musl 包；`bifrost upgrade` 与安装脚本使用同一阈值。
- 变更范围：`git status --short` 覆盖 `install-binary.sh`、`npm/bifrost/`、`npm/bifrost-linux-*-musl/package.json`、`crates/bifrost-cli/src/commands/upgrade.rs`、`e2e-tests/tests/test_install_musl_fallback.sh`、`human_tests/linux-install-musl-fallback.md`。
- 复测：
  - `bash e2e-tests/tests/test_install_musl_fallback.sh`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib glibc`
  - Node 单元测试或直接 `node -e "require('./npm/bifrost/lib/platform.js').selectPlatform(...)"` 校验纯函数返回值。

### 第 2 轮

- 检查 `install.js` 与 `lib/index.js` 是否 require 同一份 `platform.js`；防止后续维护把它们分叉。
- 检查 `scripts/npm-publish.mjs` 是否会在缺少 musl 包时报错。
- 复测：Debian 10 真实沙箱、Alpine 真实沙箱、Ubuntu 22.04（glibc 2.35） -> 走 musl、Ubuntu 24.04（glibc 2.39） -> 走 GNU。

## 校验要求

- `bash e2e-tests/tests/test_install_musl_fallback.sh`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib glibc`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 按修改范围执行本地 CI：脚本 / npm 安装入口变更不修改 Rust 逻辑，至少执行 `bash scripts/ci/local-ci.sh --skip-e2e`，如环境阻塞需记录原因。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- README 当前已展示 `install-binary.sh` 入口，不需要新增用户命令；若后续发布说明中描述兼容矩阵，应补充「旧 glibc 自动回退 musl」。

## 风险与决策

- 阈值 `glibc 2.39` 与 CI runner 上构建 GNU 产物时的最低符号版本绑定；如果未来 CI runner 降级到旧 Ubuntu，阈值需要同步降低，否则会把本可以用 GNU 的机器错误回退到 musl。
- npm musl 包不声明 `libc` 是**故意**的：glibc 主机安装主包时 npm 会用 `libc` 过滤把 musl 包滤掉，导致「回退方案」永远装不上。GNU 包仍保留 `libc: ["glibc"]`，musl 主机不会安装 GNU 包，语义仍然安全。
- Linux ARMv7 无 musl artifact 是当前发布决策；如需扩展需新增 CI 目标 `armv7-unknown-linux-musleabihf` 与对应 npm 包。
- 静态 musl 二进制比 GNU 大约多 30% 体积，是兼容性权衡；不接受牺牲兼容性换体积。
- 用户如果通过 `BIFROST_TARGET` 或 `--target` 显式覆盖，必须优先生效；测试脚本需要专门覆盖显式覆盖路径，避免自动回退把用户意图吃掉。
