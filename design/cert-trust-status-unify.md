# 证书安装与信任状态统一检测

## 背景

Bifrost 的 HTTPS 抓包依赖本机受信任的 CA 证书。历史上管理端 `/api/cert/info` 只返回 `available` 字段，仅代表 `ca.crt` 文件在数据目录内可下载；设置页也长期把 `available=true` 直接展示为 “证书可用 / 已配置”。这与实际的系统信任状态存在多种偏差：

- 用户下载了 `ca.crt` 但从未导入系统钥匙串，仍会看到“证书可用”。
- 系统钥匙串里存在同名证书但被明确设为“绝不信任”，仍返回 `available=true`。
- macOS 上 `security` / `openssl` 输出格式差异（LibreSSL vs OpenSSL、大小写、冒号分隔、`SHA256 Fingerprint=` 前缀）导致指纹解析异常时静默走 fallback，把已信任证书误报为 `unknown` 甚至 “可下载”。

CA 状态是抓包链路正确性的前提；一旦上游误判为“正常”，用户会花大量时间排查 TLS 错误，却始终看不到问题在信任配置本身。本方案统一 CA 状态语义，把“文件是否可下载”与“系统是否信任”彻底解耦，并在 macOS 上补齐指纹解析容错。

## 用户目标验证清单

### 必须实现

- `/api/cert/info` 返回四态：`not_installed / installed_not_trusted / installed_and_trusted / unknown`，同时给出 `installed / trusted` 布尔与人类可读的 `status_label`、`status_message`。
- `available` 字段保留，仅表示 `ca.crt` 文件是否可下载，不再承担“系统信任”语义。
- macOS 上无论 `openssl` / `security` 输出格式如何，只要能取到 hex fingerprint 就必须解析成功，避免因分隔符/大小写差异被判为 `unknown`。
- 设置页证书卡片以四态展示，`available` 只驱动下载按钮与二维码；下载入口在 `not_installed / installed_not_trusted` 场景保持可点。
- `unknown` 场景必须显式提示“检测失败”，不允许沿用旧的 “已可用” 视觉。
- CLI 的 `bifrost ca status` 输出与 API 状态保持一致，便于脚本消费。

### 必须不破坏

- `bifrost-tls` 现有 `CertStatus` 三态语义、CA 生成、安装、卸载流程不变。
- Linux / Windows 检测路径（`check_status_linux` / `check_status_windows`）行为不变，只做适配层扩展。
- 现有 `available` 字段消费方（下载/二维码/远端拉取）不受影响。
- 前端设置页原有 UI 结构、亮暗主题、文案国际化位置保持稳定。

### 必须真实验证

- 无 `ca.crt` 场景：API 返回 `not_installed`，设置页显示未安装提示，下载按钮可用。
- 已下载未信任场景：手工把 `ca.crt` 从系统钥匙串移除后，API 返回 `installed_not_trusted`。
- 已信任场景：安装并信任后，API 返回 `installed_and_trusted`，设置页显示绿色状态。
- 检测失败场景：mock `security find-certificate` 返回错误码，API 返回 `unknown` 且 `status_message` 明确原因。
- macOS 指纹兼容：分别 mock `SHA256 Fingerprint=AA:BB:...`、`sha256 Fingerprint = aabb...`、`SHA256 Fingerprint AA BB` 三种格式，`current_cert_fingerprint_macos` 必须都能规范化为同一 hex。

## 产品语义

### 状态四元组

| status | installed | trusted | 典型场景 | UI |
|--------|-----------|---------|----------|----|
| `not_installed` | false | false | 数据目录无 `ca.crt`，或系统钥匙串无匹配 | 未安装，展示下载/安装引导 |
| `installed_not_trusted` | true | false | 已导入但被设置为“绝不信任” 或未标记信任 | 引导用户前往系统偏好完成信任 |
| `installed_and_trusted` | true | true | 已导入且信任 | 绿色状态，显示指纹尾 8 位 |
| `unknown` | false | false | 检测命令失败、平台不支持 | 显式检测失败，附错误 message |

