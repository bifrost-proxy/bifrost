# 代理性能压力测试方案

## 功能模块说明

本方案用于系统化压测 Bifrost 代理在高并发、高吞吐、流量录制、复杂转发策略和 macOS 应用识别下的性能边界。目标不是只跑出一个峰值 RPS，而是建立可复现、可比较、可定位瓶颈的性能测试体系，覆盖以下四条主线：

1. 代理转发性能：HTTP、HTTPS CONNECT、TLS 解包、WebSocket 和 SSE 在不同 body 大小、连接复用和并发模型下的吞吐、延迟和错误率。
2. 流量录制性能：Traffic Writer、body cache、traffic DB、详情查询、列表刷新和搜索在高写入压力下的吞吐、写入延迟、内存增长和磁盘增长。
3. 转发策略性能：无规则、简单 host 规则、大量规则、多协议规则、过滤器、脚本和上游代理链组合下的规则匹配与转发开销。
4. macOS 应用识别性能：端口到进程、进程到应用、应用到 Traffic/Metrics 的识别链路在高并发和短连接压力下的准确率、延迟、回填和热路径开销。

## 目标与非目标

### 目标

- 以“行业顶级代理服务”为长期方向建立性能、稳定性、可靠性、功能完备性和可运营性五维评价体系；它不是一次压测必须达到的固定数值，而是要求每轮都能衡量现状、发现瓶颈、形成改进项，并持续抬高自身基线。
- 在不改变软件既有功能、语义、协议能力和用户可见行为的前提下持续提升代理领域能力；任何性能优化都必须先证明功能等价，再讨论性能收益。
- 输出固定压测矩阵，保证每次性能测试都能横向比较。
- 每次压测必须同时采集负载结果、Bifrost Admin 指标、进程资源、traffic DB 统计和规则策略维度。
- 将压测分为 smoke、本地基准、夜间长跑和发布门禁四档，避免把重型压测塞进普通单测。
- 复用现有 `scripts/loadtest-proxy-stability.mjs` 和 `scripts/loadtest-upstream-502-analysis.mjs`，并在后续新增 `scripts/perf/` 专项压测脚本时保持统一报告格式。
- 所有启动命令必须使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。

### 持续提升框架

“行业顶级”在本方案中不是一次性的验收口号，而是持续提升方向。每次压测都必须按五个维度衡量当前水平，并把瓶颈沉淀为下一轮优化目标：

| 维度 | 持续提升方向 | 典型退化信号 |
| --- | --- | --- |
| 性能 | 持续提升 RPS、吞吐、p95/p99、CPU/请求和写放大指标 | 只能在关闭录制/关闭规则时快，真实场景一开就退化 |
| 稳定性 | 持续降低 soak 下 RSS、FD、连接、DB 写入和后台任务的波动 | cooldown 后资源不回落、长连接泄漏、DB 写入积压 |
| 可靠性 | 持续降低请求错误、traffic 缺失、规则误命中、应用识别 unknown 和不可解释 5xx | 请求成功但无记录、规则误命中、应用识别 unknown 飙升 |
| 功能完备性 | 持续扩大 HTTP/HTTPS/SOCKS5/WebSocket/SSE/TLS 解包/规则/录制/Replay/应用维度统计的压力覆盖 | 只压 HTTP happy path，关键功能没有压力覆盖 |
| 可运营性 | 持续提升 run 目录、summary、原始采样、可复跑命令和 next actions 的可复盘质量 | 只有 console 输出，无法复盘、无法比较、无法归因 |

每轮压测结论必须至少回答：

- 当前基线是什么：代理核心、流量录制、转发策略、macOS 应用识别分别能稳定到什么水平。
- 最大瓶颈是什么：CPU、内存、FD、DB、body cache、规则匹配、进程解析、上游还是压测客户端。
- 下轮要提升什么：列出 P0/P1/P2 改进项，而不是只给“通过/失败”。
- 基线如何变化：和上一次同机同参数 run 对比，说明进步、退化或不可比原因。

### 功能不变门禁

性能提升不能以牺牲功能和能力为代价。任何后续优化在进入实现前都必须声明“被保护的功能语义”，实现后必须用对应用例证明没有改变。

