# 进程解析性能优化

## 背景

代理在本地 CONNECT/HTTP 请求上需要识别客户端进程，用于应用级 TLS 拦截策略、Traffic 归因和管理端 apps 展示。历史实现中，macOS 每次连接 miss 都可能执行一次 `proc_listpids` + `proc_pidinfo(PROC_PIDLISTFDS)` + `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` 的全量扫描；当系统代理接管浏览器/Node/Python 等大量短连接客户端时，这会带来 CPU 抖动、blocking task 排队、管理端 API 延迟拉升，甚至导致 SSE Messages 页详情打不开。

本方案的目标是在保持进程识别语义、TLS 应用策略语义、Traffic 归因语义、Response body 采集语义完全不变的前提下，把 macOS 与非 macOS 的解析路径归一到同一套“短 TTL socket 快照 + singleflight 刷新 + 全局并发阀门 + negative cache”的骨架上，并把高热路径中不必要的 `ConnectionMonitor` 写锁竞争摘出来。

真实实现位置：`crates/bifrost-proxy/src/utils/process_info.rs`、`crates/bifrost-proxy/src/utils/process_info/tests.rs`、`crates/bifrost-proxy/src/utils/tee.rs`、`crates/bifrost-admin/src/async_traffic.rs`。

## 用户目标验证清单

### 必须实现

- macOS 与 Linux 共用同一份 `SocketSnapshot` 结构与刷新语义。
- 高并发 CONNECT/HTTP 下每秒最多一次系统级 socket 扫描（快照 TTL 250ms + singleflight 保护）。
- 快照 miss 且已超过 50ms 时允许受 singleflight 保护地重刷一次，避免 200ms 内结束的短连接一直拿旧快照。
- 异步进程解析全局并发受阀门 `PROCESS_RESOLUTION_SEMAPHORE`（默认 4，`BIFROST_PROCESS_RESOLUTION_CONCURRENCY` 可调）保护，2 秒硬超时后按未知客户端继续处理。
- 后台 backfill 走独立阀门 `BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE`（默认 8，`BIFROST_BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY` 可调），`try_acquire` 抢不到直接放弃 backfill。
- 普通 HTTP 请求进入前也做一次受限、带超时的同步解析，确保 200ms 内结束的短请求也能拿到 `clientApp` 落入 Traffic。
- `/_bifrost` 管理端流量完全跳过进程解析（前台和 backfill 都不触发）。
- 响应 body 保存保持“正常路径同步、`BodyStore` 忙时后台最终保存”语义，后台并发上限由 `BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY`（默认 1）控制，且不允许永久丢 body/records。
- `AsyncTrafficWriter` 支持 `Update(id)` 早于 `Record(id)` 到达时暂存挂起、`Record` 落库后自动 apply。

### 必须不破坏

- TLS app policy 白名单/黑名单在高压下不能系统性失效（只允许短暂 unknown，不允许错误放行/错误拦截）。
- 客户端进程名/path/pid 解析结果与旧实现一致；同一 keep-alive 连接后续请求解析结果稳定。
- Traffic 记录、响应大小、响应体保存不允许因为优化路径而永久丢失。
- WebSocket / 显式 streaming 响应仍在 `ConnectionMonitor` 上记账；普通 HTTP 响应不再抢 monitor 写锁。
- CONNECT/SOCKS5 应用策略与 socks handler 内部 IP+userpass 组合语义保持不变。

### 必须真实验证

- `cargo test -p bifrost-proxy utils::process_info -- --nocapture` 全绿。
- `cargo test -p bifrost-admin async_traffic -- --nocapture` 覆盖 update 早于 record、以及 buffer=1 时 record/update 不丢。
- `cargo test -p bifrost-proxy utils::tee -- --nocapture` 覆盖 body_store 忙时后台保存、普通 HTTP 不写 monitor。
- 真人回归 `human_tests/webui-traffic.md` 的 `TC-WTR-47`（高并发 CONNECT 下 Traffic 页面与 SSE Messages 详情可打开）。

## 产品语义

