# ASR 转录后原音频无损压缩

## 背景

目录 ASR 任务会长期保留导入后的 PCM WAV。真实目录以 48 kHz、24-bit、单声道录音为主，
转录完成后仍持续占用大量空间。现有 `cleanup-source-audio` 可以删除成功文件，但不能在保留
原音频播放和后续审计能力的同时释放空间。

本功能增加一个与 ASR 转录解耦的、用户显式启动的无损压缩任务。它只处理已经完整成功且
text/timeline 产物存在的 WAV，将其转为 FLAC，再原子迁移任务记录并回收 WAV。

## 用户目标验证清单

### 必须实现

- 压缩只能在目录任务停止、failed-chunk retry 停止、外部导入停止后独立启动。
- 只处理 `success + text/timeline 完整 + audio_dir 内 + 普通 WAV 文件`。
- 使用 FLAC 无损编码，并比较原 WAV 与 FLAC 解码后的 PCM SHA-256。
- FLAC 不比 WAV 小时保留 WAV，不能为了完成计数扩大存储。
- 每个文件独立提交：文件路径、source key、大小/mtime、外部导入 manifest、内容 hash 索引和
  duplicate 引用保持一致。
- 压缩异步执行并持久化进度；重复启动幂等，进程中断后可重新启动恢复。
- 管理端展示可压缩文件/空间、累计已释放空间、当前进度、失败结果，并提供启动和取消操作。

### 必须不破坏

- `pending`、`processing`、`partial_success`、`failed`、缺少产物、目录外、符号链接和非 WAV
  文件保持不变。
- 压缩期间不能启动普通 ASR run、failed-chunk retry、外部导入或删除源音频。
- 已有 transcript、metadata、timeline、daily 文档和 diarization 产物路径不改变。
- 压缩后源音频 API 继续支持 Range 请求和 `audio/flac`，WebUI 播放入口保持可用。
- 外接设备再次接入时，不得因原 WAV 路径已迁移为 FLAC 而重复导入或重新转录。
- 正式 9900 服务和用户真实 `audio_dir` 不参与自动化测试。

### 必须真实验证

- 真实 FFmpeg 生成 WAV，构造成功/失败/partial/缺产物记录后通过 Admin API 启动压缩。
- Linux CI 与 macOS agent-extensions 分片显式准备 FFmpeg，确保真实编解码用例不会因 runner 镜像差异而被跳过或误报。
- 轮询独立压缩状态到 terminal，断言 PCM hash 相同、WAV 已回收、FLAC 可播放且支持 Range。
- 复跑压缩得到零候选，普通 Run 在压缩期间返回冲突，失败或取消不删除 WAV。
- WebUI 亮色/暗色均显示紧凑的统计、确认文案和进度，不新增硬编码颜色。

## 状态与数据模型

`FileRecord` 增加可选 `source_compression`：

- codec 固定为 `flac`
- 原路径、原大小、原修改时间
- 压缩后大小、节省字节数
- PCM SHA-256 与完成时间

Task summary 增加：

- `compressible_source_file_count/bytes`
- `compressed_source_file_count`
- `compression_saved_bytes`

独立 `SourceAudioCompressionState` 持久化在任务目录，状态为
`queued/running/completed/completed_with_errors/cancelling/cancelled/interrupted/failed`。
内存仅保存活跃任务和取消请求；daemon 重启后，落盘的活跃状态读取为 `interrupted`，由用户显式
重新启动恢复，避免后台意外占用资源。

## 单文件事务

1. 重新检查 record 状态、产物、路径边界、符号链接和扩展名。
2. 编码到同目录隐藏 `.bifrost-compress-*.part`，显式指定 FLAC container。
3. 计算 WAV 与 FLAC 解码为 `pcm_s32le` 后的 SHA-256；不同则删除 part 并保留 WAV。
4. 若 FLAC 不小于 WAV，删除 part 并记录 skipped。
5. 把 WAV 原子 rename 为隐藏 backup，再把 part rename 为最终 `.flac`。
6. 保存替换后的 file store：删除旧 key、插入新 key，并更新 duplicate 引用。
7. 更新外部导入 manifest 和内容 hash 索引中的 canonical key/path。
8. 所有状态保存成功后删除 backup；任一步失败保留或恢复可用源文件。

恢复规则：

- 旧 record 指向 WAV，WAV 存在且只有 part：删除 part 后重做。
- 旧 record 指向 WAV，backup + FLAC 存在：丢弃未提交 FLAC 并恢复已验证身份的 WAV backup。
- 新 record 指向 FLAC 且 backup 存在：重新校验 FLAC PCM hash、补齐辅助索引后删除 backup。
- backup 存在但 FLAC 不存在：把 backup 恢复为 WAV。
- source、backup 或已迁移 FLAC 的 size/mtime/PCM 身份冲突时停止恢复并保留所有可用副本，禁止猜测删除。

## API 与互斥

- `GET /api/asr/tasks/{id}/compress-source-audio`：读取最近状态。
- `POST /api/asr/tasks/{id}/compress-source-audio`：创建异步压缩任务。
- `DELETE /api/asr/tasks/{id}/compress-source-audio`：请求取消；当前 FFmpeg 结束后在文件边界停止。

同一时间全局只运行一个压缩 job，降低磁盘争用。普通 run、bulk retry、external import、
cleanup originals 与 compression 双向互斥。

## UI

任务 Overview 沿用 Ant Design `Descriptions`、`Alert`、`Progress` 和紧凑 toolbar：

- `Compressible WAV` 展示候选空间和数量。
- `Compression Saved` 展示累计释放空间和文件数。
- `Compress WAVs` 使用非危险操作按钮，确认文案明确 FLAC、PCM 校验和执行前提。
- 活跃 job 显示当前文件、文件数进度、已释放空间和 Cancel。
- 失败使用 warning，不用新增自定义颜色，保证亮暗主题一致。

## 测试方案

### 单元测试

- 候选筛选覆盖成功、partial、failed、缺产物、FLAC、symlink、目录外。
- PCM hash 输出解析、目标/part/backup 路径、无收益与冲突处理。
- file store key 迁移、duplicate 引用、compression summary。
- external import 原 WAV 命中压缩后的完成记录，不重复导入。
- 中断 artifact 恢复和辅助索引同步。

### E2E

新增 shell E2E，使用临时 `BIFROST_DATA_DIR`、动态端口和真实 FFmpeg：

- API 启动并轮询完成。
- WAV/FLAC PCM hash 一致，原 WAV 消失，FLAC 更小。
- source API 返回 `audio/flac`，Range 返回 206。
- 重复执行不重新处理已压缩记录，非 success 文件保持 WAV。
- 生命周期互斥由单元/API 回归验证覆盖，避免依赖人为延长 FFmpeg 的脆弱时序测试。

### Human tests

更新 `human_tests/qwen3-asr-local-server.md`，新增转录后无损压缩用例并在写入后立即执行。

## 交付门禁

- 先执行专项 E2E，再执行 `rust-project-validate`。
- 本地不执行 coverage 脚本；远端 CI 运行 `bash scripts/ci/coverage-all.sh --json --gate`。
- 至少两轮 Review/Fix/Test，推送 PR、看护全部 CI 绿色后合入 main。
