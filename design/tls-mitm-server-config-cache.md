# TLS MITM ServerConfig 缓存

## 背景

Bifrost 的 MITM 路径已经有 `CertCache` 按域名复用动态证书,但 `rustls::ServerConfig` 仍在每次新连接现建:

- HTTP CONNECT MITM: 每次拦截连接都会重新构建 `ServerConfig` 并设置 ALPN。
- SOCKS5 TLS MITM: 每次拦截连接同样重建。

`ServerConfig` 内含 crypto provider、cipher suite 排列、SNI resolver 引用、ALPN protocol 列表等,构造成本主要来自 Arc 装箱与 ALPN 拷贝。虽然单次构造不算慢,但在高频短连场景(测试脚本、爬虫、批量 API 调用)会累积可见的 CPU 与短期内存抖动。

本次将 `ServerConfig` 也纳入共享缓存,粒度为 `域名 + ALPN 协议列表`,让 HTTP CONNECT MITM 与 SOCKS5 TLS MITM 复用同一份 `Arc<ServerConfig>`。

## 用户目标验证清单

### 必须实现

- 在 `crates/bifrost-tls` 新增 `ServerConfigCache`,基于 LRU (`lru` crate) + `Mutex` (`parking_lot` 或 `std`) 存 `Arc<rustls::ServerConfig>`。
- 缓存 key 必须由 `domain` 与 `alpn_protocols` (`Vec<Vec<u8>>`) 联合构成,不能只按 domain,以免 H2 场景与无 ALPN 场景错误复用。
- 默认容量与现有 `CertCache` 一致,推荐 1000,可通过 `with_capacity` 调整。
- `SniResolver` 内挂载 `CertCache` + `ServerConfigCache`,对外统一暴露 `resolve_server_config` / `resolve_server_config_with_alpn` / `clear_cache`。
- `TlsConfig::resolve_server_config(server_name, alpn_protocols)` 在 `sni_resolver` 存在时直接走缓存,不再手工组装。
- `clear_cache()` 必须同时清 `CertCache` 与 `ServerConfigCache`(单点入口保证 CA 轮换/删除后彻底失效)。
- 保留 `cert_generator` 回退路径,兼容测试与未启用 `SniResolver` 的场景。

### 必须不破坏

- MITM 证书生成、SNI 分派、ALPN 选择、TLS 版本协商行为不变。
- `SingleCertResolver`(测试用固定证书)行为不变。
- `TlsConfig::resolve_server_config` 的既有调用点(HTTP CONNECT MITM、SOCKS5 MITM)按各自 ALPN 传参,不改变 ALPN 值。
- `clear_cache` 在 CA 变更/清空时被调用的时机保持不变。
- LRU 淘汰不能阻塞异步任务,`get`/`insert` 是 O(1) 短锁。

### 必须真实验证

- 使用真实浏览器多次访问 `https://example.test` 命中同一域名,第二次起构建耗时(可以打 tracing)显著低于首次。
- 使用 SOCKS5 客户端与 HTTP CONNECT 客户端交替访问同一域名,`server_config_cache_len()` 保持 1 (相同 ALPN)。
- CA 重置后 `clear_cache()` 触发,新请求可以重建证书与 ServerConfig。

## 产品语义

- Key 语义: `(domain, alpn_protocols_sorted_stable)`;当前实现在 `ServerConfigCacheKey::new` 内直接使用 `Vec<Vec<u8>>` 拷贝,依赖调用方保证 ALPN 顺序稳定。HTTP CONNECT MITM 固定传 `["h2", "http/1.1"]`,SOCKS5 传 `&[]`,顺序一致即可稳定命中。
- 无 ALPN (`&[]`) 与有 ALPN 的 config 分别独立缓存,防止 SOCKS5 与 HTTP CONNECT 误共享。
- 缓存粒度不区分客户端 fingerprint、cipher preference。若未来引入按客户端定制的 `ServerConfig`,需要扩展 key。

## 技术细节

### `ServerConfigCache`

`crates/bifrost-tls/src/cache.rs`:

