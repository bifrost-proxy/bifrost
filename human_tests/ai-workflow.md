# AI Workflow 自定义编排真实场景测试

## 功能模块说明

AI Workflow 是 AI 模块中的自定义编排能力，允许用户把脚本、Agent Runner、ASR 转录任务和 IM 通知任务串成可复用工作流。每个节点可指定输入、输出和有效资源；输入可以来自 Workflow 启动参数、用户自定义文本/文件/脚本，也可以来自上游节点输出。每个 Runner 可以产出文档，并把文档作为后续节点输入。

Workflow 需要能替换现有 ASR 定时任务模式：旧模式中的“扫描目录并转录音频”和“Daily Agent 读取每日文档生成报告”应拆成两个独立节点；转录节点无更新时跳过下游；Daily Agent 节点可配置重试和备用 Runner；后续还能继续接多个 Daily Agent、脚本节点和飞书/微信通知节点。

本文件覆盖设计静态验收、真实 Bifrost 服务、WebUI、API、ASR 测试输入、External CLI Runner、Bifrost CLI 和 React Flow 编辑器的端到端验证。

## 前置条件

1. 仓库位于 `/Users/eden/work/github/bifrost`。
2. 已新增或更新 `design/ai-workflow.md`。
3. 本轮仅设计方案，不启动 Bifrost 服务、不修改系统代理、不执行真实 ASR 模型下载。

## 测试用例列表

### TC-AIW-01：设计覆盖 AI 模块 Workflow 入口与自定义编排

操作步骤：

1. 执行：
   ```bash
   rg -n "AI Workflow|AI 模块|Workflow 列表|Workflow 编辑器|Workflow Run" design/ai-workflow.md
   ```
2. 检查设计是否说明 Workflow 属于 AI 模块，并包含列表、编辑器、Run 详情。

预期结果：

- 命令返回匹配内容。
- 设计明确 Workflow 在 AI 模块中提供自定义编排能力。
- 设计明确用户可以保存模板、手动运行、查看执行状态和 artifacts。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 AI Workflow 功能说明、AI 模块入口、Workflow 列表、Workflow 编辑器和 Workflow Run 详情。

### TC-AIW-02：设计覆盖脚本、Runner、ASR 转录和通知四类节点

操作步骤：

1. 执行：
   ```bash
   rg -n "script|runner|asr_transcription|notification|Script 节点|Runner 节点|ASR 转录节点|Notification 节点" design/ai-workflow.md
   ```
2. 检查每类节点是否有用途、执行域、输入输出和错误处理说明。

预期结果：

- 命令返回四类节点相关内容。
- `script` 节点支持脚本内容或脚本文件。
- `runner` 节点支持内置 Bifrost Agent、Codex Runner、ChatGPT Web Runner、自定义 Runner。
- `asr_transcription` 节点支持扫描音频文件/目录并生成 Daily Markdown。
- `notification` 节点支持把报告发到飞书、微信等 IM 通道。

实际结果：

- 通过。2026-05-29 更新后执行静态检查，匹配到四类节点表格、Script 节点、Runner 节点、ASR 转录节点和 Notification 节点配置示例。

### TC-AIW-03：设计覆盖显式输入、上游输出、文本/文件资源与 Runner 有效输入选择

操作步骤：

1. 执行：
   ```bash
   rg -n "InputBinding|workflow_input|node_output|literal_text|literal_script|file_ref|artifact_query|effective input|有效输入|资源选择" design/ai-workflow.md
   ```
2. 检查设计是否明确：节点输入既可来自用户自定义输入，也可来自上游输出。
3. 检查设计是否明确：Runner 只接收声明的资源，不能默认读取所有上游输出。

预期结果：

- 命令返回输入绑定和资源选择内容。
- 设计包含文本、文件、文件集、JSON、文档等输入形态。
- 设计包含 Runner effective input preview 和默认最小授权策略。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 InputBinding、workflow_input、node_output、literal_text、literal_script、file_ref、artifact_query、effective input preview 和资源选择策略。

### TC-AIW-04：设计覆盖 ASR 定时任务替换为转录节点 + Daily Agent 节点

操作步骤：

1. 执行：
   ```bash
   rg -n "替换现有 ASR 定时任务|transcribe_daily_audio|run_daily_agent|Daily Agent|旧 ASR task|旧 Daily Agent" design/ai-workflow.md
   ```
2. 检查设计是否把旧 ASR 定时任务拆成转录节点和 Daily Agent Runner 节点。

预期结果：

- 命令返回 ASR 定时任务替换、`transcribe_daily_audio`、`run_daily_agent` 和迁移规则。
- 设计明确旧 `audio_dir`、schedule、ASR 转录配置、Daily Agent Runner/Prompt/report 目录如何映射到 Workflow。
- 设计明确旧 processed state 迁移为 artifact manifest / node output hash，避免无变化时重复跑 Daily Agent。

实际结果：

- 通过。2026-05-29 更新后执行静态检查，匹配到替换现有 ASR 定时任务、`transcribe_daily_audio`、`run_daily_agent`、Daily Agent 和旧配置迁移规则。

### TC-AIW-05：设计覆盖进程隔离、权限边界和可观测性

操作步骤：

1. 执行：
   ```bash
   rg -n "主进程禁止直接执行|独立 Agent/Runner worker|独立脚本进程|File Access|default.*deny|events.jsonl|input_manifest.json|output_manifest.json|attempts/.*/stdout.log|logs/index.json" design/ai-workflow.md
   ```
