# CLI 日志输出默认行为测试用例

## 功能模块说明

本文档验证 `--log-output` 全局参数的默认行为修复（Bug 修复回归测试）：

**修复前的问题**：默认前台启动会把 tracing 标准日志写到 Console Terminal，后续无法在终端区域稳定承载额外交互能力。

**修复后的预期行为**：
- `start -d`（daemon 模式）：日志仅输出到文件（由 `reinit_logging_for_daemon` 控制）
- `start`（前台模式）：日志默认仅输出到文件，stdout/stderr 不出现 tracing 标准日志行
- 其他所有命令：日志默认仅输出到文件，stdout/stderr 只保留命令协议或用户可见结果
- 用户可通过全局 `--log-output console` 或 `--log-output console,file` 显式启用 Console Terminal 日志；文件日志仍保留
- 日志目录清理默认保留 7 天，并设置 1GiB 目录总量上限；超过年龄或容量上限的已知 Bifrost 日志产物会按旧到新删除，避免资源持续膨胀

## 前置条件

1. 确保项目已编译或可编译
2. 确保端口 8800 未被占用
3. 所有启动类测试必须禁用系统代理并禁用 Sync 自动登录弹窗：
   ```bash
   export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   ```
4. 本文档验证日志输出通道时统一显式设置 `RUST_LOG=info`，避免本机 shell 环境中的 `RUST_LOG=warn` 或更高等级把验证用日志过滤掉：
   ```bash
   export RUST_LOG=info
   ```
5. 所有测试命令统一使用临时数据目录：
   ```bash
   export BIFROST_DATA_DIR=./.bifrost-test
   ```
6. 清理旧日志文件：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```

---

## 测试用例

### TC-LOD-01：status 命令默认写日志文件且不输出 tracing 标准日志

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 status 命令（不带 --log-output 参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status > /tmp/bifrost-status.stdout 2> /tmp/bifrost-status.stderr || true
   ```
3. 检查日志目录是否产生了日志文件，且 stdout/stderr 不包含 tracing 标准日志行：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   cat /tmp/bifrost-status.stdout /tmp/bifrost-status.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: log file created`
- 终端输出 `PASS: no console tracing log`

---

### TC-LOD-02：stop 命令默认写日志文件且不输出 tracing 标准日志

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 stop 命令（不带 --log-output 参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop > /tmp/bifrost-stop.stdout 2> /tmp/bifrost-stop.stderr || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   cat /tmp/bifrost-stop.stdout /tmp/bifrost-stop.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: log file created`
- 终端输出 `PASS: no console tracing log`

---

### TC-LOD-03：rule list 命令默认写日志文件且不输出 tracing 标准日志

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 rule list 命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- rule list > /tmp/bifrost-rule-list.stdout 2> /tmp/bifrost-rule-list.stderr || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   cat /tmp/bifrost-rule-list.stdout /tmp/bifrost-rule-list.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: no console tracing log`

---

### TC-LOD-04：全局 --log-output console 显式启用 Console Terminal 日志且保留文件日志

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 status 命令并显式指定 --log-output console：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-output console status > /tmp/bifrost-status-console.stdout 2> /tmp/bifrost-status-console.stderr || true
   ```
3. 检查日志目录和 stdout/stderr：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   cat /tmp/bifrost-status-console.stdout /tmp/bifrost-status-console.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "PASS: console tracing log enabled" || echo "FAIL: console tracing log missing"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: log file created`
- 终端输出 `PASS: console tracing log enabled`

---

### TC-LOD-05：start 前台模式默认写文件且不输出 tracing 标准日志（回归验证）

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 启动前台服务（不带 --log-output 参数），必须禁用系统代理，等待启动后立即停止：
   ```bash
   timeout 8 bash -c 'BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check --unsafe-ssl --no-system-proxy' > /tmp/bifrost-start.stdout 2> /tmp/bifrost-start.stderr || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   cat /tmp/bifrost-start.stdout /tmp/bifrost-start.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: log file created`
- 终端输出 `PASS: no console tracing log`

---

### TC-LOD-06：start -d daemon 模式写日志到文件

**操作步骤**：
1. 清理日志目录和旧进程：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop 2>/dev/null || true
   ```