```rust
struct ServerConfigCacheKey {
    domain: String,
    alpn: Vec<Vec<u8>>,
}

pub struct ServerConfigCache {
    cache: Mutex<LruCache<ServerConfigCacheKey, Arc<ServerConfig>>>,
}

impl ServerConfigCache {
    pub fn new() -> Self { /* default capacity */ }
    pub fn with_capacity(cap: usize) -> Self { /* ... */ }
    pub fn get(&self, domain: &str, alpn: &[Vec<u8>]) -> Option<Arc<ServerConfig>>;
    pub fn insert(&self, domain: &str, alpn: &[Vec<u8>], config: Arc<ServerConfig>);
    pub fn clear(&self);
    pub fn len(&self) -> usize;
}
```

`Debug` 单独实现,避免打印内部 LRU 数据。

### `SniResolver` 集成

`crates/bifrost-tls/src/sni.rs`:

```rust
pub struct SniResolver {
    cert_generator: DynamicCertGenerator,
    cert_cache: CertCache,
    server_config_cache: ServerConfigCache,
}

impl SniResolver {
    pub fn resolve_server_config(&self, server_name: &str) -> Result<Arc<ServerConfig>> {
        self.resolve_server_config_with_alpn(server_name, &[])
    }

    pub fn resolve_server_config_with_alpn(
        &self,
        server_name: &str,
        alpn_protocols: &[Vec<u8>],
    ) -> Result<Arc<ServerConfig>> {
        if let Some(config) = self.server_config_cache.get(server_name, alpn_protocols) {
            return Ok(config);
        }
        let cert_key = self.resolve(server_name)?; // 命中 CertCache 或新建
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertResolver { cert_key: cert_key.clone() }));
        config.alpn_protocols = alpn_protocols.to_vec();
        let config = Arc::new(config);
        self.server_config_cache.insert(server_name, alpn_protocols, config.clone());
        Ok(config)
    }

    pub fn clear_cache(&self) {
        self.cert_cache.clear();
        self.server_config_cache.clear();
    }
}
```

- `cache_len()` 返回 `cert_cache.len()`,新增 `server_config_cache_len()` 供测试与运维观察。
- `ResolvesServerCert` trait 实现仍走 `cert_cache` 路径,给 rustls 分派 SNI 使用。

### `TlsConfig::resolve_server_config`

`crates/bifrost-proxy/src/server.rs`:

```rust
pub fn resolve_server_config(
    &self,
    server_name: &str,
    alpn_protocols: &[Vec<u8>],
) -> Result<Arc<ServerConfig>> {
    if let Some(sni_resolver) = &self.sni_resolver {
        return sni_resolver.resolve_server_config_with_alpn(server_name, alpn_protocols);
    }
    // fallback: 直接用 cert_generator 现建,不缓存
    // ...
}
```

调用点:

- HTTP CONNECT MITM: 传 `[b"h2".to_vec(), b"http/1.1".to_vec()]`。
- SOCKS5 TLS MITM: 传 `&[]`。

### 依赖

- `lru` crate: 复用现有版本,与 `CertCache` 共用。
- `parking_lot` (可选): 若已引入则复用,否则用 `std::sync::Mutex`。
- 复用 `DynamicCertGenerator`、`CertCache`、`SingleCertResolver`。

## CLI + Web + Admin API

- 无新增 CLI 参数、无新增 Admin API 字段。
- `bifrost ca clear` 或规则变更触发的 CA 重置路径最终调用 `SniResolver::clear_cache()`,同时清 cert 与 ServerConfig。
- 内部可用 `bifrost status --format json` 扩展诊断字段(可选),暴露 `sni_resolver.server_config_cache_len` 便于运维。

## Sync 边界

- 纯内存缓存,不落盘、不 sync。
- CA/证书变更由现有路径触发 `clear_cache()`,保证不出现 stale ServerConfig。

## Phase 1: `ServerConfigCache` 类型

- 在 `crates/bifrost-tls/src/cache.rs` 增加 struct 与 API。
- 单元测试覆盖 `new`/`with_capacity`/`get`/`insert`/`clear`/`Debug`。

## Phase 2: `SniResolver` 集成

- 引入 `server_config_cache` 字段。
- 实现 `resolve_server_config` / `resolve_server_config_with_alpn`。
- `clear_cache` 同时清两级缓存。
- 单元测试 (`test_resolve_server_config_cached`, `test_resolve_server_config_with_alpn`, `test_clear_cache`)。

## Phase 3: 代理侧接入

- `TlsConfig::resolve_server_config` 优先走 `sni_resolver`。
- HTTP CONNECT MITM 与 SOCKS5 TLS MITM 都调用统一入口。
- 保留无 sni_resolver 的 fallback 路径。

