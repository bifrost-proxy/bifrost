# ASR 定时任务 Runtime 策略

## 功能模块说明

验证 WebUI ASR 定时任务的 runtime 策略、状态持久化、日志证据和资源释放行为。当前默认策略为 `reuse_per_file`：每个离线文件内部复用一个 `asr-server`，文件处理完后停止本次任务拉起的服务，便于获得当前实测最快的离线文件处理性能，同时仍保留 `fork_per_chunk`、`reuse_server`、`auto` 和 `compare` 用于对照实验。

核心改动：
- 新增 `asr_cli_invoke.rs` 共享模块（CLI 和 WebUI 共用）
- `asr_jobs.rs` 默认使用 `reuse_per_file`，并保留可显式选择的 fork-per-chunk 对照路径
- 0 字节文件在发现阶段跳过
- chunk 级别进度实时同步到 FileStore（WebUI 可查看）
- 每 chunk 最多重试 3 次应对瞬态 GPU 崩溃
- native `asr` 子进程默认启用 macOS physical-footprint guard；阈值先按模型官方规模和 30 秒 chunk 实测峰值定上限，再用宿主机内存做安全阀，1.7B 默认 18432 MiB，超过 `BIFROST_ASR_MAX_FOOTPRINT_MB` 安全收紧值或默认上限后立即 kill 并进入 bisect，避免 1.7B 特定 chunk 把 unified memory 顶到 20G+；为避免 `vmmap -summary` 采样拖慢推理，physical footprint 默认每 5 秒采样一次，可通过 `BIFROST_ASR_PHYSICAL_SAMPLE_INTERVAL_SECS=2..60` 调整
- WebUI 托管 `asr-server` 启动后同样启用 Bifrost 外层 physical-footprint watchdog；当前 qwen3_asr_rs v0.2.0 二进制内部没有设置 MLX memory/cache/wired limit，默认沿用 MLX runtime；`bifrost ai asr stream-file` 默认临时启动/复用 `asr-server` 做文件级复用，长驻 `bifrost ai asr start` 后续需收敛到 daemon/supervisor 托管才能持续 watchdog
- streaming plain-text ASR 请求通过 `BIFROST_ASR_TEXT_REQUEST_TIMEOUT_SECS` 限制，默认 45 秒；managed `asr-server` 连续失败达到阈值后明确切换剩余 chunk 到 `fork_per_chunk` 隔离，避免连接失败或服务卡死导致整批任务长时间停滞
- memory-limit chunk 会记录 `memory_limit_hints`，后续同一文件、同一模型、同一 chunk 直接用已学习的小窗口，不再先重撞完整 30 秒风险路径
- 长音频逐 chunk 切片、逐 chunk 删除，不预先并发切出全部 30 秒窗口，避免长录音触发无界 `ffmpeg` 进程和临时文件膨胀
- 运行中的目录任务支持资源让路 pause/resume；普通 pause 在文件或 chunk 边界释放计算资源，`pause?force=true` 会主动 abort 当前 native `asr` 或 `ffmpeg` 子进程，resume 复用现有 pending/failed 文件恢复

## 前置条件

- 当前目录为 Bifrost 仓库根目录
- 机器为 Apple Silicon（`uname -m` 输出 `arm64`）
- ASR 资产已安装（`~/.bifrost/asr/qwen3_asr_rs/asr` 可执行）
- 测试音频目录存在且包含多个 WAV 文件（建议包含 0 字节文件用于跳过验证）
- 使用临时数据目录 `.bifrost-test-planb`，端口不使用 9900

## 测试用例列表

### TC-ASPB-01 单元测试通过

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-admin asr_cli_invoke
   ```

预期结果：

- 5 个测试全部通过：parse_asr_tag, parse_asr_tag_no_close, parse_fallback_text_prefix, parse_fallback_last_line, parse_empty
- exit code 0

### TC-ASPB-02 Clippy 检查通过

操作步骤：

1. 执行：
   ```bash
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   ```

预期结果：

- 编译成功，无 clippy 错误或警告
- exit code 0

### TC-ASPB-03 服务启动与任务创建

操作步骤：

1. 启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-planb cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
   ```
2. 创建定时任务：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks \
     -H 'Content-Type: application/json' \
     -d '{"name":"Plan B Test","audio_dir":"~/Downloads/TX_MIC001_20260514_170046","recursive":true,"language":"zh","model":"qwen3-asr-1.7b","interval_seconds":3600}'
   ```
3. 查看任务列表：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks
   ```

预期结果：

- 服务正常启动，无 panic
- 任务创建成功，返回包含 task id 的 JSON
- 任务列表返回刚创建的任务

### TC-ASPB-04 0 字节文件跳过

操作步骤：

1. 确认测试目录中存在 0 字节文件：
   ```bash
   find ~/Downloads/TX_MIC001_20260514_170046 -name "*.wav" -empty
   ```
2. 触发任务运行：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/run
   ```
3. 查看服务日志中的 0 字节跳过记录
4. 查看任务详情中的 discovered 数量

预期结果：

- 日志中出现 `skipping 0-byte audio file` 消息
- discovered 数量 = 总文件数 - 0字节文件数
- 0 字节文件不出现在 files 列表中

### TC-ASPB-05 chunk 级别进度实时更新

操作步骤：

1. 任务运行中，查询任务详情：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
2. 检查 processing 状态的文件的 `progress_current` 和 `progress_total` 字段
3. 间隔 10 秒后再次查询，观察 progress_current 是否递增

预期结果：

- processing 文件有 `progress_current` 和 `progress_total` 字段
- `progress_total` 为该文件的 chunk 总数（如 30 分钟文件约 30 个 chunk）
- `progress_current` 随时间递增（如 `[5/30]` → `[10/30]`）
- chunk 进度通过 API 实时可查

### TC-ASPB-06 串行处理验证（同一时间只有 1 个文件在处理）

操作步骤：

1. 任务运行中，多次查询任务详情
2. 统计 status 为 `processing` 的文件数量

预期结果：

- 任何时刻最多 1 个文件处于 `processing` 状态
- 其余文件为 `pending`、`success` 或 `failed`

### TC-ASPB-07 chunk 失败后文件标记 Failed 并继续下一个

操作步骤：

1. 观察任务运行日志
2. 当某个 chunk 失败并重试 3 次后，检查该文件状态
3. 确认下一个文件开始处理

预期结果：

- 日志显示 `retrying ASR chunk after transient failure`（如有失败）
- 重试 3 次后该文件标记为 `failed`，error 字段包含失败原因
- 下一个 pending 文件开始处理，不会因单文件失败中断批量

### TC-ASPB-07A native ASR footprint 超限后直接 bisect

操作步骤：

1. 使用临时数据目录启动 Bifrost，创建指向 `~/Downloads/we` 的 ASR task，model 使用 `Qwen3-ASR-1.7B`。
2. 手动 Run 任务，监控 native `asr` 子进程：
   ```bash
   ps -axo pid,ppid,rss,args | grep qwen3_asr_rs/asr
   vmmap -summary <asr_pid> | grep 'Physical footprint'
   ```
3. 观察服务日志中是否出现 footprint limit 和 bisect 相关日志。
4. 继续查询任务详情，确认任务没有因为单个高风险 chunk 卡死。

预期结果：

- 默认仍按 30 秒 chunk 开始处理，不把全局默认切到 15 秒或更短。
- 当 native `asr` physical footprint 超过模型感知默认阈值（1.7B 默认 18432 MiB；`BIFROST_ASR_MAX_FOOTPRINT_MB` 只能向下收紧，不能越过安全上限）时，该子进程被 kill。
- 日志包含 `asr cli exceeded memory footprint limit` 和 `bisecting without same-size retry`。
- 后端不再对同一个 30 秒 chunk 做 3 次同尺寸重试，而是立即拆分更小子段。
- 系统不会出现 20G+ physical footprint 持续上涨导致卡死。

### TC-ASPB-07B memory-limit hint 复用高风险 chunk

操作步骤：

1. 基于 TC-ASPB-07A 的同一临时数据目录和 `~/Downloads/we` 任务，等待首次运行触发 memory-limit bisect。
2. 查询文件状态，确认 `memory_limit_hints` 非空：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id> | jq '.files[] | select(.memory_limit_hints != null)'
   ```
3. 将该文件状态重置为 pending 或创建同源同 mtime 的测试副本后再次运行任务。
4. 观察服务日志。

预期结果：

- 文件记录持久化 `memory_limit_hints`，包含 `model`、`offset_secs`、`duration_secs`、`preferred_chunk_secs` 和 `trigger_count`。
- 第二次处理匹配 chunk 时，日志先出现 `using remembered ASR memory-limit bisect hint`。
- 后端直接按 `preferred_chunk_secs` 切子窗口，不再先启动同一个完整 30 秒高风险 `asr` 子进程。

### TC-ASPB-07C 托管 asr-server 启动后也受内存 watchdog 保护

操作步骤：

1. 使用临时数据目录启动 Bifrost。
2. 通过 WebUI Start Service 或 API 启动托管 ASR 服务：
   ```bash
   curl -s -X POST 'http://127.0.0.1:8801/_bifrost/api/asr/service/start?model=Qwen3-ASR-1.7B&language=chinese'
   ```
3. 查看 `BIFROST_DATA_DIR/asr/service.json` 中的 `pid`，并确认该 pid 对应 `qwen3_asr_rs/asr-server`。
4. 观察服务日志和 `~/.bifrost/asr/qwen3_asr_rs/bifrost-managed-asr-server.log`。

预期结果：

- 启动命令本身不依赖 qwen3_asr_rs 内部 MLX 限额；Bifrost 在服务健康后注册外层 watchdog。
- `asr-server` 处于独立进程组，Stop Service 或 watchdog 终止时不会遗留子进程。
- 如果可靠 physical footprint 超过模型感知默认阈值（1.7B 默认 18432 MiB），Bifrost kill 当前 `asr-server` 进程组并清理 `service.json`；如果只是 footprint 连续采样不可用或 RSS-only fallback，则只记录 warning，不提前杀掉托管服务。
- 正常情况下服务 `/health` 返回 ready，未触发 watchdog 时不影响 Start/Stop 基本能力。

### TC-ASPB-08 长音频逐 chunk 切片且可中断

操作步骤：

1. 使用包含 1 小时以上录音的目录创建任务并手动 Run。
2. 观察任务运行日志和系统进程：
   ```bash
   ps -axo pid,ppid,command | grep ffmpeg
   ls -lh /var/folders/*/*/* 2>/dev/null | head
   ```
3. 在任务处于 normalize 或 chunk split 阶段时调用 `pause?force=true`。
4. 继续轮询 `ffmpeg` 子进程和任务详情。

预期结果：

- 同一任务不会一次性拉起大量 `ffmpeg`；长音频按当前 chunk 临时切片，识别后删除当前 chunk。
- force-pause 会终止当前 `ffmpeg` normalize/split 或 native `asr` 子进程，不需要等待全量切片完成。
- 当前文件回到 pending，resume 后可以继续处理。

### TC-ASPB-09 批量任务最终结果

操作步骤：

1. 等待整个批量任务完成
2. 查询最终任务状态：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
3. 检查各文件状态分布

预期结果：

- running=False
- 大多数文件为 success 状态
- 成功文件有 text_chars > 0 和 output_text_path 不为空
- 失败文件有 error 描述

### TC-ASPB-10 暂停未运行任务并阻止手动 Run

操作步骤：