2. 检查设计是否明确主进程只负责编排，不执行 CPU 密集节点。
3. 检查设计是否包含事件日志、输入快照、输出 manifest 和 artifact 审计。
4. 检查设计是否包含每次 Workflow 执行后的 run 日志、节点 attempt 日志、重试/fallback/skip 原因。

预期结果：

- 命令返回进程隔离、权限和可观测性内容。
- Runner 仍通过独立 worker 进程执行。
- 脚本和 ASR 节点也不在主进程内执行。
- 文件访问受 allowlist/File Access policy 限制。
- 设计包含每次 Workflow 执行后的 run 日志、节点 attempt 日志、重试/fallback/skip 原因。

实际结果：

- 通过。2026-05-29 更新后执行静态检查，匹配到主进程禁止直接执行节点业务逻辑、独立 Agent/Runner worker、独立脚本进程、File Access、default deny、events.jsonl、input_manifest.json、output_manifest.json、attempt 日志和 logs/index.json。

### TC-AIW-06：设计覆盖 no_update 无更新跳过下游

操作步骤：

1. 执行：
   ```bash
   rg -n "no_update|succeeded_no_update|skip_downstream|无新增音频|Daily Markdown hash|不会继续触发 Daily Agent" design/ai-workflow.md
   ```
2. 检查设计是否说明定时 Workflow 无新增音频时仍记录日志，但不继续执行 Daily Agent、脚本和通知节点。

预期结果：

- 命令返回 `no_update`、`succeeded_no_update` 和下游跳过规则。
- 设计明确无新增音频或 Daily Markdown hash 不变时输出 `no_update`。
- 设计明确下游 Daily Agent、脚本、通知节点被标记为 `skipped`。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 no_update 节点结果语义、ASR no_update policy、替换 ASR 定时任务中的无更新行为和 run 终态。

### TC-AIW-07：设计覆盖 Daily Agent 重试和备用 Runner fallback

操作步骤：

1. 执行：
   ```bash
   rg -n "fallback_runners|fallback Runner|retry|max_attempts|attempt|同一输入快照|备用 Runner" design/ai-workflow.md
   ```
2. 检查设计是否说明 Daily Agent 节点失败后可重试，并可切换备用 Runner。

预期结果：

- 命令返回重试和 fallback Runner 相关内容。
- 设计明确 fallback 使用同一输入快照和 Prompt。
- 设计明确每次 attempt 都保留日志，不覆盖失败证据。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 `fallback_runners`、`max_attempts`、attempt metadata、fallback Runner 切换记录和同一输入快照要求。

### TC-AIW-08：设计覆盖多个 Daily Agent 分流到不同报告目录

操作步骤：

1. 执行：
   ```bash
   rg -n "多个 Daily Agent|fan-out|reports/action-items|reports/customer-issues|reports/engineering-risks|不同输出目录" design/ai-workflow.md
   ```
2. 检查设计是否支持一个转录节点后接多个 Daily Agent，分别使用不同 Prompt 和输出目录。

预期结果：

- 命令返回 fan-out 与多个报告目录示例。
- 设计明确同一 Daily Markdown 可以分流给多个 Runner。
- 设计明确每个 Runner 只读取声明资源并产出独立文档。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到多个 Daily Agent fan-out 和三个报告目录示例。

### TC-AIW-09：设计覆盖脚本与 IM 通知后续节点

操作步骤：

1. 执行：
   ```bash
   rg -n "normalize_reports|notification|send_to_feishu|Feishu|Weixin|delivery_receipt|Notification 节点" design/ai-workflow.md
   ```
2. 检查设计是否支持 Daily Agent 后继续接脚本节点和 IM 通知节点。

预期结果：

- 命令返回脚本汇总、通知节点、飞书/微信和发送回执内容。
- 设计明确通知节点只在上游报告 `produced` 时执行。
- 设计明确通知节点记录 provider、target、message id、失败原因和重试次数。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 `normalize_reports`、`send_to_feishu`、Notification 节点、Feishu/Weixin、delivery receipt 和发送日志要求。

### TC-AIW-10：设计覆盖 Workflow 日志回溯

操作步骤：

1. 执行：
   ```bash
   rg -n "每次 Workflow 执行|日志文件布局|run.log|attempt.json|stdout.log|stderr.log|为什么跑|扫描了什么|Runner 尝试了几次" design/ai-workflow.md
   ```
2. 检查设计是否能在每次 run 后回溯执行情况。

预期结果：

- 命令返回日志文件布局和回溯问题清单。
- 设计明确每次 Workflow 执行都落日志文件。
- 设计明确失败、无更新、跳过、重试和 fallback 都可回溯。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到日志文件布局、run.log、attempt.json、stdout/stderr、为什么跑、扫描了什么、Runner 尝试次数等回溯要求。

### TC-AIW-11：设计覆盖 Workflow 协议与 CLI 创建入口

操作步骤：

1. 执行：
   ```bash
   rg -n "apiVersion|bifrost.ai.workflow/v1alpha1|kind: Workflow|bifrost ai workflow validate|bifrost ai workflow create|bifrost ai workflow preview|bifrost ai workflow export" design/ai-workflow.md
   ```
