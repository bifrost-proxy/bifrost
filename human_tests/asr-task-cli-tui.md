# ASR Task CLI TUI 真实场景测试

## 功能模块说明

验证 `bifrost ai asr task watch/tui` 可以在不打开浏览器的情况下观察 ASR Directory Task 的执行状态、进展、消耗信息和 Daily/Jennie Agent 处理状态，并且在单任务、多任务、非交互、只读模式、文件打开和控制快捷键下行为符合预期。

## 前置条件

1. 在仓库根目录执行。
2. 使用临时数据目录，避免污染本机数据。
3. 启动服务必须带 `--no-system-proxy`。

服务启动命令：

```bash
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-tui-human.XXXXXX")"
PORT=18884
cargo build --bin bifrost
BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost start -p "$PORT" --unsafe-ssl --no-system-proxy
```

在另一个终端创建测试任务：

```bash
mkdir -p "$DATA_DIR/audio-one" "$DATA_DIR/audio-two"
curl -sS -X POST -H 'content-type: application/json' \
  --data "{\"name\":\"CLI TUI One\",\"audio_dir\":\"$DATA_DIR/audio-one\",\"recursive\":true,\"enabled\":true,\"schedule\":{\"kind\":\"hourly\",\"minute\":0}}" \
  "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks"
```

## 测试用例列表

### TC-ASR-TUI-01 单任务自动进入详情 TUI

操作步骤：

1. 确保只有一个 ASR task。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui --read-only`。
3. 按 `q` 退出。

预期结果：

- 不出现任务选择菜单。
- 直接进入 `ASR Task: CLI TUI One` 详情页。
- 页面显示 Files 进度、Consumption 区块和 Runtime 区块。
- 页面显示 Daily/Jennie Agent 区块。
- 按 `q` 后终端恢复正常输入状态。

### TC-ASR-TUI-02 多任务交互式选择进入指定任务

操作步骤：

1. 再创建一个名为 `CLI TUI Two` 的 ASR task。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui --read-only`。
3. 在选择菜单中选择 `CLI TUI Two`。
4. 按 `q` 退出。

预期结果：

- 出现 `Select ASR task to watch` 选择菜单。
- 菜单列出 `CLI TUI One` 和 `CLI TUI Two`。
- 选择后进入对应任务详情页。

### TC-ASR-TUI-03 多任务传 task 直接进入

操作步骤：

1. 记录 `CLI TUI One` 的 task id。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task watch <task_id> --json-snapshot`。

预期结果：

- 命令不出现交互选择。
- JSON 输出中的 `task.id` 等于传入 task id。
- JSON 包含 `progress`、`consumption` 和 `snapshot_source` 字段。

### TC-ASR-TUI-04 快照展示进度和消耗信息且不触发模型下载

操作步骤：

1. 使用空音频目录创建 ASR task，避免下载或启动本地 ASR 大模型。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task watch <task_id> --json-snapshot`。
3. 检查 JSON 输出中的 `progress`、`consumption`、`snapshot_source`。

预期结果：

- 命令不触发 ASR 模型下载或推理。
- JSON 包含文件进度字段、消耗字段和快照来源字段。
- 没有可计算 ETA 时 `eta_ms` 为空，`eta_confidence` 为 `none`，不显示假精确 ETA。

### TC-ASR-TUI-05 服务未启动、任务不存在、多任务非交互错误提示

操作步骤：

1. 停止 Bifrost 服务后执行 `target/debug/bifrost --port "$PORT" ai asr task watch --json-snapshot`。
2. 服务启动后执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task watch not-exist --json-snapshot`。
3. 多任务存在时执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task watch --no-interactive-select --json-snapshot`。

预期结果：

- 服务未启动时提示无法连接 Admin API，并提示启动代理。
- 任务不存在时提示 `ASR task 'not-exist' not found`。
- 多任务非交互未传 task 时提示传 task id、唯一前缀、唯一名称或 `--all`。

### TC-ASR-TUI-06 窄终端和非 Unicode 环境下布局可读

操作步骤：

1. 将终端宽度调整到约 80 列。
2. 执行 `TERM=dumb BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui <task_id> --read-only`。
3. 按 `q` 退出。

预期结果：

- 页面文本不重叠。
- 路径保留文件名或任务名关键信息。
- 按 `q` 后终端恢复正常输入状态。

### TC-ASR-TUI-07 read-only 模式不触发写操作

操作步骤：

1. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui <task_id> --read-only`。
2. 在 TUI 中按 `R`、`p` 和 `P`。
3. 按 `q` 退出。

预期结果：

- 页面底部提示 `read-only mode`。
- 按 `R` 不触发手动 run。
- 按 `p` 不触发 pause/resume。
- 按 `P` 不触发 force pause。
- 退出后任务状态没有因快捷键改变。

### TC-ASR-TUI-08 daily list/show 复用任务选择规则

操作步骤：

1. 只有一个 ASR task 时，准备 `.daily/2026-05-20.md`。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task daily list`。
3. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task daily show 2026-05-20`。
4. 创建第二个 ASR task 后，在非交互环境执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task daily list`。
5. 再执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task daily list <task_id>`。

预期结果：

- 单任务时 `daily list` 不要求 `<TASK_ID>`，自动选择唯一任务并列出 `2026-05-20`。
- 单任务时 `daily show 2026-05-20` 自动选择唯一任务并输出 Markdown 内容。
- 多任务非交互未传 task 时提示 `Multiple ASR directory tasks exist`。
- 多任务传 task id 后可直接列出指定任务 Daily 文档。

### TC-ASR-TUI-09 Daily/Jennie Agent 状态与文档打开

操作步骤：

1. 准备 `.daily/2026-05-20.md`、`.daily/report/2026-05-20-report.md` 和 `daily_agent_processed.json`。
2. 执行 `BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task watch <task_id> --json-snapshot`。
3. 执行 `BIFROST_ASR_TUI_OPEN_LOG="$DATA_DIR/open.log" BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui <task_id>`。
4. 在 TUI 中切到 Daily Agent Docs 列表，按 `Enter` 打开当前文档，然后按 `q` 退出。

预期结果：

- JSON snapshot 包含 `daily_agent` 字段。
- `processed_documents` 和 `pending_documents` 反映已处理/待处理数量。
- TUI 显示 Daily/Jennie Agent 面板和 Daily Agent Docs 列表。
- `open.log` 记录了 `2026-05-20-report.md`，说明 Enter 打开的是渲染列表中的 report 文件。

### TC-ASR-TUI-10 刷新、立即运行、暂停/恢复和强制暂停动作

操作步骤：

1. 执行 `BIFROST_ASR_TUI_OPEN_LOG="$DATA_DIR/open.log" BIFROST_DATA_DIR="$DATA_DIR" target/debug/bifrost --port "$PORT" ai asr task tui <task_id>`。
2. 按 `r` 刷新。
3. 按 `R` 立即运行；如果任务已在运行，继续观察底部消息。
4. 按 `p` 暂停，再通过 API 或 TUI 恢复。
5. 按 `P` 强制暂停。
6. 在暂停状态下调用 run API：`curl -sS -o "$DATA_DIR/run-paused.json" -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/<task_id>/run"`。

预期结果：

- `r` 刷新不改变任务状态且页面继续可操作。
- 空闲任务按 `R` 会请求运行；已运行任务按 `R` 不显示误导性 `Config error`，而是显示任务已在运行。
- `p` 暂停/恢复后 API watch snapshot 中 `task.paused` 与预期一致。
- `P` 触发 force pause，底部显示服务端返回的 force-pause/pause message。
- 暂停状态下 run API 返回 HTTP 409，JSON message 为 `ASR task is paused; resume it before starting a run`。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除临时目录：`rm -rf "$DATA_DIR"`。

## 执行记录

- 2026-05-26：针对 CI `E2E Shell (Linux, shard 2/3)` 中 `multi-task selector did not choose second task` 回归更新 TC-ASR-TUI-02 的自动化等待条件。多任务 PTY 选择用例在发送 `Down + Enter` 前必须等待选择器 prompt 和两个任务选项 `CLI TUI One` / `CLI TUI Two` 都已渲染，避免慢速 CI 终端里 prompt 先出现但列表尚未绘制完就输入导致选中不稳定。执行 `bash e2e-tests/tests/test_asr_task_tui.sh`，真实启动临时 Bifrost 服务（`--no-system-proxy`）并通过。
- 2026-05-24：执行 `bash e2e-tests/tests/test_asr_task_tui.sh`，真实启动临时 Bifrost 服务（`--no-system-proxy`）并通过。覆盖单任务自动进入、多任务 TTY 选择、非交互错误、`watch --json-snapshot`、Daily/Jennie Agent 统计、`daily list/show` 自动/参数选择、TUI Daily Agent Docs 打开日志、刷新、暂停/恢复、强制暂停、暂停状态 run 409 文案和临时目录清理。
