# Script 协议与脚本系统

## 背景

Bifrost 早期 RFC 只定义了 `reqScript` / `resScript` 两个协议，脚本只暴露纯请求/响应对象，并规划“完全禁止文件与网络访问”。真实实现远超出这个范围：

- 协议层已经稳定支持 `reqScript` / `resScript` / `decode` 三个协议（`crates/bifrost-core/src/protocol.rs`）。
- 脚本运行时基于 `rquickjs` 实现独立线程执行、超时/内存/网络配额、文件根隔离与云元数据地址阻断（`crates/bifrost-script/src/engine.rs`、`crates/bifrost-script/src/sandbox.rs`）。
- Admin 侧有独立的 `Scripts` 页面（`web/src/pages/Scripts/index.tsx`）和 sandbox 配置 API `/api/config/sandbox`（`crates/bifrost-admin/src/handlers/config.rs`）。
- Traffic Detail 打通了脚本执行日志和结果展示，`ScriptLogs` pane 展示日志、脚本命中和修改摘要。
- 内置 BP parser (`build_in_bp`) 通过 `crates/bifrost-script/src/builtins.rs` 释放到 `scripts/parser/build_in_bp.js`，作为 `decode` 协议的默认解析器。

本文重写为“已实现能力说明 + 运行时/协议契约 + 后续扩展点”，替代过时的 RFC。

## 用户目标验证清单

### 必须实现

- 规则语法能同时支持 `reqScript://<name>`、`resScript://<name>`、`decode://<name>`，脚本名解析到 `scripts/{request,response,parser}/<name>.js`。
- 脚本运行在独立线程 + `rquickjs` 引擎，命中超时/内存上限时立即中止并回收上下文。
- Sandbox 提供 `file`、`net` 两组 API，受 `SandboxConfig` 配额管理；默认关闭 `allow_network` 与 `allow_private_network`。
- `net.fetch` 默认拒绝 loopback、私网、link-local、metadata、文档、多播、未指定与广播地址；用户显式打开 `allow_private_network` 才能访问内网。
- Admin API 暴露 `GET /api/config/sandbox` 与 `PUT /api/config/sandbox`，Web `Scripts` 页面能实时改配置。
- 内置 `build_in_bp.js` 每次进程启动都会释放到 `scripts/parser/` 目录，用户可编辑，编辑后仍保留自定义内容。
- Traffic Detail 能显示每条脚本执行日志 (`ctx.log`) 与请求/响应改动摘要。
- 脚本执行失败（语法错误、超时、内存爆、sandbox 拒绝）时，代理链路继续走原始请求/响应，同时把错误落到脚本日志。

### 必须不破坏

- 老规则 `reqScript://xxx` / `resScript://xxx` 语义不变。
- 沙箱默认策略保持“无网络、无文件”；升级 Bifrost 不会隐式解锁网络。
- Traffic Detail 现有面板（Request/Response/Headers/Timing）不因脚本 pane 位置变化而破坏排版。
- CLI 里 `bifrost rule` 打印脚本规则时仍按 `protocol://value` 展示。

### 必须真实验证

- Web `Scripts` 页面能创建/编辑/保存 request/response/parser 脚本，Monaco 编辑器提供类型补全。
- 修改 sandbox 配置后新脚本生效，老脚本按当前配置继续跑。
- 命中脚本的流量在 Traffic Detail 中显示日志与修改。
- CLI `bifrost script list/show` 命令展示脚本列表和内容。

## 产品语义

### 三类脚本

| 协议 | 触发时机 | 输入对象 | 常见用途 |
|---|---|---|---|
| `reqScript` | 请求发出前 | `request`（method/url/headers/body） | 改写 header、注入鉴权、mock、条件短路 |
| `resScript` | 响应到达后 | `response`（status/headers/body） | 改状态码、脱敏、注入 HTML、录制 |
| `decode` | Traffic Detail 展示时 | `request` + `response` + `raw` | 私有协议解码（默认走 `build_in_bp`） |

统一暴露：
- `ctx`：命中信息（rule 名、matched pattern、request id、timestamp）
- `log(...)`：写入 pane 日志
- `file` / `net`：受配额限制的 IO API

### Sandbox 配额

`SandboxConfig` (`crates/bifrost-script/src/sandbox.rs:22`)：

```rust
pub struct SandboxConfig {
    pub timeout_ms: u64,                // 默认 10_000
    pub max_memory: usize,              // 默认 32 MiB
    pub file_root: Option<PathBuf>,     // 未设置则禁用 file.*
    pub file_allowed_dirs: Vec<PathBuf>,
    pub allow_network: bool,            // 默认 false
    pub allow_private_network: bool,    // 默认 false
    pub network_timeout_ms: u64,        // 默认 5_000
    pub max_file_bytes: usize,          // 默认 1 MiB
    pub max_net_request_bytes: usize,   // 默认 256 KiB
    pub max_net_response_bytes: usize,  // 默认 1 MiB
}
```

