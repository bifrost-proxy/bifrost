# ASR 历史录音辅助声纹初始化与多模板识别

## 背景与目标

当前声纹录入要求用户实时朗读三段固定文本，并把三个 embedding 简单平均成一个
centroid。真实多人会议来自不同录音设备、距离和混响环境，近讲朗读与会议远场之间
存在明显域差异；与此同时，当前 speaker cluster 会把重叠片段一起拼接进 embedding，
`max_speakers` 也被错误当成 Sherpa 的固定 `num_clusters`。

本方案把默认初始化路径改为“从已完成的真实录音中选择本人片段”，并保证：

- 一个身份只有一个 Speaker Profile，但可包含多个已确认模板和多个 prototype。
- 用户逐片段确认 `mine / not_mine / unsure`，不能因确认一个 cluster 就静默收录整组。
- 只有无重叠、时长合格且用户明确确认的片段能进入永久声纹库。
- 既有实时朗读 Profile 无迁移写回也能读取和匹配。
- 新录音可追加到既有 Profile；每个样本可独立删除并原子重建 centroid/prototype。
- 自动实名保持保守，低置信度继续只展示 candidate，不冒认真实身份。

## 用户目标验证清单

### 必须实现

- 从已完成 ASR task 中选择一个仍存在源音频的文件创建 assisted enrollment session。
- 根据 speaker-aware timeline 自动生成按 speaker 分组的候选片段。
- WebUI 可以试听候选，并标记“是我 / 不是我 / 跳过”。
- 新建 Profile 或向既有 Profile 追加多个已确认模板。
- Profile 保存样本来源、时间范围、质量、独立 embedding 和 prototype。
- 样本删除后重建 Profile，并拒绝删除最后一个可用模板。
- `known_speaker_count` 与 `max_speakers` 语义分离；未知人数不能强制四类。
- 匹配使用多个 prototype，并对多 Profile 的近分冲突保持匿名。

### 必须不破坏

- 现有实时朗读 enrollment、实时 identify、Voice Wake 和旧 JSON Profile 继续工作。
- MOSS 的匿名 speaker 只作为候选分段来源，不宣称其标签是真实身份。
- 不复制完整源录音；session 只生成临时裁剪 PCM，完成或取消后清理。
- 不上传、不同步、不把 embedding 写入 timeline 或 Daily Docs。
- 正式 `~/.bifrost` 和正在运行的 9900 服务不用于开发测试。

### 必须真实验证

- 使用隔离数据目录、真实 WAV、真实 Admin API 完成创建 session、标注、finish、读取
  Profile、追加样本、删除样本和删除 Profile。
- 使用最新构建 WebUI 在亮色和暗色主题逐条完成同一操作。
- 单元测试覆盖候选切分、质量门禁、旧 Profile 兼容、prototype 重建和冲突决策。
- 远端 CI 通过 changed-lines 与 workspace coverage 门禁。

## 数据模型

Profile JSON 升级为 schema v2，同时保留旧 `embedding` 字段作为兼容 centroid：

```json
{
  "schema_version": 2,
  "id": "spk-eden",
  "display_name": "Eden",
  "embedding": [0.1, 0.2],
  "templates": [
    {
      "id": "sample-1",
      "source_kind": "task_segment",
      "task_id": "task-a",
      "file_key": "file-a",
      "speaker": "speaker_00",
      "start_ms": 12000,
      "end_ms": 20000,
      "duration_ms": 8000,
      "quality": 0.92,
      "overlap": false,
      "embedding": [0.1, 0.2],
      "created_at_ms": 1780000000000
    }
  ],
  "prototypes": [
    {
      "id": "prototype-1",
      "template_ids": ["sample-1"],
      "embedding": [0.1, 0.2]
    }
  ]
}
```

兼容规则：

- v1 Profile 缺少 `schema_version/templates/prototypes` 时，旧 `embedding` 作为唯一匹配
  prototype，不自动写回磁盘。
- 实时朗读 finish 也写入每个 prompt 的独立 template，并生成 prototype。
- `embedding_model` 或维度不同的模板不能追加到同一 Profile。
- centroid 和 prototype 都由 template 重算，客户端不能提交任意 embedding。

## Assisted enrollment session

### API

```text
POST   /api/asr/speaker-profiles/assisted-sessions
GET    /api/asr/speaker-profiles/assisted-sessions/{session_id}
POST   /api/asr/speaker-profiles/assisted-sessions/{session_id}/labels
POST   /api/asr/speaker-profiles/assisted-sessions/{session_id}/finish
DELETE /api/asr/speaker-profiles/assisted-sessions/{session_id}
DELETE /api/asr/speaker-profiles/{profile_id}/samples/{sample_id}
```

