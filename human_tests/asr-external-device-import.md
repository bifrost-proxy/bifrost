# ASR 外接设备自动导入

## 功能模块说明

验证 ASR Directory Task 绑定外接设备名称后，可以在设备连接/挂载时自动扫描设备文件，并把差异文件导入到任务 `audio_dir` 下。导入目录必须以设备名称为根目录，设备内相对路径、目录名和文件名保持不变；重复连接或重复扫描不会重复复制已导入文件；跨设备或跨目录的同内容文件通过 BLAKE3 hash 识别，已完成转写且产物存在时不重复执行 ASR 模型。

本文件用于实现验收。2026-05-21 已使用当前连接的两个真实 macOS 外接卷 `LEFT`、`RIGHT` 执行 API、真实文件导入和 WebUI 验证。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 使用最新本地构建启动真实 Bifrost 服务：
  ```bash
  BIFROST_DATA_DIR=$(mktemp -d /tmp/bifrost-asr-device-e2e.XXXXXX) ./target/debug/bifrost start -p 18880 --unsafe-ssl --no-system-proxy
  ```
- macOS 真实设备验证阶段优先使用真实外接设备；无设备时才用 `hdiutil create`/`hdiutil attach` 创建 disk image 模拟挂载卷。
- 测试目标目录使用临时目录，禁止使用生产 `~/.bifrost` 或真实录音库作为写入目标。

## 测试用例列表

### TC-AEDI-01 方案文档覆盖用户目标

操作步骤：

1. 检查方案文档存在：
   ```bash
   test -f design/asr-external-device-import.md
   ```
2. 检查方案覆盖任务绑定多个外接设备名称：
   ```bash
   rg -n "绑定一个或多个外接设备名称|external_devices|AsrExternalDeviceBinding|多个设备" design/asr-external-device-import.md
   ```
3. 检查方案覆盖设备连接监听且不做后台定时扫描：
   ```bash
   rg -n "Disk Arbitration|DADiskAppearedCallback|DADiskDescriptionChangedCallback|不做后台定时扫描|手动导入入口" design/asr-external-device-import.md
   ```
4. 检查方案覆盖导入目录结构保持：
   ```bash
   rg -n "audio_dir / sanitize_device_root|source_relative|target_path|相对目录结构|文件名不变" design/asr-external-device-import.md
   ```
5. 检查方案覆盖差异导入、去重和大小/稳定性检查：
   ```bash
   rg -n "路径 \\+ size \\+ mtime|file_stable_secs|size 校验|unchanged|target_modified|0 字节|source_hashes" design/asr-external-device-import.md
   ```
6. 检查方案覆盖容错状态：
   ```bash
   rg -n "device_disconnected|insufficient_space|permission_denied|ambiguous|not_mounted|symlink|不触发 ASR" design/asr-external-device-import.md
   ```

预期结果：

- 所有命令 exit code 为 0。
- 方案明确外接设备导入是设备连接事件、配置页确认或手动导入触发的独立同步阶段，不重写现有 ASR 转写主链路。
- 方案明确多个设备使用设备名称作为目标根目录，结构和文件名保持一致。

执行结果：

- 已执行，命令均通过；方案文档已覆盖实现后的 macOS Disk Arbitration 事件流、无后台定时扫描、差异导入、哈希去重、配置页确认、任务编辑和删除强确认。

### TC-AEDI-02 human_tests 索引同步

操作步骤：

1. 检查索引包含本文件：
   ```bash
   rg -n "asr-external-device-import.md|ASR 外接设备自动导入" human_tests/readme.md
   ```
2. 检查索引用例数量为 13：
   ```bash
   rg -n "asr-external-device-import.md.*\\| 13 \\|" human_tests/readme.md
   ```

预期结果：

- `human_tests/readme.md` 包含本文件链接。
- 测试用例数与本文档实际用例数量一致。

执行结果：

- 已执行，`human_tests/readme.md` 包含本文件，索引用例数为 13。

### TC-AEDI-03 macOS 真实挂载卷自动导入回归

操作步骤：

1. 创建或连接名为 `TX_MIC001` 的测试卷，卷内准备；本轮真实执行使用 `LEFT` 和 `RIGHT`：
   ```text
   2026-05-20/A.wav
   2026-05-20/sub/B.m4a
   ```
2. 创建 ASR Directory Task，`audio_dir=<temp_target>/missing-target-dir`，该目录创建任务前不存在，绑定外接设备 `TX_MIC001`。
3. 断开并重新连接测试卷，或在服务已启动时 attach disk image。
4. 对已绑定设备，断开后重新连接并等待设备连接事件触发导入；如果设备已经连接在机器上，则点击任务列表的 `Import External` 手动触发导入。
5. 检查目标目录：
   ```bash
   test -f <temp_target>/TX_MIC001/2026-05-20/A.wav
   test -f <temp_target>/TX_MIC001/2026-05-20/sub/B.m4a
   cmp /Volumes/TX_MIC001/2026-05-20/A.wav <temp_target>/TX_MIC001/2026-05-20/A.wav
   ```