`available` 仅表示 `data_dir/ca.crt` 存在且可通过 `/api/cert/download` 拉到。它与四态相互独立，不再进入 UI 状态判定。

### 文案与信号分离

- 状态标签（`status_label`）：面向用户的短文案，可国际化。
- 状态说明（`status_message`）：具体原因，允许包含错误摘要（不含敏感路径）。
- 下载按钮 tooltip：仅描述“下载证书文件”，不再暗示“系统已可用”。

## 技术细节

### `bifrost-tls` 侧

`crates/bifrost-tls/src/install.rs`：

- 复用现有 `pub enum CertStatus { NotInstalled, InstalledNotTrusted, InstalledAndTrusted }`。
- 已存在 `CertStatus::is_installed() / is_trusted()`；本方案不新增枚举 variant，`unknown` 通过 `check_status()` 返回 `Result::Err` 表达。
- 新增独立解析工具：

```rust
pub(crate) fn parse_sha256_fingerprint_hex(raw: &str) -> Option<String>;
```

  - 剥离 `SHA256 Fingerprint`、`sha256`、`=`、`:`、空格、换行。
  - 校验最终字符串长度为 64 hex，返回大写归一化结果。
  - `current_cert_fingerprint_macos` 与 `list_macos_keychain_fingerprints` 内部改为调用它。

### `bifrost-admin` 侧

`crates/bifrost-admin/src/handlers/cert.rs`：

- `CertStateView`：`status / status_label / installed / trusted / status_message` 已定义，本方案强制 `status_label` 使用固定英文常量集合（前端负责翻译），避免 label 漂移。
- `cert_state_from_status(status: CertStatus)`：映射 `NotInstalled / InstalledNotTrusted / InstalledAndTrusted` 三态。
- `cert_state_from_check_failure(err)`：返回 `status="unknown"`, `status_message` 拼接 err 短摘要（不含 stderr 原文）。
- `/api/cert/info` 响应示例：

```json
{
  "available": true,
  "status": "installed_not_trusted",
  "status_label": "Installed, not trusted",
  "installed": true,
  "trusted": false,
  "status_message": "CA certificate is installed, but the system does not trust it yet.",
  "fingerprint_tail": "3F9A22B1"
}
```

### CLI 侧

`bifrost ca status` 复用同一 `CertStateView`，输出示例：

```text
CA status: installed_and_trusted
Installed: true
Trusted:   true
File:      /Users/eden/.bifrost/ca/ca.crt (available for download)
Note:      CA certificate is installed and trusted by the system.
```

`--json` 输出与 `/api/cert/info` 一致，便于自动化消费。

### 前端

- `web/src/api/cert.ts`：`CertInfoResponse` 类型补齐 `status / status_label / installed / trusted / status_message`。
- `web/src/pages/Settings/tabs/CertificateTab.tsx`：
  - 状态区域直接绑定 `status`，不再读 `available`。
  - `unknown` 状态显示黄色 warning，附 `status_message` 全文。
  - 下载/二维码入口仅在 `available===true` 时启用，与状态解耦。
  - 亮暗主题、i18n 键位保持稳定。

## API

| Method | Path | 说明 |
|--------|------|------|
| GET | `/_bifrost/api/cert/info` | 返回四态 + `available` |
| GET | `/_bifrost/api/cert/download` | 仅在 `available===true` 时返回 `ca.crt` |
| POST | `/_bifrost/api/cert/install` | 本机管理调用，安装完成后重新计算状态 |

## Sync 边界

CA 状态是本机能力，不参与远端 sync。远端 sync 消费方在展示他人机器状态时不得复用本机四态；必要时通过独立的 remote host status 结构上报，避免与本机 `CertStateView` 字段冲突。

## 实现切分

### Phase 1：解析层加固（bifrost-tls）

- 抽出 `parse_sha256_fingerprint_hex`。
- `current_cert_fingerprint_macos` 与 `list_macos_keychain_fingerprints` 迁移。
- 单元测试覆盖三种 openssl/security 输出格式。

