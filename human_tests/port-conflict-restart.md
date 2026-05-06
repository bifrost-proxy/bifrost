# 端口冲突检测与自动重启测试用例

## 功能模块说明

本文档覆盖 Bifrost 启动时端口冲突的检测与处理功能，包括：
- 启动时检测目标端口是否被其他进程占用
- 显示占用端口的进程名称和 PID
- 提示用户是否终止占用进程并继续启动
- `--yes` 参数自动确认终止占用进程

## 前置条件

1. 确保项目已编译或可编译
2. 确保无正在运行的 Bifrost 测试实例
3. 所有启动命令统一使用临时数据目录，避免污染正式环境：
   ```bash
   export BIFROST_DATA_DIR=./.bifrost-test
   ```
4. 清理残留 PID 文件：
   ```bash
   rm -f ./.bifrost-test/bifrost.pid ./.bifrost-test/runtime.json
   ```

---

## 测试用例

### TC-PCR-01：端口未占用时正常启动

**操作步骤**：
1. 确认端口 8800 未被占用：
   ```bash
   lsof -i TCP:8800 -sTCP:LISTEN -n -P
   ```
2. 启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check
   ```

**预期结果**：
- 不出现端口冲突提示
- 服务正常启动，输出包含监听地址 `0.0.0.0:8800`
- 按 Ctrl+C 停止服务

---

### TC-PCR-02：端口被其他进程占用时提示进程信息并用户选择拒绝

**操作步骤**：
1. 用 Python 占用端口 8800：
   ```bash
   python3 -c "import socket,time,os; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('0.0.0.0',8800)); s.listen(1); print('PID:', os.getpid()); time.sleep(300)" &
   ```
2. 清理 PID 文件：
   ```bash
   rm -f ./.bifrost-test/bifrost.pid ./.bifrost-test/runtime.json
   ```
3. 启动 Bifrost（管道输入 `n`）：
   ```bash
   echo "n" | BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check
   ```

**预期结果**：
- 输出包含 `Port 0.0.0.0:8800 is already in use by process "Python" (PID: <pid>). Kill it and continue? (y/n)`
- 显示正确的进程名（Python）和 PID
- 用户输入 `n` 后输出 `Start cancelled.` 并退出
- Python 进程仍在运行（未被终止）

**清理**：
```bash
kill $(lsof -t -i TCP:8800 -sTCP:LISTEN) 2>/dev/null
```

---

### TC-PCR-03：端口被其他进程占用时用户选择终止并成功启动

**操作步骤**：
1. 用 Python 占用端口 8800：
   ```bash
   python3 -c "import socket,time,os; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('0.0.0.0',8800)); s.listen(1); print('PID:', os.getpid()); time.sleep(300)" &
   ```
2. 清理 PID 文件：
   ```bash
   rm -f ./.bifrost-test/bifrost.pid ./.bifrost-test/runtime.json
   ```
3. 使用 `--yes` 参数自动确认启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check --yes
   ```

**预期结果**：
- 输出包含 `Port 0.0.0.0:8800 is already in use by process "Python" (PID: <pid>). Kill it and continue? (y/n)`
- 输出 `> y (auto-confirmed with --yes)`
- 输出 `Stopping process Python (PID: <pid>)...`
- 输出 `Process stopped. Continuing startup...`
- Python 进程被终止
- Bifrost 成功启动并监听在 8800 端口
- `curl -x http://127.0.0.1:8800 http://httpbin.org/get` 返回正常响应

**清理**：
- 按 Ctrl+C 停止 Bifrost

---

### TC-PCR-04：已有 Bifrost 进程运行时的 PID 检测仍正常工作

**操作步骤**：
1. 启动第一个 Bifrost 实例：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check &
   ```
2. 等待启动完成后，尝试启动第二个实例（管道输入 `n`）：
   ```bash
   echo "n" | BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check
   ```

**预期结果**：
- 第二个实例检测到已有 Bifrost 进程，输出 `Detected an existing Bifrost proxy process (PID: <pid>). Restart? (y/n)`
- 用户输入 `n` 后输出 `Start cancelled.`
- 第一个实例不受影响，继续运行

**清理**：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop
```

---

### TC-PCR-05：端口被 Bifrost 自身（无 PID 文件）占用时的处理

**操作步骤**：
1. 启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check &
   ```
2. 等待启动完成后，手动删除 PID 文件：
   ```bash
   rm -f ./.bifrost-test/bifrost.pid ./.bifrost-test/runtime.json
   ```
3. 使用 `--yes` 参数启动第二个实例：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --skip-cert-check --yes
   ```

**预期结果**：
- 由于 PID 文件不存在，PID 检查不触发
- 端口冲突检查检测到 8800 被占用，显示进程信息
- `--yes` 自动确认后终止旧进程
- 新实例成功启动

**清理**：
- 按 Ctrl+C 停止 Bifrost

---

### TC-PCR-06-回归：非交互端口冲突应先于系统代理摘要失败

**背景**：
CI 曾出现 `test_port_conflict_no_system_proxy_enable.sh` 中端口占用断言不稳定的问题。回归目标是验证普通进程占用端口时，`bifrost start --system-proxy` 在非交互环境会先报告端口占用并失败，不会进入或打印系统代理启用阶段。

**操作步骤**：
1. 构建当前源码的 release CLI：
   ```bash
   cargo build --release --bin bifrost
   ```
2. 执行自动化回归脚本：
   ```bash
   BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh
   ```

**预期结果**：
- 脚本输出 `应报告端口占用` 断言通过
- 脚本输出 `端口冲突检查应早于系统代理启动摘要` 断言通过
- 脚本输出 `端口冲突时不应启用系统代理` 断言通过
- 汇总结果为 `Total: 3`、`Passed: 3`、`Failed: 0`
- 测试不使用 9900 端口，且通过临时 `BIFROST_DATA_DIR` 隔离数据

**执行记录**：
- 2026-05-04 先执行脚本失败，原因是 `target/release/bifrost` 在本轮 `cargo clean` 后不存在，不属于产品行为失败。随后执行 `cargo build --release --bin bifrost` 构建当前源码，再执行 `bash e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh`，3/3 断言通过。

---

## 清理步骤

1. 停止所有测试实例：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- stop 2>/dev/null
   ```
2. 清理端口占用的测试进程：
   ```bash
   kill $(lsof -t -i TCP:8800 -sTCP:LISTEN) 2>/dev/null
   ```
3. 清理 PID 文件：
   ```bash
   rm -f ./.bifrost-test/bifrost.pid ./.bifrost-test/runtime.json
   ```