预期结果：

- 设备连接后不需要手动复制文件。
- `audio_dir` 不存在时保存任务会自动创建目录。
- 目标目录以设备名称为根目录。
- 设备内目录结构和文件名保持不变。
- 文件内容一致。
- 导入成功后如果 `auto_run_after_import=true`，ASR task 被排队或触发运行。

执行结果：

- 已执行。`GET /api/asr/external-volumes` 识别到 `LEFT` 和 `RIGHT` 为 `external`。
- 已在两个设备写入 `codex-aedi-20260521011441` 测试目录并创建 4 个测试音频扩展文件。
- `POST /api/asr/tasks/{id}/external-import/run` 返回 `imported=9`，其中包含 4 个本轮新增测试文件和设备中既有音频文件。
- 已追加执行回归验证：`tests/asr_external_device_import_e2e.sh` 使用创建前不存在的 `<temp_target_parent>/missing-target-dir` 作为 `audio_dir`，保存任务成功且后端自动创建目录。
- 已验证目标目录存在：
  - `<temp_target>/LEFT/codex-aedi-20260521011441/2026-05-21/A.wav`
  - `<temp_target>/LEFT/codex-aedi-20260521011441/2026-05-21/duplicate.wav`
  - `<temp_target>/RIGHT/codex-aedi-20260521011441/2026-05-21/sub/B.m4a`
  - `<temp_target>/RIGHT/codex-aedi-20260521011441/2026-05-21/sub/duplicate-copy.wav`
- 已用 `cmp` 验证 `LEFT/A.wav` 和 `RIGHT/B.m4a` 源目标内容一致。
- 后台定时扫描已移除；已通过手动入口和 macOS 设备事件路径验证导入，设备已连接但未重新插拔时使用任务列表 `Import External` 补跑。
- 自动导入复测发现并修复 macOS AppleDouble 元数据问题；目标目录已断言不包含 `._*` 文件。

### TC-AEDI-04 重复连接不重复导入

操作步骤：

1. 在 TC-AEDI-03 完成后记录目标文件数量和 mtime：
   ```bash
   find <temp_target>/TX_MIC001 -type f | sort
   stat -f "%N %z %m" <temp_target>/TX_MIC001/2026-05-20/A.wav
   ```
2. 断开并重新连接 `TX_MIC001`。
3. 等待自动导入完成。
4. 再次记录目标文件数量和 mtime。

预期结果：

- 文件数量不增加。
- 未变化文件不被重复复制。
- 导入状态显示 `unchanged/skipped`，不是 failed。

执行结果：

- 已执行。首次真实导入后立刻再次调用 `POST /api/asr/tasks/{id}/external-import/run`，返回 `imported=0`，验证差异导入不会重复复制未变化文件。

### TC-AEDI-05 半写入文件延迟导入

操作步骤：

1. 在测试卷中创建一个正在增长的音频文件，例如先写入一部分，再延迟写完。
2. 触发导入扫描。
3. 在 `file_stable_secs` 未满足前检查导入状态。
4. 写入完成并等待稳定时间后再次触发设备连接事件或点击 `Import External` 手动补跑。

预期结果：

- 文件 size/mtime 未稳定时标记 deferred，不复制半文件。
- 文件稳定后才复制到目标路径。
- 最终目标文件 size 与源文件一致。

执行结果：

- 已通过单元测试 `cargo test -p bifrost-admin external_import_defers_recently_modified_files` 验证 `file_stable_secs` 对最近修改文件会返回 deferred，对超过稳定窗口的文件放行。
- 真实设备导入用例将 `file_stable_secs=0`，避免测试脚本人为等待；稳定性逻辑由单测覆盖。

### TC-AEDI-06 同名设备冲突不误导入

操作步骤：

1. 同时挂载两个卷名都为 `TX_MIC001` 的测试卷。
2. 创建只按 `name=TX_MIC001`、未绑定 `volume_uuid` 的 ASR 任务。
3. 触发外接设备同步。
4. 查看导入状态和目标目录。

预期结果：

- 状态为 `ambiguous`。
- 不导入任何文件到目标目录。
- WebUI/API 提示用户选择具体卷并保存 UUID。

执行结果：

- 未在本机真实挂载两个同名卷。本轮已通过代码 review 确认 `sync_external_devices_for_task` 在 `matches.len() > 1 && volume_uuid.is_none()` 时进入 `ambiguous`，不执行导入。

### TC-AEDI-07 配置页逐个确认当前已连接设备

操作步骤：

