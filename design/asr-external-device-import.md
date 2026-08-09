# ASR 外接设备自动导入方案

## 背景

当前 ASR Directory Task 的核心模型是：用户创建任务并指定 `audio_dir`，调度器按 `schedule` 定时扫描这个本地目录，发现音频文件后写入 `files.json`，再执行转写和 Daily Docs 生成。

新需求是在 ASR 任务上绑定一个或多个外接设备名称。设备连接并挂载后，Bifrost 自动扫描设备中的文件，并把差异文件导入到任务绑定的本地目录下。导入后的目录和文件名必须保持设备内原始结构；多个设备通过设备名称作为目标目录下的根目录隔离，避免重名冲突。

示例：

```text
任务 audio_dir = /Users/eden/Recordings
绑定设备 = TX_MIC001, TX_MIC002

/Volumes/TX_MIC001/2026-05-20/A.wav
  -> /Users/eden/Recordings/TX_MIC001/2026-05-20/A.wav

/Volumes/TX_MIC002/2026-05-20/A.wav
  -> /Users/eden/Recordings/TX_MIC002/2026-05-20/A.wav
```

外接设备导入是 ASR Directory Task 的能力扩展，不引入独立的"设备任务"实体；导入完成后文件就是普通本地 `audio_dir` 文件，走既有转写/Daily Docs/Daily Agent 链路。

## 用户目标验证清单

### 必须实现

- ASR 任务可绑定多个外接设备名称。
- 设备连接/挂载后自动触发导入（V1 macOS 通过 Disk Arbitration，planned；当前实现为手动补跑 + 配置页轮询兜底）。
- 平台事件漏掉或设备已在启动前连接时，WebUI 和 API 必须提供手动补跑入口。
- 按设备名称作为 `audio_dir` 下一级根目录导入。
- 导入保持设备内相对目录结构和文件名不变。
- 只导入差异文件；已经完成转写且产物仍有效的精确相同内容，必须在写入本地临时文件之前跳过，避免压缩或清理本地源文件后再次从外盘复制。
- ASR 处理前做内容哈希去重；同一任务内已有相同内容文件完成转写且产物存在时，后续重复文件跳过模型推理并复用转写结果。
- 导入时检查大小、稳定性和完整性，避免复制半写入文件。
- 容错、去重、断点/临时文件恢复、异常状态可见。
- 打开 ASR 任务配置页面时，如果当前已有未绑定设备连接，逐个弹窗确认是否监听并导入。
- ASR 定时任务创建后允许编辑所有配置：任务名称、数据源目录、递归开关、启停状态、启动时间、定时周期、模型/语言/runtime、外接设备绑定与导入策略。
- 切换数据源目录只影响后续扫描/导入；已转写文件记录、转写文本、timeline、metadata、Daily Docs 与 Daily Agent report 不迁移、不删除、不重算。
- 删除 ASR 任务弹窗明确确认，要求输入完整任务名称。
- V1 只承诺 macOS，但 macOS 必须完整、全面、可靠、稳定；外接设备导入不做后台定时扫描。

### 必须不破坏

- 现有只指定本地目录的 ASR Directory Task 行为不变。
- 现有 `files.json`、Daily Docs、retry chunks、pause/resume 与 task scheduler 主链路不被重写。
- 现有 ASR runtime、模型服务租约、Speech Workbench 与 CLI 主链路不受影响。
- 现有 CLI `bifrost ai asr task ...` 兼容读写既有任务字段。

### 必须真实验证

- macOS 真实挂载卷或可替代 disk image 触发导入。
- 重复连接同设备不重复复制（manifest 快路径）。
- 跨设备/跨目录同内容文件只转写一次，重复文件在详情页仍能展示复用的转写文本。
- 半写入文件被 `file_stable_secs` 稳定性闸口延迟到下一轮。
- 设备中途拔出：当前文件失败，run 标记 `device_disconnected`，不触发 ASR。
- 目标空间不足：停止本轮，记录 `insufficient_space`。
- 删除任务危险确认；未输入完整任务名 Delete 按钮禁用。

## 产品语义

### 导入是 Directory Task 的能力扩展，不是新的任务实体

外接设备导入不引入独立"Device Task"实体，而是给现有 `AsrDirectoryTask` 增加 `external_devices` 与 `import_policy` 字段。导入完成后文件成为普通本地 `audio_dir` 文件，走既有 ASR 转写、Daily Docs、Daily Agent 主链路。这样：

- WebUI/CLI/详情页所有既有能力免改造。
- Pause/Resume/删除/schedule 与原任务语义完全一致。
- 未绑定设备的老任务与新任务共存，不需要迁移。

### 设备目录一级根目录隔离

多个设备的相对路径都挂到 `<audio_dir>/<binding.name>/...`，避免不同设备下同名目录/文件互相覆盖。`binding.name` 是用户可读名称，也是目录名，必须经过 `sanitize_device_root` 归一化；`volume_uuid` 优先用于卷匹配，卷名用于展示与目录命名。

### 触发方式：事件为主，手动补跑兜底

V1 以设备事件监听 + 手动补跑组合为主，禁止后台定时扫描：

- macOS Disk Arbitration 事件订阅（planned, not yet shipped as of 2026-06-16）
- 配置页设备候选发现确认
- 手动 API/按钮补跑
- ASR run 不隐式扫描外接设备；导入后的本地文件走既有 discover 流程

### 差异导入与内容 hash 去重分层

导入用 "路径 + size + mtime" 判定差异（保持设备目录结构完整），去重发生在 ASR 处理前：

- T0 设备 manifest 快路径：同 UUID + relative_path + size + mtime 命中即零读取跳过。
- T1 已处理记录：source_key、压缩前原路径或可信 manifest BLAKE3 命中，且转写产物和 ASR 参数仍有效时，不再从外设拉回。
- T2 轻量候选筛选：size/duration/codec/sample_fingerprint 缩小候选，不能单独跳过 ASR。
- T3 精确 BLAKE3：已有 manifest hash 时先查任务级内容索引；manifest 不足但存在同尺寸已完成候选时，在复制前串行读取外盘文件计算 BLAKE3。只有 hash + size + 产物 + ASR 参数四重匹配才允许零写入跳过；未命中才进入复制流。
- T4 canonical audio hash：只在 normalize/ffmpeg 已发生时顺手计算。

### 危险操作显式确认

- 删除任务必须弹窗输入完整任务名。
- 切换 `audio_dir` 弹窗提示"历史转写数据保留，新目录不影响历史"。
- 运行中修改 `audio_dir` / `model` / `runtime_strategy` / `external_devices` 返回 `409 task_running`。

## 调研结论

### 当前代码基线

现有 ASR 任务主链路位于 `crates/bifrost-admin/src/handlers/asr_jobs`：

- `AsrDirectoryTask` 已持久化 `audio_dir`、`recursive`、`enabled`、`schedule`、`language`、`model`、`runtime_strategy`、`daily_agent`。
- `ensure_scheduler_started()` 每 10 秒检查 `next_run_at_ms`，到期后启动 `run_directory_task()`。
- `run_directory_task()` 先 `discover_audio_files(task.audio_dir, task.recursive)`，再把新文件写入 `FileStore`。
- `source_key(path)` 当前由 canonical path + size + modified time 计算；同一路径文件 size/mtime 变化会变成新 key。
- `output_paths()` 已按 `audio_dir` 的相对路径保留输出结构。
- `discover_audio_files()` 只处理音频扩展名并跳过 0 字节文件。

因此外接设备导入不应替代 ASR 转写逻辑，而应作为设备连接事件、用户确认或手动 API 触发的独立同步阶段：

```text
device event / user confirmation / manual import trigger
  -> sync_external_devices_for_task(task)
  -> discover_audio_files(task.audio_dir, task.recursive)
  -> existing FileStore / transcription / Daily Docs / Daily Agent
```

