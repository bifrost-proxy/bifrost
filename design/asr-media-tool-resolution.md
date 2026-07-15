# ASR 媒体工具定位与说话人分段容错方案

## 背景与目标

macOS 桌面应用启动的 Bifrost 后台进程可能只继承 `/usr/bin:/bin:/usr/sbin:/sbin`。即使用户已经通过 Homebrew 安装 `ffmpeg` 和 `ffprobe`，ASR Directory Task 仍可能在音频标准化阶段报 `failed to spawn process: No such file or directory`，导致外接录音已经成功导入却无法进入转录。

本方案让 Bifrost Admin 的 ASR 路径在保留现有 PATH 语义的同时，可靠识别 Homebrew、MacPorts 和常见 Unix 安装目录中的媒体工具。

真实录音重跑还暴露出第二个可靠性问题：说话人分离模式已经产出大量可用文字后，如果最后一个很短的 ASR unit 撞到内存保护，整个文件仍会被标记为 `failed`。非说话人分离路径早已把同类情况保存为 `partial_success`，本方案将相同的容错语义补到说话人分离路径，避免一个局部失败丢弃整段录音的可用结果或阻塞 Daily Agent。

## 用户目标验证清单

### 必须实现

- PATH 可见时继续优先使用 PATH 中的 `ffmpeg` / `ffprobe`。
- PATH 不包含 Homebrew 时，macOS 后台仍能从 `/opt/homebrew/bin` 或 `/usr/local/bin` 找到工具。
- ASR 状态检查、上传音频处理、WebSocket 音频处理和 Directory Task 标准化/切片统一使用同一定位逻辑。
- 找不到工具时保留原有裸命令回退，让现有错误处理和安装提示继续生效。
- 说话人分离模式下，单个 ASR unit 失败时记录 `failed_chunks` 并继续处理后续 unit；只要存在可用转录，文件保存为 `partial_success`。
- 如果全部说话人 ASR unit 都失败且没有可用文字或时间线，文件仍保持 `failed`，避免把空结果误报为成功。
- 强制暂停仍立即中断，不得被局部失败容错吞掉。

### 必须不破坏

- 不自动修改系统 PATH，不安装或删除 Homebrew 包。
- 不修改系统代理，不使用正式端口 9900 运行自动化测试。
- 不改变 ASR 模型、说话人分离算法、外接设备导入、Daily Agent 或研究流水线内容语义。
- 不删除源录音；外接设备导入完成后仍从本地副本执行转录。

### 必须真实验证

- 使用不含 Homebrew 的 PATH 启动临时 Bifrost，`/api/asr/status` 仍返回 `ffmpeg_available=true`。
- 正式外接设备场景中，导入文件逐个完整落盘，修复后不再出现 `ffmpeg normalize: failed to spawn process`。
- 单元测试覆盖 PATH 优先、常见目录回退、不可执行文件跳过和完全找不到时的裸命令回退。
- 单元测试覆盖说话人 unit 局部失败保留已有文字，以及暂停信号不会被当作可容错失败。

## 实现逻辑

在 `crates/bifrost-admin/src/handlers/media_tools.rs` 提供统一的 Tokio `Command` 构造器：

1. 读取当前进程 PATH，并按顺序查找可执行文件。
2. PATH 未命中时查找当前可执行文件同级目录，以及 `/opt/homebrew/bin`、`/usr/local/bin`、`/opt/local/bin`、`/home/linuxbrew/.linuxbrew/bin`、`/usr/bin` 和 `/bin`。
3. Unix 下要求候选是普通文件且具有执行位；Windows 下兼容 `.exe` 后缀。
4. 仍未命中时返回裸工具名，由 `Command` 保持原有 spawn 错误语义。

ASR handler 中所有直接启动 `ffmpeg` / `ffprobe` 的路径改为调用该构造器；`brew` 等非媒体命令不受影响。

说话人分离转录循环不再对 `attempt.result` 直接使用 `?`：

1. 成功 unit 按原逻辑追加文字和时间线。
2. 普通失败写入带 offset、duration、RMS 和错误信息的 `FailedChunkRecord`，同时在正文保留与 `retry-chunks` 兼容的占位符，再继续处理后续 unit。
3. 强制暂停仍向上返回。
4. 独立跟踪真实文字或时间线是否已产生；全部 unit 都失败时即使正文含占位符也返回明确错误，否则由现有文件收尾逻辑写成 `partial_success`，供后续 `retry-chunks` 精确替换恢复文本。

## 测试方案

### 单元测试

- `media_tool_prefers_path_before_fallback_directories`
- `media_tool_uses_fallback_when_path_is_sanitized`
- `media_tool_ignores_non_executable_candidates`
- `media_tool_returns_bare_name_when_unresolved`
- `merge_diarized_segment_attempt_records_failure_without_discarding_text`
- `merge_diarized_segment_attempt_reports_usable_success`
- `merge_diarized_segment_attempt_propagates_pause_without_recording_failure`

### E2E

新增 `e2e-tests/tests/test_asr_media_tool_resolution.sh`：

- 复用进程护栏，使用临时 `BIFROST_DATA_DIR`、动态非 9900 端口、`--no-system-proxy`。
- 仅给临时 Bifrost 进程设置系统最小 PATH，明确排除 Homebrew/MacPorts。
- 请求 `/_bifrost/api/asr/status` 并断言 `ffmpeg_available=true`。
- 测试退出时只按当前测试 PID 清理，不影响正式 Bifrost。

### human_tests

新增 `human_tests/asr-media-tool-resolution.md`，覆盖最小 PATH 临时服务与真实外接录音导入/转录回归；创建后立即逐条执行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照真实失败错误复核所有 Admin ASR `ffmpeg` / `ffprobe` 调用点。
- 执行 `git status --short`、`git diff`、相关单元测试、E2E 和 human test。
- 修复跨平台路径、执行权限或测试清理问题。

### 第 2 轮

- 复核第 1 轮后的最新 diff，确认没有测试触碰正式端口、正式数据或系统代理。
- 复跑受影响测试、E2E 启动护栏和 human test。
- 再执行 fmt、clippy、workspace all-features 与项目校验；覆盖率 90% 由远端 CI 门禁验证。

## 文档更新要求

- `human_tests/readme.md` 增加本模块索引。
- 行为不新增用户配置项，不需要修改 CLI help；媒体工具安装提示保持不变。
