# Qwen3-ASR 本地 API Server

## 功能模块说明

验证 32GB Apple Silicon Mac 上使用 Qwen3-ASR-1.7B、qwen3_asr_rs、MLX/Metal 后端启动本地 OpenAI-compatible API server，并完成中文音频转写。同时验证 Bifrost WebUI AI -> Tools -> ASR 中的语音转换器异步初始化状态、下载进度、异常展示、文件输入、麦克风入口和流式输出。该用例只操作 `~/.bifrost/asr` 固定目录；启动 Bifrost 时必须使用临时数据目录和 `--no-system-proxy`，不修改系统代理，不修改 `~/.zshrc`。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 机器为 Apple Silicon：`uname -m` 输出 `arm64`。
- 非 macOS Apple Silicon 平台默认不启用本地 Qwen3-ASR，Web/API/CLI 入口必须明确提示不支持当前操作系统。
- 内存建议 32GB：`sysctl hw.memsize | awk '{printf "%.0f GB\n", $2/1024/1024/1024}'` 输出 `32 GB` 或更高。
- 已安装 Homebrew；如果缺少 `ffmpeg`，初始化自检、WebUI Start Service 自检和 CLI 启动自检都应自动执行平台匹配的 Homebrew 安装流程。自动安装失败时必须提示 `brew install ffmpeg`，用户处理后可重试同一操作继续。
- 网络可访问 GitHub release 与 Hugging Face 模型文件。
- 脚本级 ASR 测试可使用临时端口；WebUI 托管服务不配置默认端口，Start Service 时由 Bifrost 动态选择 loopback 空闲端口；Bifrost WebUI 使用临时端口，不使用 9900，不修改系统代理。
- 模型、二进制、样例音频固定存放在 `~/.bifrost/asr`，`--home` 或 API query 不得改变实际目录。

## 测试用例列表

### TC-QASR-01 Apple Silicon 与依赖检查

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-admin asr_download_requests_include_missing_runtime_model_and_samples --lib
   ```
2. 观察架构、内存、依赖检查输出。

预期结果：

- 输出包含 `platform ok`。
- release asset 为 `asr-macos-aarch64`。
- 如缺少 `ffmpeg`，初始化自检、WebUI Start Service 自检和 CLI 启动自检应自动安装；如 Homebrew 不可用或安装失败，必须明确失败并提示原因与 `brew install ffmpeg`。
- 非 macOS Apple Silicon 平台必须直接提示不支持该操作系统，不展示可用初始化入口。
- 不修改 `~/.zshrc`，不启动系统代理。

### TC-QASR-02 非交互安装 Qwen3-ASR-1.7B

操作步骤：

1. 执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-web.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18883 --unsafe-ssl --no-system-proxy
   ```
2. 检查安装产物：
   ```bash
   test -x ~/.bifrost/asr/qwen3_asr_rs/asr
   test -x ~/.bifrost/asr/qwen3_asr_rs/asr-server
   test -f ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/config.json
   test -f ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/tokenizer.json
   test -f ~/.bifrost/asr/qwen3_asr_rs/sample3.wav
   ```

预期结果：

- 命令无需交互选择模型，默认或显式安装 `Qwen3-ASR-1.7B`。
- 二进制、模型配置、tokenizer 和中文样例音频均存在。
- 重复执行安装命令会复用已下载文件，不重复破坏安装目录。

### TC-QASR-03 CLI 中文样例转写

操作步骤：

1. 执行：
   ```bash
   ~/.bifrost/asr/qwen3_asr_rs/asr \
     ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B \
     ~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
     chinese
   ```
2. 观察输出文本。

预期结果：

- 输出包含 `Language:` 和 `Text:`。
- 文本中包含 `Qwen3`、`语音` 或 `测试` 等中文 sample 关键词。
- 命令直接使用 MLX/Metal release binary，不依赖 CUDA/vLLM。

### TC-QASR-04 API server 健康检查与中文转写

操作步骤：

1. 启动本地 API server：
   ```bash
   ~/.bifrost/asr/qwen3_asr_rs/asr-server \
     --model-dir ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B \
     --host 127.0.0.1 \
     --port 8080 \
     --language chinese
   ```
2. 在另一个终端执行：
   ```bash
   curl -fsS http://127.0.0.1:8080/health
   curl -fsS http://127.0.0.1:8080/v1/models
   curl -fsS -X POST http://127.0.0.1:8080/v1/audio/transcriptions \
     -F file=@~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
     -F language=chinese \
     -F response_format=text
   ```
3. 测试完成后停止 `asr-server` 进程。

预期结果：

- `/health` 输出 `{"status":"ok"}`。
- `/v1/models` 输出包含 `qwen3-asr`。
- `/v1/audio/transcriptions` 返回中文文本，包含 `Qwen3`、`语音` 或 `测试` 等关键词。
- server 仅监听 `127.0.0.1` 指定端口，不修改系统代理。

### TC-QASR-05 长音频切片与批量转写

操作步骤：

1. 使用中文 sample 模拟切片输入：
   ```bash
   rm -rf /tmp/bifrost-qwen3-asr-chunks
   ffmpeg -y -hide_banner -loglevel error \
     -i ~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
     -f segment -segment_time 30 -c copy /tmp/bifrost-qwen3-asr-chunks/seg_%04d.wav
   ls /tmp/bifrost-qwen3-asr-chunks/seg_0000.wav
   ```
2. 在 API server 已启动时执行：
   ```bash
   curl -fsS -X POST http://127.0.0.1:8080/v1/audio/transcriptions \
     -F file=@/tmp/bifrost-qwen3-asr-chunks/seg_0000.wav \
     -F language=chinese \
     -F response_format=text > /tmp/bifrost-qwen3-asr-transcript.txt
   grep -E 'Qwen3|语音|测试' /tmp/bifrost-qwen3-asr-transcript.txt
   ```

预期结果：

- `chunk` 生成 `seg_0000.wav`。
- `batch-transcribe` 生成 transcript 文件。
- transcript 中包含中文 sample 关键词。

### TC-QASR-05B CLI 30 秒窗口 JSON Lines 输出

操作步骤：

1. 确保本地 API server 已启动。
2. 执行：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-qwen3-asr-cli-data \
     cargo run --bin bifrost -- ai asr stream-file ~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
     --language chinese \
     > /tmp/bifrost-qwen3-asr-stream.jsonl
   grep '"type": "segment"' /tmp/bifrost-qwen3-asr-stream.jsonl
   grep '"type": "final"' /tmp/bifrost-qwen3-asr-stream.jsonl
   ```

预期结果：

- 输出为 JSON Lines。
- 长音频输出多个 `segment` 事件和一个 `final` 事件，segment 事件包含 `start_ms`、`end_ms`、`text`；窗口按 30 秒最大送模、2 秒 overlap 推进。
- 短 sample 至少输出一个 `segment` 和一个 `final`，最终 `text` 包含中文 sample 关键词。

### TC-QASR-13 Bifrost CLI ASR 服务控制与单文件流式输出

操作步骤：

1. 使用临时 Bifrost 数据目录，确认 CLI 状态命令可读：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-cli.XXXXXX)" \
     cargo run --bin bifrost -- ai asr status --json
   ```
2. 在已初始化模型的机器上启动模型服务：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-qwen3-asr-cli-data \
     cargo run --bin bifrost -- ai asr start --model Qwen3-ASR-1.7B --language chinese
   ```
3. 查看状态：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-qwen3-asr-cli-data \
     cargo run --bin bifrost -- ai asr status --json
   ```
4. 对样例文件做标准输出流式转写：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-qwen3-asr-cli-data \
     cargo run --bin bifrost -- ai asr stream-file ~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
       --language chinese \
       > /tmp/bifrost-qwen3-asr-cli-stream.jsonl
   ```
5. 停止模型服务：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-qwen3-asr-cli-data \
     cargo run --bin bifrost -- ai asr stop
   ```

预期结果：

- `status --json` 输出包含 `ready` 和 `service` 字段。
- `start` 动态选择 loopback 端口，写入 `BIFROST_DATA_DIR/asr/service.json`，WebUI 刷新后能读取同一个服务状态。
- `stream-file` 标准输出为 JSON Lines，长音频包含 30 秒窗口的 `segment` 和最终 `final`；不要求用户手工部署 qwen3_asr_rs 代码，只复用固定目录里的 runtime/weights。
- `stop` 停止 pid 并删除 service state；停止后 WebUI 状态同步为 not ready。
- 缺失音频路径会返回明确错误，不启动模型服务。

### TC-QASR-06 AI Tools ASR 语音转换器状态、进度、错误与服务生命周期

操作步骤：

1. 确保没有旧 ASR server 残留进程；WebUI 托管服务不需要手动指定端口。
2. 使用临时数据目录启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-web.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18883 --unsafe-ssl --no-system-proxy
   ```
3. 在浏览器打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`。
4. 确认页面左侧 AI 导航包含 Tools 分组，且第一个工具项为 ASR；页面显示 Speech Converter 状态面板，点击 Refresh。
5. 点击 Initialize，观察初始化事件流；已安装时应快速进入 installed/done，不启动常驻模型服务。
6. 点击 Start Service，等待状态变为 Ready，并确认 Managed 显示为 Yes。
7. 刷新页面，确认状态仍然读取当前托管服务的动态端口，而不是回到固定端口。
8. 点击 Stop Service，观察状态变为 Not Ready，Server 字段显示动态端口将在启动时选择。

预期结果：

