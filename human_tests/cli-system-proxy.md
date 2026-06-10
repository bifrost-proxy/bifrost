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
- macOS 支持系统代理的环境；本用例步骤使用 `networksetup` 验证 per-service proxy。
- Windows 等价 startup recovery 覆盖由 TC-CSP-23 和 TC-CSP-34 验证，不能复用本用例中的 `networksetup` 步骤。
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
- one-shot daemon 确认旧 runtime pid 已不存在并完成 startup recovery 后，应删除 stale `runtime.json` / `bifrost.pid`，且 stderr 不应出现 `/bin/kill` 形式的 `No such process` 噪声。
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

### TC-CSP-25：cleanup-daemon 修复 OS 代理现场后，保留配置偏好但 UI 开关只展示 live 接管状态

**前置条件**：
- macOS 机器。
- 使用临时 `TEST_DATA_DIR`，并启动一次 Bifrost，使 `config.toml` 中 `[system_proxy].enabled = true`。
- 准备一个可连接的 Bifrost 代理端口，例如 `18889`。

**操作步骤**：
1. 启动 Bifrost 并启用系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     ./target/debug/bifrost -H 127.0.0.1 -p 18889 start --unsafe-ssl --system-proxy
   ```
2. 确认 Admin API 同时返回 live 和 configured 状态：
   ```bash
   curl -s "http://127.0.0.1:18889/_bifrost/api/proxy/system" | jq '{enabled, managed_by_bifrost, configured_enabled}'
   ```
3. 模拟异常退出或重启后的 cleanup 路径，使 OS 系统代理被恢复/关闭，但不要调用 Admin API 关闭开关：
   ```bash
   kill -9 "$PROXY_PID"
   sudo ./target/debug/bifrost system-proxy cleanup-daemon --data-dir "$TEST_DATA_DIR"
   ```
4. 检查配置文件：
   ```bash
   grep -A3 '^\[system_proxy\]' "$TEST_DATA_DIR/config.toml"
   ```
5. 再次启动 Bifrost，打开 Settings -> Proxy，查看 `Enable System Proxy` 开关；或直接请求：
   ```bash
   curl -s "http://127.0.0.1:18889/_bifrost/api/proxy/system" | jq '{enabled, managed_by_bifrost, configured_enabled}'
   ```
6. 如果系统代理当前指向外部代理（例如正式服务 `127.0.0.1:9900`），确认 Settings -> Proxy 的 warning 显示 `System proxy is occupied by another proxy`，但 `Enable System Proxy` 开关显示关闭，并且仍可点击让当前 Bifrost 接管。

**预期结果**：
- cleanup-daemon 可以把 macOS live OS 代理恢复为关闭或原始外部代理；这是正确的现场清理。
- cleanup-daemon 不修改 `$TEST_DATA_DIR/config.toml`，`[system_proxy].enabled` 保持 `true`，除非用户显式通过 CLI/Admin API/UI 关闭。
- Admin API 返回 `configured_enabled=true`，即使 cleanup 刚执行完导致 live `enabled=false`。
- Settings、StatusBar、Traffic toolbar 的系统代理开关只以 live Bifrost 接管状态为准：只有 `enabled=true && managed_by_bifrost!=false` 才显示打开。
- 当 `configured_enabled=true` 但 live OS 代理未由当前 Bifrost 接管时，UI 可以展示 pending/occupied warning，但开关必须显示关闭，保证用户能点击并强制接管。

---

### TC-CSP-26：cleanup-daemon 遇到 macOS network services 未 ready 时应持续定时重试

**前置条件**：
- macOS 机器。
- 已构建当前源码 `target/debug/bifrost`。
- 使用临时 `TEST_DATA_DIR`，避免污染默认 `~/.bifrost`。

**操作步骤**：
1. 确认普通 macOS 设备可通过真实命令列出网络服务；注意这不是 `network service list` 命令：
   ```bash
   networksetup -listallnetworkservices
   ```
2. 在临时目录写入 Bifrost managed state，模拟关机前系统代理由 Bifrost 管理：
   ```bash
   cat > "$TEST_DATA_DIR/proxy_state.json" <<'EOF'
   {
     "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
     "target": {"enable": true, "host": "127.0.0.1", "port": 18889, "bypass": ""},
     "applied": true
   }
   EOF
   cat > "$TEST_DATA_DIR/proxy_backup.json" <<'EOF'
   {"enable": false, "host": "", "port": 0, "bypass": ""}
   EOF
   ```
3. 使用测试脚本中的 fake `networksetup` 场景执行回归：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true \
     BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 \
     bash e2e-tests/tests/test_system_proxy_e2e.sh
   ```
4. 检查输出中包含：
   ```text
   macOS: cleanup-daemon 等待 network services ready 后才完成恢复判定
   ```

**预期结果**：
- 普通 macOS 设备不需要启用额外功能；如果用户直接执行 `network service list`，shell 报 `command not found` 是因为该字符串不是 macOS 命令，真实命令是 `networksetup -listallnetworkservices`。
- fake `networksetup -listallnetworkservices` 前两次只返回 header、无 enabled service 时，cleanup-daemon 不应退回 `scutil --proxy` 聚合状态并删除 `proxy_state.json`。
- cleanup-daemon 应保持 one-shot 进程存活并按固定间隔重试；直到第三次能读取真实 service list 后，才继续执行恢复判断。
- 日志包含 `retryable error; retrying`，并且 `keep_waiting_for_network_services=true`。

---