2. 检查设计是否定义 JSON/YAML 协议，并提供 Bifrost CLI validate/preview/create/update/export/run/logs 命令。

预期结果：

- 命令返回协议版本、Workflow kind 和 CLI 命令。
- 设计明确 `spec` 是执行语义唯一事实源。
- 设计明确 CLI 可被 Agent Runner 调用来创建、校验、预览和更新 Workflow。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 `apiVersion`、`bifrost.ai.workflow/v1alpha1`、`kind: Workflow` 和 `bifrost ai workflow` CLI 命令族。

### TC-AIW-12：设计覆盖 Agent Runner 自然语言创建 Workflow

操作步骤：

1. 执行：
   ```bash
   rg -n "workflow_schema_get|workflow_draft_create|workflow_validate|workflow_preview|workflow_apply|workflow_export|自然语言|Agent Runner" design/ai-workflow.md
   ```
2. 检查设计是否允许 Runner Chat 模式下由 Agent 根据自然语言生成 Workflow draft。
3. 检查设计是否要求 Agent 先 validate/preview，再在用户确认后 apply。

预期结果：

- 命令返回 Agent 工具注入和自然语言创建流程。
- 设计包含 typed tools 列表。
- `workflow_apply` 默认 dry-run，未确认不得直接创建。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 Agent Runner 工具、自然语言创建流程、validate/preview/apply 和 dry-run 要求。

### TC-AIW-13：设计覆盖 React Flow 渲染与二次编辑

操作步骤：

1. 执行：
   ```bash
   rg -n "React Flow|reactFlow|ui.reactFlow|自动布局|二次编辑|只读模式|semanticNode|semanticEdge" design/ai-workflow.md
   ```
2. 检查设计是否使用 React Flow 渲染 Workflow DAG。
3. 检查设计是否支持拖拽新增节点、连线、表单编辑、协议视图和保存回写。

预期结果：

- 命令返回 React Flow、`ui.reactFlow`、自动布局和只读模式。
- 设计明确 React Flow 只保存布局，执行语义仍以 `spec` 为唯一事实源。
- 从 Agent/CLI 创建的 Workflow 即使没有 UI layout，也能自动布局渲染。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 React Flow 渲染、编辑规则、协议映射、自动布局和 run 详情只读模式。

### TC-AIW-14：设计覆盖协议往返编辑不漂移

操作步骤：

1. 执行：
   ```bash
   rg -n "spec.*唯一事实源|不得存储独立执行语义|往返编辑|workflow_protocol_roundtrip_preserves_spec|workflow_render_generates_react_flow_layout_when_missing_ui_state" design/ai-workflow.md
   ```
2. 检查设计是否避免 React Flow UI state 与执行 spec 漂移。

预期结果：

- 命令返回 spec 唯一事实源、UI 不存执行语义和 roundtrip 测试计划。
- 设计明确 WebUI 保存前必须调用后端 `workflow_validate`。
- 协议视图直接编辑后必须重新校验并渲染。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 spec 唯一事实源、UI state 边界、roundtrip 单元测试和保存前校验要求。

### TC-AIW-15：设计覆盖 Agent 生成协议的安全校验与自动修复边界

操作步骤：

1. 执行：
   ```bash
   rg -n "协议校验与自动修复|自动修复|必须要求用户确认|workflow_agent_tool_validate_blocks_implicit_all_upstream_inputs|Runner 输入必须显式声明" design/ai-workflow.md
   ```
2. 检查设计是否规定 Agent 生成协议必须经过 schema 校验和资源边界校验。

预期结果：

- 命令返回协议校验、自动修复边界和 Runner 输入显式声明要求。
- 无歧义字段可自动修复。
- 涉及 Runner 选择、文件路径、IM target、权限扩大的修复必须要求用户确认。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到协议校验、自动修复、用户确认边界和 Runner 输入显式声明要求。

### TC-AIW-16：设计包含外部开源方案调研来源

操作步骤：

1. 执行：
   ```bash
   rg -n "React Flow / xyflow|Node-RED|n8n|Langflow|Flowise|Dify|Temporal|Prefect|Airflow|Argo Workflows|调研资料来源" design/ai-workflow.md
   ```
2. 检查设计是否明确本轮调研资料来自官方文档、GitHub 仓库或项目文档。

预期结果：

- 命令返回全部指定项目或方向的调研内容。
- 设计包含资料核验时间和每个项目的可借鉴能力。
- 设计没有把外部项目当作未经验证的印象引用。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到调研资料来源、React Flow / xyflow、Node-RED、n8n、Langflow、Flowise、Dify、Temporal、Prefect、Airflow 和 Argo Workflows。

### TC-AIW-17：设计覆盖可借鉴能力、不采用边界和选型理由

操作步骤：

1. 执行：
   ```bash
   rg -n "可借鉴能力|不采用边界|技术选型依据|不嵌入 Node-RED|不引入 Temporal|不把 React Flow" design/ai-workflow.md
   ```
2. 检查设计是否说明哪些能力采用、哪些 runtime 不采用，以及为什么选择 Bifrost 自有协议和 Runtime。

预期结果：

- 命令返回可借鉴能力、不采用边界和技术选型依据。
- 设计明确 React Flow 只做 UI，不做 runtime。
- 设计明确不直接嵌入 Node-RED/n8n/Langflow/Flowise/Dify 或 Temporal/Prefect/Airflow/Argo runtime。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到可借鉴能力、不采用边界、技术选型依据、React Flow UI 边界和外部 runtime 不采用原因。