| 保护对象 | 不允许的优化方式 | 必须保留的行为 |
| --- | --- | --- |
| 代理协议 | 为提高 RPS 删除、绕过或弱化 HTTP/HTTPS/SOCKS5/WebSocket/SSE/TLS 解包路径 | 所有已支持协议继续可用，错误语义和连接生命周期保持一致 |
| 流量录制 | 为降低写入成本丢弃请求/响应记录、body、headers、frames 或详情字段 | 请求成功后记录可追踪，允许异步回填但不能永久丢失 |
| 规则系统 | 为降低匹配成本改变优先级、filter、merge、script、proxy chain 或 TLS routing 语义 | 同一规则输入在优化前后产生同一转发和修改结果 |
| macOS 应用识别 | 为减少系统调用直接关闭应用识别、扩大 unknown、跳过应用级 TLS 策略 | 高压下可以降级但必须可解释、可回填，不能系统性破坏策略 |
| 管理端与 CLI | 为减少后台压力删除 Metrics、Traffic、Search、Replay、Export 或诊断字段 | 用户可见 API/CLI/WebUI 行为保持兼容 |
| 可靠性边界 | 为追求峰值吞吐取消 timeout、backpressure、清理、错误归因或安全限制 | 失败必须可恢复、可诊断，不能 silent drop 或 silent corruption |

功能等价验证要求：

- 每次性能优化必须先跑对应功能回归：规则、Traffic、Search、Replay、Metrics apps、TLS/SOCKS5/WebSocket/SSE 和 macOS 应用识别按影响范围选择。
- 对任何“异步化、采样、缓存、批处理、延迟回填”优化，必须证明最终一致性：数据可以晚到，但不能永久缺失或错误。
- 对任何“跳过热路径工作”的优化，必须列出跳过条件、降级结果、用户可见表现和后续回填/诊断路径。
- 若优化确实需要改变语义，不能作为性能修复直接落地，必须单独进入产品设计和兼容性评审。

### 非目标

- 不把性能结果和开发机绝对数值强绑定；门禁优先使用相对退化阈值。
- 不用公网服务作为默认上游；默认使用本地 mock upstream，公网目标只能作为手工诊断补充。
- 不在普通 PR CI 中跑 10 分钟以上的长压测；长跑进入 nightly 或手工发布前验证。
- 不通过关闭、删除、弱化既有功能来换取性能数据提升；这类结果不能进入性能基线。

## 持续提升闭环

本方案不仅用于一次压测，更用于驱动本机持续提升。后续执行时按以下闭环推进，直到主要卡点都被定位、修复并重新纳入基线：

1. Evaluate：在本机用固定矩阵跑出当前基线，输出 `summary.md` 和原始采样。
2. Diagnose：按分析路径定位最大瓶颈，明确是代理核心、traffic writer、DB/body cache、规则匹配、macOS 应用识别、上游或压测客户端。
3. Optimize：只做保持功能语义不变的优化，记录优化假设、影响范围和风险。
4. Verify Function：先跑功能等价验证，证明代理能力、录制能力、规则语义、应用识别和管理端行为没有被削弱。
5. Verify Performance：复跑同一性能矩阵，和优化前同机同参数 run 对比。
6. Ratchet：若性能提升且功能等价成立，将新结果写入基线；若退化或不可解释，回滚或继续定位。
7. Repeat：选择下一项 P0/P1/P2 瓶颈继续推进。

每一轮闭环必须留下：

- baseline run id 与 after run id。
- 优化假设和改动范围。
- 功能等价验证命令与结果。
- 性能变化表，包括提升、退化和不可比项。
- 下一轮优化队列。

## 指标口径

### 代理转发指标

| 指标 | 口径 | 采集来源 | 门禁建议 |
| --- | --- | --- | --- |
| RPS | 完成请求数 / 稳态窗口秒数 | loadtest report | 相对 main 基线下降不超过 10% |
| 吞吐 | 响应 bytes / 稳态窗口秒数 | loadtest report | 相对 main 基线下降不超过 10% |
| p50/p95/p99 延迟 | 完成请求端到端延迟 | loadtest report | p95/p99 不超过基线 1.2x |
| 错误率 | timeout + connection error + 5xx 中非预期项 | loadtest report + traffic detail | smoke 为 0，长跑小于 0.1% |
| CPU/RSS | Bifrost 进程 CPU 与 RSS | `/_bifrost/api/system/memory`、`/_bifrost/api/metrics` | 稳态无持续线性增长 |
| FD/连接数 | 打开连接和 socket 数 | `lsof`、metrics | cooldown 后回落到接近 idle |

### 流量录制指标

| 指标 | 口径 | 采集来源 | 门禁建议 |
| --- | --- | --- | --- |
| 记录完整率 | traffic records / 成功请求数 | `/_bifrost/api/traffic` | 默认记录模式下大于 99.9% |
| 写入滞后 | 请求结束时间到 record 可查询时间 | loadtest 轮询 API | p95 小于 2s |
| 详情读取延迟 | `GET /api/traffic/{id}` 延迟 | loadtest 采样 | p95 小于 200ms |
| DB 增长率 | traffic DB bytes / 请求数 | 文件系统统计 | 大小随 body 策略符合预期 |
| body cache 增长 | body cache bytes / 大 body 请求数 | 文件系统统计 | 清理策略触发后可回收 |
| 查询退化 | list/search 在高记录数下的 p95 | Admin API 采样 | p95 不超过基线 1.2x |