- “进程解析”是尽力而为的旁路能力：请求成功不依赖解析成功。
- 允许短暂 `clientApp=unknown`，但必须最终一致：后台 backfill 或后续请求应能回填。
- 管理端自访问不应被自身识别拖慢，因此 `/_bifrost` 直接跳过。
- 响应 body 采集绝不能因为性能开关而永久缺失；只在锁竞争严重时才降级为后台最终写入并回填 `response_body_ref`。

## 技术细节

### SocketSnapshot 与 singleflight

`ProcessResolver` 内部维护 `RwLock<Option<SocketSnapshot>>`（`crates/bifrost-proxy/src/utils/process_info.rs:61`）。刷新流程：

1. 首次或 TTL(250ms) 过期时进入 refresh 路径。
2. 大量并发进来时，通过 singleflight 只放一个任务真正执行系统扫描，其余任务复用刷新后的快照。
3. miss 但快照已经 >50ms 时，允许受 singleflight 保护的一次“强制刷新”。
4. 结果同时写入完整 `ConnKey`（包含 `proxy_addr`）与仅 `peer_addr` 的兼容 key，兼容 `resolve_for_connection` / `resolve` 两个入口。

### 并发阀门

- `PROCESS_RESOLUTION_SEMAPHORE`：所有前台异步解析 `spawn_blocking` 都要拿 permit，2 秒 `PROCESS_RESOLUTION_WAIT_TIMEOUT` 总预算内等待 permit + 完成解析；预算耗尽记 warn 后按未知客户端继续，不写连接级 negative cache，避免高并发下误压制后续解析。
- `BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE`：`spawn_async_process_resolver_with_finish` 专用，`try_acquire` 抢不到立即跳过 backfill。
- `BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY`：`tee.rs:52` 读取的后台 body 保存并发上限，默认 1。

### Negative cache

命中近期 miss/timeout 时直接返回未知客户端，不再排队或创建 blocking 任务；解析超时 / 完成 / 跳过 backfill 时都会复位连接级 in-flight 标记，允许同一 keep-alive 连接后续请求再次尝试。

### AsyncTrafficWriter 抗乱序

`crates/bifrost-admin/src/async_traffic.rs` 里 `TrafficCommand::{Record, Update}` 支持先到达的 `Update(id)` 暂存，`Record(id)` 后续入库时会先 apply 挂起 update。相关测试：

- `test_async_traffic_update_before_record_is_applied`（`async_traffic.rs:358`）
- `test_full_channel_defers_without_dropping_records_or_updates`（`async_traffic.rs:440`）

### ConnectionMonitor 写锁降压

普通 HTTP 响应不再无条件写 `ConnectionMonitor`。只有 WebSocket 或已经注册为显式 streaming 的连接才更新 monitor 状态，避免普通短请求在响应结束时抢全局 monitor 写锁。Traffic 记录、响应大小、响应体保存全部保留。

## CLI / Web / Admin API

前序方案不新增用户可见 API 字段，只落地环境变量与内部行为；本次增量新增一个按需读取、无历史和无推送的诊断 API，见文末 Phase 5：

| 入口 | 作用 |
| --- | --- |
| `BIFROST_PROCESS_RESOLUTION_CONCURRENCY` | 前台异步解析并发（默认 4） |
| `BIFROST_BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY` | 后台 backfill 并发（默认 8） |
| `BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY` | body_store 后台保存并发（默认 1） |
| `/_bifrost/*` 管理端路径 | 完全跳过前台与 backfill 进程解析 |
| Traffic detail `client_app` / `client_process` / `client_pid` | 语义保持不变，允许短暂空，后台 backfill 回填 |
| `apps` metrics（`/_bifrost/api/metrics/apps`） | 归因来源与旧实现一致 |

## Sync 边界

进程解析结果不参与云端 sync，不写入用户配置。TLS 应用策略白/黑名单本身仍走 access-control sync 通道，本方案不改变字段。

## Phase 1 —— 快照与阀门归一

- 提取 `SocketSnapshot` 到跨平台位置，删除 macOS 每连接全量扫描分支。
- 引入两个 `LazyLock<Semaphore>`（`process_info.rs:855/867`），2 秒 wait 预算。
- `/_bifrost` 路径显式跳过解析。
- 单测：`tests.rs:79 test_process_resolution_timeout_returns_none_and_negative_caches`、`tests.rs:100 test_async_process_resolution_respects_negative_cache`、`tests.rs:116 test_process_resolution_concurrency_wait_timeout_does_not_negative_cache`、`tests.rs:158 test_conn_key_uses_proxy_addr`。