`allow_network=false` 时 `net.fetch` 直接抛错；`allow_network=true` 且 `allow_private_network=false` 时经过 `validate_net_fetch_target` 阻断 loopback/私网/链路本地/云元数据等地址。

### 失败关闭

脚本抛错、超时、内存爆、语法错误、sandbox 拒绝：
- 请求脚本失败：请求原样继续。
- 响应脚本失败：响应原样返回。
- Decode 脚本失败：Traffic Detail 展示 raw + 错误提示。
- 所有失败写入 `logs`，Traffic Detail `ScriptLogs` pane 可见。

## 技术细节

### 引擎与执行模型

`ScriptEngine`（`crates/bifrost-script/src/engine.rs:34`）：

- 每次 execute 都在专用 tokio blocking 线程 + 新 `rquickjs::Runtime` 中运行，避免脚本间状态污染。
- `SandboxConfig` 由 `ScriptEngineConfig`（`get_sandbox_config` 的值）实时映射，改配置无需重启 Bifrost。
- 提供 `execute_request_script[_with_config]`、`execute_response_script[_with_config]`、`execute_parser_script`、`test_script` 等 API；proxy 层调用带 `_with_sandbox` 版本以传入 admin 覆盖的运行时配置。
- 中断机制：`InterruptState` 记录起始时间，命中超时后 `rquickjs::Runtime::set_interrupt_handler` 立即中止 JS 循环并抛 `ScriptError::Timeout`。

### 协议解析与加载

- `crates/bifrost-core/src/protocol.rs`：`Protocol::{ReqScript, ResScript, Decode}` 常量与字符串互转。
- `crates/bifrost-core/src/rule/parser/mod.rs`：解析 `reqScript://foo`、`resScript://bar`、`decode://baz`，映射到规则动作。
- `RulesResolver` 命中脚本规则后把脚本名交给 `ScriptEngine`；`decode` 规则在 Traffic Detail 展示时按响应内容再触发一次执行。

### 内置 parser

- `crates/bifrost-script/src/builtins.rs`：`BUILD_IN_BP_SCRIPT_NAME = "build_in_bp"`，包含 `include_str!("../../../assets/scripts/parser/build_in_bp.js")`。
- 启动时 `release_parser_scripts` 释放到 `scripts/parser/`；已存在同名文件不覆盖，允许用户接管。
- BP parser 可以调用 `net.fetch` 拉取 BAM schema，只有当用户显式打开 `allow_private_network` 时才能访问内部 BAM mock。

### 沙箱管理 API

`crates/bifrost-admin/src/handlers/config.rs`：

- `GET /api/config/sandbox`：返回当前 `sandbox` 配置。
- `PUT /api/config/sandbox`：接受 `{ file, net, limits }` 三段更新，字段 `#[serde(default)]`，未传字段保留旧值。
- `UpdateSandboxNetConfigRequest` 字段：`enabled`、`allow_private_network`、`timeout_ms`、`max_request_bytes`、`max_response_bytes`。
- 校验：`sandbox_dir` 非空、`allowed_dirs` 必须绝对路径，否则 400。
- 更新写入 `ConfigManager`，同时刷新内存运行时用的 `SandboxConfig`。

### Web `Scripts` 页面

- `web/src/pages/Scripts/index.tsx`：Monaco 编辑器 + 类型定义 + 保存/删除/测试。
- 类型定义暴露 `request`, `response`, `ctx`, `log`, `file`, `net`。
- 侧栏三分类：`request` / `response` / `parser`；`parser` 分类里含 `build_in_bp` 与用户新增。
- 顶部 `Sandbox settings` 抽屉调用 `/api/config/sandbox`，展示 file/net/limits 三段，`allow_private_network` 有明确 opt-in 警示。

## CLI / Web / Admin API 快照

| 层 | 入口 | 能力 |
|---|---|---|
| CLI | `bifrost script list/show/set/delete/test`（若已实现） | 命令行管理脚本 |
| CLI | `bifrost rule *` | 规则中通过 `protocol://value` 使用脚本 |
| Web | `/scripts` 页 | Monaco 编辑器、类型提示、测试、Sandbox 抽屉 |
| Admin API | `GET/PUT /api/config/sandbox` | 沙箱运行时配置 |
| Admin API | `GET/PUT/POST/DELETE /api/scripts/...` | 脚本列表与内容管理 |