这样导入后的文件成为普通本地 `audio_dir` 文件，后续 ASR 处理、WebUI 详情、CLI task show/files/daily 均复用现有逻辑。

### macOS 设备监听

V1 以 macOS 为第一优先级，因为当前 ASR 本地模型链路本身主要运行在 Apple Silicon macOS。

官方 Disk Arbitration 适合监听磁盘/卷出现、消失、挂载路径变化和卷名变化。它支持 `DADiskAppearedCallback`、`DADiskDescriptionChangedCallback`、`DADiskDisappearedCallback`，并要求创建 `DASession` 后注册 callback，再绑定 run loop 或 dispatch queue。

关键设计含义：

- 事件源用 Disk Arbitration，而不是仅监听 `/Volumes` 目录。
- 需要处理“出现”和“挂载路径后续变化”两个阶段；设备刚出现时未必已有 mount path。
- 设备拔出、未安全弹出、卷名改动、多个分区等情况都要作为正常异常路径处理。
- callback 只做轻量入队，不在 callback 中做扫描/复制。
- V1 不做后台定时扫描；如果 Bifrost 启动前设备已经连接，用户可通过配置页确认或手动导入触发，后续重新插拔会由设备事件触发。

### FSEvents / notify 的角色

Apple FSEvents 是目录层级变化通知，适合在设备已挂载后观察设备目录是否继续写入。Rust `notify` 提供跨平台目录监听，macOS 默认可用 FSEvents backend，也提供 PollWatcher 作为限制场景下的替代。

但目录监听不是设备连接真相源：

- 设备未挂载时没有稳定目录可 watch。
- 大目录事件可能丢失或合并。
- Linux inotify 大目录递归会遇到 watch 数量限制。

因此 V1 的可靠策略是“Disk Arbitration 触发 + 手动补跑兜底”，FSEvents/notify 只作为优化项：

- V1 必做：设备事件监听、配置页确认和手动补跑。
- 后续可选增强：对已匹配设备的 mount path 建立目录 watcher，发现新增/修改后 debounced 入队同步。
- 即使目录 watcher 出错，也不能影响设备连接事件和手动补跑入口。

### 平台边界与后续路线

架构保留 `ExternalVolumeProvider` 抽象，避免后续支持 Linux/Windows 时重写 ASR 任务、导入和 UI 主链路。但首版 V1 的交付边界只覆盖 macOS：

- V1 必须实现 macOS Disk Arbitration 事件源。当前实现尚未接入 Disk Arbitration 事件流，仅通过同步扫描 `/Volumes`（配合 `diskutil info` 解析 UUID/external/read-only）枚举已挂载卷，并依赖配置页轮询和手动 `POST /external-import/run` 触发同步（planned, not yet shipped as of 2026-06-16：事件订阅、debounce 入队、`ExternalDeviceImportManager`）。
- V1 不做 `PollingVolumeProvider` 后台定时扫描，避免设备长期连接时反复遍历和对比；Bifrost 启动前已挂载、事件监听权限/环境不可用等情况通过配置页确认和手动导入入口处理。
- V1 必须在 macOS 上完整通过真实 disk image 或真实外接设备验证，不能把监听、确认弹窗、导入、去重、异常恢复中的任一关键路径留到后续。
- Linux/Windows 只保留 provider 接口和设计说明，不作为 V1 可用能力宣传，也不作为 V1 验收门槛。

后续跨平台 provider 路线：

- Linux：优先 UDisks2 D-Bus 的 block device/filesystem 对象与 InterfacesAdded/PropertiesChanged。
- Windows：GUI/desktop 进程可接收 `WM_DEVICECHANGE`/`DBT_DEVICEARRIVAL`/`DBT_DEVICEREMOVECOMPLETE`。
- 各平台都应保留显式手动补跑入口，但不默认做后台定时扫描。

### 内容哈希成本评估

新增哈希去重是可以接受的，但必须设计成“顺手计算、只算一次、长期缓存”，不能让每次设备连接都全量重读所有文件。

本机轻量基准：

- 旧 SHA-256 基准只能作为历史对照；V1 实现不写入、不消费旧 SHA-256 索引。
- BLAKE3 支持流式、SIMD 和多线程，但 Bifrost 在代理主服务内默认用全局串行队列流式计算，避免多个大文件并行 hash 抢占 CPU。

结论：

- 对外接录音设备，实际瓶颈通常是设备顺序读取速度和 USB/存储介质 I/O，而不是 BLAKE3 CPU 计算。
- 导入时本来就要顺序读取源文件并写入目标文件，因此应在复制流中同时更新 BLAKE3 digest，不额外再读一遍源设备。
- 对已经在 `audio_dir` 中存在、但不是本轮导入产生的历史文件，ASR run 主流程不做同步全文件 hash；缺少 hash 时直接退化为路径级处理，避免 Resume、任务列表刷新或启动恢复把几十 GB 历史录音重新读一遍。
- 需要补充内容 hash 的场景必须进入后台内容哈希队列：队列全局串行、低优先级、流式 chunk 读取，不能占用 Admin API 请求线程或 Tokio async worker，也不能把多个大音频文件并行 hash 到拖慢代理服务或外接设备。
- 如果 hash 计算失败、文件超出 `max_file_bytes`、文件不稳定或设备断开，不能因为缺少 hash 阻塞正常 ASR；该文件退化为现有按路径的处理流程并记录 `hash_unavailable`。
- V1 默认启用内容哈希去重，因为成本可控，收益明确：多个设备或不同目录下的重复录音不会重复跑 ASR 模型。

### 高性能哈希算法选型

算法选型必须先区分用途：有些算法适合做最终身份，有些只适合做候选筛选。Bifrost ASR 的跳过模型推理属于高风险决策，不能只看“快”，还必须保证不会因为碰撞或近似匹配导致漏转写。

| 算法 / 指纹 | 性质 | 适合用途 | 不适合用途 | Bifrost 取舍 |
|---|---|---|---|---|
| BLAKE3-256 | 加密哈希，支持流式、SIMD 和多线程，默认输出 256 bit | 本地精确内容身份、后台补 hash、复制流顺手 hash | 跨系统兼容旧索引时的唯一格式 | 新增本地 `content_hash` 固定使用 `blake3:<hex>`；后台队列默认单文件流式计算，不在请求线程并行拉满 CPU |
| SHA-256 | 加密哈希，生态兼容最好 | 只作为调研对照或外部系统互操作信息 | 新增本地大文件索引、ASR 跳过判定、历史兼容链路 | 当前实现不兼容旧 SHA-256 数据；本地精确去重统一使用 BLAKE3，旧数据可丢弃重建 |
| XXH3 / XXH128 | 非加密哈希，接近内存速度 | `sample_fingerprint`、候选缩小、缓存失效检测、UI 风险提示 | 不能单独作为重复判定，不能直接跳过 ASR | 只用于轻量窗口指纹，例如首/中/尾若干 MiB + size/duration 组合；命中后仍需精确 hash 或可信产物记录 |
| CRC32C | 非加密校验，硬件加速常见 | 复制传输错误的廉价校验、临时诊断 | 内容去重身份、ASR 跳过依据 | 可作为复制完整性辅助字段，不进入 dedupe index 主键 |
| FastCDC / Rabin / Gear CDC | 内容定义分块，适合跨版本、跨偏移存储去重 | 未来录音仓库级 chunk 索引、备份/归档节省空间 | V1 导入、Resume、ASR 前置路径 | 暂不进入主链路；如果做，也必须是独立后台索引服务 |
| Chromaprint / 声学指纹 | 近似音频指纹，需先解码为 PCM | 发现转码后“听起来相同”的候选、人工确认 | 自动跳过 ASR 的唯一依据 | 只生成候选；除非后续有 `canonical_audio_hash` 精确命中，否则不能自动复用 transcript |
| `canonical_audio_hash` | 对规范化 PCM 帧做 BLAKE3 | 跨容器/转码后的精确音频内容身份 | 导入阶段默认计算 | 只在 ASR normalize/ffmpeg decode 已经发生时顺手计算；或后台对高价值候选计算 |