### TC-CSP-27：更新后执行 `bifrost restart` 时，fresh daemon 应重新接管系统代理

**前置条件**：
- macOS 支持系统代理的环境。
- 当前没有外部代理 owner 占用本轮测试端口；如存在正式 Bifrost 9900 或 Surge 系统代理，应先记录快照并避免本用例抢占外部 owner。
- 使用临时数据目录与非默认端口：
  ```bash
  source ~/.zshrc && TEST_DATA_DIR="$(mktemp -d)"
  source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo build --bin bifrost
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   ```
2. 等待 Admin API ready，并确认系统代理指向旧进程：
   ```bash
   source ~/.zshrc && until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   source ~/.zshrc && networksetup -getwebproxy "Wi-Fi"
   ```
3. 执行重启：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost restart
   ```
4. 等待 `runtime.json` 中 pid 更新且新 Admin API ready：
   ```bash
   source ~/.zshrc && for i in $(seq 1 90); do curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1 && break; sleep 1; done
   source ~/.zshrc && cat "$TEST_DATA_DIR/runtime.json"
   ```
5. 再次检查系统代理：
   ```bash
   source ~/.zshrc && networksetup -getwebproxy "Wi-Fi"
   source ~/.zshrc && networksetup -getsecurewebproxy "Wi-Fi"
   ```
6. 检查 restart 日志：
   ```bash
   source ~/.zshrc && tail -n 120 "$TEST_DATA_DIR/logs/restart.log"
   ```
7. 停止新 daemon，并按测试前快照恢复系统代理。

**预期结果**：
- 第 3 步 `bifrost restart` 返回 0，并输出 `Restart scheduled`。
- restart 过程中旧进程 stop 可以按正常语义恢复/关闭系统代理；这是允许的短暂清理窗口。
- 第 4 步 `runtime.json` 中 pid 已更新，新 daemon 可访问。
- 第 5 步 Web/Secure Web proxy 最终重新指向 `127.0.0.1:18889`，不会停留在 `Enabled: No`。
- 第 6 步 `restart.log` 的 fresh start argv 包含 `--system-proxy`，如旧系统代理有 bypass，则包含 `--proxy-bypass <旧 bypass>`。
- 如果 stop 前系统代理并不指向旧 Bifrost runtime（外部 owner 或已关闭），restart 不应强行追加 `--system-proxy`，也不应抢占外部代理。

---

### TC-CSP-28：系统代理 lifecycle event log 应记录 enable、wake、cleanup 和 helper 状态

**前置条件**：
- macOS 或 Windows 支持系统代理的环境；Windows 使用 `target\debug\bifrost.exe` 和 PowerShell 等价命令执行。
- 已构建当前源码 `target/debug/bifrost`。
- 使用临时数据目录，避免污染正式配置：
  ```bash
  source ~/.zshrc && TEST_DATA_DIR="$(mktemp -d)"
  ```

**操作步骤**：
1. 启动 Bifrost 并显式启用系统代理：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   PROXY_PID=$!
   ```
2. 等待 Admin API ready：
   ```bash
   source ~/.zshrc && until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   ```
3. 检查结构化 lifecycle event log：
   ```bash
   source ~/.zshrc && jq -r '.event' "$TEST_DATA_DIR/logs/system_proxy_events.jsonl"
   ```
4. 停止服务并再次检查 cleanup 相关事件：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost stop
   source ~/.zshrc && jq -r '.event' "$TEST_DATA_DIR/logs/system_proxy_events.jsonl"
   ```

**预期结果**：
- event log 为 JSONL，每一行都能被 `jq` 解析。
- 启动阶段包含 `startup_state_snapshot`、`system_proxy_enable_requested`、`system_proxy_enable_applied`、`helper_start_requested`、`helper_started`。
- helper 存活期间 `system_proxy_owner_state.json` 的 `helper_last_heartbeat_at` 持续更新，并包含 helper pid、start time 和 runtime id。
- event log 不按 5 秒频率写入 `helper_heartbeat`；JSONL 只记录 `helper_started`、`helper_heartbeat_stale`、`helper_heartbeat_recovered`、`helper_missing` 等状态跃迁事件。
- 停止或 cleanup 阶段包含 `cleanup_started` 和 `cleanup_restored`，或在外部代理接管时包含 `cleanup_skipped_external_owner`。
- 日志不包含 URL path、cookie、authorization header、请求体等流量敏感信息。

---

### TC-CSP-29：`bifrost status` 应展示系统代理残留、helper 缺失和外部代理归属诊断

**前置条件**：
- macOS 或 Windows 支持系统代理的环境；Windows 使用 `target\debug\bifrost.exe` 和 PowerShell 等价命令执行。
- 使用临时数据目录和非默认端口。
- 当前系统代理初始状态已快照，测试后必须恢复。

**操作步骤**：
1. 启动 Bifrost 并启用系统代理，确认 status 正常：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   source ~/.zshrc && until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost status
   ```
2. 模拟 helper 缺失或 heartbeat 超时，然后再次执行 status。
3. 模拟 listener dead 但系统代理仍指向 `127.0.0.1:18889`，再次执行 status。
4. 将系统代理改为外部端口，例如 `127.0.0.1:6152`，再次执行 status。