## Phase 4: 验证与文档

- 通过 Rust 单测 + 现有 TLS MITM E2E 验证行为不变。
- 更新 design 文档,补充缓存 key/LRU 策略说明。
- 无 README/CLI 文档变更。

## 测试方案

### 单元测试

- `crates/bifrost-tls/src/cache.rs`:
  - `test_server_config_cache_insert_and_get`
  - `test_server_config_cache_alpn_key_isolation` (相同 domain 不同 ALPN 独立)
  - `test_server_config_cache_default_and_with_capacity`
  - `test_server_config_cache_with_capacity_zero` (退化行为)
  - `test_server_config_cache_debug_render`
- `crates/bifrost-tls/src/sni.rs`:
  - `test_resolve_server_config`
  - `test_resolve_server_config_cached` (相同 domain 无 ALPN 返回同一 Arc)
  - `test_resolve_server_config_with_alpn` (相同 ALPN 命中,不同 ALPN 隔离)
  - `test_clear_cache` (同时清两级)
- `crates/bifrost-proxy/src/server.rs`:
  - `test_tls_config_resolve_server_config_reuses_sni_cache`

### 集成/E2E 测试

- `crates/bifrost-e2e/src/tests/tls_switch_test.rs`: 覆盖 TLS 拦截切换。
- `crates/bifrost-e2e/src/tests/tls_config_disconnect.rs`: 覆盖 CA 变更后连接重建。
- `e2e-tests/tests/test_tls_intercept_e2e.sh`: MITM 端到端。
- `e2e-tests/tests/test_tls_intercept_mode_api.sh`: mode 切换。
- 手工验证同一域名多次请求 `server_config_cache_len == 1`。

### 真实场景

- Chrome 反复访问 `https://example.test` (h2) 与另一域名 (`https://legacy.test`, h1),观察 cache 命中率。
- SOCKS5 客户端 (`curl --socks5`) 与 HTTP CONNECT 客户端交替访问同一域名,ALPN 差异导致缓存独立但不错误共享。
- CA 清空后再次访问,构建新 ServerConfig,缓存重新填充。
- 使用 `--no-system-proxy`、临时数据目录、非 9900 端口、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核: cache key 是否稳定; ALPN Vec 顺序是否被调用方保持一致。
- 复核: `clear_cache` 是否覆盖所有触发点 (CA 变更、`bifrost ca reset`、SNI resolver 重建)。
- 复测: `cargo test -p bifrost-tls`、`cargo test -p bifrost-proxy tls_config_resolve_server_config`、TLS MITM E2E 脚本。

### 第 2 轮

- 检查在并发高压下 `Mutex<LruCache>` 是否成为瓶颈;必要时评估 `RwLock` 或分段锁。
- 检查 `Arc<ServerConfig>` 强引用是否会拖延 CA 撤销后的清理(clear_cache 是否足够)。
- 复测 stress 场景、`cargo test --workspace --all-features`。

## 校验要求

- 先执行本模块相关 E2E / 单测。
- 再执行 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、按修改范围 `cargo test`、`cargo build --all-targets --all-features` (交由 CI 或本地手动)。
- 本地约定 no-local-coverage,不跑 `make coverage`;实现阶段可用 `make coverage-unit` 单元覆盖。

## 风险与决策

- 决策: cache 粒度按 `(domain, ALPN)`,不引入 fingerprint。原因: 当前 MITM 不做客户端指纹伪装,再细分只会浪费内存与命中率。
- 决策: `Vec<Vec<u8>>` 作为 key 直接持有拷贝。原因: `Bytes` 或 `Arc<[u8]>` 收益有限,ALPN 列表通常只有 1-2 项。
- 风险: `SingleCertResolver` 与 `ResolvesServerCert` 双路径可能出现不同步。缓解: `clear_cache` 是唯一失效入口,双路径都被清空。
- 风险: 若未来新增 rustls provider(例如 `aws-lc-rs` vs `ring`),缓存里的旧 `ServerConfig` 可能持有旧 provider 引用。缓解: 切换 provider 时显式调用 `clear_cache`。
- 风险: `Arc<ServerConfig>` 被 rustls 长期持有,LRU 淘汰不能立刻回收内存。缓解: LRU 容量按 domain 数量估计,默认 1000 足够常见场景。