### 转发策略指标

| 指标 | 口径 | 采集来源 | 门禁建议 |
| --- | --- | --- | --- |
| 规则集加载时间 | 写入/启用规则到 active 生效时间 | Admin API + loadtest | 大规则集小于 3s |
| 转发匹配开销 | 同负载下规则组相对 no-rule 延迟差 | loadtest report | simple 小于 5%，large 小于 20% |
| 候选规则规模 | 每请求命中候选数和最终命中数 | traffic detail `matched_rules` | 与规则矩阵预期一致 |
| 策略正确率 | host/path/filter/proxy chain 转发结果 | upstream echo + traffic detail | 100% |
| 脚本规则开销 | reqScript/resScript 与无脚本对比 | loadtest report | 明确单独报告，不并入核心代理门禁 |

### macOS 应用识别指标

macOS 上的端口应用识别属于代理核心能力。它会影响应用级 TLS 策略、Traffic 归因、Metrics apps 统计、问题诊断和用户对代理可信度的感知，因此必须纳入压力测试。

| 指标 | 口径 | 采集来源 | 门禁建议 |
| --- | --- | --- | --- |
| app recognition rate | 成功识别应用的 sampled records / 可识别客户端 records | traffic detail + metrics apps | 常见客户端大于 99%，高压 unknown 不应系统性升高 |
| app recognition latency | 连接建立到 app info 可见的时间 | traffic detail polling | p95 小于 500ms，p99 小于 2s |
| resolver overhead | 开启应用识别与关闭/跳过识别场景的 p95 差值 | loadtest 对比 | p95 增量小于 5%，p99 增量小于 10% |
| resolver timeout rate | 进程解析超时或并发阀门饱和次数 / 请求数 | logs + metrics | steady 小于 0.1%，burst 后可恢复 |
| snapshot efficiency | 每秒系统 socket/process 扫描次数 / 请求数 | resolver metrics 或日志采样 | 高并发下不随请求数线性增长 |
| attribution correctness | 端口、PID、bundle/app name 与 ground truth 对齐率 | client harness + traffic detail | 100% 用例正确，允许短期 pending 但最终回填 |

### 功能等价指标

| 指标 | 口径 | 采集来源 | 门禁建议 |
| --- | --- | --- | --- |
| behavior parity | 优化前后同一功能用例输出一致 | E2E/human_tests/API response diff | 必须 100% |
| traffic data integrity | 成功请求对应记录、详情、body/header/frame 可追踪 | Traffic API + detail sample | 不允许永久缺失 |
| rule semantic parity | 同一规则 profile 的命中、转发、改写结果一致 | upstream echo + traffic detail | 必须 100% |
| app policy parity | macOS 应用识别与应用级策略结果一致 | client manifest + Traffic + metrics apps | 必须 100% |
| API/CLI compatibility | 公开 API/CLI schema 和关键字段兼容 | snapshot/diff | 不允许无设计评审的破坏性变化 |

## 测试方法

性能测试必须按“先校准、再隔离变量、最后组合压力”的顺序执行，避免把上游、压测客户端、流量录制和规则匹配混成一团。

### 总体流程

1. 环境校准：
   - 使用同一台机器、同一网络、同一 release binary、同一端口范围。
   - 先直连本地 upstream，不经过 Bifrost，测出 load generator 与 upstream 自身上限。
   - 记录 `git rev-parse HEAD`、`rustc -Vv`、`node -v`、CPU 型号、内存、macOS 版本和电源状态。
2. 代理核心基线：
   - 使用空规则、默认 traffic 记录配置，跑 HTTP small、HTTP large、HTTPS CONNECT 和 SSE。
   - 目标是建立 Bifrost 在无复杂规则时的基线吞吐、延迟、CPU/RSS 和错误率。
3. 流量录制隔离：
   - 固定相同请求负载，分别跑 metadata-heavy、小 body、大 body、详情高频读取、list/search 并发查询场景。
   - 目标是拆出 traffic writer、body cache、DB 写入、详情读取和搜索对代理链路的影响。
4. 转发策略隔离：
   - 固定相同请求负载，按 `no-rule -> single-host -> path-prefix-100 -> mixed-1000 -> proxy-chain -> script-heavy` 顺序压测。
   - 目标是用相对 `no-rule` 的差值衡量规则匹配、过滤器、脚本和代理链成本。
5. macOS 应用识别隔离：
   - 在 macOS 上固定请求负载，分别用 curl、Node.js、Python、浏览器和短连接子进程发起流量。
   - 目标是验证端口到进程、进程到应用、应用到 metrics/traffic 的链路在高压下仍准确且不拖慢代理。
6. 组合压力：
   - 选择最接近真实使用的规则集、body 配置和请求 mix，执行 30-60 分钟 soak。
   - 目标是发现内存增长、FD 泄漏、DB 膨胀、写入积压和 cooldown 后无法恢复的问题。

