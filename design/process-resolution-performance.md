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

## Phase 6 —— 准确性无损的 Client 与扫描合并

本阶段只消除重复工作，不缩短应用策略进程识别的等待窗口、不延长策略关键路径的
negative cache，也不复用跨连接的最终进程结论。TLS 应用 include/exclude 的 PID、名称、
路径和 unknown passthrough 语义保持不变。

### 长期共享 outbound HTTP Client

`bifrost-core` 为通用 outbound 请求提供 generation-aware 共享 Client：

1. Client 与 native certificate snapshot generation 绑定；同一 generation 返回同一个
   `Arc<reqwest::Client>`，复用连接池和已经解析的证书；
2. native certificate TTL 到期或安装本地 CA 触发 invalidation 后，下一次获取 Client
   会基于新 generation 原子重建；
3. 仍需要独立 proxy、unsafe SSL、redirect 或超时策略的 Replay/下载器保留专用 Builder；
   普通请求改用 request-level timeout，不为每个附件或轮询重新构建 Client。

### 连接生命周期解析状态

每个 accept 后的 TCP 连接持有一个 `ConnectionProcessState`：

- `OnceLock<ClientProcess>` 只缓存成功结果，因此同一连接的 CONNECT、Traffic、TLS policy
  和 keep-alive 请求共享完全相同的 PID/name/path；
- 前台与后台解析共用连接级 CAS in-flight 状态和 `Notify`，等待同一个解析 future，避免
  同一连接重复启动系统扫描；前台 future 被取消时由 RAII guard 释放状态并唤醒等待者；
- unknown 不写入 `OnceLock`，后续请求仍可等待新的 socket snapshot generation
  再尝试；
- 后台 backfill 成功时也写入同一个 cell，连接关闭后 cell 随连接一起释放，避免端口复用
  导致跨连接误归因。

### Snapshot generation 协调器

系统 socket 扫描由 `SnapshotRefreshCoordinator { generation, refreshing } + Condvar` 协调：

1. 一个请求成为当前 generation 的 refresh leader，并在锁外执行系统扫描；
2. 其他连接等待 generation 变化，不再排队后各自扫描；
3. leader 发布完整 snapshot、递增 generation 并唤醒所有等待者；
4. 等待者只消费刚发布的 generation；若仍 miss，沿用原有 retry delay 等待后续 generation；
5. TTL、miss refresh interval、最大重试次数和 unknown TLS 决策本阶段不变。

### Phase 6 验证

- 单元测试：同 generation 共享 HTTP Client、证书 generation 变化后重建 Client；
- 单元测试：同连接并发解析只执行一个 initializer、成功结果稳定、unknown 不缓存；
- 单元测试：多连接并发 snapshot miss 只执行一次 scanner，全部读取同一 generation；
- E2E/human test：高并发 CONNECT 下扫描次数受控，同时 Chrome/Edge include 仍解包、
  exclude 仍透传、unknown 仍按既有安全语义透传；
- CI：changed-lines 95% 与 `coverage-all.sh --json --gate` 90% 门禁保持全绿。

## Phase 7 —— 消除 10k cache 悬崖与跨协议连接级归因

### 问题与目标

旧连接缓存超过 10,000 项后，每次插入都会在独占写锁内对整张 `HashMap` 执行
`retain`。如果 30 秒 positive TTL 内条目都仍有效，清理不会删除任何项目，随后每个新连接
都重复执行 O(N) 扫描。这是确定性的吞吐悬崖，也会放大 Tokio blocking pool 等待和尾延迟。

本阶段必须同时满足：

- 新连接插入的均摊复杂度为 O(1)，不再存在阈值触发的全表扫描；
- 内存有硬上限，过期项和高基数 live 项都有确定性淘汰路径；
- HTTP、SOCKS、SOCKS TLS intercepted request 与 CONNECT tunnel backfill 共享一个连接级
  解析结果和一个 in-flight 状态；
- 新连接不复用旧连接的 positive 归因，避免本地临时端口快速复用时把旧应用归给新应用；
- 公开同步/异步 resolver API 仍返回 owned `ClientProcess`，不破坏调用方源码兼容性；
- app policy 的 retry、500ms negative TTL 与 unknown passthrough 安全语义不改变。

### L0：连接拥有的 `Arc<ClientProcess>`

`ConnectionProcessState` 下沉到 `utils::process_info`，作为 HTTP 与 SOCKS 的共同连接状态：

```text
accepted TCP connection
  -> OnceLock<Arc<ClientProcess>>
  -> AtomicBool resolution_in_flight
  -> Notify resolution_finished
```