参考调研：

- BLAKE3 官方 README 强调其加密哈希属性、Merkle tree 带来的 SIMD/多线程能力，以及 Rust/C 官方实现的 CPU feature detection 和 streaming/incremental update 能力：https://github.com/BLAKE3-team/BLAKE3
- xxHash 官方文档把 XXH3/XXH128 定位为极快的非加密哈希，适合性能敏感的候选指纹，但不能承担加密级身份判定：https://xxhash.com/
- FastCDC 论文说明 CDC 能提升数据去重冗余检测能力，但传统 CDC 有较重 CPU 开销，FastCDC 通过 Gear hash 优化、跳过 sub-minimum cut-point 等手段加速：https://www.usenix.org/conference/atc16/technical-sessions/presentation/xia
- Chromaprint/AcoustID 是从解码后的未压缩音频提取音频指纹，适合相似音频识别候选，不负责容器解码，也不是字节级精确内容 hash：https://acoustid.org/chromaprint

### 行业方案调研与取舍

公开同步、备份和去重系统通常把“快速判定”和“强一致去重”分开，而不是默认全量读取所有大文件：

- `rsync` 默认 quick check 使用文件 size + modification time 判断是否需要传输，只有开启 checksum 模式才做更慢的内容校验。这类策略适合重复扫描同一目录或同一设备，因为绝大多数未变化文件可以只读目录元数据。
- `rclone copy/sync` 的默认思路也是优先用 size + modtime，只有远端支持或用户开启 `--checksum` 时才用 hash；其文档也把 checksum 作为更贵但更强的比较方式。
- `Syncthing` 为文件维护 block list，每个 block 有 size/hash；发生变化时比较 block 列表，只请求缺失或过期 block。这适合持续同步系统，但需要先为文件建立并维护 block 索引。
- `restic`、`Borg`、`Kopia` 这类备份/归档系统会使用内容定义分块（CDC）和 chunk hash 做跨路径、跨版本去重。它们的目标是存储节省和版本化，愿意在备份窗口内付出 chunking/hash 成本，并且依赖本地缓存避免重复 chunk。
- 研究和工业实现都把 CDC/hash 视为 CPU 密集路径，通常通过缓存、分阶段管线、限流或并行化处理；它不适合放在低延迟代理服务的请求线程或主 async worker 里。

参考来源：rsync manpage quick check / `--checksum`、rclone docs `--checksum`/`--size-only`、Syncthing synchronization docs、restic design CDC、Borg internals deduplication、BLAKE3 official README、xxHash official docs、FastCDC USENIX paper、Chromaprint/AcoustID docs。

对 Bifrost ASR 来说，最佳平衡不是追求“首次就识别所有跨路径重复”，而是：

1. 外接设备导入以同步系统思路为主：同设备同相对路径优先用 metadata manifest 快速跳过，保证重复连接、重复扫描是零读取。
2. ASR 转写与导入都以准确性为第一约束：只有可信内容 hash、原始字节数、transcript artifacts 和 ASR 参数同时匹配时，才允许在复制前跳过；轻量指纹不能单独导致漏导入或漏转写。
3. 内容 hash 作为增量增强能力：同一路径 manifest 已缓存 hash 时先做零读取索引命中；manifest 缺失或过期但任务索引存在同尺寸已完成候选时，在后台导入线程和全局串行 hash 队列中先读取外盘计算精确 BLAKE3。未命中才执行复制，复制流继续顺手计算并持久化 hash。
4. CDC/block 级去重暂不进入 V1 主链路：录音文件通常是完整文件粒度输入，ASR 输出按文件/timeline 管理；CDC 适合备份仓库省空间，但会显著增加索引复杂度和 CPU 压力。后续如要做“录音仓库级重复片段识别”，应作为独立后台索引服务，而不是导入/Resume 的同步前置步骤。

### 大文件极致性能去重策略

录音文件可能单个数百 MiB 到数 GiB、总量几十 GiB，因此去重必须分层，优先走“不读文件”的快路径，完整内容 hash 只能作为精确兜底。

1. **T0 设备 manifest 去重**：外接设备导入首先用 `volume_uuid/device_identifier + relative_path + source_size + source_modified_ms` 判断同一设备同一路径是否已经导入且目标文件仍匹配。命中时直接 `unchanged/skipped`，不打开源文件、不计算 hash、不复制。目标文件因压缩或清理而不存在时，如果同一条 manifest 已缓存 BLAKE3，则继续用该 hash 查询任务级内容索引；索引记录的原始字节数、转写产物和 ASR 参数有效时同样零读取跳过。
2. **T1 已处理记录去重**：目标文件被用户清理后，如果 `files.json` 中已有相同目标路径或 `source_compression.original_source_path` 的 `success/partial_success`，且 transcript artifacts 仍存在，导入阶段直接记为 `processed_record_skipped`，不再从外设拉回本地。源路径关联元数据缺失时，不能退化为仅按文件名判断，必须进入 T3 精确 hash。
3. **T2 轻量候选筛选**：跨路径或跨设备的“可能重复”先用 size、mtime、duration、codec、device relative path、已有 source key、已有 manifest hash 和 `sample_fingerprint` 缩小候选。`sample_fingerprint` 可用 XXH3/XXH128 或 BLAKE3 采样窗口计算，但只能减少候选和展示风险，不能单独作为“内容相同”并跳过 ASR 的最终依据，避免 partial fingerprint 误判导致漏转写。
4. **T3 精确文件内容 hash**：当 manifest 无可复用 hash、但任务级内容索引存在同尺寸且产物有效的候选时，导入后台线程必须在创建本地 `.part` 文件之前通过全局内容哈希队列串行读取外盘源文件。读取前后再次校验 size/mtime；精确 BLAKE3 命中后写入 `processed_record_skipped` manifest 状态并停止，不创建目标或临时文件。未命中才执行复制；复制流继续顺手更新 BLAKE3 digest。该兜底可能让“同尺寸但全新内容”多读一次外盘，这是确保已处理内容零本地写入的必要代价，但不会占用 Admin API 请求线程、Tokio async worker 或 ASR 模型处理锁。
5. **T4 规范化音频 hash**：如果两个文件字节不同但可能是同一段录音的不同容器/码率版本，只在 ASR normalize/ffmpeg decode 已经发生时顺手计算 `canonical_audio_hash`，即对统一采样率、声道和 PCM sample format 后的 PCM frame 流做 BLAKE3。它用于识别转码后精确相同的音频内容，但不在导入阶段默认触发全量解码。
6. **ASR 主流程不等待 hash**：ASR run 只消费已经存在的 `content_hash`、`canonical_audio_hash` 和相关索引。缺少 hash 的大文件按普通文件继续处理，不为了去重同步读完整音频；后台 hash 后续命中只能优化下一轮或后续重复文件。
7. **算法演进**：V1 持久化字段保留 `content_hash_algorithm`，当前只写入和消费 `blake3`；旧 SHA-256 数据不做兼容，必要时重建索引。

因此“重复的就不导入/不转写”分两种：同一设备同一路径、同 size/mtime 的重复连接导入必须零读取跳过；目标 WAV 被压缩为 FLAC、被清理或 manifest 路径过期时，只要原始 WAV 的 BLAKE3 仍能命中有效内容索引，就必须在复制前跳过。跨路径内容若只有同尺寸候选，则在后台串行计算精确 hash 后决定；没有可信候选时正常导入，不能用文件名、mtime 或轻量指纹单独跳过。