**预期结果**：
- 正常状态输出包含 `System proxy: managed by Bifrost`、listener alive、helper alive、LaunchDaemon installed/loaded/current 信息。
- helper 缺失或 heartbeat 超时时，输出明确 warning，提示 lifecycle cleanup helper 不可用。
- listener dead 且系统代理仍指向 Bifrost target 时，输出：
  ```text
  System proxy points to Bifrost 127.0.0.1:18889, but listener is not reachable.
  ```
  并给出 `bifrost system-proxy restore` 或等价恢复建议。
- 外部代理占用时，输出 `occupied by another proxy` 或 `managed_by_bifrost=false` 语义，不提示清理外部代理。

---

### TC-CSP-30：`doctor system-proxy` 应生成诊断包并默认脱敏

**前置条件**：
- macOS 或 Windows 支持系统代理的环境；Windows 使用 `target\debug\bifrost.exe` 和 PowerShell 等价命令执行。
- 已存在一次启用系统代理、helper heartbeat 和 cleanup/reconcile 的 lifecycle event。
- 使用临时输出目录：
  ```bash
  source ~/.zshrc && DOCTOR_OUT="$(mktemp -d)"
  ```

**操作步骤**：
1. 执行诊断命令：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost doctor system-proxy --since "2026-06-08 00:00" --bundle "$DOCTOR_OUT/bifrost-system-proxy-diagnostic.zip"
   ```
2. 解压诊断包：
   ```bash
   source ~/.zshrc && unzip -l "$DOCTOR_OUT/bifrost-system-proxy-diagnostic.zip"
   source ~/.zshrc && unzip -q "$DOCTOR_OUT/bifrost-system-proxy-diagnostic.zip" -d "$DOCTOR_OUT/unpacked"
   ```
3. 查看 summary：
   ```bash
   source ~/.zshrc && sed -n '1,160p' "$DOCTOR_OUT/unpacked/summary.txt"
   ```
4. 检查脱敏：
   ```bash
   source ~/.zshrc && rg -n "Authorization|Cookie|token=|Set-Cookie" "$DOCTOR_OUT/unpacked" || true
   ```

**预期结果**：
- zip 中包含 summary、version/status 输出、`scutil --proxy`、`networksetup` snapshot、runtime/proxy/owner state、`system_proxy_events.jsonl`、cleanup daemon 日志。
- summary 能输出诊断结论，例如 previous runtime clean/unclean、当前系统代理 target、listener alive/dead、helper alive/missing、launchd installed/loaded。
- 默认不包含 request body、cookie、authorization header、token query 等敏感内容。
- 若当前系统代理指向外部代理，summary 明确标记 external owner，不建议恢复外部代理。
- Windows bundle 不包含 `scutil --proxy`、`networksetup`、LaunchDaemon、macOS unified log；对应采集项必须在 `collection_manifest.json` 中标记为 `not_applicable`。

---

### TC-CSP-31：睡眠唤醒后 listener 存活但网络栈未 ready 时，不应误恢复系统代理

**前置条件**：
- macOS 支持系统代理的环境。
- 使用临时数据目录启动 Bifrost，并启用系统代理。
- 准备 fake upstream 或网络故障注入，使代理请求出现 `Network is unreachable`、`Can't assign requested address` 或 DNS lookup failed，但 Bifrost listener 仍可访问。

**操作步骤**：
1. 启动 Bifrost：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   ```
2. 等待 Admin API ready，并确认系统代理指向 `127.0.0.1:18889`。
3. 通过测试 hook、fake networksetup 或手动合盖/唤醒制造 wake-gap。
4. 在 wake-gap 后立即发起若干上游请求，使 request log 出现网络栈未 ready 类错误，但确认 listener 仍可达：
   ```bash
   source ~/.zshrc && curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null
   ```
5. 检查 lifecycle event：
   ```bash
   source ~/.zshrc && jq -r 'select(.event=="network_stack_unready_summary" or .event=="cleanup_restored")' "$TEST_DATA_DIR/logs/system_proxy_events.jsonl"
   ```

**预期结果**：
- wake-gap 后 event log 包含 `wake_gap_detected`。
- listener alive 时，即使出现 `ENETUNREACH` / `EADDRNOTAVAIL` / DNS lookup failed 聚合，也只写 `network_stack_unready_summary`。
- 不出现 `cleanup_restored` 或 `cleanup_disabled_stale_proxy`，系统代理仍保持指向当前 Bifrost listener。
- 如果 listener 后续确实不可达，才允许进入 guarded restore。

---

### TC-CSP-32：macOS 原生 wake notification 应触发系统代理检查

**前置条件**：
- macOS 支持系统代理的环境。
- 当前实现已在 lifecycle helper 内启用 IOKit power notification watcher。
- 使用临时数据目录启动 Bifrost，并启用系统代理。
- 当前系统代理初始状态已快照，测试后必须恢复。

**操作步骤**：
1. 启动 Bifrost：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --skip-cert-check --unsafe-ssl --system-proxy > "$TEST_DATA_DIR/proxy.log" 2>&1 &
   ```
2. 等待 Admin API ready，并确认 lifecycle helper 与 watcher 启动：
   ```bash
   source ~/.zshrc && until curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null 2>&1; do sleep 0.2; done
   source ~/.zshrc && grep -E "system proxy lifecycle cleanup helper started|system proxy lifecycle helper started after Admin API enable" "$TEST_DATA_DIR/proxy.log" "$TEST_DATA_DIR"/logs/bifrost*.log
   source ~/.zshrc && grep -E "macOS power notification watcher started|system proxy lifecycle helper power watcher started" "$TEST_DATA_DIR/proxy.log" "$TEST_DATA_DIR"/logs/bifrost*.log
   ```
