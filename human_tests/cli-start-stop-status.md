# CLI 服务管理（start / stop / status）测试用例

## 功能模块说明

本文档覆盖 Bifrost CLI 的核心服务管理功能，包括：
- `bifrost start`：启动代理服务（含各种参数组合）
- `bifrost stop`：停止代理服务
- `bifrost status`：查看服务状态（含 TUI 模式）
- `-v`：版本信息查看

## 前置条件

1. 确保项目已编译或可编译：
   ```bash
   cd /path/to/bifrost
   ```
2. 确保端口 8800、8801、8802 未被占用
3. 确保无正在运行的 Bifrost 测试实例（可先执行 `cargo run --bin bifrost -- stop`）
4. 准备一个规则文件用于 `--rules-file` 测试：
   ```bash
   echo "httpbin.org reqHeaders://(X-Bifrost-Test: 1)" > /tmp/bifrost-test-rules.txt
   ```
5. 所有启动命令统一使用临时数据目录，避免污染正式环境：
   ```bash
   export BIFROST_DATA_DIR=./.bifrost-test
   ```

---

## 测试用例

### TC-CSS-01：默认参数启动服务（前台模式）

**操作步骤**：
1. 执行以下命令启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 观察终端输出

**预期结果**：
- 终端输出包含启动成功信息，显示监听地址 `0.0.0.0:8800`
- 服务在前台运行，终端被占用
- 执行 `curl -x http://127.0.0.1:8800 http://httpbin.org/get` 返回正常 JSON 响应
- 按 Ctrl+C 可正常停止服务

---

### TC-CSS-02：指定自定义端口启动（-p 8801）

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl
   ```
2. 使用 curl 验证代理功能

**预期结果**：
- 终端输出显示监听地址为 `0.0.0.0:8801`
- 执行 `curl -x http://127.0.0.1:8801 http://httpbin.org/get` 返回正常 JSON 响应
- 端口 8800 未被监听

---

### TC-CSS-03：后台守护进程模式启动（-d / --daemon）

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
2. 观察终端输出
3. 检查进程是否在后台运行

**预期结果**：
- 命令执行后终端立即返回（不阻塞）
- 输出包含类似 "Proxy started in daemon mode" 或显示 PID 的信息
- 执行 `curl -x http://127.0.0.1:8800 http://httpbin.org/get` 返回正常响应
- `ps aux | grep bifrost` 可以看到后台进程

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSS-04：启用 --unsafe-ssl 跳过上游 TLS 验证

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 通过代理请求一个 HTTPS 网站：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动
- HTTPS 请求通过代理成功完成，返回正常 JSON 响应
- 不会因为上游 TLS 证书问题而报错

---

### TC-CSS-05：使用 --no-intercept 禁用 TLS 拦截

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-intercept
   ```
2. 通过代理请求 HTTPS 网站：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动，日志中显示 TLS 拦截已禁用
- HTTPS 请求以 CONNECT 隧道方式通过，代理不解密内容
- 请求正常返回

---

### TC-CSS-06：使用 --intercept 启用 TLS 拦截

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept
   ```
2. 通过代理请求 HTTPS 网站（需要信任 CA 或使用 -k）：
   ```bash
   curl -x http://127.0.0.1:8800 https://httpbin.org/get -k
   ```

**预期结果**：
- 服务正常启动，日志中显示 TLS 拦截已启用
- HTTPS 请求通过代理时被拦截解密，代理可以看到请求内容
- 请求正常返回 JSON 响应
- `--intercept` 和 `--no-intercept` 不可同时使用（CLI 会报错并拒绝启动）

---

### TC-CSS-07：使用 --rules 指定内联规则启动

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --rules "httpbin.org reqHeaders://(X-Bifrost: hello)"
   ```
2. 通过代理请求验证规则生效：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```

**预期结果**：
- 服务正常启动
- curl 响应的 headers 字段中包含 `"X-Bifrost": "hello"`，说明规则已生效
- 请求 httpbin.org 以外的域名不受该规则影响

---

### TC-CSS-08：使用 --rules-file 指定规则文件启动

**前置条件**：已创建 `/tmp/bifrost-test-rules.txt`，内容为 `httpbin.org reqHeaders://(X-Bifrost-Test: 1)`

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --rules-file /tmp/bifrost-test-rules.txt
   ```
2. 通过代理请求验证规则生效：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```

**预期结果**：
- 服务正常启动
- curl 响应的 headers 字段中包含 `"X-Bifrost-Test": "1"`，说明规则文件已被加载并生效

---

### TC-CSS-09：使用 --socks5-port 指定独立 SOCKS5 端口

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --socks5-port 8802
   ```
2. 通过 HTTP 代理验证：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 通过 SOCKS5 代理验证：
   ```bash
   curl -x socks5://127.0.0.1:8802 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动，日志中显示 HTTP 代理监听 8800 端口，SOCKS5 代理监听 8802 端口
- 步骤 2 通过 HTTP 代理正常返回响应
- 步骤 3 通过 SOCKS5 代理正常返回响应

---

### TC-CSS-10：使用 --allow-lan 允许局域网访问

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --allow-lan
   ```
2. 从本机使用局域网 IP 访问代理（假设局域网 IP 为 `192.168.x.x`）：
   ```bash
   curl -x http://192.168.x.x:8800 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动
