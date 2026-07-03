# Windows Root Store 证书安装与检测

## 背景

Bifrost 在 Windows 上把自签 `Bifrost CA` 装到 `Root` 证书库以完成 TLS MITM 抓包。历史实现只按证书名字符串 `Bifrost CA` 匹配，导致以下问题：

- 上一次安装的旧证书（thumbprint 已过期或换算法）会被同名匹配识别为"已安装且已信任"，桌面端状态页和 CLI 都会给出误报。
- 首次安装默认要求 UAC 提权到 `LocalMachine\Root`，桌面端启动流程需要弹 UAC，交互沉重且失败无兜底。
- CLI 与桌面端各自实现 Windows 安装/检测路径，行为容易漂移。

修复目标：让 Windows 检测与其他平台一致地做 thumbprint 精确匹配；让安装优先落在 `CurrentUser\Root` 无提权路径，失败再走 UAC 到 `LocalMachine\Root`；桌面端 GUI 复用同一入口，避免两份实现。

## 用户目标验证清单

### 必须实现

- 检测阶段解析当前 `ca.crt` 得到 SHA-1 thumbprint，然后：
  - 通过 `certutil -user -store Root Bifrost CA` 拿 `CurrentUser\Root` 内所有同名证书 thumbprint。
  - 通过 `certutil -store Root Bifrost CA` 拿 `LocalMachine\Root` 内所有同名证书 thumbprint。
  - 仅当集合中存在与当前 `ca.crt` 相同的 thumbprint 才判为"已安装"。
- 若发现同名 `Bifrost CA` 但 thumbprint 不匹配，`CertSystemInfo.fingerprint_match = Some(false)`，状态页/CLI 明确显示 mismatch 而不是"已安装"。
- 安装阶段优先执行 `certutil -user -addstore Root <ca.crt>`（不需要 UAC）。
- 当 `CurrentUser` 安装失败或返回非 0 时，通过 `ShellExecuteExW` + `runas` 提权执行机器级 `certutil -addstore Root <ca.crt>`。
- 桌面端 `install_and_trust_gui()` 在 Windows 下直接复用 `install_and_trust()`，不再维护第二套 Windows 逻辑。
- CLI `bifrost ca install` 与桌面端使用同一 `crates/bifrost-tls/src/install.rs` 入口。

### 必须不破坏

- macOS / Linux 的检测和安装路径保持不变（macOS 走 openssl SHA-256 fingerprint + security keychain，Linux 走 CA bundle 内容匹配）。
- 手动安装说明（`get_install_instructions` 在 Windows 下输出 `certutil -addstore Root ...`）保留，供 UAC 也失败时用户手工兜底。
- `CertSystemInfo.fingerprint_match` 三态语义（`Some(true)` / `Some(false)` / `None`）保持，供上层区分"匹配 / 同名不匹配 / 完全没有"。

### 必须真实验证

- 单元测试覆盖 `parse_windows_certutil_thumbprint` 与 `normalize_thumbprint` 各种脏输入形态。
- 行为验证：仅存在同名旧证书时状态应报告 fingerprint mismatch；安装优先写入 `CurrentUser\Root`；当前用户失败时 UAC 提权路径仍可用。
- E2E：Windows 桌面端启动触发证书安装；Windows CLI `bifrost ca install` 成功后状态页与 CLI 一致显示已信任。

## 产品语义

### 三态检测结果

`CertSystemInfo.fingerprint_match` 用于上层 UI/CLI 分类展示：

| 状态 | 语义 | UI 建议 |
| --- | --- | --- |
| `Some(true)` | 当前 `ca.crt` thumbprint 在 `Root` 库中命中 | 显示"已安装并信任"，绿色 |
| `Some(false)` | 找到了同名 `Bifrost CA`，但 thumbprint 与当前 `ca.crt` 不一致 | 显示"已安装但证书不匹配"，橙色，并提示重装 |
| `None` | 完全没有 `Bifrost CA` | 显示"未安装"，红色，提供安装按钮 |

### 安装优先级

