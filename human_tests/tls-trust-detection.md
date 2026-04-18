# TLS 信任检测改进 — 降低误伤率

## 功能模块说明

Bifrost 代理在进行 TLS MITM 拦截时，会使用自签名 CA 证书替换目标域名证书。当客户端不信任 Bifrost CA 时，TLS 握手会失败。`ClientTlsTrustTracker` 模块负责追踪这些握手结果，判断客户端的信任状态，并通过通知系统告知用户。

本次改进核心目标：**降低误伤率**，避免将非 CA 信任相关的握手失败错误地标记为"客户端不信任 CA"。

### 改进点

1. **错误分类精细化**：
   - `UnknownCA` → `ClientDoesNotTrustCa`（确定性不信任）
   - `BadCertificate` / `CertificateUnknown` → `ProbablyClientDoesNotTrustCa`（可能性不信任，降级处理）
   - `decrypt` 类错误 → `DecryptionError`（非信任相关，不计入 untrust）

2. **状态判定门槛提升**：
   - 新增 `PossiblyNotTrusted` 中间状态（1 次确定性失败 + 0 次成功）
   - `NotTrusted` 需要至少 2 次确定性失败（`MIN_DEFINITE_FOR_NOT_TRUSTED = 2`）
   - `LikelyUntrusted` 仅基于比率推断（需 ≥3 样本 + >70% 失败率）

3. **Per-Domain 追踪**：记录每个失败域名的确定性/可能性失败次数及是否有成功记录

## 前置条件

```bash
# 启动 Bifrost 代理服务（TLS 拦截启用，使用临时数据目录）
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

- 管理端 Web UI 地址：`http://localhost:8800/_bifrost/`
- 确保 TLS 拦截已启用（Settings → TLS → Enable TLS Interception = true）
- 准备 curl 命令用于模拟代理请求

## 测试用例

### TC-TD-01：decrypt 类错误不应计入 untrust

**步骤**：
1. 启动 Bifrost 服务
2. 使用 curl 通过代理发起 HTTPS 请求，使用 `--tls-max 1.1` 强制低版本 TLS 触发协议不兼容或 decrypt 类错误：
   ```bash
   curl -x http://localhost:8800 --tls-max 1.1 https://httpbin.org/get 2>&1 || true
   ```
3. 多次重复以上请求（至少 3 次）
4. 查询信任状态 API：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/notifications/tls-trust | python3 -m json.tool
   ```

**预期结果**：
- 该客户端 IP 的 `handshake_fail_definite_untrust` 为 0
- 该客户端 IP 的 `handshake_fail_probable_untrust` 为 0
- `handshake_fail_other` > 0（decrypt/协议类错误计入 other）
- `trust_status` 为 `unknown`（非 untrust 相关错误不影响信任判定）

### TC-TD-02：单次 UnknownCA 失败应进入 PossiblyNotTrusted 而非 NotTrusted

**步骤**：
1. 清理之前的状态（重启 Bifrost 或调用 clear API）
2. 使用 curl 不信任 CA 证书发起一次 HTTPS 请求（触发 UnknownCA alert）：
   ```bash
   curl -x http://localhost:8800 https://httpbin.org/get 2>&1 || true
   ```
3. 查询信任状态：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/notifications/tls-trust | python3 -m json.tool
   ```

**预期结果**：
- 该客户端 IP 的 `handshake_fail_definite_untrust` 为 1
- `trust_status.status` 为 `possibly_not_trusted`（而非 `not_trusted`）
- `trust_status.reason` 包含失败原因描述

### TC-TD-03：2 次 UnknownCA 失败应进入 NotTrusted

**步骤**：
1. 在 TC-TD-02 的基础上，再发起一次不信任 CA 的 HTTPS 请求：
   ```bash
   curl -x http://localhost:8800 https://httpbin.org/get 2>&1 || true
   ```
2. 查询信任状态：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/notifications/tls-trust | python3 -m json.tool
   ```

**预期结果**：
- `handshake_fail_definite_untrust` 为 2
- `trust_status.status` 为 `not_trusted`
- `trust_status.reason` 包含失败原因

### TC-TD-04：成功握手后恢复为 Trusted

**步骤**：
1. 在 TC-TD-03 的基础上（客户端已标记为 not_trusted）
2. 导出并安装 Bifrost CA 证书：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/cert/ca -o /tmp/bifrost-ca.pem
   ```
