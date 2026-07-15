# 进程识别与原生证书缓存性能真实场景测试

## 功能模块说明

验证管理接口 `/_bifrost/...` 在请求入口硬跳过客户端进程识别，进程识别的调用、缓存、扫描次数与扫描耗时只通过专用诊断接口暴露，主 metrics 保持产品指标语义；同时验证原生证书信任库缓存的并发、失效和失败回退行为。

## 前置条件

- 当前仓库已构建 `target/debug/bifrost`。
- 本机可用 `bash`、`curl`、`jq`、`python3`。
- 所有服务使用动态端口和临时 `BIFROST_DATA_DIR`，不修改系统代理，不停止或复用正式 9900 服务。

## 测试用例列表

### TC-PRP-01 管理接口硬跳过进程识别

操作步骤：

1. 运行：

   ```bash
   REQUEST_COUNT=10000 CONCURRENCY=32 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_process_resolution_performance.sh
   ```

2. 比较脚本输出的 `before` 与 `after` 诊断快照。

预期结果：

- 10,000 个 `/_bifrost/api/proxy/address` 请求全部成功。
- `lookup_requests_total` 前后相等。
- `snapshot_refreshes_total` 前后相等。
- 管理请求不会加锁查询 client-process cache，也不会触发同步或后台 socket/PID 扫描。

### TC-PRP-02 专用诊断接口与主 metrics 边界

操作步骤：

1. 观察 TC-PRP-01 脚本对 `/_bifrost/api/diagnostics/process-resolver` 的字段断言。
2. 观察脚本对 `/_bifrost/api/metrics` 的隔离断言。

预期结果：

- 专用诊断接口返回 lookup、正负缓存命中、snapshot 命中/未命中/刷新/失败、扫描总耗时/最大耗时、PID/FD 扫描量和解析成功/失败计数。
- 主 metrics 不包含 `snapshot_refreshes_total`、`scan_duration_total_us`、`scanned_pids_total`、`scanned_fds_total` 等进程识别内部诊断字段。
- 主进程可持有共享原子计数器供专用接口读取，但不把该统计混入产品 metrics 聚合与历史序列。

### TC-PRP-03 普通代理链路回归

操作步骤：

1. 观察 TC-PRP-01 脚本启动的本地 Python upstream。
2. 脚本通过 Bifrost HTTP 代理访问该 upstream。

预期结果：

- 普通代理请求返回成功。
- 非管理请求仍保留原有进程识别路径，管理接口的硬跳过不会改变正常代理转发行为。

### TC-PRP-04 外部 Admin-like path 防误伤回归

操作步骤：

1. 观察 TC-PRP-01 脚本通过 Bifrost 代理请求动态端口 upstream 的 `/_bifrost/api/proxy/address`。
2. 比较该请求前后的进程识别诊断快照。

预期结果：

- upstream 返回其真实 404，而不是 Bifrost 本机 Admin API 的 200。
- `lookup_requests_total` 增加，证明绝对 URI 指向外部 upstream 时没有仅因 path 而误入 `SkipAdmin`。
- 本机 Admin path 与 `bifrost.local` 虚拟主机仍由共享 `AdminRoutingDecision` 判定为 Admin 流量。

### TC-PRP-05 原生证书信任库缓存

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-core native_cert_cache -- --nocapture
   ```

预期结果：

- 并发首次读取只执行一次原生证书加载。
- 显式失效后下一次读取重新加载。
- 已有成功快照时刷新失败继续使用 stale 快照。
- 首次加载失败会缓存空结果，避免短时间重复触发 TrustSettings。

### TC-PRP-06 诊断接口状态与只读语义

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-admin diagnostics -- --nocapture
   ```

预期结果：

- 配置共享诊断对象后接口返回同一组计数快照。
- 未配置诊断对象时返回 503，而不是伪造全零成功响应。
- 写方法被拒绝，专用诊断接口保持只读。