### 代理性能怎么测

代理性能压测以“上游直连基线 + Bifrost 空规则基线 + 协议矩阵”为核心。

| 步骤 | 执行动作 | 观察重点 | 产物 |
| --- | --- | --- | --- |
| 1 | 启动本地 HTTP/HTTPS/SSE/WebSocket upstream | upstream 无错误、直连延迟稳定 | upstream log |
| 2 | 直连 upstream 跑 fixed-rate 阶梯：100、300、600、1000、1500 RPS | load generator 自身是否先饱和 | `direct-baseline.json` |
| 3 | 启动 Bifrost release binary，空规则，经代理跑同一阶梯 | 找到 Bifrost 饱和点 | `proxy-no-rule.json` |
| 4 | 对 HTTP small、HTTP large、HTTPS CONNECT、TLS 解包、SSE 分别跑 steady + burst + cooldown | 协议差异、连接生命周期、RSS 回落 | `proxy-protocol-*.json` |
| 5 | 对比 direct 与 proxy | 代理额外延迟、吞吐折损、CPU/请求 | `compare-direct-proxy.json` |

阶梯压测规则：

- 每个 RPS 档位至少包含 15s warmup、60s steady、15s burst、30s cooldown。
- 当任一条件出现时停止继续加压：错误率大于 0.1%、p99 超过基线 2 倍、CPU 连续 30s 高于 90%、RSS 线性增长且 cooldown 不回落、load generator 自身开始超时。
- 饱和点定义为“最后一个同时满足错误率、p99、CPU/RSS 和 cooldown 条件的最高 RPS 档位”。

### 流量录制性能怎么测

流量录制性能压测要把“代理转发成功”和“记录可用”分开统计。请求成功不代表 traffic 记录已经稳定落库。

| 场景 | 负载 | 采集动作 | 评估点 |
| --- | --- | --- | --- |
| metadata-only | 小 body，高 RPS | 每秒查询 `/_bifrost/api/traffic?limit=1` 和总数 | record completeness、write lag |
| small-body | 1-4 KiB JSON body | 抽样读取 `/_bifrost/api/traffic/{id}` | detail p95、body 是否完整 |
| large-body | 1-10 MiB body | 统计 DB 与 body cache 文件增长 | 磁盘增长率、RSS 峰值、清理回收 |
| list-pressure | 写入同时并发 `traffic?limit=100` | list p95 和代理 p99 | 管理端查询是否拖慢代理 |
| search-pressure | 写入同时执行 body/header/search | search p95、max-scan 成本 | 搜索是否造成写入积压 |

关键计算：

```text
record_completeness = matched_traffic_records / successful_requests
write_lag_ms = first_visible_at_ms - request_finished_at_ms
detail_read_p95_ms = percentile(api_traffic_detail_latency_ms, 95)
db_bytes_per_request = (traffic_db_bytes_after - traffic_db_bytes_before) / successful_requests
body_cache_bytes_per_large_request = body_cache_delta_bytes / large_body_successful_requests
```

流量录制专项必须额外记录：

- `traffic records total`：压测开始前、steady 结束、cooldown 结束各采一次。
- `traffic detail sample`：每 1000 个成功请求至少抽样 10 条详情。
- `body_cache` 与 traffic DB 文件大小：每 5s 采样。
- 写入积压信号：请求已完成但 traffic record 未在 2s 内可见的比例。
- 管理端查询干扰：有无 list/search 并发时代理 p95/p99 的差值。

### 转发策略性能怎么测

转发策略性能压测以固定流量、替换规则集为原则。除规则文件外，其他变量必须保持一致。

| 规则 profile | 规则规模 | 请求分布 | 评估点 |
| --- | --- | --- | --- |
| `no-rule` | 0 | 所有请求直达 upstream | 代理核心基线 |
| `single-host` | 1 | 100% 命中 host 转发 | 简单规则成本 |
| `path-prefix-100` | 100 | 80% 命中、20% miss | path 前缀匹配成本 |
| `mixed-1000` | 1000 | host/path/header/filter 混合 | 候选剪枝与 miss 成本 |
| `proxy-chain` | 2 层代理 | 100% 经下游代理 | 下游连接复用与错误传播 |
| `script-heavy` | reqScript/resScript | 10%、50%、100% 命中三档 | JS 执行和 body 变更成本 |

策略评估公式：

```text
matcher_latency_overhead = rule_profile_p95_ms / no_rule_p95_ms - 1
matcher_throughput_overhead = 1 - rule_profile_rps / no_rule_rps
rule_correctness = traffic_records_with_expected_matched_rules / sampled_traffic_records
proxy_chain_error_rate = downstream_errors / completed_requests
```

每个规则 profile 必须抽样检查 traffic detail：