1. 使用临时数据目录启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-planb cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
   ```
2. 创建一个绑定空音频目录的目录任务：
   ```bash
   mkdir -p ./.bifrost-test-planb/audio-empty
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks \
     -H 'Content-Type: application/json' \
     -d '{"name":"Pause API Test","audio_dir":"./.bifrost-test-planb/audio-empty","recursive":true,"enabled":true,"schedule":{"kind":"daily","hour":2,"minute":0},"language":"chinese","model":"Qwen3-ASR-1.7B"}'
   ```
3. 调用暂停接口：
   ```bash
   curl -s -X POST 'http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/pause?mode=long_term'
   ```
4. 暂停状态下调用手动 Run：
   ```bash
   curl -s -i -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/run
   ```

预期结果：

- pause 响应中 `paused=true`、`running=false`
- pause 响应中 `pause_mode=long_term`
- 返回的 task 中 `paused=true`、`next_run_at_ms=null`
- paused 状态下手动 Run 返回 HTTP 409
- Run 错误体包含 `paused=true` 和“resume it before starting a run”提示

### TC-ASPB-11 继续未运行且无 pending 文件的任务

操作步骤：

1. 基于 TC-ASPB-10 的 paused task，调用继续接口：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/resume
   ```
2. 查看任务列表：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks
   ```

预期结果：

- resume 响应在请求线程内快速返回 `paused=false`
- 响应 message 说明任务已排入后台恢复处理
- 任务列表中该任务 `paused=false`
- 后台任务发现空目录后快速结束，后续任务列表中 `summary.running=false`
- 因为空目录没有 pending/failed 文件，不会触发模型下载、初始化或 ASR CLI 运行

### TC-ASPB-12 运行中暂停后继续处理

操作步骤：

1. 使用包含多个真实音频文件的目录创建任务并手动 Run：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/run
   ```
2. 任务显示 `summary.running=true` 后，调用 pause：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/pause
   ```
3. 每 5 秒查询任务详情，直到 `summary.running=false`：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
4. 调用 resume：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/resume
   ```
5. 继续查询任务详情，观察 pending/processing/success 状态变化。

预期结果：

- pause 后任务进入 `paused=true`
- 正在运行的任务不会被硬杀；它在当前文件或长音频 chunk 边界后释放运行状态
- 释放后 `summary.running=false`，全局运行锁不再阻止其它任务
- 已经 `success` 的文件不会重跑
- 当前未完成文件保持 pending/processing 可恢复状态，resume 后会继续处理 pending/failed 文件
- WebUI 列表和详情页分别显示 Pausing/Paused/Running/Ready 状态，Running 时有 Pause 按钮，Paused 时有 Resume 按钮

### TC-ASPB-13 运行中强制暂停立即释放 native ASR 子进程

操作步骤：

1. 使用包含真实长音频的目录创建任务并手动 Run。
2. 任务显示 `summary.running=true` 且能看到 native `asr` 子进程后，调用：
   ```bash
   curl -s -X POST 'http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/pause?force=true'
   ```
3. 立即轮询 native 子进程和任务详情：
   ```bash
   ps -axo pid,ppid,args | grep qwen3_asr_rs/asr
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
4. 调用 resume 并确认任务可继续处理 pending/failed 文件。

预期结果：

- pause 响应包含 `paused=true`、`force=true`，message 明确说明会 abort 当前 ASR process。
- 当前 native `asr` 或 `ffmpeg` 子进程被 prompt kill，日志包含 `asr cli aborted by task control` 或任务返回 paused。
- 当前文件回到 pending 状态，`summary.running` 在后台清理后变为 false，资源不需要等到完整 chunk 自然结束。
- resume 清除 force-pause 状态并重新启动后台运行。

### TC-ASPB-14 30 分钟真实音频按 30 秒窗口完成性能基准

操作步骤：

1. 构建当前分支二进制：
   ```bash
   cargo build --bin bifrost
   ```
2. 使用真实 30 分钟录音执行 CLI 转写：
   ```bash
   /usr/bin/time -p target/debug/bifrost ai asr stream-file \
     ~/Downloads/we/TX01_MIC007_20260514_183241_orig.wav \
     --model Qwen3-ASR-1.7B \
     --language chinese \
     >/tmp/bifrost-asr-cli-bench.jsonl \
     2>/tmp/bifrost-asr-cli-bench.err
   ```
3. 检查 stderr 中的 chunk 计划、每 chunk 耗时、总耗时和 RTF。

预期结果：

- stderr 显示 `Split into ... chunks (30s each, 2s overlap)`，不得回退到 60 秒或 63 秒窗口。
- 30 分钟左右音频在当前机器上应在 5 分钟内完成；如果超过，需要先检查是否有 `vmmap -summary` 首采样、服务端 whole-file 上传或异常 memory-limit bisect。
- 结束后不得遗留 `target/debug/bifrost ai asr stream-file` 或 `qwen3_asr_rs/asr` 子进程。

### TC-ASPB-15 任务详情表展示文件开始时间和执行耗时

操作步骤：

1. 使用正在运行或已完成的真实 ASR 目录任务打开 WebUI：
   ```text
   http://127.0.0.1:8801/_bifrost/?aiTool=asr&asrTask=<task_id>
   ```
2. 在任务详情页查看文件表格列头。
3. 查询同一个任务详情 API：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
4. 对比 `processing`、`success`、`partial_success` 或 `failed` 文件的 `started_at_ms`、`finished_at_ms` 与 UI 展示。

预期结果：

- 文件表格包含 `Started`、`Elapsed`、`Finished` 三列，原媒体时长列显示为 `Audio`，避免把音频长度误认为执行耗时。
- `processing` 文件的 `Started` 不为空，`Elapsed` 随页面时间刷新增长，chunk 进度仍正常展示。
- `success`、`partial_success` 或 `failed` 文件保留开始时间，`Elapsed` 等于 `finished_at_ms - started_at_ms`，不会因为后端重建 FileRecord 只剩结束时间。
- `pending` 文件若没有实际进入过处理，`Started`、`Elapsed` 和 `Finished` 均可为空。
- 表格仍通过内部横向滚动承载长路径，不撑出 ASR 主页面。

### TC-ASPB-16 服务重启后孤儿 processing 文件自动恢复 pending

操作步骤：

1. 使用真实长音频目录任务手动 Run，并等待任务详情出现 `status=processing` 且 `progress_current > 0` 的文件。
2. 模拟 Bifrost daemon 重启或进程崩溃后恢复：停止旧 Bifrost 进程，再用同一个 `BIFROST_DATA_DIR` 重新启动服务。
3. 访问任务详情 API：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
4. 点击 Run 或 Resume，让任务继续处理。

预期结果：

- 服务启动后如果旧 `run.lock` 不属于仍存活的 Bifrost 进程，旧 `processing` 文件会恢复为 `pending`，旧 `started_at_ms`、旧进度和旧 transient error 被清空。
- 如果 `run.lock` 指向仍存活的其它 Bifrost 进程，则不会抢占或重置该任务。
- Run/Resume 后该文件会重新进入 `processing`，获得新的 `started_at_ms`，chunk 进度从当前新运行重新计算。
- WebUI 不再长期展示 `summary.running=false` 但文件仍是 `processing` 的假运行状态。

### TC-ASPB-16A 任务详情文件列表按 Recorded 倒排并支持状态筛选

操作步骤：

1. 使用一个包含 `success` 和 `pending` 文件的真实目录任务打开 WebUI：
   ```text
   http://127.0.0.1:8801/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>
   ```
2. 查询同一个任务详情 API：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>
   ```
3. 检查 API `summary.pending`、`files.length` 和文件表第一页。
4. 在文件表顶部依次切换 `Processing`、`Pending`、`Completed`、`Failed`、`All` 筛选。

预期结果：

- WebUI 文件表始终按 `Recorded` 时间倒序展示，最新录音/文件排在最前面。
- 状态筛选只缩小列表集合，不改变 `Recorded` 倒排语义。
- WebUI 文件表顶部显示 `Processing`、`Pending`、`Completed`、`Failed`、`All` 五个筛选项及各自数量。
- 切换筛选后，表格只展示对应状态文件，`Completed` 只包含 `success` 文件，`Failed` 包含 `failed` 和 `partial_success` 文件，`All` 展示全部文件。
- 缺少 `Recorded` 时间的旧文件使用 source modified time 兜底，并保持稳定排序。
- 表格总数、入口百分比和 API `summary.processed/pending/failed` 一致。

### TC-ASPB-17 目录任务 runtime_strategy 默认值和 API 可见性

操作步骤：

1. 使用临时数据目录启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-planb cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
   ```
2. 创建一个未显式传 `runtime_strategy` 的目录任务：
   ```bash
   mkdir -p ./.bifrost-test-planb/audio-empty
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks \
     -H 'Content-Type: application/json' \
     -d '{"name":"Runtime Default Test","audio_dir":"./.bifrost-test-planb/audio-empty","recursive":true,"enabled":false,"schedule":{"kind":"daily","hour":2,"minute":0},"language":"chinese","model":"Qwen3-ASR-1.7B"}'
   ```
3. 查看创建响应、任务列表和任务详情。

预期结果：

- 创建响应、列表和详情都包含 `"runtime_strategy":"reuse_per_file"`。
- 旧任务 JSON 缺少 `runtime_strategy` 时反序列化为当前默认 `reuse_per_file`，新旧任务都进入最快的文件级复用路径。
- WebUI Directory Tasks 创建表单默认选中 `Reuse / file`，任务列表和详情页展示 Runtime 字段。

### TC-ASPB-18 reuse_per_file 每个文件结束后重启模型服务

操作步骤：

1. 使用包含至少 2 个短音频文件的目录创建任务，并显式传 `runtime_strategy=reuse_per_file`：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/asr/tasks \
     -H 'Content-Type: application/json' \
     -d '{"name":"Reuse Per File Test","audio_dir":"/path/to/audio-dir","recursive":true,"enabled":false,"schedule":{"kind":"daily","hour":2,"minute":0},"language":"chinese","model":"Qwen3-ASR-1.7B","runtime_strategy":"reuse_per_file"}'
   ```
2. 手动 Run 任务并观察 Bifrost 日志。
3. 任务完成后查询任务详情和 `BIFROST_DATA_DIR/asr/tasks/<task_id>/files.json`。

预期结果：

- 日志每个文件出现 `ASR runtime strategy acquired managed server`，`scope=file`，文件结束后出现 `stopping ASR managed server after file-scoped runtime strategy`。
- 文件内 chunk metric 的 `runner` 为 `reuse_server`，`server_url` 非空，且 `runtime_strategy` 为 `reuse_per_file`。
- 任务结束后如果该 server 是本次任务启动的，`service.json` 被清理，不长期占用 MLX/Metal 资源；如果任务复用了进入任务前已经存在的 managed service，则不会擅自停止该既有服务。

### TC-ASPB-19 auto 策略 fallback 状态和日志可追踪

操作步骤：

1. 在模型服务不可启动或故意使用无效 server 资源的临时环境中创建 `runtime_strategy=auto` 任务。
2. 手动 Run 任务，观察 Bifrost 日志。
3. 查询任务详情、`files.json` 和单文件 metadata JSON。

预期结果：

- server 启动失败时日志出现 `ASR auto strategy falling back to fork_per_chunk during startup`，任务不会直接失败。
- 每个文件记录包含 `runtime_strategy=auto` 和非空 `fallback_reason`。
- 后续 chunk metric 的 `runner` 为 `fork_per_chunk`，`fallback_reason` 指向 server startup 或 chunk failure 原因。
- 如果 server 初期可用但后续 chunk RTF 相对前三个稳定样本恶化超过阈值，日志出现 `ASR auto strategy switching remaining chunks to fork_per_chunk`，后续 chunk metric 可看出切换点。

### TC-ASPB-20 compare 策略持久化 fork/server 对照证据

操作步骤：

1. 使用真实短音频创建 `runtime_strategy=compare` 任务。
2. 手动 Run 任务并观察日志。
3. 查询 `files.json` 和 metadata JSON 中的 `chunk_metrics`。

预期结果：