3. 让 Mac 进入睡眠：可以手动合盖、点击 Apple 菜单 Sleep，或在可控测试机上执行：
   ```bash
   source ~/.zshrc && pmset sleepnow
   ```
4. 唤醒 Mac，等待 10 秒。
5. 检查 lifecycle helper 日志：
   ```bash
   source ~/.zshrc && grep -E "SystemWillSleep|SystemWillPowerOn|SystemHasPoweredOn|system proxy lifecycle helper received power notification|system proxy wake reconcile starting|system proxy wake reconcile reapplied proxy for live runtime|runtime_restart_started|runtime_restart_succeeded" "$TEST_DATA_DIR/proxy.log" "$TEST_DATA_DIR"/logs/bifrost*.log
   ```
6. 检查系统代理和 listener：
   ```bash
   source ~/.zshrc && curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null
   source ~/.zshrc && networksetup -getwebproxy "Wi-Fi"
   source ~/.zshrc && networksetup -getsecurewebproxy "Wi-Fi"
   ```

**预期结果**：
- 日志包含 `macOS power notification watcher started` 和 `system proxy lifecycle helper power watcher started`。
- watcher 启动日志来自 lifecycle helper，而不是主进程；helper 启动日志中应包含 helper pid、parent pid、data dir 和 helper program。
- 睡眠前包含 `SystemWillSleep`；如果本次是 idle sleep，也应包含 `CanSystemSleep` 且应用未阻止系统睡眠。
- 唤醒过程中包含 `SystemWillPowerOn` 和 `SystemHasPoweredOn`；如果部分机型不稳定提供 early wake 事件，至少必须记录 `SystemHasPoweredOn` 或明确的 watcher warning。
- `SystemHasPoweredOn` 后触发 `system proxy wake reconcile starting`，并写出 `system proxy wake reconcile reapplied proxy for live runtime`、`runtime_restart_started`/`runtime_restart_succeeded` 或 guarded cleanup 之一。
- listener 仍可达时，不误执行 `cleanup_restored` / `cleanup_disabled_stale_proxy`。
- listener 不可达且系统代理仍指向 Bifrost target 时，进入 guarded restore 并写出对应 cleanup event。
- 如果 watcher 初始化失败，必须写 `system proxy lifecycle helper power watcher failed to start`，helper parent-death cleanup 仍保留，且现有 scheduler wake-gap reconcile 仍能兜底。

---

### TC-CSP-33：Windows 系统代理诊断不应启用 macOS-only wake watcher

**前置条件**：
- Windows 环境。
- 已构建当前源码 `target\debug\bifrost.exe`。
- 使用临时数据目录，避免污染正式配置：
  ```powershell
  $env:BIFROST_DATA_DIR = Join-Path $env:TEMP ("bifrost-csp33-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Force $env:BIFROST_DATA_DIR | Out-Null
  ```

**操作步骤**：
1. 启动 Bifrost 并启用系统代理：
   ```powershell
   Start-Process -FilePath ".\target\debug\bifrost.exe" -ArgumentList "-p","18889","start","--skip-cert-check","--unsafe-ssl","--system-proxy" -PassThru
   ```
2. 等待服务 ready：
   ```powershell
   for ($i = 0; $i -lt 60; $i++) {
     try {
       Invoke-WebRequest "http://127.0.0.1:18889/_bifrost/api/system" -UseBasicParsing | Out-Null
       break
     } catch { Start-Sleep -Milliseconds 500 }
   }
   ```
3. 检查 owner state：
   ```powershell
   Get-Content (Join-Path $env:BIFROST_DATA_DIR "system_proxy_owner_state.json") | ConvertFrom-Json
   ```
4. 执行 status：
   ```powershell
   .\target\debug\bifrost.exe status
   ```
5. 执行 doctor：
   ```powershell
   .\target\debug\bifrost.exe doctor system-proxy --bundle (Join-Path $env:BIFROST_DATA_DIR "doctor.zip")
   ```

**预期结果**：
- owner state 中 `wake_watcher_status` 为 `unsupported`。
- event log 不包含 `wake_notification_watcher_started`、`system_can_sleep`、`system_will_sleep`、`system_has_powered_on`。
- status 展示 wake watcher 为 not applicable / unsupported，不显示 warning，不触发 helper restart。
- doctor bundle 的 `collection_manifest.json` 中 macOS-only 项（`networksetup`、`scutil --proxy`、LaunchDaemon、macOS unified log、`/var/log/bifrost-system-proxy-cleanup.*`）标记为 `not_applicable`。
- Windows 系统代理仍可由 helper parent-death cleanup 保护；不因 wake watcher unsupported 影响 cleanup。

---

### TC-CSP-34：托管运行时 listener dead 且系统代理残留时应优先自动重启主进程

**前置条件**：
- macOS daemon/desktop 托管模式必须执行；Windows 只执行 parent-death 或 startup recovery 触发分支，不执行 sleep/wake 分支。
- 使用临时数据目录和非默认端口。
- 当前系统代理初始状态已快照，测试后必须恢复。
- 测试实现需提供可控 hook 或 fixture，使 `runtime_start_mode=daemon|desktop`、`restartable_runtime=true`，并能模拟 listener dead 但系统代理仍指向 Bifrost target。

