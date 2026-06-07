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

### TC-CSP-19：无 backup/state 时，应按上次 runtime target 清理 Bifrost 系统代理残留

**前置条件**：
- macOS 支持系统代理的环境。
- 使用临时数据目录，避免污染正式配置：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 保存当前系统代理快照，便于测试后恢复：
   ```bash
   networksetup -getwebproxy "Wi-Fi" > "$TEST_DATA_DIR/wifi-web-before.txt"
   networksetup -getsecurewebproxy "Wi-Fi" > "$TEST_DATA_DIR/wifi-secure-before.txt"
   ```
2. 删除崩溃恢复的 managed state 和 backup，模拟关机/历史版本清理后只剩 runtime 信息：
   ```bash
   rm -f "$TEST_DATA_DIR/proxy_state.json" "$TEST_DATA_DIR/proxy_backup.json"
   ```
3. 写入上一次 Bifrost runtime target：
   ```bash
   cat > "$TEST_DATA_DIR/runtime.json" <<'EOF'
   {
     "pid": 999999,
     "port": 18889,
     "host": "0.0.0.0"
   }
   EOF
   echo 999999 > "$TEST_DATA_DIR/bifrost.pid"
   ```
4. 将 Wi-Fi Web/Secure Web 代理设置为与 runtime target 等价的 `127.0.0.1:18889`：
   ```bash
   networksetup -setwebproxy "Wi-Fi" 127.0.0.1 18889
   networksetup -setwebproxystate "Wi-Fi" on
   networksetup -setsecurewebproxy "Wi-Fi" 127.0.0.1 18889
   networksetup -setsecurewebproxystate "Wi-Fi" on
   ```
5. 使用同一数据目录启动 Bifrost，但显式不启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --no-system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
6. 检查 Wi-Fi Web/Secure Web 代理：
   ```bash
   networksetup -getwebproxy "Wi-Fi"
   networksetup -getsecurewebproxy "Wi-Fi"
   grep -E "last Bifrost runtime target|Recovered stale Bifrost system proxy" "$TEST_DATA_DIR/proxy.log"
   ```
7. 停止临时服务并按快照恢复系统代理。

**预期结果**：
- 第 5 步启动前恢复会读取 `runtime.json` 的 `host=0.0.0.0 port=18889`，映射为系统代理 target `127.0.0.1:18889`。
- 第 6 步显示 Wi-Fi Web/Secure Web 代理不再指向 `127.0.0.1:18889`；默认无代理环境下应为 `Enabled: No`。
- 日志包含 `No managed proxy state found, but current system proxy matches last Bifrost runtime target` 或 `Recovered stale Bifrost system proxy from last runtime target`。
- `proxy_state.json` / `proxy_backup.json` 缺失不再导致 cleanup 直接跳过。
- 如果当前系统代理指向其他 host/port，则必须保留外部代理不变。

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
- one-shot daemon 在明确没有 `proxy_state.json` / `proxy_backup.json` / `runtime.json` 可恢复时应快速退出，不等待完整 retry 窗口；只有 `networksetup` 暂不可用或 macOS network service 暂时枚举为空这类启动期 transient 失败才在 60 秒窗口内有限重试。
- lifecycle helper 启动日志应包含 helper pid 和 `helper_program`，helper 由独立 process group 启动；开发环境中 `current_exe()` 指向陈旧路径时应能回退到现存 `argv[0]`。
- 所有 restore/recover/enable 路径应通过 `.system_proxy.lock` 串行化，日志包含 `waiting for system proxy cross-process file lock` 与 `acquired system proxy cross-process file lock`，避免主进程、helper、LaunchDaemon 同时写 macOS network service。
- LaunchDaemon 未安装、未加载或需要升级时，CLI start 应输出 boot/shutdown cleanup 尚未 ready 的提示；Admin API/Web UI 运行中打开系统代理时，日志应明确记录 reboot-time cleanup 在授权安装成功前不可用。用户取消授权时服务继续运行，但日志/提示必须说明重启期 cleanup 仍不可用。

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
   grep -E "system proxy lifecycle helper started after Admin API enable|LaunchDaemon cleanup install starting asynchronously after system proxy enable|LaunchDaemon cleanup installed asynchronously after system proxy enable|LaunchDaemon cleanup already installed and current after system proxy enable|LaunchDaemon cleanup install disabled by environment" "$TEST_DATA_DIR/proxy.log"
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
- 同一次 Admin API/Web UI 启用成功后，服务日志必须出现 `system proxy lifecycle helper started after Admin API enable`，确保启动时未启用 system proxy 的长驻进程在运行中启用后也具备主进程崩溃兜底恢复能力。
- 对运行中才启用 system proxy 的服务执行 `kill -9` 后，lifecycle helper 必须在父进程确认消失后清理 `127.0.0.1:18889` 残留系统代理，防止系统代理继续指向已退出的 Bifrost 端口。
- 仅版本号不一致时不会触发授权安装；`Installed version` 只作为诊断字段展示。
- 用户授权成功后，`system-proxy launchd status` 显示 `Installed: true`、`Loaded: true`、`Installed mode: one-shot`、`Needs upgrade: false`。
- 用户取消授权时，系统代理主开关保持已打开，服务继续运行，日志记录取消，不把取消误报为系统代理启用失败。
- 设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 的调试验证中，系统代理仍可打开，但日志显示 LaunchDaemon 自动安装检查被环境变量禁用。

