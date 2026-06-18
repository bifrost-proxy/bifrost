# 代理性能压力测试方案真实场景测试

## 功能模块说明

验证 Bifrost 代理性能压测方案是否可执行、可复现、可防止误伤本机环境，并覆盖代理转发性能、流量录制性能和转发策略性能三类目标。

## 前置条件

- 在仓库根目录执行。
- 已安装 Node.js 和 Rust toolchain。
- 本用例的默认执行不修改系统代理；所有启动 Bifrost 的脚本必须显式包含 `--no-system-proxy`。
- 若执行真实短跑，必须使用临时数据目录，并设置：
  ```bash
  BIFROST_DISABLE_TRAY=1
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  ```

## 测试用例列表

### TC-PPS-01：方案覆盖三类性能目标

**操作步骤**：

1. 执行静态检查：
   ```bash
   rg -n "代理转发性能|流量录制性能|转发策略性能" design/proxy-performance-stress-test.md
   ```
2. 确认三类目标都存在。

**预期结果**：

- 输出同时包含 `代理转发性能`、`流量录制性能` 和 `转发策略性能`。
- 方案不是单一吞吐测试，必须覆盖 traffic recording 与 rule strategy。

### TC-PPS-02：压测指标与阈值口径完整

**操作步骤**：

1. 执行静态检查：
   ```bash
   rg -n "RPS|吞吐|p50|p95|p99|错误率|记录完整率|写入滞后|规则集加载时间|转发匹配开销|相对 main 基线" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 输出包含代理、traffic 和 rules 三类指标。
- 输出包含相对基线退化门禁，避免把开发机绝对性能数字写死。

### TC-PPS-03：启动保护不修改系统代理且禁用托盘和自动登录弹窗

**操作步骤**：

1. 检查现有压测脚本启动参数：
   ```bash
   rg -n -- "BIFROST_BIN|--no-system-proxy|BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT" scripts/loadtest-proxy-stability.mjs scripts/loadtest-upstream-502-analysis.mjs
   ```
2. 检查方案文档中的执行命令：
   ```bash
   rg -n -- "--no-system-proxy|BIFROST_DISABLE_TRAY=1|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1|BIFROST_DATA_DIR" design/proxy-performance-stress-test.md human_tests/proxy-performance-stress-test.md
   ```

**预期结果**：

- 两个现有 loadtest 脚本都支持 `BIFROST_BIN`，并包含 `--no-system-proxy`。
- 两个现有 loadtest 脚本启动 Bifrost 时都设置 `BIFROST_DISABLE_TRAY=1` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 方案和用例文档都明确临时数据目录与启动保护要求。

### TC-PPS-04：现有压测脚本语法可解析

**操作步骤**：

1. 执行 Node.js 语法检查：
   ```bash
   node --check scripts/loadtest-proxy-stability.mjs
   node --check scripts/loadtest-upstream-502-analysis.mjs
   ```

**预期结果**：

- 两个脚本均通过 `node --check`。
- 启动保护改动没有引入 JS 语法错误。

### TC-PPS-05：测试方法、采集、评估和分析路径完整

**操作步骤**：

1. 检查方案是否包含关键方法论：
   ```bash
   rg -n "测试方法|代理性能怎么测|流量录制性能怎么测|转发策略性能怎么测|采集器职责|评估方法|分析方法|输出分析报告" design/proxy-performance-stress-test.md
   ```
2. 检查关键计算公式：
   ```bash
   rg -n "record_completeness|write_lag_ms|matcher_latency_overhead|matcher_throughput_overhead|db_bytes_per_request|RSS steady 斜率|summary.md" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 方案明确说明三类性能分别如何测试。
- 方案明确说明如何采集 load、Admin metrics、process、traffic、files 和 logs。
- 方案明确说明 pass/investigate/fail 评估标准，以及性能异常的分析顺序。

### TC-PPS-06：方案索引和后续脚本结构可检索

**操作步骤**：

1. 检查 human_tests 索引：
   ```bash
   rg -n "proxy-performance-stress-test.md|代理性能压力测试方案" human_tests/readme.md
   ```