- AI -> Tools -> ASR 页面包含明确状态标签；未安装时只显示初始化下载进度条、当前资源、已下载体积、总体积、速度、预计剩余时间和错误详情，不显示日志面板。
- Initialize 启动后台初始化任务；刷新页面后重新进入 ASR 页面或重新连接初始化流时，仍能看到同一个后台任务的当前下载进度。已安装时不显示 Initialize 按钮，也不显示初始化进度模块。
- Start Service 由 Bifrost 后台动态选择空闲端口并启动 qwen3_asr_rs `asr-server`，Ready 后可转写；Stop Service 停止同一个托管进程以释放资源。
- Bifrost 服务启动后没有自动下载、加载或启动 ASR 模型；只有点击 Initialize / Start Service 后才出现对应事件。
- 页面 Storage 字段显示固定 `~/.bifrost/asr`，不可输入其它目录。
- 页面展示外部依赖提示：GitHub runtime 与 Hugging Face 权重下载不可达时会在下载进度区和错误区显示具体异常。
- 亮色和暗色主题下文字、进度条、错误详情均可读。

### TC-QASR-07 AI Tools ASR 文件输入 30 秒窗口转写

操作步骤：

1. 保持 TC-QASR-06 中 Bifrost WebUI 运行，并通过 Start Service 启动 ASR 托管服务。
2. 在浏览器打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`。
3. 点击 Choose File 选择 `~/.bifrost/asr/qwen3_asr_rs/sample3.wav`，或将该文件拖入上传区域。
4. 观察 `Speech to Text` 工作卡片中的 Audio Input、Transcript 和 stream events 区域。

预期结果：

- 页面显示单个 `Speech to Text` 工作卡片，Audio Input 输入模块位于卡片顶部，Transcript 转写模块位于同一卡片下方；顶层侧边栏不出现独立 Speech/ASR 入口。
- 上传文件后显示 `File transcription progress` 进度条并开始变化，stream events 至少包含 preflight、upload、preprocess、final、transcribe、done。
- 文件上传链路按 30 秒窗口、2 秒 overlap 顺序送模；短于 30 秒的 `sample3.wav` 可以只产生一个 final，长音频必须按多个 30 秒以内的 final segment 推进，不得 whole-file 一次性推给模型。
- Transcript 显示中文文本，包含 `Qwen3`、`语音` 或 `测试` 等 sample 关键词。
- ASR server 未 ready 时，页面提示在 AI -> Tools -> ASR 初始化，不吞掉错误，并展示当前 server 地址或健康检查错误。

### TC-QASR-08 WebUI 麦克风入口

操作步骤：

1. 保持 WebUI 打开 `/_bifrost/ai?aiSection=tools-asr`。
2. 点击 Start Mic。
3. 如果浏览器允许麦克风，录制 5-8 秒，期间观察 Audio Input 面板下方的音轨电平是否随声音输入波动，同时观察 stream events 是否出现 WebSocket connected/stream 事件并在 Stop Mic 前持续出现 partial/final；随后点击 Stop Mic。
4. 如果当前环境无麦克风或权限被拒绝，观察错误展示。

预期结果：

- 有麦克风权限时，按钮切换为 Stop Mic，WebUI 建立 `/api/asr/transcribe-ws` WebSocket，发送 `start` 控制帧和约 1 秒 timeslice 的二进制 `microphone.webm` 音频 chunk；后台通过 FFmpeg 标准化为 16kHz mono WAV，并进入实时转写流。
- Start Mic 后 Audio Input 面板显示 live input level 音轨，电平条和百分比随输入音量变化；Stop Mic、Cancel 或错误后音轨回到 0%，不继续占用麦克风分析资源。
- Start Mic 后不显示文件转写进度条或固定百分比处理进度；实时输入只展示麦克风电平音轨和 stream events。
- 录音过程中不需要等待 Stop Mic 才开始转写；stream events 应出现 `connected`/`stream` 并持续出现 partial/final，服务端 `stream` event detail 包含递增的 `processed_ms`。Stop Mic 后发送 `finish`，flush 尾段并回到 idle。
- 无麦克风或权限被拒绝时，页面显示 `Microphone capture failed` 以及浏览器返回的具体异常。
- 浏览器录音产物不得再直接导致 qwen3_asr_rs 返回 `Failed to open WAV file`；后续 MediaRecorder timeslice 即使不包含完整 WebM header，也不得被后端当作独立文件直接送 `ffmpeg -i` 解析。
- 麦克风错误不影响文件上传路径继续使用。

### TC-QASR-12 WebUI 麦克风实时电平音轨

操作步骤：

1. 保持 WebUI 打开 `/_bifrost/ai?aiSection=tools-asr`，ASR 状态为 Ready。
2. 确认 Audio Input 面板中上传区域和 Start Mic/Cancel 按钮下方显示一条 `Mic level` 音轨，未录音时所有电平条为低位且右侧为 `0%`。
3. 点击 Start Mic 并允许麦克风权限。
4. 对着麦克风说话或播放测试音频，观察音轨电平条与百分比。
5. 点击 Stop Mic，观察音轨状态。
6. 再次点击 Start Mic 后点击 Cancel，观察音轨状态。
7. 切换亮色/暗色主题后重复观察音轨可读性。

预期结果：

- 未录音时音轨稳定归零，不出现持续动画。
- 录音时电平条随输入音量实时波动，右侧百分比不固定为 0；静音时回落，说话或播放音频时升高。
- Stop Mic 和 Cancel 后音轨回到 `0%`，不会继续动画或保留上一次峰值。
- Web Audio 不改变原本 MediaRecorder / WebSocket 转写链路，stream events 仍能继续出现 `connected`/`stream`/`partial`/`final`。
- 亮色和暗色主题下电平条、边框、标签和百分比均可读。

### TC-QASR-14 WebUI ASR 目录定时任务与进度详情

操作步骤：

1. 使用临时数据目录启动 Bifrost：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-task.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18883 --unsafe-ssl --no-system-proxy
   ```