### TC-AIW-18：设计覆盖稳定协议、revision、导入导出和 patch

操作步骤：

1. 执行：
   ```bash
   rg -n "稳定协议契约|metadata.revision|base revision|schemaRef|导入导出与 Patch|JSON Patch|merge patch|export --redact" design/ai-workflow.md
   ```
2. 检查设计是否定义稳定协议兼容规则，并避免 CLI/Agent/WebUI 互相覆盖。

预期结果：

- 命令返回稳定协议契约、revision、schemaRef、导入导出和 patch 内容。
- 设计明确 update 必须基于 base revision。
- 设计明确导出时可脱敏 secret、IM target 和本机路径。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到稳定协议契约、`metadata.revision`、base revision、`schemaRef`、导入导出与 Patch、JSON Patch / merge patch 和 `export --redact`。

### TC-AIW-19：设计覆盖 CLI 与 typed tools 的 draft/validate/preview/render/apply 闭环

操作步骤：

1. 执行：
   ```bash
   rg -n "bifrost ai workflow draft|bifrost ai workflow render|workflow_patch_propose|workflow_render|baseRevision|previewHash|riskAccepted|draftHash" design/ai-workflow.md
   ```
2. 检查设计是否支持 Agent Runner 通过 Bifrost CLI 或 typed tools 创建和更新 Workflow。
3. 检查设计是否要求自然语言只能生成 draft，并经过 validate/preview/render/apply。

预期结果：

- 命令返回 CLI draft/render 命令、typed tools、baseRevision、previewHash、riskAccepted 和 draftHash。
- 设计明确 `workflow_apply` 缺少确认时只能 dry-run。
- 设计明确 patch/update 不覆盖用户 WebUI 二次编辑。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 CLI draft/render、`workflow_patch_propose`、`workflow_render`、`baseRevision`、`previewHash`、`riskAccepted` 和 `draftHash`。

### TC-AIW-20：设计覆盖 React Flow 资料依据、渲染、二次编辑和 UI/runtime 分层

操作步骤：

1. 执行：
   ```bash
   rg -n "toObject|ReactFlowJsonObject|custom node|custom edge|React Flow state|render API|ui.reactFlow 可以由 render API 丢弃并重建" design/ai-workflow.md
   ```
2. 检查设计是否支持 Workflow 渲染、二次编辑、协议视图修改后重新校验和 run 详情只读高亮。

预期结果：

- 命令返回 React Flow 保存恢复、JSON object、custom node/edge、render API 和 UI state 可重建内容。
- 设计明确 React Flow state 与 Runtime spec 分层。
- 设计明确从自然语言/CLI 创建的 Workflow 没有 UI layout 时仍可渲染。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 `toObject`、`ReactFlowJsonObject`、custom node/edge、React Flow state、render API 和 `ui.reactFlow` 可重建规则。

### TC-AIW-21：设计覆盖运行态日志、重试、fallback 和 durable execution 借鉴

操作步骤：

1. 执行：
   ```bash
   rg -n "Event History|source of truth|AwaitingRetry|task try|retryStrategy|backoff|attempts/.*/attempt.json|fallback Runner|events replay" design/ai-workflow.md
   ```
2. 检查设计是否把外部 runtime 的重试、日志、事件历史经验转化为 Bifrost Runtime 设计。

预期结果：

- 命令返回 Temporal Event History、Prefect retry state、Airflow task try、Argo retryStrategy/backoff、attempt 日志和 events replay。
- 设计明确重试不会覆盖失败证据。
- 设计明确 fallback 使用同一输入快照。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到 Event History/source of truth、AwaitingRetry、task try、retryStrategy/backoff、attempt metadata、fallback Runner 和 events replay。

### TC-AIW-22：设计覆盖自然语言创建的安全边界和 ASR 替换路径

操作步骤：

1. 执行：
   ```bash
   rg -n "request_only|host side effect|allow_network=false|allow_shell=false|confirmedBy|转换为 Workflow|migrate/asr-task|旧 Run Records|succeeded_no_update" design/ai-workflow.md
   ```
2. 检查设计是否限制 Agent 自然语言 draft 的权限，并保留 ASR 定时任务迁移路径。

预期结果：

- 命令返回 request_only、host side effect、脚本默认网络/shell 禁止、confirmedBy、ASR 转换入口、migrate API、旧 Run Records 和 succeeded_no_update。
- 设计明确自然语言 draft 不能直接获得文件、secret、IM target 授权。
- 设计明确旧 ASR 定时任务可迁移为 Workflow trigger + ASR 转录节点 + Daily Agent Runner 节点。

实际结果：

- 通过。2026-05-29 执行静态检查，匹配到自然语言 draft 权限边界、host side effect preview、脚本默认限制、confirmedBy、ASR 转换入口、migrate API、旧 Run Records 和 `succeeded_no_update`。

## 后续实现阶段端到端用例占位

### TC-AIW-23：真实服务创建并 Dry Run 一个脚本 + Runner Workflow

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 启动 Bifrost，必须带 `--no-system-proxy`。
2. 通过 API 或 WebUI 创建包含一个脚本节点和一个 External CLI 测试 Runner 节点的 Workflow。
3. 执行 Dry Run。
4. 查看 effective input preview。

预期结果：

