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

### TC-CSP-13：睡眠恢复后系统代理漂移时，Bifrost 应自动重新收敛并保持可用

**前置条件**：
- macOS 支持系统代理的环境。
- 使用临时数据目录，避免污染正式配置：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
2. 确认系统代理指向 Bifrost：
   ```bash
   networksetup -getwebproxy "Wi-Fi"
   ```
3. 模拟睡眠恢复后 macOS 网络服务刷新导致代理配置漂移：
   ```bash
   networksetup -setwebproxystate "Wi-Fi" off
   networksetup -setsecurewebproxystate "Wi-Fi" off
   ```
4. 如果是真实休眠恢复测试，唤醒后等待最多 10 秒再次检查系统代理；如果只是用 `networksetup` 手动模拟漂移而没有发生系统休眠，则等待最多 75 秒，让 30 秒周期 reconcile 覆盖：
   ```bash
   TIMEOUT=10   # 真实休眠恢复
   # TIMEOUT=75 # 仅手动 networksetup 模拟漂移
   for i in $(seq 1 "$TIMEOUT"); do
     networksetup -getwebproxy "Wi-Fi" | grep -q "Enabled: Yes" \
       && networksetup -getwebproxy "Wi-Fi" | grep -q "Server: 127.0.0.1" \
       && networksetup -getwebproxy "Wi-Fi" | grep -q "Port: 18889" \
       && break
     sleep 1
   done
   networksetup -getwebproxy "Wi-Fi"
   ```
5. 验证 Bifrost 服务仍可访问：
   ```bash
   curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null
   ```
6. 停止 Bifrost：
   ```bash
   kill "$PROXY_PID"
   wait "$PROXY_PID" 2>/dev/null || true
   ```

**预期结果**：
- 第 4 步最终显示 Wi-Fi Web Proxy 重新指向 `127.0.0.1:18889`。
- `proxy.log` 包含 `system proxy scheduler or wake gap detected; reconciling immediately` 或周期 reconcile 的 `system proxy applied or reconciled`；真实休眠恢复测试应优先观察 wake-gap 日志，手动漂移模拟允许只观察周期 reconcile 日志。wake-gap 日志只代表触发幂等重新收敛，不代表进程异常或清理动作。
- 第 5 步 API 请求成功，说明睡眠恢复式配置漂移后 Bifrost 服务仍正常工作。
- 第 6 步停止后系统代理恢复，不再指向 `127.0.0.1:18889`。

---

### TC-CSP-14：崩溃或重启后，下次启动失败前也必须清理 Bifrost 系统代理残留

**前置条件**：
- macOS 或 Windows 支持系统代理的环境。
- 使用临时数据目录，避免污染正式配置：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
2. 确认系统代理指向 Bifrost：
   ```bash
   networksetup -getwebproxy "Wi-Fi"
   ```
   预期 `Enabled: Yes`，`Server: 127.0.0.1`，`Port: 18889`。
3. 模拟电脑重启、进程崩溃或异常中断：强制结束 Bifrost 进程，不执行 `bifrost stop`。
   ```bash
   kill -9 "$PROXY_PID"
   wait "$PROXY_PID" 2>/dev/null || true
   rm -f "$TEST_DATA_DIR/bifrost.pid" "$TEST_DATA_DIR/runtime.json"
   ```
4. 用一个临时进程占用同一个端口，模拟下次启动因为端口冲突失败：
   ```bash
   python3 - <<'PY' &
   import socket, time
   s = socket.socket()
   s.bind(("127.0.0.1", 18889))
   s.listen(1)
   time.sleep(120)
   PY
   BLOCKER_PID=$!
   ```