2. 准备一个本地音频目录，至少包含一个可转写音频文件和一个非音频文件；真实录音验证可使用 `~/Downloads/TX_MIC001_20260514_114433`，该目录 WAV 文件名和容器 tags 均包含录音开始时间，且包含一个 0 字节坏文件用于验证单文件失败不会中断任务。
3. 打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`。
4. 在 Directory Tasks 区域输入任务名、音频目录、递归开关；分别切换 Cycle 为 Daily、Weekly、Monthly，确认表单展示对应的时间、星期或日期控件；选择 Weekly Friday 09:15 后点击 Add。
5. 确认任务表展示目录、`Weekly Fri 09:15`、processed/pending/failed/deleted-after-processing 总体进度、下一次运行时间，页面不再出现秒级 interval 输入或 `Every Ns` 文案。
6. 点击任务行中的 View details，确认页面进入 `ai?aiSection=tools-asr&asrTask=<task_id>` 子页面，且页面没有 `dialog` / Drawer 弹窗；子页面顶部直接展示 tab 导航，第一个 tab 为 `Overview`，其中展示 schedule、last/next run、processed/pending/failed/deleted-after-processing、运行状态、音频占用和错误信息。
7. 切换到 `Files` tab，在任务详情子页面检查文件结果表格：页面整体不出现横向溢出，超宽文件路径和结果路径只在表格内部横向滚动；文件列表始终按 `Recorded` 时间倒排，最新录音/文件在最前面；分页默认显示 8 行，切换到 `20 / page` 后表格立即显示 20 行且选择器保持 `20 / page`。
8. 点击 Run 手动运行任务。
9. 运行完成后检查：
   ```bash
   find "$BIFROST_DATA_DIR/asr/data/text" -type f
   ```
10. 打开任务详情子页面后，切换到 Daily Docs tab，确认按 `YYYY-MM-DD` 展示日文档列表；点击某一天的 Open document，确认页面进入 `ai?aiSection=tools-asr&asrTask=<task_id>&asrDay=<YYYY-MM-DD>` 子页面，展示完整 Markdown 内容、文档路径、大小和更新时间，内容包含该日所有已转写文件的时间段文本；详情页正文不设置内部纵向滚动，长文档自然撑开页面并只使用最外层页面滚动条；点击 Back to daily docs 后回到任务详情并移除 `asrDay`。
11. 调用 API 验证按天文档：
   ```bash
   curl -s "http://127.0.0.1:18883/_bifrost/api/asr/tasks/<task_id>/daily"
   curl -s "http://127.0.0.1:18883/_bifrost/api/asr/tasks/<task_id>/daily/<YYYY-MM-DD>"
   curl -s -i "http://127.0.0.1:18883/_bifrost/api/asr/tasks/<task_id>/daily/../secret"
   ```
12. 打开任务详情子页面后，点击成功文件文件名旁的 Open transcript，确认页面进入 `ai?aiSection=tools-asr&asrTask=<task_id>&asrFile=<file_key>` 单文件详情页。单文件详情页顶部显示 Original Audio 播放器，播放器可读取 `/api/asr/tasks/<task_id>/files/<file_key>/source` 源音频；下方 File Timeline 按 `audio_start_ms/audio_end_ms` 音频相对时间和录音创建时间推算出的绝对时间展示分段文本，右侧 Full Transcript 展示完整合并文本。通过 timeline API 或页面 Segments 逐项确认 `audio_end_ms - audio_start_ms <= 30000`，不得出现几十分钟音频被合成一个 segment 的情况。点击任意 segment 的时间点，确认播放器跳转到对应音频时间；播放或拖动播放器到其它位置，确认 File Timeline 自动高亮并滚动到当前 segment；手动滚动字幕区后，确认自动滚动暂停，连续 5 秒没有新的滚动操作后恢复跟随当前播放段；暂停期间操作音频播放轴、点击播放或点击字幕时间点时，确认自动跟随立即恢复到用户指定位置。
11. 点击子页面左上角返回按钮，确认 URL 删除 `asrTask`，页面回到 Directory Tasks 上一级列表，后续仍可点击 Run 手动运行任务。
12. 删除一个已经处理过的源音频文件，刷新页面或等待 10 秒自动刷新。
13. 模拟服务重启后的 stale lock：在任务目录写入旧格式锁文件 `printf '' > "$BIFROST_DATA_DIR/asr/tasks/<task_id>/run.lock"`，再点击 Run。
14. 停止 ASR 模型服务后，创建两个启用的目录定时任务，Cycle 设置为 Daily 且时间为当前本地小时和分钟，分别绑定两个只包含一个音频文件的目录。
15. 等待 scheduler 自动触发，不点击 Run，观察两个任务都完成处理；随后查看 ASR 状态。

预期结果：

- WebUI 创建任务后无需离开 AI -> Tools -> ASR 即可看到任务详情和总体进度。
- 点击任务详情会进入 ASR 子页面而不是弹窗；URL 包含 `asrTask=<task_id>`，页面没有 `dialog` / Drawer，返回按钮可回到 Directory Tasks 上一级列表。
- 子页面可以查看逐文件执行结果；底部文件结果表格不会撑出 ASR 页面宽度，长路径通过表格内部横向滚动查看；分页大小切换后立即按新 page size 渲染并保持选择状态；Daily Docs tab 按天展示聚合文档，点击后进入 `asrDay=<YYYY-MM-DD>` 完整内容页，返回后不影响 Files tab 和 Open transcript 路径；成功文件展示输出文本路径和 timeline 路径；点击 Open transcript 后进入单文件详情页，顶部 Original Audio 播放器加载源音频，Timeline 阅读区按音频相对时间和绝对时间展示分段文本，并展示完整合并文本；每个 timeline segment 最大跨度不超过 30 秒，既覆盖新转写结果，也覆盖旧版本遗留超长单段 timeline 的读取兼容；点击时间点可跳转播放器位置，播放或拖动播放器时当前字幕段会自动高亮并滚动到可见区域；用户手动滚动字幕区时自动滚动暂停，最后一次手动滚动 5 秒后恢复跟随当前播放段；暂停期间用户操作音频播放轴、点击播放或点击字幕时间点会立即恢复自动跟随；失败文件展示错误信息，源文件删除后已完成记录仍保留。
- 录音创建时间优先从 `ffprobe` 的 `date + creation_time` 或 RFC3339 `creation_time` 解析，其次从文件名 `YYYYMMDD_HHMMSS`、filesystem birthtime、mtime 回退；坏文件仍记录可解析的创建时间和失败状态，不影响其它文件处理。
- 目录任务支持 hourly/daily/weekly/monthly 墙钟周期；WebUI 和 API 均提交 `schedule` 对象，不再提交 `interval_seconds`。
- 任务扫描音频目录时忽略非音频文件；递归开启时能发现子目录音频。
- 运行前检查模型服务状态：若模型已运行则复用并保持运行；若未运行则临时启动，运行完成后恢复为停止状态。
- 多个任务不会同时抢占模型服务 start/stop；竞争时显示明确错误，不覆盖其它任务状态。
- 服务重启或崩溃后遗留的旧格式、损坏或 pid 已不存在的 `run.lock` 会在下次运行前自动清理；真实仍在运行的任务仍会被拒绝并显示任务正在运行。
- 成功转写文本保存在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/`，同目录 `.timeline.json` 记录时间片段，`.json` 元数据记录源文件、任务 ID、模型、语言、创建时间来源和媒体时长；`daily/<YYYY-MM-DD>.md` 聚合该日完整整理内容。
- 源音频被删除后，已转写文本和元数据仍保留；任务表 `deleted after processing` 增加，不把已删除文件重新计入 pending。
- 如果同一路径后续出现大小或 mtime 不同的新音频文件，任务应把它视为新的 pending 文件，而不是复用旧 transcript。
- 启用状态的定时任务到期后会由后台 scheduler 自动执行；多个到期任务通过运行锁串行处理，不会并发 start/stop 同一个模型服务。
- 如果定时任务运行前模型服务是 stopped，任务可以临时启动模型，运行完成后恢复为 stopped；如果运行前模型服务已 ready，则任务复用服务并保持 ready。

### TC-QASR-16 ASR 定时任务 CLI 与按日文档检查

操作步骤：

1. 使用临时数据目录和临时端口启动当前 Bifrost 二进制，必须带 `--no-system-proxy`：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-asr-task-cli.XXXXXX)" \
     target/debug/bifrost start -p 18990 --unsafe-ssl --no-system-proxy
   ```
2. 通过 Admin API 创建一个绑定空音频目录、`enabled=false` 的 ASR 目录任务。
3. 在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/daily/2026-05-17.md` 写入一份 Markdown 文档。
4. 在同一个 `BIFROST_DATA_DIR` 下执行 CLI，不传 `-p`：
   ```bash
   target/debug/bifrost ai asr task list
   target/debug/bifrost ai asr task show <task_id>
   target/debug/bifrost ai asr task files <task_id>
   target/debug/bifrost ai asr task daily list <task_id>
   target/debug/bifrost ai asr task daily show <task_id> 2026-05-17
   target/debug/bifrost ai asr task daily show <task_id> 2026-05-17 --output /tmp/asr-day.md
   target/debug/bifrost ai asr task run <task_id> --wait
   ```
5. 再次执行 `target/debug/bifrost ai asr task daily list <task_id>`。

预期结果：

- 不传 `-p` 时，CLI 从当前 `BIFROST_DATA_DIR/runtime.json` 解析运行端口；读不到 runtime 时才回退默认 9900。
- `task list` 展示任务 ID、名称、状态和 processed/pending 汇总。
- `task show` 展示任务配置、summary、文件数量和 Daily documents 数量。
- 空任务的 `task files` 输出明确的空结果提示。
- `daily list` 展示 `2026-05-17`、文档路径、大小、字符数和更新时间。
- `daily show` 默认向 stdout 输出完整 Markdown；带 `--output` 时写入指定文件。
- `task run --wait` 在没有 pending 音频文件时不要求启动 ASR 模型服务，仍会触发后端刷新 daily 文档并等待任务结束。

### TC-QASR-11 API WebSocket 实时转写链路

操作步骤：

1. 保持 TC-QASR-06 中 Bifrost WebUI 运行，并通过 Start Service 启动 ASR 托管服务。
2. 准备浏览器麦克风格式音频：
   ```bash
   ffmpeg -y -hide_banner -loglevel error \
     -i ~/.bifrost/asr/qwen3_asr_rs/sample3.wav \
     -c:a libopus /tmp/bifrost-qwen3-asr-ws.webm
   ```
3. 使用 WebSocket 客户端连接：
   ```text
   ws://127.0.0.1:18883/_bifrost/api/asr/transcribe-ws?host=127.0.0.1&language=chinese&model=Qwen3-ASR-1.7B&flush_interval_ms=500
   ```
4. 发送文本帧 `{"type":"start","mime_type":"audio/webm","file_name":"e2e-microphone.webm"}`。
5. 将 `/tmp/bifrost-qwen3-asr-ws.webm` 切成至少 4 个片段，按顺序作为多个 binary frame 发送，模拟 `MediaRecorder.start(1000)` 连续 timeslice。
6. 发送文本帧 `{"type":"finish"}`，持续读取服务端事件直到收到 `done` 或连接关闭。

预期结果：

- WebSocket 握手返回 HTTP 101。
- 服务端事件顶层 `type` 包含 `connected`、`stream`、`partial`、`final`、`text`、`done`，事件 payload 为前端可直接消费的扁平 JSON；`connected`/`stream` 不得被统一折叠为 `progress`，否则视为实时 WebSocket 事件协议回归。
- `partial`/`final` 至少出现一次，`text` 或 `final.committed` 包含 `Qwen3`、`语音` 或 `测试` 等 sample 关键词。
- 服务端基于完整 WebM 会话转码并只切出新增音频片段，`stream` event detail 包含 `processed_ms`；不会因为第二个及后续 binary frame 缺少 WebM header 而返回 FFmpeg 解码错误。
- `finish` 后会触发 final flush；没有遗留临时音频文件，连接可正常关闭。
- 如果 ASR server 未 ready，服务端返回 `error` 事件，消息提示从 AI -> Tools -> ASR 启动模型服务。

### TC-QASR-09 临时缺失模型文件时展示下载进度

操作步骤：

1. 确保没有测试 ASR server 正在占用目标端口，并在 ASR 工具中先点击 Stop Service。
2. 临时移动一个已下载模型文件到备份目录：
   ```bash
   mkdir -p /tmp/bifrost-qwen3-asr-model-backup
   mv ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/model-00002-of-00002.safetensors \
      /tmp/bifrost-qwen3-asr-model-backup/
   ```
3. 在 AI -> Tools -> ASR 页面点击 Initialize。
4. 观察下载进度条、当前资源名称、已下载体积、总体积、速度和预计剩余时间。
5. 验证下载完成后文件恢复到固定模型目录：
   ```bash
   test -f ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/model-00002-of-00002.safetensors
   ```
6. 如下载失败或测试中断，手动恢复：
   ```bash
   mv /tmp/bifrost-qwen3-asr-model-backup/model-00002-of-00002.safetensors \
      ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/
   ```

预期结果：

- ASR 工具显示 `download` 阶段的单一进度条，不出现日志面板。
- 当前资源名称显示缺失的 safetensors 文件名。
- 进度条按 Rust 后台下载模块返回的 `downloaded_bytes / total_bytes / download_percent` 推进到 ready 或明确错误。
- 测试完成后固定目录中的模型文件完整恢复，不留下缺失文件。

### TC-QASR-15 启动服务自检自动修复缺失资源

操作步骤：

