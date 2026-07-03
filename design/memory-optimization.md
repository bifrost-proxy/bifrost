# 内存优化设计方案（Body 读取 / SQLite Cache / 流量存储）

## 背景

Bifrost 长期在开发机上驻留，内存增长是最容易被用户感知的痛点。历史上出现过三类真实事故：

1. 未知长度或极大的 request/response body 一次性读入内存，个别 POST / 大文件下载能把 RSS 顶到几个 G。
2. 流量与帧元数据用无界 `HashMap` + 常驻大对象缓存，长时间运行后不断增长。
3. SQLite 连接的 `cache_size` PRAGMA 沿用默认值，9 个连接叠加在一起理论上限 176 MB 页面缓存，配合 macOS malloc 高碎片率，空闲态 RSS 也能到 300 MB。

本文件汇总 body 读取消峰、SQLite cache_size 收敛、缓存 LRU 化、帧元数据落 DB、前端 SSE 上限 这一整套已经落地的优化，并给出后续要继续验证的点，避免文档退化为“提案汇编”。

## 用户目标验证清单

### 必须实现

- 未知长度或超过 `max_body_probe_size` 的 body 不会一次性读入内存，会走流式转发、跳过 body 规则/脚本。
- `max_body_probe_size` 支持通过运行时 config 覆盖，proxy fallback 默认 64 KB。
- Traffic DB / Frame DB / Replay DB 的 `cache_size` PRAGMA 收敛到 500 – 2000 页，读连接池收敛到 2 条。
- `frame_store.metadata_cache` 使用 `LruCache` 限容，不随连接数线性增长。
- 帧连接 metadata 持久化到 `frame_connection_metadata` 表，启动时按需从 DB 读，不再依赖散落的 `frames/*.meta.json` 预热。
- SSE 前端事件列表有 `MAX_SSE_EVENTS = 20_000` 上限，超出后按尾部截断。
- Traffic DB `recent_cache` 只缓存 summary（`TrafficSummaryCompact`），不缓存完整 `TrafficRecord`。
- 提供 `/debug/memory` 或等价诊断接口暴露 `rss_mb`、`frame_store.metadata_cache_len` 等指标供人工核对。

### 必须不破坏

- Body 规则、脚本、Traffic detail 展示对**小体积** body 仍完整生效。
- 流量列表分页/搜索/详情不因 `recent_cache` 缩减出现明显延迟增长。
- SQLite cache_size 缩减后不引起写入 QPS 下降 > 10%。
- 帧元数据 LRU 驱逐后能自动回退到 DB 查询，用户无感知。
- 前端 SSE 截断后仍保留最新事件，不能只保留最旧事件。

### 必须真实验证

- 单元测试覆盖 body probe 上限、metadata LRU 容量、traffic 缓存 summary 类型。
- 集成测试覆盖：大 body POST 不会撑爆 RSS、SSE 事件超上限时正确截断。
- 真实场景测试：`human_tests/memory-sqlite-cache-optimization.md` 记录空闲 RSS、metadata_cache_len。

## 产品语义

### Body 读取的三态

Bifrost 在处理 HTTP request/response body 时按顺序判断：

1. Content-Length 已知且 `<= max_body_probe_size` → 完整读入内存，参与 body 规则/脚本/detail 展示。
2. Content-Length 未知（chunked、SSE、streaming）→ 只读取 probe 窗口，超过后切流式转发。
3. Content-Length 已知但 `> max_body_probe_size` → 直接流式转发，Traffic detail 标记 body 被截断，避免误导用户。

### SQLite cache_size 与连接池

Bifrost 内置三套 SQLite 存储：`traffic.db`、frame `frame_store.db`、`replay.db`。每套都区分写连接（1 条）和读连接池（现降至 2 条）。cache_size 现状（`crates/bifrost-admin/src/traffic_db/schema.rs`、`crates/bifrost-admin/src/frame_store.rs`、`crates/bifrost-admin/src/replay_db/schema.rs`）：

| 连接 | cache_size (pages) | 备注 |
| --- | --- | --- |
| traffic.db 写 | 2000 (~8MB) | `schema.rs:50` 初始化 PRAGMA |
| traffic.db 读连接 | 1000 (~4MB) | `store.rs:53` |
| frame.db 写 | 1000 (~4MB) | `frame_store.rs:167` |
| frame.db 读 | 500 (~2MB) | `frame_store.rs:188` |
| replay.db 写 | 1000 (~4MB) | `replay_db/schema.rs:31` |
| replay.db 读 | 500 (~2MB) | `replay_db/store.rs:52` |

所有读连接都开 mmap（`mmap_size = 64MB/128MB`），页面 cache miss 由操作系统文件缓存兜底。

### Traffic / Frame 缓存约束