## 数据模型

### AsrDirectoryTask 扩展

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrDirectoryTask {
    // existing fields...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_devices: Vec<AsrExternalDeviceBinding>,
    #[serde(default)]
    pub import_policy: AsrExternalImportPolicy,
}
```

### 设备绑定

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrExternalDeviceBinding {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_identifier: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_import_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
}
```

匹配规则：

- `name` 是用户配置和目标目录根名，必须唯一。
- 默认按卷名匹配；如果用户选择绑定时能拿到 `volume_uuid`，则优先用 UUID 精确匹配，卷名作为可读展示和目标目录名。
- 多个已挂载卷同名时：
  - 若 binding 有 `volume_uuid`，只匹配 UUID。
  - 若只有 `name`，进入 ambiguous 状态，不导入，提示用户在 WebUI 选择具体卷并保存 UUID。

### 导入策略

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AsrExternalImportPolicy {
    #[serde(default = "default_external_import_enabled")]
    pub enabled: bool,
    #[serde(default = "default_external_file_stable_secs")]
    pub file_stable_secs: u64,
    #[serde(default = "default_external_min_free_bytes")]
    pub min_free_bytes: u64,
    #[serde(default = "default_external_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_external_auto_run_after_import")]
    pub auto_run_after_import: bool,
    #[serde(default = "default_content_hash_dedupe_enabled")]
    pub content_hash_dedupe_enabled: bool,
    #[serde(default = "default_content_hash_algorithm")]
    pub content_hash_algorithm: String,
    #[serde(default)]
    pub delete_source_after_import: bool,
}
```

默认值：

| 字段 | 默认值 | 说明 |
|---|---:|---|
| `enabled` | `false` | 只有绑定设备后显式启用 |
| `file_stable_secs` | `10` | size/mtime 连续稳定后才复制 |
| `min_free_bytes` | `10 GiB` | 目标盘剩余空间安全阈值 |
| `max_file_bytes` | `50 GiB` | 单文件上限，防误导入超大非音频文件 |
| `auto_run_after_import` | `true` | 导入完成后触发 ASR task run |
| `content_hash_dedupe_enabled` | `true` | ASR 处理前按内容哈希跳过已转写重复文件 |
| `content_hash_algorithm` | `blake3` | 本地精确内容身份固定使用 BLAKE3；旧 SHA-256 数据不做兼容 |
| `delete_source_after_import` | `false` | V1 不默认删除外接设备源文件 |

### 导入状态文件与内容哈希索引

每个任务一个导入状态：

```text
<BIFROST_DATA_DIR>/asr/tasks/<task_id>/external_imports.json
<BIFROST_DATA_DIR>/asr/tasks/<task_id>/content_hash_index.json
```

```rust
pub(crate) struct AsrExternalImportStore {
    pub version: u32,
    pub devices: BTreeMap<String, AsrExternalDeviceState>,
    pub runs: Vec<AsrExternalImportRunSummary>,
}

pub(crate) struct AsrExternalDeviceState {
    pub binding_name: String,
    pub last_seen_mount_path: Option<PathBuf>,
    pub last_scan_at_ms: Option<u64>,
    pub last_import_at_ms: Option<u64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub files: BTreeMap<String, AsrImportedFileRecord>,
}

pub(crate) struct AsrImportedFileRecord {
    pub relative_path: PathBuf,
    pub source_size: u64,
    pub source_modified_ms: Option<u64>,
    pub source_hashes: BTreeMap<String, String>,
    pub sample_fingerprint: Option<String>,
    pub target_path: PathBuf,
    pub target_size: u64,
    pub first_seen_at_ms: Option<u64>,
    pub imported_at_ms: u64,
    pub status: String,
    pub error: Option<String>,
}

pub(crate) struct AsrContentHashIndex {
    pub version: u32,
    pub hashes: BTreeMap<String, AsrContentHashRecord>,
}

pub(crate) struct AsrContentHashRecord {
    pub algorithm: String,
    pub hash: String,
    pub canonical_audio_hash: Option<String>,
    pub size: u64,
    pub canonical_source_key: String,
    pub canonical_source_path: PathBuf,
    pub transcript_artifacts: AsrTranscriptArtifacts,
    pub model: String,
    pub language: String,
    pub runtime_strategy: AsrRuntimeStrategy,
    pub completed_at_ms: u64,
    pub duplicate_count: u64,
}

pub(crate) struct AsrTranscriptArtifacts {
    pub text_path: PathBuf,
    pub metadata_path: PathBuf,
    pub timeline_path: Option<PathBuf>,
}
```

`files` key 使用 `relative_path` 的规范化字符串，不使用绝对源路径，避免同一设备挂载到不同 mount path 后被当成不同文件。

`content_hash_index.json` 是 ASR task 级内容哈希索引，不放在 `external_imports.json` 里，避免把普通目录任务和数据源切换后的去重能力耦合到外接设备导入。`hashes` key 必须使用 `algorithm:<hex>`，当前实现只写入和消费 `blake3:<hex>`；索引不跨 task 复用，避免不同任务的模型、语言、runtime、Daily Agent 配置不同却错误共享结果。`sample_fingerprint` 只进入候选索引，不能进入 `hashes` 主键；`canonical_audio_hash` 只有在规范化音频内容精确一致且 ASR 参数兼容时，才允许参与 transcript 复用。

状态保留目的：

- 支持 UI 展示最近导入结果。
- 支持崩溃后清理 `.bifrost-import-*` 临时文件。
- 支持跳过 unchanged 文件。
- `external_imports.json` 支持 UI 展示最近导入结果、崩溃恢复、跳过 unchanged 文件。
- `content_hash_index.json` 支持 ASR 处理阶段按内容 hash 跳过已完成且产物存在的重复文件。

## 导入路径规则

目标根目录：

```text
task.audio_dir / sanitize_device_root(binding.name)
```

相对路径：

```text
source_relative = source_path.strip_prefix(mount_path)
target_path = task.audio_dir / sanitize_device_root(binding.name) / source_relative
```

路径安全规则：

- `binding.name` 必须经过文件名安全化，只允许普通目录名；原始名称保留在 config 中展示。
- `source_relative` 必须是 normal relative path，拒绝 absolute、`..`、prefix escape、symlink escape。
- 默认跳过 symlink，避免导入设备外文件或循环目录。
- 隐藏系统目录默认排除：`.Spotlight-V100`、`.Trashes`、`.fseventsd`、`System Volume Information`、`$RECYCLE.BIN`。
- 音频扩展名复用现有 `AUDIO_EXTENSIONS`。

## 差异算法

V1 用 “路径 + size + mtime” 判定导入差异，用 “已有处理记录 / 精确内容 hash / 规范化音频 hash” 判定 ASR 处理去重。两者不要混在一起：导入仍然要保持设备目录结构，所以即使内容重复，也要把目标目录下对应文件补齐；去重发生在后续 ASR 模型处理阶段。

1. 枚举设备文件，过滤非音频、0 字节、超过 `max_file_bytes`、隐藏系统目录和用户 exclude globs。
2. 对每个候选文件读取 metadata。
3. 稳定性检查：
   - 第一次看到文件时记录 `size/mtime`，加入 `deferred`。
   - 至少 `file_stable_secs` 后再次读取，若 `size/mtime` 不变才允许复制。
   - 若设备已经断开，本轮标记 `device_disconnected`，下次重来。
4. 目标文件不存在：复制。
5. 目标文件存在且 size/mtime 与源一致：跳过 unchanged。
6. 目标文件存在但不同：
   - 如果状态里有同 relative path 的成功导入记录，且记录与源一致但目标不同，说明目标被用户修改；默认不覆盖，记录 `target_modified`。
   - 如果目标是上次未完成的 temp 残留，清理 temp 后重试。
   - 如后续增加 `overwrite_policy`，必须在 UI 中显式选择。
7. 复制完成后做 size 校验；复制流同步计算 BLAKE3 内容 hash，并写入 `source_hashes["blake3"]`。
8. 成功后把目标文件 mtime 设置为源 mtime，便于现有 `source_key()` 稳定。

不使用内容 hash 决定“是否复制目标文件”，原因是需求要求 `audio_dir/<device_name>/<relative_path>` 结构完整保留；重复文件也必须能在目标目录中看到。内容 hash 只决定“是否重复转写”。

## ASR 进入前置去重流程

ASR 进入模型前必须有单独的去重闸口，覆盖用户手动把文件复制到 `audio_dir` 的场景。这个闸口不能假设所有文件都经过外接设备导入流程，也不能为了去重同步读取几十 GiB 历史音频。

ASR run 的处理顺序调整为：

```text
discover_audio_files(task.audio_dir, task.recursive)
  -> stable_stat_filter(size/mtime unchanged enough for local files)
  -> source_key / processed artifact hit
  -> existing exact content_hash hit
  -> existing canonical_audio_hash hit
  -> load_or_create_lightweight_candidate_fingerprint()
  -> candidate lookup by size/duration/codec/sample_fingerprint
  -> decide_hash_cost_against_asr_cost()
  -> enqueue_asr_or_mark_duplicate_completed()
