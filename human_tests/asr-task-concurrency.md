# ASR Directory Task 文件级动态并发

## 功能模块说明

验证 ASR Directory Task 可以配置文件级并发数，并在运行中通过任务编辑/API PATCH 动态调整。并发在 `fork_per_chunk` 任务中实际生效，包括启用 speaker diarization 的真实任务；共享 runtime 任务应展示 desired 值但 effective 降级为 1。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 启动服务必须使用临时数据目录，并设置：
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - `BIFROST_DISABLE_TRAY=1`
  - `--no-system-proxy`
- 端口示例使用 `18994`，如被占用需换用空闲端口。
- 可用 API 静态验证不要求真实 Qwen3-ASR 资产；真实吞吐验证需要本机 ASR assets 已初始化。

## 测试用例列表

### TC-ASR-CONC-01 创建任务时保存 desired 并发

操作步骤：

1. 创建临时音频目录：
   ```bash
   AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-concurrency-audio.XXXXXX")"
   ```
2. 启动临时 Bifrost：
   ```bash
   DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-concurrency-data.XXXXXX")"
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_DATA_DIR="$DATA_DIR" \
     cargo run --bin bifrost -- start -p 18994 --unsafe-ssl --no-system-proxy
   ```
3. 创建 `fork_per_chunk` 任务：
   ```bash
   curl -fsS -X POST "http://127.0.0.1:18994/_bifrost/api/asr/tasks" \
     -H 'Content-Type: application/json' \
     -d "{\"name\":\"ASR concurrency\",\"audio_dir\":\"$AUDIO_DIR\",\"recursive\":false,\"enabled\":false,\"runtime_strategy\":\"fork_per_chunk\",\"max_concurrent_files\":3,\"diarization\":{\"enabled\":false,\"profile\":\"sherpa-onnx-balanced\"}}"
   ```
4. 读取任务详情。

预期结果：

- 创建响应和详情 JSON 均包含 `max_concurrent_files: 3`。
- `summary.max_concurrent_files` 为 3。
- `summary.effective_max_concurrent_files` 为 3。

### TC-ASR-CONC-02 运行中 PATCH 可以动态调低 desired 并发

操作步骤：

1. 对 TC-ASR-CONC-01 创建的任务执行：
   ```bash
   curl -fsS -X PATCH "http://127.0.0.1:18994/_bifrost/api/asr/tasks/<task_id>" \
     -H 'Content-Type: application/json' \
     -d '{"max_concurrent_files":2}'
   ```
2. 再次读取任务详情。

预期结果：

- PATCH 返回 200，不返回 `task_running` 冲突。
- 详情中 `max_concurrent_files` 更新为 2。
- 对 `fork_per_chunk` 任务，`summary.effective_max_concurrent_files` 更新为 2。

### TC-ASR-CONC-03 共享 runtime 策略 effective 降级为 1

操作步骤：

1. PATCH 任务 runtime：
   ```bash
   curl -fsS -X PATCH "http://127.0.0.1:18994/_bifrost/api/asr/tasks/<task_id>" \
     -H 'Content-Type: application/json' \
     -d '{"runtime_strategy":"reuse_per_file","max_concurrent_files":4}'
   ```
2. 读取任务详情。

预期结果：

- `max_concurrent_files` 为 4。
- `summary.effective_max_concurrent_files` 为 1。
- 任务仍可保存，不修改 audio_dir/model/language。

### TC-ASR-CONC-04 WebUI 展示 desired/effective 并发

操作步骤：

1. 打开 `http://127.0.0.1:18994/_bifrost/ai?aiSection=tools-asr`。
2. 找到 ASR Directory Tasks 列表中的任务。
3. 点击 Edit，查看 `File Concurrency` 字段。

预期结果：

- 列表 tag 展示 `effective/desired files`。
- Edit 弹窗中 `File Concurrency` 数值等于任务保存的 `max_concurrent_files`。
- 当 runtime 为 `reuse_per_file` 且 desired 大于 1 时，列表展示 effective 降级提示。