**macOS 操作步骤**：
1. 以 daemon 或 desktop 托管模式启动 Bifrost，并显式启用系统代理：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost -p 18889 start --daemon --skip-cert-check --unsafe-ssl --system-proxy -y
   ```
2. 确认 owner state 记录可托管运行时：
   ```bash
   source ~/.zshrc && jq '.runtime_start_mode, .restartable_runtime' "$TEST_DATA_DIR/system_proxy_owner_state.json"
   ```
3. 强制结束主进程，使 listener 不可达，但保留系统代理指向 `127.0.0.1:18889`。
4. 通过测试 hook 触发 wake reconcile，或在可控测试机上 sleep/wake。
5. 检查 lifecycle event：
   ```bash
   source ~/.zshrc && jq -r '.event' "$TEST_DATA_DIR/logs/system_proxy_events.jsonl" | rg "runtime_restart_considered|runtime_restart_started|runtime_restart_succeeded|wake_notification_reconcile_restarted_runtime"
   ```
6. 检查 listener 与系统代理：
   ```bash
   source ~/.zshrc && curl -sS "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null
   source ~/.zshrc && networksetup -getwebproxy "Wi-Fi"
   source ~/.zshrc && networksetup -getsecurewebproxy "Wi-Fi"
   ```

**Windows 操作步骤**：
1. 使用临时数据目录以可托管模式启动 Bifrost 并启用系统代理。
2. 强制结束主进程，使 helper parent-death 或下一次 startup recovery 触发判断。
3. 检查 event log 包含 `runtime_restart_considered`、`runtime_restart_started`、`runtime_restart_succeeded`。
4. 确认 `wake_watcher_status=unsupported`，且未出现 macOS wake notification event。

**预期结果**：
- listener dead 且当前系统代理仍匹配 Bifrost target 时，先写 `runtime_restart_considered`，再尝试自动重启托管主进程。
- 重启成功时写 `runtime_restart_succeeded` 和 `wake_notification_reconcile_restarted_runtime` 或 parent-death 等价 completed event，系统代理继续指向 Bifrost，新 listener 可达。
- 前台 `bifrost start`、用户 clean stop、外部代理抢占、runtime identity 不可信、binary path/data dir 不可信时，不自动重启，写 `runtime_restart_skipped` 并进入 guarded restore/disable。
- 重启失败或超时时写 `runtime_restart_failed`，随后 restore original 或 disable stale Bifrost target，避免用户网络继续指向死端口。
- Windows 不出现 IOKit/wake notification 事件；Windows 分支只验证托管 runtime restart-before-restore 和 fallback restore，不要求 `networksetup`、`pmset` 或 LaunchDaemon。

---

### TC-CSP-35：Admin API / WebView / CLI 显式关闭应优先于脏 system proxy backup

**前置条件**：
- macOS 机器。
- 使用临时数据目录和非默认端口，避免影响正式 `~/.bifrost`。
- 当前系统代理初始状态已快照，测试结束必须恢复。

**操作步骤**：
1. 构造临时数据目录并启动 Bifrost，但先不要让启动参数自动开启系统代理：
   ```bash
   source ~/.zshrc && TEST_DATA_DIR="$(mktemp -d /tmp/bifrost-csp35.XXXXXX)"
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 ./target/debug/bifrost start -p 18889 --skip-cert-check --unsafe-ssl --no-system-proxy >"$TEST_DATA_DIR/proxy.log" 2>&1 &
   source ~/.zshrc && PROXY_PID=$!
   ```
2. 等待 Admin API ready，然后通过 Admin API / WebView 等价接口开启系统代理：
   ```bash
   source ~/.zshrc && curl -sS --retry 30 --retry-delay 1 "http://127.0.0.1:18889/_bifrost/api/system" >/dev/null
   source ~/.zshrc && curl -sS -X PUT "http://127.0.0.1:18889/_bifrost/api/proxy/system" -H "Content-Type: application/json" -d '{"enabled":true}'
   ```
3. 模拟 0.0.95 现场的脏备份：把 `proxy_backup.json` 写成当前 Bifrost target，并删除 `proxy_state.json`：
   ```bash
   source ~/.zshrc && printf '{"enable":true,"host":"127.0.0.1","port":18889,"bypass":"localhost,127.0.0.1,::1,*.local"}' > "$TEST_DATA_DIR/proxy_backup.json"
   source ~/.zshrc && rm -f "$TEST_DATA_DIR/proxy_state.json"
   ```
4. 通过 Admin API / WebView 等价接口关闭系统代理：
   ```bash
   source ~/.zshrc && curl -sS -X PUT "http://127.0.0.1:18889/_bifrost/api/proxy/system" -H "Content-Type: application/json" -d '{"enabled":false}'
   ```
5. 轮询确认 OS system proxy 不再由本轮 Bifrost 管理：
   ```bash
   source ~/.zshrc && curl -sS "http://127.0.0.1:18889/_bifrost/api/proxy/system"
   source ~/.zshrc && networksetup -getwebproxy "Wi-Fi"
   source ~/.zshrc && networksetup -getsecurewebproxy "Wi-Fi"
   ```
6. 检查日志：
   ```bash
   source ~/.zshrc && grep -E "explicit system proxy disable ignored saved backup|system proxy reconcile skipped because runtime desired state is disabled|system proxy lifecycle helper stopped after Admin API disable" "$TEST_DATA_DIR/proxy.log"
   ```
7. CLI 等价验证：重复第 1-3 步准备脏 backup，然后执行：
   ```bash
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost system-proxy disable
   source ~/.zshrc && BIFROST_DATA_DIR="$TEST_DATA_DIR" ./target/debug/bifrost system-proxy status
   ```

**预期结果**：
- 第 4 步接口返回 `managed_by_bifrost=false`，且 `configured_enabled=false`。
- 系统代理不会恢复到脏 backup 中的 `127.0.0.1:18889`。
- 关闭后等待超过一个 reconcile 周期，系统代理仍不会被后台 reconcile 重新打开。
- 日志包含 `explicit system proxy disable ignored saved backup because it points back to the managed Bifrost target`，证明脏 backup 被用户显式关闭语义覆盖。
- 如果用户机器上关闭后被其它外部代理接管，Admin API 仍可返回 `enabled=true`，但必须满足 `managed_by_bifrost=false`，WebView 开关应显示关闭。
- CLI disable 在运行中服务可达时优先走 Admin API，输出 `✓ System proxy disabled via running Bifrost`；Admin API 不可用但 `runtime.json` 存在时，CLI 仍能用 runtime host/port fallback 执行 explicit disable。
- CLI status 输出 `Managed by Bifrost`、`Configured enabled`、`Configured bypass`；当系统代理由外部代理接管时，显示外部代理提示，不把外部 enabled 误报成 Bifrost 管理。

---

### TC-CSP-36：stop 前置清理系统代理，macOS 多 network service 写入有界并行

**前置条件**：
- macOS 机器。
- 使用临时数据目录和非默认端口，避免影响正式 `~/.bifrost`。
- 当前系统代理初始状态已快照，测试结束必须恢复。

**操作步骤**：
1. 执行 focused 单测，验证 `foreground_cleanup` marker 与系统代理模块编译/单元逻辑：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-core system_proxy_shutdown_mode_marker_is_read_and_consumed -- --nocapture
   source ~/.zshrc && cargo test -p bifrost-core system_proxy -- --nocapture
   ```