- Dry Run 不执行节点，只返回 DAG 校验、输入绑定和 Runner 有效输入预览。
- Runner 预览不包含未声明 artifact。

实际结果：

- 未执行。本轮仅设计方案，后续实现阶段补齐真实服务验证。

### TC-AIW-24：真实服务执行 ASR Daily Markdown 多 Runner 分流 Workflow

操作步骤：

1. 使用 ASR fixture 或 ASR 测试输入生成 Daily package。
2. 运行包含 ASR 节点、两个 Runner 分流节点、一个汇总节点的 Workflow。
3. 查看每个节点 artifacts。

预期结果：

- ASR 节点产出 Daily Markdown、segments JSON 和 manifest。
- 两个 Runner 节点分别读取声明资源并产出 Markdown 文档。
- 汇总节点只读取两个 Runner 文档并产出 final report。

实际结果：

- 未执行。本轮仅设计方案，后续实现阶段补齐真实服务验证。

## 清理步骤

本轮仅执行静态 `rg` 验收命令，无临时服务、临时端口或临时数据目录需要清理。

### TC-AIW-25：真实服务校验、预览、保存并生成 Workflow Run 轨迹

操作步骤：

1. 使用临时数据目录启动或由 E2E 启动 Bifrost 服务，必须带 `--no-system-proxy` 或使用 in-process E2E 管理端。
2. 执行：
   ```bash
   CARGO_TARGET_DIR=target/ai-workflow-e2e BIFROST_E2E_RUNNER_JOBS=1 SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test ai_workflow_create_validate_preview_run --timeout 180 --port 18891
   ```
3. 检查 E2E 是否依次调用 `POST /_bifrost/api/ai/workflows/validate`、`POST /preview`、`POST /ai/workflows`、`GET /ai/workflows`、`POST /{workflow_id}/run`。
4. 检查 run 记录是否包含 `success` 状态、`finishedAt`、两个 node state、事件序列、input/output manifest、attempt 日志和 artifacts 目录。

预期结果：

- Workflow draft 校验通过。
- Preview 返回 DAG Markdown、React Flow 数据和 Runner effective inputs。
- Workflow definition 保存到后端数据目录，revision 从 1 开始。
- Run 记录保存到后端并返回 `success`，页面刷新或重新查询时可从后端恢复。
- Run 轨迹包含 `run_created`、`topology_planned`、`node_started`、`node_finished`、`run_finished` 事件。
- artifacts 目录包含 `run.json`、`events.jsonl`、`logs/run.log`、`logs/index.json`、每个节点的 `input_manifest.json`、`output_manifest.json`、`attempts/1/attempt.json`、`stdout.log`、`stderr.log`。

实际结果：

- 通过。2026-06-02 执行上述 E2E 命令，`ai_workflow_create_validate_preview_run` 通过，真实服务 API 创建、校验、预览、保存、列表查询、run 记录、事件序列、节点状态、manifest、attempt 日志和 artifact 文件均符合预期。

### TC-AIW-28：CLI 真实服务验证 Workflow 运行日志、记录、结果与轨迹

操作步骤：

1. 创建临时 Workflow YAML，包含 `transcribe` ASR 节点和 `summarize` Runner 节点，Runner 通过 `node_output` 显式读取 `transcribe.daily_markdown`。
2. 使用临时数据目录启动真实 Bifrost 服务，必须带 `--no-system-proxy`，并在无交互测试环境中使用 `--skip-cert-check --access-mode allow_all`：
   ```bash
   BIFROST_DATA_DIR="$TMPDIR/data" SKIP_FRONTEND_BUILD=1 target/debug/bifrost start -p 18894 --unsafe-ssl --no-system-proxy --skip-cert-check --access-mode allow_all
   ```
3. 执行 CLI 真实链路：
   ```bash
   target/debug/bifrost --port 18894 ai workflow apply "$TMPDIR/workflow.yaml"
   target/debug/bifrost --port 18894 ai workflow run human-aiw-trace --input audio_dir="$TMPDIR/audio" --json
   target/debug/bifrost --port 18894 ai workflow logs human-aiw-trace "$RUN_ID"
   ```
4. 使用 `jq`、`grep` 和文件断言检查运行过程、运行日志、运行记录、运行结果和运行轨迹：
   - JSON run 响应中 `.run.status == "success"`，`.run.finishedAt` 是字符串，`.run.events | length >= 7`，`.run.nodeStates | length == 2`。
   - CLI logs 文本包含 `Events:`、`Nodes: 2` 和每个节点的 `attempt:` 路径。
   - artifacts 目录存在 `run.json`、`events.jsonl`、`logs/run.log`、`logs/index.json`。
   - `nodes/transcribe` 与 `nodes/summarize` 均存在 `input_manifest.json`、`output_manifest.json`、`attempts/1/attempt.json`、`stdout.log`、`stderr.log`。
   - `events.jsonl` 包含 `run_finished`，`logs/run.log` 包含 `finished status=success`。
   - `logs/index.json` 中 `eventCount >= 7` 且 `nodeCount == 2`。
   - `nodes/summarize/input_manifest.json` 中上游 artifact 的 `sha256` 以 `sha256:` 开头。
   - `nodes/summarize/output_manifest.json` 中结果 artifact 的 `sha256` 以 `sha256:` 开头。

预期结果：