- 从局域网 IP 访问代理时请求成功（未被拒绝）
- 不使用 `--allow-lan` 时，局域网 IP 访问会被访问控制拒绝

---

### TC-CSS-11：使用 --proxy-user 设置代理认证

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --proxy-user "testuser:testpass"
   ```
2. 不带认证访问代理：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/get
   ```
3. 带正确认证访问代理：
   ```bash
   curl -x http://testuser:testpass@127.0.0.1:8800 http://httpbin.org/get
   ```
4. 带错误认证访问代理：
   ```bash
   curl -x http://testuser:wrongpass@127.0.0.1:8800 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动
- 步骤 2：返回 HTTP 407 Proxy Authentication Required
- 步骤 3：正常返回 httpbin.org 的 JSON 响应
- 步骤 4：返回 HTTP 407 Proxy Authentication Required

---

### TC-CSS-12：查看服务状态（status 命令）

**前置条件**：服务已通过 TC-CSS-03 以 daemon 模式启动

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```

**预期结果**：
- 输出包含代理服务的运行状态信息
- 输出顶部包含 `Service Overview`
- 输出顶部包含 `Proxy Local Address: http://127.0.0.1:8800`
- 输出顶部包含 `Proxy LAN Addresses:`；如果服务监听 `0.0.0.0` 且机器存在局域网地址，应列出 `http://<局域网IP>:8800`，如果只监听 localhost 则明确显示局域网不可用
- 输出顶部包含 `System Proxy:`，并明确当前系统代理是否指向该 Bifrost 服务
- 输出顶部包含 `TLS Interception:`，展示 TLS 全局开关、上游证书校验和配置变更断连状态
- 输出顶部包含 `TLS Domain Whitelist:`，展示 TLS 域名 include 白名单状态；如果存在多个条目，必须完整列出，不用 `... +N more` 省略
- 输出顶部包含 `TLS App Whitelist:`，展示 TLS 应用 include 白名单状态；如果存在多个条目，必须完整列出，不用 `... +N more` 省略
- 输出顶部包含 `TLS IP Whitelist:`，展示 TLS IP include 白名单状态；如果存在多个条目，必须完整列出，不用 `... +N more` 省略
- 输出顶部包含 `TLS Domain Passthrough:`、`TLS App Passthrough:`、`TLS IP Passthrough:`，分别展示域名、应用和 IP 的 exclude 边界；如果存在多个条目，必须完整列出，不用 `... +N more` 省略
- 显示监听端口（如 `8800`）
- 显示进程 PID
- 显示运行时长或启动时间
- 显示 TLS 拦截状态、规则数量等关键配置信息

---

### TC-CSS-13：服务未运行时查看状态

**前置条件**：确保 Bifrost 服务未在运行

**操作步骤**：
1. 先停止服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   ```
2. 查看状态：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```

**预期结果**：
- 输出提示服务未在运行（如 "Proxy is not running" 或类似信息）

---

### TC-CSS-14：TUI 仪表盘模式查看状态（status --tui）

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status --tui
   ```
2. 观察终端输出

**预期结果**：
- 显示交互式 TUI 仪表盘界面
- 界面包含实时的代理状态信息（连接数、流量统计等）
- 按 `q` 或 Ctrl+C 可退出 TUI 界面

---

### TC-CSS-15：停止服务（stop 命令）

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 确认服务正在运行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```
2. 执行停止命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   ```
3. 再次检查状态：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```

**预期结果**：
- 步骤 1：显示服务正在运行
- 步骤 2：输出停止成功的消息（如 "Proxy stopped"）
- 步骤 3：显示服务未在运行
- 代理端口 8800 不再监听

---

### TC-CSS-16：服务未运行时执行 stop

**前置条件**：确保 Bifrost 服务未在运行

**操作步骤**：
1. 执行停止命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   ```

**预期结果**：
- 输出提示服务未在运行（如 "Proxy is not running" 或类似信息）
- 不会报错或崩溃

---

### TC-CSS-17：服务已运行时再次启动（交互式重启提示）

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 先以 daemon 模式启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
2. 再次执行启动命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
3. 当出现重启提示时，输入 `n` 拒绝

**预期结果**：
- 步骤 2 检测到已有 Bifrost 进程在运行
- 终端输出类似 "Detected an existing Bifrost proxy process (PID: xxx). Restart? (y/n)" 的提示
- 输入 `n` 后，保持原有进程继续运行，不做任何变更

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSS-18：使用 -y 自动确认重启

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 先以 daemon 模式启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
2. 使用 -y 参数再次启动：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl -y
   ```

**预期结果**：
- 不出现交互式提示，自动停止旧进程并启动新进程
- 输出表明旧进程已被停止、新进程已启动
- 执行 `curl -x http://127.0.0.1:8800 http://httpbin.org/get` 新服务正常工作

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSS-19：查看版本信息（-v）

**操作步骤**：
1. 执行以下命令：
   ```bash
   cargo run --bin bifrost -- -v
   ```

**预期结果**：
- 输出 Bifrost 版本号（格式如 `bifrost x.y.z`）
- 命令执行后立即退出，不启动服务

---