- `TrafficDbStore::recent_cache: RwLock<LruCache<String, TrafficSummaryCompact>>`，容量常量 `DEFAULT_CACHE_SIZE = 500`（`traffic_db/store.rs:25`）。**只**缓存 summary，用户点开 detail 时再回 DB 拉完整 record。
- `FrameStore::metadata_cache: RwLock<LruCache<String, FrameStoreMetadata>>`，容量与 `frame_connection_metadata` LRU 一致（`frame_store.rs:126`）。缓存驱逐后自动 fallback 到 DB 查询，延迟约 +0.1 ms。
- `frame_connection_metadata` 表（`FRAME_METADATA_TABLE`）持久化连接元信息，启动时不再读散落 JSON。

### 前端 SSE 与消息列表

- `web/src/components/TrafficDetail/panes/Messages/index.tsx:51` 定义 `MAX_SSE_EVENTS = 20_000`。
- `index.tsx:733-735`、`index.tsx:899-901` 两条 append 路径都在 `> MAX_SSE_EVENTS` 时 `slice(next.length - MAX_SSE_EVENTS)`，保留最新事件。
- 前端不承担完整 SSE 历史缓存，历史内容以 response body / `sse/stream` 后端读取链路为准。

## 技术细节

### Body probe 上限接线

- 配置字段：`bifrost-storage/src/unified_config.rs:410`（`max_body_probe_size: usize`，默认 `64 * 1024`）。
- 配置热更新入口：`bifrost-storage/src/config_manager.rs:325`。
- proxy 侧 fallback：`bifrost-proxy/src/server.rs:193 / 241`（默认 64 KB），运行时优先从 `admin_state.get_max_body_probe_size()` 读取。
- 覆盖点：
  - HTTP tunnel `bifrost-proxy/src/proxy/http/tunnel/mod.rs:797-815, 1107, 1229, 1357-1466`
  - SOCKS `bifrost-proxy/src/proxy/socks/tcp.rs:1708-1740, 2071-2182`
  - 主 server 中间件 `bifrost-proxy/src/server.rs:1720-1732`

### Frame metadata 持久化

- 表名常量：`crates/bifrost-admin/src/frame_store.rs:19` `FRAME_METADATA_TABLE = "frame_connection_metadata"`.
- `FrameStoreMetadata` 结构与 `LruCache` 上限、读连接 mmap 参数集中定义。
- API 层：`FrameStore::write_metadata` / `FrameStore::read_metadata`（`frame_store.rs:259`, `frame_store.rs:419-545`）先查 LRU，未命中再走 DB。

### `/debug/memory`（人工核对）

- 返回 JSON 包含 `rss_mb`、`frame_store.metadata_cache_len` 等字段（详见 `human_tests/memory-sqlite-cache-optimization.md`）。
- `metadata_cache_len` 断言：不超过 1000 上限。

## CLI / Web / Admin API

### Admin API（既有）

- `GET /_bifrost/api/config`：读取 `traffic.max_body_probe_size` 当前值。
- `PATCH /_bifrost/api/config`：更新 `max_body_probe_size`（`config_manager.rs:325`）。
- `GET /_bifrost/api/debug/memory`：诊断入口，输出 RSS 与关键缓存长度。

### CLI（既有）

- `bifrost config get traffic.max_body_probe_size`
- `bifrost config set traffic.max_body_probe_size <bytes>`（走 Admin API 落 `unified_config.json`）

### Web

- Settings -> Traffic 页展示当前 `max_body_probe_size`。
- Traffic detail 在 body 被截断时展示 `body truncated at <n> bytes` 提示，避免误导。

本次不新增外部 CLI 子命令或 Web 组件，仅收敛缓存与 body probe 语义。

## Sync 边界

- `max_body_probe_size` 属本机运行时配置，通过 `unified_config` 同步机制生效，不做跨设备同步。
- SQLite `cache_size`、LRU 容量、读连接数均为编译期常量，与其它设备无关。
- SSE 前端上限、Traffic recent cache 都是运行时状态，不持久化、不同步。

## Phase 1-4

### Phase 1：Body 读取消峰（已落地）

1. 新增 `max_body_probe_size` 配置字段并接入 `unified_config`。
2. proxy server / SOCKS / HTTP tunnel 全链路读取 admin_state 覆盖值，未初始化时使用编译默认 64 KB。
3. 单测断言 `max_body_probe_size: Some(910)` 生效（`config_manager.rs:1163-1180`）。

### Phase 2：SQLite cache_size 与连接池（已落地）

1. `traffic.db` / `frame.db` / `replay.db` PRAGMA `cache_size` 收敛到 500 – 2000 页。
2. traffic.db 读连接池从 4 降到 2。
3. 所有读连接开 mmap 兜底。
4. `human_tests/memory-sqlite-cache-optimization.md` 记录 6 条真实场景 TC。