- CLI 能保存 Workflow、创建 run、查询 run logs。
- Run 不再停留在只有 `queued` 的记录态，而是产出可检查的完成态轨迹。
- 用户能从 API/CLI 文本、`events.jsonl`、`run.log`、`logs/index.json`、节点 manifest、attempt 日志和 artifact hash 复盘一次执行。
- 临时服务停止后清理临时数据目录，不修改系统代理。

实际结果：

- 通过。2026-06-02 执行真实服务 CLI 链路通过，输出 `human_test_pass run_id=run-ea7dde0b-4c13-48f9-9cd2-c8de09ab21ff`。
- `ai workflow logs` 输出 `Status: success`、`Events: 7`、`Nodes: 2`，并列出 `transcribe` 与 `summarize` 两个节点的 attempt 路径。
- 文件断言确认 `run.json`、`events.jsonl`、`logs/run.log`、`logs/index.json`、两个节点的 input/output manifest、attempt metadata、stdout/stderr、artifact hash 与上游输入引用均存在且符合预期。
- 临时服务使用 `--no-system-proxy`，测试结束后清理临时目录和进程。

### TC-AIW-26：CLI 本地校验 Workflow 协议和 Runner 显式输入

操作步骤：

1. 创建临时 Workflow YAML，包含 `asr_transcription` 节点、`runner` 节点、`node_output` 显式输入和 `resourcePolicy.default=deny`。
2. 执行：
   ```bash
   cargo run -p bifrost-cli -- ai workflow validate /tmp/bifrost-ai-workflow-human.yaml --json
   cargo run -p bifrost-cli -- ai workflow preview /tmp/bifrost-ai-workflow-human.yaml --format json
   cargo run -p bifrost-cli -- ai workflow render /tmp/bifrost-ai-workflow-human.yaml
   ```
3. 检查 JSON 输出中的 `valid=true`、`effectiveInputs`、`reactFlow.nodes` 和 `reactFlow.edges`。

预期结果：

- CLI 不依赖 WebUI 即可校验 Workflow 协议。
- Runner 节点只展示显式声明的上游 `daily_markdown` 输入。
- Render 输出可被 WebUI React Flow 使用。

实际结果：

- 通过。2026-05-30 使用临时 YAML 执行 CLI validate/preview/render，均返回成功 JSON，包含 `valid=true`、1 条 Runner effective input 和 React Flow nodes/edges。

### TC-AIW-26-REG-01：Workflow ID 与输出路径拒绝不安全值

操作步骤：

1. 执行单元回归测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin ai_workflow --lib
   ```
2. 检查 `rejects_workflow_ids_that_are_not_url_and_file_safe` 是否覆盖含 `/` 的 Workflow ID。
3. 检查 `rejects_implicit_runner_inputs_and_unsafe_paths` 是否覆盖 Windows 风格 `..\escape.md` 路径穿越。

预期结果：

- Workflow ID 只能包含 ASCII 字母、数字、`-`、`_`。
- 输出路径模板拒绝绝对路径、`..`、Windows 反斜杠穿越和 Windows drive 前缀。
- 后端保存前复用同一校验，避免不安全 ID 与路径进入持久化数据目录。

实际结果：

- 通过。2026-06-01 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin ai_workflow --lib`，新增回归测试通过。

### TC-AIW-27：WebUI AI 模块展示 Workflow 入口并支持明暗主题

操作步骤：

1. 执行：
   ```bash
   pnpm --dir web run build
   ```
2. 打开 AI 页面或静态检查 `web/src/pages/AI/index.tsx`，确认 Tools 分组包含 `Workflow` 导航项。
3. 检查 `web/src/pages/AI/WorkflowSection.tsx` 使用 Ant Design `theme.useToken()` / 组件 token，不硬编码亮色背景文本。
4. 在亮色和暗色主题下分别打开 `aiSection=tools-workflow`，检查标题、draft 编辑器、Saved Workflows 和 Preview 卡片可读。

预期结果：

- WebUI 构建通过。
- AI 页面 Tools 分组展示 Workflow 入口。
- Workflow 页面在亮色和暗色主题下均可读，交互按钮可识别。
- Validate & Preview、Check Apply、Save、Run 按钮调用后端 API。

实际结果：

- 部分通过。2026-05-30 `pnpm --dir web run build` 通过；静态检查确认 AI 页面包含 Workflow 入口，WorkflowSection 使用 Ant Design token 和组件主题。真实浏览器明暗主题交互未执行，风险记录在最终交付残余风险。

### TC-AIW-29：默认 ASR 转录 Workflow 模板可查看、套用、修改和运行

操作步骤：