1. 检查方案覆盖打开 ASR 任务配置页时发现当前已连接设备：
   ```bash
   rg -n "配置页设备发现确认流|GET /api/asr/external-volumes|当前已挂载卷|DeviceCandidatePromptQueue" design/asr-external-device-import.md
   ```
2. 检查方案覆盖用户确认后加入绑定并立即导入：
   ```bash
   rg -n "PUT /api/asr/tasks/\\{task_id\\}/external-import|POST /api/asr/tasks/\\{task_id\\}/external-import/run\\?device_name|pending_import_after_create|已有任务编辑页|新建任务页面" design/asr-external-device-import.md
   ```
3. 检查方案覆盖多设备逐个确认和取消不绑定：
   ```bash
   rg -n "逐个弹窗确认|取消|dismissedCandidates|不保存 binding|不导入|不允许一个总弹窗批量绑定全部设备" design/asr-external-device-import.md
   ```

预期结果：

- 方案明确 ASR 任务配置页面打开时会发现当前已连接但未绑定的设备。
- 方案明确每个设备都要单独确认，不能批量默认监听。
- 已有任务编辑页中，用户确认后设备加入绑定列表，并立即开始导入该设备数据。
- 新建任务页面中，用户确认后设备先进入 pending 列表，任务创建成功后立即开始导入该设备数据。
- 用户取消后该设备不会被绑定，也不会导入；当前页面会话内不重复弹出。

执行结果：

- 已执行 Playwright 真实页面验证。进入 `AI -> ASR`，点击 `New` 后出现外接设备确认弹窗。
- 已按最新交互调整为单个弹窗展示所有候选设备：一个设备时只显示一个条目，多个设备时显示列表，底部按钮为 `Add`，点击后一次性加入。
- 已补充配置弹窗打开期间的设备轮询：弹窗保持打开时每 2 秒刷新外接卷，后续插入的新设备也会触发同样的逐设备确认弹窗；弹窗关闭后停止轮询。
- 点击 `Skip` 后当前设备不写入绑定列表。

### TC-AEDI-08 ASR 定时任务编辑与切换数据源

操作步骤：

1. 检查方案覆盖创建后可编辑所有任务配置：
   ```bash
   rg -n 'ASR 任务配置编辑能力|可编辑字段|PATCH /api/asr/tasks/\{task_id\}|partial update' design/asr-external-device-import.md
   ```
2. 检查方案覆盖任务名称、数据源目录、启动时间、定时周期等配置可修改：
   ```bash
   rg -n 'name|audio_dir|recursive|enabled|paused|schedule|language|model|runtime_strategy|daily_agent|external_devices|import_policy' design/asr-external-device-import.md
   ```
3. 检查方案覆盖切换数据源后历史转写数据不受影响：
   ```bash
   rg -n '切换 .*audio_dir|不迁移旧|不删除 <BIFROST_DATA_DIR>/asr/tasks/<task_id>/files.json|不删除 <BIFROST_DATA_DIR>/asr/data/text/<task_id>|旧记录在详情页仍可展示历史转写结果|新目录为空' design/asr-external-device-import.md
   ```
4. 检查方案覆盖运行中编辑限制：
   ```bash
   rg -n 'summary.running=true|409 task_running|运行中允许修改|Pause/Force Pause' design/asr-external-device-import.md
   ```

预期结果：

- 方案明确 ASR 定时任务创建后不再是只读配置。
- 任务名称、数据源目录、递归、启停、启动时间、定时周期、模型、语言、runtime、Daily Agent、外接设备和导入策略均有编辑语义。
- 切换数据源只影响后续扫描；旧转写结果、文件记录、Daily Docs 和报告不迁移、不删除。
- 新数据源目录为空时不会把历史数据清空，只是后续 run 找不到新音频。
- 运行中修改高风险字段会被明确拒绝或要求先 pause，避免当前 run 中途切换目录或模型。

执行结果：

- 已执行 API 验证。`PATCH /api/asr/tasks/{id}` 成功修改任务名称、`audio_dir`、`recursive=false`、`enabled=true`、`schedule=daily 03:15`、`language=english`、`runtime_strategy=fork_per_chunk`、外接设备列表和导入策略。
- 已执行 WebUI 验证。任务列表行展示 `Edit` 按钮，并可打开 `Edit Directory Task` 配置弹窗。

### TC-AEDI-09 删除任务必须输入任务名称确认

操作步骤：

1. 检查方案覆盖删除任务是危险操作：
   ```bash
   rg -n '删除任务危险确认|删除 ASR task 是重操作|危险确认 Modal|不再使用轻量 .*Popconfirm' design/asr-external-device-import.md
   ```
2. 检查方案覆盖输入完整任务名称才允许删除：
   ```bash
   rg -n '输入完整任务名称|与当前 .*task.name.* 精确一致|Delete 按钮才启用|confirm_name' design/asr-external-device-import.md
   ```