```

规则：

1. 导入产生的新文件优先复用 `external_imports.json` 中的 `source_hashes["blake3"]`，不重新读取目标文件。
2. 用户手动复制到 `audio_dir` 的文件，先走 `source_key`、历史 file record、产物存在性和 metadata 候选筛选；如果已有相同 source_key 的 completed/partial_success 且 transcript artifacts 存在，直接标记重复或历史已处理，不进入模型。
3. 非导入文件或旧文件缺少 hash 时，ASR run 不在同步路径补算完整 BLAKE3；文件按普通 pending/failed 进入模型处理，避免 Resume 或启动恢复卡住代理主服务。
4. 如后续需要给历史文件补 hash，必须通过后台内容哈希队列执行；计算必须基于稳定文件：size/mtime 在 hash 前后不变；若不稳定，文件保持 pending 或跳过本次 hash，等待下一轮后台队列。
5. `sample_fingerprint` 的计算只能读取少量窗口，例如首部、中部、尾部各 1-4 MiB，再结合 size、duration、codec 和 mtime 缩小候选。它不能单独导致 `duplicate_completed`，也不能单独跳过 ASR。
6. 当 `content_hash_index.json` 中已存在同 algorithm-prefixed hash、同 size 的记录，并且 canonical file 状态为 completed、`text_path`/`metadata_path`/`timeline_path` 等转写产物实际存在时：
   - 当前文件不进入 ASR 模型推理队列。
   - 当前 file record 标记为 `duplicate_completed`。
   - 写入 `duplicate_of_source_key`、`content_hash`、`transcript_alias`。
   - WebUI/CLI 展示当前文件时，通过 alias 读取 canonical transcript artifacts。
   - Daily Docs 聚合时仍可列出当前文件名和路径，但正文内容来自 canonical transcript，并在 metadata 中标记 duplicate source。
7. 当 `canonical_audio_hash` 命中时，只有在它来自可信规范化流程、ASR 参数兼容、产物存在且音频 normalize 参数版本一致时才允许跳过模型。它解决“用户手动拷贝了转码后文件”的场景，但不能在导入阶段强行解码全量音频。
8. 如果 hash 命中但 canonical 文件尚未完成、产物缺失、产物损坏、模型/语言/runtime 不兼容，不能跳过；当前文件按普通文件进入 ASR。
9. 如果两个重复文件在同一轮同时发现，只有第一个成功完成转写后才建立 canonical 记录；其它重复文件可以在本轮尾部再次检查 hash index，命中后跳过，否则下一轮跳过。
10. 切换 `audio_dir` 后，旧 hash index 保留；新目录中相同内容文件可以复用旧转写产物，但必须满足产物存在和任务配置兼容检查。

`decide_hash_cost_against_asr_cost()` 的策略：

- 小文件或候选数量很少时，可以派发后台精确 hash 并给 ASR 前置闸口一个很小的等待预算；等待超时即继续 ASR，不阻塞任务。
- 大文件、几十 GiB 批量导入、CPU/IO 繁忙或代理服务正在承压时，直接进入 ASR 队列，并把精确 hash/canonical audio hash 留给后台补齐，优化后续重复文件。
- 如果 ASR normalize 阶段本来已经在读完整音频，顺手计算 `canonical_audio_hash`；如果 ASR 失败，不写入可复用 canonical record。

允许跳过 ASR 的唯一条件：

- 同 source_key 或同任务历史记录已 completed/partial_success，且转写产物实际存在。
- 精确 `content_hash` 命中，size 一致，canonical 转写产物实际存在，ASR 参数兼容。
- 精确 `canonical_audio_hash` 命中，normalize 参数版本和 ASR 参数兼容，canonical 转写产物实际存在。

不允许跳过 ASR 的信号：

- 仅 size/mtime 相同。
- 仅 duration/codec 相同。
- 仅 `sample_fingerprint` 或 XXH3/XXH128 采样窗口命中。
- 仅 Chromaprint/声学指纹近似匹配。
- hash 命中但 transcript/timeline/metadata 缺失、损坏或参数不兼容。

兼容检查：

- V1 hash index 只在同一 task 内复用。
- canonical record 必须记录 `model`、`language`、`runtime_strategy`、关键 ASR 参数版本；当前文件参数一致才允许复用。
- `canonical_audio_hash` 还必须记录 normalize pipeline 版本、采样率、声道、sample format 和 ffmpeg/decoder 策略版本；这些字段变化时不能复用。
- Daily Agent report 不作为文件级转写产物复用依据；文件级 transcript/timeline/metadata 才是跳过 ASR 的必要条件。

错误与降级：

- `hash_unavailable`：后台 hash 计算失败，继续普通 ASR，不做内容去重。
- `hash_changed_during_read`：hash 前后 size/mtime 变化，延迟到下一轮。
- `duplicate_artifacts_missing`：hash 命中但产物缺失，当前文件普通转写并刷新 hash index。
- `duplicate_param_mismatch`：hash 命中但 ASR 参数不兼容，当前文件普通转写。

## 复制与原子性

复制流程：

```text
target_dir/.bifrost-import-<uuid>.part
  -> fsync file where available
  -> verify size
  -> rename target_path
  -> set mtime
  -> update external_imports.json atomically
```

关键约束：

- 同一 task 的导入 run 需要文件锁：`external-import:<task_id>`。
- 同一设备同一目标路径不得并发复制。
- 导入过程中设备断开时，当前文件标记 failed/interrupted，保留或清理 temp；不得写入半文件到最终路径。
- 目标磁盘剩余空间不足时，本轮停止复制，记录 `insufficient_space`，不触发 ASR run。
- 导入成功的文件才进入现有 ASR 发现流程；失败文件不写入 `FileStore`。

## 调度与事件流

### 新组件

```text
ExternalDeviceImportManager (planned, not yet shipped as of 2026-06-16)
  - load tasks with external_devices/import_policy
  - start providers
  - maintain per-task queue
  - debounce device events
  - run event/manual reconcile

ExternalVolumeProvider (planned, not yet shipped as of 2026-06-16)
  - list_mounted_volumes()
  - watch_volume_events()