2. 以 daemon 模式启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -d -p 8800 --unsafe-ssl -y
   ```
3. 等待 daemon 启动并写入日志：
   ```bash
   sleep 3
   ```
4. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   ```
5. 清理 daemon 进程：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件（daemon 模式默认写文件）
- 终端输出 `PASS: log file created`

---

### TC-LOD-07：daemon 模式日志文件包含结构化 tracing 元数据

**操作步骤**：
1. 清理临时目录并构建当前 release 二进制：
   ```bash
   rm -rf /tmp/bifrost-human-daemon-log-level
   mkdir -p /tmp/bifrost-human-daemon-log-level
   source ~/.zshrc
   cargo build --release --bin bifrost
   ```
2. 使用临时数据目录启动 daemon，必须禁用系统代理：
   ```bash
   RUST_LOG=debug BIFROST_DATA_DIR=/tmp/bifrost-human-daemon-log-level/data \
   target/release/bifrost -l debug --log-dir /tmp/bifrost-human-daemon-log-level/logs \
     start -p 18891 --skip-cert-check --unsafe-ssl --no-system-proxy --daemon
   ```
3. 等待管理端可用并发起一次代理请求：
   ```bash
   for i in {1..60}; do
     curl -fsS http://127.0.0.1:18891/_bifrost/api/proxy/address >/dev/null && break
     sleep 0.5
   done
   curl -sS -o /tmp/bifrost-human-daemon-log-level/response.html \
     -x http://127.0.0.1:18891 http://example.com
   ```
4. 检查 daemon rolling log 是否包含带时间、级别、文件和行号的 tracing 日志：
   ```bash
   grep -R -nE '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) .+\.rs:[0-9]+:' \
     /tmp/bifrost-human-daemon-log-level/logs/bifrost*.log \
     && echo "PASS: structured daemon tracing log exists" \
     || echo "FAIL: structured daemon tracing log missing"
   ```
5. 停止服务并清理：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-human-daemon-log-level/data target/release/bifrost stop
   rm -rf /tmp/bifrost-human-daemon-log-level
   ```

**预期结果**：
- daemon 服务成功启动，且没有修改系统代理
- 代理请求成功
- 终端输出 `PASS: structured daemon tracing log exists`
- 匹配到的日志行包含 `.rs:<line>:` 元数据，证明 daemon 子进程重建后的文件日志链路可用

---

### TC-LOD-08：start 默认 info 日志不刷常态连接生命周期噪声（回归验证）

**操作步骤**：
1. 清理临时数据目录并准备日志目录：
   ```bash
   rm -rf /tmp/bifrost-human-log-noise
   mkdir -p /tmp/bifrost-human-log-noise/logs
   ```
2. 使用临时数据目录和非正式端口以前台模式启动服务，必须加载用户环境变量并禁用系统代理：
   ```bash
   source ~/.zshrc
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DATA_DIR=/tmp/bifrost-human-log-noise/data CARGO_TARGET_DIR=/tmp/bifrost-human-log-noise/target RUST_LOG=info cargo run --bin bifrost -- --log-dir /tmp/bifrost-human-log-noise/logs start --host 127.0.0.1 -p 18884 --skip-cert-check --unsafe-ssl --no-system-proxy > /tmp/bifrost-human-log-noise/server.stdout 2> /tmp/bifrost-human-log-noise/server.stderr &
   echo $! > /tmp/bifrost-human-log-noise/server.pid
   ```
3. 等待管理端 API ready：
   ```bash
   for i in {1..60}; do
     curl -fsS http://127.0.0.1:18884/_bifrost/api/proxy/address >/dev/null && break
     sleep 0.5
   done
   ```
4. 制造一次短连接提前关闭，模拟浏览器/健康检查类连接生命周期噪声：
   ```bash
   python3 - <<'PY'
