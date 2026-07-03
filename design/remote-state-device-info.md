# Remote State Device Info

> 状态：已实现 | 更新时间：2026-07-03

## 背景

`bifrost remote conn status` 通过 Remote Invoke 只读 `status` 命令透传 target 侧 `/_bifrost/api/system`。旧响应仅包含 `version / rust_version / os / arch / uptime_secs / pid`：

- 用户本地通常保留多台远端连接（家中 Mac、公司 Mac、远程测试机），仅凭 `version + os + arch` 不足以辨认设备。
- `rust_version` 对识别设备与判断可用性无实际价值，反而占用状态展示的密度预算。
- 前端 `SystemInfo` 类型没有关键设备识别字段，Remote Invoke Recent Calls 与 UI 无法展示 device_name / 存储 / 内存 等易读信息。

## 用户目标验证清单

### 必须实现

- `remote conn status` 与 `/_bifrost/api/system` 输出 `device_name`（等价于 `sysinfo::System::host_name()`）。
- 输出 CPU 逻辑核心数 `cpu_logical_cores`；若可获取物理核心数则输出 `cpu_physical_cores`。
- 输出内存字段 `memory_total_bytes` / `memory_available_bytes`。
- 输出当前 `cwd` 所在挂载点的存储字段 `storage_total_bytes` / `storage_available_bytes` / `storage_mount_point`；不可采集时省略。
- 移除 `rust_version` 字段，避免对用户无价值信息占位。
- 保留 `version` / `os` / `arch` / `uptime_secs` / `pid`，降低现有调用方迁移成本。
- 前端 `web/src/types/index.ts::SystemInfo` 与后端 `bifrost-admin::metrics::SystemInfo` 字段完全对齐。

### 必须不破坏

- `/_bifrost/api/system` 仍是系统状态的唯一事实源，不引入新的 Remote Invoke 命令。
- 只读 grant 的 `remote conn status` 权限模型不变。
- SSH saved connection 场景（`remote connect --ssh-key`）与直接 `remote connect --relay` 场景返回字段一致。
- Recent Calls、审计与 SSE 推送对 `status` 响应结构的现有解析继续工作。

### 必须真实验证

- 真实 CLI 执行 `bifrost remote conn status` 至少一台 macOS 与一台 Linux target，肉眼确认字段可读且能区分设备。
- SSH saved connection 场景执行同样命令，字段完全一致。
- 断言响应不再包含 `rust_version`。

## 产品语义

### 字段清单

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `version` | string | Bifrost 版本 |
| `device_name` | string | `sysinfo::System::host_name()`，无法获取时回退 `"unknown"` |
| `os` | string | e.g. `macOS`, `Linux`, `Windows` |
| `arch` | string | e.g. `aarch64`, `x86_64` |
| `cpu_logical_cores` | number | 逻辑核心数 |
| `cpu_physical_cores` | number, optional | 物理核心数（`sysinfo::System::physical_core_count`） |
| `memory_total_bytes` | number | 总内存 |
| `memory_available_bytes` | number | 可用内存 |
| `storage_total_bytes` | number, optional | 当前 cwd 挂载点总容量 |
| `storage_available_bytes` | number, optional | 当前 cwd 挂载点可用容量 |
| `storage_mount_point` | string, optional | 挂载点路径 |
| `uptime_secs` | number | 进程运行时间 |
| `pid` | number | 进程号 |

**已移除**：`rust_version`。

### 存储字段选择规则

`sysinfo::Disks::new_with_refreshed_list()` 拿到磁盘列表，按当前 `std::env::current_dir()` 匹配最长 mount_point 前缀，找不到时依次回退：

1. 根挂载点 `/`（Windows 下为当前盘符根）。
2. 第一个可枚举磁盘。
3. 全部失败时 `storage_*` 字段全部省略；序列化时通过 `Option + #[serde(skip_serializing_if = "Option::is_none")]` 保持兼容。

### CLI 输出示例