1. `CurrentUser\Root`：`certutil -user -addstore Root <ca.crt>`。走用户级证书库，不弹 UAC，桌面端启动最平滑。
2. UAC 提权到 `LocalMachine\Root`：`ShellExecuteExW` + `runas` 调 `certutil -addstore Root <ca.crt>`。仅当步骤 1 失败或返回非 0 时才触发。
3. 完全失败：抛 `BifrostError::Tls("Failed to install CA certificate. Administrator privileges required.")`，同时打印手动指令 `certutil -addstore Root "<path>"` 供用户兜底。

## 技术细节

### 关键函数（`crates/bifrost-tls/src/install.rs`）

- `check_status_windows()` / `get_detailed_status_windows()`：拿 `current_cert_thumbprint_windows()`，分别检查 user & machine 两个 store。
- `current_cert_thumbprint_windows()`：读取 `ca.crt`（PEM）→ `rustls::pki_types::CertificateDer` → SHA-1 → 大写 hex thumbprint。
- `list_windows_store_thumbprints(store_scope, store_name, cert_name)`：
  - `store_scope = Some("user")` → `certutil -user -store Root <cert_name>`。
  - `store_scope = None` → `certutil -store Root <cert_name>`（machine）。
  - 逐行调用 `parse_windows_certutil_thumbprint` 提取 `Cert Hash(sha1): xx xx ...` 里的 thumbprint。
- `windows_store_contains_thumbprint(...)`：在上述集合里查找当前 thumbprint。
- `install_windows()`：先 `certutil -user -addstore Root`，失败调用 `install_cert_with_uac()`。
- `install_cert_with_uac()`：`ShellExecuteExW` + `runas` + `-addstore Root "<path>"`，`WaitForSingleObject(INFINITE)` 等待退出码，用 `GetExitCodeProcess` 判断 UAC 结果。
- `parse_windows_certutil_thumbprint(line)`：只保留 `(sha1):` 之后的 40 位十六进制。
- `normalize_thumbprint(value)`：过滤非 hex 字符 + 转大写，得到稳定形态。

### 与桌面端集成

`install_and_trust_gui()` 分平台派发：

- macOS：`install_macos_gui()`（用 SecurityKeychain GUI 授权）。
- Linux / Windows：直接调用 `install_and_trust()`，共用同一入口。

因此 Windows 的 UAC 提权逻辑不需要在 `desktop/src-tauri/src/main.rs` 中重复实现。

## CLI + Web + Admin API

- CLI：`bifrost ca install`、`bifrost ca status` 直接使用 `CertInstaller`。
- Admin API：`GET /api/ca/status` 返回 `CertSystemInfo`（含 `fingerprint_match`），Web 状态页读取并渲染三态。
- Web：证书状态卡片按 `fingerprint_match` 分色展示，mismatch 时提供"重装"按钮触发 `POST /api/ca/install`。

## Sync 边界

- CA 证书是本机安全 boundary，绝不参与多设备 sync。
- `ca.crt` 由 `bifrost-tls` 首次启动本地生成，同一账户不同机器各自拥有独立的私钥，跨机器不复制。
- 检测/安装状态只反映本机 store，不上报服务器。

## Phase 拆分

### Phase 1：thumbprint 检测

- 引入 `current_cert_thumbprint_windows()` + `list_windows_store_thumbprints()` + `parse_windows_certutil_thumbprint()`。
- `check_status_windows()` 从"名字匹配"改成"thumbprint 精确匹配"。
- `get_detailed_status_windows()` 返回三态 `fingerprint_match`。

### Phase 2：安装优先当前用户

- `install_windows()` 首先 `-user -addstore Root`。
- 失败调用 `install_cert_with_uac()` 提权到 machine store。
- 完全失败时打印手动指令并返回 `BifrostError::Tls`。

### Phase 3：桌面端复用

- `install_and_trust_gui()` 在 Windows 下走 `install_and_trust()`。
- 移除 `desktop/src-tauri/src/main.rs` 中残留的 Windows 专用安装分支（如果存在）。