- 每个有效 chunk 至少记录两个 metric：canonical `fork_per_chunk` 和 shadow `reuse_server`。
- shadow metric 的 `fallback_reason` 为 `compare_shadow`，`server_url` 非空。
- 日志包含 `ASR compare strategy completed paired chunk`，记录 fork/server RTF 和状态。
- 如果两边文本不同，日志包含 `ASR compare strategy produced different text hashes`，metadata 中可以通过 `text_sha1` 对照具体 chunk。
- 最终 transcript 采用 fork canonical 输出，不被 shadow server 输出覆盖。

### TC-ASPB-21 reuse_per_file 服务死亡后当前 chunk 降级并自动重启 server

操作步骤：

1. 启动 Bifrost，创建或复用一个 `runtime_strategy=reuse_per_file` 的真实目录任务。
2. 在任务处理长音频时触发或模拟 file-scoped `asr-server` 死亡：例如降低 `BIFROST_ASR_MAX_FOOTPRINT_MB` 后运行 1.7B 长音频，或在 chunk 运行中终止当前 `asr-server` 进程。
3. 继续观察任务日志、任务详情、`BIFROST_DATA_DIR/asr/tasks/<task_id>/files.json` 和单文件 metadata JSON，重点确认失败 chunk 的 fork 处理完成后才出现下一次 managed server restart。

预期结果：

- 日志先记录失败的 `reuse_server` chunk metric，包含原 `server_url` 和连接/服务错误。
- 同一个 chunk 会立即以 `fork_per_chunk` 重试，并记录 `fallback_reason`：`reuse_per_file strategy transport failure; retrying current chunk via fork_per_chunk and scheduling managed ASR server restart for later chunks: ...` 或 `reuse_per_file strategy server failure; ...`。
- 同一文件后续 server-eligible chunk 不再继续请求死掉的 `server_url`；后端先停止 stale managed service state，再同步启动新的 `asr-server` 和新端口，成功后后续 chunk 回到 `reuse_server` metric。
- 如果重启失败，当前 chunk 只降级一次为 `fork_per_chunk`，`restart_required` 保留，下一次 server-eligible chunk 再重试重启。
- 日志顺序必须体现串行资源占用：当前 chunk 的 fork fallback 完成后，才出现 `restarting managed ASR server before next chunk after prior server failure` / `managed ASR server restarted for later chunk`；不得在 fork/native chunk 运行期间并发启动 server。
- 任务不因为单次 `asr-server` watchdog kill 产生连续大量 connect-failed chunks；如果 fork/bisect 仍失败，才以真实 chunk 错误记录 `failed_chunks`。
- 任务详情和 metadata 保留三类证据：server 失败 metric、当前 chunk 的 fork 恢复 metric、后续新 `server_url` 的 server 成功 metric，便于判断该策略是否适合当前机器和音频。

### TC-ASPB-21B reuse_server 跨文件复用失败后重启 task-scoped server

操作步骤：

1. 准备一个包含至少 2 个音频文件的 ASR Directory Task，并显式设置 `runtime_strategy=reuse_server`。
2. 在第 1 个文件的第 1 个 chunk 触发或模拟 task-scoped `asr-server` 传输失败，例如让 managed server URL 指向已拒绝连接的端口，或在第 1 个 chunk 请求前终止该 task-scoped `asr-server`。
3. 继续观察第 1 个文件后续 chunk 和第 2 个文件的 chunk metric、`fallback_reason` 与 `files.json`。
4. 可用单元测试 `cargo test -p bifrost-admin reuse_server_fallback_schedules_restart_for_later_chunks --lib` 覆盖无需真实模型的回归路径。

预期结果：

- 第 1 个失败 chunk 先记录 `runner=reuse_server`、`status=error`、原始 `server_url` 和连接错误；随后同一 chunk 立即以 `runner=fork_per_chunk` 重试。
- task-scoped recovery 状态被保存在同一次 run 的共享 `ServerRunnerState` 中，`fallback_reason` 为 `reuse_server strategy transport failure; retrying current chunk via fork_per_chunk and scheduling managed ASR server restart for later chunks: ...` 或 `reuse_server strategy server failure; ...`。
- 后续 chunk 和后续文件不再重新请求同一个死掉的 task-scoped `server_url`；在没有 fork/native chunk 正在运行时，后端同步重启 task-scoped managed server 并使用新的 `server_url`。
- 重启成功后，后续 chunk metric 回到 `runner=reuse_server`，`server_url` 为新端口且该成功 metric 不带旧 chunk 的 `fallback_reason`；重启失败时只当前 chunk 走 `fork_per_chunk`，下一次 server-eligible chunk 继续尝试重启。
- `files.json` 和任务详情中的 `chunk_metrics` 足以区分首次 server 失败 metric 与后续 fork fallback metric，不会把整批慢处理误归因为 memory-limit exceedance。

### TC-ASPB-21C watchdog 不因 physical footprint unavailable 误杀 asr-server

操作步骤：

1. 执行单元测试 `cargo test -p bifrost-admin service_watchdog_kills_only_on_reliable_physical_footprint_over_limit --lib`。
2. 准备或模拟一个 `read_process_footprint_bytes` 返回 RSS fallback、`reliable=false` 且 RSS 高于模型阈值的场景。
3. 观察 watchdog 决策与日志文案。
4. 再准备一个可靠 physical footprint 样本高于模型阈值的场景作为对照。

预期结果：

- 当 `reliable=false` 时，watchdog 只记录 `physical footprint unavailable` / RSS-only advisory warning，不调用 kill，不清理 managed service state。
- 当采样过程报错但进程仍存活时，连续失败只记录 warning，不杀掉 server。
- 只有 `reliable=true` 且 physical footprint 明确超过模型阈值时，watchdog 才终止 `asr-server`。
- 这避免一次采样不可用导致服务死亡，并减少后续 chunk 由于连接 dead server 而集体降级的概率。

### TC-ASPB-22 failed chunks 重试成功后刷新所有派生产物

操作步骤：

1. 准备一个包含 `partial_success` 文件的目录任务，文件中至少有一个 placeholder：`[chunk N failed: ...]`。
2. 调用：
   ```bash
   curl -s -X POST "http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/files/<file_key>/retry-chunks"
   ```
3. 检查响应、该文件 `.txt`、`.timeline.json`、metadata JSON、`files.json` 和 `/api/asr/tasks/<task_id>/daily`。

预期结果：

- 响应包含 `recovered > 0`、`still_failed=0` 或剩余真实失败 chunk，并返回 `daily_documents_refreshed`。
- `.txt` 中成功 chunk 的 placeholder 被恢复文本替换。
- `.timeline.json` 追加恢复 segment，按 `audio_start_ms` 排序并重新编号。
- metadata JSON 包含 `retry_updated_at_ms`、`retry_recovered_chunks` 和 `retry_still_failed_chunks`。
- `files.json` 中对应文件的 `failed_chunks` 被更新；全部恢复时状态变为 `success`，`text_chars` 重新计数。
- Daily Docs 中不再展示已恢复 chunk 的 placeholder。

### TC-ASPB-23 WebUI 批量排队重试所有 failed chunks

操作步骤：

1. 准备或定位一个目录任务，任务详情中至少有 2 个 `partial_success` 文件，且 `summary.failed_chunk_count > 0`。
2. 打开 WebUI：
   ```text
   http://127.0.0.1:<port>/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>
   ```
3. 在任务详情页确认顶部显示 `Partial Success` 与 failed chunks 数量。
4. 点击 `Retry all failed chunks`，在确认弹窗中点击 OK。
5. 观察任务详情页的 `Bulk chunk retry` 状态区和 Bifrost 日志。
6. 等待批量重试完成后刷新任务详情和 Daily Docs。

预期结果：

- WebUI 发起 `POST /api/asr/tasks/<task_id>/retry-failed-chunks`，不是逐个文件在浏览器侧并发调用。
- 后端返回 `202` 和 `bulk_retry` 状态；按钮进入 `Retrying...`，状态区显示 `queued` 或 `running`。
- 后端日志包含批量 retry queued、started、每个文件 started/finished、completed 事件。
- 后端只选择 `failed_chunks` 非空的文件；每个文件内部只重试该文件的失败 chunks，不重跑该文件所有 chunks。
- 文件按队列串行处理，同一时刻只有一个文件的 failed chunks retry 占用 ASR 全局处理锁。
- 状态区展示 `processed_files/queued_files`、`recovered_chunks/total_failed_chunks`、当前文件路径和最后一个文件结果。
- 完成后如果全部恢复，任务 `partial_success=0`、`failed_chunk_count=0`；若仍有失败，状态区和文件表保留真实 still-failed 数量。
- 成功恢复的 chunks 同步刷新 transcript、timeline、metadata、`files.json` 和 Daily Docs。

### TC-ASPB-24 ASR jobs 模块拆分后任务 API 行为不变

操作步骤：