2. 执行 stop/restart shutdown marker E2E：
   ```bash
   source ~/.zshrc && cargo build --bin bifrost
   source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh
   ```
3. 执行系统代理真实场景 E2E：
   ```bash
   source ~/.zshrc && BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh
   ```
4. 检查代码路径：
   ```bash
   source ~/.zshrc && rg -n "run_macos_services_parallel|MACOS_NETWORKSETUP_MAX_PARALLEL_SERVICES|ForegroundCleanup" crates/bifrost-core/src/system_proxy.rs crates/bifrost-core/src/system_proxy_launchd.rs crates/bifrost-cli/src/commands
   ```

**预期结果**：
- `bifrost stop` 输出 `Cleaning system proxy before stopping Bifrost proxy...`，且不输出 `System proxy cleanup continues in background if needed.`。
- stop 在系统代理 cleanup 成功后才停止主服务；如果 cleanup 失败，应返回错误并保持主服务运行，避免 OS proxy 指向 dead listener。
- restart / upgrade restart 使用 `preserve_for_restart` marker，旧 daemon 和 lifecycle helper 不清理系统代理，fresh daemon 继续接管同一 host/port。
- fresh `start --system-proxy` 在 marker 与旧 runtime 同时存在时跳过启动前 crash recovery；无 runtime 或不启用系统代理时必须执行普通 recovery。进入新 runtime 前失败时由 startup guard 兜底 recovery。
- restart handoff 在进入新 runtime 前失败时，启动期 guard 会执行 crash recovery，避免 system proxy 挂在已停旧 listener。
- restart handoff 在 exec fresh daemon 前 re-apply 系统代理失败时，默认中止并执行 crash recovery；只有用户显式传 `--force` 时才继续，以免系统代理状态不确定。
- Linux 暂不支持 Bifrost 托管系统代理写入，`SystemProxyManager::is_supported()` 必须返回 false；Linux CI 只验证无系统代理 restart/stop 等平台一致路径，不启动 lifecycle helper，也不写 shutdown marker。
- macOS 普通 `networksetup` 与 sudo 写入/恢复/disable 路径按 network service 有界并行；单个 service 内 HTTP/HTTPS/bypass 配置顺序保持不变；GUI 授权路径不并行弹窗。
- `.system_proxy.lock` 仍然覆盖并行写入外层，跨进程不会同时修改同一批系统代理配置。

---

## 执行记录