### TC-CSS-20：同时指定多个 --rules 参数

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --rules "httpbin.org reqHeaders://(X-First: 1)" \
     --rules "httpbin.org reqHeaders://(X-Second: 2)"
   ```
2. 通过代理验证两条规则均生效：
   ```bash
   curl -x http://127.0.0.1:8800 http://httpbin.org/headers
   ```

**预期结果**：
- 服务正常启动
- curl 响应的 headers 中同时包含 `"X-First": "1"` 和 `"X-Second": "2"`
- 多次指定 --rules 参数可叠加生效

---

### TC-CSS-21：同时指定多个 --proxy-user 参数

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     --proxy-user "user1:pass1" \
     --proxy-user "user2:pass2"
   ```
2. 分别用两个用户认证访问：
   ```bash
   curl -x http://user1:pass1@127.0.0.1:8800 http://httpbin.org/get
   curl -x http://user2:pass2@127.0.0.1:8800 http://httpbin.org/get
   ```

**预期结果**：
- 服务正常启动
- 两个用户均可通过认证，正常返回 httpbin.org 的 JSON 响应
- 使用未注册的用户名密码仍返回 407

---

### TC-CSS-22：--intercept 和 --no-intercept 互斥检查

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --no-intercept
   ```

**预期结果**：
- CLI 报错，提示 `--intercept` 和 `--no-intercept` 不能同时使用（clap 的 conflicts_with 机制）
- 服务不会启动
- 进程以非零退出码退出

---

### TC-CSS-23：version-check 子命令检查新版本

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 以 daemon 模式启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
2. 执行版本检查命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 version-check
   ```

**预期结果**：
- 输出当前版本和最新可用版本的对比信息
- 如果是最新版本，提示已是最新
- 如果有新版本，显示新版本号及升级提示
- 命令执行后立即退出

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-CSS-24：status 命令别名 st

**前置条件**：服务已以 daemon 模式运行

**操作步骤**：
1. 执行以下命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- st
   ```

**预期结果**：
- 输出与 `status` 命令完全一致的服务状态信息
- `st` 作为 `status` 的别名正常工作

---

### TC-CSS-25：status 在运行时追加展示活跃规则合并摘要

**前置条件**：服务未运行

**操作步骤**：
1. 清理旧测试数据：
   ```bash
   rm -rf ./.bifrost-test
   ```
2. 创建并启用两条本地规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- rule add status-active-1 -c "status-active-1.example.com statusCode://200"
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- rule add status-active-2 -c "status-active-2.example.com reqHeaders://(X-Status-Test: 1)"
   ```
3. 启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 -d --unsafe-ssl
   ```
4. 执行状态命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```
5. 停止服务后再次执行状态命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status
   ```

**预期结果**：
- 步骤 4 输出包含 `Default Port Active Rules: 8800`
- 步骤 4 输出包含 `Scope: default/main proxy port 8800`，明确这是默认/主代理端口规则，不是临时端口绑定规则
- 步骤 4 输出包含 `Default Port Merged Rules (in parsing order): 8800`
- 步骤 4 输出包含 `status-active-1.example.com statusCode://200`
- 步骤 4 输出包含 `status-active-2.example.com reqHeaders://(X-Status-Test: 1)`
- 步骤 5 的停止态 `status` 输出不包含 `Default Port Active Rules`

---

### TC-CSS-26：前台启动时 listener 任务失败必须退出主进程（回归）

**前置条件**：服务未运行，端口 `18930` 未被 TCP 占用。

**操作步骤**：
1. 使用临时数据目录并先占用同端口 TCP：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   python3 - <<'PY' &
import socket, time
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("0.0.0.0", 18930))
sock.listen(1)
while True:
    time.sleep(1)
PY
   TCP_PID=$!
   ```
2. 启动前台服务：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18930 start --skip-cert-check --unsafe-ssl --no-system-proxy >"$TEST_DATA_DIR/foreground.log" 2>&1 &
   BIFROST_PID=$!
   ```
3. 等待最多 8 秒，检查 `BIFROST_PID` 是否已退出。
4. 验证 Admin API 不可达：
   ```bash
   curl -fsS http://127.0.0.1:18930/_bifrost/api/proxy/address
   ```
5. 清理 TCP 占用和临时目录。

**预期结果**：
- 前台 Bifrost 进程在 listener task 失败后自动退出，不会继续假运行。
- `foreground.log` 包含同端口 TCP listener bind 失败相关错误，例如 `another process is already listening on this port`，或 Linux CI 非交互 auto-resolve 路径中的 `already in use`。
- Admin API 请求失败，端口不会呈现半启动状态。

