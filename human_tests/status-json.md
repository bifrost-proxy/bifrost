# `bifrost status --format json` 手测用例

## 功能模块说明

`bifrost status --format json` / `--format json-pretty` 向运维和上层工具暴露机器可读的 schema v1 状态文档。本文件验证：

- 默认 `--format text` 不改变旧 CLI 输出（向后兼容门禁）。
- `--format json` 在 running / stopped / admin API 部分失败三种场景下都能产出符合 schema v1 的 JSON。

## 前置条件

- macOS 或 Linux 开发机
- 已构建二进制：`cargo build -p bifrost-cli`
- 临时数据目录 `./.bifrost-test-status-json`
- 启动命令统一带护栏：`BIFROST_DATA_DIR=./.bifrost-test-status-json BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost start -p 28800 --no-system-proxy`

## 用例

### TC-SJ-01 running 全字段

操作步骤：

1. 启动 bifrost（见前置条件）。
2. 执行 `target/debug/bifrost status --format json | jq .`
3. 执行 `target/debug/bifrost status --format json-pretty`

预期结果：

- 单行 JSON 与 pretty JSON 内容等价（仅缩进不同）。
- `schema_version == 1`、`running == true`、`pid` 与 `runtime.json` 内 pid 一致、`listener.port == 28800`。
- `system_proxy.supported == true`、`system_proxy.enabled == false`（启动时带 `--no-system-proxy`）。
- `tls`、`active_rules`、`ports` 均不是 `null`，`errors == []`。
- `data_dir` 指向 `./.bifrost-test-status-json` 的绝对路径。

### TC-SJ-02 stopped

操作步骤：

1. 停止 bifrost：`target/debug/bifrost stop`
2. 执行 `target/debug/bifrost status --format json | jq .`

预期结果：

- `running == false`、`pid == null`、`uptime_sec == null`、`listener == null`。
- `tls == null`、`active_rules == null`、`ports == null`。
- `system_proxy` 字段仍存在（不影响 OS 当前代理读取）。
- `errors == []`（stopped 不会主动调用 admin API，因此不算失败）。

### TC-SJ-03 admin API 部分失败

操作步骤：

1. 启动 bifrost（同 TC-SJ-01）。
2. 用 `sudo pfctl -e` 或临时 iptables/route 规则把回环 `127.0.0.1:28800` -> drop（macOS 可改用临时 `--port` 把 admin 切到另一个端口，然后让 status 读到旧端口）。
   > 简化版：直接 `kill -STOP <bifrost pid>`，使 admin TCP accept 卡住，然后在另一个终端执行 status；测试结束 `kill -CONT <pid>` 恢复。
3. 执行 `target/debug/bifrost status --format json | jq .`
4. `kill -CONT <pid>` 恢复。

预期结果：

- `running == true`、`pid` 仍正确。
- `tls`、`active_rules`、`ports` 均为 `null`。
- `errors` 数组至少包含三条 `{source: "tls_config"|"rules"|"ports"|"active_summary", message: "..."}`。
- 退出码为 0（status 仍然成功，只是部分字段降级）。

### TC-SJ-04 text 默认输出回归

操作步骤：

1. running 状态下执行 `target/debug/bifrost status > new.txt`。
2. 用此分支前的 main 构建（或 stash 临时撤掉新代码）的二进制生成 `old.txt`。
3. `diff new.txt old.txt`。

预期结果：

- 两份文本完全一致；新增 `--format` 不能改变默认文本输出。

## 清理步骤

- `target/debug/bifrost stop`
- `rm -rf ./.bifrost-test-status-json new.txt old.txt`