2. 检查后续脚本结构与报告 schema：
   ```bash
   rg -n "scripts/perf|proxy-throughput|traffic-recording|rule-strategy|bifrost-perf-report/v1" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- `human_tests/readme.md` 包含本文件索引行。
- 设计文档包含后续 `scripts/perf/` 结构和统一报告 schema，方便后续实现压测工具。

### TC-PPS-07：macOS 应用识别与端口归因纳入压力测试

**操作步骤**：

1. 检查 macOS 应用识别指标和测试方法：
   ```bash
   rg -n "macOS 应用识别指标|macOS 应用识别怎么测|app recognition rate|resolver overhead|unknown_after_2s|client-manifest|metrics/apps" design/proxy-performance-stress-test.md
   ```
2. 检查后续脚本结构和报告输出：
   ```bash
   rg -n "app-resolution.mjs|client-spawner.mjs|app-resolution-samples.ndjson|appRecognition|macOS App And Port Recognition" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 方案明确把 macOS 端口到进程、进程到应用、应用到 Metrics apps/Traffic 的识别链路纳入压测。
- 方案明确评估识别率、正确率、unknown 比例、回填延迟、resolver 开销和系统调用放大。
- 方案明确应用识别异常时的分析路径，包括 snapshot、negative cache、backfill、blocking resolver 和管理端跳过识别。

### TC-PPS-08：行业顶级表述是持续提升框架而非一次性硬目标

**操作步骤**：

1. 检查持续提升表述：
   ```bash
   rg -n "长期方向|不是一次压测必须达到的固定数值|持续提升框架|当前基线是什么|最大瓶颈是什么|下轮要提升什么|基线如何变化" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 方案将“行业顶级代理服务”表述为长期方向和持续提升框架。
- 方案要求每轮压测输出当前基线、最大瓶颈、下轮改进项和基线变化，而不是把行业顶级写成单次硬性目标。

### TC-PPS-09：持续提升必须以功能语义不变为前提

**操作步骤**：

1. 检查功能不变门禁：
   ```bash
   rg -n "功能不变门禁|不改变软件既有功能|功能等价验证|behavior parity|rule semantic parity|app policy parity|invalid" design/proxy-performance-stress-test.md
   ```
2. 检查持续提升闭环：
   ```bash
   rg -n "持续提升闭环|Evaluate|Diagnose|Optimize|Verify Function|Verify Performance|Ratchet|baseline run id|after run id" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 方案明确性能优化不能改变既有功能、语义、协议能力和用户可见行为。
- 方案明确每轮优化必须先功能等价验证，再进行性能结果对比。
- 方案明确如果性能数字变好但功能被削弱，则该轮结果为 invalid，不能进入提升基线。

### TC-PPS-10：压测脚本必须避免误连既有代理实例

**操作步骤**：

1. 检查脚本在启动自有 Bifrost 前会检测端口可用性：
   ```bash
   rg -n "assertTcpPortFree|TCP port .* is not available" scripts/loadtest-proxy-stability.mjs scripts/loadtest-upstream-502-analysis.mjs
   ```
2. 检查脚本在等待 ready 时会识别子进程提前退出：
   ```bash
   rg -n "Bifrost exited before readiness check passed|assertProxyOwnedByChild|listener pids|waitForProxyReady\\(bifrost" scripts/loadtest-proxy-stability.mjs scripts/loadtest-upstream-502-analysis.mjs
   ```

**预期结果**：

- 如果目标代理端口已被既有 Bifrost 或其他进程占用，压测脚本必须 fail-fast，而不是误连已有服务。
- 如果本次启动的 Bifrost 子进程在 ready 前退出，压测脚本必须报错并停止本轮结果入库。

### TC-PPS-11：binary fast path 大响应不应残留 open connection monitor

**操作步骤**：

1. 构建 release 二进制：
   ```bash
   cargo build --release --bin bifrost
   ```
2. 在独立端口执行 fixed-rate 压测，包含 HTTP large chunked 响应和 70 秒 cooldown：
   ```bash
   LOADTEST_MODE=fixed-rate \
   LOADTEST_BASELINE_MS=1000 \
   LOADTEST_WARMUP_MS=3000 \
   LOADTEST_STEADY_MS=10000 \
   LOADTEST_BURST_MS=3000 \
   LOADTEST_COOLDOWN_MS=70000 \
   LOADTEST_PROXY_MAX_SOCKETS=1024 \
   BIFROST_BIN=./target/release/bifrost \
   BIFROST_PROXY_PORT=19903 \
   BIFROST_UPSTREAM_PORT=28083 \
   BIFROST_UPSTREAM_HTTPS_PORT=28446 \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   node scripts/loadtest-proxy-stability.mjs
   ```