**执行记录**：
- 2026-05-04 执行 `bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh` 覆盖本用例。脚本使用临时 `BIFROST_DATA_DIR`、端口 `18930`、`--no-system-proxy`，先占用同端口 UDP 后启动前台服务；断言前台进程退出、Admin API 不可达、日志包含 `Address already in use`，全部通过。
- 2026-05-07 因统一代理增加 UDP relay ephemeral fallback，改为占用同端口 TCP 来覆盖主 listener bind 失败。执行 `PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`，脚本使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、非 9900 端口；断言前台进程退出、Admin API 不可达、日志包含 `another process is already listening on this port`，通过。
- 2026-05-07 CI run `25471477201` 的 Linux shard 3 显示本用例在前台退出断言后卡住：Admin API 不可达探针请求命中仍占用端口的 TCP holder，但 curl 未设置超时。修复后 `admin_unreachable` 使用 `--connect-timeout 1 --max-time 2`，确保 TCP holder 场景下快速返回不可达而不是等到 suite timeout。随后执行 `SKIP_BUILD=true PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`，脚本汇总 `Total: 8 / Passed: 8 / Failed: 0`。
- 2026-05-07 CI run `25474240006` 的 Linux shard 3 显示同一前台 listener 失败会输出非交互端口冲突提示 `Port 0.0.0.0:<port> is already in use`，而 macOS 本地仍输出旧的 `another process is already listening on this port`。本次将断言调整为同时接受这两种平台相关文案，继续验证前台进程退出、Admin API 不可达、日志确认为端口占用。随后执行 `SKIP_BUILD=true PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`，脚本汇总 `Total: 8 / Passed: 8 / Failed: 0`。

---

### TC-CSS-27：daemon 启动必须等待 listener 真正 ready（回归）

**前置条件**：服务未运行，端口 `18930` 未被 TCP 占用。

**操作步骤**：
1. 使用临时数据目录并先占用同端口 TCP：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   python3 - <<'PY' &
import socket, time
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("0.0.0.0", 18930))
sock.listen(1)
while True:
    time.sleep(1)
PY
   TCP_PID=$!
   ```
2. 启动 daemon：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18930 start --daemon --skip-cert-check --unsafe-ssl --no-system-proxy
   ```
3. 记录命令退出码和输出。
4. 验证 Admin API 不可达：
   ```bash
   curl -fsS http://127.0.0.1:18930/_bifrost/api/proxy/address
   ```
5. 清理 TCP 占用和临时目录。

**预期结果**：
- daemon 启动命令返回非零退出码。
- 输出包含 listener readiness 失败提示，例如 `before the proxy listener became ready`，或 Linux CI 非交互 auto-resolve 路径中的 `already in use`。
- 输出不包含 `Daemon started with PID`。
- Admin API 请求失败，不会出现父进程提前报告 daemon 已启动但端口未监听的状态。

**执行记录**：
- 2026-05-04 执行 `bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh` 覆盖本用例。脚本使用临时 `BIFROST_DATA_DIR`、端口 `18930`、`--no-system-proxy`，占用同端口 UDP 后执行 daemon 启动；断言命令非零退出、输出包含 readiness 失败、输出不包含 `Daemon started with PID`、Admin API 不可达，全部通过。脚本汇总 `8/8` 断言通过。
- 2026-05-07 因统一代理增加 UDP relay ephemeral fallback，改为占用同端口 TCP 来覆盖主 listener readiness 失败。执行 `PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`，脚本使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、非 9900 端口；断言 daemon 命令非零退出、输出包含 readiness 失败、输出不包含 `Daemon started with PID`、Admin API 不可达，脚本汇总通过。
- 2026-05-07 同步覆盖 TCP holder 下的 Admin API 不可达探针超时保护；daemon 分支复用 `admin_unreachable`，因此同样要求探针在 2 秒内返回失败而不是挂起。`SKIP_BUILD=true PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh` 已验证 daemon 分支通过。
- 2026-05-07 CI run `25475008050` 的 Linux shard 3 显示 daemon 分支也会在非交互 auto-resolve 路径中直接输出 `Port 0.0.0.0:<port> is already in use`，而不是 readiness 等待文案。本次将 daemon 错误断言调整为同时接受 readiness 失败与端口占用提示，仍保留非零退出、无 `Daemon started with PID`、Admin API 不可达三个核心断言。随后执行 `SKIP_BUILD=true PROXY_PORT=18130 bash e2e-tests/tests/test_startup_listener_readiness_e2e.sh`，脚本汇总 `Total: 8 / Passed: 8 / Failed: 0`。

---

### TC-CSS-28：status 顶部展示代理能力与 TLS 边界

**前置条件**：服务未运行，端口 `18991` 未被占用。

**操作步骤**：
1. 使用临时数据目录启动一个不修改系统代理的服务：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18991 start --skip-cert-check --unsafe-ssl --no-system-proxy >"$TEST_DATA_DIR/status.log" 2>&1 &
   BIFROST_PID=$!
   ```
2. 等待 Admin API ready：
   ```bash
   curl -fsS http://127.0.0.1:18991/_bifrost/api/system
   ```
3. 执行 status：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18991 status
   ```