5. 用同一个数据目录再次启动 Bifrost，并显式不启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --no-system-proxy
   ```
6. 再次检查 Wi-Fi 系统代理：
   ```bash
   networksetup -getwebproxy "Wi-Fi"
   ```
7. 清理端口占用进程：
   ```bash
   kill "$BLOCKER_PID"
   ```

**预期结果**：
- 第 5 步由于端口被占用而非零退出，输出包含端口已被占用或 bind 失败信息。
- 即使第 5 步启动失败，第 6 步也显示 Wi-Fi 系统代理不再指向 `127.0.0.1:18889`；默认无代理环境下应为 `Enabled: No`。
- `proxy_state.json` / `proxy_backup.json` 不再残留在 `TEST_DATA_DIR` 下。
- 用户网络访问恢复，不再因为系统代理指向已不存在的 Bifrost 端口而断网。

---

### TC-CSP-15：主进程没有优雅退出机会时，macOS lifecycle helper 应清理 Bifrost 系统代理残留

**前置条件**：
- macOS 支持系统代理的环境。
- 使用临时数据目录，避免污染正式配置：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理，不设置 `BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER`：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
2. 确认 `proxy.log` 包含 lifecycle helper 启动日志：
   ```bash
   grep "system proxy lifecycle cleanup helper started" "$TEST_DATA_DIR/proxy.log"
   ```
3. 确认系统代理指向 Bifrost：
   ```bash
   networksetup -getwebproxy "Wi-Fi"
   ```
4. 模拟主进程无优雅退出机会：
   ```bash
   kill -9 "$PROXY_PID"
   wait "$PROXY_PID" 2>/dev/null || true
   ```
5. 等待最多 45 秒，检查 Wi-Fi 系统代理：
   ```bash
   for i in $(seq 1 45); do
     networksetup -getwebproxy "Wi-Fi" | grep -q "Port: 18889" || break
     sleep 1
   done
   networksetup -getwebproxy "Wi-Fi"
   ```

**预期结果**：
- 第 2 步能看到 helper 已独立启动。
- 第 5 步显示 Wi-Fi 系统代理不再指向 `127.0.0.1:18889`；默认无代理环境下应为 `Enabled: No`。
- `proxy_state.json` / `proxy_backup.json` 不再残留在 `TEST_DATA_DIR` 下。
- helper 日志先输出连续父进程不可见计数，达到 3 次后才输出 `system proxy lifecycle helper confirmed parent exit` 并执行 cleanup；CPU 高占用或系统恢复后的单次调度延迟不应触发父进程退出误判。
- 该用例不依赖下一次 `bifrost start`，覆盖系统关机或主进程被中断时的进程外兜底。

---

### TC-CSP-16：启用系统代理启动后，应异步触发 macOS LaunchDaemon cleanup 授权安装且不阻塞服务 ready

**前置条件**：
- macOS 环境，当前用户具备管理员授权能力。
- cleanup LaunchDaemon 未安装、未加载，或已安装但 `program`、`data_dir`、运行模式与当前 `target/debug/bifrost` 不一致。旧版 `KeepAlive` 常驻 plist 应被识别为需要升级；仅 `Installed version` 与 `Current version` 不一致不应触发重新安装。可先执行：
  ```bash
  ./target/debug/bifrost system-proxy launchd status
  ```
- 使用临时数据目录和非默认端口：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 RUST_LOG=bifrost_cli::startup=info,bifrost_cli::shutdown=info ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   ```
2. 立即等待服务 ready，不等待授权完成：
   ```bash
   for i in $(seq 1 60); do
     curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1 && break
     sleep 0.2
   done
   curl -sS "http://127.0.0.1:18889/_bifrost/api/system"
   ```
3. 观察 macOS 是否弹出系统授权窗口；输入管理员密码或使用指纹授权。
4. 授权完成后检查 LaunchDaemon 状态：
   ```bash
   ./target/debug/bifrost system-proxy launchd status
   launchctl print system/com.bifrost.system-proxy-cleanup >/tmp/bifrost-launchd-status.txt && cat /tmp/bifrost-launchd-status.txt | head
   ```
5. 检查日志：
   ```bash
   grep -E "LaunchDaemon cleanup install starting asynchronously|LaunchDaemon cleanup installed asynchronously|launchd cleanup daemon started|startup recovery skipped" "$TEST_DATA_DIR/proxy.log"
   ```
6. 停止服务：
   ```bash
   kill "$PROXY_PID"
   wait "$PROXY_PID" 2>/dev/null || true
   ```

**预期结果**：
- 第 2 步服务 API 在授权完成前已经 ready，证明 LaunchDaemon 安装不阻塞主服务启动。
- 第 3 步出现 macOS GUI 授权窗口，支持密码或指纹授权；用户取消授权时服务仍继续运行，只记录取消日志。
- 第 4 步显示 `Installed: true`、`Loaded: true`、`Installed mode: one-shot`、`Needs upgrade: false`。
- plist 中 ProgramArguments 包含 `system-proxy cleanup-daemon --data-dir "$TEST_DATA_DIR" --installed-version <current>`，包含 `RunAtLoad`，且不包含 `KeepAlive`。
- one-shot daemon 启动后如果看到 `runtime.json` 中的 Bifrost pid 仍存活，应跳过 startup cleanup，然后退出，不会误清正在运行的系统代理，也不会在空闲时保留 cleanup-daemon 进程。

