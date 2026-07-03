# TLS 证书有效期与 CA 一致性

## 背景

Bifrost 作为 MITM 代理，需要在运行时为每个抓包目标动态签发 TLS 叶子证书。历史实现有两处默认行为在实际使用中直接导致"用户信任根证书后仍被浏览器报证书错误"：

1. **叶子证书未设有效期**：`rcgen` 的默认 `CertificateParams` 有效期在浏览器（Chrome/Safari）看来不合规，导致 `NET::ERR_CERT_DATE_INVALID`。
2. **`load_root_ca()` 重新自签名**：加载磁盘 CA 时按 subject + 私钥重新生成 `Certificate`，序列号/时间/扩展字段与原始 PEM 不一致，用户已安装到系统信任链中的 CA 指纹与运行时签发链的 CA 指纹发生偏差，Chrome 判为 `NET::ERR_CERT_AUTHORITY_INVALID`。

本方案统一显式设置 root/leaf 有效期，并让 `load_root_ca()` 保留磁盘 PEM/DER 原始字节，确保运行时签发链根 CA 与用户已信任的 CA 是同一张证书。

## 用户目标验证清单

### 必须实现

- Root CA：`not_before = now - 1 day`，`not_after = now + 3650 days`（约 10 年）；显式设置在 `generate_root_ca()`。
- 动态叶子证书：`not_before = now - 1 day`，`not_after = now + 90 days`；显式设置在 `DynamicCertGenerator::generate_for_domain()`。
- `load_root_ca(cert_path, key_path)`：保留磁盘 PEM + DER 原始字节，不重新调用 `params.self_signed(&key_pair)`；也校验 cert 与 key 匹配。
- `save_root_ca()`：使用 `CertificateAuthority` 内保存的原始 PEM，不重新序列化。
- 动态叶子证书签发与链拼装统一使用原始 CA DER（`ca_issuer`）而非临时重建的证书对象。
- 叶子证书 CN/SAN 支持 DNS 名与 IP 地址（`SanType::DnsName` / `SanType::IpAddress`）。

### 必须不破坏

- 现有已生成的 root CA（磁盘上的 PEM/DER）不需要重新生成；升级后加载仍工作。
- `save/load` 往返不改变证书字节：`load(save(ca)).der() == ca.der()`。
- 现有 `bifrost-tls` 单元测试全部通过；`ca.rs` / `dynamic.rs` / `config.rs` / `install.rs` 不引入不兼容变更。
- 叶子证书重用逻辑（`leaf_keypair_pkcs8_der` / `leaf_signing_key`）保持不变——只在 keypair 缺失时才重新生成。

### 必须真实验证

- Chrome / Safari / Firefox 在信任 root CA 后访问抓包目标不再报 `ERR_CERT_DATE_INVALID` / `ERR_CERT_AUTHORITY_INVALID`。
- `openssl x509 -in ca.pem -noout -dates` 显示 `notBefore` = 昨天，`notAfter` ≈ 10 年后。
- `openssl s_client -connect target:443` 抓下的叶子证书 `notAfter` ≈ 90 天后，且 `Issuer` 指纹与磁盘 CA 一致。
- 卸载并重装 CA 后再启动 Bifrost，运行时签发的 leaf 链根节点仍与已安装的 CA 匹配。

## 产品语义

### 有效期选择原理

- **Root CA 3650 天**：CA 是用户手动信任的锚，重装成本高；10 年既足够长又不至于超过主流浏览器对根证书 `notAfter` 的合理容忍。
- **叶子 90 天**：Chrome/Apple 政策要求公开信任的叶子证书 ≤ 398/825 天；本地 MITM 场景取 90 天，符合 Let's Encrypt 惯例，也避免长期证书触发 Chrome 的 "certificate too long" 警告。
- **`not_before = now - 1 day`**：抵御客户端时钟略微偏后（如刚开机、NTP 未同步）导致的 "not yet valid" 错误。