4. 停止服务并删除临时目录：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost stop
   rm -rf "$TEST_DATA_DIR"
   ```

**预期结果**：
- 输出顶部 `Service Overview` 位于 `Runtime` 区块之前。
- `Proxy Local Address` 显示 `http://127.0.0.1:18991`，明确本机代理地址与端口。
- `Proxy LAN Addresses` 单独展示局域网地址列表；如果当前机器未检测到局域网地址，显示 `none detected`，如果监听 localhost only 则显示不可用原因。
- `System Proxy` 显示当前 OS 系统代理状态；本用例使用 `--no-system-proxy`，不得因为执行 status 修改系统代理。
- `TLS Interception` 显示 TLS 拦截开关、上游证书校验和配置变更断连状态。
- `TLS Domain Whitelist` 显示域名 include 白名单状态，多个条目完整展示，不用 `... +N more` 省略。
- `TLS App Whitelist` 显示应用 include 白名单状态，多个条目完整展示，不用 `... +N more` 省略。
- `TLS IP Whitelist` 显示 IP include 白名单状态，多个条目完整展示，不用 `... +N more` 省略。
- `TLS Domain Passthrough`、`TLS App Passthrough`、`TLS IP Passthrough` 分别显示域名、应用和 IP 的 exclude 边界状态，多个条目完整展示，不用 `... +N more` 省略。
- 输出底部包含 `Temporary Port Bindings`；本用例未创建临时端口绑定时显示 `No temporary port bindings.`。

**执行记录**：
- 2026-05-18 执行聚焦真实 CLI 验证：使用 `mktemp -d` 临时 `BIFROST_DATA_DIR`，`target/debug/bifrost -p 18991 start --skip-cert-check --unsafe-ssl --no-system-proxy` 启动服务，创建默认规则 `status-active-1` 和临时端口绑定 `18992` 后执行 `status`。实际结果命中 14 个断言：`Service Overview`、`Proxy Local Address: http://127.0.0.1:18991`、`Proxy LAN Addresses:`、`System Proxy:`、`TLS Interception:`、`TLS Domain Whitelist:`、`TLS App Whitelist:`、`Default Port Rule Groups: 18991`、`Default Port Active Rules: 18991`、默认端口 scope 说明、`Default Port Merged Rules (in parsing order): 18991`、`Temporary Port Bindings`、`:18992 [running] (status temp port)`、`local:status-active-1`。测试结束后执行 `bifrost stop` 并删除临时目录。
- 2026-05-18 执行脚本级 E2E：`BIFROST_BIN=~/work/github/bifrost/target/debug/bifrost SKIP_BUILD=true PROXY_PORT=18991 TEMP_PORT=18992 e2e-tests/tests/test_cli_online_commands_e2e.sh`。脚本使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`，status 阶段验证顶部代理地址、系统代理、TLS 域名/应用白名单、默认端口规则摘要、默认端口 scope 说明、底部临时端口绑定规则，最终汇总 `通过: 87 / 失败: 0 / 总计: 87`。
- 2026-05-18 针对 TLS 列表完整展示追加验证：使用 `mktemp -d` 临时 `BIFROST_DATA_DIR`，`target/debug/bifrost -p 18993 start --skip-cert-check --unsafe-ssl --no-system-proxy` 启动服务后执行 `status`。实际输出 `TLS App Whitelist: 8 [Google Chrome*, Microsoft Edge*, *Safari*, *Firefox*, *Opera*, *Brave*, *Arc*, *Vivaldi*]`，没有出现 `... +N more`；`TLS Domain Whitelist`、`TLS IP Whitelist`、`TLS Domain Passthrough`、`TLS App Passthrough`、`TLS IP Passthrough` 均完整输出各自状态。测试结束后执行 `bifrost stop` 并删除临时目录。

---

### TC-CSS-29：daemon 启动在非交互缺失 CA 时必须阻断（回归）

**前置条件**：服务未运行，端口 `18892` 未被占用。

**操作步骤**：
1. 创建临时数据目录，确保该目录下没有已安装到系统信任的 CA：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   ```
2. 在无交互 stdin 的场景启动 daemon，模拟脚本/异步启动：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/release/bifrost -p 18892 start --daemon --unsafe-ssl --no-system-proxy </dev/null
   ```
3. 记录命令退出码和输出。
4. 验证 daemon 没有启动：
   ```bash
   curl --connect-timeout 1 --max-time 2 -fsS http://127.0.0.1:18892/_bifrost/api/proxy/address
   ```
5. 使用新的临时数据目录显式跳过证书检查，确认 escape hatch 仍可用：
   ```bash
   TEST_DATA_DIR_SKIP="$(mktemp -d)"
   BIFROST_DATA_DIR="$TEST_DATA_DIR_SKIP" target/release/bifrost -p 18892 start --daemon --skip-cert-check --unsafe-ssl --no-system-proxy
   curl -fsS http://127.0.0.1:18892/_bifrost/api/proxy/address
   BIFROST_DATA_DIR="$TEST_DATA_DIR_SKIP" target/release/bifrost -p 18892 stop
   ```
6. 删除临时目录。

**预期结果**：
- 步骤 2 返回非零退出码。
- 输出明确说明 CA 未安装或未信任，且无交互式终端可用。
- 输出提示可使用 `--yes` 自动安装/信任、先执行 `bifrost ca install`，或显式传入 `--skip-cert-check`。
- 步骤 2 输出不包含 `Daemon started with PID`，Admin API 不可达。
- 步骤 5 在显式 `--skip-cert-check` 下仍可启动 daemon，Admin API 可访问，说明跳过参数没有被破坏。
- 全流程不修改系统代理，因为所有启动命令都包含 `--no-system-proxy`。

**执行记录**：
- 2026-05-20 执行 `bash e2e-tests/tests/test_daemon_cert_check_e2e.sh` 覆盖本用例。脚本使用临时 `BIFROST_DATA_DIR` 和端口 `18892`；第一段通过 `</dev/null` 模拟非交互 daemon 启动，断言命令非零退出、输出包含 `no interactive terminal is available` 和 `--yes`、输出不包含 `Daemon started with PID`、本地 CA 文件已生成但 Admin API 不可达；第二段执行 `--skip-cert-check --daemon --no-system-proxy`，断言 daemon 启动成功且 Admin API ready，随后停止服务并删除临时目录。

---

### TC-CSS-30：daemon stop 不因 zombie 状态误升级到 SIGKILL（回归）

**前置条件**：服务未运行，测试使用临时数据目录和动态端口。

**操作步骤**：
1. 执行 daemon shutdown focused test：
   ```bash
   cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture
   ```
2. 观察 stop 命令输出和测试断言。

**预期结果**：
- 测试启动临时 `BIFROST_DATA_DIR` 下的 daemon，并使用 `--skip-cert-check --no-intercept` 避免证书交互和 TLS 拦截副作用。
- `bifrost stop` 返回成功，输出不包含 `Sending SIGKILL`。
- daemon 进程退出后，即使 Unix 系统短暂保留 zombie 状态，CLI 进程状态检测也不应把它误判为仍在运行。
- 测试结束后临时目录自动清理，不修改系统代理。

**执行记录**：
- 2026-06-04 执行 `cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture` 覆盖本用例。修复前本地全量测试失败，`bifrost stop` 输出 `Sending SIGKILL`；修复后 `is_process_running` 在非 Linux Unix 通过 `ps -o stat=` 排除 zombie 状态，focused test 通过。

---

### TC-CSS-31：stop 前置清理系统代理，restart 保留系统代理（回归）

**前置条件**：服务未运行，测试使用临时数据目录和动态端口。

**操作步骤**：
1. 执行 focused 单测验证 shutdown marker、foreground cleanup marker 与 restart stop 模式：
   ```bash
   cargo test -p bifrost-core system_proxy_shutdown_mode_marker_is_read_and_consumed -- --nocapture
   cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture
   ```
2. 使用临时数据目录启动不修改系统代理的 daemon：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 cargo run --bin bifrost -- start -p "$PORT" -H 127.0.0.1 --daemon --skip-cert-check --no-system-proxy --no-intercept
   ```