## Phase 2 —— 短请求同步解析 + 日志降级

- 普通 HTTP 请求进入前跑一次带超时的同步解析，把 clientApp 提前塞入 Traffic 记录，避免 200ms 短请求全部靠 backfill。
- CONNECT/SOCKS5 应用策略解析的逐请求 log 由 info 降级为 debug，避免系统代理高压下日志放大 CPU/IO。
- 若 CONNECT 已同步解析过但仍未命中，不再追加后台 backfill，避免失败路径重复扫描。

## Phase 3 —— body_store 与 monitor 降压

- `tee.rs` 引入后台保存并发阀门，默认 1；`BodyStore` 读锁可立即写时仍走同步路径。
- 普通 HTTP 响应不再无条件写 `ConnectionMonitor`，仅 WebSocket/显式 streaming 保留。
- 单测：`tee.rs:1053` 起的 tokio 用例（body_store 忙时后台等待并最终保存、普通 HTTP 不写 monitor、streaming 保留 monitor）。

## Phase 4 —— async traffic 抗乱序 & 真人回归

- `AsyncTrafficWriter` 允许 update 早于 record；record 落库时 apply pending update。
- `human_tests/webui-traffic.md TC-WTR-47`：临时端口高并发 CONNECT，Traffic 列表可打开、SSE Messages 详情可打开、cooldown 后 `ConnectionMonitor` open 计数回到 0。
- `human_tests/readme.md` 同步 Web UI Traffic 用例数量。

## 测试方案

### 单元测试

- `bifrost-proxy utils::process_info`：
  - `test_format_client_info_with_process` / `_without_process`
  - `test_process_resolver_cache` / `_cached_lookup_miss` / `_retry_caches_miss`
  - `test_process_resolver_async_returns_cached_hit`
  - `test_process_resolution_timeout_returns_none_and_negative_caches`
  - `test_async_process_resolution_respects_negative_cache`
  - `test_process_resolution_concurrency_wait_timeout_does_not_negative_cache`
  - `test_conn_key_uses_proxy_addr`
  - macOS-only：`test_process_resolver_detects_curl_client` / `_node_client` / `_python_client`
- `bifrost-admin async_traffic`：
  - `test_async_traffic_writer`
  - `test_async_traffic_update`
  - `test_async_traffic_update_before_record_is_applied`
  - `test_full_channel_defers_without_dropping_records_or_updates`
- `bifrost-proxy utils::tee`：body_store 忙时后台保存 / 普通 HTTP 不写 monitor / 流式连接继续写 monitor。

### E2E

- `e2e-tests/tests/test_sse_frames.sh`：SSE 捕获与详情读取。
- `scripts/loadtest-proxy-stability.mjs` release baseline 复跑：`ok=27854`、`cooldownWsConnectionCount=0`（对齐 `design/proxy-performance-stress-test.md` 已记录基线）。

### 真实场景

- `human_tests/webui-traffic.md TC-WTR-47`：Chrome/Safari + curl 混合高并发 CONNECT，验证 Traffic 页面和 SSE Messages 详情打开、`clientApp` 最终一致。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核阀门默认值与环境变量能否在同一 crate、同一 CI 环境下正确覆盖。
- review `spawn_async_process_resolver_with_finish` 复位 in-flight 标记的所有分支（miss / done / skip）。
- 复测：process_info + async_traffic + tee 全套单测。

### 第 2 轮

- 复核 `/_bifrost` 跳过路径是否覆盖 push、metrics、traffic、bifrost_file 全部管理端 handler。
- 复核 body_store 后台保存在 cooldown 后 monitor open 数是否为 0。
- 复测：release 二进制 `loadtest-proxy-stability.mjs` 采样 cooldown 状态。

## 风险与决策

