# Remote State Device Info

> 状态：已实现 | 更新时间：2026-05-21

## 背景

`bifrost remote conn status` 当前通过 Remote Invoke 的只读 `status` 命令透传目标端 `/_bifrost/api/system`。旧响应只包含版本、Rust 编译器版本、系统、架构、uptime 和 pid；当本地保存多个远端连接时，仅凭这些字段不容易区分具体设备，其中 `rust_version` 对用户识别设备也没有实际价值。

## 目标

1. 在 Remote state / `remote conn status` 输出中补充设备可识别信息：设备名称、CPU 核心数、内存容量与可用内存、当前工作目录所在存储卷容量与可用空间。
2. 移除 `rust_version` 输出，避免无意义字段占用状态信息。
3. 保留现有 `version`、`os`、`arch`、`uptime_secs`、`pid` 字段，降低现有调用方迁移成本。
4. 复用 `/_bifrost/api/system` 作为单一状态事实来源，确保本地 API、Remote Invoke 与 Web 类型定义一致。

## 实现逻辑

- `crates/bifrost-admin/src/metrics.rs` 的 `SystemInfo` 作为系统状态结构：
  - 使用 `sysinfo::System::host_name()` 生成 `device_name`。
  - 使用 `system.cpus().len()` 与 `system.physical_core_count()` 生成逻辑/物理核心数。
  - 使用 `system.total_memory()` 与 `system.available_memory()` 生成内存字段。
  - 使用 `sysinfo::Disks::new_with_refreshed_list()` 选择当前工作目录所在挂载点；找不到时回退根挂载点或第一个磁盘。
  - 序列化时省略无法采集的可选字段。
- `RemoteInvokeExecutor::get_status()` 继续请求 `/_bifrost/api/system`，无需引入新的 Remote Invoke 命令。
- `web/src/types/index.ts` 同步 `SystemInfo` 字段，避免前端类型与 API 漂移。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin metrics::tests::test_system_info`：
  - 验证 `device_name`、`cpu_logical_cores`、`memory_total_bytes`、`memory_available_bytes` 存在且合理。
  - 验证存储字段存在时总容量大于 0，且可用容量不超过总容量。

### E2E 测试

- `bash -n e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash -n e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
- `e2e-tests/tests/test_remote_invoke_e2e.sh` 的 `TC-RI-02`：
  - 断言 `remote conn status` 返回 `version/device_name/os/arch/cpu_logical_cores/memory_total_bytes/memory_available_bytes/storage_total_bytes/storage_available_bytes/storage_mount_point`。
  - 断言响应不包含 `rust_version`。
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 的 SSH saved connection 路径执行同样字段断言。

### 真实场景测试

- 更新 `human_tests/remote-invoke.md`，新增 `TC-RI-回归-141`。
- 使用真实 CLI 执行 `bifrost remote conn status`，验证新增字段可读且 `rust_version` 不再出现。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：确认新增字段覆盖设备名、CPU、内存、存储，且移除 `rust_version`。
- 代码 review：检查 `SystemInfo` 序列化兼容性、磁盘选择 fallback、Web 类型同步、E2E 断言是否覆盖正负字段。
- 复测：运行 admin 单元测试、E2E 语法检查和真实 CLI 场景。

### 第 2 轮

- 再次复核：确认文档、API 示例、human_tests 索引与当前输出一致。
- 复测：复跑受影响测试与 workspace 校验，确认无新增 clippy/fmt/类型错误。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin metrics::tests::test_system_info`
- `cargo test --workspace --all-features`
- 受影响 E2E 脚本语法检查与至少一个真实 `remote conn status` CLI 场景。

## 文档更新要求

- 更新 `crates/bifrost-admin/ADMIN_API.md` 的 System API 示例。
- 更新 `human_tests/api-system.md` 的字段预期。
- 更新 `human_tests/remote-invoke.md` 与 `human_tests/readme.md`。