### 一次生成、终身保留 CA 字节

- `generate_root_ca()` 一次生成 → 立即通过 `CertificateAuthority::new(der, pem, key_pair)` 保存原始 DER + PEM。
- `save_root_ca()` 写盘的就是 `CertificateAuthority::pem()`；不会因为字段顺序/时间戳/序列号差异导致新的字节。
- `load_root_ca()` 从磁盘读 PEM，解析后直接把 PEM/DER 原样封装进 `CertificateAuthority`；私钥仅用于签发叶子。
- 结果：签发叶子时使用的 `Issuer` 字段与用户安装的 CA 完全 byte-identical。

### 私钥匹配校验

`load_root_ca()` 在读入 cert + key 后校验二者匹配（用 key 签一段数据、用 cert 里的 pubkey 验签，或直接比较 pubkey），不匹配立即返回错误，避免"cert 是 A、key 是 B"的静默配置错误。

## 技术细节

### 关键源码

| 文件 | 责任 |
| --- | --- |
| `crates/bifrost-tls/src/ca.rs` | `CertificateAuthority` 结构；`generate_root_ca()` 显式设 3650 天有效期；`load_root_ca()` 保留原始 PEM/DER + key 匹配校验；`save_root_ca()` 使用原始 PEM |
| `crates/bifrost-tls/src/dynamic.rs` | `DynamicCertGenerator::generate_for_domain()` 显式设 90 天有效期；DNS/IP SAN；叶子 key 复用 |
| `crates/bifrost-tls/src/config.rs` | TLS 配置聚合入口 |
| `crates/bifrost-tls/src/install.rs` | 系统信任链安装（macOS keychain / Windows certutil / Linux ca-trust） |
| `crates/bifrost-tls/src/cache.rs` | 域名 → cert 缓存 |
| `crates/bifrost-tls/src/sni.rs` | SNI 解析与 hostname 归一 |

### `CertificateAuthority`

```rust
pub struct CertificateAuthority {
    pub der: CertificateDer<'static>,   // 磁盘原始 DER
    pub pem: String,                     // 磁盘原始 PEM
    pub key_pair: KeyPair,               // 用于签发叶子
}

impl CertificateAuthority {
    pub fn new(der: CertificateDer<'static>, pem: String, key_pair: KeyPair) -> Self { ... }
    pub fn der(&self) -> &CertificateDer<'static> { &self.der }
    pub fn pem(&self) -> &str { &self.pem }
}
```

### `generate_root_ca()` 关键片段

```rust
params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
params.key_usages = vec![KeyCertSign, CrlSign, DigitalSignature];
params.extended_key_usages = vec![ServerAuth, ClientAuth];
params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
params.not_after  = OffsetDateTime::now_utc() + Duration::days(3650);
let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
let cert = params.self_signed(&key_pair)?;
Ok(CertificateAuthority::new(cert.der().to_vec().into(), cert.pem(), key_pair))
```

### `generate_for_domain()` 关键片段

```rust
params.subject_alt_names = vec![SanType::DnsName(dns) | SanType::IpAddress(ip)];
params.key_usages = vec![DigitalSignature, KeyEncipherment];
params.extended_key_usages = vec![ServerAuth, ClientAuth];
params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
params.not_after  = OffsetDateTime::now_utc() + Duration::days(90);
// 优先复用 self.leaf_keypair_pkcs8_der / self.leaf_signing_key
// 否则 KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
// 用 self.ca_issuer(原始 CA DER + key) 签发
```

### `load_root_ca()` 关键片段

```rust
let cert_pem = fs::read_to_string(cert_path)?;
let key_pem  = fs::read_to_string(key_path)?;
let pem = parse_x509_pem(cert_pem.as_bytes())?.1;
let der = CertificateDer::from(pem.contents.clone());
let key_pair = KeyPair::from_pem(&key_pem)?;
verify_key_matches_cert(&der, &key_pair)?;   // 私钥匹配校验
Ok(CertificateAuthority::new(der, cert_pem, key_pair))
```

