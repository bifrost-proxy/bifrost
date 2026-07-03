# 代理性能压力测试方案

## 背景

Bifrost 是本机代理 + Traffic 录制 + 规则/脚本转发 + macOS 应用识别的组合体，任何一项能力都可能在高压下退化并把其它能力拖下水。本方案用于系统化压测 Bifrost 在高并发、高吞吐、流量录制、复杂转发策略和 macOS 应用识别下的性能边界，目标不是跑出一个峰值 RPS，而是建立可复现、可比较、可定位瓶颈的持续提升体系，并把每一次优化都固化到基线中，避免为了性能牺牲功能。

四条主线：

1. 代理转发性能：HTTP、HTTPS CONNECT、TLS 解包、WebSocket、SSE，不同 body 大小、连接复用与并发模型下的吞吐、延迟、错误率。
2. 流量录制性能：`AsyncTrafficWriter`、`BodyStore`、traffic DB、详情读取、列表刷新与搜索在高写入压力下的吞吐、写入滞后、内存增长和磁盘增长。
3. 转发策略性能：无规则、简单 host 规则、大量规则、脚本、上游代理链组合下的规则匹配与转发开销。
4. macOS 应用识别性能：`ProcessResolver` 的 socket 快照、singleflight、并发阀门与 backfill 在高并发短连接下的准确率、延迟、热路径开销。

## 用户目标验证清单

### 必须实现

- 在同机同参数下可复现的压测矩阵与统一 JSON 报告 schema。
- smoke / baseline / soak / regression 四档分层，PR CI 不承担长压测。
- 每次压测同时采集 loadtest 结果、Admin metrics、进程资源、traffic DB 统计、apps 归因和规则匹配抽样。
- 复用 `scripts/loadtest-proxy-stability.mjs`、`scripts/loadtest-upstream-502-analysis.mjs`、`scripts/loadtest-traffic-storage-100k.mjs`；新脚本进入 `scripts/perf/` 时保持同一 run 目录布局。
- 每轮压测输出 `summary.md`，写清 baseline / bottleneck / next actions。

### 必须不破坏

- 性能优化必须先证明功能等价：HTTP/HTTPS/SOCKS5/WebSocket/SSE、Traffic 完整性、规则语义、TLS 应用策略、macOS 应用归因、Admin/CLI/WebUI 用户可见行为都不允许被削弱。
- 任何异步化 / 采样 / 缓存 / 批处理 / 延迟回填优化必须证明最终一致性：数据可以晚到，但不能永久缺失或错误。
- 任何跳过热路径工作的优化必须列出跳过条件、降级结果、用户可见表现和后续回填/诊断路径。
- 长跑不再使用公网服务作为默认上游；默认走本地 mock upstream。

### 必须真实验证

- 每次 release baseline 都留下 `.artifacts/loadtest/<run-id>/` 目录（run-meta、load、admin-metrics、traffic-samples、app-resolution-samples、logs）。
- 每次 regression 都产出 `compare-runs` 结果，说明 RPS / p95/p99 / RSS / write lag / recognition rate 对比。
- 10 万条流量存储专项每次都验证 `retainedRecords`、`writeRps`、`responseBodySearchP95Ms`。
- macOS 应用识别专项要抽样 `traffic detail`、`metrics apps`、`client-manifest.ndjson` 三向对齐。

## 产品语义

### 持续提升五维

| 维度 | 持续提升方向 | 典型退化信号 |
| --- | --- | --- |
| 性能 | RPS、吞吐、p95/p99、CPU/请求、写放大 | 关闭录制/规则才快，一开真实场景就退化 |
| 稳定性 | soak 下 RSS、FD、连接、DB、后台任务波动 | cooldown 不回落、连接泄漏、DB 写入积压 |
| 可靠性 | 请求错误、traffic 缺失、规则误命中、unknown、5xx | 请求成功但无记录、规则误命中、unknown 飙升 |
| 功能完备性 | 协议 / 录制 / Replay / apps 统计的覆盖 | 只压 HTTP happy path |
| 可运营性 | run 目录、summary、原始采样、可复跑命令、next actions | 只有 console 输出 |

每轮压测结论必须至少回答：当前基线是什么、最大瓶颈是什么、下轮 P0/P1/P2 要提什么、和上一次同机同参 run 的差异。

### 功能不变门禁