---

### TC-CSP-17：Web UI 可通过 GUI 授权安装/卸载 macOS one-shot LaunchDaemon cleanup，结构一致时不重复安装

**前置条件**：
- 已按 TC-CSP-16 构建 `target/debug/bifrost`。
- 使用临时数据目录启动真实服务并启用系统代理：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy
  ```

**操作步骤**：
1. 打开 Web UI：
   ```text
   http://127.0.0.1:18889/_bifrost/settings?tab=proxy
   ```
2. 在 `System Proxy` 卡片中找到 `Boot/Shutdown Cleanup` 开关。
3. 如果开关为开启，先关闭它；观察 macOS GUI 授权窗口，输入密码或指纹授权卸载。
4. 检查状态：
   ```bash
   ./target/debug/bifrost system-proxy launchd status
   ```
5. 回到 Web UI，再打开 `Boot/Shutdown Cleanup`；观察 macOS GUI 授权窗口，输入密码或指纹授权安装。
6. 再次检查状态、运行模式和版本诊断信息：
   ```bash
   ./target/debug/bifrost system-proxy launchd status
   ```
7. 在 LaunchDaemon 已安装且 `program`、`data_dir`、`Installed mode: one-shot` 均一致的情况下，停止并重新启动 Bifrost：
   ```bash
   ./target/debug/bifrost -p 18889 stop || true
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 RUST_LOG=bifrost_cli::startup=info ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/restart.log" 2>&1 &
   ```
8. 检查重启日志：
   ```bash
   grep "LaunchDaemon cleanup already installed and current" "$TEST_DATA_DIR/restart.log"
   ! grep -q "LaunchDaemon cleanup install starting asynchronously" "$TEST_DATA_DIR/restart.log"
   ```

**预期结果**：
- Web UI 关闭开关时触发 GUI 授权卸载，授权成功后 CLI status 显示 `Installed: false`、`Loaded: false`。
- Web UI 打开开关时触发 GUI 授权安装，授权成功后 CLI status 显示 `Installed: true`、`Loaded: true`、`Installed mode: one-shot`、`Needs upgrade: false`。
- 若用户取消授权，Web UI 显示授权取消或失败，服务继续运行，系统代理主开关状态不被错误修改。
- 第 7-8 步不会再次弹出授权窗口；日志显示已安装且结构一致并跳过重复安装。即使 `Installed version` 与 `Current version` 不一致，只要 `program`、`data_dir` 和 one-shot 模式一致，也不应重复弹授权。

---

### TC-CSP-18：运行中的服务通过 Admin API/Web UI 打开系统代理时，应自动检查并安装 macOS LaunchDaemon cleanup

**前置条件**：
- macOS 环境，当前用户具备管理员授权能力。
- 使用临时数据目录启动真实服务，但启动时先禁用 Bifrost 系统代理，模拟 9900 长驻服务已经在跑、随后用户才打开系统代理：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 RUST_LOG=bifrost_admin::proxy=info ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --no-system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
  PROXY_PID=$!
  until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
  ```
- 如需避免真实弹出授权窗口验证“检查链路被触发”，可临时设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 后启动服务；完整人工验收不应设置该变量。

**操作步骤**：
1. 确认 LaunchDaemon 当前状态：
   ```bash
   ./target/debug/bifrost system-proxy launchd status
   ```
2. 通过 Admin API 打开系统代理：
   ```bash
   curl -sS -X PUT "http://127.0.0.1:18889/_bifrost/api/proxy/system" \
     -H "Content-Type: application/json" \
     -d '{"enabled":true}' | tee "$TEST_DATA_DIR/system-proxy-enable.json"
   ```
3. 或在 Web UI 打开 `Settings -> Proxy -> Enable System Proxy`：
   ```text
   http://127.0.0.1:18889/_bifrost/settings?tab=proxy
   ```