3. 执行 stop，并记录命令耗时：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- stop
   ```
4. 再次执行 status，并检查代理端口不再监听。
5. 使用新的临时数据目录启动不修改系统代理的 daemon，执行真实 `bifrost restart`，验证旧 PID 被替换、新 daemon ready，且 restart log 不包含 `--system-proxy`。

**预期结果**：
- `stop` 写入 `foreground_cleanup` marker，先在前台清理系统代理/CLI proxy，再发送 SIGTERM；无系统代理场景也不应输出 `Sending SIGKILL`。
- `stop` 输出 `Cleaning system proxy before stopping Bifrost proxy...`，且不输出 `System proxy cleanup continues in background if needed.`。
- restart 专用 stop 使用 preserve marker，旧 daemon 和旧 lifecycle helper 不清理系统代理，fresh daemon 继续接管同一 host/port。
- 无系统代理 restart 是跨平台一致能力：Linux/macOS CI 均应完成 daemon handoff，fresh start argv 不应包含 `--system-proxy`，并且不残留 `.system_proxy_shutdown_mode`。
- `stop` 返回后 status 显示服务未运行，端口不再监听，临时数据目录可删除。
- 测试命令统一带 `--no-system-proxy`，不修改本机系统代理；涉及真实系统代理的耗时回归由 `cli-system-proxy.md` 的系统代理用例覆盖。

**执行记录**：
- 2026-06-10 执行 focused 单测、E2E 与真实 CLI 验证通过。`source ~/.zshrc && cargo test -p bifrost-core system_proxy_shutdown_mode_marker_is_read_and_consumed -- --nocapture` 通过，确认 `preserve_for_restart`、`background_cleanup`、`foreground_cleanup` marker 均可 read 且 consume 只消费一次；`source ~/.zshrc && cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture` 通过，确认 daemon stop 未升级到 SIGKILL；`source ~/.zshrc && cargo build --bin bifrost && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh` 结果 10/10 PASS，覆盖 startup 清理 stale restart marker、stop 先输出 `Cleaning system proxy before stopping Bifrost proxy...` 再输出 `Stopping Bifrost proxy`、不输出后台 cleanup 提示、stop 实测 `508ms`、status stopped；同一脚本通过 fake `scutil` / `networksetup` 隔离执行真实 `bifrost restart`，确认 restart argv 带 `--system-proxy`，fake 系统代理在旧 daemon 停止、端口释放、fresh daemon ready 期间持续指向同一 `127.0.0.1:$PORT`，未出现 disable gap。真实 CLI 使用临时数据目录和动态端口；普通 stop 用例启动参数带 `--no-system-proxy`，不修改本机系统代理；restart handoff 用例使用 fake 系统命令，不触碰本机真实系统代理。
- 2026-06-10 跨平台补强：针对 review 指出的 Linux 不支持完整系统代理配置但其他 restart 能力应对齐，`test_stop_restart_shutdown_marker.sh` 新增无系统代理 restart 子用例。执行真实 `bifrost restart` 后确认 fresh daemon ready、runtime PID 已变化、restart argv 不含 `--system-proxy`、`.system_proxy_shutdown_mode` 不残留；该子用例不依赖 macOS fake `networksetup`，会在 Linux/macOS shell CI 中执行。
- 2026-06-10 本轮复测：`source ~/.zshrc && cargo fmt --all -- --check && cargo test -p bifrost-core test_is_supported -- --nocapture && cargo test -p bifrost-cli restart_handoff_recovery -- --nocapture && cargo build --bin bifrost && SKIP_BUILD=true e2e-tests/tests/test_stop_restart_shutdown_marker.sh` 通过。脚本结果 14/14 PASS，新增无系统代理 restart 子用例通过，macOS fake system proxy handoff 仍保持无 cleanup gap。
- 2026-06-10 CI shell 覆盖确认：执行 `source ~/.zshrc && for shard in 1 2 3; do BIFROST_E2E_SHARD_INDEX="$shard" BIFROST_E2E_SHARD_TOTAL=3 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg 'test_stop_restart_shutdown_marker|test_system_proxy_e2e' || true; done`，确认 `test_stop_restart_shutdown_marker.sh` 被 Linux/macOS shell CI 的 shard 1/3 收集，且未被 CI skip 列表过滤；`test_system_proxy_e2e.sh` 仍按既有策略跳过 Linux，避免在 Linux 写半成品系统代理。

---

### TC-CSS-32：Remote Invoke 历史按需分页且不阻塞代理监听（回归）

**前置条件**：服务未运行，测试使用临时数据目录和动态端口。

**操作步骤**：
1. 构造临时 `BIFROST_DATA_DIR`，并在其中写入旧格式 `admin/remote_invoke_call_history.json`。该文件用于验证旧历史不兼容读取，启动后应被直接删除。
2. 在临时目录中写入新版 `admin/remote_invoke_call_history/<client-key>.jsonl`，包含超过 1000 条 Remote Invoke 调用快照和一行坏 JSON，模拟新版历史较大的用户现场。
3. 使用最新源码启动真实 Bifrost 前台服务，必须带 `--no-system-proxy` 和禁用 Sync 自动登录弹窗：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/debug/bifrost start -p "$PORT" --skip-cert-check --unsafe-ssl --no-system-proxy --no-intercept
   ```