3. 读取报告中的 `analysis.cooldownWsConnectionCount`、`analysis.cooldownWsOpenConnectionCount`、`analysis.cooldownWsClosedConnectionCount` 和各 phase 的 `errors/non2xx`。

**预期结果**：

- HTTP large 请求完成后，binary performance fast path 不创建无帧、无详情价值的 `ConnectionMonitor` 项。
- cooldown 结束时 `cooldownWsConnectionCount=0`、`cooldownWsOpenConnectionCount=0`、`cooldownWsClosedConnectionCount=0`。
- 所有 HTTP/HTTPS/SSE 子场景 `errors=0`、`non2xx=0`，说明优化没有削弱转发与采集功能。

### TC-PPS-12：10 万条流量存储压测脚本语法与启动保护

**操作步骤**：

1. 执行 Node.js 语法检查：
   ```bash
   node --check scripts/loadtest-traffic-storage-100k.mjs
   ```
2. 检查脚本启动保护：
   ```bash
   rg -n -- "LOADTEST_STORAGE_MAX_RECORDS|100000|--no-system-proxy|BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT|assertTcpPortFree|assertProxyOwnedByChild|childExited" scripts/loadtest-traffic-storage-100k.mjs
   ```
3. 检查方案文档记录专项方法：
   ```bash
   rg -n "10 万条流量存储专项|LOADTEST_STORAGE_RECORDS=100000|traffic.max_records|urlSearchP95Ms|responseBodySearchP95Ms|LOADTEST_STORAGE_OVERFLOW_RECORDS" design/proxy-performance-stress-test.md
   ```

**预期结果**：

- 脚本通过 `node --check`。
- 脚本默认使用独立端口和临时数据目录，启动 Bifrost 时包含 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 脚本启动前检查代理端口和 upstream 端口占用，ready 后校验监听 PID 属于本次启动的 Bifrost 子进程；清理时按子进程退出状态判断，必要时升级 SIGKILL。
- 方案文档明确 `traffic.max_records=100000`、写入、读列表、批量详情、URL 搜索、response body 搜索和 overflow 极限行为。

### TC-PPS-13：真实执行 10 万条写入、读取和搜索压测

**操作步骤**：

1. 构建 release 二进制：
   ```bash
   cargo build --release --bin bifrost
   ```