创建请求包含 `name`、可选 `profile_id`、`task_id` 和 `file_key`。服务端只从任务
store 解析源文件，拒绝客户端传入任意文件系统路径。

### 候选生成

- 使用保存的 speaker-aware timeline；Sherpa 和 MOSS timeline 均可作为来源。
- 排除 `overlap=true`、无 speaker、短于 3 秒的区间。
- 超过 12 秒的区间按自然上限切分；每个 speaker 最多返回 8 个代表候选。
- 候选质量由有效时长和重叠状态决定；模型级质量评分是后续兼容扩展字段。
- 候选只引用源音频；试听复用现有带 HTTP Range 的 task file source endpoint。

### 标注与 finish 门禁

- 标签只接受 `mine/not_mine/unsure`，未知 candidate id 整批拒绝。
- finish 至少需要 3 个 `mine` 片段和 12 秒有效语音；UI 推荐 30 秒以上。
- finish 前为每个 mine 片段裁剪 16kHz mono PCM，并在 diarization worker 中提取
  embedding，避免 Admin 主进程加载模型。
- 任一片段失败则不写 Profile；所有片段成功后原子写 Profile，再删除 session。
- 追加既有 Profile 时先校验名称、模型和维度，随后原子重建所有 prototype。

## Prototype 与匹配

- template 按 embedding 相似性增量聚类；无法归入既有 prototype 时保留新 prototype，
  不把不同设备/声学条件强制平均掉。
- Profile 匹配分数取兼容 centroid 与各 prototype 的最高余弦分数。
- 多 Profile 场景中，第一名与第二名差值不足 `conflict_margin` 时只能 suggestion。
- 单 Profile 的 self-priority 仍要求最低语音时长，但候选 cluster embedding 必须排除
  overlap 片段。
- 本次不自动把识别结果写回 Profile；永久样本只能来自用户显式确认。

## WebUI

`Voiceprint & Wake` 保持紧凑工作台布局：

- `Add from recording` 是推荐入口，`Record voice` 保留为快速入口。
- 第一步选择新建/追加 Profile、任务和已成功且源文件仍存在的录音。
- 第二步使用单个稳定播放器和按 speaker 分组的候选列表；每行展示时间、文本、时长、
  质量以及三个标签按钮。
- 完成区持续展示已选片段数和总时长；未过门禁时按钮禁用并解释原因。
- Profile 列表展示 template/prototype 数、总时长，并允许查看和删除样本。
- 所有颜色来自 Ant Design token，亮暗主题保持同一信息层级。

## 隐私、并发与清理

- Profile 与 session 目录权限沿用本机 Bifrost data dir 边界，不参与 Rule Sync。
- session id、profile id、task id 和 file key 均经既有校验/任务 store 解析，禁止路径穿越。
- session JSON、Profile JSON 使用 atomic temp + rename；同一 Profile finish/delete 用进程内锁
  串行化，避免 lost update。
- finish、取消和过期清理删除临时 PCM；原始录音不复制、不删除。
- 删除 Profile 时删除 JSON 和该 Profile 的所有派生数据；timeline 只保留历史映射证据。

## TS-VAD 边界

目标说话人 VAD 是后续质量增强层，但需要新增模型、Apple Silicon 性能验证、重叠会议
黄金集和独立发布资产。本 MR 固化 Profile/template/prototype 和候选 span 契约，使未来
TS-VAD 可以直接消费已确认模板；在没有真实 DER/FAR/RTF 证据前不把 TS-VAD 描述为已交付。

## 测试与上线门禁

- 单元测试：speaker count、候选切分、label 校验、finish 门禁、v1 兼容、prototype 聚类、
  多 Profile conflict、sample delete rebuild。
- E2E：隔离服务生成真实 WAV 与 timeline，完整调用 assisted API，并验证临时文件清理。
- Playwright：任务/文件选择、试听跳转、标签状态、完成门禁、Profile 详情和样本删除。
- human_tests：真实录音、亮暗主题、追加既有 Profile、误选撤销、删除和旧实时录入回归。
- proxy coverage shell manifest 保持与当前 15 项契约一致；数量漂移时输出
  expected/actual 诊断并失败，避免只留下前一条成功日志。
- 本地先执行相关测试，再执行 `cargo test --workspace --all-features`；远端 CI 执行
  `bash scripts/ci/coverage-all.sh --json --gate` 和 changed-lines 95% 门禁。

上线质量目标以本地黄金集校准：自动实名 precision 不低于 99%，无目标用户会议的误认
率低于 0.5%，并分别报告不同设备、重叠/非重叠和未见会议日期的结果。