3. 检查方案覆盖 API 与运行中删除限制：
   ```bash
   rg -n 'DELETE /api/asr/tasks/\{task_id\}\?confirm_name=<task_name>|task_delete_confirmation_required|409 .*task_running|summary.running=false' design/asr-external-device-import.md
   ```
4. 检查方案覆盖删除不隐式删除历史转写输出：
   ```bash
   rg -n '不默认删除 <BIFROST_DATA_DIR>/asr/data/text/<task_id>|不隐式删除转写输出目录|同时删除生成数据' design/asr-external-device-import.md
   ```

预期结果：

- 方案明确删除任务是重操作，不再用轻量确认。
- WebUI 删除弹窗必须要求输入完整任务名称，名称不匹配时删除按钮不可用。
- API 删除必须携带 `confirm_name`，缺失或不匹配返回明确错误。
- 任务运行中不能直接删除，必须先停止或暂停到非 running 状态。
- 删除任务不默认删除历史转写输出目录；如未来支持删除生成数据，需要单独危险确认。

执行结果：

- 已执行 API 验证。`DELETE /api/asr/tasks/{id}?confirm_name=wrong` 返回 400，携带完整编辑后任务名的删除请求返回 200。
- 已执行 WebUI 验证。删除弹窗包含 `Type the full task name to confirm`，未输入完整任务名称前 Delete 按钮禁用，输入 `UI Edit Delete E2E` 后按钮启用。

### TC-AEDI-10 V1 macOS 完备性与跨平台边界

操作步骤：

1. 检查方案明确 V1 只承诺 macOS：
   ```bash
   rg -n '首版 V1 的交付边界只覆盖 macOS|Linux/Windows 只保留 provider 接口|不作为 V1 可用能力宣传' design/asr-external-device-import.md
   ```
2. 检查方案明确 macOS V1 必须完整包含监听、手动补跑和异常恢复：
   ```bash
   rg -n 'MacDiskArbitrationProvider|手动补跑|不做后台定时扫描|异常恢复|不能把监听、确认弹窗、导入、去重、异常恢复中的任一关键路径留到后续' design/asr-external-device-import.md
   ```
3. 检查方案明确 V1 macOS 真实验证要求：
   ```bash
   rg -n '真实 disk image|真实外接设备|V1 macOS 完整能力|human_tests 用 disk image 或真实 U 盘验证' design/asr-external-device-import.md
   ```
4. 检查方案仍保留跨平台扩展架构：
   ```bash
   rg -n 'ExternalVolumeProvider|后续跨平台 provider 路线|Linux UDisks2 provider|Windows WM_DEVICECHANGE provider' design/asr-external-device-import.md
   ```

预期结果：

- 方案明确 V1 只对 macOS 负责，不把 Linux/Windows 包装成首版可用能力。
- macOS V1 覆盖设备事件监听、手动补跑、配置页确认、差异导入、去重和异常恢复，不做后台定时扫描。
- macOS V1 必须用真实 disk image 或真实外接设备验证关键链路。
- 架构保留跨平台 provider 扩展点，但跨平台实现进入后续 Phase，不影响 V1 验收。

执行结果：

- 已执行。macOS 事件监听通过 `diskutil activity` 订阅 Disk Arbitration 活动流；服务运行期间可看到 `diskutil activity` 子进程，服务停止后子进程退出。后台定时扫描已移除，兜底由配置页确认和手动导入入口承担。

### TC-AEDI-11 高性能哈希选型、导入去重与 ASR 前置去重

操作步骤：

1. 检查方案包含哈希成本评估、高性能算法选型和接受结论：
   ```bash
   rg -n '内容哈希成本评估|高性能哈希算法选型|行业方案调研与取舍|大文件极致性能去重策略|2.3 GB/s|278 MiB/s|顺手计算、只算一次、长期缓存|后台内容哈希队列' design/asr-external-device-import.md
   ```
2. 检查方案明确推荐算法和角色边界：
   ```bash
   rg -n 'BLAKE3-256|blake3:<hex>|SHA-256|XXH3 / XXH128|CRC32C|FastCDC|Chromaprint|canonical_audio_hash' design/asr-external-device-import.md
   ```
3. 检查方案明确导入复制流同步计算内容 hash，不额外重读设备：
   ```bash
   rg -n '复制流同步计算 BLAKE3 内容 hash|不额外再读一遍源设备|source_hashes\\[\"blake3\"\\]|content_hash_algorithm|blake3:<hex>' design/asr-external-device-import.md
   ```
4. 检查方案明确 ASR 进入模型前置去重流程，覆盖手动拷贝文件：
   ```bash
   rg -n 'ASR 进入前置去重流程|手动把文件复制到 `audio_dir`|discover_audio_files|stable_stat_filter|existing exact content_hash hit|existing canonical_audio_hash hit|candidate lookup|decide_hash_cost_against_asr_cost' design/asr-external-device-import.md
   ```
