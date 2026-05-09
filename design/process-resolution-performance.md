# 进程解析性能优化

## 功能模块说明

代理在本地 CONNECT 请求上需要识别客户端进程，用于应用级 TLS 拦截策略、流量归因和管理端展示。该模块必须保持现有判断语义不变，同时降低极限流量下的系统调用成本。

## 当前问题

macOS 进程归属解析需要通过 `proc_listpids`、`proc_pidinfo(PROC_PIDLISTFDS)` 和 `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` 扫描进程和 socket。此前 macOS 每次连接 miss 都可能执行一次全量扫描；当系统代理流量中出现大量浏览器 CONNECT 时，会产生重复系统调用和 blocking task 排队，进而拖慢代理与管理端响应。

非 macOS 路径已经通过短 TTL socket 快照复用 `socket -> pid` 映射，macOS 缺少同等缓存。

## 实现逻辑

- 将 `SocketSnapshot` 提升为跨平台结构，由 `ProcessResolver` 统一维护。
- macOS 不再针对单个连接逐进程查找；改为构建一次 `ConnKey -> pid` 快照，再按连接 key 查询。
- 快照 TTL 保持 250ms，降低高并发 CONNECT/HTTP 下的重复系统调用，但仍保持短窗口内足够新鲜。
- 快照刷新使用 singleflight 保护；当快照过期且大量请求同时进入时，只有一个 blocking 任务执行系统 socket 扫描，其余任务复用刷新后的快照，避免同一时间多次全量扫描进程表。
- 快照命中缺失不再在整个 TTL 内盲目返回 miss；当新连接没有出现在当前快照且快照已经超过 50ms，会触发受 singleflight 保护的刷新，避免 200ms 内结束的短连接因为复用旧快照而丢失 appinfo，同时避免过于频繁地全量扫描 socket。
- 快照同时写入带 `proxy_addr` 的完整连接 key 和仅 `peer_addr` 的兼容 key，保持 `resolve_for_connection` 与 `resolve` 两类调用语义。
- 所有异步客户端进程解析统一增加 2 秒硬超时，超时后记录 warn、写入短期 negative cache，并按未知客户端继续处理请求。
- 异步解析入口读取缓存时保留 negative cache 语义；命中近期 miss/timeout 后直接返回未知客户端，不再继续排队或创建 blocking 任务。
- 所有异步进程解析的 `spawn_blocking` 统一经过全局并发阀门（默认 4，可通过 `BIFROST_PROCESS_RESOLUTION_CONCURRENCY` 调整）。请求会在 2 秒总预算内等待并发 permit 和实际解析；如果预算耗尽则按未知客户端继续处理，但不会把“并发饱和”写成连接级 negative cache，避免高并发下错误压制后续解析。
- 普通 HTTP 请求也在请求开始时执行受限、带超时的同步进程解析，保证 200ms 内结束的短请求仍有机会在 Traffic 记录创建前拿到 `clientApp`。管理端请求仍完全跳过进程解析。
- `/_bifrost` 管理端请求完全跳过客户端进程解析和后台 backfill，管理端接口不依赖 app 信息，避免管理端自访问被进程识别拖慢。
- 后台 backfill 仍保留给同步解析失败后的兜底路径；进程解析后台队列饱和时直接跳过，不排队积压，并在完成、miss 或跳过后复位连接级 in-flight 标记，允许同一 keep-alive 连接后续请求再次尝试。
- 异步 Traffic writer 支持先到达的 `Update(id)` 暂存；当对应 `Record(id)` 后续入库时会先应用挂起更新，避免 appinfo/body ref 等后台回填因为更新早于记录落库而永久丢失。
- CONNECT/SOCKS5 应用策略进程解析的逐请求日志降级为 debug，避免系统代理高流量下 info 日志本身造成 CPU 和 I/O 放大。
- 如果 CONNECT 已经因为应用策略同步解析过客户端进程但仍未命中，不再对同一连接立即追加后台 backfill，避免失败路径重复扫描进程表。
- 普通 HTTP 响应不再无条件写入 `ConnectionMonitor`；只有 WebSocket/显式流式响应这类已经注册监控的连接才更新连接监控状态，避免普通短请求在响应结束时抢全局 monitor 写锁。Traffic 记录、响应大小、响应体保存不因此丢失。
- 响应 body 保存保持“正常路径同步、极端锁等待后台最终保存”：当 `BodyStore` 可立即读取时仍同步写入并返回 `response_body_ref`；只有遇到 `BodyStore` 写锁占用或等待、会拖慢热路径时，才将已解码 body 放入后台保存任务。后台保存有全局并发上限（默认 1，可通过 `BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY` 调整），完成后回填 `response_body_ref`，不允许丢失记录或永久丢失 body。
- 不改变 TLS app policy、进程名/path 解析、流量记录或响应体采集语义；极端情况下只允许客户端应用信息短暂未知、body 引用短暂延迟回填，不能阻塞管理端，也不能牺牲数据完整性或让应用黑白名单在高并发下系统性失效。

## 依赖项

- macOS `libproc` 系统接口
- `ProcessResolver` 现有 per-connection cache 与 pid cache
- 非 macOS `netstat2` socket 快照逻辑

## 测试方案

- 单元测试：执行 `cargo test -p bifrost-proxy utils::process_info -- --nocapture`，覆盖进程解析缓存、negative cache 快速返回、并发容量等待超时不写负缓存、2 秒超时降级路径与真实 curl/node/python 客户端识别回归。
- 单元测试：执行 `cargo test -p bifrost-admin async_traffic -- --nocapture`，覆盖异步 Traffic writer 中 update 早于 record 到达时仍能最终应用。
- 单元测试：执行 `cargo test -p bifrost-proxy utils::tee -- --nocapture`，覆盖普通 HTTP 不写 `ConnectionMonitor`、流式连接继续写 monitor、`BodyStore` 忙时后台等待并最终保存响应体。
- E2E 测试：执行 `e2e-tests/tests/test_sse_frames.sh`，确认 SSE 捕获与详情读取不受进程解析优化影响。
- 真实场景测试：更新并执行 `human_tests/webui-traffic.md` 的 `TC-WTR-47`，在临时端口高并发 CONNECT 压力下验证 Traffic 页面和 SSE Messages 详情仍可打开。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 修改范围属于代理核心热路径，最终按需执行 `bash scripts/ci/local-ci.sh --e2e-only rules` 或等效相关 E2E。

## 文档更新要求

- `human_tests/webui-traffic.md` 增加高并发 CONNECT 下管理端/SSE 详情真实场景回归。
- `human_tests/readme.md` 同步 Web UI Traffic 用例数。