4. 轮询 `http://127.0.0.1:$PORT/_bifrost/api/proxy/address`，记录 Admin API 首次 ready 时间。
5. 等待日志出现 `remote invoke worker initialized asynchronously`。
6. 检查日志顺序：
   - `remote invoke worker initialization scheduled`
   - `Unified proxy server listening`
   - `foreground runtime initialization completed`
   - `remote invoke worker initialized asynchronously`
7. 请求 `http://127.0.0.1:$PORT/_bifrost/api/remote-invoke/calls?limit=25`，确认响应最多 25 条，并返回 `next_cursor`。
8. 再请求 `http://127.0.0.1:$PORT/_bifrost/api/remote-invoke/calls?limit=25&before=<next_cursor>`，确认返回下一页且不重复第一页最后一条。
9. 检查旧 `admin/remote_invoke_call_history.json` 已删除，新版 JSONL compaction 后只保留最新 1000 条有效记录，坏 JSON 行已清理。
10. 停止服务并删除临时目录。

**预期结果**：
- Admin API ready 不需要等待 Remote Invoke worker 初始化完成。
- `foreground runtime initialization completed` 先于 `remote invoke worker initialized asynchronously` 出现。
- Remote Invoke worker 最终仍会初始化并启动。
- Remote Invoke worker 内存不保留历史列表；Recent Calls 只在 API 请求时按需读取 JSONL。
- 旧整文件历史直接删除，不迁移、不兼容读取；新版 JSONL 最终落盘最多 1000 条有效记录。
- calls API 支持 `limit` / `before` 分页，页面打开不全量加载历史。
- 启动命令全程使用临时数据目录和 `--no-system-proxy`，不修改用户正式数据和系统代理。

**执行记录**：
- 2026-06-12 执行真实启动回归通过。使用 `target/debug/bifrost`、临时 `BIFROST_DATA_DIR=/var/folders/.../tmp.d57AtaTou9`、动态端口 `54572`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`--no-system-proxy --no-intercept` 启动；Admin API ready 成功。日志顺序为 `remote invoke worker initialization scheduled` -> `Unified proxy server listening` -> `foreground runtime initialization completed total_elapsed_ms=41` -> `removed legacy remote invoke call history store` -> `remote invoke worker initialized asynchronously elapsed_ms=6`。旧 `admin/remote_invoke_call_history.json` 被删除，全流程使用临时数据目录且未修改系统代理。
- 2026-06-12 追加 1000 条历史压力启动验证通过。使用临时 `BIFROST_DATA_DIR=/tmp/bifrost-ri-1000-startup.a2WorR` 预置新版 `admin/remote_invoke_call_history/perf-client.jsonl` 共 1000 行、约 `696780` bytes，并额外写入旧版 `admin/remote_invoke_call_history.json`。执行 `BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/release/bifrost start -p 60804 --skip-cert-check --unsafe-ssl --no-system-proxy --no-intercept`，端口监听实测 `312ms` 内打开；启动采样峰值 RSS `40816KB`，CPU 峰值整数部分 `6%`；启动后旧版 JSON 文件已删除，新版 JSONL 仍为 1000 行。该验证确认 1000 条落盘历史不会被启动路径全量加载，也不会阻塞监听端口。