```text
$ bifrost remote conn status my-mac
Device       : eden-macbook-air.local
Bifrost      : 4.7.2 (macOS aarch64)
CPU          : 8 logical / 8 physical
Memory       : used 12.3 GiB / 16.0 GiB (available 3.7 GiB)
Storage      : / — used 210.4 GiB / 500.7 GiB (available 290.3 GiB)
Uptime       : 4h 12m
PID          : 88231
```

`remote conn status --json` 直接返回上表结构，用于自动化与 UI 消费。

## 技术细节

### 后端

`crates/bifrost-admin/src/metrics.rs`：

```rust
pub struct SystemInfo {
    pub version: String,
    pub device_name: String,          // line 567
    pub os: String,
    pub arch: String,
    pub cpu_logical_cores: usize,     // line 570
    pub cpu_physical_cores: Option<usize>,
    pub memory_total_bytes: u64,      // line 573
    pub memory_available_bytes: u64,
    pub storage_total_bytes: Option<u64>,      // line 576
    pub storage_available_bytes: Option<u64>,
    pub storage_mount_point: Option<String>,   // line 580
    pub uptime_secs: u64,
    pub pid: u32,
}
```

`SystemInfo::new(start_time)` 构造入口（line 585）：

- `System::host_name().unwrap_or_else(|| "unknown".to_string())`。
- `system.cpus().len()` 与 `system.physical_core_count()`。
- `system.total_memory()` / `system.available_memory()`。
- `sysinfo::Disks::new_with_refreshed_list()` + cwd 前缀匹配。
- 无法采集的字段返回 `None`；`Serialize` 时通过 `skip_serializing_if` 省略。

### Remote Invoke 复用

`RemoteInvokeExecutor::get_status()` 继续走 `GET /_bifrost/api/system` 只读通道，不新增 Remote Invoke 命令：

- caller `bifrost remote conn status` 调用 `remote_query.status` 只读 opcode。
- worker 侧命中现有 `/_bifrost/api/system` handler，返回 `SystemInfo`。
- 保持 `remote_query` scope 覆盖，不需要重新申请 grant。

### 前端类型

`web/src/types/index.ts`：

```ts
export interface SystemInfo {
  version: string;
  device_name: string;                 // line 539
  os: string;
  arch: string;
  cpu_logical_cores: number;
  cpu_physical_cores?: number;
  memory_total_bytes: number;
  memory_available_bytes: number;
  storage_total_bytes?: number;
  storage_available_bytes?: number;
  storage_mount_point?: string;        // line 548
  uptime_secs: number;
  pid: number;
}
```

Remote Invoke Recent Calls 详情面板与 Settings → Status Tab 消费此结构。

### CLI + Web + Admin API

- CLI：`bifrost remote conn status <alias> [--json]`。JSON 输出即上表；文本输出按上例排版。
- Web：Remote Invoke → Connections → Detail 弹窗展示 device_name / cpu / memory / storage，Recent Calls status 命令响应体也走同 schema。
- Admin API：`GET /_bifrost/api/system` 返回 `SystemInfo`。字段变化不新增 API 路径。

### Sync 边界

- `SystemInfo` 是运行时状态，不参与配置 sync。
- Remote Invoke 只读通道本身不写入 target 数据目录，无迁移成本。
- 存量客户端解析 `rust_version` 时会拿到 `undefined`，需要在前端和 CLI 端一次性移除该字段的展示。

## Phase 1-4 拆分

### Phase 1：后端字段扩展与移除

- `SystemInfo` 增加 `device_name / cpu_logical_cores / cpu_physical_cores / memory_* / storage_*`。
- 删除 `rust_version` 字段与 `env!("...")` 采集。
- 补齐 `SystemInfo::new` 的 disk fallback 逻辑。
- 单元测试 `test_system_info`：覆盖字段存在与合理性。

### Phase 2：Remote Invoke 端到端联通

- 保持 `RemoteInvokeExecutor::get_status()` 走 `/_bifrost/api/system`。
- E2E `TC-RI-02` 与 SSH saved connection 场景断言新字段与 `!has("rust_version")`。