不再调用 `params.self_signed(&key_pair)`；原始 PEM/DER 全程保留。

## CLI + Web + Admin API

本改动是 TLS 内部行为修复，不新增 CLI / Web / Admin API。间接受益：

- `bifrost cert info` 显示 root CA `notAfter` 为 10 年后，leaf 为 90 天后。
- `bifrost cert regen` 走 `generate_root_ca()`，新 CA 立即符合有效期规范。
- Admin `/api/tls/ca` 返回的证书元数据与磁盘 byte-identical。
- Web Settings → Certificate 页面显示的 CA 指纹与用户在系统 keychain 中看到的一致。

## Sync 边界

CA 是每台设备的信任锚，不参与 Sync / 导入导出。只有本机 `bifrost cert install` 才会把 CA 装进系统 keychain。叶子证书完全动态生成，也不同步。

## Phase 1-4

### Phase 1：叶子证书显式有效期

- `DynamicCertGenerator::generate_for_domain()` 显式设 `not_before/not_after`（-1d / +90d）。
- 补 `test_generate_for_domain_has_browser_safe_validity_period`。

### Phase 2：Root CA 显式有效期

- `generate_root_ca()` 显式设 -1d / +3650d。
- 补 `test_generate_root_ca_has_reasonable_validity_period`。

### Phase 3：load_root_ca 保留原始字节

- `CertificateAuthority` 增加 `der` + `pem` 字段。
- `load_root_ca()` 不再重签，直接封装磁盘字节。
- 补 `test_load_root_ca_preserves_original_certificate` 断言 `load(save(x)).der() == x.der()`。
- `save_root_ca()` 使用 `CertificateAuthority::pem()`。

### Phase 4：私钥匹配校验 + 集成测试

- `load_root_ca()` 加入 cert/key 匹配校验，不匹配返回 `BifrostError::Tls`。
- 补 `test_load_root_ca_rejects_mismatched_key`。
- E2E 验证浏览器信任 root CA 后不再报证书错误。

## 测试方案

### 单元测试（`cargo test -p bifrost-tls`）

- `test_generate_root_ca` — 生成 CA 不 panic。
- `test_generate_root_ca_has_reasonable_validity_period` — `notBefore` 在过去 2 天内、`notAfter` 在 3649~3651 天后。
- `test_save_and_load_root_ca` — save → load 往返，`der/pem` 完全一致。
- `test_load_root_ca_preserves_original_certificate` — 手动构造 CA PEM，`load` 后 `der/pem` byte-identical。
- `test_load_root_ca_rejects_mismatched_key` — cert 是 A、key 是 B 时返回错误。
- `test_load_root_ca_invalid_pem_errors` — 非法 PEM 返回错误。
- `test_validate_ca_files_accepts_rsa_pkcs8_ca` — RSA PKCS8 CA 可加载。
- `test_validate_ca_files_missing_files` — 缺文件返回错误。
- `test_validate_ca_files_rejects_non_ca_cert` — 非 CA cert 被拒。
- `test_ensure_valid_ca_missing_returns_false` / `test_ensure_valid_ca_valid_returns_true` / `test_ensure_valid_ca_invalid_removes_files` — `ensure_valid_ca` 分支覆盖。
- `test_cert_info_validity_helpers` — `notBefore/notAfter` 解析工具函数。
- `test_generate_for_domain_has_browser_safe_validity_period` — leaf `notAfter - notBefore` 在 80~100 天区间（避免时钟抖动 flake）。
- `test_generate_for_domain_invalid_dns_name_errors` — 非法 DNS 名返回错误。
- `test_generate_for_domain_uses_fallback_leaf_keypair_when_cache_missing` — 缺 keypair 时 fallback 生成。

### E2E 测试