MacDiskArbitrationProvider (planned, not yet shipped as of 2026-06-16)
```

当前实现：`list_external_volumes()` 同步读取 `/Volumes`、对每个卷调用 `diskutil info` 提取 `volume_uuid/device_identifier/external/read_only`，并以 `df -k` 计算 `available_bytes`。导入由 `start_external_import_background()` 在独立线程内调用 `sync_external_devices_for_task()` 执行；尚无事件驱动 manager 或 provider trait。

### 触发入口

- Bifrost admin/server 启动：启动 ASR scheduler 时同时启动 import manager（planned, not yet shipped as of 2026-06-16）。
- Provider 事件：设备 appeared/mounted/description changed/disappeared（planned, not yet shipped as of 2026-06-16）。
- 手动入口：`POST /api/asr/tasks/{task_id}/external-import/run`（已实现，当前忽略 `device_name`，始终对该任务全部 enabled binding 执行 reconcile，并以 `trigger="manual_api"` 记录）。
- ASR run 不隐式扫描外接设备；已经导入到目标目录的文件按普通目录任务处理。

### 去抖与排队

设备刚插入时可能连续出现 appeared、description changed、mounted 等多个事件。处理规则（事件驱动部分 planned, not yet shipped as of 2026-06-16）：

- 事件按 `(task_id, binding_name, mount_path)` debounce 2 秒。
- 同一任务已有 import run 时，新事件只设置 `rerun_requested=true`，当前 run 结束后再跑一次 reconcile。
- 导入完成且 `auto_run_after_import=true` 且 `imported_count > 0` 时，触发现有 ASR task run。
- 如果 task 当前 ASR 正在 running，不抢占；只记录 pending import 或等待本轮 ASR 后再触发。

当前实现：`start_external_import_background()` 通过进程内单 task 互斥（“ASR external import is already running”）拒绝并发；尚无事件 debounce、rerun_requested 标记或 ASR running 抢占协调。

## API 设计

```text
GET  /api/asr/external-volumes
PATCH /api/asr/tasks/{task_id}
DELETE /api/asr/tasks/{task_id}?confirm_name=<task_name>
GET  /api/asr/tasks/{task_id}/external-import
PUT  /api/asr/tasks/{task_id}/external-import
POST /api/asr/tasks/{task_id}/external-import/run
GET  /api/asr/tasks/{task_id}/external-import/runs  (planned, not yet shipped as of 2026-06-16; recent runs 当前直接由 GET external-import 的 `runs` 字段返回)
```

说明：

- `GET /api/asr/external-volumes` 返回当前 mounted volumes：`name`、`mount_path`、`volume_uuid`、`device_identifier`、`kind`、`read_only`、`available_bytes`。
- `PUT` 保存 bindings 和 policy。
- `run` 当前实现忽略请求参数，始终对该任务全部 enabled binding 执行一次 reconcile；`device_name` 单设备触发 planned, not yet shipped as of 2026-06-16。
- `run` 必须后台执行：接口只负责创建导入 run、写入 `current_run` 进度并立即返回 HTTP 202，不能在请求处理链路中等待外接设备扫描和大文件复制完成。
- 后台导入必须运行在独立阻塞任务/worker 中，文件遍历、读取、内容 hash 计算和复制不得占住 Admin API 主请求路径；完整文件 hash 计算还必须通过全局内容哈希队列串行化。导入期间 `GET /api/asr/tasks`、任务详情和状态接口必须持续响应。
- `GET /api/asr/tasks/{task_id}/external-import` 返回 `current_run`：`run_id`、`status`、`current_device`、`current_file`、`current_file_size`、`current_file_copied_bytes`、`processed_files`、`total_files_discovered`、`imported/skipped/processed_record_skipped/failed`、`message`。如果服务重启导致 `current_run.status=importing` 但本进程无对应后台任务，状态归一为 `failed` 并提示用户重新导入。
- 返回状态区分 `ready`、`not_connected`、`ambiguous`、`importing`、`insufficient_space`、`permission_denied`、`device_disconnected`、`completed_with_errors`。

## WebUI 设计

入口放在 ASR Directory Task 创建/编辑和任务详情页，不放到全局 Settings：

- 创建任务 Modal 增加 “External Devices” 区域。
- 用户可从当前已连接设备列表选择，也可手动输入设备名称。
- 每个绑定展示：设备名、当前连接状态、上次看到时间、上次导入结果、UUID 是否已绑定。
- 任务详情页新增 “External Import” tab，展示最近导入 run、导入文件列表、失败原因和手动 Run import 按钮。
- 任务列表中的 `Import External` 点击后按钮进入 loading，但页面不能等待导入完成；前端轮询 `GET /external-import` 的 `current_run`，在任务行下方展示全宽总进度条、当前设备/文件、扫描总数、已处理数、已导入成功数、已有处理记录跳过数和失败数。后台导入必须先扫描出当前设备候选音频文件总数，再进入复制/导入循环；总进度条只表达整体文件处理进度（`processed_files / total_files_discovered`），不能误用当前单文件复制字节数作为主进度。
- ASR 页面刷新或重新进入后，前端必须根据任务列表中的外接设备绑定重新拉取各任务 `current_run`，恢复正在导入的进度展示。
- Paused 状态只阻止 ASR 转写和导入完成后的 auto-run，不阻止外接设备事件导入或手动 `Import External`；用户清理已导入的本地源音频后，如果该文件还没有成功或部分成功的 ASR 处理记录，重新插入设备必须能把缺失文件重新导入；如果同一路径已经有成功或部分成功的处理记录，则视为“已处理归档”，即使本地目标源文件已被清理，也不能再从外设重新导入，进度区必须展示已处理跳过数量。
- 同名设备冲突时 UI 需要提示 “同名卷不唯一，请选择具体 UUID”。
- 目标目录预览必须清楚显示：`<audio_dir>/<device_name>/...`。

WebUI 双主题要求：

- 颜色使用现有 CSS 变量或 Ant Design token。
- Light/Dark 都验证连接状态 tag、错误提示、导入进度和文件表可读。

### 配置页设备发现确认流

打开 ASR Directory Task 创建/编辑配置页面时，页面进入“设备候选发现”模式：

1. 页面先调用 `GET /api/asr/external-volumes` 拉取当前已挂载卷。
2. 页面只把可读、已挂载、非系统卷、未绑定到当前 task 的设备作为 candidate。
3. 页面停留期间继续监听设备连接事件；V1 可以通过轻量轮询 `GET /api/asr/external-volumes` 实现，后续可升级为 SSE。
4. 每个 candidate 独立进入 `DeviceCandidatePromptQueue`，一次只展示一个确认弹窗。
5. 弹窗文案必须明确：
   - 设备名称。
   - 挂载路径。
   - 将导入到的目标根目录：`<audio_dir>/<device_name>/...`。
   - 导入会保持设备内目录结构和文件名。
6. 用户点击确认：
   - 已有任务编辑页：前端立即调用 `PUT /api/asr/tasks/{task_id}/external-import`，把该设备加入 `external_devices`；保存成功后立即调用 `POST /api/asr/tasks/{task_id}/external-import/run?device_name=<device_name>`。
   - 新建任务页面：由于还没有 `task_id`，前端先把该设备加入表单中的 pending `external_devices` 列表；用户点击 Create 并创建任务成功后，立即按用户已确认的设备逐个调用 `POST /api/asr/tasks/{new_task_id}/external-import/run?device_name=<device_name>`。
   - 若 API 返回同名冲突或 UUID ambiguity，弹窗切换为选择具体卷/UUID 的确认态。
   - 该设备在列表中显示 `importing` 或 `pending_import_after_create`，任务详情 External Import tab 同步刷新。
7. 用户点击取消：
   - 不保存 binding，不导入。
   - 当前页面会话内记录 `dismissedCandidates`，避免同一设备反复弹出。
   - 设备断开后重新连接或页面重新打开时可以再次提示。
8. 如果同时发现多个 candidate，必须逐个弹窗确认；确认或取消当前设备后才弹出下一个，不允许一个总弹窗批量绑定全部设备。

该流程是用户授权监听设备的入口：后台 import manager 只会自动导入已绑定设备；未绑定设备即使被页面发现，也必须等用户确认后才监听并导入。

### ASR 任务配置编辑能力

当前 ASR Directory Task 只有创建和删除，没有完整编辑能力。外接设备导入需要一个长期可维护的任务配置页，因此 V1 同时补齐“创建后可编辑”：

可编辑字段：

- `name`：任务名称，立即影响列表、详情页、后续 Daily Docs 标题展示。
- `audio_dir`：本地数据源/导入目标根目录。
- `recursive`：后续扫描是否递归。
- `enabled` / `paused`：启停和暂停状态。
- `schedule`：定时周期和启动时间，覆盖 hourly/daily/weekly/monthly。
- `language`、`model`、`runtime_strategy`：后续新文件处理所用 ASR 参数。
- `daily_agent`：Daily Agent Runner 配置。
- `external_devices` 和 `import_policy`：外接设备绑定与导入策略。

API：

```text
PATCH /api/asr/tasks/{task_id}
```

请求体采用 partial update；未传字段保持不变。服务端保存前必须完整校验：

- `audio_dir` 保存时如果不存在，后端必须自动 `create_dir_all` 创建；如果路径已存在但不是目录，则返回明确错误。
- `schedule` 必须通过现有 `AsrTaskSchedule::validate()`。
- `name` 为空时保留原名称或回退为默认名称。
- `external_devices` 内 `name` 去重，`volume_uuid` 冲突进入明确错误。
- `import_policy` 的时间、大小和空间阈值必须在合理范围内。

运行中编辑规则：

- 如果 ASR task 当前 `summary.running=true`，涉及 `audio_dir`、`recursive`、`model`、`language`、`runtime_strategy`、`external_devices`、`import_policy` 的修改返回 `409 task_running`，避免一次 run 中途切换数据源或模型。
- 运行中允许修改 `name`、`enabled=false`、`paused=true` 和下次 schedule；这些变更不影响当前正在处理的文件，只影响后续运行。
- Force pause 仍按现有 pause/resume 语义释放资源；用户可 pause 后再修改数据源。

切换 `audio_dir` 的核心语义：

- 只更新任务配置和后续扫描根目录。
- 不迁移旧 `audio_dir` 下的源文件。
- 不删除 `<BIFROST_DATA_DIR>/asr/tasks/<task_id>/files.json` 中的既有记录。
- 不删除 `<BIFROST_DATA_DIR>/asr/data/text/<task_id>/` 下已有 `.txt`、`.json`、`.timeline.json`、`daily/` 和 `daily/report/`。
- 旧记录在详情页仍可展示历史转写结果；如果旧源文件不在新目录或已不存在，Source Audio 播放/API 按现有缺失源文件路径返回不可用状态。
- 后续 run 只从新的 `audio_dir` 发现文件；如果新目录为空，则本轮 discovered/pending 不新增，只刷新已有 daily 状态或返回 no-op。
- 新目录中与旧目录同名同 size/mtime 的文件，因为 absolute path 变了，按现有 `source_key()` 会成为新 source record；V1 使用 task 级 algorithm-prefixed content hash index 在 ASR 处理前识别内容重复，若旧记录已完成且转写产物存在，则新文件可跳过模型推理并复用转写结果。

WebUI：

- Directory Task 详情页增加 Edit action，打开与创建任务复用的配置表单。
- 表单内所有字段都可编辑，保存后刷新任务详情。
- 修改 `audio_dir` 时展示确认提示：历史转写数据保留，但后续只扫描新目录；新目录没有文件时不会删除历史结果。
- 如果任务正在运行且用户修改受限字段，Save 按钮展示运行中不可修改原因，并提供 Pause/Force Pause 入口。
- 配置页设备发现确认流复用同一个表单；新建任务使用 pending binding，已有任务保存后立即导入。

### 删除任务危险确认

删除 ASR task 是重操作：它会把任务从列表和 scheduler 中移除，外接设备绑定、定时配置、后续自动导入和 Daily Agent 触发都会停止。V1 删除任务不默认删除 `<BIFROST_DATA_DIR>/asr/data/text/<task_id>/` 下已生成的转写文件和报告；如果未来支持“同时删除生成数据”，必须作为单独危险选项再次确认。

WebUI 删除流程：

1. 列表和详情页的 Delete 不再使用轻量 `Popconfirm`。
2. 点击 Delete 后打开危险确认 Modal。
3. Modal 必须展示任务名称、`audio_dir`、最近运行状态、已发现/已处理/失败文件数量，以及删除后果。
4. 用户必须在输入框中输入完整任务名称，且与当前 `task.name` 精确一致，Delete 按钮才启用。
5. 如果任务正在运行，Modal 展示运行中不可删除，并提供 Pause/Force Pause 引导；真正删除必须等 `summary.running=false`。
6. 删除成功后回到 Directory Tasks 列表，并清理 URL 中的 `asrTask/asrFile/asrDay`。

API 删除确认：

```text
DELETE /api/asr/tasks/{task_id}?confirm_name=<urlencoded task.name>
```

服务端要求：

- 找不到任务返回 404。
- `confirm_name` 缺失或不等于当前任务名称时返回 400 `task_delete_confirmation_required`。
- 任务正在 running 时返回 409 `task_running`。
- 删除成功只移除 task 配置和运行中 bulk retry 状态，不隐式删除转写输出目录。

## CLI 设计

后续 CLI 扩展（planned, not yet shipped as of 2026-06-16，当前 `bifrost-cli` 未提供任何 `external-volumes` / `external-import` 子命令）：

```text
bifrost ai asr task external-volumes
bifrost ai asr task external-import get <task_id>
bifrost ai asr task external-import set <task_id> --device TX_MIC001 --device TX_MIC002
bifrost ai asr task external-import run <task_id> [--device TX_MIC001] [--wait]
```

CLI 和 WebUI 都调用同一 Admin API，不直接扫描设备。

## 容错与边界

| 场景 | 行为 |
|---|---|
| 设备连接但未挂载 | 状态为 `not_mounted`，等待 description changed；如果事件漏掉，用户可手动补跑 |
| 设备中途拔出 | 当前文件失败，run 标记 `device_disconnected`，不触发 ASR |
| 同名设备多个 | 无 UUID 时 `ambiguous`，不导入 |
| 只读设备 | 允许读取导入；不执行源删除 |
| 目标空间不足 | 停止本轮，记录 `insufficient_space` |
| 目标已有不同文件 | 默认不覆盖，记录 `target_modified` |
| 源文件正在写入 | size/mtime 未稳定，deferred 到后续 run |
| 权限不足 | 记录 `permission_denied`，其它文件继续 |
| 非音频/0 字节 | 跳过，不进入 ASR |
| symlink | 默认跳过 |
| 重复文件已转写 | 标记 `duplicate_completed`，复用 canonical transcript，不重复跑 ASR |
| hash 命中但转写产物缺失 | 记录 `duplicate_artifacts_missing`，当前文件正常转写并刷新索引 |
| hash 计算期间文件变化 | 记录 `hash_changed_during_read`，延迟到下一轮 |
| Bifrost 重启且设备已连接 | 不自动扫盘；用户通过任务列表 `Import External` 或配置页确认触发，状态从 external_imports.json 恢复 |

## Sync 边界

- 外接设备绑定、`import_policy`、`external_imports.json`、`content_hash_index.json` 与设备事件均为本机运行时状态，不参与 Rules/Values sync。
- Directory Task 本身当前也不参与 sync；若未来支持"任务云同步"，需要单独设计 device binding 冲突合并策略，本方案不承诺。
- 导入产生的音频文件是本地素材，不通过 Bifrost 同步到远端；用户如需跨设备同步应使用外部工具（例如 iCloud / rsync）。
- 转写产物（txt / metadata / timeline / Daily Docs）当前也不参与 sync。

## 实施计划

### Phase 1：V1 macOS 完整能力

- 新增数据结构和持久化（已实现）。
- 新增导入差异算法、路径安全、原子复制、状态文件和内容 hash index（已实现）。
- 手动入口同步已连接设备（已实现）；设备事件驱动同步 planned, not yet shipped as of 2026-06-16。
- ASR 进入模型前只读取已有 source_key、content hash、canonical audio hash 和产物索引；缺少精确 hash 时不在同步路径补算，命中已完成转写产物时才跳过重复模型推理。
- 新增手动 API 和单元测试。
- 新增 Disk Arbitration provider（planned, not yet shipped as of 2026-06-16）。
- 事件 debounce 入队（planned, not yet shipped as of 2026-06-16）。
- 创建/编辑任务支持绑定设备。
- 任务详情 External Import tab。
- WebUI 展示连接状态、任务列表 `Import External` 手动导入按钮、确认弹窗、最近导入结果和错误状态。
- CLI external-import 子命令（planned, not yet shipped as of 2026-06-16）。
- E2E 覆盖 API、CLI、WebUI。
- human_tests 用 disk image 或真实 U 盘验证目录结构保持、重复连接去重、内容哈希去重、半写入文件延迟、同名冲突和设备断开恢复。

### Phase 2：跨平台增强

- Linux UDisks2 provider。
- Windows WM_DEVICECHANGE provider。
- 可选目录 watcher 优化。

## 测试计划

### 单元测试

- `sanitize_device_root`：特殊字符、空名、同名冲突后缀。
- `safe_relative_path`：拒绝 absolute、`..`、symlink escape。
- `diff_plan_skips_unchanged`：目标 size/mtime 一致跳过。
- `diff_plan_detects_target_modified`：目标被用户改过时不覆盖。
- `stable_file_gate_defers_changing_files`：size/mtime 未稳定时 deferred。
- `external_import_store_roundtrip`：状态文件版本和原子写。
- `volume_match_prefers_uuid`：UUID 优先、同名 ambiguity。
- `hash_dedupe_reuses_completed_transcript`：同 hash 且产物存在时标记 duplicate，不进入 ASR 模型队列。
- `hash_dedupe_requires_artifacts`：hash 命中但 transcript artifact 缺失时不跳过。
- `hash_dedupe_param_mismatch_transcribes`：模型/语言/runtime 不兼容时不复用旧产物。
- `hash_changed_during_read_defers`：hash 前后 size/mtime 变化时延迟处理。

### E2E 测试

- 新增 shell E2E：创建临时 source volume 目录模拟 provider，绑定 `TX_MIC001`，执行 manual import，断言目标结构和文件内容一致。
- 新增 Admin API E2E：`GET external-volumes`、`PUT external-import`、`POST run`、`GET status/runs`。
- 新增 CLI E2E：`external-import set/run --wait`。
- 新增 ASR E2E：两个设备目录下放置同内容不同路径文件，断言两个目标文件都被导入，但只有 canonical 文件执行 ASR，duplicate 文件复用 transcript alias。
- 新增 WebUI Playwright：创建任务时绑定设备，详情页展示 External Import tab，手动导入后列表刷新。
- 新增 WebUI Playwright：打开已有任务配置页时 mock 当前已连接多个设备，断言逐个确认；确认的设备保存 binding 并立即导入，取消的设备不保存不导入。
- 新增 WebUI Playwright：打开新建任务配置页时确认当前已连接设备，断言设备先进入 pending 列表，任务创建成功后立即触发导入。
- 新增 WebUI/API E2E：编辑任务名称、`audio_dir`、schedule、runtime 和外接设备配置，断言旧转写结果保留，新目录为空时不删除历史文件记录。

### human_tests

- 新增 `human_tests/asr-external-device-import.md`。
- 使用 macOS disk image 创建两个可挂载卷，模拟真实外接设备连接/断开。
- 验证 `audio_dir/<device_name>/...` 结构保持。
- 验证重复连接不重复复制。
- 验证跨设备/跨目录同内容文件只转写一次，重复文件在详情页仍能展示复用的转写文本。
- 验证同名冲突、半写入文件、目标空间不足或权限不足的用户可见状态。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：逐条核对设备绑定、自动监听、手动补跑、差异导入、结构保持、容错、路径去重和内容 hash 去重。
- 代码/文档 review：检查方案是否与现有 `AsrDirectoryTask`、`run_directory_task()`、`FileStore`、WebUI Directory Tasks 入口一致。
- 测试运行：执行 design/human_tests 关键字检查，确认方案与测试文档互相覆盖。
- 修复：补齐遗漏的 API、状态或测试项。

### 第 2 轮

- 再次目标复核：确认没有把设备连接监听误设计成仅目录 watcher；确认没有改变现有本地目录任务行为。
- 再次变更范围复核：执行 `git status --short`、`git diff`，检查新增文档和索引。
- 复跑测试：重复文档验收命令。
- 结论：若仍有缺口，追加第 3 轮。

## 风险与决策点

- V1 macOS 事件驱动 provider 尚未落地：当前仅手动补跑 + 配置页轮询兜底；Disk Arbitration 事件订阅、debounce 入队与 `ExternalDeviceImportManager` 是 planned 状态。若在 V1 发布前无法完成，需要明确以"手动 + 轮询"为 GA 交付，并在 UI 中标注设备事件监听为 beta。
- 内容 hash 索引成本：BLAKE3 复制流顺手计算 CPU 成本可控，但历史文件补 hash 若大批量运行仍会占用 IO。后台队列必须串行、低优先级，避免抢占代理主服务。
- 跨路径去重误伤：仅在同 task 内启用 hash 索引，避免跨 task 模型/语言/runtime 不同导致复用错的 transcript。
- 同名卷冲突：同名多个卷插入时 V1 进入 ambiguous 状态不导入，需要用户在 WebUI 手动选卷。ambiguous 状态若长期占位可能造成用户体验困惑，需要在 UI 中主动提示。
- 目标目录被用户手动改动：`target_modified` 场景默认不覆盖，用户可能困惑为何某些文件"没导入"；UI 需要明确展示该状态。
- 半写入文件延迟：`file_stable_secs` 默认 10 秒，若设备写入速率不稳定可能持续延迟；本 V1 不引入复杂的写入速率探测。
- 删除任务与转写产物：V1 删除任务不删除转写产物文件；若未来支持"同时删除生成数据"必须作为独立 danger option 再次确认。
- 平台矩阵：V1 只承诺 macOS；Linux/Windows provider 抽象保留但不作为 GA 能力，避免宣传误导。
- Sync：所有外接设备状态本机保留，不参与 sync；未来跨设备同步方案单独设计。

## 参考资料

- Apple Disk Arbitration Programming Guide: https://developer.apple.com/library/archive/documentation/DriversKernelHardware/Conceptual/DiskArbitrationProgGuide/Introduction/Introduction.html
- Apple Disk Arbitration callbacks: https://developer.apple.com/library/archive/documentation/DriversKernelHardware/Conceptual/DiskArbitrationProgGuide/ArbitrationBasics/ArbitrationBasics.html
- Apple File System Events: https://developer.apple.com/documentation/coreservices/file_system_events
- Rust notify crate: https://docs.rs/notify/latest/notify/
- UDisks2 block devices: https://storaged.org/doc/udisks2-api/latest/ref-dbus-block-devices.html
- Windows WM_DEVICECHANGE: https://learn.microsoft.com/en-us/windows/win32/devio/detecting-media-insertion-or-removal