### Phase 4：文档与手动兜底

- README/docs 补充 Windows 优先写入 `CurrentUser\Root`、失败回退到 UAC 的说明。
- 保留手动指令 `certutil -addstore Root "<ca.crt>"`。
- 更新 human_tests/ca-install-system-keychain.md 或新增 Windows 专属条目。

## 测试方案

### 单元测试（`crates/bifrost-tls/src/install.rs`）

- `test_cert_status_display`：`CertStatus::Trusted/Untrusted/NotInstalled` 展示。
- `test_cert_status_helpers`：状态谓词。
- `test_cert_installer_new`：构造。
- `test_get_install_instructions`：Windows 下含 `certutil -addstore Root`。
- `test_get_platform_name`：`Windows`。
- `test_parse_openssl_sha256_fingerprint`（macOS 相邻测试保持）。
- `test_normalize_thumbprint`（第 1111 行）：混合空格 / 小写 / 冒号统一为大写 hex。
- `test_parse_windows_certutil_thumbprint`（第 1204 行）：
  - `Cert Hash(sha1): aa bb cc ...`（40 位 hex）→ Some。
  - 缺 `(sha1):` → None。
  - `Cert Hash(sha1): aa bb`（< 40 位）→ None。
  - 大小写和空格混合 → 归一化为大写连续 hex。
  - 含非 ASCII 兜底 lossy 转换。
- Linux 相邻测试（`test_check_status_linux_not_installed` 等）验证 Windows 修改不破坏 Linux 检测路径。

### 行为验证

- 手工在 Windows VM 装一份旧 `Bifrost CA`，删除本地 `ca.crt` 让 Bifrost 重新生成，验证 status 报告 `fingerprint_match = Some(false)` 而非 `Some(true)`。
- 干净环境执行 `bifrost ca install`，观察不弹 UAC，`certutil -user -verifystore Root` 能看到新 thumbprint。
- 模拟 `CurrentUser` 安装失败（例如临时把 `certutil` 重命名），验证 UAC 提权路径被触发并成功安装到 `LocalMachine\Root`。

### E2E

- Windows 桌面端：首次启动触发 `install_and_trust_gui()`，无 UAC 完成安装，状态页立刻显示"已信任"。
- Windows CLI：`bifrost ca install` → `bifrost ca status`，与桌面端状态页一致显示已信任。
- 二次执行 `bifrost ca install`：因 thumbprint 已匹配，快速返回幂等成功。

### 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-tls`
- `cargo build --all-targets --all-features`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核检测：thumbprint 集合是否真的分别从 user & machine 两个 store 提取；`parse_windows_certutil_thumbprint` 是否处理了 hex 位数 < 40 的脏行。
- 复核安装：UAC 分支的 `WaitForSingleObject(INFINITE)` 是否等到子进程真正结束才判断退出码。
- 跑 `cargo test -p bifrost-tls` + Windows 单元测试子集。

### 第 2 轮

- 手工 Windows VM 三种场景走查（干净 / 只有旧证书 / user 安装失败）。
- CLI/桌面端状态页一致性对齐。
- 检查 `install_and_trust_gui` 是否所有平台都对齐同一入口。

## 风险与决策点

- `certutil -user -addstore` 若被组策略禁用会在 `CurrentUser` 环境下也失败，会立即触发 UAC 提权到 machine store；对企业受管终端可能导致连续弹 UAC，需要产品评估是否需要"关闭 fallback"选项。
- `parse_windows_certutil_thumbprint` 依赖 `Cert Hash(sha1):` 输出格式，若未来 certutil 输出格式变（例如换成 SHA-256），需要同步更新解析。
- 使用 SHA-1 thumbprint 是因为 `certutil` 只暴露 SHA-1，虽然 SHA-1 已不推荐做安全签名，但用于同证书识别足够；无需切换到 SHA-256 thumbprint。
- CurrentUser store 只对当前 Windows 用户生效；如果用户以另一个账户登录，浏览器信任状态不会继承。这属于 Windows 证书体系本身语义，不在本方案修复范围。
