# Rust Dependency Audit CI 真实场景测试

## 功能模块说明

验证 Rust 依赖审计能力：未使用依赖通过 `cargo-udeps` 检查，重复版本依赖通过 `cargo-deny` 检查，并接入 GitHub Actions 与本地 `local-ci`。

## 前置条件

1. 当前目录为仓库根目录。
2. 已安装 Rust stable 与 nightly toolchain。
3. 已安装 `cargo-deny` 与 `cargo-udeps`。
4. 当前工作区依赖已可正常解析。

## 测试用例列表

### TC-RDA-01 直接执行 Rust 依赖审计脚本

操作步骤：

1. 执行 `bash scripts/ci/rust-dependency-audit.sh`。

预期结果：

1. 命令退出码为 0。
2. 输出包含 `Checking duplicate Rust dependencies with cargo-deny`。
3. 输出包含 `Checking unused Rust dependencies with cargo-udeps`。
4. `cargo-udeps` 输出未报告 unused dependencies。

### TC-RDA-02 local-ci 默认静态路径执行依赖审计

操作步骤：

1. 执行 `bash scripts/ci/local-ci.sh --skip-e2e`。

预期结果：

1. 命令退出码为 0。
2. Local CI Report 中包含 `Rust dependency audit`。
3. `Rust dependency audit` 结果为 PASS。

### TC-RDA-03 local-ci 可显式跳过依赖审计

操作步骤：

1. 执行 `bash scripts/ci/local-ci.sh --skip-e2e --skip-deps-audit`。

预期结果：

1. 命令退出码为 0。
2. Local CI Report 中包含 `Rust dependency audit (skipped)`。
3. 其他静态检查仍正常执行。

### TC-RDA-04 工具缺失时给出明确错误

操作步骤：

1. 执行 `PATH="/usr/bin:/bin" bash scripts/ci/rust-dependency-audit.sh`。

预期结果：

1. 命令退出码非 0。
2. stderr 输出包含 `required command not found`。
3. 错误信息包含缺失工具名称。

## 清理步骤

本测试不启动服务、不创建运行时数据目录，无需清理临时服务。若本地执行过程中产生 Cargo 编译缓存，可保留在标准 `target/` 目录中。