- 阀门默认值偏保守：真机上如果 CPU 富余可以调 `BIFROST_PROCESS_RESOLUTION_CONCURRENCY` 到 8/16；但 macOS 上 `spawn_blocking` 会与 tokio worker 争核，默认不放大。
- 快照 TTL=250ms 是折中：过大会让新进程识别延迟拉大，过小会退回全量扫描；已通过短连接实测确认命中率。
- Negative cache 只针对 timeout/miss 语义；不覆盖“并发饱和”场景，避免误压制后续解析。
- Body 后台保存 `BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY=1` 是为了避免与 SQLite WAL 争 IO；大机可上调，但需要重跑 traffic-storage-100k 保证 write lag 不劣化。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 视范围执行 `bash scripts/ci/local-ci.sh --e2e-only rules` 或对应 E2E 分片。

## 文档更新要求

- `human_tests/webui-traffic.md` 保留高并发 CONNECT 真实回归。
- `human_tests/readme.md` 同步 Web UI Traffic 用例数量。
- 本方案与 `design/proxy-performance-stress-test.md` 中“macOS 应用识别怎么测”一节双向引用，保证性能压测框架能验证本次优化的功能等价与热路径开销门禁。

## Phase 5 —— 管理请求硬跳过、按需诊断与原生证书缓存

### 管理请求硬跳过

请求进入 `handle_request` 后先复用现有 Admin 路由判定生成 `AdminRoutingDecision`：

- `SkipAdmin`：请求会被路由到本机 `AdminRouter`，包括本机 Admin path、`bifrost.local` 虚拟主机和 DevTools bridge；
- `SyncAppPolicy`：CONNECT 且存在应用级 TLS include/exclude；
- `Normal`：其他流量。

`AdminRoutingDecision` 由 `is_admin_virtual_host_request` 与 `is_proxy_request_to_other_for_admin_routing` 的既有结果构成，进程识别和后续 Admin 分发共用同一个判定。不能仅按 path 跳过：例如代理到外部 upstream 的绝对 URI `http://example.com/_bifrost/...` 必须继续按普通代理流量识别进程。`SkipAdmin` 必须在读取连接级进程缓存之前返回 `client_process=None`，不得访问正负缓存、socket snapshot，不得创建阻塞任务、后台任务或重试。管理请求仍保留 client IP、client port、Admin 认证和审计信息。

### 诊断数据边界

`bifrost-core::ProcessResolverDiagnostics` 使用 relaxed 原子计数器，由 resolver 持有并在实际 cache/lookup/scan 位置更新。`AdminState` 只保存共享句柄并提供按需快照：

```text
GET /_bifrost/api/diagnostics/process-resolver
```

接口包含 lookup/resolved/unresolved、正负缓存命中、snapshot hit/miss/refresh/failure、累计与最大扫描耗时、扫描 PID/FD 总数。它不进入 `/api/metrics`、metrics history、数据库或 WebSocket 周期推送。已有 `client_process_resolution_failures` 与 `client_process_policy_unknown_decisions` 作为用户可感知的策略降级指标继续保留。

### 原生证书缓存

`bifrost-core::NativeCertCache` 缓存 `Arc<[Vec<u8>]>` DER：

1. TTL 10 分钟，未过期直接克隆 Arc；
2. miss/过期通过 refresh mutex + double-check 保证并发只加载一次；
3. 部分成功时发布可用证书并记录 warning；
4. 刷新完全失败时保留最后一份可用快照；首次失败缓存空结果至 TTL，避免每次 client build 重复读取 TrustSettings；
5. Admin API 成功安装本地 CA 后显式失效；外部 Keychain 变更通过 TTL 最终收敛。

reqwest 保留 WebPKI roots，关闭 reqwest 内建 native roots 自动加载，再添加缓存 DER。HTTP Replay 和 WebSocket Replay 使用同一 DER 快照构建 rustls `RootCertStore`；unsafe SSL 分支不读取缓存。

### Phase 5 验证

- 单元测试：Admin 路由决策、外部 Admin-like path 防误伤、诊断计数、证书缓存并发/失效/失败回退；
- E2E：隔离端口大量访问管理 API，断言 `snapshot_refreshes_total` 增量为 0；代理到外部 upstream 的 `/_bifrost/...` 必须正常转发并增加 lookup；普通代理请求仍可转发；
- human test：隔离实例执行 10,000 次管理请求，核对诊断快照和扫描增量；
- 远端 CI：workspace、E2E、changed-lines 与 `coverage-all.sh --json --gate` 全绿。