| 保护对象 | 不允许的优化方式 | 必须保留的行为 |
| --- | --- | --- |
| 代理协议 | 删除/绕过/弱化 HTTP/HTTPS/SOCKS5/WebSocket/SSE/TLS 解包 | 已支持协议继续可用，错误语义与连接生命周期一致 |
| 流量录制 | 丢 request/response record、body、headers、frames、详情字段 | 请求成功后记录可追踪，允许异步回填但不能永久丢 |
| 规则系统 | 改变优先级、filter、merge、script、proxy chain、TLS routing | 同一规则输入优化前后产生同一转发和改写结果 |
| macOS 应用识别 | 关闭识别、扩大 unknown、跳过应用级 TLS 策略 | 高压下可以降级但必须可解释、可回填 |
| 管理端与 CLI | 删 Metrics/Traffic/Search/Replay/Export/诊断字段 | 用户可见 API/CLI/WebUI 行为兼容 |
| 可靠性边界 | 取消 timeout、backpressure、清理、错误归因、安全限制 | 失败可恢复可诊断，不允许 silent drop/silent corruption |

## 技术细节

### 指标口径

#### 代理转发

| 指标 | 口径 | 采集 | 门禁建议 |
| --- | --- | --- | --- |
| RPS | 完成请求数 / 稳态秒数 | loadtest report | 相对 main 下降不超过 10% |
| 吞吐 | 响应 bytes / 稳态秒数 | loadtest report | 相对 main 下降不超过 10% |
| p50/p95/p99 | 端到端延迟 | loadtest report | p95/p99 不超过基线 1.2x |
| 错误率 | timeout + connection error + 非预期 5xx | loadtest + traffic detail | smoke 为 0，长跑小于 0.1% |
| CPU/RSS | 进程 CPU/RSS | `/_bifrost/api/system/memory`、`/_bifrost/api/metrics` | steady 无线性增长 |
| FD/连接数 | 打开连接和 socket | `lsof`、metrics | cooldown 回落到接近 idle |

#### 流量录制

| 指标 | 口径 | 采集 | 门禁建议 |
| --- | --- | --- | --- |
| 记录完整率 | records / 成功请求 | `/_bifrost/api/traffic` | 默认大于 99.9% |
| 写入滞后 | request 结束到 record 可见 | 轮询 API | p95 小于 2s |
| 详情读取延迟 | `GET /api/traffic/{id}` | 抽样 | p95 小于 200ms |
| DB 增长率 | traffic DB bytes / 请求 | 文件统计 | 按 body 策略符合预期 |
| body cache 增长 | body cache bytes / 大 body 请求 | 文件统计 | 清理触发后可回收 |
| 查询退化 | list/search p95 高记录数下 | Admin API | p95 不超过基线 1.2x |

#### 转发策略

| 指标 | 口径 | 采集 | 门禁建议 |
| --- | --- | --- | --- |
| 规则集加载时间 | 写入到 active 生效 | Admin API + loadtest | 大规则集小于 3s |
| 匹配开销 | 相对 no-rule 延迟差 | loadtest | simple < 5%、large < 20% |
| 候选规则规模 | 命中候选数 / 最终命中 | `matched_rules` | 与规则矩阵预期一致 |
| 策略正确率 | host/path/filter/proxy chain 转发结果 | upstream echo + detail | 100% |
| 脚本规则开销 | reqScript/resScript 与无脚本对比 | loadtest | 单独报告 |

#### macOS 应用识别

| 指标 | 口径 | 采集 | 门禁建议 |
| --- | --- | --- | --- |
| app recognition rate | recognized / eligible | traffic detail + metrics apps | 常见客户端 > 99% |
| app recognition latency | 连接建立到 app info 可见 | detail polling | p95 < 500ms、p99 < 2s |
| resolver overhead | 开启 vs 跳过路径 p95 差 | loadtest 对比 | p95 增量 < 5%、p99 < 10% |
| resolver timeout rate | 超时 or 阀门饱和 / 请求 | logs + metrics | steady < 0.1%，burst 可恢复 |
| snapshot efficiency | 每秒扫描次数 / 请求 | resolver log | 不随请求数线性增长 |
| attribution correctness | 与 ground truth 对齐 | client-manifest + detail | 100% 用例最终正确 |

#### 功能等价

| 指标 | 口径 | 采集 | 门禁 |
| --- | --- | --- | --- |
| behavior parity | 优化前后同一用例输出 | E2E/human_tests/API diff | 100% |
| traffic data integrity | 成功请求可追踪 | Traffic API | 不允许永久缺失 |
| rule semantic parity | 同一规则 profile 命中/改写一致 | upstream echo + detail | 100% |
| app policy parity | macOS 应用识别与策略结果一致 | manifest + Traffic + apps | 100% |
| API/CLI compatibility | 公开 schema 和关键字段 | snapshot/diff | 无破坏性变化 |

