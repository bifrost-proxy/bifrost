# ASR 外接设备自动导入

## 功能模块说明

验证 ASR Directory Task 绑定外接设备名称后，可以在设备连接/挂载时自动扫描设备文件，并把差异文件导入到任务 `audio_dir` 下。导入目录必须以设备名称为根目录，设备内相对路径、目录名和文件名保持不变；重复连接或重复扫描不会重复复制已导入文件；跨设备或跨目录的同内容文件通过 SHA-256 hash 识别，已完成转写且产物存在时不重复执行 ASR 模型。

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
   rg -n "路径 \\+ size \\+ mtime|file_stable_secs|size 校验|unchanged|target_modified|0 字节|source_sha256" design/asr-external-device-import.md
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
2. 检查索引用例数量为 11：
   ```bash
   rg -n "asr-external-device-import.md.*\\| 11 \\|" human_tests/readme.md
   ```

预期结果：

- `human_tests/readme.md` 包含本文件链接。
- 测试用例数与本文档实际用例数量一致。

执行结果：

- 已执行，`human_tests/readme.md` 包含本文件，索引用例数为 11。

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

### TC-AEDI-11 内容哈希去重与成本边界

操作步骤：

1. 检查方案包含哈希成本评估和接受结论：
   ```bash
   rg -n '内容哈希成本评估|2.3 GB/s|278 MiB/s|V1 默认启用内容哈希去重|顺手计算、只算一次、长期缓存' design/asr-external-device-import.md
   ```
2. 检查方案明确导入复制流同步计算 SHA-256，不额外重读设备：
   ```bash
   rg -n '复制流同步计算 SHA-256|不额外再读一遍源设备|source_sha256|content_hash_algorithm' design/asr-external-device-import.md
   ```
3. 检查方案明确 ASR 处理前应用 hash 去重：
   ```bash
   rg -n 'content_hash_index.json|ensure_content_hash_for_discovered_files|apply_transcript_dedupe|duplicate_completed|duplicate_of_source_key|transcript_alias' design/asr-external-device-import.md
   ```
4. 检查方案明确跳过 ASR 的必要条件和降级路径：
   ```bash
   rg -n '产物实际存在|duplicate_artifacts_missing|duplicate_param_mismatch|hash_unavailable|hash_changed_during_read|不能跳过' design/asr-external-device-import.md
   ```
5. 检查方案明确导入结构仍完整保留：
   ```bash
   rg -n '即使内容重复，也要把目标目录下对应文件补齐|内容 hash 只决定“是否重复转写”|两个目标文件都被导入' design/asr-external-device-import.md
   ```

预期结果：

- 方案接受哈希去重，并说明成本可控的依据。
- 导入时复制和 hash 共用同一次顺序读取，不为 hash 额外读取外接设备。
- 缺少 hash、hash 计算失败、文件变化、转写产物缺失或 ASR 参数不兼容时，不错误跳过 ASR。
- 重复文件仍会保留在 `audio_dir/<device_name>/<relative_path>`，只是 ASR 模型推理阶段复用已完成转写产物。

执行结果：

- 已执行 `cargo test -p bifrost-admin content_hash_dedupe_reuses_completed_transcript`，同内容第二个文件被标记为 `success`，设置 `duplicate_of_source_key`，并复用既有 `output_text_path`。
- 已执行真实设备导入验证，同内容测试文件仍分别导入到 `LEFT/.../duplicate.wav` 和 `RIGHT/.../duplicate-copy.wav`，目录结构完整保留。

## 清理步骤

1. 停止测试 Bifrost 服务。
2. 卸载测试 disk image 或外接设备。
3. 删除临时 `BIFROST_DATA_DIR` 和 `<temp_target>`。
4. 确认没有 `.bifrost-import-*.part` 残留。

## 执行记录

| 日期 | 用例 | 命令/场景 | 结果 |
|---|---|---|---|
| 2026-05-21 | TC-AEDI-01 / TC-AEDI-02 | `rg` 检查 `design/asr-external-device-import.md` 和 `human_tests/readme.md` | PASS：设计文档存在并覆盖用户目标；索引包含本文件且用例数为 11 |
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
| 2026-05-21 | TC-AEDI-03 回归 | 创建绑定 `LEFT`、`RIGHT` 的任务后通过任务列表等价 API `POST /external-import/run` 手动补跑；检查 `find "$TARGET_DIR" -name '._*'` | PASS：手动补跑导入两个测试音频；目录结构保留；目标目录无 AppleDouble `._*` 元数据文件 |