4. 观察 macOS GUI 授权窗口；输入管理员密码或使用指纹授权。若本轮设置了 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`，改为检查日志包含禁用提示。
5. 授权完成后检查 LaunchDaemon 状态：
   ```bash
   ./target/debug/bifrost system-proxy launchd status
   ```
6. 检查服务日志：
   ```bash
   grep -E "LaunchDaemon cleanup install starting asynchronously after system proxy enable|LaunchDaemon cleanup installed asynchronously after system proxy enable|LaunchDaemon cleanup already installed and current after system proxy enable|LaunchDaemon cleanup install disabled by environment" "$TEST_DATA_DIR/proxy.log"
   ```
7. 清理：
   ```bash
   curl -sS -X PUT "http://127.0.0.1:18889/_bifrost/api/proxy/system" \
     -H "Content-Type: application/json" \
     -d '{"enabled":false}' >/dev/null
   kill "$PROXY_PID"
   wait "$PROXY_PID" 2>/dev/null || true
   ```

**预期结果**：
- 第 2 步响应显示 `enabled=true`、`host=127.0.0.1`、`port=18889`、`managed_by_bifrost=true`。
- 系统代理由运行中的服务接管后，服务自动检查 cleanup LaunchDaemon；缺失、未加载、二进制/data-dir 不一致、旧 `KeepAlive` 常驻模式或缺少 `RunAtLoad` 时会异步触发授权安装，不需要用户再手动点击 `Boot/Shutdown Cleanup`。
- 仅版本号不一致时不会触发授权安装；`Installed version` 只作为诊断字段展示。
- 用户授权成功后，`system-proxy launchd status` 显示 `Installed: true`、`Loaded: true`、`Installed mode: one-shot`、`Needs upgrade: false`。
- 用户取消授权时，系统代理主开关保持已打开，服务继续运行，日志记录取消，不把取消误报为系统代理启用失败。
- 设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 的调试验证中，系统代理仍可打开，但日志显示 LaunchDaemon 自动安装检查被环境变量禁用。

---

## 执行记录

- 2026-06-05：真实执行 TC-CSP-18 的 Admin API 路径。使用 `/tmp/bifrost-csp18.*` 临时数据目录和 `target/debug/bifrost` 在 `18889` 端口启动服务，启动参数包含 `--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 避免本轮弹出真实授权窗口。服务 ready 后调用 `PUT /_bifrost/api/proxy/system {"enabled":true}`，响应为 `enabled=true host=127.0.0.1 port=18889 managed_by_bifrost=true`；随后日志出现 `system proxy LaunchDaemon cleanup install disabled by environment`，证明运行中服务通过 Admin API 打开系统代理后已触发 LaunchDaemon 自动检查路径。测试结束调用 API 关闭系统代理并停止临时服务，临时数据目录已清理。
- 2026-06-05：按 one-shot LaunchDaemon 改造补充执行 TC-CSP-16 的 CLI dry-run 验证。执行 `./target/debug/bifrost system-proxy launchd install --data-dir "$TEST_DATA_DIR" --program ./target/debug/bifrost --label com.bifrost.test-system-proxy-cleanup --plist-path "$TEST_DATA_DIR/com.bifrost.test-system-proxy-cleanup.plist" --dry-run`，确认输出包含 `RunAtLoad`、`cleanup-daemon`、`--installed-version` 和 data dir，且不包含 `KeepAlive`。执行 `./target/debug/bifrost system-proxy cleanup-daemon --data-dir "$TEST_DATA_DIR" --installed-version 0.0.1`，命令在 one-shot recovery 检查后退出，无常驻 cleanup-daemon 进程。
- 2026-06-04：执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_system_proxy_e2e.sh`，覆盖 TC-CSP-11、TC-CSP-12、TC-CSP-13、TC-CSP-14 相关真实系统代理场景。第一轮发现 macOS `scutil --proxy` 聚合视图漏掉非 Wi-Fi network service 残留，USB/Thunderbolt 等服务仍指向 `127.0.0.1:18889`；修复为逐 service 检查 `networksetup -getwebproxy` / `-getsecurewebproxy` 后重跑通过。第 1 轮 review 又发现 shutdown restore 后 reconcile 线程可能醒来重新 enable，补充 stop flag 后第三轮重跑：8/8 PASS，包含系统代理启用、`--no-system-proxy` 外部代理保留、外部代理 disable 归属边界、正常退出恢复、睡眠恢复式漂移重新收敛、崩溃后再次启动恢复、启动失败前同步清理残留。后续全面分析又发现前台 listener 异常退出路径只依赖 restore guard，可能未先停止 reconcile；已将 stop flag 纳入 restore guard，确保异常退出也先停止 reconcile 再恢复。执行后确认 Wi-Fi Web Proxy 与 Secure Web Proxy 均恢复到测试前 `127.0.0.1:9900`。
- 2026-06-04：按重启/休眠排查要求补充日志验证点。启动恢复路径应输出 `checking for stale system proxy state before startup` 与 `System proxy crash recovery check starting`；关机/停止信号路径应输出 `system proxy shutdown restore starting; stopping reconcile first`、`System proxy restore requested`、`Restoring macOS system proxy to saved original state`、逐 network service 的 `Disabling macOS network service web proxies` 或 `Setting macOS network service proxy to requested target`，以及 `system proxy shutdown restore completed` 和耗时。异常退出兜底 guard 应输出 `system proxy restore guard triggered; stopping reconcile before restore`，失败场景应输出 `failed to restore system proxy` / `system proxy restore guard failed to restore proxy` / `system proxy reconcile failed`，用于定位重启前清理是否真正执行。
- 2026-06-04：全面审查退出顺序后补强 listener 异常退出路径。前台和 daemon listener task 非 signal 结束时，也应先输出 `system proxy shutdown restore starting; stopping reconcile first` 并执行 restore，再进入 listener error 返回和后续任务清理；日志 context 分别为 `foreground listener exit` / `daemon listener exit`。
- 2026-06-04：本轮新增 lifecycle helper、wake-gap reconcile、target-aware macOS restore 与锁/耗时日志。真实执行记录待本机按 TC-CSP-13、TC-CSP-15 以及 `e2e-tests/tests/test_system_proxy_e2e.sh` 补充；验证重点为 helper 启动日志、主进程 `kill -9` 后无需下一次启动即可清理、helper 连续 3 次父 PID 不可见才确认退出、睡眠恢复后尽快 reconcile、shutdown restore 出现 `waiting_for_system_proxy_lock` / `acquired_system_proxy_lock`，以及 restore 只对仍指向 Bifrost target 的 network service 输出 service 级 elapsed 日志。
- 2026-06-04：真实执行 TC-CSP-16、TC-CSP-17。使用 `/tmp/bifrost-launchd-human2.KxevxD` 临时数据目录和 `target/debug/bifrost` 启动 18889 服务，确认服务 ready 先于 LaunchDaemon 授权安装；启动时如果 `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist` 已安装且 binary/data-dir/运行模式匹配，日志显示 `system proxy LaunchDaemon cleanup already installed and current`，不会再次弹出授权。通过 Web UI `Boot/Shutdown Cleanup` 开关真实触发 macOS GUI 授权卸载/安装，授权后 API 与 CLI 均显示 `installed=false loaded=false` / `installed=true loaded=true needs_upgrade=false`。授权弹窗文案已收敛为英文短描述，说明 Bifrost network protection helper 用于异常退出后自动恢复 system proxy settings。
- 2026-06-04：真实验证外部代理占用与覆盖/恢复语义。测试前系统代理由正式 9900 服务占用；18889 服务启动时日志输出 `system proxy is already owned by another proxy; startup auto-apply skipped`，未抢占 9900。18889 Settings Proxy 页面中 `Enable System Proxy` 开关显示关闭，并以 warning 提示 `System proxy is occupied by another proxy`，API 返回 `enabled=true host=127.0.0.1 port=9900 managed_by_bifrost=false`。在 18889 Web UI 手动打开 System Proxy 后，系统代理切到 `127.0.0.1:18889` 且 `managed_by_bifrost=true`；再次关闭后恢复到 `127.0.0.1:9900` 且 `managed_by_bifrost=false`。
- 2026-06-04：真实验证主进程强杀保护。先在 18889 Web UI 手动打开 System Proxy，使系统代理从 9900 切到 18889 并保存 9900 为 original；随后对 18889 主进程执行 `kill -9`。2 秒轮询内 `./target/debug/bifrost system-proxy status` 显示恢复为 `Host: 127.0.0.1`、`Port: 9900`。18889 服务不可访问，证明运行期异常退出由 lifecycle helper 快速恢复；LaunchDaemon 只负责系统启动/bootstrap/kickstart 后的一次性遗留清理。

---

## 清理

测试完成后清理临时数据并确保系统代理已关闭：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- system-proxy disable
rm -rf .bifrost-test
```