## CLI+Web+Admin API 触点

- Admin metrics：`/_bifrost/api/metrics`、`/_bifrost/api/metrics/apps`、`/_bifrost/api/system/memory`
- Traffic：`/_bifrost/api/traffic?limit=100`、`/_bifrost/api/traffic/batch`、`/_bifrost/api/traffic/{id}`
- Performance 配置：`PUT /_bifrost/api/config/performance`（10 万条专项用于设置 `traffic.max_records=100000`）
- 启动约束：所有压测命令必须使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 环境变量：`BIFROST_PROCESS_RESOLUTION_CONCURRENCY`、`BIFROST_BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY`、`BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY`（参见 `design/process-resolution-performance.md`）。

## Sync 边界

压测数据、run artifact、matched_rules 抽样、client-manifest 全部为本机文件，不参与云端 sync。任何压测过程中不得使用生产账号或触发用户级 sync；启动前须显式设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 并使用临时数据目录。

## Phase 1 —— 现有脚本 + 环境校准

复用：

- `scripts/loadtest-proxy-stability.mjs`：HTTP small/large、HTTPS small/large、SSE、fixed-rate/closed-loop、metrics/memory 采样，输出 `.artifacts/loadtest/proxy-stability-<run-id>.json`。
- `scripts/loadtest-upstream-502-analysis.mjs`：特定上游 502 分析，输出 `.artifacts/loadtest/upstream-502-<run-id>.json`。
- `scripts/loadtest-traffic-storage-100k.mjs`：10 万条流量存储专项。

环境校准：同一机器、同一网络、同一 release binary、同一端口范围；记录 `git rev-parse HEAD`、`rustc -Vv`、`node -v`、CPU/内存/macOS 版本/电源状态。

## Phase 2 —— 协议矩阵与录制矩阵

协议矩阵：

| 场景 | 协议 | body | 观测 |
| --- | --- | --- | --- |
| `http-small` | HTTP absolute-form | 1-4 KiB | 纯转发 RPS、延迟 |
| `http-large` | HTTP absolute-form | 1-10 MiB | 吞吐、body cache、RSS |
| `https-connect` | CONNECT passthrough | 1-4 KiB | tunnel 成本、复用 |
| `https-mitm` | TLS 解包 | 1-4 KiB + JSON | 证书、解包、规则匹配 |
| `sse-stream` | SSE | 长连接 + 小 frame | 活跃连接、内存、push |
| `websocket-echo` | WS/WSS | 小 frame + burst | frame 录制、生命周期 |
| `status-mix` | HTTP | 2xx/4xx/5xx | 错误归因 |

阶梯规则：每档 15s warmup + 60s steady + 15s burst + 30s cooldown。饱和判定：错误率 > 0.1%、p99 > 基线 2x、CPU 30s > 90%、RSS 线性增长且 cooldown 不回落、load generator 自身超时。

录制矩阵：metadata-only、small-body、large-body、list-pressure、search-pressure。计算 `record_completeness = matched_records / successful_requests`、`write_lag_ms = first_visible_at - request_finished_at`、`db_bytes_per_request`。

## Phase 3 —— 规则与应用识别专项

规则 profile：`no-rule` / `single-host` / `path-prefix-100` / `mixed-1000` / `proxy-chain` / `script-heavy` / `mitm-routing`。评估 `matcher_latency_overhead`、`matcher_throughput_overhead`、`rule_correctness`、`proxy_chain_error_rate`。

macOS 应用识别场景：`app-curl-steady`、`app-node-burst`、`app-python-many-pids`、`app-browser-connect`、`app-mixed`。使用 `client-manifest.ndjson` 记录 ground truth。评估 `recognized/eligible`、`correctly_attributed/recognized`、`unknown_after_2s/eligible`，并对比“启用识别 vs `/_bifrost` 跳过路径”的 p95/p99 差异。

必须验证：

- 高并发 CONNECT 不导致每请求全量扫描进程表。
- 大量短连接允许先记录后 backfill，但最终 Traffic/Metrics apps 一致。
- `/_bifrost` 请求不触发识别。
- 解析超时降级为 unknown 而非阻断请求。
- 应用级 TLS 白/黑名单在高压下不因 unknown 系统性失效。