### Phase 3：LRU 化与 metadata 落 DB（已落地）

1. `TrafficDbStore::recent_cache` 用 `LruCache<..., TrafficSummaryCompact>` 替换无界结构。
2. `FrameStore::metadata_cache` 用 `LruCache<..., FrameStoreMetadata>` 替换无界 HashMap。
3. 帧元数据落 `frame_connection_metadata` 表，替代散落 `frames/*.meta.json`。

### Phase 4：前端 SSE 上限与 detail 展示（已落地）

1. `MAX_SSE_EVENTS = 20_000` 常量与截断逻辑。
2. Traffic detail 面板对超过 probe 上限的 body 提示 “truncated”。
3. `/debug/memory` 输出 `rss_mb` 与 `metadata_cache_len` 便于人工比对。

## 测试方案

### 单元测试

| 位置 | 用例 | 断言 |
| --- | --- | --- |
| `crates/bifrost-storage/src/config_manager.rs:1163` | body probe patch | `patch.max_body_probe_size = Some(910)` 落到 config |
| `crates/bifrost-admin/src/traffic_db/store.rs` | `recent_cache` | 只缓存 `TrafficSummaryCompact`；LRU 上限 500 |
| `crates/bifrost-admin/src/frame_store.rs` | metadata LRU + fallback | 驱逐后能回退到 DB 读到相同结果 |
| `crates/bifrost-core/src/rule/resolver/tests.rs:336` | resolver LRU | 容量为 2 时旧条目被驱逐 |

### 集成测试

| 场景 | 断言 |
| --- | --- |
| 大 body POST（无 Content-Length） | 只读到 probe 上限，剩余流式转发；Traffic detail 标记截断 |
| 前端 SSE 超过 20 000 事件 | 列表保留最新 20 000 条，滚动不卡顿 |
| SQLite 收敛后写 QPS | 与旧值差异 < 10% |

### 真实场景

`human_tests/memory-sqlite-cache-optimization.md`（6 个 TC）覆盖：

- `TC-MEM-SQLITE-01`：空闲 30 min RSS 采样 < 200 MB。
- `TC-MEM-SQLITE-02`：`frame_store.metadata_cache_len` 上限断言 <= 1000。
- `TC-MEM-SQLITE-03`：读连接池 == 2。
- `TC-MEM-SQLITE-04`：大 body POST 不撑爆 RSS。
- `TC-MEM-SQLITE-05`：SSE 事件截断保留最新。
- `TC-MEM-SQLITE-06`：`/debug/memory` 报告字段完整。

## Review / Fix / Test 闭环

- **第 1 轮**：核对 body probe 全链路、SQLite PRAGMA 与 LruCache 容量、SSE 上限；跑 `cargo test -p bifrost-admin traffic_db::` 与 `cargo test -p bifrost-proxy tunnel::`。
- **第 2 轮**：基于最新 diff 复查 `human_tests/memory-sqlite-cache-optimization.md` 与 `human_tests/readme.md` 索引一致性；再跑受影响单测；采样 `/debug/memory` 记录。
- **第 3 轮（按需）**：如出现内存回退（30 min 空闲 RSS > 250 MB）追加轮次直至关闭。

## 校验要求

- 优先执行受影响单元与集成测试：
  - `cargo test -p bifrost-storage config_manager::`
  - `cargo test -p bifrost-admin traffic_db:: frame_store:: replay_db::`
  - `cargo test -p bifrost-proxy body_probe::`
- 再执行 `rust-project-validate`：fmt / clippy / `cargo test --workspace --all-features`。
- `scripts/ci/local-ci.sh` 仅在最终范围需要完整本地 CI 时执行。

## 风险与决策

| 风险 | 决策 |
| --- | --- |
| cache_size 降低导致查询 miss 率上升 | 依赖 mmap 与 OS 文件缓存兜底；实测延迟无感知（<0.1ms） |
| metadata LRU 驱逐引起短暂延迟抖动 | LRU 命中率高，miss 回退 DB 查询延迟 ~0.1ms，用户无感 |
| 大 body 截断影响脚本命中 | body 规则明确 skip；detail 展示提示，避免误判为“无 body” |
| 前端 SSE 截断保留最新，历史丢失 | 后端 body / `sse/stream` 是权威来源；前端仅做窗口展示 |
| 读连接池 4→2 导致高并发读阻塞 | 目前 Web/CLI 读并发实际 < 2，压测未见排队；如未来需要，可通过 config 再放宽 |

## 文档更新要求

- 更新 `human_tests/memory-sqlite-cache-optimization.md`（6 个 TC 与证据）。
- 更新 `human_tests/readme.md` 索引条目 `memory-sqlite-cache-optimization.md`。
- README / 协议 / Hook 文档无需修改；本次不引入新的用户 CLI 子命令。
