# CLI 日志输出默认行为测试用例

## 功能模块说明

本文档验证 `--log-output` 参数的默认行为修复（Bug 修复回归测试）：

**修复前的问题**：`--log-output` 默认值为 `console,file`，导致所有命令（stop、status、rule 等）都会向磁盘写入日志文件。

**修复后的预期行为**：
- `start -d`（daemon 模式）：日志仅输出到文件（由 `reinit_logging_for_daemon` 控制）
- `start`（前台模式）：日志默认仅输出到 console
- 其他所有命令：日志默认仅输出到 console
- 用户可通过 `--log-output file` 或 `--log-output console,file` 显式指定输出到文件

## 前置条件

1. 确保项目已编译或可编译
2. 确保端口 8800 未被占用
3. 所有测试命令统一使用临时数据目录：
   ```bash
   export BIFROST_DATA_DIR=./.bifrost-test
   ```
4. 清理旧日志文件：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```

---

## 测试用例

### TC-LOD-01：status 命令默认不写日志文件

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 status 命令（不带 --log-output 参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- status 2>&1 || true
   ```
3. 检查日志目录是否产生了日志文件：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "FAIL: log file created" || echo "PASS: no log file"
   ```

**预期结果**：
- 日志目录下不存在 `bifrost*.log` 文件
- 终端输出 `PASS: no log file`

---

### TC-LOD-02：stop 命令默认不写日志文件

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 stop 命令（不带 --log-output 参数）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop 2>&1 || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "FAIL: log file created" || echo "PASS: no log file"
   ```

**预期结果**：
- 日志目录下不存在 `bifrost*.log` 文件
- 终端输出 `PASS: no log file`

---

### TC-LOD-03：rule list 命令默认不写日志文件

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 rule list 命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- rule list 2>&1 || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "FAIL: log file created" || echo "PASS: no log file"
   ```

**预期结果**：
- 日志目录下不存在 `bifrost*.log` 文件

---

### TC-LOD-04：非 start 命令使用 --log-output file 时写日志文件

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 执行 status 命令并显式指定 --log-output file：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- --log-output file status 2>&1 || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "PASS: log file created" || echo "FAIL: no log file"
   ```

**预期结果**：
- 日志目录下存在 `bifrost*.log` 文件
- 终端输出 `PASS: log file created`

---

### TC-LOD-05：start 前台模式默认不写日志文件（回归验证）

**操作步骤**：
1. 清理日志目录：
   ```bash
   rm -rf ./.bifrost-test/logs/bifrost*.log
   ```
2. 启动前台服务（不带 --log-output 参数），等待启动后立即停止：
   ```bash
   timeout 5 bash -c 'BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl' 2>&1 || true
   ```
3. 检查日志目录：
   ```bash
   ls ./.bifrost-test/logs/bifrost*.log 2>/dev/null && echo "FAIL: log file created" || echo "PASS: no log file"
   ```

**预期结果**：
- 日志目录下不存在 `bifrost*.log` 文件
- 终端输出 `PASS: no log file`
- 日志信息仅在终端（console）中可见

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

**执行记录**：
- 2026-05-04 执行 `bash e2e-tests/tests/test_daemon_log_level_e2e.sh` 补充验证 daemon 启动 readiness 后 Admin API 可达、代理请求成功、日志文件包含结构化 tracing 行，脚本汇总 `3/3` 断言通过。

---

### TC-LOD-07：start 默认 info 日志不刷常态连接生命周期噪声（回归验证）

**操作步骤**：
1. 清理临时数据目录并准备日志文件：
   ```bash
   rm -rf /tmp/bifrost-human-log-noise
   mkdir -p /tmp/bifrost-human-log-noise
   ```
2. 使用临时数据目录和非正式端口以前台模式启动服务，必须加载用户环境变量并禁用系统代理：
   ```bash
   source ~/.zshrc
   BIFROST_DATA_DIR=/tmp/bifrost-human-log-noise/data CARGO_TARGET_DIR=/tmp/bifrost-human-log-noise/target RUST_LOG=info cargo run --bin bifrost -- start --host 127.0.0.1 -p 18884 --unsafe-ssl --no-system-proxy > /tmp/bifrost-human-log-noise/server.log 2>&1 &
   echo $! > /tmp/bifrost-human-log-noise/server.pid
   ```
3. 等待代理监听启动完成：
   ```bash
   for i in {1..60}; do
     grep -q "Unified proxy server listening on 127.0.0.1:18884" /tmp/bifrost-human-log-noise/server.log && break
     sleep 1
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
   grep -E "Noisy connection close|Failed to resolve client process after retries|Async client process resolution completed without a match|Push client registered|Push client unregistered|Client closed connection|WebSocket connection closed|dispatching SSE event.*ping" /tmp/bifrost-human-log-noise/server.log && echo "FAIL: noisy lifecycle logs visible at info" || echo "PASS: noisy lifecycle logs hidden at info"
   ```
6. 停止服务：
   ```bash
   kill "$(cat /tmp/bifrost-human-log-noise/server.pid)" 2>/dev/null || true
   wait "$(cat /tmp/bifrost-human-log-noise/server.pid)" 2>/dev/null || true
   ```

**预期结果**：
- 服务在 `127.0.0.1:18884` 成功启动，且没有修改系统代理
- 终端输出 `PASS: noisy lifecycle logs hidden at info`
- `/tmp/bifrost-human-log-noise/server.log` 中不出现以下默认等级噪声：
  - `Noisy connection close`
  - `Failed to resolve client process after retries`
  - `Async client process resolution completed without a match`
  - `Push client registered` / `Push client unregistered`
  - `Client closed connection` / `WebSocket connection closed`
  - `dispatching SSE event event=ping`

**执行记录**：
- 2026-05-05 执行 TC-LOD-07：`source ~/.zshrc` 后使用 `BIFROST_DATA_DIR=/tmp/bifrost-human-log-noise/data CARGO_TARGET_DIR=/tmp/bifrost-human-log-noise/target RUST_LOG=info cargo run --bin bifrost -- start --host 127.0.0.1 -p 18884 --unsafe-ssl --no-system-proxy` 启动，通过短连接提前关闭复现连接生命周期噪声入口，最终输出 `PASS: noisy lifecycle logs hidden at info`。

---

## 清理步骤

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop 2>/dev/null || true
rm -rf ./.bifrost-test
rm -rf /tmp/bifrost-human-log-noise
```