## Phase 4 —— 10 万条流量存储专项 + 长跑

执行入口：

```bash
cargo build --release --bin bifrost
BIFROST_BIN=./target/release/bifrost \
BIFROST_PROXY_PORT=19904 \
BIFROST_UPSTREAM_PORT=28084 \
LOADTEST_STORAGE_MAX_RECORDS=100000 \
LOADTEST_STORAGE_RECORDS=100000 \
LOADTEST_STORAGE_WRITE_CONCURRENCY=256 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
node scripts/loadtest-traffic-storage-100k.mjs
```

必须采集：`before/after.performanceConfig.traffic.max_records`、`write.rps/p95/p99/errors`、`analysis.recordCompleteness`、`latestListP95Ms`、`hostFilterP95Ms`、`statusFilterP95Ms`、`batchDetailP95Ms`、`urlSearchP95Ms`、`responseBodySearchP95Ms`、`after.memory.process/traffic_db`。

长跑 soak：30-60 分钟稳态 + burst + cooldown，观察内存、FD、DB、后台任务 cooldown 恢复。

## 工具链 & Run 目录

```text
.artifacts/loadtest/<run-id>/
  run-meta.json
  load.json
  admin-metrics.ndjson
  process-samples.ndjson
  traffic-samples.ndjson
  app-resolution-samples.ndjson
  file-size-samples.ndjson
  rule-samples.ndjson
  client-manifest.ndjson
  bifrost.log
  upstream.log
  summary.md
```

后续 `scripts/perf/` 建议：

```text
scripts/perf/
  proxy-throughput.mjs
  traffic-recording.mjs
  rule-strategy.mjs
  app-resolution.mjs
  compare-runs.mjs
  fixtures/{upstream-http.mjs, upstream-websocket.mjs, client-spawner.mjs, rules/*.bifrost}
```

统一报告 schema：

```json
{
  "schema": "bifrost-perf-report/v1",
  "runId": "2026-06-18T00-00-00.000Z",
  "git": { "commit": "...", "branch": "..." },
  "bifrost": { "mode": "release", "port": 9900, "dataDir": "..." },
  "scenario": { "name": "http-small", "loadModel": "fixed-rate" },
  "load": { "rps": 0, "bytesPerSec": 0, "p50Ms": 0, "p95Ms": 0, "p99Ms": 0, "errorRate": 0 },
  "traffic": { "recordCompleteness": 0, "writeLagP95Ms": 0, "detailP95Ms": 0 },
  "appRecognition": { "rate": 0, "correctness": 0, "unknownAfter2sRate": 0, "backfillP95Ms": 0 },
  "resources": { "rssMaxMiB": 0, "cpuMaxPercent": 0, "fdMax": 0 },
  "rules": { "profile": "no-rule", "expectedHits": 0, "actualHits": 0 },
  "thresholds": { "status": "pass", "failures": [] }
}
```

## 测试方案

### 结论分档

| 结论 | 条件 |
| --- | --- |
| pass | 错误率、吞吐、p95/p99、RSS、FD、record completeness、规则正确率全部在阈值内 |
| investigate | 功能正确但 p95/p99、RSS、DB、write lag、list/search 有单项明显退化 |
| fail | 请求错误 / traffic 缺失 / 规则转发错误 / 崩溃 / cooldown 不恢复 / 资源持续增长 |
| invalid | 数字变好但功能语义、协议能力、数据完整性、规则行为或应用识别被削弱 |

### 已固化的基线（示例）

- `proxy-stability-2026-06-18T16-36-28.116Z.json`：HTTP small/large + HTTPS + SSE，`ok=27854`、`errors=0`、`non2xx=0`、`cooldownWsOpenConnectionCount=0`、RSS 峰值 110.08 MiB（binary fast path monitor 泄漏修复后）。
- `traffic-storage-100k-2026-06-18T18-48-34.439Z.json`：`recordCompleteness=1`、`retainedRecords=100000`、`writeRps=20973.15`、`writeP95Ms=19.2`、response body 搜索 p95 417ms（未优化）。
- `traffic-storage-100k-2026-06-18T20-24-18.611Z.json`：ASCII fast path + BodyStore 预分配 + 缩短锁持有后，response body 搜索 p95 从 417ms 降到 349.88ms，URL 搜索 p95 从 118.72ms 降到 115.32ms，`recordCompleteness=1`。

### 首轮已修复问题