- `matched_rules` 是否符合预期。
- `actual_url`、`actual_host`、`listener_port` 是否正确。
- miss 请求是否未被错误转发。
- proxy chain 场景下游代理是否记录 CONNECT 或 HTTP absolute-form 请求。

### macOS 应用识别怎么测

macOS 应用识别压测分为“准确性压测”和“热路径开销压测”。准确性只看识别是否正确，热路径开销看识别过程是否拖慢代理。

| 场景 | 客户端 | 负载 | 验证点 |
| --- | --- | --- | --- |
| `app-curl-steady` | curl | 100-1000 RPS fixed-rate | Traffic 记录显示 curl 或可解释的 CLI 进程名 |
| `app-node-burst` | Node.js 子进程池 | burst 短连接 | 短连接仍能最终回填 app info |
| `app-python-many-pids` | Python 多进程 | 频繁 PID 变化 | negative cache 不误压制新进程 |
| `app-browser-connect` | Chrome/Safari | CONNECT/TLS | 应用级 TLS 策略和 Metrics apps 归因正确 |
| `app-mixed` | curl + node + python + browser | 混合协议 | apps 统计按应用拆分，unknown 不随 RPS 放大 |

执行步骤：

1. 记录 ground truth：
   - 每个客户端启动时写入 `client-manifest.ndjson`，包含 `scenario`、`pid`、`ppid`、`command`、`expected_app`、`start_time`。
   - 对浏览器类客户端额外记录 bundle id、app path 和版本。
2. 产生流量：
   - 每个客户端使用固定 host/path 标记，例如 `/app/curl/<pid>`、`/app/node/<pid>`，便于从 Traffic 反查。
   - 同时覆盖 HTTP、HTTPS CONNECT、TLS 解包和短连接 burst。
3. 采集识别结果：
   - 每秒查询 `/_bifrost/api/metrics/apps`。
   - 每 1000 个请求抽样 traffic detail，记录 `client_app`、`client_process`、`client_pid`、`listener_port`、`actual_host`。
   - 抽样 `lsof -nP -iTCP -sTCP:ESTABLISHED` 或等效系统视图，和 manifest 对齐。
4. 评估准确率：
   - `recognized_records / eligible_records`。
   - `correctly_attributed_records / recognized_records`。
   - `unknown_after_2s / eligible_records`。
5. 评估热路径开销：
   - 同一负载下对比“启用应用识别”和“管理端请求/跳过识别路径”的 p95/p99 差异。
   - 观察 `spawn_blocking`、进程解析 timeout、并发阀门饱和、snapshot 刷新次数是否随请求数线性增长。

macOS 应用识别专项必须验证：

- 高并发 CONNECT 不导致每请求全量扫描进程表。
- 大量短连接不会长期 unknown；允许先记录、后 backfill，但最终 Traffic 和 Metrics apps 要一致。
- 管理端 `/_bifrost` 请求不触发应用识别，不能被 resolver 拖慢。
- 进程解析超时后必须降级为 unknown 并继续代理，不能阻断请求。
- 应用级 TLS 拦截白名单/黑名单在高压下不能因为 unknown 系统性失效。

## 压测矩阵

### 负载模型

| 档位 | 用途 | 时长 | 并发 / 速率 | 运行位置 |
| --- | --- | --- | --- | --- |
| smoke | PR 前快速回归 | 30-60s | 小并发 + 固定速率 | 本地 / 可选 CI |
| baseline | 建立当前分支基准 | 3-5min | closed-loop + fixed-rate | 本地 release build |
| soak | 检查内存、FD、DB 长期增长 | 30-60min | 稳态 + burst + cooldown | nightly / 手工 |
| regression | 对比 main 与当前分支 | 两次 baseline | 相同机器、相同参数 | 发布前 |

### 请求场景

| 场景 | 协议 | body | 重点观测 |
| --- | --- | --- | --- |
| `http-small` | HTTP absolute-form | 1-4 KiB | 纯转发 RPS、延迟 |
| `http-large` | HTTP absolute-form | 1-10 MiB | 吞吐、body cache、RSS |
| `https-connect` | CONNECT passthrough | 1-4 KiB | tunnel 建立成本、连接复用 |
| `https-mitm` | TLS 解包 | 1-4 KiB + JSON | 证书、解包、规则匹配 |
| `sse-stream` | SSE | 长连接 + 小 frame | 活跃连接、内存、push |
| `websocket-echo` | WS/WSS | 小 frame + burst | frame 录制、连接生命周期 |
| `status-mix` | HTTP | 2xx/4xx/5xx | 错误归因、traffic 状态统计 |

### 转发策略场景