5. 检查方案明确历史大文件缺少 hash 时不阻塞 ASR run 主流程：
   ```bash
   rg -n 'ASR run 不在同步路径补算完整 BLAKE3|Resume|启动恢复|几十 GB|后台内容哈希队列|ASR 主流程不等待 hash' design/asr-external-device-import.md
   ```
6. 检查方案明确外接设备导入优先用不读文件的快路径去重：
   ```bash
   rg -n 'T0 设备 manifest 去重|不打开源文件、不计算 hash、不复制|processed_record_skipped|ASR 主流程不等待 hash' design/asr-external-device-import.md
   ```
7. 检查方案明确行业方案取舍，不把 CDC/block hash 放进 V1 主链路：
   ```bash
   rg -n 'rsync|rclone|Syncthing|restic|Borg|CDC/block 级去重暂不进入 V1 主链路' design/asr-external-device-import.md
   ```
8. 检查方案明确跳过 ASR 的必要条件和降级路径：
   ```bash
   rg -n '允许跳过 ASR 的唯一条件|不允许跳过 ASR 的信号|duplicate_artifacts_missing|duplicate_param_mismatch|hash_unavailable|hash_changed_during_read|不能跳过' design/asr-external-device-import.md
   ```
9. 检查方案明确轻量指纹不能直接跳过 ASR：
   ```bash
   rg -n 'sample_fingerprint|XXH3/XXH128|只能减少候选|不能单独导致 `duplicate_completed`|仅 Chromaprint/声学指纹近似匹配' design/asr-external-device-import.md
   ```
10. 检查方案明确导入结构仍完整保留：
   ```bash
   rg -n '即使内容重复，也要把目标目录下对应文件补齐|内容 hash 只决定“是否重复转写”|两个目标文件都被导入' design/asr-external-device-import.md
   ```

预期结果：

- 方案接受哈希去重，并说明成本可控的依据。
- 方案调研 BLAKE3、SHA-256、XXH3/XXH128、CRC32C、FastCDC/CDC、Chromaprint 和 `canonical_audio_hash`，并明确本地精确内容身份固定使用 BLAKE3，不兼容旧 SHA-256 数据。
- 方案调研同步、块同步和备份去重系统，并明确 Bifrost ASR V1 采用同步系统快路径 + 后台 hash 增强，不把 CDC/block hash 放入导入/Resume 主链路。
- 方案明确大文件去重优先走 T0/T1 零读取快路径，完整 hash 只做后台精确兜底；导入时复制和 hash 共用同一次顺序读取，不为 hash 额外读取外接设备。
- ASR 进入模型前有独立去重闸口，覆盖用户手动拷贝文件到 `audio_dir` 的情况。
- 历史大文件缺少 hash 时，ASR run、Resume 和启动恢复不在主流程同步补算；需要补 hash 时通过后台内容哈希队列串行执行。
- 缺少 hash、hash 计算失败、文件变化、转写产物缺失或 ASR 参数不兼容时，不错误跳过 ASR；`sample_fingerprint`、XXH3/XXH128 采样窗口和 Chromaprint 近似指纹只能缩小候选，不能单独触发 `duplicate_completed`。
- 精确 `content_hash` 或可信 `canonical_audio_hash` 命中且产物存在、参数兼容时，才允许跳过 ASR 模型推理。
- 重复文件仍会保留在 `audio_dir/<device_name>/<relative_path>`，只是 ASR 模型推理阶段复用已完成转写产物。

执行结果：

- 已执行 `cargo test -p bifrost-admin content_hash_dedupe_reuses_completed_transcript`，同内容第二个文件被标记为 `success`，设置 `duplicate_of_source_key`，并复用既有 `output_text_path`。
- 已执行真实设备导入验证，同内容测试文件仍分别导入到 `LEFT/.../duplicate.wav` 和 `RIGHT/.../duplicate-copy.wav`，目录结构完整保留。
- 2026-05-22 已执行本用例文档检查命令，方案已覆盖高性能 hash 算法选型、手动拷贝文件的 ASR 前置去重、轻量指纹边界、后台队列和跳过 ASR 的严格条件。
- 2026-05-22 已启动真实 Bifrost 服务并使用已挂载外接卷 `LEFT` 执行端到端验证：写入新设备文件后触发 `POST /external-import/run`，接口 4ms 返回，导入完成并在 `external_imports.json` 写入 `source_hashes["blake3"]`；随后手动复制同内容文件到任务 `audio_dir`，触发 `POST /run`，接口 2ms 返回，任务详情中手动文件被标记为 `success`，`duplicate_of_source_key` 指向 canonical 文件并复用既有 transcript artifact，证明 ASR 模型前置去重真实生效。