- `crates/bifrost-e2e/src/tests/tls.rs`（或对应 shell）：真实浏览器/curl 抓 MITM 证书，断言 `notAfter` 90 天、Issuer 与磁盘 CA byte-identical。
- 现有 TLS 相关 E2E 全量回归（`--suite rules` 覆盖 HTTPS 抓包路径）。

### human_tests

- 安装 CA 后 Chrome/Safari/Firefox 访问 HTTPS 目标不报证书错误。
- 主机时钟调后 1 小时仍不触发 "not yet valid"。
- `openssl x509 -in ca.pem -noout -dates` 输出符合预期。
- `openssl s_client -connect example.com:443 -showcerts` 抓下的 leaf `notAfter` 与 Issuer 指纹符合预期。

### 项目校验

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p bifrost-tls
cargo test --workspace --all-features
cargo build --workspace --all-targets --all-features
```

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 diff：`crates/bifrost-tls/src/{ca.rs,dynamic.rs,config.rs}`；确认所有 `params.self_signed` 调用点都传入 `not_before/not_after`。
- 重点 review：`load_root_ca` 是否真的不再重签；`save/load` 往返是否 byte-identical；私钥匹配校验错误消息是否可定位；leaf 有效期是否在 80~100 天 test range 内（避免时钟 flake）。
- 复测：`cargo test -p bifrost-tls`；`openssl x509 -in` 手工验证。

### 第 2 轮

- 复核第 1 轮修复。
- 再次 `git diff` `bifrost-tls`；`bifrost-e2e` 相关 tls 用例通过。
- 重点 review：`ensure_valid_ca` 是否遇到过期 CA 会自动重生（而非静默使用）；`install.rs` 对 macOS keychain / Windows certutil 是否使用 `pem()` 字段（不是重签的字节）。
- 复测：真实浏览器信任链回归；多机（Windows / macOS / Linux）安装 CA 后抓 HTTPS 无警告。

## 风险与决策

- **旧 CA 兼容**：升级前生成的 CA 无 `notBefore/notAfter` 显式设置（rcgen 默认给了一个可能不合规的值），但已在磁盘上。用户升级 Bifrost 后 `load_root_ca` 会直接使用旧 PEM，行为不变；用户若发现旧 CA 报错，可 `bifrost cert regen` 生成新 CA。
- **叶子 90 天 vs 更短**：太短（如 7 天）会导致每次重启缓存全失效；90 天符合业界惯例，也在浏览器容忍范围内。
- **时钟偏差**：`not_before = now - 1 day` 是最小抵御时钟微前置的方式；不再多减是避免 CA 在装机后立即被系统 keychain 认为"有效期开始时间过早"报警。
- **CA `notAfter` 3650 天**：某些浏览器/OS 对根证书没有严格 `notAfter` 上限，10 年是保守选择；到期后 `ensure_valid_ca` 会检测出并触发 `bifrost cert regen` 提示。
- **重新签发导致的字节漂移**：曾经的 `load_root_ca` 用 `subject + key + params` 重签，`self_signed` 内部随机 serial + 当前时间戳导致每次结果不同，浏览器已装的 CA 与运行时签发链不匹配。本次修复的核心就是消除这种字节漂移。
- **key 匹配校验开销**：`load_root_ca` 启动路径上做一次 sign+verify，是 ms 级别，不影响启动时延。

## 依赖项

- `rcgen`（`CertificateParams` / `KeyPair::generate_for` / `PKCS_ECDSA_P256_SHA256`）。
- `rustls` + `rustls-pki-types`（`CertificateDer` / `PrivatePkcs8KeyDer`）。
- `x509-parser`（`parse_x509_pem` / cert 元信息解析）。
- `time`（`OffsetDateTime` + `Duration` 计算相对当前时间的有效期）。
- 系统 keychain / cert 存储由 `crates/bifrost-tls/src/install.rs` 负责，不在本文件范围。
