# CLI 系统代理命令测试用例

## 功能模块说明

测试 `bifrost system-proxy`（别名 `bifrost sp`）子命令的完整功能，包括查看系统代理状态、启用系统代理（含自定义 host/port/bypass）、禁用系统代理，以及 Surge 等外部系统代理与 Bifrost 自身系统代理的归属边界。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保当前操作系统支持系统代理设置（macOS / Windows）
3. 当前用户具有修改系统代理配置的权限（macOS 下可能需要管理员权限）

---

## 测试用例

### TC-CSP-01：查看系统代理状态

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy status
   ```

**预期结果**：
- 输出包含 `Supported: true`（在 macOS/Windows 上）
- 输出包含 `Enabled:` 字段，值为 `true` 或 `false`
- 输出包含 `Host:` 字段，显示当前系统代理主机地址
- 输出包含 `Port:` 字段，显示当前系统代理端口号
- 输出包含 `Bypass:` 字段，显示当前系统代理绕过列表
- 命令退出码为 0

---

### TC-CSP-02：使用别名查看系统代理状态

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- sp status
   ```

**预期结果**：
- 输出与 TC-CSP-01 一致，`sp` 别名正常工作
- 命令退出码为 0

---

### TC-CSP-03：使用默认参数启用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 system-proxy enable
   ```

**预期结果**：
- 输出包含 `✓ System proxy enabled: 127.0.0.1:8800`
- 默认 host 为 `127.0.0.1`，port 使用全局 `-p` 指定的 `8800`
- bypass 使用配置文件中的默认值
- 命令退出码为 0
- 执行 `system-proxy status` 确认 `Enabled: true`，`Host: 127.0.0.1`，`Port: 8800`

---

### TC-CSP-04：指定 host 和 port 启用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy enable --host 127.0.0.1 --port 8800
   ```

**预期结果**：
- 输出包含 `✓ System proxy enabled: 127.0.0.1:8800`
- host 为指定的 `127.0.0.1`
- port 为指定的 `8800`
- 命令退出码为 0

---

### TC-CSP-05：指定 bypass 列表启用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 system-proxy enable --bypass "localhost,127.0.0.1,*.local"
   ```

**预期结果**：
- 输出包含 `✓ System proxy enabled: 127.0.0.1:8800 (bypass: localhost,127.0.0.1,*.local)`
- bypass 列表为指定的 `localhost,127.0.0.1,*.local`
- 命令退出码为 0
- 执行 `system-proxy status` 确认 `Bypass:` 字段包含 `localhost,127.0.0.1,*.local`

---

### TC-CSP-06：同时指定 host、port、bypass 启用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy enable --host 127.0.0.1 --port 8800 --bypass "localhost,127.0.0.1,*.local,10.0.0.0/8"
   ```

**预期结果**：
- 输出包含 `✓ System proxy enabled: 127.0.0.1:8800 (bypass: localhost,127.0.0.1,*.local,10.0.0.0/8)`
- 所有参数均按指定值生效
- 命令退出码为 0

---

### TC-CSP-07：禁用系统代理

**前置条件**：已通过 TC-CSP-03 或 TC-CSP-04 启用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy disable
   ```

**预期结果**：
- 输出包含 `✓ System proxy disabled`
- 命令退出码为 0
- 执行 `system-proxy status` 确认 `Enabled: false`

---

### TC-CSP-08：禁用后再次查看状态确认已关闭

**前置条件**：已通过 TC-CSP-07 禁用系统代理

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy status
   ```

**预期结果**：
- 输出中 `Enabled:` 字段值为 `false`
- `Host:` 和 `Port:` 字段仍显示（可能为空或上次配置值）
- 命令退出码为 0

---

### TC-CSP-09：需要管理员权限时的提示（macOS）

**前置条件**：在非管理员权限下运行，且系统代理设置需要权限提升

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 system-proxy enable
   ```

**预期结果**：
- 如果权限不足，输出 `System proxy requires administrator privileges.`
- 提示 `Try enabling via sudo now? [y/n]`
- 输入 `y` 后尝试通过 sudo 启用，成功后输出 `✓ System proxy enabled via sudo`
- 输入 `n` 后输出 `Cancelled.`

---

### TC-CSP-10：禁用时需要管理员权限的提示（macOS）

**前置条件**：系统代理已启用，当前用户权限不足

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy disable
   ```

**预期结果**：
- 如果权限不足，输出 `System proxy disable requires administrator privileges.`
- 提示 `Try disabling via sudo now? [y/n]`
- 输入 `y` 后成功禁用，输出 `✓ System proxy disabled via sudo`
- 输入 `n` 后输出 `Cancelled.`

---

### TC-CSP-11：外部系统代理开启时，CLI 禁用不应清理外部代理

**前置条件**：
- macOS 环境；如本机安装 Surge，可直接开启 Surge 的系统代理；如未安装 Surge，用 `networksetup` 设置一个外部本机端口代理模拟：
  ```bash
  networksetup -setwebproxy "Wi-Fi" 127.0.0.1 6152
  networksetup -setwebproxystate "Wi-Fi" on
  ```
- Bifrost 使用临时数据目录，且未执行 `system-proxy enable`。

**操作步骤**：
1. 执行命令查看状态：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy status
   ```
2. 执行 Bifrost 禁用系统代理：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy disable
   ```
3. 再次用 `networksetup -getwebproxy "Wi-Fi"` 或 Surge 状态确认外部代理。

**预期结果**：
- 第 1 步可以显示系统代理已启用，host/port 为 Surge 或模拟外部代理端口。
- 第 2 步输出 `System proxy is enabled by another application; left unchanged.` 或等价“外部代理未归 Bifrost 管理”的提示，命令退出码为 0。
- 第 3 步确认外部代理仍保持启用，host/port 未被 Bifrost 改写或关闭。
- 不出现 `Proxy is still enabled` 这类把外部代理当作 Bifrost 关闭失败的误报。

---

### TC-CSP-12：外部代理开启时，`bifrost stop` 不应关闭外部代理

**前置条件**：
- macOS 环境，Surge 系统代理已开启，或按 TC-CSP-11 设置外部本机端口代理。
- 使用临时数据目录启动 Bifrost，必须禁用 Bifrost 自身系统代理：
  ```bash
  BIFROST_DATA_DIR=./.bifrost-test BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
  ```

**操作步骤**：
1. 确认 Bifrost 运行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```
2. 停止 Bifrost：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   ```
3. 退出 Surge 或关闭模拟外部代理，再访问一个网页或执行：
   ```bash
   curl -I https://example.com
   ```

**预期结果**：
- `bifrost stop` 只停止 Bifrost 进程，不输出 `System proxy disabled.`，除非当前系统代理确实指向 Bifrost 端口。
- Bifrost 停止后，Surge/外部代理仍保持原本状态，未被 Bifrost 清理。
- 退出 Surge 或关闭外部代理后，系统不残留指向 Bifrost 的代理配置；网页访问或 `curl -I https://example.com` 可正常连通。

---

## 清理

测试完成后清理临时数据并确保系统代理已关闭：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy disable
rm -rf .bifrost-test
```
