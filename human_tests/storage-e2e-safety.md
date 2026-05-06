# Storage and E2E Safety 真实场景测试用例

## 功能模块说明

覆盖 E2E 测试环境变量作用域安全与规则文件 size guard 抽象回归。目标是避免并发测试直接长期修改进程 env，并确保规则文件大小限制从 `bifrost-core::limits` 统一复用。

## 前置条件

```bash
cd <REPO_ROOT>
export CARGO_TARGET_DIR=./.codex-target/fixpass
```

## 测试用例列表

### TC-SES-01: temp-env 作用域编译回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo check -p bifrost-e2e --quiet
  ```
- **预期结果**: 编译通过；`im_gateway_agent` 不再依赖手写 `EnvVarGuard` 和直接 `std::env::set_var`。
- **本次执行结果**: 2026-05-03 通过，`cargo check -p bifrost-e2e --quiet` 无错误输出。

### TC-SES-02: core size guard 单元回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-core ensure_file_size_within_limit_rejects_oversized_file --quiet
  ```
- **预期结果**: 测试通过；超过 limit 的文件返回 `file too large` 错误。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-SES-03: storage rules size guard 编译回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo check -p bifrost-storage --quiet
  ```
- **预期结果**: 编译通过；`RulesStorage` 使用 `bifrost_core::limits::{ensure_file_size_within_limit, MAX_RULE_FILE_BYTES}`。
- **本次执行结果**: 2026-05-03 通过，`cargo check -p bifrost-storage --quiet` 无错误输出。

## 清理步骤

测试使用临时文件或编译检查，无需手动清理。