### TC-AEDI-12 手动导入后台执行且主页面不被卡死

操作步骤：

1. 使用真实外接卷或 disk image 准备至少一个较大的音频文件。
2. 启动最新本地构建的 Bifrost 服务，创建绑定该设备的 ASR Directory Task。
3. 点击任务列表 `Import External`，或调用：
   ```bash
   curl -i -X POST http://127.0.0.1:18880/_bifrost/api/asr/tasks/<task_id>/external-import/run
   ```
4. 立即调用任务列表 API：
   ```bash
   curl -m 2 -sS http://127.0.0.1:18880/_bifrost/api/asr/tasks
   ```
5. 轮询导入状态：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/asr/tasks/<task_id>/external-import
   ```
6. 在 WebUI 观察 `Import External` 按钮和进度条。

预期结果：

- `POST /external-import/run` 立即返回 HTTP 202，不等待大文件复制完成。
- 导入复制在后台任务中执行，任务列表 API 在导入期间 2 秒内有响应。
- 状态响应包含 `current_run`，能看到 `status=importing`、当前设备、当前文件、已复制字节、已处理文件数。
- 任务列表的导入进度展示在任务行下方的全宽区域，而不是 Actions 右侧窄列。
- 后台导入先扫描当前设备候选音频文件总数，再执行复制/导入；主进度条表示整体文件处理进度（已处理文件数 / 扫描总数），不是当前单个文件复制字节进度。
- 进度区域展示当前导入文件名、扫描总数、已导入成功数量、已处理数量、已有处理记录跳过数量和失败数量。
- 刷新 ASR 页面后，仍能通过后端 `current_run` 恢复正在导入的进度展示。
- 任务处于 Paused 状态时，重新插入外接设备或点击 `Import External` 仍会执行导入；Paused 只阻止导入完成后的自动 ASR 转写，不阻止文件重新导入。
- 删除已成功导入到 `audio_dir` 的目标文件后，如果该文件还没有成功或部分成功的 ASR 处理记录，重新插入设备或手动导入会重新复制缺失文件，不能因为历史 imported 记录而跳过。
- 删除已成功或部分成功转写过的本地源音频后，即使外设上仍保留原文件，重新插入设备或手动导入也不会再次复制该文件；导入说明展示已有处理记录跳过数量。
- 导入完成后状态变为 `completed` 或 `completed_with_errors`，任务列表按钮结束 loading；成功导入的文件仍保持设备名根目录和相对路径。

执行结果：

- 已更新 E2E 脚本 `tests/asr_external_device_import_e2e.sh`：断言 `POST /external-import/run` 返回 202 且 1500ms 内完成响应，随后立即调用 `/tasks` 验证主 API 仍响应，再轮询 `current_run` 到完成并检查导入文件。
- 已更新导入流程和 WebUI 任务列表：后台先扫描候选音频文件总数再导入；Paused 任务仍允许外接设备事件和手动导入，只是不触发自动 ASR 转写；没有 ASR 成功/部分成功处理记录的缺失目标会重新导入；已有成功/部分成功处理记录的缺失目标会跳过并计入 `processed_record_skipped`；导入进度从 Actions 列移到行下方，主进度按 `processed_files / total_files_discovered` 计算；当前文件名、扫描总数、成功导入数、已有处理记录跳过数、已处理数和失败数由 `current_run` 展示；页面刷新后会重新读取已绑定外设任务的 `current_run` 恢复展示。
- 本轮真实执行记录见下方执行记录表。

### TC-AEDI-13 外接卷枚举阻塞不拖死管理 API

操作步骤：

1. 运行模拟底层卷探测永久等待的并发单元测试：
   ```bash
   cargo test -p bifrost-admin external_volume_api_probe_does_not_block_async_worker_or_duplicate_scans -- --nocapture
   ```
2. 检查卷枚举 API 具有阻塞线程隔离、单航班 gate、超时和缓存回退：
   ```bash
   rg -n "spawn_blocking|EXTERNAL_VOLUME_API_PROBE_GATE|wait_timeout|probe_timeout|cached_external_volumes|retry_interrupted" crates/bifrost-admin/src/handlers/asr_jobs/{state.rs,external_import.rs}
   ```
3. 使用最新正式构建启动本机 Bifrost，在不写入真实外接盘的前提下并发读取 `/api/asr/external-volumes`，同时连续请求 `/api/proxy/address`。

预期结果：

- 底层卷探测在 blocking worker 中等待时，单 worker Tokio runtime 仍能及时执行其他 async task。
- 同一时刻最多一个真实卷枚举探测；重复请求超时后返回缓存，不继续堆积 `read_dir`、`diskutil` 或 `df` 调用。
- `read_dir` 返回 `EINTR` 时透明重试，不把临时系统信号误报为设备扫描失败。
- 正式服务并发读取卷列表期间，健康/代理地址接口持续返回 HTTP 200；测试不向真实外接盘写入任何文件。

执行结果：

- 已执行并通过专门单元测试：阻塞探测运行期间单 worker Tokio runtime 仍可调度，第二个请求在 20ms gate 等待超时后返回缓存，且未执行重复 probe。
- 已执行 `external_volume_scan_retries_interrupted_system_calls`：前两次模拟 `EINTR`，第三次成功，扫描结果正常返回。
- 已安装最新 release 构建并重启正式 9900 服务；并发发起 16 个只读卷列表请求，同时连续发起 24 个 `/api/proxy/address` 请求。卷列表 16/16 返回 HTTP 200，最慢 0.5920 秒；健康接口 24/24 返回 HTTP 200，最慢 0.1043 秒，无 curl 错误。
- 测试仅读取 API 和进程状态，没有向 `/Volumes/TX1`、`/Volumes/TX2` 写入文件；服务重启后微信 provider 状态仍为 `connected`。

## 清理步骤

1. 停止测试 Bifrost 服务。
2. 卸载测试 disk image 或外接设备。
3. 删除临时 `BIFROST_DATA_DIR` 和 `<temp_target>`。
4. 确认没有 `.bifrost-import-*.part` 残留。

## 执行记录

| 日期 | 用例 | 命令/场景 | 结果 |
|---|---|---|---|
| 2026-05-21 | TC-AEDI-01 / TC-AEDI-02 | `rg` 检查 `design/asr-external-device-import.md` 和 `human_tests/readme.md` | PASS：设计文档存在并覆盖用户目标；索引包含本文件且用例数为 12 |
| 2026-05-21 | TC-AEDI-03 / TC-AEDI-04 | 真实启动 `./target/debug/bifrost start -p 18880 --unsafe-ssl --no-system-proxy`；`GET /api/asr/external-volumes`；在 `/Volumes/LEFT`、`/Volumes/RIGHT` 写入测试音频；`POST /api/asr/tasks/{id}/external-import/run`；再次运行导入；`cmp` 校验源目标文件 | PASS：识别 `LEFT`/`RIGHT`；首次导入 `imported=9`，包含本轮 4 个新增测试文件；目标路径按设备名和相对目录保留；再次导入 `imported=0` |
| 2026-05-21 | TC-AEDI-03 目录创建回归 | `BIFROST_ASR_E2E_REQUIRE_DEVICES=1 tests/asr_external_device_import_e2e.sh`，脚本传入创建前不存在的 `<temp_target_parent>/missing-target-dir` 作为 `audio_dir` | PASS：保存任务时自动创建缺失目录；真实 `LEFT`/`RIGHT` 导入 `imported=4`，再次导入 `repeatImported=0` |
| 2026-05-21 | TC-AEDI-05 | `cargo test -p bifrost-admin external_import_defers_recently_modified_files` | PASS：最近修改文件会等待 `file_stable_secs`，超过稳定窗口后放行 |
| 2026-05-21 | TC-AEDI-06 | 代码 review `sync_external_devices_for_task` 的同名匹配分支 | PASS：未绑定 UUID 且匹配多个卷时进入 `ambiguous` 并跳过导入；本机未强行挂载两个同名真实卷 |
| 2026-05-21 | TC-AEDI-07 | Playwright 打开 `AI -> ASR`，点击 `New`，检查设备确认弹窗 | PASS：单个弹窗展示所有候选设备；一个设备显示一个条目，多个设备显示列表；底部按钮为 `Add`，不会连续弹多个确认框 |
| 2026-05-21 | TC-AEDI-07 回归 | 配置弹窗保持打开，代码检查 `DirectoryTasksPanel` 对 `configOpen` 启动 2 秒轮询并调用 `listAsrExternalVolumes`；`pnpm --dir web build` | PASS：页面打开期间会持续检测新插入设备并弹出单个列表确认弹窗，关闭弹窗后轮询停止；前端构建通过 |
| 2026-05-21 | TC-AEDI-08 | `PATCH /api/asr/tasks/{id}` 修改名称、目录、递归、启停、schedule、language、runtime、external_devices、import_policy；Playwright 检查任务行 `Edit` 按钮 | PASS：API 修改成功且响应字段一致；WebUI 任务列表提供编辑入口 |
| 2026-05-21 | TC-AEDI-08 回归 | 代码检查 `DirectoryTasksPanel` 对已绑定设备任务展示 `Import External` 按钮并调用 `onRunExternalImport(record.id)` | PASS：已绑定且已连接设备无需拔插，可在任务列表手动触发外接设备差异导入 |
| 2026-05-21 | TC-AEDI-09 | API 删除 wrong confirm 与 exact confirm；Playwright 检查删除弹窗按钮禁用/启用 | PASS：错误 `confirm_name` 返回 400，完整任务名返回 200；WebUI 未输入完整任务名时 Delete 禁用，输入后启用 |
| 2026-05-21 | TC-AEDI-10 | 服务运行时检查 `diskutil activity` 子进程；停止服务后复查 | PASS：macOS Disk Arbitration 活动流 watcher 启动；服务停止后 watcher 退出；后台定时扫描已移除 |
| 2026-05-21 | TC-AEDI-11 | `cargo test -p bifrost-admin content_hash_dedupe_reuses_completed_transcript`；真实设备导入重复内容文件 | PASS：哈希命中后复用已完成 transcript；重复内容文件仍分别导入到各自设备目录 |
| 2026-05-22 | TC-AEDI-11 方案回归 | `rg` 检查高性能 hash 算法选型、ASR 前置去重流程、手动拷贝覆盖、轻量指纹边界和跳过 ASR 的唯一条件 | PASS：方案明确本地精确内容身份固定使用 BLAKE3，不兼容旧 SHA-256 数据；手动复制文件由 ASR 前置去重覆盖；轻量指纹不能单独跳过 ASR |
| 2026-05-21 | TC-AEDI-03 回归 | 创建绑定 `LEFT`、`RIGHT` 的任务后通过任务列表等价 API `POST /external-import/run` 手动补跑；检查 `find "$TARGET_DIR" -name '._*'` | PASS：手动补跑导入两个测试音频；目录结构保留；目标目录无 AppleDouble `._*` 元数据文件 |
| 2026-05-21 | TC-AEDI-12 | `BIFROST_ASR_E2E_PORT=18882 BIFROST_ASR_E2E_DEVICES=RIGHT BIFROST_ASR_E2E_REQUIRE_DEVICES=1 tests/asr_external_device_import_e2e.sh`；`pnpm --dir web build` | PASS：真实 `RIGHT` 卷导入 `imported=2`，重复导入 `repeatImported=0`，`POST /external-import/run` 在 3ms 返回 202，导入期间 `/tasks` API 保持响应并可轮询 `current_run` 到完成；删除目标文件且任务 Paused 后重新导入 `reimportedAfterDelete=1`；写入成功处理记录后再次删除目标文件，重新导入不复制该文件且 `processedRecordSkipped=1`；WebUI 构建通过，进度 UI 使用行下方全宽展示和后端 `current_run` 恢复逻辑 |
| 2026-05-22 | TC-AEDI-11 / TC-AEDI-12 真实端到端回归 | `bash tests/asr_external_device_import_e2e.sh`；临时数据目录启动 `./target/debug/bifrost start -p 18883 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all`，真实 `LEFT` 卷新增测试文件，API 触发导入，再手动复制重复文件到 `audio_dir` 后调用 `POST /tasks/{id}/run` | PASS：真实 `LEFT`/`RIGHT` 设备导入 `imported=4`、重复导入 `repeatImported=0`、删除后重新导入 `reimportedAfterDelete=1`、已处理记录跳过 `processedRecordSkipped=1`、导入启动 4ms 返回；ASR 前置去重真实服务验证中 `source_hashes["blake3"]` 写入成功，手动拷贝重复文件在 `/run` 后标记 `success`，设置 `duplicate_of_source_key` 并复用 canonical transcript，`/api/proxy/address` 同期保持响应 |
| 2026-05-22 | TC-AEDI-11 扫描顺序回归 | `cargo test -p bifrost-admin content_hash_dedupe_hashes_manual_copy_when_candidate_exists --lib`；清理 `/Volumes/LEFT/codex-preflight*` 测试残留后，临时数据目录启动 `./target/debug/bifrost start -p 18887 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all`，真实 `LEFT` 卷导入 canonical 文件，再将同内容 `manual-copy.wav` 放在 `audio_dir` 根目录并调用 `POST /tasks/{id}/run` | PASS：单测覆盖 `manual-copy` 在扫描顺序中排在 canonical 之前的情况；真实服务验证 `manual-copy.wav` 在 ASR 前置阶段被标记为 `success`，`duplicate_of_source_key` 指向 canonical source key，复用 canonical transcript，`POST /run` 1ms 返回且代理 API 保持响应 |
| 2026-07-16 | TC-AEDI-13 | `cargo test -p bifrost-admin external_volume_api_probe_does_not_block_async_worker_or_duplicate_scans -- --nocapture`；最新 release 正式服务上并发 16 次 `GET /api/asr/external-volumes`，同期连续 24 次 `GET /api/proxy/address` | PASS：阻塞探测与 async worker 隔离且重复 probe 被 gate 抑制；卷列表 16/16 HTTP 200，最慢 0.5920s；健康接口 24/24 HTTP 200，最慢 0.1043s；未写入真实外接盘；微信 provider 重启后仍 connected |