| 场景 | 规则集 | 验证点 |
| --- | --- | --- |
| `no-rule` | 空规则 | 代理核心基线 |
| `single-host` | 1 条 host/http 转发 | 简单匹配开销 |
| `path-prefix-100` | 100 条 host + path 前缀 | path matcher 退化 |
| `mixed-1000` | 1000 条 host/path/header/filter | 大规则集候选剪枝 |
| `proxy-chain` | 下游代理转发 | 连接复用与错误传播 |
| `script-heavy` | reqScript/resScript | JS 执行开销单独报告 |
| `mitm-routing` | HTTPS 解包 + 路由例外 | TLS 解包触发与 passthrough 边界 |

### macOS 应用识别场景

| 场景 | 连接模式 | 客户端规模 | 重点观测 |
| --- | --- | --- | --- |
| `app-curl-steady` | HTTP keep-alive + 短连接 | 1-16 个 curl 进程 | app 识别准确率、resolver p95 |
| `app-node-burst` | HTTP/HTTPS burst | 32-256 个短生命周期 Node.js 子进程 | snapshot 新鲜度、backfill 成功率 |
| `app-browser-connect` | CONNECT + TLS 解包 | Chrome/Safari 稳态浏览器流量 | 应用级 TLS 策略、Metrics apps |
| `app-mixed-soak` | HTTP/HTTPS/SSE/WS 混合 | curl + node + python + browser | unknown 比例、RSS/FD 回落、apps 统计稳定性 |

## 工具链设计

### 现有脚本

- `scripts/loadtest-proxy-stability.mjs`
  - 适合作为代理转发和资源稳定性的第一版 smoke/baseline 工具。
  - 已覆盖 HTTP small/large、HTTPS small/large、SSE、fixed-rate/closed-loop、metrics 和 memory 采样。
  - 输出 `.artifacts/loadtest/proxy-stability-<run-id>.json`。
- `scripts/loadtest-upstream-502-analysis.mjs`
  - 适合分析特定上游在并发下的 502、错误详情和 traffic detail 归因。
  - 输出 `.artifacts/loadtest/upstream-502-<run-id>.json`。

### 采集器职责

后续性能脚本必须把采集分成七类，统一写入同一个 run 目录：

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

| 采集类别 | 采样频率 | 采集内容 | 用途 |
| --- | --- | --- | --- |
| load | 每请求 + 阶段汇总 | started/completed/ok/error、latency、bytes、inflight | 吞吐、延迟、错误率 |
| admin metrics | 1s | `/_bifrost/api/metrics`、`/_bifrost/api/system/memory` | CPU/RSS、连接、缓存、队列 |
| process | 1s | pid、RSS、CPU、FD 数、线程数 | 识别 OS 资源瓶颈 |
| traffic | 1-5s + 抽样详情 | list total、最新 record、detail latency、matched_rules | 录制完整率与规则正确性 |
| app resolution | 1s + 抽样详情 | metrics apps、client app、client pid/process、unknown、timeout、snapshot refresh | macOS 应用归因准确率与开销 |
| files | 5s | traffic DB、body cache、日志目录大小 | 磁盘增长与清理 |
| logs | 全量 | Bifrost stdout/stderr、upstream error | 错误归因 |

最低采集命令集合：

```bash
curl -sS "http://127.0.0.1:${BIFROST_PROXY_PORT}/_bifrost/api/metrics"
curl -sS "http://127.0.0.1:${BIFROST_PROXY_PORT}/_bifrost/api/metrics/apps"
curl -sS "http://127.0.0.1:${BIFROST_PROXY_PORT}/_bifrost/api/system/memory"
curl -sS "http://127.0.0.1:${BIFROST_PROXY_PORT}/_bifrost/api/traffic?limit=100"
lsof -nP -p "${BIFROST_PID}"
lsof -nP -iTCP -sTCP:ESTABLISHED
ps -o pid,ppid,%cpu,rss,etime,command -p "${BIFROST_PID}"
du -sk "${BIFROST_DATA_DIR}"
find "${BIFROST_DATA_DIR}" -type f -maxdepth 4 -print
```

### 后续新增脚本结构

建议后续将性能专项脚本集中放入 `scripts/perf/`：

```text
scripts/perf/
  proxy-throughput.mjs
  traffic-recording.mjs
  rule-strategy.mjs
  app-resolution.mjs
  compare-runs.mjs
  fixtures/
    upstream-http.mjs
    upstream-websocket.mjs
    client-spawner.mjs
    rules/
      no-rule.bifrost
      single-host.bifrost
      path-prefix-100.bifrost
      mixed-1000.bifrost
```

脚本职责：

- `proxy-throughput.mjs`：执行直连 upstream 与 Bifrost proxy 的协议吞吐矩阵，输出 direct/proxy 对比。
- `traffic-recording.mjs`：执行 traffic 写入、详情读取、list/search 并发、body cache 增长和写入滞后测试。
- `rule-strategy.mjs`：生成或加载规则 profile，执行固定负载并抽样验证 `matched_rules` 与转发目标。
- `app-resolution.mjs`：仅在 macOS 执行应用识别与端口归因压测，输出识别率、unknown 比例、回填延迟、resolver 开销和 Metrics apps 一致性。
- `compare-runs.mjs`：对比两个 run 或当前 run 与 main 基线，输出退化项、可能瓶颈和优先排查建议。