## Sync 边界

- 脚本内容视为设备本地资产，第一版不自动 sync（避免多设备共用同一 fetch 授权导致误 opt-in）。
- Sandbox 配置也不 sync：每台设备的 file_root/allowed_dirs 路径不同。
- 允许用户手动导入/导出脚本（`bifrost script export/import` 或复制文件）。

## Phase 拆分

- **Phase 1（已完成）**：`reqScript` / `resScript` 协议 + 执行引擎 + 基础 sandbox。
- **Phase 2（已完成）**：`decode` 协议、内置 `build_in_bp`、Traffic Detail script log pane。
- **Phase 3（已完成）**：Sandbox 配置 API、Web Scripts 页面、Monaco 类型提示、`allow_private_network` opt-in。
- **Phase 4（进行中）**：更强的 sandbox 观测（每次 fetch 的 audit log）、脚本级 CPU/IO metrics、脚本 grants 与规则关联。

## 测试方案

### 单元测试

- `crates/bifrost-script/src/engine/tests.rs`：request/response/parser 三类脚本 happy path 与错误路径。
- `crates/bifrost-script/src/sandbox.rs`：`validate_net_fetch_target`、`is_private_netfetch_ip`、文件根隔离、`file_allowed_dirs` 白名单。
- `crates/bifrost-admin/src/handlers/config.rs::update_sandbox_config_request_deserializes_nested_sections`、`update_sandbox_config_accepts_valid_payload`、`update_sandbox_config_rejects_non_absolute_allowed_dir`。
- `crates/bifrost-core/src/protocol.rs`：`Protocol::{ReqScript,ResScript,Decode}` 大小写不敏感解析。

### E2E 测试

- `e2e-tests/tests/test_bp_parser_e2e.sh`：默认 `allow_private_network=false` 时 BAM mock 拒绝；显式 opt-in 后 `build_in_bp` 可解码。
- `e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`：脚本执行链路启动断言。
- Web Playwright：`web/tests/ui/admin-scripts.spec.ts` 覆盖 Scripts 页面创建、编辑、保存、测试脚本。

### 真实场景测试 human_tests

- `human_tests/script-protocol-rfc.md` 或与 sandbox/BP 合并的用例：
  - TC-SCRIPT-01：创建 request 脚本，改写 header 并在 Traffic Detail 看到修改。
  - TC-SCRIPT-02：response 脚本改状态码，命中规则 Traffic 显示脚本日志。
  - TC-SCRIPT-03：decode 脚本对私有协议 body 正确解析，`build_in_bp` 默认可用。
  - TC-SCRIPT-04：sandbox `allow_network=false` 时 `net.fetch` 拒绝，日志显示原因。
  - TC-SCRIPT-05：sandbox `allow_private_network=false` 时 `net.fetch('http://127.0.0.1')` 拒绝。
  - TC-SCRIPT-06：`allow_private_network=true` 后 BAM mock 可访问，日志显示成功。

### 覆盖率与项目校验

- `cargo test -p bifrost-script`
- `cargo test -p bifrost-admin update_sandbox_config`
- `cargo test -p bifrost-core protocol`
- `pnpm --dir web test tests/ui/admin-scripts.spec.ts`
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 本地遵循 no-local-coverage 规则；coverage 门禁走远端 CI。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核协议解析、脚本引擎线程隔离、超时/内存/网络中止路径。
- 复核 Admin API 拒绝路径（非绝对 `allowed_dirs`、空 `sandbox_dir`、非法 payload）。
- 手工在 Web Scripts 页面创建各类脚本并观察 Traffic Detail 日志。

### 第 2 轮

- 复核 `allow_private_network` opt-in 语义与文档一致，避免默认策略被误改。
- 复核 `build_in_bp` 释放/接管逻辑不会覆盖用户修改。
- 复跑 BP parser E2E 和 Scripts Playwright，确认端到端还原。

## 风险与决策

- **Sandbox 越权风险**：`allow_private_network` 直接放开内网访问，必须默认关闭、UI 上带明显警示，并在 Admin API 层做参数校验。
- **quickjs 内存回收**：每次执行都新建 runtime，成本略高但换来严格隔离；未来若成为热点，再考虑 pool + strict reset。
- **decode 脚本失败降级**：Traffic Detail 展示 raw + 错误提示，避免误导用户以为解析成功。
- **文档偏差**：旧 RFC 里的“完全禁止文件/网络”表述已过时；文档必须描述“默认关闭 + 可显式 opt-in”的当前策略，并给出安全边界。
- **多设备同步**：脚本内容 sync 需要额外的信任模型（谁能改、能否禁用 net），第一版不做，交由手动导入/导出。
