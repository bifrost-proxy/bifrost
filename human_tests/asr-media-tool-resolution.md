# ASR 媒体工具定位

## 功能模块说明

验证 Bifrost 从 macOS 桌面环境启动、PATH 不包含 Homebrew/MacPorts 时，ASR 仍能找到已安装的 `ffmpeg` / `ffprobe`，并确保外接录音导入后可以从本地副本继续转录。

## 前置条件

- macOS 已通过 Homebrew 或 MacPorts 安装 `ffmpeg`。
- 已构建当前 checkout 的 `target/debug/bifrost`。
- 正式服务验证只读取 `~/.bifrost` 和 `~/Recordings`，不重启服务、不修改任务配置。
- 自动化服务使用临时数据目录、动态非 9900 端口、`--no-system-proxy`、禁用托盘和 Sync 登录弹窗。

## 测试用例列表

### TC-AMT-01：桌面最小 PATH 仍能识别 ffmpeg

操作步骤：

1. 执行：

   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_asr_media_tool_resolution.sh
   ```

2. 观察脚本创建的临时服务和 `/api/asr/status` 断言。

预期结果：

- 临时服务 PATH 只有系统目录，不包含 Homebrew/MacPorts。
- `ffmpeg_available=true`。
- 输出包含 `PASS`，并在退出后清理临时服务和数据。

### TC-AMT-02：正式 ASR 重跑不再出现媒体工具 spawn 失败

操作步骤：

1. 设置正式 ASR 任务 ID，然后读取本轮进度与文件记录：

   ```bash
   TASK_ID="<task-id>"
   TASK_ROOT="$HOME/.bifrost/asr/tasks/$TASK_ID"
   jq -s -e '
     .[0].started_at_ms as $start
     | [.[1].files[] | select((.started_at_ms // 0) >= $start)] as $records
     | any($records[]; ((.status == "processing" or .status == "success") and (.text_chars // 0) > 0))
       and all($records[]; ((.error // "") | contains("failed to spawn process") | not))
   ' "$TASK_ROOT/run_progress.json" "$TASK_ROOT/files.json"
   ```

2. 读取 `run_progress.json`，确认 `current_chunk_done` 持续增加。

预期结果：

- 本轮至少一个文件已经进入真实 ASR 并产生文本。
- 本轮所有已触碰记录都不再包含 `failed to spawn process`。
- 进度持续前进，不是导入后立即把全部文件标为失败。

### TC-AMT-03：外接录音完整导入且重复扫描幂等

操作步骤：

1. 检查导入 ledger 中存在成功导入 10 个文件、失败数为 0 的完成记录：

   ```bash
   TASK_ID="<task-id>"
   TASK_ROOT="$HOME/.bifrost/asr/tasks/$TASK_ID"
   jq -e 'any(.runs[]; .status == "completed" and .imported == 10 and .failed == 0)' \
     "$TASK_ROOT/external_imports.json"
   ```

2. 检查后续扫描没有重复导入，并确认任务关闭源文件删除：

   ```bash
   jq -e '.runs[-1] | .status == "completed" and .imported == 0 and .failed == 0' \
     "$TASK_ROOT/external_imports.json"
   jq -e --arg id "$TASK_ID" \
     '.tasks[] | select(.id == $id) | .import_policy.delete_source_after_import == false' \
     "$HOME/.bifrost/asr/tasks.json"
   ```

3. 设备仍连接时，设置 `SOURCE_ROOT="<mounted-device-root>"`，逐个比较 `TX02_MIC049` 至 `TX02_MIC058` 的源文件与 `~/Recordings` 本地副本字节数；同时确认 10 个本地副本均存在且非空。

预期结果：

- 首轮导入 10、失败 0；后续扫描导入 0、失败 0。
- `delete_source_after_import=false`。
- 10 个源文件与本地副本字节数逐个一致，后续 ASR 只读取本地副本。

### TC-AMT-04：说话人分段局部失败保留可用转录

操作步骤：

1. 安装当前 release 二进制并等待正在运行的正式批次安全结束后重启正式服务。
2. 对曾在最后一个短 ASR unit 触发内存保护、但已经生成大量文字的真实录音执行重试。
3. 读取该文件记录、转录正文和 `failed_chunks`。

预期结果：

- 如果所有 unit 本次均成功，文件状态为 `success` 且保留完整文字。
- 如果仍有局部 unit 失败，文件状态为 `partial_success`，已生成文字继续保留，`failed_chunks` 记录失败区间，正文包含可供 `retry-chunks` 精确替换的占位符。
- 只有所有有效 unit 均失败且没有可用文字或时间线时，文件才保持 `failed`。
- 重试不会删除源录音，也不会触发重复 ChatGPT Pro 研究或测试微信消息。

## 清理步骤

- TC-AMT-01 由脚本 trap 清理临时进程和临时数据。
- TC-AMT-02/03 是只读检查，不清理正式录音、转录或任务记录。
- TC-AMT-04 只使用任务原有的精确重试能力；保留正式录音、转录与失败区间记录。

## 执行记录

| 日期 | 用例 | 实际结果 | 结论 |
|---|---|---|---|
| 2026-07-16 | TC-AMT-01 | 使用不含 Homebrew/MacPorts 的最小 PATH 启动当前 debug 二进制；临时服务返回 `ffmpeg_available=true`，脚本输出 `PASS` 并完成清理 | 通过 |
| 2026-07-16 | TC-AMT-02 | 正式任务完成新导入的 `TX02_MIC049`，状态 `success`、`text_chars=7684`，随后自动进入 `MIC050`；本轮已触碰记录均不含 `failed to spawn process` | 通过 |
| 2026-07-16 | TC-AMT-03 | 导入 ledger 记录首轮 `imported=10, failed=0`，后续扫描 `imported=0, failed=0`；删除源文件策略关闭，10 个源文件与本地副本的字节数逐个一致 | 通过 |