1. 使用临时数据目录和临时端口启动最新源码编译出的 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start -p <free_port> --unsafe-ssl --no-system-proxy
   ```
2. 创建一个空音频目录，调用任务创建 API：
   ```bash
   curl -s -X POST http://127.0.0.1:<free_port>/_bifrost/api/asr/tasks \
     -H 'Content-Type: application/json' \
     -d '{"name":"ASR Jobs Split Smoke","audio_dir":"<temp_audio_dir>","recursive":true,"enabled":false,"schedule":{"kind":"daily","hour":2,"minute":0},"language":"chinese","model":"Qwen3-ASR-1.7B"}'
   ```
3. 调用任务列表和任务详情 API：
   ```bash
   curl -s http://127.0.0.1:<free_port>/_bifrost/api/asr/tasks
   curl -s http://127.0.0.1:<free_port>/_bifrost/api/asr/tasks/<task_id>
   ```
4. 调用任务级 failed chunks 批量重试 API：
   ```bash
   curl -s -X POST http://127.0.0.1:<free_port>/_bifrost/api/asr/tasks/<task_id>/retry-failed-chunks
   ```

预期结果：

- 服务使用最新源码启动成功，且启动命令包含 `--no-system-proxy`。
- 创建任务、任务列表、任务详情 API 均返回 200。
- 新建任务默认 `runtime_strategy` 为 `reuse_per_file`，`summary.discovered=0`、`summary.failed_chunk_count=0`、`summary.running=false`。
- 批量 failed chunks 重试 API 在无失败 chunk 时返回 200，`message=No failed chunks to retry`，`bulk_retry.status=completed`，`queued_files=0`。
- 该用例证明 `asr_jobs.rs` 拆分后对外 API 路径、默认策略、summary 计算和 bulk retry no-op 行为未变。

### TC-ASPB-25 Daily Agent Runner 方案文档验收

操作步骤：

1. 检查技术方案文档存在：
   ```bash
   test -f design/asr-daily-agent-runner.md
   ```
2. 检查方案覆盖 ASR 定时任务配置页的 Daily Agent Runner 配置：
   ```bash
   rg -n "ASR 创建/编辑页面|Daily Agent Runner|单一 Runner 下拉|AI -> Agent -> Runners|Instructions / AGENTS.md" design/asr-daily-agent-runner.md
   ```
3. 检查方案覆盖内置默认指导手册和用户可编辑 `AGENTS.md`：
   ```bash
   rg -n "内置 AGENTS.md 模板|assets/asr_daily_agents_default.md|PUT /api/asr/tasks/\\{task_id\\}/daily-agent/agents|instructions_source" design/asr-daily-agent-runner.md
   ```
4. 检查方案覆盖 daily workspace 初始化、`report/`、Git 初始化和 Git 不可用降级：
   ```bash
   rg -n "Daily Workspace 初始化|daily/report|git init|Git 是增强能力|git unavailable" design/asr-daily-agent-runner.md
   ```
5. 检查方案覆盖 Runner 执行逻辑、手动 `Run now`、ASR 完成后触发和测试计划：
   ```bash
   rg -n "Runner 执行逻辑|Run now|ASR 完成后触发|maybe_enqueue_daily_agent_after_asr_run|测试计划" design/asr-daily-agent-runner.md
   ```
6. 检查方案覆盖可选 IM 通道绑定和发送策略：
   ```bash
   rg -n "IM delivery|IM Channel|im_delivery.channel|Send policy|IM 发送逻辑|未绑定 IM|绑定 IM" design/asr-daily-agent-runner.md
   ```
7. 检查方案覆盖 ChatGPT Web 无法读取 `AGENTS.md`、但需要保持同一个长期 conversation 的特殊消息组织：
   ```bash
   rg -n "Runner 消息组织差异|ChatGPT Web 不能读本地|每个 ASR 任务一个固定 conversation|第一条消息|第二条消息|asr-daily:<task_id>|Reset ChatGPT Web conversation" design/asr-daily-agent-runner.md
   ```
8. 检查方案覆盖已处理文档记录、变更整理和不同 Runner 的投递策略：
   ```bash
   rg -n "DailyAgentChangePlanner|daily_agent_processed.json|已处理文档记录|unchanged|appended|rewritten|IncrementalPayload|FileList|增量文本|文件清单" design/asr-daily-agent-runner.md
   ```

预期结果：

- 方案文档存在且包含以上所有关键章节。
- 方案明确 Daily Agent Runner 配置入口在 ASR 任务创建/编辑页面和任务详情页，而不是只放在 Settings。
- 方案明确 Daily Agent Runner 放在 ASR 定时任务内部，跟随父级 ASR task schedule，不维护独立 scheduler。
- 方案明确每次 ASR task run 必须先完成音频处理、failed chunk retry 合并、daily markdown 刷新和 ASR 状态持久化，然后才排队启动 Daily Agent Runner。
- 方案明确 ASR run 仍在 processing 时不会启动 Daily Agent；如果本轮没有 daily markdown 新增或变更，Daily Agent 只记录 skipped，不调用外部 Runner。
- 方案明确通过 `DailyAgentChangePlanner` 和 `daily_agent_processed.json` 记录之前处理过的 `YYYY-MM-DD.md`，相同 source hash 的 unchanged 文档不会再次发进 Runner loop。
- 方案明确 daily 文件 append-only 增长时只投递新增 tail；hash 变化但不是 append 时投递 diff/changed ranges；Runner 成功后才更新 processed state。
- 方案明确 ChatGPT Web 不能读本地文件，所以只接收 change plan 中新增/变更的增量文本或 diff，不重复发送 unchanged 文档。
- 方案明确 Bifrost Agent / Codex 可以读工作目录，所以只给它们更新文件清单、变化类型、hash 和目标 report 路径，由它们自行检查文件并刷新任务。
- 方案明确手动 `Run now` 是补跑/调试入口，不替代默认的 ASR completion hook。
- 方案明确 WebUI 和 API 都只使用单一 `runner` 字段，选项包含 `Bifrost Agent` 与 AI -> Agent -> Runners 中已配置的 External CLI Runner，并说明 trigger policy、timeout、session key 的保存方式。
- 方案明确默认指导手册内置到系统，初始化为 `daily/AGENTS.md`，并允许用户在 WebUI 修改。
- 方案明确创建任务时初始化 `daily/`、`daily/report/`、`daily/AGENTS.md`，并 best-effort `git init`。
- 方案明确 Git 不存在或失败时跳过并记录 warning，不阻塞 ASR 任务创建或 Agent run。
- 方案明确 Daily Agent Runner 可以不绑定 IM 通道；未绑定时只写入 report 和 Git 历史。
- 方案明确 WebUI 和 API 都只使用单一 `im_delivery.channel` 字段绑定可发送通道，成功处理后的结论按 send policy 通过 IM Gateway 发送出去。
- 方案明确 IM 发送失败只记录 `last_send_error` 和 run detail，不回滚 report，不影响 ASR 转写状态。
- 方案明确 Bifrost Agent/Codex 可以直接读取 daily 目录里的 `AGENTS.md`，不需要每次把全文塞进消息。
- 方案明确 ChatGPT Web 不能读取本地 `AGENTS.md`，但默认每个 ASR 任务保持同一个长期 conversation。
- 方案明确 ChatGPT Web 第一次处理该任务时第一条消息注入 `AGENTS.md` 全文，第二条消息发送 change plan 中需要处理的新增/变更内容；后续每天只发送新增 tail、diff 或 changed ranges。
- 方案明确只有用户手动 reset、切换 session key 或显式要求新对话时才切换 conversation。

### TC-ASPB-30 Daily Agent Runner 实现闭环回归

操作步骤：

1. 检查 `daily_agent.rs` 不超过单文件 1500 行限制，且 Daily Agent API/IM 已拆分：
   ```bash
   wc -l crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_im.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_api.rs
   ```
2. 检查 Bifrost Agent 分支不再返回未集成错误，WebUI 不再有 `any` lint 问题：
   ```bash
   ! rg -n "bifrost_agent runner not yet|catch \\(.*: any\\)|no-explicit-any" crates/bifrost-admin/src/handlers/asr_jobs web/src/pages/ASR/components/DailyAgentTab.tsx
   ```
3. 执行 Daily Agent targeted 单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent --lib
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin bifrost_agent_runner --lib
   ```