1. 执行后端模板单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin default_asr_template_is_valid_and_editable --lib
   ```
2. 使用 CLI 查看模板列表并导出默认 ASR 模板：
   ```bash
   cargo run -p bifrost-cli -- ai workflow templates
   cargo run -p bifrost-cli -- ai workflow template default-asr-transcription --output /tmp/bifrost-default-asr-workflow.yaml
   cargo run -p bifrost-cli -- ai workflow validate /tmp/bifrost-default-asr-workflow.yaml --json
   cargo run -p bifrost-cli -- ai workflow preview /tmp/bifrost-default-asr-workflow.yaml --format json
   ```
3. 检查导出的 YAML 是否包含：`transcribe_daily_audio`、`run_daily_agent`、`audio_dir`、`focus_topics`、`noUpdatePolicy.skipDownstream=true`、disabled schedule trigger。
4. 使用真实服务 E2E 验证模板 API、保存、run、日志和轨迹：
   ```bash
   CARGO_TARGET_DIR=target/ai-workflow-e2e BIFROST_E2E_RUNNER_JOBS=1 SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test ai_workflow_create_validate_preview_run --timeout 180 --port 18891
   ```
5. 执行 WebUI 构建并静态检查 Workflow 页面调用模板 API 且提供 `Use Template` 操作：
   ```bash
   pnpm --dir web run build
   rg -n "listAiWorkflowTemplates|Use Template|default-asr-transcription|Workflow Templates" web/src/pages/AI/WorkflowSection.tsx web/src/api/aiWorkflow.ts
   ```

预期结果：

- 默认 ASR 模板由后端统一提供，API/CLI/WebUI 使用同一份结构。
- 模板可作为 YAML draft 导出，用户能基于该文件修改配置后 validate / preview / apply。
- 模板默认表达 ASR 目录扫描转录、无更新跳过下游、Daily Agent 报告和可配置关注主题。
- E2E 真实服务从 `/templates` 获取默认模板，基于模板修改 Workflow ID 后保存、运行，并生成 run events、node states、manifest、attempt logs 和 artifacts。
- WebUI 默认展示模板入口，用户可以点击 `Use Template` 将默认 ASR 模板载入草稿编辑器继续修改。

实际结果：

- 通过。2026-06-02 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin default_asr_template_is_valid_and_editable --lib`，默认 ASR 模板解析、校验、节点结构、显式 Runner 输入、`runner: codex` 和 `noUpdatePolicy` 断言通过。
- 通过。2026-06-02 执行 CLI 模板链路，`ai workflow templates` 输出 `default-asr-transcription`，`ai workflow template default-asr-transcription --output /tmp/bifrost-default-asr-workflow.yaml` 成功导出 YAML，文件包含 `transcribe_daily_audio`、`run_daily_agent`、`runner: codex`、`audio_dir`、`focus_topics`、`skipDownstream: true`。
- 通过。2026-06-02 执行 `ai workflow validate /tmp/bifrost-default-asr-workflow.yaml --json` 返回 `"valid": true`；执行 `ai workflow preview ... --format json` 返回 `blockingErrors: []`、两个节点的 runbook、Runner `effectiveInputs`、React Flow nodes/edges 和 DAG Markdown。
- 通过。2026-06-02 执行真实服务 E2E，`ai_workflow_create_validate_preview_run` 从 `/templates` 获取默认模板，基于模板修改 Workflow ID 后完成 validate、preview、save、list、run、logs 查询，并验证 run events、node states、manifest、attempt logs、artifacts 与 `sha256`。
- 通过。2026-06-02 执行 `pnpm --dir web run build` 通过；静态检查确认 WebUI 调用 `listAiWorkflowTemplates`，默认包含 `default-asr-transcription`，并展示 `Workflow Templates` 与 `Use Template` 操作。

### TC-AIW-30：WebUI 使用 React Flow 可视化 Workflow 并支持 Quick Debug

操作步骤：

1. 执行前端构建：
   ```bash
   pnpm --dir web run build
   ```
2. 静态检查 React Flow 依赖、画布、控件和快速调试入口：
   ```bash
   rg -n "@xyflow/react|ReactFlow|MiniMap|Controls|Background|ai-workflow-reactflow-preview|Quick Debug|Quick Debug Trace|debugAudioDir" web/src/pages/AI/WorkflowSection.tsx web/package.json
   ```
3. 检查 Workflow 页面初次加载默认模板后会自动调用 preview，确保用户无需先读 YAML 也能看到 DAG 图。
4. 检查 Quick Debug 是否按 `validate -> preview -> check apply -> save -> execute -> logs` 记录每一步状态，并允许用户修改调试输入 `audio_dir`。
5. 检查画布节点是否展示节点 id、节点类型和 outputs 数量，且按 `asr_transcription`、`runner`、`script`、`notification` 区分颜色。
6. 检查 Quick Debug 失败路径是否会写入 Trace 面板，而不是只弹出 toast。

预期结果：

- WebUI 构建通过。
- Workflow 页面包含 React Flow 画布、MiniMap、Controls 和 Background。
- 默认 ASR 模板可自动渲染为可缩放/平移 DAG，而不是只展示 YAML。
- Quick Debug 面板能让用户快速验证、预览、执行保存前检查、保存、运行并查看日志摘要。
- 调试输入 `audio_dir` 可配置，Run 与 Quick Debug 使用同一输入值。
- Quick Debug 校验、预览或运行失败时，Trace 面板保留失败步骤与错误摘要，方便继续调试。

实际结果：