3. 使用信任 CA 的方式发起请求：
   ```bash
   curl -x http://localhost:8800 --cacert /tmp/bifrost-ca.pem https://httpbin.org/get
   ```
4. 查询信任状态

**预期结果**：
- `handshake_success` ≥ 1
- `trust_status.status` 为 `trusted`（成功握手在失败之后，最新状态以最后成功为准）

### TC-TD-05：BadCertificate 错误归类为 probable 而非 definite

**步骤**：
1. 重启 Bifrost 清理状态
2. 检查 `cargo test -p bifrost-admin -- classify_bad_certificate` 单元测试的通过情况
3. 查看 `classify_tls_accept_error` 对 BadCertificate 的分类结果

**预期结果**：
- 单元测试 `classify_bad_certificate_as_probable` 通过
- BadCertificate 被分类为 `ProbablyClientDoesNotTrustCa`（非 `ClientDoesNotTrustCa`）

### TC-TD-06：仅有 probable 失败时需要足够样本和高比率才进入 LikelyUntrusted

**步骤**：
1. 验证单元测试 `probable_only_below_threshold_stays_unknown`：
   ```bash
   cargo test -p bifrost-admin -- probable_only_below_threshold_stays_unknown
   ```
2. 验证单元测试 `probable_only_reaches_likely_untrusted_at_threshold`：
   ```bash
   cargo test -p bifrost-admin -- probable_only_reaches_likely_untrusted_at_threshold
   ```

**预期结果**：
- 2 次 probable 失败 + 0 次成功 = `unknown`（样本不足 3）
- 3 次 probable 失败 + 0 次成功 = `likely_untrusted`（达到阈值：3 样本 + 100% > 70%）
- `LikelyUntrusted` 的 `confidence` 为 1.0，`sample_count` 为 3

### TC-TD-07：per-domain 追踪正确记录失败域名详情

**步骤**：
1. 重启 Bifrost 清理状态
2. 对不同域名发起不信任 CA 的请求：
   ```bash
   curl -x http://localhost:8800 https://httpbin.org/get 2>&1 || true
   curl -x http://localhost:8800 https://example.com/ 2>&1 || true
   ```
3. 查询信任状态：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/notifications/tls-trust | python3 -m json.tool
   ```

**预期结果**：
- `failed_domains` 数组包含 2 个条目
- 每个条目包含 `domain`、`definite_count`、`probable_count`、`has_success` 字段
- `httpbin.org` 和 `example.com` 各自有独立的失败计数

### TC-TD-08：WebUI Notifications 页面正确展示新状态

**步骤**：
1. 在有 TLS 信任事件数据的情况下，打开 `http://localhost:8800/_bifrost/` 并导航到 Notifications 页面
2. 查看 TLS Trust 表格中的 Status 列和 Fail (Untrust) 列

**预期结果**：
- `possibly_not_trusted` 状态显示为蓝色 Tag，文本 "Possibly Not Trusted"，hover 时显示 Tooltip 说明
- `not_trusted` 状态显示为红色 Tag
- Fail (Untrust) 列：hover 时显示 Tooltip 展示 "Definite: X, Probable: Y" 的细分数据

### TC-TD-09：28 个单元测试全部通过

**步骤**：
```bash
cargo test -p bifrost-admin -- client_trust_tracker
```

**预期结果**：
- 28 个测试全部通过（test result: ok. 28 passed）
- 覆盖场景包括：
  - 错误分类（UnknownCA/BadCertificate/CertificateUnknown/decrypt/expired/reset/unknown）
  - 信任状态流转（unknown → possibly_not_trusted → not_trusted → trusted）
  - 最小样本阈值
  - probable-only 推断逻辑
  - per-domain 追踪
  - 事件发送与订阅
  - 混合失败类型

### TC-TD-10：clippy 无警告通过

**步骤**：
```bash
SKIP_FRONTEND_BUILD=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**预期结果**：
- 退出码 0，无任何 clippy 警告

## 清理步骤

```bash
# 停止测试服务
# Ctrl+C 终止运行中的 Bifrost 进程

# 清理临时数据目录
rm -rf ./.bifrost-test
```