4. 执行后端编译、clippy、前端 lint/build：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --lib
   SKIP_FRONTEND_BUILD=1 cargo clippy -p bifrost-admin --lib -- -D warnings
   pnpm --dir web exec eslint src/pages/ASR/components/DailyAgentTab.tsx src/api/asr.ts
   pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts
   pnpm --dir web run build
   ```
5. 复核任务创建、failed chunk retry、IM 配置、ChatGPT Web/Codex 投递策略相关 diff：
   ```bash
   git diff -- crates/bifrost-admin/src/handlers/asr_jobs/api.rs crates/bifrost-admin/src/handlers/asr_jobs/retry.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_im.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_api.rs web/src/pages/ASR/components/DailyAgentTab.tsx web/src/api/asr.ts
   ```
6. 使用默认数据目录启动真实服务，验证 IM 通道配置和 Daily Agent 发送链路：
   ```bash
   cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy -y
   curl -sS http://127.0.0.1:9900/_bifrost/api/im-gateway/providers/feishu/status
   curl -sS -X PUT http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent \
     -H 'content-type: application/json' \
     --data '{"im_delivery":{"enabled":true,"channel":""}}'
   curl -sS -X PUT http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent \
     -H 'content-type: application/json' \
     --data '{"im_delivery":{"enabled":true,"channel":"owner:feishu","mode":"summary","send_policy":"always"}}'
   curl -sS -X POST http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent/send
   ```
7. 使用默认数据目录创建临时 ASR 任务，验证初始化和 runner 配置校验后删除临时任务：
   ```bash
   curl -sS -X POST http://127.0.0.1:9900/_bifrost/api/asr/tasks \
     -H 'content-type: application/json' \
     --data '{"name":"Daily Agent Regression","audio_dir":"<existing-empty-dir>","enabled":false,"recursive":true,"schedule":{"kind":"daily","hour":3,"minute":17}}'
   test -f ~/.bifrost/asr/data/text/<task_id>/daily/AGENTS.md
   test -d ~/.bifrost/asr/data/text/<task_id>/daily/report
   test -d ~/.bifrost/asr/data/text/<task_id>/daily/.git
   curl -sS -X PUT http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent \
     -H 'content-type: application/json' \
     --data '{"enabled":true,"runner":""}'
   curl -sS -X PUT http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent \
     -H 'content-type: application/json' \
     --data '{"enabled":true,"runner":"bifrost_agent","trigger_policy":"manual_only"}'
   curl -sS -X DELETE http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>
   ```

预期结果：

- `daily_agent.rs` 小于 1500 行，API handler 和 IM 发送逻辑拆分到独立文件。
- `Bifrost Agent` runner 可进入内置 agent 执行路径，不再返回 `not yet fully integrated`。
- External CLI `codex` / Bifrost Agent 只接收文件清单、变化类型、hash 和目标 report；ChatGPT Web 首轮注入 `AGENTS.md`，后续只发送新增/变更内容。
- Runner 成功返回但未生成 report 时不会写入 `daily_agent_processed.json`，下次不会被误判为 unchanged。
- failed chunk retry 刷新 daily markdown 后会排队 Daily Agent。
- 创建 ASR 任务时初始化 `daily/`、`daily/report/`、`daily/AGENTS.md`，并 best-effort `git init`。
- IM 绑定只支持单字段 `im_delivery.channel`；发送 owner 时使用 `owner:<provider_id>`，发送配置目标时使用 `target:<target_id>`。
- 默认数据目录真实启动后，`feishu` provider 为 connected；空 channel 返回 400；合法 `channel=owner:feishu` 配置返回 200；Daily Agent `/send` 通过 `/_bifrost/api/im-gateway/messages/send` 实际发送成功。
- 默认数据目录中新建任务会立即初始化 `daily/AGENTS.md`、`daily/report/` 和 `.git`；空 runner 返回 400，`runner=bifrost_agent` 返回 200。
- WebUI Daily Agent 配置页无 `no-explicit-any` lint 错误；配置区只展示一个 Runner 下拉，不展示 Runner Type / Runner ID 两个字段；下拉包含 `Bifrost Agent` 和 Runners 中配置的 `codex` / `web` 等 runner id；选择自定义 runner 后保存 payload 为 `runner=<id>`；IM Delivery 只展示一个 Channel 下拉，不展示 Provider ID / Target ID 两个输入；下拉包含 Provider Owner 和 IM Targets，选择目标通道后保存 payload 为 `im_delivery.channel=target:<target_id>`。

### TC-ASPB-33 Daily Agent Instructions 自适应高度回归

操作步骤：

1. 在 mock WebUI 测试中让 `GET /daily-agent/agents` 返回超过 30 行的 `AGENTS.md` 内容。
2. 打开 `/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>`，进入 `Daily Agent` tab。
3. 检查 `Agent Instructions (AGENTS.md)` 编辑框展示完整长内容：
   ```bash
   pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts
   ```
4. 检查编辑框 `clientHeight` 覆盖 `scrollHeight`，且 `overflow-y` 为 `hidden`。

预期结果：

- `Agent Instructions (AGENTS.md)` 编辑框不再使用固定 `rows` 高度。
- 长 `AGENTS.md` 内容会撑高编辑框，页面外层滚动承接长内容。
- 编辑框内部不出现独立滚动条，用户不需要在输入框内部滚动阅读或编辑。

### TC-ASPB-34 默认目录多文件 ChatGPT Web Daily Agent 与 FullReport IM 分片回归

操作步骤：

1. 使用默认数据目录重启源码服务：
   ```bash
   BIFROST_DATA_DIR=/Users/eden/.bifrost cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy
   ```
2. 查询默认任务：
   ```bash
   curl -sS 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb' | jq '{id,name,daily_agent}'
   ```
3. 确认 `daily/` 下存在多个源文件：
   ```bash
   find /Users/eden/.bifrost/asr/data/text/76612de33e9740bc92440ce64a98a4cb/daily -maxdepth 1 -name '*.md' -print | sort
   ```
4. 强制运行默认任务：
   ```bash
   curl -sS -X POST 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/run?force=true'
   ```
5. 轮询任务详情，直到 `daily_agent.last_status` 变为 `success` 或 `failed`。
6. 检查本轮 ChatGPT Web run artifacts：
   ```bash
   find /Users/eden/.bifrost/im_gateway/runs -maxdepth 1 -type d -name '<run-prefix>*' -print | sort
   ```
7. 检查 `daily/report/*-report.md` 与 IM provider message log。

预期结果：

- 默认任务配置为 `runner=web`，IM delivery 为 `channel=owner:acc`、`mode=full_report`。
- 强制运行按 daily 文件日期升序逐个处理；每个 ChatGPT Web prompt 只包含一个 `YYYY-MM-DD.md`，不会把多个大文件合成一个巨型 prompt。
- 每个 ChatGPT Web run 的 `result.json.status` 为 `succeeded`，最终生成对应 `YYYY-MM-DD-report.md`。
- `full_report` 发送失败时不再降级成 `ASR Daily Agent 完成报告整理` 摘要；超长报告按 `ASR Daily Agent Report X/N` 分片发送，失败时 `last_send_error` 指向具体分片。
- 如果 Weixin provider 返回 `ret=-2`，任务本身仍可为 `success`，但 `daily_agent.im_delivery.last_send_error` 必须保留真实 IM 失败原因，message log 预览必须是原文报告分片而不是摘要。

### TC-ASPB-35 Daily Agent Processed Documents report 全屏 Markdown 详情

操作步骤：

1. 使用默认数据目录源码服务打开真实页面：
   ```bash
   BIFROST_DATA_DIR=/Users/eden/.bifrost cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy
   ```
2. 在浏览器中打开：
   ```text
   http://localhost:9900/_bifrost/ai?aiSection=tools-asr&asrTask=76612de33e9740bc92440ce64a98a4cb
   ```
3. 进入 `Daily Agent` tab，确认 `Processed Documents` 表格中每一行 `Report` 列的 `YYYY-MM-DD-report.md` 是可点击入口。
4. 检查 URL 包含 `asrTaskTab=daily-agent`；刷新浏览器页面后，确认仍然停留在 `Daily Agent` tab，不需要重新点击 tab。
5. 点击任意一个 report，例如 `2026-05-14-report.md`。
6. 检查页面切换为全屏 report 详情，而不是在表格内展开或弹小浮层。
7. 检查详情页顶部展示返回按钮、文件名、任务名、日期、路径、大小、修改时间、处理时间和 Runner。
8. 检查正文通过 Markdown 渲染器展示：标题渲染为标题样式，列表/表格/代码块按 Markdown 语义排版，不是原始纯文本 `<pre>`。
9. 刷新浏览器页面，确认 URL 仍包含 `asrDailyReport=2026-05-14` 且页面直接恢复全屏 report 详情与 Markdown 正文。
10. 点击返回按钮，确认回到 ASR 任务详情页、URL 中移除 `asrDailyReport`，并仍保留 `asrTaskTab=daily-agent`。
11. 直接访问非法日期 API：
   ```bash
   curl -i 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/reports/../secret'
   ```

预期结果：

- `Report` 列中所有已生成 report 的文件名均为可点击按钮。
- 点击任意 report 后进入 `data-testid=asr-daily-agent-report-page` 的全屏详情页。
- 正文位于 `data-testid=asr-daily-agent-report-content`，并由 Markdown 渲染器渲染为结构化 HTML。
- Daily Agent tab 和 report 全屏详情都通过 URL 参数保持状态，刷新页面后仍恢复到同一视图。
- 详情页元信息与对应 report 文件一致，内容包含实际 report Markdown。
- 返回按钮回到任务详情页，不影响任务文件详情和 Daily Docs 详情 URL。
- 非法日期或路径穿越不会读取任意文件，返回 400/404。

### TC-ASPB-36 Directory Task Runtime 选项说明

操作步骤：

1. 启动 WebUI 或使用 Playwright mock 页面打开 `/_bifrost/ai?aiSection=tools-asr`。
2. 点击 Directory Tasks 右上角 `New`。
3. 在 `New Directory Task` 弹窗中展开 `Runtime` 下拉。
4. 逐项查看 `Reuse / file`、`Fork / chunk`、`Reuse server`、`Auto fallback`、`Compare`。
5. 关闭下拉后继续填写 `Name=Recordings`、`Audio Directory=/tmp/asr-audio` 并创建任务。

预期结果：

- Runtime 下拉中每个选项名称下面都展示明确说明。
- `Reuse / file` 说明这是多数离线任务的默认策略，并描述文件内复用、文件边界释放。
- `Fork / chunk` 说明这是最隔离策略、每个 chunk 新建 ASR 进程，适合稳定性排障但更慢。
- `Reuse server` 说明整个任务 run 复用一个 ASR server，性能可能更好但会跨文件携带内存或 server 状态风险。
- `Auto fallback` 说明先尝试 server 复用，遇到 server 错误或明显性能退化后自动切回隔离 chunk 处理。
- `Compare` 说明这是诊断模式，会同时运行两条路径并记录性能差异，最终保留隔离路径输出。
- 选中值仍以短标题显示，不把表单输入框撑高。
- 创建任务提交的 `runtime_strategy` 默认仍为 `reuse_per_file`，不破坏原有任务创建流程。

### TC-ASPB-37 服务重启后中断 ASR run 自动恢复且不假 Running

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 预置一个目录任务，任务目录下包含 stale `run.lock`，`files.json` 中至少一个文件为 `status=processing`，且 `started_at_ms/progress_current/progress_total/error` 均有旧值。
2. 启动最新 `target/debug/bifrost start -p <port> --unsafe-ssl --skip-cert-check --no-system-proxy`。
3. 访问 `/_bifrost/api/asr/tasks/<task_id>` 触发 ASR scheduler startup。
4. 对 paused 任务验证不会自动恢复运行；对 enabled 且未 paused 的中断任务，用单元测试或真实模型环境验证会被重新入队。

预期结果：

- stale `run.lock` 如果不属于仍存活的 Bifrost 进程，会在启动恢复阶段被删除。
- orphan `processing` 文件恢复为 `pending`，旧 `started_at_ms`、旧进度和旧 transient error 被清空。
- paused 任务不会自动 run，API `summary.running=false`，避免 UI 长期展示假 `Running`。
- enabled 且未 paused、仍有 pending/failed 文件的中断任务会在 scheduler startup 后立即 re-enqueue，不等待下一次 daily/hourly 周期。
- 如果 `run.lock` 指向仍存活的其它 Bifrost 进程，恢复逻辑不抢占、不重置文件状态。

### TC-ASPB-38 Resume 不阻塞主服务且后台恢复 ASR 处理

操作步骤：

1. 使用默认数据目录启动最新服务：
   ```bash
   cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy -y
   ```
2. 打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=tools-asr`，找到一个 `paused=true` 且有 pending/processing 文件的目录任务。
3. 点击任务行 `Resume`，同时在另一个终端连续请求轻量接口：
   ```bash
   for i in $(seq 1 20); do
     time curl -fsS http://127.0.0.1:9900/_bifrost/api/proxy/address >/dev/null
     sleep 1
   done
   ```
4. 继续观察任务列表和 `ps -axo pid,pcpu,command | rg 'bifrost|asr'`。

预期结果：

- 点击 `Resume` 后页面不会整页卡死，按钮状态快速变为 running。
- `POST /api/asr/tasks/<task_id>/resume` 只负责取消 paused 状态并派发后台 run，不在请求线程内递归扫描音频目录或重建 heavy summary。
- 任务列表在 `summary.running=true` 期间使用已持久化 `files.json` 的 cached summary，避免 10 秒自动刷新再次触发重型目录扫描。
- Resume 和启动恢复不能在 ASR run 主流程同步补算历史大文件内容 hash；缺少 hash 时按普通文件处理，导入复制产生的 BLAKE3 只在后台内容哈希队列中串行执行。
- `/api/proxy/address` 等主服务轻量接口在 ASR 恢复处理期间仍能快速响应。
- ASR 文件解析继续在后台任务/子进程链路中推进，不阻塞 WebUI 主流程或管理端 API。

### TC-ASPB-39 临时暂停在下一次调度自动恢复

操作步骤：

1. 使用临时数据目录启动服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-planb cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 创建一个绑定空音频目录且 enabled=true 的 daily 任务。
3. 调用临时暂停接口：
   ```bash
   curl -s -X POST 'http://127.0.0.1:8801/_bifrost/api/asr/tasks/<task_id>/pause?mode=temporary'
   ```
4. 检查任务仍保留 `next_run_at_ms`，并在测试数据目录中把该 task 的 `next_run_at_ms` 调整为过去时间，模拟下一次调度到点：
   ```bash
   python3 - <<'PY'
   import json
   path = './.bifrost-test-planb/asr/tasks.json'
   data = json.load(open(path))
   data['tasks'][0]['next_run_at_ms'] = 1
   json.dump(data, open(path, 'w'), indent=2)
   PY
   ```
5. 轮询任务列表：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/asr/tasks
   ```
6. 在 WebUI 任务列表和任务详情页点击 `Pause` 下拉，分别确认存在 `Pause until next schedule` 和 `Pause indefinitely` 两个选项。

预期结果：

- 临时暂停响应中 `paused=true`、`pause_mode=temporary`，返回 task 中 `next_run_at_ms` 非空。
- 调度到点后 scheduler 自动清除 `paused` 并派发后台 run；空目录任务会快速结束，最终任务列表中 `paused=false`、`summary.running=false`。
- 临时暂停期间手动 Run 仍返回 HTTP 409，用户如需立刻继续必须点击 Resume。
- 长期暂停仍清空 `next_run_at_ms`，不会被 scheduler 自动恢复，必须手动 Resume。
- WebUI 的普通 Pause 操作提供临时暂停和长期暂停两个明确选项；Force Pause 保持长期暂停语义，用于立即释放运行中的 ASR/ffmpeg 子进程。

### TC-ASPB-40 ASR streaming timeout 与 managed server breaker

操作步骤：

1. 执行 request timeout 单测：
   ```bash
   cargo test -p bifrost-admin asr_runtime_timeouts_are_bounded_for_short_chunks --lib
   ```
2. 执行 breaker 状态单测：
   ```bash
   cargo test -p bifrost-admin server_failure_breaker --lib
   ```
3. 执行策略级失败阈值单测：
   ```bash
   cargo test -p bifrost-admin reuse_server_failure_threshold --lib
   ```
4. 执行 E2E guard 脚本：
   ```bash
   bash e2e-tests/tests/test_qwen3_asr_runtime_guards.sh
   ```

预期结果：

- streaming text endpoint 默认 request timeout 为 45 秒，whole-file request timeout 仍按音频时长限制在 60 到 180 秒，未知时长为 600 秒。
- fork-per-chunk 原生 `asr` 默认 timeout 对短 chunk 更快失败：30 秒 chunk 为 90 秒，10 秒 bisect 子 chunk 为 45 秒；显式 `BIFROST_ASR_CHUNK_TIMEOUT_SECS` 仍可覆盖。
- managed server 连续失败达到阈值后，`force_fork_for_remaining=true`、`restart_required=false`，fallback reason 包含 `switching remaining chunks to fork_per_chunk isolation`。
- server chunk 首次失败后，当前 chunk 的恢复路径是先停止/标记失败服务，再立即用 `fork_per_chunk` 处理；只有后续 server-eligible chunk 才重启 managed server，避免 fallback 与 server 初始化并发抢占内存。
- native chunk timeout 与 memory-limit kill 一样进入 bisect，不做同尺寸三次重试；子 chunk timeout 会继续尝试更小分片，直到最小分片仍失败才留下 failed chunk。
- watchdog 对仍存活进程的 RSS-only advisory 或 sampler failure warning 至少间隔 60 秒记录一次，日志包含 `process_alive=true`，不会清理 managed service state。
- `run_chunk_with_strategy` 在模拟 `test-error:connection refused by watchdog` 的失败路径下，当前 chunk 立即返回 `fork_per_chunk` fallback metric，并在 shadow metric 中保留原始 `reuse_server` error 证据，同时触发上述 breaker 状态。
- E2E guard 脚本不下载或启动真实 Qwen3-ASR，只执行离线断言，exit code 0。

### TC-ASPB-41 运行中追加音频文件继续纳入同一 run

操作步骤：

1. 执行增量重扫排序单测：
   ```bash
   cargo test -p bifrost-admin pending_batch_rescan_picks_up_appended_files_without_retrying_same_run_failures --lib
   ```
2. 执行 pending 时间优先排序单测：
   ```bash
   cargo test -p bifrost-admin pending_batch_sorts_older_source_time_first --lib
   ```
3. 执行真实 Admin API E2E：
   ```bash
   bash e2e-tests/tests/test_asr_task_append_during_run.sh
   ```

预期结果：

- 手动 run 启动时只有第一个音频文件，任务进入 running 后向同一 `audio_dir` 追加第二个音频文件。
- 当前批次处理完后，后台 run 会重新扫描目录，发现追加文件并继续处理，不需要等待下一次 daily schedule 或人工再次点击 Run。
- 两个文件均在同一个 run 中进入 `success`，详情 API 的 `files` 按更早录音时间优先展示第一个文件。
- Daily Docs 中生成 `2026-05-25.md`，后续 Daily Agent 待处理文档可以看到 25 号汇总。
- 如果本次 run 中某个文件失败，它不会在同一 run 的后续重扫中无限重试；历史 failed 文件仍会在新 run 开始时被尝试一次。

### TC-ASPB-42 ASR 首页三 Tab 交互改造

操作步骤：

1. 使用临时数据目录启动最新服务，避免污染默认数据：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test-asr-tabs BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 18898 --unsafe-ssl --skip-cert-check --no-system-proxy --access-mode allow_all
   ```
2. 打开 ASR 首页：
   ```text
   http://127.0.0.1:18898/_bifrost/ai?aiSection=tools-asr
   ```
3. 确认页面顶部显示三个 Tab，顺序分别是 `定时任务`、`ASR 管理`、`声纹识别与唤醒`。
4. 在默认状态下检查 `定时任务` 被选中，页面只展示 Directory Tasks 列表/新建/运行管理，不展示 `Speech Converter`、`Speech to Text`、`Speaker Diarization` 和 `Voice Wake Actions` 卡片。
5. 点击 `ASR 管理`，确认 URL 增加 `asrTab=management`，页面显示 `Model Management` 和 `Speech to Text`，不展示 Directory Tasks 和声纹唤醒卡片。
6. 点击 `声纹识别与唤醒`，确认 URL 增加 `asrTab=voice`，页面显示 `Speaker Diarization` 和 `Voice Wake Actions`，不展示 Directory Tasks 和 ASR 工作台。
7. 刷新浏览器，确认仍停留在 `声纹识别与唤醒` Tab。
8. 直接访问任务详情深链：
   ```text
   http://127.0.0.1:18898/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTab=voice
   ```
   使用任意已存在任务或通过 `New` 创建一个空目录任务后替换 `<task_id>`。
9. 分别切换亮色和暗色主题，重复查看三个 Tab 标题、选中态、卡片文字和按钮可读性。

预期结果：

- ASR 首页不再把模型管理、声纹识别、声纹唤醒、定时任务和转写工作台揉在一个滚动页面中。
- 默认进入第一个 `定时任务` Tab，符合用户主要入口预期。
- `asrTab=management` 和 `asrTab=voice` 可通过 URL 持久化，刷新后恢复同一 Tab。
- `asrTask=<task_id>` 详情深链继续直接进入任务详情页，不被首页 Tab 包裹或拦截。
- 亮色和暗色主题下 Tab 文案、选中态和各卡片内容均清晰可读，没有硬编码颜色导致的对比度问题。

## 清理步骤

```bash
# 停止服务（Ctrl+C 或 kill）
# 删除临时数据目录
rm -rf ./.bifrost-test-planb
```

## 执行记录

| 日期 | 用例 | 命令 / 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-06-03 | TC-ASPB-42 ASR 首页三 Tab 交互改造 | `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_UI_TEST_RUN_ID=manual-asr-tabs BIFROST_UI_TEST_PORT=18898 BACKEND_PORT=18898 WEB_PORT=53991 ... pnpm --dir web exec playwright test tests/ui/asr-home-tabs.spec.ts --reporter=line` | PASS：真实浏览器打开 `/_bifrost/ai?aiSection=tools-asr` 默认选中 `定时任务`，仅展示 Directory Tasks；切到 `ASR 管理` 后 URL 写入 `asrTab=management` 且展示 `Model Management` 和 `Speech to Text`；切到 `声纹识别与唤醒` 后 URL 写入 `asrTab=voice` 且展示 `Speaker Diarization` 和 `Voice Wake Actions`；刷新后仍保持 voice Tab；`asrTask=<task_id>&asrTab=voice` 直接进入任务详情页，不渲染首页 Tab；测试使用临时数据目录、`--no-system-proxy` 和禁用 Sync 自动登录弹窗。 |
| 2026-05-26 | TC-ASPB-41 运行中追加音频文件继续纳入同一 run | `cargo test -p bifrost-admin pending_batch_rescan_picks_up_appended_files_without_retrying_same_run_failures --lib`；`cargo test -p bifrost-admin pending_batch_sorts_older_source_time_first --lib`；`bash e2e-tests/tests/test_asr_task_append_during_run.sh` | PASS：单测证明运行中第二轮扫描会发现追加文件且同一 run 已尝试失败的文件不会无限重试，pending 队列按录音时间早到晚排序；E2E 使用临时 Bifrost 服务和 fake ASR runtime，手动 run 启动时只有第一个音频，running 后追加第二个音频，最终两个文件均为 success，详情文件顺序为 09:00 后 10:00，Daily Docs 生成 `2026-05-25`。 |
| 2026-05-22 | TC-ASPB-21C watchdog 不因 physical footprint unavailable 误杀 asr-server | `cargo test -p bifrost-admin service_watchdog_kills_only_on_reliable_physical_footprint_over_limit --lib` | PASS：测试断言只有 `reliable=true` 且 footprint 超过阈值才触发 kill；RSS-only fallback 即使数值高于阈值也不触发 kill，等于阈值也不触发 kill。代码复核确认连续 `physical footprint unavailable` 或 sampler error 只写 warning 并继续，不清理 managed service state。 |
| 2026-05-22 | TC-ASPB-21 reuse_per_file 服务死亡后当前 chunk 降级并自动重启 server | `cargo test -p bifrost-admin restart_failure_forks_only_current_chunk_and_keeps_retry_pending --lib` | PASS：测试模拟 managed server restart 失败后设置一次性 fork reason，即使 `server_url` 指向可成功的 test server，本 chunk 仍只走 `fork_per_chunk` 且没有 shadow server metric；状态保持 `restart_required=true`、`force_fork_for_remaining=false`，证明不会在 native/fork fallback 同时尝试 server 请求或重启，下一次 server-eligible chunk 才继续重启。 |
| 2026-05-22 | TC-ASPB-21B reuse_server 跨文件复用失败后重启 task-scoped server | `cargo test -p bifrost-admin reuse_server_fallback_schedules_restart_for_later_chunks --lib` | PASS：测试模拟 task-scoped `reuse_server` 首个 chunk 连接失败后立即 fork fallback，断言共享 `ServerRunnerState.restart_required=true`、`force_fork_for_remaining=false` 且 `fallback_reason` 持久化；随后模拟 managed server 重启到 `test-ok:*` 并清除 restart flag，第二个 chunk 重新走 `reuse_server`、使用新 `server_url` 且不携带旧 fallback reason，证明后续 chunk/文件不会永久退化。 |
| 2026-05-22 | TC-ASPB-10 / TC-ASPB-11 / TC-ASPB-39 | `bash e2e-tests/tests/test_asr_task_pause_resume.sh` | PASS：长期暂停使用 `pause?mode=long_term` 后 `pause_mode=long_term`、`next_run_at_ms=null`，paused 状态手动 Run 返回 409；临时暂停使用 `pause?mode=temporary` 后 `pause_mode=temporary` 且 `next_run_at_ms` 非空，将调度时间置为过去后 scheduler 自动清除 paused 并完成空目录后台 run；Resume 空目录任务仍快速进入后台并恢复 Ready |
| 2026-05-18 | TC-ASPB-01 / TC-ASPB-07A / TC-ASPB-07B / TC-ASPB-10 / TC-ASPB-13 | `cargo test -p bifrost-admin asr_cli_invoke --lib`；`cargo test -p bifrost-admin asr_jobs --lib` | PASS：10 个 `asr_cli_invoke` 测试通过，覆盖模型感知 footprint 阈值、`vmmap` 单位解析、physical-footprint 采样间隔边界和 abort check kill 长跑 CLI 子进程；43 个 `asr_jobs` 测试通过，覆盖 force-pause 查询参数、force-pause 必须结合持久 paused 状态、memory-limit event 合并为 root chunk hint、30 秒 chunk/timeline 回归、可中断 ffmpeg 和 normalize/split timeout |
| 2026-05-18 | TC-ASPB-14 真实 30 分钟 CLI 性能基准 | `cargo build --bin bifrost`；`/usr/bin/time -p target/debug/bifrost ai asr stream-file ~/Downloads/we/TX01_MIC007_20260514_183241_orig.wav --model Qwen3-ASR-1.7B --language chinese >/tmp/bifrost-asr-cli-bench-grace-20260518155114.jsonl 2>/tmp/bifrost-asr-cli-bench-grace-20260518155114.err`；`ps -axo ... | rg 'qwen3_asr_rs/asr|target/debug/bifrost ai asr stream-file'` | PASS：stderr 显示 `Split into 65 chunks (30s each, 2s overlap)`；1801s audio 处理为 65 chunks，内部统计 `210.2s total, RTF=0.117`，`/usr/bin/time` wall time `real 216.77`，低于 5 分钟目标；修复前旧逻辑同样 30s 窗口但第 1 个 chunk 因立即 `vmmap -summary` 采样从 3.5s 膨胀到 9.9s，现已通过首采样 grace 修复；结束后未发现遗留 ASR 子进程 |
| 2026-05-18 | TC-ASPB-07C 托管 asr-server 启动内存保护 | 默认服务 `target/debug/bifrost start -p 9900 --no-system-proxy`；`POST /api/asr/tasks/a911c68b0f7a43afa29d1863cc02229a/pause?force=true`；`POST /api/asr/service/start?model=Qwen3-ASR-1.7B&language=chinese`；检查 `~/.bifrost/asr/service.json` 与 `ps -o pid,pgid,rss,command -p <pid>`；`POST /api/asr/service/stop`；resume 目录任务 | PASS：目录任务 force-pause 后 running=false 且 processed=2/pending=40/failed=0；Start Service 返回 ready/managed=true，`service.json` 写入 pid=71667、port=51885、managed_by=webui；`ps` 显示 PGID=PID=71667，确认托管 `asr-server` 独立进程组；Stop Service 成功，随后目录任务 resume 为 running=true |
| 2026-05-18 | 工作区回归复测 | `pnpm --dir web exec tsc -b --pretty false`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo test --workspace --all-features`；`cargo fmt --all -- --check` | PASS/PARTIAL：前端类型检查通过；clippy 全绿；workspace all-features 全量测试通过；fmt check 仅被既有非 ASR 文件 `crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs` 的格式差异阻塞，ASR 修改文件已用 rustfmt 2021 格式化 |
| 2026-05-18 | TC-ASPB-13 真实服务 API 链路 | 临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-force-pause.* BIFROST_ASR_MAX_FOOTPRINT_MB=3000 cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy`，创建 `~/Downloads/we` 目录任务，手动 Run 后调用 `POST /_bifrost/api/asr/tasks/<task_id>/pause?force=true` | PASS：pause 响应包含 `force:true`、`paused:true`、`running:true` 和 force-pause 文案；后台清理后 `summary.running=false`。该次在模型 CLI 推理前完成暂停，原生推理中 kill 路径由 `abort_check_kills_running_cli_child` 单测覆盖 |
| 2026-05-18 | WebUI Force Pause 控制 | `pnpm --dir web exec tsc -b --pretty false`；`pnpm --dir web run build` | PASS：ASR Directory Tasks 列表和详情页新增 Force Pause 按钮，前端类型检查和生产构建通过 |
| 2026-05-18 | 工作区回归 | `cargo test --workspace --all-features`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo fmt --all -- --check` | PARTIAL PASS：workspace all-features 测试全绿，clippy 全绿；fmt check 仅被既有非 ASR 文件 `crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs` 的格式差异阻塞，ASR 修改文件已用 rustfmt 2021 格式化 |
| 2026-05-18 | TC-ASPB-15 任务详情表开始时间/耗时 | `pnpm --dir web exec tsc -b --pretty false`；Playwright 打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a`；截图 `/tmp/bifrost-asr-task-detail-timing.png` | PASS：任务文件表列头包含 `Audio`、`Started`、`Elapsed`、`Finished`；真实 API 中当前 processing 文件包含新的 `started_at_ms`，UI 表格可显示耗时 |
| 2026-05-19 | TC-ASPB-16A 任务详情默认优先展示未完成文件并支持状态筛选 | `cargo test -p bifrost-admin task_detail_sorts_unfinished_files_before_successes --lib`；`pnpm --dir web exec tsc -b --pretty false`；`pnpm --dir web build`；`cargo build --bin bifrost`；重启 9900 后打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=tools-asr&asrTask=a911c68b0f7a43afa29d1863cc02229a`；切换 `Pending (30)`、`Completed (42)`、`Failed (0)`、`All (72)` | PASS：API 返回 `paused=true`、`summary.discovered=72/processed=42/pending=30/failed=0/running=false`，`files[0..5]` 均为 `pending`；WebUI 顶部显示 `Processing (0)`、`Pending (30)`、`Completed (42)`、`Failed (0)`、`All (72)`；默认 `All` 显示 `Showing 72 of 72 files` 且第一页展示未处理录音；切到 `Pending` 显示 `Showing 30 of 72 files`；切到 `Completed` 显示 `Showing 42 of 72 files`；切到 `Failed` 显示 `Showing 0 of 72 files` 和空态 |
| 2026-05-18 | TC-ASPB-16 服务重启后孤儿 processing 恢复 pending | `cargo build --bin bifrost`；使用临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-orphan.*` 和随机端口启动 `target/debug/bifrost start --unsafe-ssl --no-system-proxy`，预置 `files.json` 中 `status=processing/started_at_ms=123/progress=29/65/error=old transient error` 后访问 `/api/asr/tasks/orphan-task` | PASS：启动后的 API 返回该文件 `status=pending`，`started_at_ms/progress_current/progress_total/error` 均为 null，`summary.running=false` 且 `summary.pending=1`；临时服务和数据目录已清理 |
| 2026-05-18 | TC-ASPB-17 / TC-ASPB-18 / TC-ASPB-19 / TC-ASPB-20 runtime_strategy 状态与日志证据 | `cargo test -p bifrost-admin asr_jobs --lib`；`cargo test -p bifrost-admin asr_cli_invoke --lib`；`pnpm --dir web exec tsc -b --pretty false`；`BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`；临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-runtime-*` 启动 Bifrost，使用 `~/.bifrost/asr/qwen3_asr_rs/sample3.wav` 分别创建 `runtime_strategy=compare` 与 `runtime_strategy=reuse_per_file` 目录任务 | PASS/PARTIAL：单测覆盖旧任务默认 runtime、chunk metric 的 runner/RTF/text hash/fallback/error；前端类型检查通过，WebUI 暴露 `fork_per_chunk/reuse_server/reuse_per_file/auto/compare`；离线 E2E 覆盖默认 runtime；真实 compare smoke 成功，任务详情持久化 fork `RTF=0.507` 与 server shadow `RTF=0.326`、两边 text hash 相同，stdout 和 `BIFROST_DATA_DIR/logs/bifrost.2026-05-18.log` 均出现 `ASR CLI child started/completed`、`ASR compare strategy completed paired chunk` 和两条 `ASR chunk metric`；真实 `reuse_per_file` smoke 成功，metric runner 为 `reuse_server` 且 server_url 非空，日志出现 `stopping ASR managed server after file-scoped runtime strategy`。`auto` 的 RTF 恶化切换需要更长音频压测才能真实触发，本轮由代码路径、日志字段和 fallback 持久化单测/结构验证覆盖 |
| 2026-05-18 | TC-ASPB-17 默认性能策略切换 | `cargo test -p bifrost-admin runtime_strategy_defaults_to_reuse_per_file_for_old_task_json --lib`；临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-default-runtime.* cargo run --quiet --bin bifrost -- start -p 61973 --unsafe-ssl --no-system-proxy` 后创建未显式传 `runtime_strategy` 的空目录任务；`BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：未显式传 `runtime_strategy` 的目录任务创建响应和详情默认返回 `reuse_per_file`；旧任务 JSON 缺少该字段时也进入 `reuse_per_file`；WebUI 表单默认选中 `Reuse / file`；CLI `stream-file` 默认启动/复用 `asr-server` 做文件级复用，保留 fork 路径作为可显式选择的对照策略 |
| 2026-05-19 | TC-ASPB-23 WebUI 批量排队重试所有 failed chunks | `cargo test -p bifrost-admin bulk_retry_targets_include_only_files_with_failed_chunks_in_path_order --lib`；`pnpm --dir web test:ui asr-microphone-meter.spec.ts -g "bulk retry"` | PASS：后端目标选择只包含 `failed_chunks` 非空文件且按路径稳定排队；WebUI 任务详情点击 `Retry all failed chunks` 后调用任务级 `POST /retry-failed-chunks`，返回 queued 状态并展示 `Bulk chunk retry`、`0/2 files`、`0/3 chunks recovered`，验证按钮与状态区链路 |
| 2026-05-19 | TC-ASPB-24 ASR jobs 模块拆分后任务 API 行为不变 | `BIFROST_DATA_DIR=/tmp/bifrost-asr-split-human.* cargo run --bin bifrost -- start -p 18894 --unsafe-ssl --no-system-proxy`；跳过临时 CA 安装；创建空目录任务；查询列表/详情；调用 `POST /_bifrost/api/asr/tasks/<task_id>/retry-failed-chunks` | PASS：任务 `6f353a0f66a54862a93649865bd05bbc` 创建成功，列表 `list_count=1`，详情 `runtime_strategy=reuse_per_file`、`summary_discovered=0`、`summary_failed_chunk_count=0`；bulk retry no-op 返回 `status=completed`、`queued_files=0`；临时服务已 Ctrl+C 停止，临时数据目录和临时 JSON 响应文件已清理 |
| 2026-05-19 | TC-ASPB-25 Daily Agent Runner 方案文档验收 | `test -f design/asr-daily-agent-runner.md`；`rg -n "ASR 创建/编辑页面|Daily Agent Runner|Runner type|Runner ID|Instructions / AGENTS.md" design/asr-daily-agent-runner.md`；`rg -n "内置 AGENTS.md 模板|assets/asr_daily_agents_default.md|PUT /api/asr/tasks/\\{task_id\\}/daily-agent/agents|instructions_source" design/asr-daily-agent-runner.md`；`rg -n "Daily Workspace 初始化|daily/report|git init|Git 是增强能力|git unavailable" design/asr-daily-agent-runner.md`；`rg -n "Runner 执行逻辑|Run now|ASR 完成后触发|maybe_enqueue_daily_agent_after_asr_run|测试计划" design/asr-daily-agent-runner.md`；`rg -n "IM delivery|provider_id|target_id|Send policy|IM 发送逻辑|未绑定 IM|绑定 IM" design/asr-daily-agent-runner.md`；`rg -n "Runner 消息组织差异|ChatGPT Web 不能读本地|每个 ASR 任务一个固定 conversation|第一条消息|第二条消息|asr-daily:<task_id>|Reset ChatGPT Web conversation" design/asr-daily-agent-runner.md`；`rg -n "ASR 定时任务内部|父级 ASR task schedule|音频处理、failed chunk retry|daily markdown 刷新|skipped_no_daily_changes|trigger_source=asr_completion" design/asr-daily-agent-runner.md human_tests/asr-scheduled-task-plan-b.md human_tests/readme.md`；`rg -n "DailyAgentChangePlanner|daily_agent_processed.json|已处理文档记录|unchanged|appended|rewritten|IncrementalPayload|FileList|增量文本|文件清单" design/asr-daily-agent-runner.md` | PASS：方案文档存在；覆盖 ASR 创建/编辑页面 Daily Agent Runner 配置、Runner type/Runner ID、`Instructions / AGENTS.md`、内置默认手册、用户可编辑 agents API、daily workspace/report/Git 初始化和 Git 不可用降级；明确 Runner 放在 ASR 定时任务内部，跟随父级 ASR task schedule，不维护独立 scheduler；明确 ASR 音频处理、failed chunk retry 合并、daily markdown 刷新和 ASR 状态持久化完成后才排队 Daily Agent；明确 processing 期间不启动 Runner，无 daily 变更时记录 skipped；明确 `DailyAgentChangePlanner` 使用 `daily_agent_processed.json` 记录已处理文档，unchanged 不再投递；ChatGPT Web 只接收新增/变更增量文本或 diff；Bifrost Agent/Codex 只接收更新文件清单、变化类型、hash 和目标 report 路径；覆盖手动 Run now、可选 IM 绑定、provider/target、send policy、未绑定只落盘、绑定后发送结论、ChatGPT Web 任务级固定 conversation、首次注入 `AGENTS.md`、后续只发送新增/变更内容与测试计划 |
| 2026-05-19 | TC-ASPB-30 Daily Agent Runner 实现闭环回归 | `wc -l crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_im.rs crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_api.rs`；`! rg -n "bifrost_agent runner not yet|catch \\(.*: any\\)|no-explicit-any" crates/bifrost-admin/src/handlers/asr_jobs web/src/pages/ASR/components/DailyAgentTab.tsx`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent --lib`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin bifrost_agent_runner --lib`；`SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --lib`；`SKIP_FRONTEND_BUILD=1 cargo clippy -p bifrost-admin --lib -- -D warnings`；`pnpm --dir web exec eslint src/pages/ASR/components/DailyAgentTab.tsx src/api/asr.ts`；`pnpm --dir web run build`；`git diff --check`；默认数据目录 `cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy -y` 后执行 IM config/send 和临时任务初始化 API 回归 | PASS：`daily_agent.rs` 为 1433 行，API/IM 分别拆到 `daily_agent_api.rs` 和 `daily_agent_im.rs`；未检出未集成 Bifrost Agent 字符串、`catch (...: any)` 或 no-explicit-any；Daily Agent prompt/report gate 单测通过，Bifrost Agent 无 runner_id ready 单测通过；bifrost-admin lib check 和 clippy `-D warnings` 通过；DailyAgentTab/asr.ts targeted eslint 通过；Web 生产构建通过；diff whitespace 检查通过；默认数据目录启动确认 `/Users/eden/.bifrost`，系统代理 Disabled，`feishu` provider connected；owner 缺 provider 返回 400，合法 `provider_id=feishu` + `target_id=owner` 返回 200，Daily Agent `/send` 返回 200 并发送最近 report；临时 ASR 任务创建后 `daily/AGENTS.md`、`daily/report/`、`.git` 均存在，External CLI 无 runner_id 返回 400，Bifrost Agent 无 runner_id 返回 200，临时任务和临时数据已清理 |
| 2026-05-20 | TC-ASPB-30 Daily Agent Runner 单字段配置回归 | `SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --lib`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent --lib`；`SKIP_FRONTEND_BUILD=1 cargo clippy -p bifrost-admin --lib -- -D warnings`；`pnpm --dir web exec tsc -b --pretty false`；`pnpm --dir web exec eslint src/pages/ASR/components/DailyAgentTab.tsx tests/ui/asr-daily-agent-runner.spec.ts src/api/asr.ts`；`pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts` | PASS：服务端 Daily Agent 配置收敛为单字段 `runner` 与 `im_delivery.channel`，空 runner 校验失败、`bifrost_agent` 与自定义 runner id 均可被识别；WebUI Daily Agent 配置只显示 Runner 下拉和 Channel 下拉，不显示 Runner Type / Runner ID / Provider ID / Target ID；Runner 下拉来自 Runners 配置，选择 `web` 后提交 `runner=web`；Channel 下拉来自 Provider Owner 与 IM Targets，选择 Daily Report Room 后提交 `im_delivery.channel=target:daily-report-room` |
| 2026-05-20 | TC-ASPB-31 默认目录 Live WebUI + Runner 真实执行回归 | `SKIP_FRONTEND_BUILD=1 RUST_LOG=bifrost_admin::handlers::asr_jobs=info,bifrost_admin::im_gateway=info,bifrost_agent=info cargo run --bin bifrost -- start -p 18896 --unsafe-ssl --no-system-proxy -y`；`BIFROST_LIVE_BASE_URL=http://127.0.0.1:18896 ASR_DAILY_AGENT_LIVE_DATE=2026-05-20 ASR_DAILY_AGENT_LIVE_TIMEOUT_MS=900000 pnpm --dir web exec node tests/ui/asr-daily-agent-live-e2e.mjs`；发现 IM self-call 固定打 `127.0.0.1:9900` 后修复为读取默认目录 `runtime.json.port`，并执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin daily_agent_im_self_call --lib`；重启 18896 后再次执行真实 IM send；升级 live 脚本后复跑 `BIFROST_LIVE_BASE_URL=http://127.0.0.1:18896 ASR_DAILY_AGENT_LIVE_DATE=2026-05-20 ASR_DAILY_AGENT_LIVE_TIMEOUT_MS=900000 ASR_DAILY_AGENT_LIVE_KEEP_ARTIFACTS=1 pnpm --dir web exec node tests/ui/asr-daily-agent-live-e2e.mjs` | PASS：当前源码服务使用默认目录 `/Users/eden/.bifrost` 启动，系统代理保持 Disabled；live WebUI 打开 `/_bifrost/ai?...&asrTask=<task_id>`，在真实页面将 Runner 下拉切到 `codex`，将 IM Channel 下拉保存为 `owner:feishu-main`，页面未出现 Runner Type / Runner ID / Provider ID / Target ID；默认目录真实 ASR Daily Agent 任务中 `runner=bifrost_agent` 生成 report 且 `processedRunner=bifrost_agent`；`runner=codex` 生成 report 且 `processedRunner=codex`；`runner=web` 生成 `/Users/eden/.bifrost/asr/data/text/159f0fa758334ab1b3f1191c7921b322/daily/report/2026-05-20-report.md`，`processedRunner=web`；三份报告均包含 `ASR Daily Agent Live Runner Result`、`runner validation passed` 和唯一 marker；ChatGPT Web 长输入日志确认 `injected composer text via paste path`、`expectedLength=767`、`pasteDispatched=true`、`ok=true`，随后捕获 `f/conversation` 并成功写 report；真实 IM send 脚本任务 `c1e96d75ca2d4cf4beb0506fcfa0e162` 通过 `channel=owner:feishu-main`、`mode=full_report` 成功发送报告原文，`sentPreview` 以 `# ASR Daily Agent Live Runner Result` 开头，`last_sent_at_ms=1779212857792` 且无 `last_send_error` |
| 2026-05-20 | TC-ASPB-32 Directory Tasks 新建弹窗回归 | `pnpm --dir web exec tsc -b --pretty false`；`pnpm --dir web exec eslint src/pages/ASR/components/DirectoryTasksPanel.tsx src/pages/ASR/index.tsx tests/ui/asr-microphone-meter.spec.ts`；`pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts -g "ASR directory tasks can be created and refreshed in the tools panel"` | PASS：`/_bifrost/ai?aiSection=tools-asr` 的 Directory Tasks 卡片正文不再常驻展示 Name / Audio Directory 创建表单；右上角只展示 `New` 按钮；点击 `New` 后弹出 `New Directory Task` Modal，填写 Name 和 Audio Directory 后点击 `Create` 成功创建任务并关闭弹窗，任务列表仍展示 `Recordings`、`/tmp/asr-audio`、`Daily at 02:00` 和处理进度 |
| 2026-05-20 | TC-ASPB-33 Daily Agent Instructions 自适应高度回归 | `pnpm --dir web exec tsc -b --pretty false`；`pnpm --dir web exec eslint src/pages/ASR/components/DailyAgentTab.tsx tests/ui/asr-daily-agent-runner.spec.ts`；`pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts` | PASS：mock 返回 36 段长 `AGENTS.md` 后，`Agent Instructions (AGENTS.md)` 编辑框 `clientHeight + 2 >= scrollHeight` 且 `overflow-y=hidden`；编辑框自适应撑高，内部不出现独立滚动条，Runner / IM Channel 单下拉回归仍通过 |
| 2026-05-20 | TC-ASPB-34 默认目录多文件 ChatGPT Web Daily Agent 与 FullReport IM 分片回归 | `BIFROST_DATA_DIR=/Users/eden/.bifrost cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy`；`curl -sS -X POST 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/run?force=true'`；轮询任务详情；检查 `/Users/eden/.bifrost/im_gateway/runs/1779216503662-*`、`1779216632038-*`、`1779216764881-*`、`1779216872746-*`；检查 `/Users/eden/.bifrost/asr/data/text/76612de33e9740bc92440ce64a98a4cb/daily/report/*-report.md`；查询 `/_bifrost/api/im-gateway/providers/acc/messages` | PASS/PARTIAL：默认任务 `day` 配置为 `runner=web`、`channel=owner:acc`、`mode=full_report`；daily 目录包含 `2026-05-14.md`、`2026-05-15.md`、`2026-05-16.md`、`2026-05-17.md` 四个源文件；force run 生成四个 ChatGPT Web run，prompt 分别只包含单日文件且按日期升序处理，大小约 217KB、262KB、267KB、149KB；四个 run 的 `result.json.status=succeeded`，四个 report 均生成（约 24.7KB、23.5KB、25.6KB、21.6KB）；ASR Daily Agent `last_status=success`。IM provider `acc` 当前即使发送短文本和上线通知也返回 `weixin sendmessage failed: ret=-2`，因此 FullReport 分片第 1/4 条发送失败；message log 预览为 `ASR Daily Agent Report 1/4` 加报告原文，确认不再降级为摘要。 |
| 2026-05-20 | TC-ASPB-35 Daily Agent Processed Documents report 全屏 Markdown 详情 | `BIFROST_DATA_DIR=/Users/eden/.bifrost cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy --daemon -y`；`curl -sS 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/reports/2026-05-14'`；`curl -i 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/daily-agent/reports/%2E%2E%2Fsecret'`；`pnpm --dir web exec node --input-type=module` 使用 Playwright 打开真实 9900 页面、进入 Daily Agent、刷新 tab、点击 `2026-05-14-report.md`、刷新 report 详情、检查 Markdown DOM 并返回 | PASS：真实 API 返回 `runner=web`、report 路径 `/Users/eden/.bifrost/asr/data/text/76612de33e9740bc92440ce64a98a4cb/daily/report/2026-05-14-report.md` 和 Markdown 正文；路径穿越日期返回 400；真实页面进入 Daily Agent 后 URL 增加 `asrTaskTab=daily-agent`，刷新后 tab 仍选中；点击 report 后 URL 增加 `asrDailyReport=2026-05-14`，出现 `asr-daily-agent-report-page` 和 `asr-daily-agent-report-content`；刷新 report 详情后仍恢复全屏 Markdown；Markdown 渲染出 H1 `2026-05-14 日报（Force 更新版）`、多级标题与 140 个列表项，`preCount=0`；点击返回后 URL 移除 `asrDailyReport` 并保留 `asrTaskTab=daily-agent` |
| 2026-05-21 | TC-ASPB-36 Directory Task Runtime 选项说明 | `pnpm --dir web exec tsc -b --pretty false`；临时 `BIFROST_DATA_DIR=/tmp/bifrost-runtime-desc.stX3OY CARGO_TARGET_DIR=/tmp/bifrost-runtime-desc-target cargo run --bin bifrost -- start -p 18897 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all`；`BIFROST_UI_TEST_RUN_ID=manual-runtime-desc BIFROST_UI_TEST_PORT=18897 BACKEND_PORT=18897 WEB_PORT=53990 ... pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts -g "ASR directory tasks can be created and refreshed in the tools panel" --reporter=line --timeout=60000` | PASS：Runtime 下拉展示 `Reuse / file`、`Fork / chunk`、`Reuse server`、`Auto fallback`、`Compare` 及各自说明；下拉菜单加宽后五个策略均可直接看到；选中态保持短标题 `Reuse / file`；创建任务流程仍通过，默认提交 `runtime_strategy=reuse_per_file`；临时后端通过 `--no-system-proxy` 启动且由 Playwright teardown 停止 |
| 2026-05-21 | TC-ASPB-37 服务重启后中断 ASR run 自动恢复且不假 Running | `cargo test -p bifrost-admin startup_recovery --lib`；`bash e2e-tests/tests/test_asr_task_startup_recovery.sh` | PASS：单测覆盖 enabled 未暂停任务从 stale run.lock + processing 恢复后进入 startup recovery 计划、paused 任务不自动 requeue、live owner lock 不被抢占、RAII running guard drop 后释放内存 running 标记；E2E 使用临时数据目录预置 paused stale run，启动最新 bifrost 后 API 返回文件 `pending`、旧进度清空、`summary.running=false` 且 run.lock 已删除 |
| 2026-05-24 | TC-ASPB-40 ASR streaming timeout 与 managed server breaker | `cargo test -p bifrost-admin asr_runtime_timeouts_are_bounded_for_short_chunks --lib`；`cargo test -p bifrost-admin server_failure_breaker --lib`；`cargo test -p bifrost-admin reuse_server_failure_threshold --lib`；`bash e2e-tests/tests/test_qwen3_asr_runtime_guards.sh` | PASS：streaming text endpoint 默认 timeout 为 45s；whole-file timeout 保持 duration-aware bounds；breaker 达阈值后设置 `force_fork_for_remaining=true`、`restart_required=false`，fallback reason 明确包含 `switching remaining chunks to fork_per_chunk isolation`；策略级模拟 server 连接失败路径当前 chunk 返回 `fork_per_chunk` fallback metric，并通过 shadow metric 保留原始 `reuse_server` error 证据且触发 breaker；E2E guard 脚本离线通过且不启动真实模型 |
| 2026-05-24 | TC-ASPB-40 ASR timeout/fallback/watchdog 回归补充 | `cargo test -p bifrost-admin asr_runtime_timeouts_are_bounded_for_short_chunks --lib`；`cargo test -p bifrost-admin server_failure_recovery_reason_uses_fork_for_current_chunk --lib`；`cargo test -p bifrost-admin service_watchdog_warning_log_is_rate_limited --lib`；代码复核 `memory_bisect.rs` timeout 分支和 `chunk_runtime.rs` server fallback 顺序 | PASS：30s native chunk 默认 timeout 为 90s、10s 子 chunk 为 45s；fallback reason 明确当前 chunk 走 `fork_per_chunk` 且 server restart 延后到 later chunks；watchdog warning 60s 限流；timeout 与 memory-limit 均不做同尺寸重试而进入 bisect，子 chunk timeout 不立即中止父 chunk；server fallback 不在当前 fork fallback 前重启 managed server |