### Phase 3：前端与文档

- `web/src/types/index.ts::SystemInfo` 同步。
- Settings → Status Tab / Remote Invoke Connections Detail / Recent Calls 使用新字段。
- 更新 `crates/bifrost-admin/ADMIN_API.md` System API 示例。
- 更新 `human_tests/api-system.md`、`human_tests/remote-invoke.md`、`human_tests/readme.md`。

### Phase 4：真实场景回归

- 至少一台 macOS + 一台 Linux target 真机执行 `bifrost remote conn status`。
- 断言 `device_name` 与 `hostname` 一致；`memory_available_bytes <= memory_total_bytes`。
- 记录执行结果到 `human_tests/remote-invoke.md`（`TC-RI-回归-147`）。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin metrics::tests::test_system_info`（`crates/bifrost-admin/src/metrics.rs::697-706`）：
  - `!info.device_name.is_empty()`
  - `info.cpu_logical_cores > 0`
  - `info.memory_total_bytes > 0`
  - `info.memory_available_bytes <= info.memory_total_bytes`
  - `storage_total_bytes` 存在时 `> 0` 且 `available <= total`

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_e2e.sh` 的 `TC-RI-02`（line 525）：直接 `bifrost remote conn status`，使用 `jq` 断言：
  - `.device_name`、`.memory_total_bytes | type == "number"`
  - `.storage_total_bytes | type == "number"`
  - `.storage_available_bytes <= .storage_total_bytes`
  - `has("rust_version") | not`
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：SSH saved connection 走同样断言。
- `bash -n e2e-tests/tests/test_remote_invoke_e2e.sh` / `test_remote_invoke_ssh_e2e.sh`：脚本语法自检。

### 真实场景测试（human_tests）

`human_tests/remote-invoke.md`：

- `TC-RI-回归-147`：真实 CLI 执行 `bifrost remote conn status` 验证字段可读且 `rust_version` 不再出现。
- 分别在 macOS aarch64 与 Linux x86_64 target 上记录执行输出。

`human_tests/api-system.md`：更新 `/_bifrost/api/system` 字段预期。

`human_tests/readme.md`：更新用例数量与最新变更条目。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：确认新增字段覆盖 device_name / cpu / memory / storage，且 `rust_version` 已删除。
- 代码 review：`SystemInfo` 序列化兼容、磁盘选择 fallback、Web 类型同步、E2E 断言覆盖正负字段。
- 复测：`cargo test -p bifrost-admin metrics`、`bash e2e-tests/tests/test_remote_invoke_e2e.sh TC-RI-02`、真实 CLI 场景。

### 第 2 轮

- 复核文档、API 示例、human_tests 索引与当前输出一致。
- 检查 `git status --short` 与 `git diff`，确认无遗留 `rust_version` 引用（前端、CLI 格式化、测试断言）。
- 复测：workspace 校验 + 复跑受影响 E2E。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin metrics::tests::test_system_info`
- `cargo test --workspace --all-features`
- 受影响 E2E 脚本语法检查与至少一个真实 `remote conn status` CLI 场景

## 文档更新要求

- `crates/bifrost-admin/ADMIN_API.md` System API 示例。
- `human_tests/api-system.md` 字段预期。
- `human_tests/remote-invoke.md`（`TC-RI-回归-147`）。
- `human_tests/readme.md` 索引与用例数量。

## 风险与决策

- **`rust_version` 一次性删除**：前端与 CLI 需要同时删除，不做兼容旧字段的输出，避免用户误读。
- **磁盘挂载点选择**：cwd 命中最长前缀，Windows 路径大小写与 UNC 前缀需额外规范化；先按现有 `sysinfo` 语义处理，后续如出现误判再补规范化。
- **Recent Calls 存储**：`SystemInfo` 结果不写入 relay 长期存储，只走短期 SSE / call meta；如果需要跨端持久化，需要单独的 metrics history 设计。
- **新字段体积**：单次 `status` 增加约 80~120 字节；对 SSE 与 traffic export 均无实际影响。