---

### TC-CSP-20：PID 复用场景下 lifecycle helper 应执行受保护恢复（macOS + Windows）

**前置条件**：
- macOS 或 Windows 任一支持系统代理的环境（Windows 启用 WinINET）。Bifrost 不在 Linux 上写系统代理，因此 Linux 不在本用例覆盖范围内；helper 行为本身由 `bifrost-core` 单元测试在 Linux runner 上做编译/解析层面回归。
- 使用临时数据目录：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  ```

**操作步骤**：
1. 启动 Bifrost 启用系统代理（记录 PID 与 started_at）：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
2. 检查 helper 启动日志包含 `parent_started_at_ms`：
   ```bash
   grep "system proxy lifecycle cleanup helper started" "$TEST_DATA_DIR/proxy.log"
   ```
3. 强杀主进程并立即用相同 PID 启动其它进程占位（构造 PID 复用）。可借助单元测试内置的 mock 钩子或在临时脚本中 fork 多个进程直至复用同 PID（实际 CI 中由 `pid_reuse_detected_when_start_time_mismatches_current_process` 和 LaunchDaemon runtime identity 单元测试覆盖确定性场景）。
4. 观察 helper 日志：
   ```bash
   grep -E "pid_reuse_check=mismatch|running guarded cleanup|system proxy cleanup helper restore starting" "$TEST_DATA_DIR/proxy.log"
   ```

**预期结果**：
- helper 启动日志包含 `parent_started_at_ms=<u64>` 字段。
- 当 helper 观测到 `parent pid` 仍存活但 `start_time` 不再匹配 `parent_started_at_ms`（容差 2000ms 内），日志输出 `pid_reuse_check=mismatch` 与 `running guarded cleanup`，并进入 `recover_from_crash`。
- 如果当前系统代理仍指向旧 Bifrost target，则恢复原始代理或禁用残留代理；如果已经被外部代理接管，则 guarded recovery 保留外部代理不变。
- 跨平台一致：macOS 通过 `proc_pidinfo`+`PROC_PIDTBSDINFO` 取 start_time；Windows 通过 `GetProcessTimes`。Linux 不启动 helper，不在本用例覆盖范围；`bifrost-core` 在 Linux runner 上仍会执行 `process_start_time` 单元测试，确保 `/proc/<pid>/stat` 解析逻辑回归。

---

### TC-CSP-21：系统代理跨进程 lock 文件应安全可迁移（macOS）

**前置条件**：
- macOS 环境（Bifrost 当前只在 macOS 上对该 lock 做 chmod 0666，因为 Windows 不走 POSIX 模式位，Linux 不写系统代理）。
- 临时数据目录：
  ```bash
  TEST_DATA_DIR="$(mktemp -d)"
  ```

**操作步骤**：
1. 以 root 身份（或 sudo）启动一次 Bifrost 启用系统代理后立即停止：
   ```bash
   sudo BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost -p 18889 start --no-system-proxy &
   sleep 1
   sudo kill $!
   wait $! 2>/dev/null || true
   ```
2. 检查 lock 文件权限：
   ```bash
   ls -l "$TEST_DATA_DIR/.system_proxy.lock"
   stat -c '%a' "$TEST_DATA_DIR/.system_proxy.lock" 2>/dev/null \
     || stat -f '%Lp' "$TEST_DATA_DIR/.system_proxy.lock"
   ```

**预期结果**：
- lock 文件存在，权限位为 `0666`（即使创建时进程的 umask 限制 group/other 写权限，也通过 fd 级 `fchmod(0o666)` 强制放开）。
- 普通用户与 root 启动的 helper / cleanup-daemon / 主进程都可获得 advisory lock，避免 root 启动的服务给 lock 文件留下普通用户无法访问的权限。
- symlink 形式的 `.system_proxy.lock` 必须被拒绝，不能被 root LaunchDaemon 跟随 chmod；该路径由 Rust 单元测试 `system_proxy_lock_rejects_symlink` 覆盖。
- 旧版本留下的 strict regular lock 可通过隐藏命令 `system-proxy repair-lock --data-dir "$TEST_DATA_DIR"` 迁移到 `0666`；迁移内部仍使用 `O_NOFOLLOW` + fd 校验 + `fchmod`。
- 该用例由 Rust 单元测试 `system_proxy_lock_is_world_writable_after_creation` / `system_proxy_lock_rejects_symlink` 在 macOS CI 上执行，Linux/Windows 不写系统代理或不使用 POSIX mode 因此不覆盖。

---

### TC-CSP-22：lifecycle helper recover_from_crash 应在 60 秒内有限重试

**前置条件**：
- macOS 或 Windows（运行期 helper 平台）。Linux 不启动 helper，不在本用例覆盖范围；`bifrost-core` 单元测试仍会在 Linux runner 上执行重试策略测试。
- 临时数据目录：`TEST_DATA_DIR="$(mktemp -d)"`。

**操作步骤**：
1. 由 `bifrost-core` 的 `system_proxy_recovery::tests::retry_with_policy_returns_after_success`、`retry_with_policy_gives_up_after_window` 这两个单元测试在 CI 跨平台执行：
   ```bash
   cargo test -p bifrost-core system_proxy_recovery
   ```
2. helper 真实路径下，可在测试环境注入瞬时失败（如临时禁用 `networksetup`）后强杀主进程，观察日志：
   ```bash
   grep -E "system proxy recover_from_crash failed; will retry|system proxy recover_from_crash succeeded after retry" "$TEST_DATA_DIR/proxy.log"
   ```

**预期结果**：
- `RECOVERY_RETRY_WINDOW = 60s`、`RECOVERY_RETRY_INTERVAL = 5s`，重试期间 helper 不立即退出。
- 区分可重试错误（`is_retryable_recovery_error`：networksetup 暂不可用、网络服务枚举为空、临时 IO 错误）与不可重试错误（解析失败、状态文件损坏）。
- 60 秒窗口超时仍未恢复时记录最后一次错误并退出，避免 helper 永远阻塞。
- Rust 单元测试 `system_proxy_recovery::tests::*` 在 macOS / Linux / Windows 上均编译通过；运行期 helper 行为只在 macOS / Windows 验证。

---

### TC-CSP-23：lifecycle helper 在 Windows 上同样能在主进程崩溃后清理系统代理（macOS + Windows 范围）

**前置条件**：
- 仅适用于 macOS / Windows。Bifrost 不对 Linux 提供系统代理写入能力，因此 Linux 上不会启动 lifecycle helper，本用例不在 Linux 平台执行。
- macOS：参考 TC-CSP-15 已覆盖。
- Windows：Windows 10+，临时数据目录，PowerShell。

**Windows 操作步骤**（PowerShell）：
1. 启动 Bifrost：
   ```powershell
   $env:BIFROST_DATA_DIR = (New-Item -ItemType Directory -Path "$env:TEMP\bifrost-test-$(Get-Random)").FullName
   Start-Process -FilePath ".\target\debug\bifrost.exe" -ArgumentList "-p","18889","start","--skip-cert-check","--unsafe-ssl","--system-proxy" -PassThru
   ```
2. 验证 helper 日志包含 `system proxy lifecycle cleanup helper started`；Windows 下 helper 通过 `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` 启动，独立于父进程。
3. 用 `Stop-Process -Id <PID> -Force` 模拟崩溃，等待最多 45 秒：
   ```powershell
   Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -Name ProxyEnable
   ```

**Linux 行为说明**：
- Bifrost 当前不在 Linux 上写系统代理，因此 lifecycle helper 在 Linux 也直接 short-circuit 返回，日志输出 `system proxy lifecycle helper is only supported on macOS and Windows; skipping`。CI Linux 矩阵只验证 `cargo test --workspace --all-features` 通过，不验证 helper 行为。

**预期结果**：
- Windows：`ProxyEnable` 为 `0`，`ProxyServer` 不再指向 `127.0.0.1:18889`，且通知 `WinINET` 设置生效。
- macOS：参见 TC-CSP-15。
- Linux：helper 不被启动；`SystemProxyLifecycleHelperState::ensure_started` short-circuit；CI 矩阵 (`ubuntu-latest` / `windows-latest` / `macos-latest`) 均执行 `cargo test --workspace --all-features` 通过。

---

### TC-CSP-24：Admin API 关闭系统代理失败或未收敛时，lifecycle helper 不应被提前关闭

**前置条件**：
- 任意支持系统代理的平台。
- 启动 Bifrost 并通过 Admin API 启用系统代理；helper 已经存在。

**操作步骤**：
1. 通过 Admin API 关闭系统代理，但模拟外部代理覆盖或网络服务无法关闭：
   ```bash
   networksetup -setwebproxy "Wi-Fi" 127.0.0.1 18889
   networksetup -setwebproxystate "Wi-Fi" on
   curl -sS -X PUT "http://127.0.0.1:18889/_bifrost/api/proxy/system" \
     -H "Content-Type: application/json" \
     -d '{"enabled":false}'
   ```
2. 检查日志：
   ```bash
   grep -E "system proxy admin toggle did not converge to a clean state; lifecycle helper left running|system proxy lifecycle helper stopped after Admin API disable" "$TEST_DATA_DIR/proxy.log"
   ```
3. 强杀主进程：
   ```bash
   kill -9 "$PROXY_PID"
   ```
4. 等待最多 45 秒检查系统代理。

**预期结果**：
- 当 `request.enabled=false` 但 `status.enabled=true`（未收敛），日志输出 `system proxy admin toggle did not converge to a clean state; lifecycle helper left running`，helper **不被** 停止。
- 仅在 `!request.enabled && !status.enabled` 已确认收敛时，才输出 `system proxy lifecycle helper stopped after Admin API disable` 并 `stop()` helper。
- 第 4 步主进程崩溃后，仍存活的 helper 接管清理，系统代理不再指向 `127.0.0.1:18889`。
- 修复前的回归路径（"disable 失败但 helper 已被 detach/forget 而 stop"）不可复现。

---

## 执行记录

- 2026-06-07：使用当前修复后的独立 target 二进制真实执行系统代理 shell E2E。命令：`BIFROST_BIN="$PWD/.bifrost-verify-target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`。结果 14/14 PASS，覆盖 LaunchDaemon plist one-shot dry-run、外部代理不抢占、外部代理 disable 不误关、正常退出恢复、崩溃后再次启动恢复、无 backup/state 但 runtime target 匹配时清理残留、Admin API 运行中启用后主进程崩溃由 helper 清理、cleanup-daemon 无状态快速退出、启动失败前同步清理残留。脚本中 `Killed: 9` / `Terminated: 15` 为用例主动强杀或清理进程的预期动作；检测到本机存在外部系统代理 owner 时，两个非隔离 helper 用例按脚本规则跳过以避免误动正式代理。
- 2026-06-07：针对守护进程稳定性 review 修复执行 TC-CSP-20/TC-CSP-21 的确定性验证。执行 `CARGO_TARGET_DIR=./.bifrost-verify-target SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-core system_proxy --all-features`，结果 35/35 PASS，覆盖 `system_proxy_lock_is_world_writable_after_creation`、`system_proxy_lock_rejects_symlink`、`runtime_identity_is_not_alive_when_start_time_mismatches`、`runtime_identity_is_alive_when_start_time_matches`、`last_runtime_target_has_live_listener_resolves_localhost` 等回归；执行 `CARGO_TARGET_DIR=./.bifrost-verify-target SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli pid_reuse_detected_when_start_time_mismatches_current_process --all-features`，结果 lib/main 双路径均 PASS，验证 lifecycle helper 能识别 PID start_time mismatch。真实 CLI 执行 `bifrost system-proxy repair-lock --data-dir "$TEST_DATA_DIR"`，确认创建 `.system_proxy.lock` 后 `stat -f '%Lp'` 为 `666`；随后将 `.system_proxy.lock` 替换为 symlink 再执行同一命令，返回 `Too many levels of symbolic links (os error 62)`，结论 PASS：repair-lock 使用 nofollow + fd chmod，拒绝 symlink。
- 2026-06-06：针对 P0/P1 review 修复真实执行 TC-CSP-16、TC-CSP-18、TC-CSP-19 及系统代理回归套件。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_system_proxy_e2e.sh`，脚本使用临时 `BIFROST_DATA_DIR` 和 `18889` 端口，验证 LaunchDaemon plist one-shot dry-run、外部代理归属边界、崩溃后恢复、无 backup/state 但 runtime target 匹配时的残留清理、Admin API 运行中启用 system proxy 后 lifecycle helper 崩溃兜底、cleanup-daemon 无状态快速退出，以及启动失败前同步清理残留系统代理。结果 14/14 PASS，其中新增输出 `LaunchDaemon cleanup daemon 无状态时快速完成 one-shot retry-aware 检查`，证明 retry-aware one-shot 在明确无需恢复时不会等待完整 retry 窗口；测试结束后 `./target/debug/bifrost system-proxy status` 显示系统代理恢复到正式服务 `127.0.0.1:9900`。
- 2026-06-06：真实执行 TC-CSP-19 及系统代理回归套件。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_system_proxy_e2e.sh`，脚本使用临时 `BIFROST_DATA_DIR` 和 `18889` 端口，显式写入 `runtime.json` 的 `host=0.0.0.0 port=18889`，删除 `proxy_state.json` / `proxy_backup.json`，再将 macOS Web/Secure Web proxy 设置为 `127.0.0.1:18889` 后以 `--no-system-proxy` 启动当前构建。结果 13/13 PASS，其中新增用例输出 `macOS: 无 backup/state 时按 runtime target 清理崩溃残留系统代理`，证明无 managed state 时仍会按上次 runtime target 清理残留代理；测试结束后系统代理恢复到正式 9900 服务。
- 2026-06-05：真实执行 TC-CSP-18 的 Admin API 路径。使用 `/tmp/bifrost-csp18.*` 临时数据目录和 `target/debug/bifrost` 在 `18889` 端口启动服务，启动参数包含 `--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 避免本轮弹出真实授权窗口。服务 ready 后调用 `PUT /_bifrost/api/proxy/system {"enabled":true}`，响应为 `enabled=true host=127.0.0.1 port=18889 managed_by_bifrost=true`；随后日志出现 `system proxy LaunchDaemon cleanup install disabled by environment`，证明运行中服务通过 Admin API 打开系统代理后已触发 LaunchDaemon 自动检查路径。测试结束调用 API 关闭系统代理并停止临时服务，临时数据目录已清理。
- 2026-06-05：针对 PR #187 合入后 review 评论补充真实执行 TC-CSP-18 的 Admin API lifecycle helper 回归。使用 `/tmp/bifrost-admin-helper.*` 临时数据目录和 `target/debug/bifrost` 在 `18891` 端口启动服务，启动参数包含 `--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并设置 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 仅隔离 LaunchDaemon 授权弹窗。服务 ready 后调用 `PUT /_bifrost/api/proxy/system {"enabled":true}`，响应为 `enabled=true host=127.0.0.1 port=18891 managed_by_bifrost=true`；日志出现 `system proxy lifecycle helper started after Admin API enable`（helper pid `54898`）和 `system proxy LaunchDaemon cleanup install disabled by environment`。测试结束调用 API 关闭系统代理、停止临时服务并清理临时目录，结论 PASS。
- 2026-06-05：针对 Codex review 建议补充运行中启用后的崩溃兜底回归。使用独立临时数据目录启动服务并通过 Admin API 打开 system proxy，确认日志出现 `system proxy lifecycle helper started after Admin API enable` 后对主进程执行 `kill -9`；等待 lifecycle helper 检测父进程消失后，系统代理不再指向本轮 Bifrost 端口。用例已同步到 `e2e-tests/tests/test_system_proxy_e2e.sh` 的 `test_admin_api_enable_lifecycle_helper_cleans_after_parent_crash`。
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