1. 停止 ASR 托管服务，并备份一个小模型文件：
   ```bash
   mkdir -p /tmp/bifrost-qwen3-asr-model-backup
   mv ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/config.json \
      /tmp/bifrost-qwen3-asr-model-backup/
   ```
2. 在 AI -> Tools -> ASR 页面直接点击 Start Service，或使用临时数据目录执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-cli.XXXXXX)" \
     cargo run --bin bifrost -- ai asr start --language chinese
   ```
3. 验证资源恢复后服务启动：
   ```bash
   test -f ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/config.json
   ```
4. 测试结束后停止服务；如中途失败，恢复备份文件。

预期结果：

- WebUI Start Service 和 CLI Start 都先执行自检，不因 `config.json` 缺失直接失败。
- 缺失资源通过 Rust 通用断点续传下载模块补齐，不调用已删除的仓库脚本。
- 如果 `ffmpeg` 缺失，自检自动尝试 Homebrew 安装；如果无法自动安装，错误信息包含 `brew install ffmpeg` 和重试说明。
- 非 macOS Apple Silicon 平台 WebUI/API/CLI 均直接提示 unsupported，不执行下载或启动模型服务。
- 测试结束后固定目录中的模型文件完整恢复。

### TC-QASR-20 Qwen3-ASR-0.6B 初始化下载绕过环境代理

操作步骤：

1. 确认 Hugging Face 上游 0.6B 配置文件真实可达：
   ```bash
   curl -I -L --max-time 20 \
     https://huggingface.co/Qwen/Qwen3-ASR-0.6B/resolve/main/config.json
   ```
2. 在故意设置无效环境代理的 shell 中执行 ASR 下载 client 回归测试：
   ```bash
   HTTP_PROXY=http://127.0.0.1:1 \
   HTTPS_PROXY=http://127.0.0.1:1 \
   ALL_PROXY=http://127.0.0.1:1 \
   NO_PROXY= \
   cargo test -p bifrost-admin asr_download_client_bypasses_proxy_env --lib
   ```
3. 执行 0.6B 下载请求清单回归：
   ```bash
   cargo test -p bifrost-admin asr_download_requests_include_qwen3_asr_0_6b_files --lib
   ```

预期结果：

- `Qwen/Qwen3-ASR-0.6B` 的 `config.json` 返回 200/307 后成功解析到真实文件，而不是 404。
- ASR 下载 client 在 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 指向不可达代理时仍能直连本地测试服务器，证明初始化下载不会被当前 shell 或 Bifrost 自身代理劫持。
- 0.6B 请求清单包含 `Qwen3-ASR-0.6B/config.json` 和 `Qwen3-ASR-0.6B/model.safetensors`。
- 本用例不下载完整 0.6B 权重；完整模型初始化仍由 TC-QASR-02 / TC-QASR-15 的真实初始化链路覆盖。

### TC-QASR-17 ASR 任务详情原音频占用与一键清理

操作步骤：

1. 使用临时数据目录启动 Bifrost，必须带 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-asr-source-cleanup.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy
   ```
2. 创建一个绑定临时音频目录的 ASR Directory Task，向该目录写入两个音频文件：一个模拟 `success`，一个模拟 `partial_success`。
3. 在 `BIFROST_DATA_DIR/asr/tasks/<task_id>/files.json` 写入对应 file records，并给 success/partial-success 都写入 transcript 与 timeline 产物。
4. 打开 `http://127.0.0.1:18886/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>`，查看任务详情页。
5. 确认页面展示 `Audio Files` 当前原音频总占用和 `Cleanable Originals` 可清理占用。
6. 点击 `Clean originals`，确认弹窗文案说明 transcript/timeline 会保留，且 partial-success 文件不会删除。
7. 确认清理后刷新任务详情，并检查磁盘文件：
   ```bash
   test ! -f "$AUDIO_DIR/done.wav"
   test -f "$AUDIO_DIR/partial.wav"
   test -f "$BIFROST_DATA_DIR/asr/data/text/<task_id>/done.txt"
   test -f "$BIFROST_DATA_DIR/asr/data/text/<task_id>/done.timeline.json"
   ```
8. 再次点击或调用清理接口：
   ```bash
   curl -fsS -X POST http://127.0.0.1:18886/_bifrost/api/asr/tasks/<task_id>/cleanup-source-audio
   ```

预期结果：

- 任务详情 summary 返回并展示 `audio_source_bytes/audio_source_file_count` 和 `cleanable_source_bytes/cleanable_source_file_count`。
- 清理前 `Audio Files` 包含两个仍存在的原音频，`Cleanable Originals` 只包含 success 文件。
- 清理接口只删除 `success + transcript/timeline 已存在 + audio_dir 内` 的源音频；`partial_success` 文件保留，避免破坏 failed chunk retry。
- transcript、timeline、metadata、daily docs 和 file store 记录都不被删除。
- 清理后 `Audio Files` 占用下降，`Cleanable Originals` 变为 0，`deleted_after_processing` 增加。
- 第二次清理幂等返回 `deleted_files=0`，不报错。

### TC-QASR-18 Daily Agent 配置与运行记录拆分为平级 tab

操作步骤：

1. 使用已有 ASR Directory Task 或临时任务打开：
   ```text
   http://127.0.0.1:<port>/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>
   ```
2. 点击任务详情页的 `Daily Agent` tab。
3. 检查该 tab 内容。
4. 点击同级 `Daily Agent Records` tab。
5. 如果存在 report 链接，点击某条 report；如果没有记录，使用测试 fixture 或 API 写入一条 processed document 后刷新。
6. 在 report 详情页点击返回。

预期结果：

- `Daily Agent` tab 只展示配置、IM Delivery、Last Run Status、Run Now/Force Run/Send Report/Refresh、AGENTS.md 指令编辑等配置和执行入口。
- `Daily Agent` tab 不展示 `Processed Documents` 或运行结果表。
- `Daily Agent Records` tab 展示 Daily Agent 已处理文档/运行结果表、report 链接和独立 Refresh。
- 点击 report 链接进入 `asrDailyReport=<date>` 详情时，URL 中 `asrTaskTab=daily-agent-records`；从详情返回后仍停留在 `Daily Agent Records` tab。
- 该拆分不影响 Run Now、Force Run、Send Report 和 AGENTS.md 保存。

### TC-QASR-19 Directory Tasks 首页位置前移

操作步骤：

1. 使用临时数据目录启动 Bifrost，必须带 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-asr-layout.XXXXXX)" \
     cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy
   ```
2. 打开 AI -> Tools -> ASR 首页：
   ```text
   http://127.0.0.1:18887/_bifrost/ai?aiSection=tools-asr
   ```
3. 在页面首屏确认 `Speech Converter`、`Directory Tasks` 和 `Speech to Text` 三个区域的垂直顺序。
4. 在 Directory Tasks 区域创建一个临时目录任务，点击 `View details` 进入任务详情，再返回 ASR 首页。
5. 点击任务列表中的 `Run`，确认排序调整不影响任务操作。

预期结果：

- `Directory Tasks` 位于 `Speech Converter` 下方、`Speech to Text` 上方，不再处于页面尾部。
- Directory Tasks 移动位置后仍能创建任务、进入详情、返回首页和手动 Run。
- 页面在亮色和暗色主题下区域标题与操作按钮均可读、无遮挡。

### TC-QASR-21 CLI ASR status 管道关闭回归

操作步骤：

1. 在 Apple Silicon macOS 上使用临时数据目录执行：
   ```bash
   CLI_DATA_DIR="$(mktemp -d /tmp/bifrost-qwen3-asr-pipe.XXXXXX)"
   BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai asr status --json | grep -q '"ready"'
   rm -rf "$CLI_DATA_DIR"
   ```
2. 在其它平台执行仓库离线结构 E2E：
   ```bash
   BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh
   ```

预期结果：

- Apple Silicon macOS 上 `grep` 命中 `"ready"` 后提前关闭管道时，`bifrost ai asr status --json` 不因 `Broken pipe` panic，整条管道退出码为 0。
- 非 Apple Silicon 平台仍保持不支持提示，不下载模型、不启动 ASR server。
- 修复不改变 `status --json` 的 JSON 字段，仍包含 `ready` 和 `service`。

### TC-QASR-22 Directory Task 重启后复用 persisted ASR server 并自动重试临时失败

操作步骤：

1. 使用默认数据目录的真实 Bifrost 服务和真实 ASR Directory Task，先确认任务存在可判定为临时服务获取失败的文件记录：
   ```bash
   curl -fsS http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>
   ```
2. 确认 `~/.bifrost/asr/service.json` 指向同一 `owner_module=directory_task`、同一 `owner_id=<task_id>` 的健康 ASR server，并确认 `/health` 返回 ok：
   ```bash
   curl -fsS http://127.0.0.1:<asr_port>/health
   ```
3. 只重启 Bifrost 本体，不停止 persisted ASR server：
   ```bash
   ./target/debug/bifrost stop
   BIFROST_DATA_DIR="$HOME/.bifrost" ./target/debug/bifrost start -p 9900 --host 0.0.0.0 --no-system-proxy --daemon
   ```
4. 再次查询任务详情，观察 failed/pending/running 变化：
   ```bash
   curl -fsS http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>
   ```
5. 等待任务进入 processing 后检查 `files.json` 或任务详情中的 `chunk_metrics`。

预期结果：

- Bifrost 重启后不会把同 owner、同模型、同 home 的 persisted ASR server 判定为 busy。
- 可判定为 `managed ASR server start failed: Qwen3-ASR service is busy` 的临时失败文件会从 `failed` 恢复为 `pending`，并在 scheduler startup 后自动进入运行，不需要等下一次墙钟调度。
- 可重试失败被恢复后，任务顶层 `last_error` 同步清空，不再在 UI/API 中保留旧的 `71 file(s) failed` 误报。
- 任务处理中的 chunk metric 使用 `runner=reuse_server` 且 `server_url` 指向 persisted server；不会再批量写入新的 `Qwen3-ASR service is busy` 错误。
- 普通非临时失败文件不会因重启被无限自动重试。

### TC-QASR-23 Daily Agent 不在 ASR 未完成时触发且中断报告不误显示 filesystem Runner

操作步骤：

1. 确认默认目录任务启用了 Daily Agent，且配置为 `trigger_policy=after_asr_run`、`runner=web`：
   ```bash
   curl -fsS http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent
   ```
2. 构造或复用一次 ASR run 结束后仍存在 `pending`、`failed`、`partial_success` 或 `failed_chunk_count` 的任务状态。
3. 观察 Daily Agent 不会因为这次不完整 ASR run 自动派发新的 `asr_completion`。
4. 对于已有 report 文件但 `daily_agent_processed.json` 中缺少 metadata 的日期，查询 Run Results：
   ```bash
   curl -fsS http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent/runs
   ```
5. 如果 Bifrost 曾在 Daily Agent 运行中重启，再查询 Daily Agent config。

预期结果：

- ASR summary 存在未完成或失败工作时，Daily Agent 不会自动基于不完整 daily markdown 生成报告。
- 重启后旧进程留下的 `last_status=running` 对外显示为 `interrupted`，不再误导为当前仍有 Daily Agent 正在跑。
- Run Results 中未索引 report 行的 `runner` 展示任务绑定的 runner（如 `web`），`last_run_id` 保持 `filesystem-scan`，用于区分“文件扫描补齐 metadata”与真实执行 run id。

### TC-QASR-24 Daily Agent 大 prompt 使用 ChatGPT Web 原生剪贴板投递

操作步骤：

1. 对默认目录任务或等价测试任务启用 Daily Agent，配置 `runner=web`，并确保 ChatGPT Web runner 已登录。
2. 准备包含 `AGENTS.md`、daily markdown 和历史 report 的大 prompt，使 run artifact 中的 `prompt.md` 超过 120 字符，建议覆盖 20KB 以上场景。
3. 触发 Daily Agent：
   ```bash
   curl -sS -X POST 'http://127.0.0.1:9900/_bifrost/api/asr/tasks/<task_id>/daily-agent/run?force=true'
   ```
4. 检查对应 ChatGPT Web run 目录的 `prompt.md`、`result.json`、`failure_diagnostics.json` 和服务日志。
5. 执行代码级回归，确认阈值、发送按钮轮询和平台粘贴快捷键：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin composer_text_injection --lib
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin native_clipboard_paste_uses_platform_modifier --lib
   ```