- 2026-06-10：针对 stop/restart 系统代理顺序与 macOS 多 network service 写入加速，新增并执行 TC-CSP-36 自动化子集。执行 `source ~/.zshrc && cargo test -p bifrost-core system_proxy_shutdown_mode_marker_is_read_and_consumed -- --nocapture`，结果 1/1 PASS，覆盖 `preserve_for_restart`、`background_cleanup`、`foreground_cleanup` marker read/consume；执行 `source ~/.zshrc && cargo test -p bifrost-cli restart -- --nocapture`，结果 lib/main 各 18/18 PASS，覆盖 restart argv 保留 `--system-proxy`、`--skip-cert-check`、旧 runtime host/socks5、stop 失败 abort 与启动侧 handoff guard 相关路径；执行 `source ~/.zshrc && cargo test -p bifrost-core system_proxy -- --nocapture`，结果 44/44 PASS，覆盖系统代理恢复/锁/launchd/retry 相关单测；执行 `source ~/.zshrc && cargo build --bin bifrost && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh`，结果 10/10 PASS，stop 输出 `Cleaning system proxy before stopping Bifrost proxy...` 后才输出 `Stopping Bifrost proxy`，且未输出后台 cleanup 提示、未升级 SIGKILL；同一脚本通过 fake `scutil` / `networksetup` 隔离执行真实 `bifrost restart`，确认 fake 系统代理在旧 daemon 停止、端口释放、fresh daemon ready 期间持续指向同一 Bifrost host/port，补齐外部 system proxy owner 场景下原非隔离 restart 用例会跳过的覆盖缺口。执行 `source ~/.zshrc && BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`，结果 18/18 PASS。当前机器存在外部/正式 system proxy owner，suite 按保护规则跳过非隔离 restart、crash、Admin API helper、脏 backup、启动失败前恢复等真实写入用例，未抢占正式代理；无 backup/state runtime target 清理、LaunchDaemon one-shot、network services readiness retry 等可隔离路径均通过。
- 2026-06-10 CI 复核：PR CI 的 `E2E Shell (aarch64-apple-darwin, shard 1/3)` 首轮发现 `test_stop_restart_shutdown_marker.sh` 在 restart handoff 里未看到 fresh daemon ready，随后本地复现到旧 daemon shutdown marker skip 后底层 `SystemProxyManager::Drop` 仍可能二次 restore，导致 fake system proxy 从 `Yes` 回到 `No`。修复为 marker-skip 分支显式 `detach_in_place()` manager，并让 restart orphan 在 exec fresh daemon 前直接 re-apply 一次旧 runtime target（不再做额外 owner inspect，避免 CI 上系统代理读取阻塞）。复跑 `source ~/.zshrc && cargo fmt --all -- --check && cargo test -p bifrost-cli restart_handoff_recovery -- --nocapture && cargo build --bin bifrost && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh`，结果 startup handoff 单测 lib/main 各 3/3 PASS，E2E 10/10 PASS；复跑 `source ~/.zshrc && cargo test -p bifrost-core system_proxy -- --nocapture && cargo test -p bifrost-cli restart -- --nocapture`，结果 core 44/44 PASS，CLI restart lib/main 各 19/19 PASS，cli_commands 3/3 PASS。
- 2026-06-10 跨平台 review 补强：确认普通 stop 的 shutdown marker 仅在托管系统代理平台写入，且普通 stop marker 写入失败时降级为 warning 后继续前台 cleanup；restart 的 `preserve_for_restart` marker 在托管系统代理平台仍要求写入成功。`SystemProxyManager::is_supported()` 收敛为 macOS/Windows，Linux 固定 false，避免 Linux CI 暴露半成品系统代理写入路径。`test_stop_restart_shutdown_marker.sh` 新增无系统代理 restart 子用例，验证 Linux/macOS 都能完成 fresh daemon handoff、restart argv 不含 `--system-proxy` 且不残留 shutdown marker。
- 2026-06-10 本轮复测：执行 `source ~/.zshrc && cargo test -p bifrost-core system_proxy -- --nocapture`，结果 44/44 PASS，其中 `test_is_supported` 现在按平台断言 Linux 固定 false、macOS/Windows 对齐底层 `sysproxy`；执行 `source ~/.zshrc && cargo test -p bifrost-cli restart -- --nocapture`，结果 lib/main 各 19/19 PASS、cli_commands 3/3 PASS；执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh`，结果 14/14 PASS，覆盖无系统代理 restart 跨平台子用例与 macOS fake 系统代理 handoff。
- 2026-06-10 CI shell 覆盖确认：执行 `source ~/.zshrc && BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests`，输出包含 `test_stop_restart_shutdown_marker.sh`；shard 2/3 与 3/3 不包含该脚本，说明该跨平台用例会在 Linux/macOS shell CI shard 1 中真实执行，而不是仅本地人工验证。
- 2026-06-09：针对 macOS 合盖睡眠后主进程 scheduler 未感知 wake gap、lifecycle helper 尚未注册原生 wake notification 的问题，补充并执行 TC-CSP-32 相关自动化子集。第一轮执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh` 发现 `IORegisterForSystemPower` FFI 签名错误导致 hidden `system-proxy lifecycle-helper` 以 exit code 139 退出，Admin API 运行中启用后的 crash cleanup 也因此未清理 18889 系统代理残留；修复 FFI 后直接执行 hidden helper，确认 helper 3 秒后仍存活，`$TEST_DATA_DIR/logs/bifrost.2026-06-09.log` 包含 `macOS power notification watcher started` 和 `system proxy lifecycle helper power watcher started`。最终重跑系统代理 E2E，结果 18/18 PASS，覆盖 direct lifecycle helper 注册 IOKit power watcher、无 backup/state runtime target 清理、Admin API crash cleanup 等路径；本机正式 system proxy owner 为 `127.0.0.1:9900`，脚本按外部 owner 保护规则跳过非隔离睡眠漂移、restart 保持、daemon restart-before-restore、脏 backup disable 和启动失败前恢复用例，避免误抢正式代理。
- 2026-06-09：针对 0.0.95 WebView 停止系统代理后又恢复为开启的脏 backup 回归，补充 TC-CSP-35 和脚本自动化 `test_admin_api_disable_ignores_dirty_backup_pointing_to_self`。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`，结果 17/17 PASS；当前机器存在正式 system proxy owner `127.0.0.1:9900`，新增 TC-CSP-35 脚本用例按外部 owner 保护规则跳过非隔离真实写入，避免误抢正式代理。执行 `cargo test -p bifrost-core explicit_disable -- --nocapture`，结果 2/2 PASS，覆盖脏 backup host/port 指回当前 target 时即使 bypass 不同也应忽略；执行 `cargo test -p bifrost-admin proxy::tests -- --nocapture`，结果 4/4 PASS，覆盖 disable verification；执行 `cargo test -p bifrost-cli commands::system_proxy::tests -- --nocapture`，结果 lib/main 各 6/6 PASS，覆盖 CLI runtime target fallback、wildcard host 到 loopback 映射和托管 runtime restart 参数。
- 2026-06-09：针对托管 daemon 主进程崩溃后系统代理继续指向旧 listener 的可靠性问题，先落地 TC-CSP-34 的 helper parent-death restart-before-restore 子集。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`，结果 16/16 PASS。当前机器存在正式 system proxy owner `127.0.0.1:9900`，因此新增脚本用例 `test_lifecycle_helper_restarts_restartable_daemon_after_parent_crash` 按外部 owner 保护规则跳过非隔离真实写入，避免误抢正式代理；本轮同时执行 `cargo test -p bifrost-cli process::tests -- --nocapture` 和 `cargo test -p bifrost-cli commands::system_proxy::tests -- --nocapture`，覆盖 `runtime_start_mode` / `restartable_runtime` 兼容读写、foreground 不自动重启、缺少 binary path 不重启、daemon restart argv 保留端口/host/socks5/bypass 和系统代理接管参数。
- 2026-06-09：针对重启后系统代理残留且 cleanup-daemon 在 `networksetup -listallnetworkservices` 暂时返回空 service list 时提前退出的问题，补充 TC-CSP-26。真实现场取证：`launchctl print system/com.bifrost.system-proxy-cleanup` 显示 one-shot daemon 已运行且 exit code 为 0；`/var/log/bifrost-system-proxy-cleanup.log` 显示 `System proxy crash recovery check completed without managed state`；`~/.bifrost/logs/bifrost.2026-06-09.log` 显示关机路径出现 `No enabled macOS network services were returned by networksetup`，并且原 state 被 helper 清理后导致开机 cleanup 无恢复依据。修复后执行记录待补充。
- 2026-06-09：针对更新后 restart 清理系统代理但 fresh daemon 未恢复的问题，新增 TC-CSP-27，并执行系统代理 E2E：`BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`，结果 14/14 PASS。当前机器正式 system proxy owner 为 `127.0.0.1:9900`，因此 TC-CSP-27 对应脚本用例 `test_restart_preserves_system_proxy` 按外部 owner 保护规则跳过非隔离真实写入，避免误抢正式代理；本轮同时执行 `cargo test -p bifrost-cli runtime_system_proxy_host_maps_wildcard_listeners_to_loopback`、`cargo test -p bifrost-cli append_system_proxy_start_args_preserves_bypass`、`cargo test -p bifrost-cli test_build_restart_args_preserves_system_proxy_snapshot`，覆盖 restart / upgrade restart stop 前快照与 fresh start argv 追加 `--system-proxy --proxy-bypass` 的代码路径。测试后执行 `BIFROST_DATA_DIR="$HOME/.bifrost" ./target/debug/bifrost system-proxy status`，确认系统代理仍为 `127.0.0.1:9900`，未残留临时 18889。
- 2026-06-08：针对 Settings -> Proxy 在外部代理占用时误把 `configured_enabled=true` 显示为开关已打开的回归补充验证。当前 18890 测试服务的 `/api/proxy/system` 返回 live 代理指向 `127.0.0.1:9900` 且 `managed_by_bifrost=false`，Settings -> Proxy 展示 `System proxy is occupied by another proxy`，`Enable System Proxy` 开关显示关闭并保留可点击接管入口；最小 Playwright 脚本读取 `settings-system-proxy-switch` 的 `aria-checked=false`。执行 `pnpm -C web exec eslint src/pages/Settings/tabs/SystemProxySection.tsx src/pages/Traffic/index.tsx src/components/StatusBar/index.tsx tests/ui/admin-settings.spec.ts` 通过；执行 `pnpm -C web test:unit -- --run useProxyStore` 通过，确认 stored preference 与 live Bifrost ownership 分离。
- 2026-06-08：针对强制宕机后 one-shot cleanup 观察到的日志优化点和 configured/live 状态混用回归补充验证。执行 `cargo test -p bifrost-core system_proxy_launchd -- --nocapture`，结果 15/15 PASS，验证 missing pid 不再 shell out、stale runtime 文件可清理；执行 `cargo test -p bifrost-core system_proxy -- --nocapture`，结果 37/37 PASS，覆盖 system proxy ownership、LaunchDaemon 与 retry 相关单测；执行 `cargo test -p bifrost-admin proxy::tests -- --nocapture`，结果 4/4 PASS；执行 `pnpm --dir web test:unit -- --run useProxyStore`，结果 18 个 test files / 62 tests PASS，覆盖 configured preference 与 live switch 状态拆分；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_system_proxy_e2e.sh`，第一轮发现 no-state cleanup 功能正确但耗时 12 秒，修复为并发读取 macOS network service 代理状态后第二轮 14/14 PASS，验证 cleanup-daemon 快速退出、无 `No such process` stderr，并删除 stale `runtime.json` / `bifrost.pid`；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 bash e2e-tests/tests/test_proxy_admin_api.sh`，结果 6/6 PASS，验证 `/api/proxy/system` 返回 `enabled` 和 `configured_enabled`；执行 TC-CSP-25 focused 自动化验证，临时 data dir 中 `[system_proxy].enabled = true`，Admin API 返回 `configured_enabled=true`，cleanup-daemon 后 config 仍为 `true` 且 stale `runtime.json` / `bifrost.pid` 已删除。
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