2. 真实执行 10 万条压测：
   ```bash
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
3. 读取脚本输出的 `report:` JSON 文件，检查：
   ```bash
   node -e 'const fs=require("fs"); const p=process.argv[1]; const r=JSON.parse(fs.readFileSync(p,"utf8")); console.log(JSON.stringify(r.analysis,null,2));' <report-json>
   ```

**预期结果**：

- `analysis.configuredMaxRecords=100000`。
- `write.errors=0`、`write.non2xx=0`。
- 无 overflow 时 `analysis.retainedRecords` 接近 `100000`，`analysis.recordCompleteness` 接近 `1`。
- 报告包含 `latestListP95Ms`、`hostFilterP95Ms`、`statusFilterP95Ms`、`batchDetailP95Ms`、`urlSearchP95Ms`、`responseBodySearchP95Ms`。
- 如果任何读写或搜索 p95 明显异常，必须继续查看报告中的 `searchSamples.totalSearched`、Bifrost 日志、Traffic DB stats 和进程资源，不能只给“压测完成”。

### TC-PPS-14：10 万条存储专项索引与报告结构

**操作步骤**：

1. 检查 human_tests 索引：
   ```bash
   rg -n "loadtest-traffic-storage-100k|10 万条|读写、列表、批量详情、URL 搜索、response body 搜索" human_tests/readme.md human_tests/proxy-performance-stress-test.md
   ```
2. 检查脚本报告 schema 和关键字段：
   ```bash
   rg -n "bifrost-traffic-storage-loadtest/v1|readSearch|recordCompleteness|retainedRecords|urlSearchP95Ms|responseBodySearchP95Ms" scripts/loadtest-traffic-storage-100k.mjs
   ```

**预期结果**：

- `human_tests/readme.md` 已同步更新本模块用例数量和说明。
- 脚本报告结构稳定，后续可以按 `schema=bifrost-traffic-storage-loadtest/v1` 横向比较不同 run。
- 报告字段足够支撑“如何测试、如何评估、如何采集、如何分析”的复盘要求。

## 清理步骤

1. 本用例默认只执行静态检查和 `node --check`，不会启动服务。
2. 如手工执行了 smoke 压测，可清理：
   ```bash
   rm -rf .artifacts/loadtest .bifrost-stability .bifrost-loadtest-502 .bifrost-storage-100k
   ```

## 执行记录

- 2026-06-18：通过。按 TC-PPS-01 至 TC-PPS-05 逐条执行：
  - TC-PPS-01：`rg -n "代理转发性能|流量录制性能|转发策略性能" design/proxy-performance-stress-test.md` 成功，三类目标均可检索。
  - TC-PPS-02：`rg -n "RPS|吞吐|p50|p95|p99|错误率|记录完整率|写入滞后|规则集加载时间|转发匹配开销|相对 main 基线" design/proxy-performance-stress-test.md` 成功，指标与相对基线门禁均存在。
  - TC-PPS-03：`rg -n -- "BIFROST_BIN|--no-system-proxy|BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT" scripts/loadtest-proxy-stability.mjs scripts/loadtest-upstream-502-analysis.mjs` 和方案文档启动保护检查均成功。
  - TC-PPS-04：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过。
  - TC-PPS-05：`human_tests/readme.md` 索引与 `scripts/perf/` 后续结构、`bifrost-perf-report/v1` schema 均可检索。后续已拆分为 TC-PPS-06。
- 2026-06-18：通过。按 review 反馈补充“如何测试、如何评估、如何采集、如何分析”后，执行新增 TC-PPS-05 与拆分后的 TC-PPS-06：
  - TC-PPS-05：`rg -n "测试方法|代理性能怎么测|流量录制性能怎么测|转发策略性能怎么测|采集器职责|评估方法|分析方法|输出分析报告" design/proxy-performance-stress-test.md` 成功；`record_completeness`、`write_lag_ms`、`matcher_latency_overhead`、`matcher_throughput_overhead`、`db_bytes_per_request`、`RSS steady 斜率` 和 `summary.md` 均可检索。
  - TC-PPS-06：`human_tests/readme.md` 索引、`scripts/perf/` 后续结构与 `bifrost-perf-report/v1` schema 均可检索。
  - 回归检查：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过。
- 2026-06-19：通过。按 review 反馈补充 macOS 应用识别、端口到应用归因和“行业顶级”目标后，执行 TC-PPS-07：
  - TC-PPS-07：`rg -n "macOS 应用识别指标|macOS 应用识别怎么测|app recognition rate|resolver overhead|unknown_after_2s|client-manifest|metrics/apps" design/proxy-performance-stress-test.md` 成功。
  - TC-PPS-07：`rg -n "app-resolution.mjs|client-spawner.mjs|app-resolution-samples.ndjson|appRecognition|macOS App And Port Recognition" design/proxy-performance-stress-test.md` 成功。
  - 回归检查：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过。
- 2026-06-19：通过。按 review 反馈将“行业顶级”调整为持续提升框架后，执行 TC-PPS-08：
  - TC-PPS-08：`rg -n "长期方向|不是一次压测必须达到的固定数值|持续提升框架|当前基线是什么|最大瓶颈是什么|下轮要提升什么|基线如何变化" design/proxy-performance-stress-test.md` 成功。
  - 回归检查：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过。
- 2026-06-19：通过。按 review 反馈补充“持续提升必须不改变软件功能和能力”后，执行 TC-PPS-09：
  - TC-PPS-09：`rg -n "功能不变门禁|不改变软件既有功能|功能等价验证|behavior parity|rule semantic parity|app policy parity|invalid" design/proxy-performance-stress-test.md` 成功。
  - TC-PPS-09：`rg -n "持续提升闭环|Evaluate|Diagnose|Optimize|Verify Function|Verify Performance|Ratchet|baseline run id|after run id" design/proxy-performance-stress-test.md` 成功。
  - 回归检查：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过。
- 2026-06-19：通过。执行 TC-PPS-10 与 TC-PPS-11，验证压测隔离和首轮性能优化：
  - TC-PPS-10：`node --check scripts/loadtest-proxy-stability.mjs` 与 `node --check scripts/loadtest-upstream-502-analysis.mjs` 均通过；脚本包含 `assertTcpPortFree`、`lsof` 端口占用检测、`assertProxyOwnedByChild` 和 ready 前子进程退出保护。
  - TC-PPS-10：使用 `BIFROST_PROXY_PORT=9900` 对已被既有 Bifrost 占用的端口做负向验证，脚本 fail-fast 报出 `TCP port 127.0.0.1:9900 is not available`，避免误连既有代理实例。
  - TC-PPS-11 优化前基线：`proxy-stability-2026-06-18T16-30-18.900Z.json`，`ok=27854`、`errors=0`、`non2xx=0`、HTTP large `ok=762`、`cooldownWsConnectionCount=762`、`cooldownWsOpenConnectionCount=762`、`peakRssMiB=161.2`。
  - TC-PPS-11 优化后复测：`proxy-stability-2026-06-18T16-36-28.116Z.json`，`ok=27854`、`errors=0`、`non2xx=0`、HTTP large `ok=762`、`cooldownWsConnectionCount=0`、`cooldownWsOpenConnectionCount=0`、`peakRssMiB=110.08`。
  - 结论：binary fast path 不再留下无帧 open monitor；在请求规模完全一致且无转发错误的前提下，cooldown monitor 残留从 762 降为 0，峰值 RSS 下降约 51 MiB。
- 2026-06-19：通过。执行 TC-PPS-12 至 TC-PPS-14，并真实跑 10 万条存储读写和搜索压测：
  - TC-PPS-12：`node --check scripts/loadtest-traffic-storage-100k.mjs` 通过；`rg -n -- "LOADTEST_STORAGE_MAX_RECORDS|100000|--no-system-proxy|BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT|assertTcpPortFree|assertProxyOwnedByChild|childExited" scripts/loadtest-traffic-storage-100k.mjs` 成功；方案文档中的 `10 万条流量存储专项`、`LOADTEST_STORAGE_RECORDS=100000`、`traffic.max_records`、`urlSearchP95Ms`、`responseBodySearchP95Ms` 和 `LOADTEST_STORAGE_OVERFLOW_RECORDS` 均可检索。
  - TC-PPS-13 首轮失败复现：报告未生成，临时数据目录 `.bifrost-storage-100k/2026-06-18T18-38-15.200Z` 显示压测客户端完成 100000 个 2xx 请求后，Traffic API 和 SQLite 仅有 `13623` 条记录；`body_cache` 有 `100000` 个文件、约 `391 MiB`；日志存在大量 `Traffic channel full, dropping record/update`。该轮用于确认真实性能问题，不能作为通过基线。
  - TC-PPS-13 修复后 10 万条复测：`traffic-storage-100k-2026-06-18T18-48-34.439Z.json`，`recordCompleteness=1`、`retainedRecords=100000`、`configuredMaxRecords=100000`、`writeRps=20973.15`、`writeP95Ms=19.2`、`latestListP95Ms=0.76`、`hostFilterP95Ms=17.21`、`statusFilterP95Ms=2.68`、`batchDetailP95Ms=16.42`、`urlSearchP95Ms=118.72`、`responseBodySearchP95Ms=417.09`。
  - TC-PPS-13 overflow 复测：修正脚本等待 `server_sequence>=105001` 后执行 `LOADTEST_STORAGE_OVERFLOW_RECORDS=5000`，报告 `traffic-storage-100k-2026-06-18T18-51-02.051Z.json`，`retainedRecords=82977`、`writeRps=20908`、`writeP95Ms=19.35`、`latestListP95Ms=0.59`、`hostFilterP95Ms=8.89`、`statusFilterP95Ms=1.6`、`batchDetailP95Ms=13.7`、`urlSearchP95Ms=116.57`、`responseBodySearchP95Ms=372.77`；最终保留量符合当前 cleanup 目标水位策略。
  - TC-PPS-14：`human_tests/readme.md` 已更新本模块为 14 个用例；`scripts/loadtest-traffic-storage-100k.mjs` 包含 `bifrost-traffic-storage-loadtest/v1`、`readSearch`、`recordCompleteness`、`retainedRecords`、`urlSearchP95Ms` 和 `responseBodySearchP95Ms`；脚本显式设置搜索 `scope.all=false`，避免把 URL 搜索误测为全字段搜索。