- 通过。2026-06-02 执行 `pnpm --dir web run build`，TypeScript 与 Vite 构建通过。
- 通过。2026-06-02 执行静态检查，确认 `@xyflow/react` 依赖已加入，Workflow 页面引入 `ReactFlow`、`MiniMap`、`Controls`、`Background`，并提供 `ai-workflow-reactflow-preview` 画布。
- 通过。2026-06-02 静态检查确认页面初次加载默认模板、切换模板后都会调用 `previewAiWorkflow` 自动生成可视化 DAG 数据。
- 通过。2026-06-02 静态检查确认页面提供 `Quick Debug` 按钮与 `Quick Debug Trace` 面板，调试流程覆盖 validate、preview、check apply、save、execute、logs 摘要，失败时会追加 `Quick Debug Failed` trace。
- 通过。2026-06-02 静态检查确认 `debugAudioDir` 输入会传给 Run 与 Quick Debug，节点展示包含节点 id、节点类型和 outputs 数量，并使用 AntD 主题 token 按节点类型设置颜色以兼容亮/暗色主题。
- 通过。2026-06-02 执行真实浏览器场景 `node .agents/skills/e2e-verify/scripts/browser-test.js scenario ai-workflow-reactflow-debug --headless --verbose`，独立 Bifrost 进程启动后打开 `/_bifrost/ai?aiSection=tools-workflow`，确认 Workflow 页面、React Flow 画布、模板面板、Quick Debug Trace 面板存在；点击 `Validate & Preview` 后画布渲染 `transcribe_daily_audio` 与 `run_daily_agent`；确认 `audio_dir=./human-tests/audio` 调试输入可见；点击 `Quick Debug` 后完成 validate、preview、check apply、save、execute、logs，全场景 13/13 步通过且 54 个 API 请求 0 失败。

### TC-AIW-31：Workflow 调试执行完整真实流程而不是只做 dry-run

操作步骤：

1. 执行后端 AI Workflow 单元测试，覆盖默认 ASR 模板、无更新跳过下游和 run 轨迹文件：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin ai_workflow --lib -- --nocapture
   ```
2. 执行真实服务 E2E，覆盖默认 ASR 模板 API、真实 ASR 节点 no_update 跳过下游、真实 script 节点执行、真实 notification 节点写入本地通知和 receipt、run logs/artifacts/schedules 查询：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test ai_workflow_create_validate_preview_run --test-timeout 180 --verbose
   ```
3. 执行前端构建，确认 Quick Debug 文案不再宣传 dry-run，而是保存并执行完整 Workflow：
   ```bash
   cd web && npm run build -- --mode development
   rg -n "Check Apply|Workflow executed|quick debug executed the full workflow|dry-run" web/src/pages/AI/WorkflowSection.tsx
   ```
4. 检查 CLI 文案不再把 `run` 描述成 dry-run 记录：
   ```bash
   rg -n "Create a dry-run Workflow run record|Workflow executed|execute -> logs|Execute a saved Workflow" crates/bifrost-cli/src/commands/ai_workflow.rs crates/bifrost-cli/src/cli.rs
   ```

预期结果：

- 后端单元测试通过，并验证默认 ASR 模板 Runner 显式配置为 `codex`、未显式声明 Runner 的节点会读取 External CLI 默认 Runner 而不是隐式 mock、ASR 空目录真实调用现有 ASR Directory Task 后返回 `no_update`，下游 Runner 标记 `skipped`。
- E2E 通过真实 Admin API 创建并执行 Workflow；ASR 模板 run 生成 `run.json`、`events.jsonl`、`logs/run.log`、`logs/index.json`、节点 input/output manifest、attempt stdout/stderr；script Workflow 真实执行 shell 命令并在 artifact 中包含运行输入；notification Workflow 写入 `ai_workflow` 本地通知并生成 `notification_receipt.json`。
- Workflow schedule 状态 API 能列出 enabled schedule 的 workflowId/nextRunAt 信息，证明调度入口可观察。
- WebUI Quick Debug 仍保留不保存的 Check Apply 安全检查，但最终会 Save 并 Run，Trace 显示 `Run + Logs`。
- CLI `run` 文案表达“执行 Workflow”，而不是“创建 dry-run 记录”。

实际结果：

- 通过。2026-06-02 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin ai_workflow --lib -- --nocapture`，7 个 AI Workflow 单元测试全部通过；覆盖默认 ASR 模板、协议校验、安全路径、DAG、默认 Runner 走 External CLI 配置而非隐式 mock、真实 ASR no_update -> downstream skipped、真实 script 重试/fallback runner、真实 notification 本地通知 receipt。
- 通过。2026-06-02 执行 `SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test ai_workflow_create_validate_preview_run --test-timeout 180 --verbose`，真实 E2E Runner 1/1 通过；默认 ASR 模板链路完成 templates/detail/validate/preview/save/list/run/logs，真实 run 断言 events、node states、manifest、attempt stdout/stderr、artifacts sha256；script Workflow 真实执行 shell 命令并验证 artifact 包含 `real-execution` 输入；notification Workflow 真实写入本地通知并通过 `/notifications?type=ai_workflow` 查询；enabled schedule trigger 被后台 scheduler 真实触发，`/schedules` 返回 `lastRunId/lastStatus`，并通过 run logs 查询到 schedule run 的 events、node states、artifact 与 `scheduled` 输入。
- 通过。2026-06-02 执行 `cd web && npm run build -- --mode development`，TypeScript 与 Vite 构建通过；静态检查确认 WebUI 使用 `Check Apply`、`Workflow executed`、`quick debug executed the full workflow`，Quick Debug 空态文案为 `validate → preview → check → save → execute → logs`。
- 通过。2026-06-02 静态检查确认 CLI `run` about 为 `Execute a saved Workflow and persist run traces`，CLI schema 流程为 `draft -> validate -> preview -> apply -> execute -> logs`，运行完成输出 `Workflow executed`；旧 `Create a dry-run Workflow run record` 不再存在。