import socket
s = socket.create_connection(("127.0.0.1", 18884), timeout=3)
s.sendall(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n")
s.close()
PY
   ```
5. 等待日志 flush 后检查默认 `info` 日志：
   ```bash
   sleep 2
   grep -E "Noisy connection close|Failed to resolve client process after retries|Async client process resolution completed without a match|Push client registered|Push client unregistered|Client closed connection|WebSocket connection closed|dispatching SSE event.*ping" /tmp/bifrost-human-log-noise/logs/bifrost*.log && echo "FAIL: noisy lifecycle logs visible at info" || echo "PASS: noisy lifecycle logs hidden at info"
   cat /tmp/bifrost-human-log-noise/server.stdout /tmp/bifrost-human-log-noise/server.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```
6. 停止服务：
   ```bash
   kill "$(cat /tmp/bifrost-human-log-noise/server.pid)" 2>/dev/null || true
   wait "$(cat /tmp/bifrost-human-log-noise/server.pid)" 2>/dev/null || true
   ```

**预期结果**：
- 服务在 `127.0.0.1:18884` 成功启动，且没有修改系统代理
- 终端输出 `PASS: noisy lifecycle logs hidden at info`
- 终端输出 `PASS: no console tracing log`
- `/tmp/bifrost-human-log-noise/logs/bifrost*.log` 中不出现以下默认等级噪声：
  - `Noisy connection close`
  - `Failed to resolve client process after retries`
  - `Async client process resolution completed without a match`
  - `Push client registered` / `Push client unregistered`
  - `Client closed connection` / `WebSocket connection closed`
  - `dispatching SSE event event=ping`

---

### TC-LOD-09：start 默认 info 日志不刷规则命中详情（回归验证）

**操作步骤**：
1. 清理临时数据目录并准备日志目录：
   ```bash
   rm -rf /tmp/bifrost-human-rule-log-noise
   mkdir -p /tmp/bifrost-human-rule-log-noise/info-logs /tmp/bifrost-human-rule-log-noise/debug-logs
   ```
2. 先基于当前源码编译最新二进制：
   ```bash
   source ~/.zshrc
   CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/bifrost-human-rule-log-noise/target cargo build --bin bifrost
   ```
3. 使用临时数据目录和非正式端口以前台模式启动服务，必须加载用户环境变量并禁用系统代理：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DATA_DIR=/tmp/bifrost-human-rule-log-noise/data RUST_LOG=info /tmp/bifrost-human-rule-log-noise/target/debug/bifrost --log-dir /tmp/bifrost-human-rule-log-noise/info-logs start --host 127.0.0.1 -p 18885 --skip-cert-check --unsafe-ssl --no-system-proxy --rules "example.test status://200 resBody://ok" > /tmp/bifrost-human-rule-log-noise/info.stdout 2> /tmp/bifrost-human-rule-log-noise/info.stderr &
   echo $! > /tmp/bifrost-human-rule-log-noise/info.pid
   ```
4. 等待管理端 API ready：
   ```bash
   for i in {1..60}; do
     curl -fsS http://127.0.0.1:18885/_bifrost/api/proxy/address >/dev/null && break
     sleep 0.5
   done
   ```
5. 通过代理请求命中规则：
   ```bash
   http_proxy=http://127.0.0.1:18885 https_proxy=http://127.0.0.1:18885 no_proxy= curl -sS --max-time 5 http://example.test/
   ```
6. 等待日志 flush 后检查默认 `info` 日志：
   ```bash
   sleep 1
   grep -E "rule matcher candidate matched|rule selected|rules matched for request|matched rule detail" /tmp/bifrost-human-rule-log-noise/info-logs/bifrost*.log && echo "FAIL: rule match logs visible at info" || echo "PASS: rule match logs hidden at info"
   cat /tmp/bifrost-human-rule-log-noise/info.stdout /tmp/bifrost-human-rule-log-noise/info.stderr | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "FAIL: console tracing log" || echo "PASS: no console tracing log"
   ```
7. 停止 info 服务：
   ```bash
   kill "$(cat /tmp/bifrost-human-rule-log-noise/info.pid)" 2>/dev/null || true
   wait "$(cat /tmp/bifrost-human-rule-log-noise/info.pid)" 2>/dev/null || true
   ```
8. 再次以前台模式启动服务，但显式打开规则调试日志：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DATA_DIR=/tmp/bifrost-human-rule-log-noise/data-debug RUST_LOG='bifrost_core::rules=debug,bifrost_proxy::rules=trace,info' /tmp/bifrost-human-rule-log-noise/target/debug/bifrost --log-dir /tmp/bifrost-human-rule-log-noise/debug-logs start --host 127.0.0.1 -p 18886 --skip-cert-check --unsafe-ssl --no-system-proxy --rules "example.test status://200 resBody://ok" > /tmp/bifrost-human-rule-log-noise/debug.stdout 2> /tmp/bifrost-human-rule-log-noise/debug.stderr &
   echo $! > /tmp/bifrost-human-rule-log-noise/debug.pid
   for i in {1..60}; do
     curl -fsS http://127.0.0.1:18886/_bifrost/api/proxy/address >/dev/null && break
     sleep 0.5
   done
   ```
9. 通过代理请求命中规则并检查调试日志：
   ```bash
   http_proxy=http://127.0.0.1:18886 https_proxy=http://127.0.0.1:18886 no_proxy= curl -sS --max-time 5 http://example.test/
   sleep 1
   grep -E "rule matcher candidate matched|rule selected|rules matched for request|matched rule detail" /tmp/bifrost-human-rule-log-noise/debug-logs/bifrost*.log && echo "PASS: rule match logs visible when explicitly enabled" || echo "FAIL: rule match logs missing when explicitly enabled"
   ```
10. 停止 debug 服务：
   ```bash
   kill "$(cat /tmp/bifrost-human-rule-log-noise/debug.pid)" 2>/dev/null || true
   wait "$(cat /tmp/bifrost-human-rule-log-noise/debug.pid)" 2>/dev/null || true
   ```

**预期结果**：
- 服务分别在 `127.0.0.1:18885` 和 `127.0.0.1:18886` 成功启动，且没有修改系统代理
- 两次代理请求均返回 `ok`
- 默认 `RUST_LOG=info` 时输出 `PASS: rule match logs hidden at info`
- 默认启动 stdout/stderr 输出 `PASS: no console tracing log`
- 显式 `RUST_LOG='bifrost_core::rules=debug,bifrost_proxy::rules=trace,info'` 时输出 `PASS: rule match logs visible when explicitly enabled`

---

### TC-LOD-10：macOS LaunchDaemon cleanup 隐藏命令保留 console 日志（回归验证）

**操作步骤**：
1. 清理临时数据目录并构建当前 release 二进制：
   ```bash
   rm -rf /tmp/bifrost-human-launchd-log
   mkdir -p /tmp/bifrost-human-launchd-log/data /tmp/bifrost-human-launchd-log/logs
   source ~/.zshrc
   cargo build --release --bin bifrost
   ```
2. 在 macOS 上运行隐藏的 cleanup daemon 命令，模拟 LaunchDaemon 的 `StandardOutPath` / `StandardErrorPath` 重定向：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-human-launchd-log/data \
   target/release/bifrost --log-dir /tmp/bifrost-human-launchd-log/logs \
     system-proxy cleanup-daemon \
     --data-dir /tmp/bifrost-human-launchd-log/data \
     > /tmp/bifrost-human-launchd-log/stdout.log \
     2> /tmp/bifrost-human-launchd-log/stderr.log
   ```
3. 检查 rolling file 和 stdout/stderr：
   ```bash
   ls /tmp/bifrost-human-launchd-log/logs/bifrost*.log 2>/dev/null && echo "PASS: file log created" || echo "FAIL: no file log"
   cat /tmp/bifrost-human-launchd-log/stdout.log /tmp/bifrost-human-launchd-log/stderr.log | grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' && echo "PASS: launchd console tracing log kept" || echo "FAIL: launchd console tracing log missing"
   ```
4. 清理临时目录：
   ```bash
   rm -rf /tmp/bifrost-human-launchd-log
   ```

**预期结果**：
- rolling file 日志存在
- stdout/stderr 中出现 cleanup daemon 的 tracing 日志行，证明 LaunchDaemon `StandardOutPath` / `StandardErrorPath` 场景没有被默认 file-only 行为静音
- 该例外仅适用于 macOS `system-proxy cleanup-daemon` 隐藏命令；普通 `start` 默认仍不输出 Console tracing 日志

---

### TC-LOD-11：日志断言类 E2E 显式启用 Console tracing（CI 回归验证）

**操作步骤**：
1. 执行 ChatGPT Web startup auth 预检脚本：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh
   ```
2. 执行规则匹配日志噪声脚本：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_rule_match_logging_noise.sh
   ```
3. 执行 SOCKS5 TLS routing exceptions 脚本：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```
4. 检查以上脚本中的 Bifrost 启动命令：只有需要读取 tracing stdout 的断言场景才显式传入 `--log-output console,file`。

**预期结果**：
- 三个脚本均通过。
- 需要断言 tracing 日志内容的脚本可以在重定向日志文件中读到目标日志。
- 普通默认 `start` 行为不被放宽，仍按 TC-LOD-05 验证为不输出 Console tracing 日志。

---

### TC-LOD-12：log-output 默认文件 E2E 停止流程有界退出（CI 回归验证）

**操作步骤**：
1. 对脚本做 Bash 语法检查：
   ```bash
   bash -n e2e-tests/tests/test_cli_start_log_output_default_file.sh
   ```
2. 使用已构建 release 二进制执行真实 log-output 默认文件 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$(pwd)/target/release/bifrost" \
     e2e-tests/tests/test_cli_start_log_output_default_file.sh
   ```
3. 观察脚本输出，确认 `default-file` 与 `console-file` 两段都完成 Admin API 请求、停止代理、写入文件日志和 Console tracing 断言。
4. 若 `bifrost stop` 因运行时状态异常卡住，脚本必须通过有界 stop helper 与 `safe_cleanup_proxy` 在超时时清理代理进程，并继续进入测试汇总，不能在已输出单条 PASS 后挂到 CI 900 秒超时。

**预期结果**：
- 语法检查通过。
- 真实脚本输出 `Results: ... failed=0`，总耗时为秒级，不出现 900 秒 timeout。
- 脚本全程使用临时数据目录、`--no-system-proxy` 与隔离端口，不修改用户系统代理。

---

### TC-LOD-13：日志目录按 7 天保留并按 1GiB 上限清理（回归验证）

**操作步骤**：
1. 执行核心日志清理 focused 单测：
   ```bash
   source ~/.zshrc
   cargo test -p bifrost-core cleanup_bifrost_log_dir -- --nocapture
   ```
2. 检查单测覆盖点：
   - `cleanup_bifrost_log_dir_removes_legacy_and_shared_dated_logs`
   - `cleanup_bifrost_log_dir_removes_old_fixed_log_artifacts_by_mtime`
   - `cleanup_bifrost_log_dir_enforces_total_size_by_removing_oldest_logs`
3. 确认 CLI、daemon、Tray 和 Desktop 都调用共享清理路径：
   ```bash
   rg -n "cleanup_bifrost_log_dir|DEFAULT_LOG_RETENTION_DAYS|DEFAULT_LOG_DIR_MAX_BYTES" \
     crates/bifrost-core/src/logging.rs \
     crates/bifrost-cli/src/commands/tray/tray.rs \
     desktop/src-tauri/src/main.rs
   ```

**预期结果**：
- 7 天保留清理覆盖 `bifrost.YYYY-MM-DD.log`、历史 `log-YYYY-MM-DD.log`、`tray.log.YYYY-MM-DD` 和 `bifrost.err.YYYY-MM-DD`。
- 固定文件名日志如 `desktop-bootstrap.log`、`desktop-sidecar.out.log`、`desktop-sidecar.err.log`、`guardian.log`、`restart.log`、`upgrade-background.log` 和 `*-audit.json` 按 mtime 清理。
- 总量超过 1GiB 时，即使文件仍在 7 天窗口内，也会从最旧的已知 Bifrost 日志产物开始删除，直到目录总量不超过上限。
- 未知普通文件不被清理，避免误删用户放在日志目录中的非日志文件。

---

## 清理步骤

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop 2>/dev/null || true
rm -rf ./.bifrost-test
rm -rf /tmp/bifrost-human-log-noise
rm -rf /tmp/bifrost-human-rule-log-noise
rm -rf /tmp/bifrost-human-launchd-log
```

## 执行记录

| 日期 | 用例 | 实际结果 | 结论 |
| --- | --- | --- | --- |
| 2026-06-08 | TC-LOD-11 | 已执行 `SKIP_BUILD=true e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh`、`SKIP_BUILD=true e2e-tests/tests/test_rule_match_logging_noise.sh`、`SKIP_BUILD=true e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`；三者均通过。CI 失败 artifacts 证实旧脚本在 Linux/macOS 中依赖默认 stdout tracing，修复后这些日志断言脚本改为显式 `--log-output console,file`。 | 通过 |
| 2026-06-12 | TC-LOD-11 | PR #225 CI 中 Linux/macOS `E2E Shell shard 3/3` 均失败于 `test_rule_match_logging_noise.sh`。已将脚本断言从旧 `rule MATCHED` 更新为 `rule matcher candidate matched` 与 `rule selected`，并执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 e2e-tests/tests/test_rule_match_logging_noise.sh`，结果通过。 | 通过 |
| 2026-06-12 | TC-LOD-11 | PR #227 CI run `27423667730` macOS `E2E Shell (aarch64-apple-darwin, shard 1/3)` failed because `test_rule_match_logging_noise.sh` used fixed debug port `18888`, which was already occupied on the runner. The script now allocates info/debug ports dynamically through `allocate_free_port` while preserving `INFO_PORT` / `DEBUG_PORT` overrides; `bash -n e2e-tests/tests/test_rule_match_logging_noise.sh` passed. | 通过 |
| 2026-06-13 | TC-LOD-12 | PR #236 CI run `27472448398` 中 macOS shard 1 先显示 `console-file admin request` 通过，随后 `test_cli_start_log_output_default_file.sh` 在停止流程卡到 900 秒 timeout。已将脚本的 `bifrost stop` 改为有界 best-effort helper，并在 stop 超时后使用 `safe_cleanup_proxy` 与端口清理兜底。执行 `bash -n e2e-tests/tests/test_cli_start_log_output_default_file.sh` 通过；执行 `BIFROST_E2E_STOP_TIMEOUT=3 SKIP_BUILD=true BIFROST_BIN="$(pwd)/target/release/bifrost" e2e-tests/tests/test_cli_start_log_output_default_file.sh` 通过，输出 10/10 passed，总耗时 7.6 秒。 | 通过 |
| 2026-06-23 | TC-LOD-13 | 已执行 `cargo test -p bifrost-core cleanup_bifrost_log_dir -- --nocapture`；覆盖历史日期日志、desktop/sidecar/audit 固定日志 mtime 清理，以及容量超限时从旧到新删除到 1GiB 上限的路径。 | 通过 |