1. **binary fast path 连接监控残留**：`cooldownWsConnectionCount=762 -> 0`；根因 metrics-only forwarding 不关闭 `ConnectionMonitor`。修复：跳过无 payload 价值的 streaming monitor 注册，WebSocket/SSE 语义不变。
2. **10 万写入 async traffic channel 丢记录**：满队列时不再 `try_send` 丢弃，改为等待队列容量的发送任务；批量从 `record=64/update=32` 提升到 `record=512/update=256`；channel 容量提升到 `MAX_TRAFFIC_MAX_RECORDS * 3`。新增单测 `test_full_channel_defers_without_dropping_records_or_updates`（`crates/bifrost-admin/src/async_traffic.rs:440`）。
3. **response body 搜索热路径**：ASCII text 走 byte-level case-insensitive fast path，`BodyStore::load/load_bytes` 按 `BodyRef::File.size` 预分配，读锁离开后再搜索。功能语义仍是大小写不敏感、非 ASCII fallback 到 Unicode 路径。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标是否覆盖代理性能、流量录制、转发策略、macOS 应用识别四条主线。
- review `scripts/loadtest-*.mjs` 启动参数是否含 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 执行 `node --check` 与 rg 静态验收，确认脚本、指标、阈值、报告 schema 可被检索。

### 第 2 轮

- 复查 `design/`、`human_tests/`、索引是否一致。
- 复跑 `loadtest-proxy-stability.mjs` release baseline，采样 cooldown monitor 计数。
- 复跑 `loadtest-traffic-storage-100k.mjs`，采样 responseBodySearchP95Ms 与 retainedRecords。
- 若发现方案缺口，补充后追加新一轮 review。

## 风险与决策

- 性能数字与开发机绝对值不强绑定，门禁优先使用相对退化阈值。
- 10 万条 overflow 场景最终保留约 8.3 万条属于当前 `trigger=max+min(max*15%,2000)`、`target=max*80%` 策略结果，需要在产品/配置文档中显式说明。
- response body 搜索仍是全量扫描；下一轮如要数量级提升需单独设计 body index / cache，不能通过跳过 body 或降低 `max_scan` 换性能。
- 长跑必须使用临时 `BIFROST_DATA_DIR`，禁止在真机主 profile 上运行，避免污染 Traffic DB 和 sync 状态。

## 执行命令

### smoke

```bash
cargo build --bin bifrost
LOADTEST_BASELINE_MS=1000 \
LOADTEST_WARMUP_MS=5000 \
LOADTEST_STEADY_MS=15000 \
LOADTEST_BURST_MS=5000 \
LOADTEST_COOLDOWN_MS=5000 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
node scripts/loadtest-proxy-stability.mjs
```

### release baseline

```bash
cargo build --release --bin bifrost
LOADTEST_MODE=fixed-rate \
LOADTEST_WARMUP_MS=30000 \
LOADTEST_STEADY_MS=180000 \
LOADTEST_BURST_MS=30000 \
LOADTEST_COOLDOWN_MS=60000 \
LOADTEST_PROXY_MAX_SOCKETS=1024 \
BIFROST_BIN=./target/release/bifrost \
BIFROST_DISABLE_TRAY=1 \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
node scripts/loadtest-proxy-stability.mjs
```

### 502 归因

```bash
cargo build --bin bifrost
LOADTEST_START_PROXY=1 \
LOADTEST_TARGET_URL=http://127.0.0.1:18080/small \
LOADTEST_CONCURRENCY=64 \
LOADTEST_DURATION_MS=30000 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
node scripts/loadtest-upstream-502-analysis.mjs
```

## E2E 与 CI 分层

- PR smoke：只跑 30-60 秒轻量场景，验证脚本可运行、报告 schema 完整、启动保护生效。
- nightly：full matrix，保存 `.artifacts/loadtest/*.json`，与 main 最近 7 天中位数比较。
- release candidate：release baseline + soak + regression，对代理核心、traffic recording、rule strategy 分别出报告。
- `cargo test --workspace --all-features` 不承载性能压测；门禁由专用脚本 + 报告比较承担。

## 文档更新要求

- 本文件是性能压测体系入口。
- 每次新增可执行性能脚本必须同步更新工具链、命令、报告 schema 与 `human_tests/proxy-performance-stress-test.md`。
- 若将性能门禁接入 CI，必须同步更新 `scripts/ci/local-ci.sh` 或对应 GitHub Actions 文档并记录门禁阈值来源。
- 与 `design/process-resolution-performance.md` 双向引用，保证 macOS 应用识别专项的功能等价与热路径开销门禁一致。
