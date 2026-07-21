# ASR 历史录音辅助声纹初始化

## 功能模块说明

验证用户可从已经完成的多人会议转录中试听 speaker 候选片段，逐段标注本人语音，并把
多个确认片段保存为同一身份的多模板、多 prototype 声纹。覆盖追加样本、独立删除、旧
实时录入兼容、保守匹配、隐私边界和亮暗主题。

## 前置条件

1. 在仓库根目录执行。
2. 本地安装 `ffmpeg`、Node.js、pnpm 和 Playwright Chromium。
3. 所有服务使用脚本创建的临时 `BIFROST_DATA_DIR`、独立端口和 `--no-system-proxy`；不得复用 9900。
4. 不使用正式声纹、正式录音或 `~/.bifrost` 数据。

## 测试用例列表

### TC-AVA-01：真实 Admin API 完成历史录音初始化

操作步骤：

1. 执行 `e2e-tests/tests/test_asr_assisted_voiceprint_api.sh`。

预期结果：

- 临时服务启动后创建 speaker-aware task/file fixture。
- 创建 assisted session，至少返回三个无重叠且不少于 3 秒的候选。
- 标注三个 `mine` 后服务端返回 `ready_to_finish=true`。
- finish 写入 schema v2 Profile，包含独立 template 和 prototype，session 临时目录被清理。

### TC-AVA-02：亮色主题试听、标注和完成门禁

操作步骤：

1. 执行 `cd web && pnpm exec playwright test tests/ui/asr-home-tabs.spec.ts --grep "历史录音声纹初始化"`。
2. 检查测试中的亮色主题路径：打开 `Voiceprint & Wake`，点击 `Add from Recording`，选择任务和录音。
3. 确认播放器使用 task file source URL，逐个点击前三段的 `Mine`。

预期结果：

- 未满足三段/12 秒时 `Save Voiceprint` 禁用且显示原因。
- 三段累计 12 秒后按钮启用，候选显示 speaker、时间、文本、时长和质量。
- 保存成功后弹窗关闭。

### TC-AVA-03：暗色主题初始化入口可读

操作步骤：

1. 继续执行 TC-AVA-02 的同一 Playwright 用例。
2. 完成亮色流程后切换暗色主题，再次打开 `Add from Recording`。

预期结果：

- `html[data-theme="dark"]` 生效。
- 暗色主题下说明、姓名输入、任务和录音选择器可见，没有硬编码亮色背景或不可读文字。

### TC-AVA-04：向同一身份追加样本并独立删除

操作步骤：

1. 执行 TC-AVA-01。
2. 检查脚本第二次以首次返回的 `profile_id` 创建 session 并完成。
3. 检查脚本调用 `DELETE /speaker-profiles/{id}/samples/{sample_id}`。

预期结果：

- 第二次完成后同一 Profile 有 6 个 template，不创建第二个身份。
- 删除一个样本后剩余 5 个 template，centroid/prototype 已原子重建。
- 删除实时朗读 template 时，对应的旧版 prompt 样本元数据也同步删除，不留下幽灵样本。
- Profile 仍可读取和删除。

### TC-AVA-05：旧实时朗读声纹继续兼容

操作步骤：

1. 执行 `cargo test -p bifrost-admin voiceprint_ --lib -- --nocapture`。

预期结果：

- 旧 JSON 缺少 `schema_version/templates/prototypes` 时仍可反序列化和匹配。
- 实时朗读 finish 仍成功，并写入 v2 template/prototype。
- `deleting_live_template_removes_its_legacy_prompt_metadata` 验证实时朗读模板与旧元数据同步删除。
- 实时 identify、单 Profile self-priority 和低阈值 candidate 回归全部通过。

### TC-AVA-06：未知人数和重叠音频不会污染声纹

操作步骤：

1. 执行 `cargo test -p bifrost-admin diarization_cluster_count_is_fixed_only_when_known --lib`。
2. 执行 `cargo test -p bifrost-admin assisted_candidates_ --lib`。

预期结果：

- 仅 `known_speaker_count` 设置 Sherpa 固定 cluster 数；`max_speakers` 不再强制四类或其他固定类数。
- overlap、无 speaker 和不足 3 秒的 timeline segment 不生成候选。
- 长片段按 12 秒切分，每个 speaker 最多 8 个候选。

### TC-AVA-07：路径注入、临时数据和永久适配边界

操作步骤：

1. 执行 TC-AVA-01。
2. 检查脚本向 session create 请求额外发送 `source_path=/tmp/forbidden.wav`。
3. 检查 finish 前后临时 session 和源录音状态，并创建另一个 session 后调用取消 API。

预期结果：

- 服务端因未知字段返回 HTTP 400，只允许从 task/file store 解析源音频。
- finish 只裁剪选中的临时 PCM；完成或取消后删除 session，不删除或复制完整源录音。
- 未经用户 `mine` 确认的片段不会写入 Profile。

### TC-AVA-08：方案边界与上线质量门禁一致

操作步骤：

1. 执行 `rg -n "多模板|多原型|conflict_margin|TS-VAD|99%|0.5%|coverage-all" design/asr-assisted-voiceprint-enrollment.md`。
2. 执行 `rg -n "Add from Recording|Record Voice|ready_to_finish|prototype" web/src/pages/ASR/components/DiarizationSetupCard.tsx crates/bifrost-admin/src/handlers/asr_jobs/voiceprint.rs`。
3. 确认 `scripts/ci/proxy-coverage-shell-tests.txt` 包含 `test_asr_assisted_voiceprint_api.sh`。

预期结果：

- 设计明确本次交付和 TS-VAD 后续模型边界，不把未完成能力描述为已上线。
- UI 推荐历史录音初始化，同时保留实时录音入口。
- 上线门禁包含 precision/误认率黄金集目标、changed-lines 和 workspace coverage CI。
- PR 的 proxy coverage 使用 instrumented Bifrost 二进制执行本功能真实 API E2E，而不是只依赖 mock。

### TC-AVA-09：Proxy coverage manifest 契约包含新增 ASR E2E

操作步骤：

1. 执行 `test "$(wc -l < scripts/ci/proxy-coverage-shell-tests.txt | tr -d ' ')" -eq 14`。
2. 执行 `tail -n 1 scripts/ci/proxy-coverage-shell-tests.txt`。
3. 执行 `bash e2e-tests/tests/test_coverage_pipeline_contract.sh`。
4. 执行 `rg -n "expected_proxy_coverage_shell_tests=14|manifest count mismatch" e2e-tests/tests/test_coverage_pipeline_contract.sh`。

预期结果：

- manifest 恰好包含 14 项，最后一项是 `test_asr_assisted_voiceprint_api.sh`。
- coverage pipeline contract 输出 `Coverage pipeline contract: PASS` 并以 0 退出。
- 契约脚本包含显式 expected/actual mismatch 诊断；manifest 数量再次漂移时不会把前一条成功日志误报为失败原因。

## 清理步骤

1. 两个测试脚本/Playwright 配置自动终止它们记录的独立进程。
2. 确认 `lsof -nP -iTCP:18996 -sTCP:LISTEN` 无输出。
3. 确认正式 9900 服务 PID 和健康状态未发生变化。