---

### TC-CSS-33：macOS daemon exec child 避免 fork 后 ObjC 崩溃（回归）

**前置条件**：macOS，本机未运行测试端口上的 Bifrost；测试使用临时 `BIFROST_DATA_DIR` 和动态端口，不修改正式数据目录和系统代理。

**操作步骤**：
1. 构建当前源码二进制：
   ```bash
   cargo build --bin bifrost
   ```
2. 准备临时目录和动态端口：
   ```bash
   TEST_DATA_DIR="$(mktemp -d)"
   PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
   ```
3. 启动真实 daemon，必须使用临时数据目录、禁用 Sync 自动登录弹窗和真实系统代理：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 \
   target/debug/bifrost start -p "$PORT" -H 127.0.0.1 --daemon --skip-cert-check --no-system-proxy --no-intercept -y
   ```
4. 轮询 Admin API：
   ```bash
   curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/proxy/address"
   ```
5. 检查 `runtime.json`：
   ```bash
   python3 - "$TEST_DATA_DIR/runtime.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["port"] == int(__import__("os").environ["PORT"])
assert data["runtime_start_mode"] == "daemon"
assert data["restartable_runtime"] is True
print(data["pid"])
PY
   ```
6. 检查 daemon 错误日志中不包含 macOS Objective-C fork safety 崩溃特征：
   ```bash
   ! grep -E 'objc_initializeAfterForkError|\\+\\[NSNumber initialize\\]' "$TEST_DATA_DIR/logs/bifrost.err"
   ```
7. 停止 daemon，确认端口释放且没有同一数据目录的 tray helper 残留：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" target/debug/bifrost stop
   ! curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/proxy/address"
   ! ps -axo command | grep -F 'bifrost __tray' | grep -F -- "$TEST_DATA_DIR"
   rm -rf "$TEST_DATA_DIR"
   ```

**预期结果**：
- `start --daemon` 返回成功，并输出 daemon PID、Admin UI、日志文件路径。
- Admin API 可访问，说明 daemon listener ready 后父进程才返回。
- `runtime.json` 中 `runtime_start_mode` 为 `daemon`，`restartable_runtime` 为 `true`，PID 对应仍在运行的 daemon 子进程。
- `logs/bifrost.err` 不包含 `objc_initializeAfterForkError` 或 `+[NSNumber initialize]`，避免 v0.0.100 upgrade 重启时 fork 后初始化 ObjC runtime 的崩溃。
- `bifrost stop` 可停止该 daemon，端口释放，并且不残留同一数据目录的 `bifrost __tray` helper；全程使用 `--no-system-proxy` 和 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`，不修改本机真实系统代理。

**执行记录**：
- 2026-06-14 执行真实 macOS daemon 回归通过。先执行 `source ~/.zshrc && cargo build --bin bifrost` 构建当前 debug 二进制，再使用临时 `BIFROST_DATA_DIR=/var/folders/0q/zf2m3_nx6f9gqfd_jx0fcljr0000gn/T/tmp.wzr0I8raPf` 和动态端口 `56501` 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 target/debug/bifrost start -p 56501 --host 127.0.0.1 --daemon --skip-cert-check --no-system-proxy --no-intercept -y`。启动输出 `Daemon started with PID: 36648`，Admin API `/_bifrost/api/proxy/address` ready；`runtime.json` 校验 `pid=36648`、`port=56501`、`runtime_start_mode=daemon`、`restartable_runtime=true`；`logs/bifrost.err` 未匹配 `objc_initializeAfterForkError` 或 `+[NSNumber initialize]`；随后 `BIFROST_DATA_DIR="$TEST_DATA_DIR" target/debug/bifrost stop` 输出 `Bifrost proxy stopped.`，端口释放，临时目录已清理。全流程使用 `--no-system-proxy` 与 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`，未修改本机真实系统代理。
- 2026-06-14 发现并修复 daemon exec child 停止后的 tray helper 残留后，复跑真实 macOS daemon 回归通过。使用临时 `BIFROST_DATA_DIR=/var/folders/0q/zf2m3_nx6f9gqfd_jx0fcljr0000gn/T/tmp.t5Knv1qqNu` 和动态端口 `58707` 启动当前 debug 二进制，输出 `Daemon started with PID: 56778`；Admin API ready；`runtime.json` 校验 `pid=56778`、`port=58707`、`runtime_start_mode=daemon`、`restartable_runtime=true`；`logs/bifrost.err` 未匹配 `objc_initializeAfterForkError` 或 `+[NSNumber initialize]`；`stop` 后端口释放，并通过 `ps -axo command | grep -F 'bifrost __tray' | grep -F -- "$TEST_DATA_DIR"` 确认 `TRAY_HELPER_LEFT=0`。临时目录已清理，未修改本机真实系统代理。

---


## 清理

测试完成后清理临时数据和规则文件：
```bash
rm -f /tmp/bifrost-test-rules.txt
```
