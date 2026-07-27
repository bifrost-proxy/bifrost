# 上游连接稳定性治理

## 背景

本机系统代理承载多个应用时，某个浏览器或外部 Runner 可能在数秒内访问大量不同域名。此前 HTTPS 上游请求只有按 pool partition 的并发限制，连接池缓存达到 256 个分区时还会一次性删除所有没有外部 `Arc` 引用的 client。两者叠加后会造成大量连接同时关闭和重建，放大 `TIME_WAIT`、临时源端口压力以及网络接口切换期间的 `EADDRNOTAVAIL`。

本次优化只调整内部资源调度，不修改规则匹配、TLS 解包范围、HTTP/HTTPS/SOCKS5/WebSocket/SSE 语义、请求重放规则、Traffic 录制或 MOSS 行为。

## 用户目标验证清单

### 必须实现

- HTTPS client cache 达到上限后只渐进淘汰最老的少量闲置池，不再批量清空。
- 所有池化 HTTP/HTTPS 请求共享一个到响应头阶段的全局在途上限，避免不同域名分区相互放大。
- 原始 TCP 建连共享全局并发阀门；发生地址/文件描述符/缓冲区资源压力后，使用有界指数退避和 single-probe 恢复。
- 所有参数有安全默认值，并允许通过环境变量在压测环境调节。

### 必须不破坏

- 已建立的 CONNECT、WebSocket、SSE 和 HTTP/2 流不因限流被主动断开。
- 正常负载下请求不增加人为 delay；只有超过全局上限或处于资源恢复窗口时等待。
- 不自动重放非幂等请求，不吞掉原始连接错误，不改变现有错误响应分类。
- 不修改正式 9900 服务、系统代理和用户数据目录；测试使用隔离端口与临时目录。

### 必须真实验证

- 单元测试覆盖 LRU 次序、活跃池保护、全局建连并发、资源错误退避、probe 成功与取消恢复。
- E2E 通过本地 upstream 制造短连接突发，验证所有响应内容一致、无代理级连接错误且 cooldown 后连接回落。
- human_tests 逐条验证普通 HTTP、通用 CONNECT 突发和冷却后恢复；现有 HTTPS 压测 smoke 继续兜底 TLS 透传语义。
- 远端 CI coverage 运行 `bash scripts/ci/coverage-all.sh --json --gate`。

## 设计

### 渐进式 idle-aware LRU

`HTTPS_CLIENTS` 保存每个 key 的单调访问序号。达到 256 个 entry 后，每次插入最多淘汰 8 个最老且 `Arc::strong_count == 1` 的 entry：

- 有活跃请求持有的 client 不进入候选。
- 访问会更新 recency。
- 如果所有 entry 都活跃，允许缓存短时超过软上限，后续插入继续回收，避免为了硬上限中断活跃请求。

### 两层背压

- partition 上限保持现有默认 64，并继续持有到 response body 结束。
- 新增全局默认 256，只持有到 response headers 返回。长响应、SSE 和 WebSocket 不长期占用全局许可，HTTP/2 复用能力保持不变。
- 新增 TCP connect 默认并发 64，覆盖 HTTP CONNECT、TLS/WebSocket 上游、上游代理及 SOCKS5 直连路径。

环境变量：

- `BIFROST_UPSTREAM_MAX_INFLIGHT_GLOBAL`
- `BIFROST_UPSTREAM_MAX_INFLIGHT_PER_PARTITION`
- `BIFROST_UPSTREAM_CONNECT_CONCURRENCY`

值缺失、为零或无法解析时使用默认值。

### 资源压力恢复

以下错误进入共享恢复状态：

- `AddrNotAvailable` / `OutOfMemory`
- `too many open files`
- `resource temporarily unavailable`
- `no buffer space available`
- macOS 常见 `os error 24/49/55`

首次失败等待约 100ms，随后指数增长并封顶 2s，附加不超过 25% jitter。恢复窗口到期只允许一个请求或连接作为 probe；probe 成功或得到非资源类错误即退出恢复状态，probe 再次遇到资源错误则进入下一档退避。probe future 被取消时会释放 probe 状态，防止永久阻塞。

## 风险与边界

- 全局许可会在异常峰值下增加排队时间，这是用局部延迟换取系统代理整体可用性的预期行为。
- Hyper 内部是否复用现有 socket 不对外暴露，因此池化路径的全局许可约束的是“等待响应头的上游请求”，而不是精确的新 TCP 数；原始直连路径则精确限制建连。
- 本次不监听 macOS network path generation，也不主动清空网络切换前的活跃连接；接口切换感知可以作为后续独立优化，避免本次扩大平台特定范围。

## 验证计划

- `cargo test -p bifrost-proxy upstream_stability`
- `cargo test -p bifrost-proxy client_cache`
- 新增 `e2e-tests/tests/test_upstream_connection_stability.sh`
- 更新并执行 `human_tests/upstream-connection-stability.md`
- E2E 后执行 `rust-project-validate`、`cargo test --workspace --all-features`
- 推送后使用 fail-fast CI 看护，coverage 由远端 90% 门禁兜底
