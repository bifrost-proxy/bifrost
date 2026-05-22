# Rust Dependency Audit CI

## 功能模块说明

本模块用于在本地和 CI 中检查 Rust 依赖膨胀风险，覆盖两类问题：

- 未使用的直接依赖：通过 `cargo-udeps` 基于编译器元数据检查，避免纯字符串匹配误伤。
- 重复版本依赖：通过 `cargo-deny` 的 `bans` 检查持续暴露重复版本，但当前配置为 warning，避免对大量传递依赖做高风险的一次性强制收敛。

## 实现逻辑

1. `scripts/ci/rust-dependency-audit.sh` 统一执行依赖审计。
2. 脚本显式检查 `cargo`、`cargo-deny`、`cargo-udeps`、`rustup` 与 nightly toolchain，工具缺失时直接失败并给出错误。
3. `cargo deny check bans --hide-inclusion-graph` 使用 `deny.toml` 读取 all-features graph，并把重复版本设为 warning。
4. `cargo udeps --workspace --all-targets --all-features --locked` 使用 nightly toolchain 和 `SKIP_FRONTEND_BUILD=1`，基于真实编译图发现未使用直接依赖。
5. `.github/workflows/ci.yml` 增加 `dependency-audit` job，先安装固定版本 `cargo-deny` 与 `cargo-udeps`，再执行统一脚本。
6. `scripts/ci/local-ci.sh` 增加同一审计步骤，并提供 `--skip-deps-audit` 供本地快速验证或工具链缺失时显式跳过。

## 依赖项调整

本轮只处理工具确认且人工复核后低风险的直接依赖：

- 删除未使用的 dev-dependency：`tokio-test`、`env_logger`。
- 删除未使用的直接 dependency：`is-terminal`、`async-compression`、`bifrost-core` in `bifrost-power`。
- 把平台相关误报改为 target-specific：`netstat2` 仅非 macOS 编译，`sha1` 仅 Windows 编译。
- `bifrost-proxy` 原 `tokio-test` 只用于间接启用 `tokio/test-util`，改为显式 dev-dependency `tokio` + `test-util` feature，避免隐藏的 feature 依赖。

未在本轮强行收敛重复传递依赖版本。重复版本涉及多个上游 crate 版本链，直接统一容易改变编译图和运行行为，当前先通过 CI warning 持续观测。

## 测试方案

### 单元测试

本改动不新增 Rust 运行时代码，单元测试通过 workspace 聚合命令覆盖：

- `cargo test --workspace --all-features`

### E2E 测试

本改动不修改代理运行时、CLI 行为、WebUI 或网络协议，运行时 E2E 不适用。CI/local-ci 集成通过脚本级真实执行验证。

### 真实场景测试

对应用例文档：

- `human_tests/rust-dependency-audit-ci.md`

覆盖内容：

- 直接执行依赖审计脚本。
- 通过 `local-ci --skip-e2e` 验证审计集成。
- 通过 `local-ci --skip-e2e --skip-deps-audit` 验证显式跳过。
- 通过裁剪 `PATH` 验证工具缺失时失败且提示清晰。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：清理未使用依赖、检查重复依赖、接入 CI、推 PR。
- 复核 diff：Cargo manifest、lockfile、CI/local-ci、设计文档、human_tests。
- 执行脚本级验证：依赖审计脚本、shell 语法、工具缺失路径。
- 修复发现的 manifest 或脚本问题后复跑。

### 第 2 轮

- 复查第 1 轮修复后的完整 diff。
- 复查 target-specific 依赖没有误删跨平台依赖。
- 复跑依赖审计和 local-ci 相关路径。
- 确认没有需要第 3 轮的阻塞问题。

## 校验要求

- `bash scripts/ci/rust-dependency-audit.sh`
- `bash -n scripts/ci/rust-dependency-audit.sh`
- `bash -n scripts/ci/local-ci.sh`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 新增本设计文档。
- 新增 `human_tests/rust-dependency-audit-ci.md`。
- 保持 `human_tests/readme.md` 索引包含该用例。