### TC-PRP-07 长期共享 outbound HTTP Client

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-core shared_http_client_reuses_generation_and_rebuilds_after_change -- --nocapture
   ```

2. 检查附件、图片与飞书应用注册请求使用 request-level timeout 和共享 outbound Client。

预期结果：

- 同一 native certificate generation 返回同一个 `Arc<reqwest::Client>`。
- generation 变化后重建 Client，确保新安装或更新的系统 CA 生效。
- Client 复用不读取或修改任何 socket、PID、进程名称或 TLS 应用策略状态。

### TC-PRP-08 连接生命周期共享解析结果

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-proxy connection_process_state_ -- --nocapture
   ```

预期结果：

- 同一连接的 12 个并发调用只执行一个解析 initializer。
- 成功的 PID/name/path 在该连接后续请求中保持稳定。
- unknown 不写入连接级 `OnceLock`，后续 generation 仍可重新识别。
- 连接关闭后状态随连接释放，不把旧端口的进程结果复用到新连接。

### TC-PRP-09 跨连接 snapshot generation 合并与策略准确性

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-proxy concurrent_connections_share_one_snapshot_generation -- --nocapture
   cargo test -p bifrost-proxy test_should_not_intercept_when_app_policy_configured_but_client_app_unknown -- --nocapture
   REQUEST_COUNT=1000 CONCURRENCY=16 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_process_resolution_performance.sh
   ```

2. 检查 E2E 输出的 `burst_lookups`、`burst_snapshot_refreshes` 与 `burst_requests`。

预期结果：

- 16 个不同连接并发 miss 时只执行一次注入 scanner，并共享同一 generation。
- 128 个真实并发代理请求的 snapshot refresh 数小于请求数，证明扫描按 generation 合并。
- TTL、miss interval、应用策略重试次数均未缩短或延长。
- 配置应用策略但客户端进程 unknown 时仍保持既有 passthrough 语义；本优化不把 unknown 错当成任何应用。

### TC-PRP-10 18k 高基数连接缓存硬容量

操作步骤：

1. 运行：

   ```bash
   REQUEST_COUNT=1000 CONCURRENCY=32 CACHE_STRESS_COUNT=18000 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_process_resolution_performance.sh
   ```

2. 检查输出中的 `cache_stress` 与最终诊断快照。

预期结果：

- 18,000 个真实 curl 进程通过隔离 Bifrost 访问本地 Python upstream，全部转发成功。
- `connection_cache_peak_entries` 与 `connection_cache_entries` 都不超过 16,384。
- `connection_cache_evictions_total` 或 `connection_cache_expired_total` 有增长，证明 live 高基数和 TTL 都不会进入全表 `retain`。
- 脚本正常结束且代理保持 ready，不出现死锁、崩溃、FD 泄漏或正式 9900 服务被停止。

### TC-PRP-11 SOCKS 与 tunnel 连接级 Arc/singleflight 回归

操作步骤：

1. 运行：

   ```bash
   cargo test -p bifrost-proxy socks_handler_reuses_one_connection_owned_process_arc -- --nocapture
   cargo test -p bifrost-proxy test_maybe_backfill_joins_existing_connection_resolution -- --nocapture
   ```

2. 观察 TC-PRP-10 E2E 输出中的 `socks_status`。

预期结果：

- 同一 SOCKS 连接的多阶段解析返回 `Arc::ptr_eq=true` 的同一 PID/name/path，不克隆 String 或再次扫描。
- server 已有解析 in-flight 时，CONNECT tunnel backfill 不启动第二个 resolver；前一任务完成后状态可再次获取。
- 真实 SOCKS5 请求成功访问 Python upstream，`socks_status=200`，协议功能没有因连接状态改造回归。

## 清理步骤

- E2E 脚本自动停止自己启动的 Bifrost 与 Python 子进程并删除临时目录。
- 不运行按端口模糊清理正式 9900 服务的命令。

## 执行记录

2026-07-14 本轮执行结果：

- TC-PRP-01：通过。
  - `REQUEST_COUNT=10000 CONCURRENCY=32 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_process_resolution_performance.sh`
  - 10,000 个管理请求完成；前后 `lookup_requests_total=0`、`snapshot_refreshes_total=0`。
- TC-PRP-02：通过。
  - 专用诊断接口返回全部计数与扫描耗时字段。
  - 主 `/api/metrics` 未出现进程识别内部诊断字段。
- TC-PRP-03：通过。
  - 同一 E2E 脚本通过 Bifrost 代理访问动态端口 Python upstream 成功。
- TC-PRP-04：通过。
  - 外部 upstream 的 `/_bifrost/api/proxy/address` 返回 404，且 `lookup_requests_total` 从 0 增加到 1，未被本机 Admin path 规则误伤。
- TC-PRP-05：通过。
  - `cargo test -p bifrost-core native_cert_cache -- --nocapture`
  - 6 个测试通过，覆盖并发 singleflight、失效重载、stale 回退、首次失败缓存、部分成功缓存和公开缓存失效入口。
- TC-PRP-06：通过。
  - `cargo test -p bifrost-admin diagnostics -- --nocapture`
  - 相关测试全部通过，包含共享快照、未配置 503 与写方法拒绝。
- TC-PRP-07：通过。
  - 共享 Client 单测通过；同 generation `Arc::ptr_eq=true`，generation 变化后重建。
  - 图片、附件和飞书应用注册改为共享 Client + request-level timeout。
- TC-PRP-08：通过。
  - 12 个并发调用共享一个 initializer，调用计数为 1。
  - unknown 后再次解析成功，证明失败结果未写入连接级 `OnceLock`。
- TC-PRP-09：通过。
  - 16 个不同连接并发 miss 的注入 scanner 调用计数为 1。
  - `REQUEST_COUNT=10000 CONCURRENCY=32` 真实 E2E 通过；128 个并发普通代理请求产生
    `burst_lookups=335`、`burst_snapshot_refreshes=15`，远低于逐 lookup/逐请求扫描。
  - 第 2 轮连接状态修复后以 `REQUEST_COUNT=1000 CONCURRENCY=16` 复跑通过；128 个并发
    普通代理请求产生 `burst_lookups=302`、`burst_snapshot_refreshes=16`。
  - rebase 最新 `main`、整合共享解析 commit 并收敛 coverage 不可达分支后，以相同 1000 请求参数复跑通过；管理请求
    lookup/refresh 保持 `0 -> 0`，外部 Admin-like path lookup/refresh 均增加，128 个并发普通
    代理请求产生 `burst_lookups=325`、`burst_snapshot_refreshes=16`。
  - app policy + unknown 的 passthrough 回归测试通过；TTL、miss interval 和重试配置未修改。

2026-07-15 本轮 Phase 7 执行结果：

- TC-PRP-10：通过。
  - `REQUEST_COUNT=1000 CONCURRENCY=32 CACHE_STRESS_COUNT=18000 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_process_resolution_performance.sh`
  - 18,000 个普通代理请求全部通过真实 curl 进程、隔离 Bifrost 和 Python upstream；脚本退出码为 0。
  - `cache_stress=requests=18000 evictions=0 expired=6770 peak=11464 current=11360`；压力期间 TTL 已回收 6,770 项，峰值和当前值均低于 16,384 硬容量，没有触发旧 10k 全表扫描悬崖。
  - 128 请求预热 burst 为 `burst_lookups=203`、`burst_snapshot_refreshes=6`，snapshot generation 继续有效合并。
- TC-PRP-11：通过。
  - `cargo test -p bifrost-proxy socks_handler_reuses_one_connection_owned_process_arc -- --nocapture`：1 个测试通过，同连接返回同一 `Arc`。
  - `cargo test -p bifrost-proxy test_maybe_backfill_joins_existing_connection_resolution -- --nocapture`：1 个测试通过，已有 in-flight 时 tunnel 未启动重复解析。
  - 同一真实 E2E 输出 `socks_status=200`，SOCKS5 转发功能正常。
  - 第 1 轮兼容 wrapper 与 Drop guard 修复后，以 `REQUEST_COUNT=1000 CONCURRENCY=16 CACHE_STRESS_COUNT=0` 复跑最新二进制通过：`burst_lookups=249`、`burst_snapshot_refreshes=11`、`socks_status=200`。
  - coverage 门禁补测与 tunnel 回填副作用提取后，再次执行同参数真实 E2E 通过：`burst_lookups=247`、`burst_snapshot_refreshes=10`、`socks_status=200`；管理请求仍保持零进程扫描。