预期结果：

- 大 prompt 不通过 CDP `Input.insertText` 或完整 `Runtime.evaluate` 注入。
- ChatGPT Web adapter 通过系统剪贴板和浏览器原生粘贴快捷键提交大文本，日志可见 `native clipboard paste path`。
- ChatGPT 把大文本上传成粘贴文件时，composer 没有正文是正常状态；adapter 不采样 composer 文本。
- 粘贴后持续轮询发送按钮是否变为可发送状态，按钮可用后立即继续；长文档上传/解析慢时不走 Enter fallback 提前误发。
- Daily Agent run 不因 composer 注入超时失败，成功等待 `f/conversation` handoff、最终回复和 report 写入。

### TC-QASR-25 Daily Docs 单文档行级 Run Daily Agent

操作步骤：

1. 使用已有 ASR Directory Task 或临时任务打开 Daily Docs tab：
   ```text
   http://127.0.0.1:<port>/_bifrost/ai?aiSection=tools-asr&asrTask=<task_id>&asrTaskTab=daily
   ```
2. 确认表格至少存在一条 `YYYY-MM-DD.md` 日文档记录。
3. 点击目标日期行 `Action` 列中的 `Run Daily Agent`。
4. 通过浏览器网络请求或 Playwright route 记录确认提交的接口。
5. 点击同一行 `Open document`，确认原文档打开行为仍正常。

预期结果：

- `Run Daily Agent` 调用 `POST /_bifrost/api/asr/tasks/<task_id>/daily-agent/run?date=<YYYY-MM-DD>`，其中 `date` 等于当前行日期。
- 行级动作不携带 `force` 参数，保持 Daily Agent 普通增量语义。
- 请求提交期间该行按钮显示 loading，同一 task 下其它行的 `Run Daily Agent` 暂时不可重复点击。
- 页面出现 `Daily Agent run queued` 成功提示。
- `Open document` 仍进入 `asrDay=<YYYY-MM-DD>` 文档详情，Daily Docs tab URL 恢复行为不受影响。

### TC-QASR-26 转录完成后无损压缩源 WAV

操作步骤：

1. 确认不使用真实 `~/audio` 和当前 9900 服务；执行隔离的真实后端场景：
   ```bash
   bash e2e-tests/tests/test_asr_source_compression.sh
   ```
2. 脚本在临时 `BIFROST_DATA_DIR`、临时音频目录和临时端口中创建一个 Directory Task，注入三条已有转录记录：可解码的 `success` WAV、损坏的 `success` WAV、可解码的 `partial_success` WAV。
3. 通过真实 Admin API 调用 `POST /api/asr/tasks/<task_id>/compress-source-audio`，轮询 GET 同路径直到终态。
4. 比较正常 WAV 与生成 FLAC 解码为 `pcm_s32le` 后的 SHA-256；检查 `files.json` 对应记录主键、`source_path`、`source_compression`，并确认任务 summary 的 `pending`、节省空间和可压缩数量。
5. 执行聚焦浏览器验证：
   ```bash
   pnpm --dir web exec playwright test --grep "ASR task detail starts lossless source compression"
   ```

预期结果：

- 只有 `status=success` 且 transcript/timeline 都存在的普通 WAV 会进入队列；`partial_success`、failed、缺少产物、目录外文件和非 WAV 文件不会被压缩。
- 正常 WAV 仅在 FLAC 编码成功且解码 PCM SHA-256 完全一致后被替换；FLAC 不节省空间时保留 WAV。
- 损坏 WAV 或 ffmpeg/校验/落盘失败时原文件和原文件记录保持可用，不遗留 `.part`；原记录迁移失败时回滚，不出现半迁移状态。
- 压缩后的文件仍为原 ASR `success` 记录，transcript/timeline 保持不变，任务 `pending=0`，不会把 FLAC 当成新录音重新转录；重复文件引用、内容哈希索引和外接设备导入目标同步迁移。
- 状态 API 展示 processed/compressed/skipped/failed/saved bytes；WebUI 展示 Compressible WAV、Compression Saved、确认启动、进度/结果和取消入口。
- 压缩与 ASR run、failed-chunk retry、external import、清理和高风险任务配置互斥；取消在当前单文件安全结束后生效；重启遇到活动状态时显示 interrupted，并可再次启动恢复遗留备份。
- 测试只清理自身创建的临时目录和临时服务，不触碰 `~/audio`、默认数据目录或系统代理。

## 清理步骤

- 停止测试启动的 `asr-server` 进程。
- 停止测试启动的 Bifrost 进程。
- 删除临时切片和转写文件：
  ```bash
  rm -rf /tmp/bifrost-qwen3-asr-chunks /tmp/bifrost-qwen3-asr-transcript.txt /tmp/bifrost-qwen3-asr-web.* /tmp/bifrost-qwen3-asr-model-backup /tmp/bifrost-asr-source-cleanup.* /tmp/bifrost-asr-layout.* /tmp/bifrost-qwen3-asr-pipe.*
  rm -f /tmp/bifrost-qwen3-asr-stream.jsonl
  ```
- 保留 `~/.bifrost/asr` 模型目录供后续本地使用；不要在清理测试时删除固定模型目录。

## 执行记录