### TC-ASR-CONC-05 完整 processing 记录自动恢复为 success

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 预置一个 ASR 任务，`files.json` 中包含一条 `status=processing` 的记录。
2. 该记录必须满足：`chunk_metrics` 全部为 `ok`、`failed_chunks=[]`、`error=null`、`output_text_path` 指向真实存在的文本文件、`output_timeline_path` 指向真实存在的 timeline 文件。
3. 启动最新本地构建的 Bifrost：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_DATA_DIR="$DATA_DIR" \
     ./target/debug/bifrost start -p 18996 --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
4. 调用：
   ```bash
   curl -fsS "http://127.0.0.1:18996/_bifrost/api/asr/tasks/<task_id>"
   ```

预期结果：

- API 返回的该文件状态为 `success`。
- `summary.pending` 为 0。
- `summary.processed` 为 1。
- `summary.diarization_running` 为 false。

### TC-ASR-CONC-06 同一路径旧记录不污染 Files tab

操作步骤：

1. 在 TC-ASR-CONC-05 的同一临时任务中，为同一个 `source_path` 额外写入旧 `pending` 和旧 `processing` key。
2. 启动临时 Bifrost 后读取任务详情。

预期结果：

- `files` 数组只展示当前有效记录，不展示旧 `pending/processing` 记录。
- 当前有效记录为 `success`。
- UI/API 不再表现为“最后一片卡住”。

## 清理步骤

1. 停止临时 Bifrost 服务。
2. 删除测试创建的 `DATA_DIR` 和 `AUDIO_DIR`。

## 执行记录

| 日期 | 用例 | 实际结果 | 状态 |
| --- | --- | --- | --- |
| 2026-06-19 | TC-ASR-CONC-01/02/03 | 已启动临时服务：`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_DATA_DIR=<tmp> cargo run --bin bifrost -- start -p 18994 --unsafe-ssl --no-system-proxy --skip-cert-check`；API 创建 `fork_per_chunk,max_concurrent_files=3,diarization.enabled=false` 任务后详情返回 `max=3,effective=3`；PATCH `max_concurrent_files=2` 后详情返回 `max=2,effective=2`；PATCH `runtime_strategy=reuse_per_file,max_concurrent_files=4` 后详情返回 `max=4,effective=1` | 通过 |
| 2026-06-19 | TC-ASR-CONC-04 | 已用 in-app browser 打开 `http://127.0.0.1:18994/_bifrost/ai?aiSection=tools-asr`，列表显示任务 `ASR concurrency`、`1/4 files` 和 `effective 1`；点击 Edit 后弹窗包含 `File Concurrency`，输入值为 `4` | 通过 |
| 2026-06-19 | TC-ASR-CONC-01/02/03 | 已执行正式 E2E 脚本：`BIFROST_ASR_TASK_CONCURRENCY_E2E_PORT=18995 bash e2e-tests/tests/test_asr_task_concurrency_api.sh`，脚本构建当前 bifrost，使用临时数据目录和 `--no-system-proxy --skip-cert-check` 启动服务，覆盖 create `max=3/effective=3`、PATCH `max=2/effective=2`、共享 runtime `max=4/effective=1` | 通过 |
| 2026-06-19 | 真实 9900 稳定性/性能 | 使用当前源码二进制从 `/Users/eden_studio` 工作目录启动 9900（保持历史 `~/audio` 相对路径语义），任务 `c1c57318206c4f338f1267b7f37a81b8` 设置 `runtime_strategy=fork_per_chunk,max_concurrent_files=3,diarization.enabled=true` 并启动；首次实测发现普通 async worker 会让 admin API 30s 超时，修复为 blocking worker + 2s 调度 tick 后复测：API latency 稳定约 `0.27-0.30s`，`active_file_count=3`，PATCH `max_concurrent_files=4` 返回 `effective=4`，下一个 tick 后 `active_file_count=4` 且 4 个 diarization worker 同时运行，`chunks_failed=0` | 通过 |
| 2026-06-19 | TC-ASR-CONC-05/06 | 已执行正式 E2E 脚本：`BIFROST_ASR_TASK_STATE_RECOVERY_E2E_PORT=18996 bash e2e-tests/tests/test_asr_task_state_recovery.sh`，脚本预置完整 artifact 的 stale `processing` 记录以及同一路径旧 `pending/processing` key，启动临时 Bifrost 后通过真实 Admin API 验证详情 `files` 只返回 1 条 `success`，`summary.processed=1`、`pending=0`、`diarization_running=false` | 通过 |