统一报告 JSON 必须包含：

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

## 评估方法

### 通过 / 失败 / 需分析

| 结论 | 条件 |
| --- | --- |
| pass | 错误率、吞吐、p95/p99、RSS、FD、record completeness、规则正确率全部在阈值内 |
| investigate | 功能正确但 p95/p99、RSS、DB 增长、write lag 或 list/search 有单项明显退化 |
| fail | 出现请求错误、traffic 记录缺失、规则转发错误、进程崩溃、cooldown 不恢复或资源持续增长 |
| invalid | 性能数字变好但功能语义、协议能力、数据完整性、规则行为或应用识别被削弱 |

### 阈值策略

- smoke 阈值：重在发现明显功能和资源问题，要求错误率为 0、脚本退出码为 0、报告 schema 完整。
- baseline 阈值：用当前 main 的同机同参数结果作为基线。没有历史基线时，首次结果只入库不判失败。
- regression 阈值：
  - RPS 或吞吐下降超过 10%：需分析。
  - p95/p99 上升超过 20%：需分析。
  - p99 上升超过 50% 或错误率大于 0.1%：失败。
  - RSS steady 斜率大于 5 MiB/min 且 cooldown 不回落：失败。
  - record completeness 小于 99.9%：失败。
  - rule correctness 小于 100%：失败。
  - macOS app recognition rate 低于 99% 或 unknown_after_2s 超过 1%：需分析；应用级策略场景出现错误归因：失败。
  - behavior parity、rule semantic parity、app policy parity 任一不满足：本轮性能结果无效，不能作为提升基线。

### 分析方法

分析顺序必须从外到内，先排除压测工具和上游，再定位 Bifrost 内部瓶颈。

1. 压测客户端是否饱和：
   - direct upstream 已经高 p99 或错误，说明客户端/upstream 先到瓶颈。
   - load generator CPU 高于 85% 时，本轮不能作为 Bifrost 上限。
2. 上游是否饱和：
   - direct baseline 正常、proxy 场景异常，才进入 Bifrost 分析。
   - upstream log 出现 connection reset、timeout、backpressure 时，先降低 upstream 成本或换更轻 fixture。
3. 代理核心是否饱和：
   - CPU 高、RSS 稳定、FD 稳定、record completeness 正常：多半是 CPU 计算或协议处理瓶颈。
   - FD 持续增长：优先查连接生命周期、SSE/WS/tunnel cleanup。
   - cooldown 后活跃连接不回落：优先查 keep-alive、CONNECT、WebSocket close。
4. 流量录制是否拖慢：
   - proxy p99 和 write lag 同时升高：优先查 traffic writer 队列、DB 写入、body cache。
   - list/search 并发时代理 p99 上升：优先查 DB 查询锁、分页、索引、body scan。
   - DB bytes/request 异常升高：优先查 body 存储策略和清理策略。
5. 规则策略是否拖慢：
   - `mixed-1000` 相比 `no-rule` p95 明显升高：优先查候选剪枝、filter gate、path matcher。
   - miss 请求比 hit 请求更慢：优先查规则全量扫描和 negative cache。
   - `script-heavy` 单独退化：单独归因为脚本执行或 body decode/encode，不并入核心代理。
6. macOS 应用识别是否拖慢或失真：
   - unknown 比例随 RPS 放大：优先查 snapshot TTL、新连接刷新策略、negative cache 和 backfill 饱和。
   - p99 升高但 CPU 不高：优先查 blocking resolver 排队、系统调用阻塞和并发阀门等待。
   - Metrics apps 与 traffic detail 不一致：优先查回填顺序、async traffic update 早于 record 的暂存逻辑。
   - 管理端请求 p95 升高：确认 `/_bifrost` 路径是否跳过进程解析。
   - 应用级 TLS 策略误判：抽样 `client-manifest.ndjson`、traffic detail、resolver log 和系统 `lsof` 视图交叉验证。
7. 错误归因：
   - 所有 5xx 必须抽样 `traffic/{id}`，记录 `error_message`、`actual_url`、`matched_rules`、upstream log。
   - 502 必须用 `scripts/loadtest-upstream-502-analysis.mjs` 复跑最小场景，区分上游拒绝、连接复用错误、TLS/CONNECT 错误和规则转发错误。

### 输出分析报告

每次完整压测必须产出 `summary.md`，结构固定：