| 日期 | 用例 | 命令 | 实际结果 |
|------|------|------|----------|
| 2026-05-13 | TC-QASR-01 | legacy shell initializer, now removed | PASS：首次执行明确提示缺少 `ffmpeg`；执行 `brew update && brew install ffmpeg` 后复跑通过，输出 `Darwin arm64, memory 32GB, release asset asr-macos-aarch64` |
| 2026-05-13 | TC-QASR-02 | legacy shell initializer, now removed | PASS：下载 `asr-macos-aarch64`、`Qwen3-ASR-1.7B` 两片 safetensors 权重、tokenizer 与 sample；二次执行复用已有模型文件 |
| 2026-05-13 | TC-QASR-03 | legacy shell initializer, now removed | PASS：CLI 输出 `Language: forced`，文本为 `<asr_text>你好，这是宽增语音合成系统的持续集成测试。`，包含中文样例关键词 |
| 2026-05-13 | TC-QASR-04 | legacy shell initializer, now removed | PASS：`/health` 返回 `{"status":"ok"}`，`/v1/models` 返回 `qwen3-asr`，multipart 转写返回中文样例文本；退出时已清理 server |
| 2026-05-13 | TC-QASR-05 | `chunk` + `batch-transcribe` against `~/.bifrost/asr/qwen3_asr_rs/sample3.wav` on port `18082` | PASS：生成 `/tmp/bifrost-qwen3-asr-chunks/seg_0000.wav` 与 `/tmp/bifrost-qwen3-asr-transcript.txt`，transcript 包含 `语音/测试`；临时 server 已停止 |
| 2026-05-13 | 固定目录迁移 | `rsync -a ~/ai/asr/ ~/.bifrost/asr/` | PASS：固定目录 `~/.bifrost/asr` 已包含 qwen3_asr_rs、Qwen3-ASR-1.7B 权重、server 二进制和 sample，大小 4.6G |
| 2026-05-13 | TC-QASR-06 | Playwright 清理 `bifrost.asr.connection*` 后打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`，点击 Start Service，刷新状态 | PASS：页面不显示默认端口；停止态显示 `Dynamic port, selected when service starts`；Start Service 后 Ready 且 Server 为动态端口 `http://127.0.0.1:53014`（端口每次可不同）；Managed 为 Yes |
| 2026-05-13 | TC-QASR-07 | Playwright 打开 AI -> Tools -> ASR，不设置端口，选择 `~/.bifrost/asr/qwen3_asr_rs/sample3.wav` | PASS：Transcript 输出 `<asr_text>你好，这是宽增语音合成系统的持续集成测试。`；stream events 包含 preflight、upload、preprocess、transcribe、done；无需用户指定端口 |
| 2026-05-13 | TC-QASR-08 | Playwright 使用 fake microphone 权限打开 AI -> Tools -> ASR，点击 Start Mic / Stop Mic | PASS：按钮切换到 Stop Mic，停止后生成 `microphone.webm`；后台显示 `preprocess: Audio normalized to 16 kHz mono WAV.`；模型返回 `<asr_text>嗯。`；不再出现 `Failed to open WAV file` |
| 2026-05-13 | TC-QASR-09 | `mv ~/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/config.json /tmp/bifrost-qwen3-asr-model-backup/` 后在 AI -> Tools -> ASR 点击 Initialize | PASS：历史页面曾显示日志和 curl 进度；当前预期已改为 Rust 后台下载进度条与体积/速度/ETA 展示 |
| 2026-05-13 | TC-QASR-01 / CI 回归 | `PATH=/usr/bin:/bin BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：离线 CI 模式下缺少 `ffmpeg` 时脚本输出 `missing required command: ffmpeg`，随后明确跳过在线模型段并以 0 退出；在线模式仍要求 preflight 成功 |
| 2026-05-14 | TC-QASR-01 / 结构回归 | `BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：脚本语法、help、缺失音频错误、preflight 均通过；在线段按环境变量跳过 |
| 2026-05-14 | TC-QASR-10 / CI 禁止模型下载部署 | `CI=true BIFROST_QWEN3_ASR_E2E_ONLINE=1 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：E2E 输出 `CI environment detected; online model verification skipped` 后退出 0；验证前后无 ASR server 监听端口，未触发模型下载、安装或 `asr-server` 启动 |
| 2026-05-14 | TC-QASR-03 / TC-QASR-04 / TC-QASR-05B / TC-QASR-07 / TC-QASR-08 / 错误回归 | `BIFROST_QWEN3_ASR_E2E_ONLINE=1 BIFROST_QWEN3_ASR_E2E_PORT=18084 BIFROST_QWEN3_ASR_E2E_ADMIN_PORT=18884 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：真实模型 CLI/API 样例通过；Bifrost 托管服务动态端口 ready；`/api/asr/transcribe-stream` 对 sample3.wav 输出 3 个 partial 和 3 个 final 后输出最终文本；`stream-file` 使用托管动态端口输出多个 partial/final JSON Lines；WebM 麦克风格式归一化输出 partial/final/text；Stop Service 后未启动服务错误流可见 |
| 2026-05-14 | TC-QASR-07 WebUI 文件输入流式转写 | Playwright 打开 `http://127.0.0.1:18883/_bifrost/ai?aiSection=tools-asr`，选择 `~/.bifrost/asr/qwen3_asr_rs/sample3.wav` | PASS：页面实际渲染 `partial[` 3 次、`final[` 3 次，Transcript 匹配 `你好/语音/测试`；测试结束后调用 Stop Service 并停止临时 Bifrost 进程 |
| 2026-05-14 | TC-QASR-11 API WebSocket 事件协议回归 | `cargo test -p bifrost-admin test_ws_progress_event_type_uses_visible_realtime_phases`；`BIFROST_QWEN3_ASR_E2E_ONLINE=1 BIFROST_QWEN3_ASR_E2E_PORT=18084 BIFROST_QWEN3_ASR_E2E_ADMIN_PORT=18884 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：服务端顶层 `type` 直接输出 `connected`、`stream`、`partial`、`final`、`text`、`done`，WebUI 同时兼容这些实时事件和普通 SSE progress 事件；回归覆盖浏览器 WebSocket 无法携带 Authorization header 时通过 query token 鉴权 |
| 2026-05-14 | TC-QASR-05B / TC-QASR-08 1 秒流式延迟回归 | `cargo test -p bifrost-admin asr --lib`；`BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：默认流式窗口从 2000ms 调整为 1000ms，默认 overlap 为 300ms，WebSocket 默认 flush interval 为 800ms；结构 E2E 覆盖脚本语法、help、缺失音频错误、preflight 和 CI/离线跳过路径 |
| 2026-05-14 | TC-QASR-08 / TC-QASR-12 WebUI 麦克风 1 秒输入与电平音轨 | `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts` | PASS：真实浏览器打开 AI -> Tools -> ASR，mock ASR ready、麦克风、MediaRecorder 和 WebSocket；Start Mic 后 `Live microphone level` 电平变化，MediaRecorder interval 包含 `1000`，partial 事件为 `0-800ms`，Stop Mic / Cancel 后电平回到 `0%`，暗色主题下可读 |
| 2026-05-14 | TC-QASR-13 / 结构回归 | `BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh`；`cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands`；`cargo test -p bifrost-cli asr --lib` | PASS：`bifrost ai asr` help/status/缺失音频错误路径通过；CLI 子命令解析覆盖 `stream-file`；共享 service state 读写测试通过 |
| 2026-05-14 | TC-QASR-14 / WebUI 目录任务面板 | `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts`；`cargo test -p bifrost-admin asr_jobs --lib` | PASS：WebUI Directory Tasks 可创建 Daily 墙钟周期任务、提交 `schedule` 而非 `interval_seconds`、展示 processed/pending/deleted-after-processing 进度、点击 View details 打开任务详情并展示逐文件 success/output path/recorded time/duration/timeline、自动展示第一个成功文件的 File Timeline 阅读区、点击 Open timeline 可重新加载分段时间轴和 Full Transcript 完整文本；单测覆盖 daily/weekly/monthly next_run、真实录音文件名时间解析、timeline 文本渲染、递归音频发现、详情 files、输出目录、源文件删除后保留已处理元数据 |
| 2026-05-14 | TC-QASR-03 / TC-QASR-04 / TC-QASR-05B / TC-QASR-13 / TC-QASR-14 在线回归 | `BIFROST_QWEN3_ASR_E2E_ONLINE=1 BIFROST_QWEN3_ASR_E2E_PORT=18084 BIFROST_QWEN3_ASR_E2E_ADMIN_PORT=18884 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：复用固定 `~/.bifrost/asr` 模型目录；真实模型 CLI/API 样例通过；`bifrost ai asr` 结构路径通过；Admin ASR task API 创建 pending=1；目录任务 Run 输出 `processed_now=1`，在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/` 写入 `.txt` 和 `.json`，删除源音频后详情显示 `deleted_after_processing=1`；WebM 与 WebSocket 实时链路仍通过 |
| 2026-05-14 | TC-QASR-13 CLI 真实服务链路 | 临时 `BIFROST_DATA_DIR` 下执行 `cargo run --quiet --bin bifrost -- ai asr start --language chinese` -> `status --json` -> `stream-file ~/.bifrost/asr/qwen3_asr_rs/sample1.wav --language chinese` -> `stop` -> `status --json` | PASS：CLI 动态端口启动真实 Qwen3-ASR 服务，`status --json` 显示 `ready: true` 与 `managed_by: cli`；`stream-file` 标准输出包含 `partial` 和 `final` JSON Lines，窗口按 1000ms 推进；`stop` 后 `status --json` 显示 `ready: false` |
| 2026-05-14 | TC-QASR-14 定时任务自动触发与冲突恢复 | `cargo test -p bifrost-admin task_run_lock_rejects_concurrent_runs_and_releases_after_drop --lib`；`BIFROST_QWEN3_ASR_E2E_ONLINE=1 BIFROST_QWEN3_ASR_E2E_PORT=18085 BIFROST_QWEN3_ASR_E2E_ADMIN_PORT=18885 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：单测确认同一 task `run.lock` 拒绝并发运行且释放后可再次运行；online E2E 在服务 stopped 后创建两个 enabled Daily 当前分钟墙钟任务，未点击 Run，由 scheduler 自动触发并串行完成，两个任务均写入 `asr/data/text/<task_id>/`，最终 `/api/asr/status` 为 `ready:false`，证明任务完成后恢复 stopped 状态 |
| 2026-05-14 | TC-QASR-14 / 真实录音元信息可行性 | `ffprobe` + `stat` 读取 `~/Downloads/TX_MIC001_20260514_114433` | PASS：16 个 WAV 中 15 个可解码为 48kHz mono 24-bit PCM，多数 30 分钟分段；WAV tags 含 `date=2026-05-14` 和 `creation_time=HH:MM:SS`，文件名 `YYYYMMDD_HHMMSS` 与 filesystem birth/mtime 仅差 0-1 秒；`TX02_MIC007_20260514_124809_orig.wav` 为 0 字节坏文件，应记录 failed file record 而不阻塞其它文件 |
| 2026-05-14 | TC-QASR-14 / 真实单文件目录任务与 WebUI 时间轴查阅 | 复制 `~/Downloads/TX_MIC001_20260514_114433/TX02_MIC008_20260514_131410_orig.wav` 到临时目录 `/tmp/bifrost-asr-one-file.*`；临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-one-file-webui.* cargo run --bin bifrost -- start -p <dynamic> --unsafe-ssl --no-system-proxy`；API 创建 `real-one-file-validation` Directory Task 并 POST `/api/asr/tasks/<id>/run`；Browser 打开 `/_bifrost/ai?aiSection=tools-asr` 查看任务详情 | PASS：任务结果 `discovered=1 processed=1 pending=0 failed=0`；文件详情 `status=success`、`source_created_at_source=ffprobe.date_creation_time`、`media_duration_ms=3633`、`text_chars=2`，输出 `.txt` 和 `.timeline.json` 均在 `BIFROST_DATA_DIR/asr/data/text/<task_id>/`；timeline API 返回 1 个 segment，`audio_start_ms=0 audio_end_ms=975 text=嗯。`，落盘文本为 `[2026-05-14 13:14:10.000 - 2026-05-14 13:14:10.975] 嗯。`；WebUI 任务详情自动展示 File Timeline 和 Full Transcript，能看到 `ffprobe.date_creation_time`、`00:00:00.000 - 00:00:00.975`、`嗯。` |
| 2026-05-14 | TC-QASR-07 / TC-QASR-08 / TC-QASR-12 WebUI 单卡片输入转写布局 | `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts` | PASS：真实浏览器打开 AI -> Tools -> ASR，断言 Audio Input 和 Transcript 位于同一个 `Speech to Text` 工作卡片内，Audio Input 在顶部、Transcript 在下方；初始和麦克风实时输入时不显示文件进度条，选择文件后才显示 `File transcription progress`；麦克风电平、1 秒 MediaRecorder timeslice、partial/final 事件、Stop/Cancel 归零、暗色主题和 Directory Tasks 面板仍通过 |
| 2026-05-15 | TC-QASR-01 / TC-QASR-06 / TC-QASR-15 初始化与启动自检 | 临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-real-web.* cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy --skip-cert-check`；`curl -N /_bifrost/api/asr/init-stream`；`curl -X POST /_bifrost/api/asr/service/start` | PASS：服务使用临时数据目录启动且系统代理保持 disabled；`/api/asr/status` 返回 `installed=true/platform_supported=true/ffmpeg_available=false` 后，初始化流执行 `Checking ASR runtime dependencies` 并通过软件内 `brew install ffmpeg` 自动安装 FFmpeg；随后 `ffmpeg version 8.1.1` 可用，Start Service 动态端口 `http://127.0.0.1:65128` ready |
| 2026-05-15 | TC-QASR-07 WebUI 文件输入真实转写 | Browser 打开 `http://127.0.0.1:18891/_bifrost/ai?aiSection=tools-asr`；Playwright 在真实页面选择 `~/.bifrost/asr/qwen3_asr_rs/sample3.wav` | PASS：页面显示 `Ready`，不显示 `Initialize`；真实文件上传经过 preflight/upload/preprocess/stream，页面出现 6 个 `partial[]` 和 6 个 `final[]`，最终 `done: Transcription completed.`；Transcript 包含 `你好/持续集成/测试`；无 console error；截图保存到 `/tmp/bifrost-asr-webui-real-transcribe-done.png` |
| 2026-05-15 | TC-QASR-13 CLI 真实服务链路 | 临时 `BIFROST_DATA_DIR=/tmp/bifrost-asr-real-cli.* cargo run --quiet --bin bifrost -- ai asr start --language chinese` -> `status --json` -> `stream-file ~/.bifrost/asr/qwen3_asr_rs/sample3.wav --language chinese` -> `stop` -> `status --json` | PASS：CLI 动态端口 `http://127.0.0.1:49168` 启动真实 Qwen3-ASR 服务，`status --json` 显示 `ready:true/managed_by:cli`；`stream-file` 输出 partial/final JSON Lines，文本为 `你好，这是宽增语音合成系统的持续集成测试。`；`stop` 后状态为 `ready:false/service:null` |
| 2026-05-15 | TC-QASR-15 / CI 非支持平台回归 | `CI=true BIFROST_QWEN3_ASR_E2E_ONLINE=0 e2e-tests/tests/test_qwen3_asr_local_server.sh`；GitHub CI `E2E Shell (Linux, shard 2/3)` 失败日志 | PASS：脚本在 macOS Apple Silicon 本机离线路径只验证 help、status 和缺失音频错误后跳过在线模型；CI 失败归因为 Linux 预期 unsupported 被脚本当成失败，已改为非 `Darwin-arm64` 平台断言 `only supported on Apple Silicon macOS` 后退出 0，不下载、不启动模型 |
| 2026-05-15 | TC-QASR-14 / Directory Task 子页面回归 | `pnpm --dir web run build`；`SKIP_FRONTEND_BUILD=1 TMPDIR=/tmp/bifrost-ui-tmp.4u2OEk BIFROST_UI_TEST_TARGET_DIR=/tmp/bifrost-ui-target.zCjdVd pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"` | PASS：生产构建通过；Playwright 真实浏览器创建 Directory Task 后点击 View details，URL 包含 `asrTask=task-1`，页面没有 `Directory Task: Recordings` dialog，`asr-task-detail-page` 展示任务 summary、逐文件 output text path、status、File Timeline 和 Full Transcript；点击 Open timeline 后可见 `ffprobe.date_creation_time`、`00:00:00.000 - 00:00:02.000` 与 transcript；点击返回按钮后 URL 删除 `asrTask` 并回到 Directory Tasks 上一级列表，随后 Run 仍可执行 |
| 2026-05-15 | TC-QASR-14 / 单文件转写详情与 stale lock 回归 | `pnpm --dir web run build`；`pnpm --dir web exec eslint src/pages/ASR/index.tsx src/pages/ASR/asrUtils.ts src/pages/ASR/components/SpeechWorkbench.tsx src/pages/ASR/components/DirectoryTasksPanel.tsx src/pages/ASR/components/DirectoryTaskDetailPage.tsx src/pages/ASR/components/TaskFileTranscriptPage.tsx tests/ui/asr-microphone-meter.spec.ts`；`SKIP_FRONTEND_BUILD=1 TMPDIR=/tmp/bifrost-ui-tmp.asr-split BIFROST_UI_TEST_TARGET_DIR=/tmp/bifrost-ui-target.asr-split pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"`；`SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --lib`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin task_run_lock --lib` | PARTIAL PASS：生产构建、定向 ESLint、`cargo check -p bifrost-admin --lib` 和 Playwright 通过；Playwright 覆盖任务详情无 dialog、Open transcript 进入 `asrFile=file-1` 单文件详情、Original Audio 播放器使用 `/source` URL、File Timeline 展示两段文本、点击 `00:00:01.000 - 00:00:02.000` 后播放器 `currentTime` 跳到 1000ms、返回任务文件列表和 Directory Tasks；`cargo test -p bifrost-admin task_run_lock --lib` 被当前工作树既有 IM Gateway 测试编译错误阻塞（`ExternalCliRunRequest` 缺 `runner_id`，另有 chatgpt_web browser dead_code warning），未能执行到新增 lock 单测 |
| 2026-05-15 | TC-QASR-14 / 单文件转写详情双向时间轴绑定 | `pnpm --dir web exec eslint src/pages/ASR/components/TaskFileTranscriptPage.tsx tests/ui/asr-microphone-meter.spec.ts`；`pnpm --dir web exec tsc -b --pretty false`；`SKIP_FRONTEND_BUILD=1 TMPDIR=/tmp/bifrost-ui-tmp.asr-follow BIFROST_UI_TEST_TARGET_DIR=/tmp/bifrost-ui-target.asr-follow pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"` | PASS：Playwright 真实浏览器创建 Directory Task 并进入 `asrFile=file-1` 单文件详情；点击 `00:00:01.000 - 00:00:02.000` 后播放器 `currentTime` 跳到 1000ms 且第 2 段 `aria-current=true`；模拟播放器 `timeupdate` 到 10 秒后，第 11 段 `aria-current=true` 且字幕滚动容器 `scrollTop > 0`；模拟手动滚轮和滚动字幕区后再次 `timeupdate` 到 10 秒，第 11 段继续高亮但 300ms 内 `scrollTop` 保持 0；在暂停窗口内模拟音频 `seeking/seeked` 到 5 秒，第 6 段 `aria-current=true` 且 1 秒内 `scrollTop > 0`，验证用户操作播放轴立即恢复自动跟随；再次手动滚动后等待自动恢复，`scrollTop > 0`；验证点击字幕和播放/拖动音频双向绑定、手动滚动暂停自动跟随并在 5 秒后恢复均生效 |
| 2026-05-18 | TC-QASR-14 / 任务详情文件表格布局与分页大小回归 | 仅启动前端 dev server：`WEB_PORT=3000 BACKEND_PORT=9900 pnpm --dir web run dev`；Playwright 直连 `http://127.0.0.1:3000/_bifrost/ai?aiSection=tools-asr&asrTask=76612de33e9740bc92440ce64a98a4cb`，复用现有 9900 后端，不重启 Bifrost | PASS：页面整体 `documentElement.scrollWidth == clientWidth`，任务文件表 `.ant-table-content` 为 `overflow-x: auto`，表格内部 `scrollWidth=1370 > clientWidth=823`；默认显示 8 行和 `8 / page`，切换 `20 / page` 后显示 20 行且选择器保持 `20 / page` |
| 2026-05-18 | TC-QASR-14 / ASR timeline segment 最大 30 秒回归 | 复现：`curl -s http://127.0.0.1:3000/_bifrost/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb/files/8dc2c4875e95b4c8aac6b131fd9e2fed5a33aef8/timeline` 显示旧服务返回 1 个 segment，`audio_end_ms - audio_start_ms = 230015`；修复后执行 `cargo test -p bifrost-admin asr_jobs --lib` 覆盖 chunk plain-text fallback 与旧 timeline 读取兼容拆分 | PASS：单测证明目录任务原生 CLI 只返回纯文本时会按 30 秒 chunk window 合成多个 timeline segments，且 timeline 读取会把旧版本遗留的超长单段拆成最大 30 秒窗口；当前运行中的 9900 服务需重启/升级后才会暴露新 API 行为，已有旧转写文件无需强制重跑即可在读取时被兼容拆分 |
| 2026-05-18 | TC-QASR-05B / TC-QASR-07 非实时链路 30 秒窗口回归 | `cargo test -p bifrost-admin asr::tests --lib`；`cargo test -p bifrost-admin asr_streaming::tests --lib`；`cargo test -p bifrost-admin asr_ws::tests --lib`；`cargo test -p bifrost-cli commands::asr::tests --lib`；`target/debug/bifrost ai asr stream-file ~/Downloads/we/TX01_MIC007_20260514_183241_orig.wav --model Qwen3-ASR-1.7B --language chinese` | PASS：WebUI 文件上传服务端 chunk planner 对 180.015s 音频生成 30 秒窗口、2 秒 overlap，短音频保留单窗口；CLI 对 1801s 真实录音输出 `Split into 65 chunks (30s each, 2s overlap)`，总耗时 real 216.77s、RTF 0.117；WebSocket/mic 测试保持 1 秒实时窗口和 800ms flush，不纳入 30 秒批处理窗口 |
| 2026-05-19 | TC-QASR-16 / ASR 定时任务 CLI 与按日文档检查 | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli ai_asr_commands_parse --test cli_commands`；`e2e-tests/tests/test_asr_task_cli.sh`；`target/debug/bifrost ai asr task show 76612de33e9740bc92440ce64a98a4cb`；`target/debug/bifrost ai asr task daily list 76612de33e9740bc92440ce64a98a4cb`；`target/debug/bifrost ai asr task daily show 76612de33e9740bc92440ce64a98a4cb 2026-05-17` | PASS：CLI 解析覆盖 `task daily show --output`；E2E 使用临时 `BIFROST_DATA_DIR` 和 runtime port 验证不传 `-p` 的 `task list/show/files/daily list/daily show/run --wait`，且无 pending 文件时 `run --wait` 不要求 ASR 模型；真实 9900 任务显示 `Daily documents: 4`，daily list 展示 2026-05-14 到 2026-05-17 四份 Markdown，2026-05-17 完整内容可从 stdout 读取 |
| 2026-05-20 | TC-QASR-17 / ASR 任务详情原音频占用与一键清理 | `e2e-tests/tests/test_asr_task_cli.sh`；`pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"` | PASS：E2E 使用临时 `BIFROST_DATA_DIR` 和临时音频目录，通过真实 Admin API 构造 success/partial_success 文件记录，确认任务详情 summary 返回 `audio_source_bytes=done+partial`、`cleanable_source_bytes=done`；调用 `POST /cleanup-source-audio` 后 success 源音频删除、partial_success 源音频保留、transcript/timeline 产物保留，二次调用 `deleted_files=0`。Playwright 真实浏览器验证任务详情展示 `Audio Files` 和 `Cleanable Originals`、点击 `Clean originals` 确认后占用下降并保留后续任务详情能力。 |
| 2026-05-20 | TC-QASR-18 / Daily Agent 配置与运行记录拆分为平级 tab | `pnpm --dir web exec playwright test tests/ui/asr-daily-agent-runner.spec.ts`；`pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"` | PASS：Daily Agent Runner 专项套件验证 `Daily Agent` tab 仍可编辑 AGENTS.md、选择 Runner/IM Channel，并确认 processed report 从 `Daily Agent Records` tab 打开；目录任务 Playwright 回归验证 `Daily Agent` tab 不再展示 `Processed Documents`，`Daily Agent Records` tab 展示 `Run Results` 和 report 链接，点击 report 后 URL 包含 `asrTaskTab=daily-agent-records&asrDailyReport=2026-05-14`，返回后仍停留在 records tab。 |
| 2026-05-20 | TC-QASR-19 / Directory Tasks 首页位置前移 | `pnpm --dir web exec playwright test tests/ui/asr-microphone-meter.spec.ts --grep "ASR directory tasks"` | PASS：Playwright 真实浏览器验证 AI -> Tools -> ASR 首页中 `Speech Converter` 位于 `Directory Tasks` 上方，`Directory Tasks` 位于 `Speech to Text` 上方；随后继续创建 Directory Task、进入任务详情、执行原音频清理、验证 Daily Agent Records，不影响既有目录任务操作链路。 |
| 2026-05-20 | TC-QASR-20 / Qwen3-ASR-0.6B 初始化下载绕过环境代理 | `curl -I -L --max-time 20 https://huggingface.co/Qwen/Qwen3-ASR-0.6B/resolve/main/config.json`；`HTTP_PROXY=http://127.0.0.1:1 HTTPS_PROXY=http://127.0.0.1:1 ALL_PROXY=http://127.0.0.1:1 NO_PROXY= cargo test -p bifrost-admin asr_download_client_bypasses_proxy_env --lib`；`cargo test -p bifrost-admin asr_download_requests_include_qwen3_asr_0_6b_files --lib` | PASS：Hugging Face 返回 `307` 并带 `x-repo-commit: 5eb144179a02acc5e5ba31e748d22b0cf3e303b0`，确认 `config.json` 真实存在；无效代理环境下 direct reqwest 下载 client 仍能访问本地测试 HTTP server；0.6B 请求清单包含 `Qwen3-ASR-0.6B/config.json` 与 `Qwen3-ASR-0.6B/model.safetensors` |
| 2026-05-21 | TC-QASR-21 / CLI ASR status 管道关闭回归 | `cargo test -p bifrost-cli asr_status_output --lib`；`BIFROST_QWEN3_ASR_E2E_ONLINE=0 bash e2e-tests/tests/test_qwen3_asr_local_server.sh` | PASS：单测覆盖 stdout `BrokenPipe` 被视为下游管道关闭且其它 IO 错误继续返回；离线结构 E2E 在当前平台通过，未下载模型、未启动 ASR server。 |
| 2026-05-22 | TC-QASR-22 / 默认目录真实重启恢复 | `cargo build --bin bifrost`；`./target/debug/bifrost stop`；`BIFROST_DATA_DIR="$HOME/.bifrost" ./target/debug/bifrost start -p 9900 --host 0.0.0.0 --no-system-proxy --daemon`；查询 `/api/asr/tasks/76612de33e9740bc92440ce64a98a4cb` 与 `files.json` | PASS：重启前任务有 71 条 `managed ASR server start failed: Qwen3-ASR service is busy`；重启后自动恢复为 `pending=71 failed=0 running=true`，首个文件进入 `processing`，`chunk_metrics` 最近记录均为 `runner=reuse_server status=ok`，`files.json` 当前 `error_count=0 busy_errors=0`；补充单测验证恢复可重试失败时 task 顶层 `last_error` 会同步清空，非可重试失败仍保留错误。 |
| 2026-05-22 | TC-QASR-23 / Daily Agent incomplete ASR gate 与未索引 report runner 展示 | `cargo test -p bifrost-admin daily_agent --lib`；默认 9900 查询 `/daily-agent` 和 `/daily-agent/runs` | PASS：单测覆盖 ASR summary 存在 pending/failed/partial/failed chunks 时不允许 after_asr_run 自动触发、stale running 对外转 interrupted、未索引 report 使用任务绑定 runner；重启最新二进制后默认 9900 显示 `last_run.status=interrupted`，2026-05-18/19 `last_run_id=filesystem-scan` 且 `runner=web`。 |
| 2026-05-26 | TC-QASR-24 / Daily Agent 大 prompt 原生剪贴板投递 | `cargo build --bin bifrost`；`BIFROST_DATA_DIR=$HOME/.bifrost ./target/debug/bifrost start -p 9900 --host 0.0.0.0 --no-system-proxy --daemon`；`./target/debug/bifrost -p 9900 agent run --runner web --session chatgpt-web-native-clipboard-no-sample-20260526 --json "$(cat /Users/eden/.bifrost/asr/data/text/76612de33e9740bc92440ce64a98a4cb/.daily/2026-05-19.md)"`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin composer_text_injection --lib`；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin native_clipboard_paste_uses_platform_modifier --lib` | PASS：阈值保持 120 字符，120 以内走 `Input.insertText`，121 及以上走 `NativeClipboardPaste`；macOS 粘贴 modifier 为 Meta。真实默认目录 ChatGPT Web live run 使用 2026-05-19 daily Markdown 生成 457840 字节 prompt，run `1779727870753-c7feafc8-3173-43c8-8462-014e2b7409b1` 成功，日志 `/tmp/bifrost-chatgpt-web-no-sample-20260526005110.log` 返回 `收到文件《粘贴的文本 (1)(3).txt》`；这验证了粘贴完成后 ChatGPT 文件化且 composer 无正文时不再采样文本，adapter 通过轮询发送按钮可用状态完成发送与最终回复。 |
| 2026-08-04 | TC-QASR-26 / 转录完成后无损压缩源 WAV | `bash e2e-tests/tests/test_asr_source_compression.sh`；`pnpm --dir web exec playwright test --grep "ASR task detail starts lossless source compression"` | PASS：隔离后端真实 API 场景中正常 success WAV 转为更小 FLAC，前后解码 PCM SHA-256 一致；坏 WAV 保留并记失败，partial_success 不入队，无 `.part`/backup 残留；记录主键迁移后仍 success 且 `pending=0`。聚焦 Playwright 验证 Compress WAVs 确认操作、完成状态和 Saved 2.44 KB 展示。 |