### Phase 2：Admin API 结构统一

- `CertStateView` 字段确认为 5 项固定 schema。
- 引入 `cert_state_from_check_failure`。
- `/api/cert/info` 响应新增 `fingerprint_tail`（可选，便于 UI 展示）。
- 保留 `available` 作为独立字段。

### Phase 3：CLI 与前端同步

- `bifrost ca status --json` 与 API 对齐。
- Settings 页 UI 改造，去除 `available -> 状态` 隐式映射。
- 增加 `unknown` 视觉与提示。

### Phase 4：文档、E2E 与 human_tests

- 更新 `human_tests/api-cert.md` 与 `human_tests/cli-ca-cert.md`。
- E2E 覆盖四态。

## 测试方案

### 单元测试

- `parse_sha256_fingerprint_hex_accepts_openssl_uppercase_colon`
- `parse_sha256_fingerprint_hex_accepts_libressl_lowercase_no_prefix`
- `parse_sha256_fingerprint_hex_accepts_spaces_and_mixed_case`
- `parse_sha256_fingerprint_hex_rejects_short_input`
- `cert_state_from_status_maps_three_variants`
- `cert_state_from_check_failure_marks_unknown`
- `cert_info_available_is_independent_of_status`

### E2E 测试

`e2e-tests/tests/test_cert_status.sh`：

- 无 `ca.crt`：断言 `status=not_installed`、`available=false`。
- 有 `ca.crt` 但未信任：断言 `status=installed_not_trusted`、`available=true`。
- 已信任：断言 `status=installed_and_trusted`。
- Mock `security` 返回非零：断言 `status=unknown` 且 `status_message` 非空。

### 真实场景测试 human_tests

新增/更新 `human_tests/cert-trust-status-unify.md`：

- TC-CTS-01：卸载本机 CA 后 API 返回 `not_installed`，设置页显示未安装。
- TC-CTS-02：安装 CA 但将信任设为 “绝不”，API 返回 `installed_not_trusted`，前端显示黄色提示。
- TC-CTS-03：完整信任后 API 返回 `installed_and_trusted`。
- TC-CTS-04：mock `security` 失败时 API 返回 `unknown`，前端展示检测失败卡片。
- TC-CTS-05：macOS 上验证不同 `openssl` 输出格式都能拿到一致指纹。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-tls cert_status`
- `cargo test -p bifrost-admin handlers::cert`
- `cargo test --workspace --all-features`
- `pnpm --dir web run build`
- `rust-project-validate`

本机 no-local-coverage 约定生效，`make coverage` 交由远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核四态语义是否与 UI 展示、CLI 输出、E2E 断言一致。
- 检查 `available` 字段是否仍被前端错误当成 status 来源。
- Review macOS 指纹解析：所有分隔/大小写变体是否走同一 parser。
- 复测：`bifrost-tls` cert_status 单元测试、Admin handlers::cert、E2E 四态脚本。

### 第 2 轮

- 复核第 1 轮修复后的 diff 与 human_tests 索引。
- 再检查 `unknown` 状态下 UI 是否会把用户引导到错误路径（如仍显示“证书已可用”）。
- 复跑真实机器安装/移除信任脚本，确认状态变化在 5 秒内被 UI 感知。

## 风险与决策

- macOS 上 `security find-certificate` 需要读取系统钥匙串，不同 OS 版本输出字段偏差较大；本方案通过 parser 规范化 + fallback `unknown` 保证不因单次异常导致误判。
- Linux 场景仍存在 `update-ca-certificates` 未刷新的可能性，`InstalledNotTrusted` 判定与旧行为一致，不额外扩展。
- Windows 通过 `CertUtil` 输出判定，本方案不改变 Windows 逻辑；如需未来加固，再另行设计。
- `unknown` 状态不区分 “永久失败” 与 “瞬时错误”，前端不做自动重试；用户可手动刷新页面重新触发检测。