```markdown
# Bifrost Performance Run <run-id>

## Conclusion
- status: pass / investigate / fail
- top bottleneck:
- max stable RPS:
- largest regression:
- function parity:

## Environment
- git:
- binary:
- machine:
- command:

## Proxy Throughput
- direct baseline:
- no-rule proxy:
- protocol matrix:

## Traffic Recording
- record completeness:
- write lag:
- detail/list/search latency:
- DB/body cache growth:

## Rule Strategy
- no-rule:
- single-host:
- path-prefix-100:
- mixed-1000:
- proxy-chain:
- script-heavy:

## macOS App And Port Recognition
- recognition rate:
- correctness:
- unknown after 2s:
- backfill latency:
- resolver overhead:
- metrics apps consistency:

## Evidence
- artifacts:
- sampled 5xx:
- sampled matched_rules:

## Function Parity
- behavior parity:
- traffic data integrity:
- rule semantic parity:
- app policy parity:
- API/CLI compatibility:

## Next Actions
- P0:
- P1:
- P2:
```

## 首轮压测发现与优化记录

### 2026-06-19：binary fast path 连接监控残留

现象：

- fixed-rate 压测包含 HTTP small、HTTP large chunked、HTTPS small、HTTPS large 和 SSE。
- 优化前报告 `proxy-stability-2026-06-18T16-30-18.900Z.json`：总成功请求 `ok=27854`、`errors=0`、`non2xx=0`，HTTP large `ok=762`。
- cooldown 结束时 `cooldownWsConnectionCount=762`、`cooldownWsOpenConnectionCount=762`、`cooldownWsFramesInMemory=0`，说明 binary performance fast path 创建了无帧、无详情价值且未关闭的 `ConnectionMonitor` 项。

根因：

- HTTP large chunked 响应在 binary performance mode 下走 metrics-only forwarding，只保留字节统计和 traffic metadata，不保存响应体/帧。
- 记录创建阶段仍按 streaming 注册 `ConnectionMonitor`，但 metrics-only body 不负责关闭 monitor，导致每个 large 响应留下 open monitor 对象。

优化：

- binary fast path 仍保留 traffic streaming metadata、响应大小统计和转发行为。
- 仅跳过无 payload 价值的 streaming `ConnectionMonitor` 注册；WebSocket、SSE 和非 binary fast path 的显式 streaming 监控语义不变。
- streaming 连接关闭时把最终 socket status 写回 traffic record，frames API 在 monitor 被清理后可从持久记录兜底返回关闭状态。
- `ConnectionMonitor` memory stats 增加 open/closed connection 计数，loadtest 报告增加 cooldown connection count，后续可直接量化该类问题。

复测：

- 优化后报告 `proxy-stability-2026-06-18T16-36-28.116Z.json`：总成功请求 `ok=27854`、`errors=0`、`non2xx=0`，HTTP large `ok=762`，与优化前请求规模一致。
- cooldown 结束时 `cooldownWsConnectionCount=0`、`cooldownWsOpenConnectionCount=0`、`cooldownWsClosedConnectionCount=0`。
- 峰值 RSS 从 `161.2 MiB` 降到 `110.08 MiB`。该数值会受 allocator 和运行时状态影响，基线提升以 connection monitor 残留清零为主证据，RSS 下降作为辅助证据。
- 功能等价门禁：HTTP/HTTPS/SSE 子场景无错误、无非 2xx；未关闭转发、录制、规则匹配、TLS 决策或 macOS 应用识别能力。

## 执行命令

### 快速 smoke

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

- PR smoke：只执行 30-60 秒轻量场景，验证脚本可运行、报告 schema 完整、启动保护生效。
- nightly：执行 full matrix，保存 `.artifacts/loadtest/*.json`，并与 main 最近 7 天中位数比较。
- release candidate：执行 release baseline + soak + regression，对代理核心、traffic recording、rule strategy 三条主线分别出报告。
- 普通 `cargo test --workspace --all-features` 不承载性能压测；性能门禁由专用脚本和报告比较承担。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标是否覆盖代理性能、流量录制性能、转发策略性能和 macOS 应用识别性能。
- review `scripts/loadtest-*.mjs` 启动参数是否包含 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 执行 `node --check` 与 `rg` 静态验收，确认方案中的脚本、指标、阈值和报告 schema 可被检索。

第 2 轮：

- 复查 design、human_tests 和索引是否一致。
- 复跑静态验收命令，确认没有只更新方案但未更新真实场景测试。
- 如发现方案缺口，补充后追加新一轮 review。

## 文档更新要求

- 本方案是性能压测体系的入口文档。
- 每次新增可执行性能脚本时，必须同步更新本文件的工具链、命令、报告 schema 和 `human_tests/proxy-performance-stress-test.md`。
- 若未来将性能门禁接入 CI，必须同步更新 `scripts/ci/local-ci.sh` 或对应 GitHub Actions 文档，并记录门禁阈值来源。