成功结果只构造一次，后续 request、Traffic、TLS policy、SOCKS relay 和后台 backfill 只克隆
`Arc`。miss 不写入 `OnceLock`，OS socket table 尚未发布连接时仍可由下一 generation 重试。
RAII guard 在 future 取消、panic unwind 或普通返回时释放 in-flight 并唤醒等待者。

SOCKS 的状态由 `SocksHandler` 从 accept 持有到连接关闭；TLS interception 内部 keep-alive
请求通过 `service_fn` 克隆同一个状态。HTTP CONNECT 把相同状态传入 tunnel，tunnel 只有成功
取得该状态的 CAS 后才能启动 backfill，因此不会和 server 层后台任务重复扫描。

### L1：有界分片 TTL cache

跨连接兼容缓存实现为 `BoundedTtlCache<K, V>`：

- 32 个 `std::sync::RwLock` shard，读请求只持有目标 shard 的共享锁；
- 连接缓存硬容量 16,384，PID metadata 缓存硬容量 2,048；容量按 shard 确定性分配，所有
  shard 容量之和等于全局硬上限；
- 每个 entry 带单调 generation；替换 key 时旧 marker 自动失效，无需搜索或删除队列中间项；
- `BinaryHeap<Reverse<expiry marker>>` 同时承担 TTL 清理和超容量淘汰；短 negative TTL 会先于
  30 秒 positive TTL 淘汰；
- insert 仅弹出已经过期或为本次超容量所需的 marker，均摊 O(log shard_capacity)，不做
  O(N) `retain`；get 为一次 hash + shard read lock + `Arc` clone；
- `entry_count`、eviction、expiry 使用 relaxed atomic，诊断读取不需要遍历 shard。

这里选择 TTL-aware eviction 而不是通用 LRU：连接 tuple 基数高且绝大多数只访问一次，LRU
每次 hit 写 recency 会制造新的锁竞争；连接级 L0 已承接真正有复用价值的热数据。

### 跨连接 positive cache 的兼容边界

旧公开 resolver API 继续读取 positive cache 并返回 owned clone，避免一次性破坏外部调用方。
代理内部的新连接链路只读取短 negative cache，明确忽略跨连接 positive：第一次归因必须使用
当前 socket snapshot/PID 结果，成功后只写入本连接 L0。这样兼顾兼容层性能与代理内部端口复用
准确性；后续可在公开 API deprecation 周期结束后删除跨连接 positive。

### 诊断与门禁

专用诊断接口新增：

- `connection_cache_entries` / `connection_cache_peak_entries`；
- `connection_cache_evictions_total` / `connection_cache_expired_total`；
- `pid_cache_entries` / `pid_cache_peak_entries`；
- `pid_cache_evictions_total` / `pid_cache_expired_total`。

主 metrics 边界保持不变。真实压力测试在隔离实例创建超过 16,384 个普通代理连接，必须断言
当前值和峰值不超过硬容量，且 eviction 或 TTL expiry 有增长；同时确认 upstream 成功、管理
请求仍完全跳过归因、snapshot refresh 数显著小于 lookup 数。

### 风险控制与回滚

- 功能风险：只改变缓存所有权和复用边界，不改变 PID/name/path 生成、规则匹配或 TLS decision；
- 稳定性风险：没有常驻清理 task，也没有跨 await 持有 shard lock；锁 poison 时恢复 inner state；
- 内存风险：entry 数有硬上限，heap 中替换产生的 stale marker 最迟在其 TTL 到期时被弹出；
- 准确性风险：内部连接忽略 positive 只会增加一次当前连接 lookup，不会把 known 降级为旧应用；
  negative 仍仅保留 500ms，保持既有短时防抖；
- 回滚边界：连接状态、内部 cache policy 和有界容器均为独立层，可单独回滚；公开 API 与 JSON
  诊断字段只做向后兼容的新增。

### Phase 7 验证

- 单元测试：10,000/16,000 项 live insert 硬容量、并发插入、TTL、replacement stale marker、
  positive 兼容与连接级忽略、negative 防抖；
- 单元测试：HTTP connection singleflight、SOCKS `Arc::ptr_eq`、tunnel 已有 in-flight 时不再
  启动第二个 backfill；
- E2E：隔离 Bifrost + Python upstream，普通代理高基数连接压力、容量/eviction/expiry 诊断、
  转发成功与 snapshot generation 合并；
- human test：以 `CACHE_STRESS_COUNT=18000` 执行真实进程、真实 socket table、真实 Admin API
  链路，并保留诊断快照；
- CI：E2E shell、workspace all-features、changed-lines 与 `coverage-all.sh --json --gate` 90%
  门禁全部通过。
