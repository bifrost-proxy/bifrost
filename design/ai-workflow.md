# AI Workflow 自定义编排设计方案

## 功能模块说明

AI Workflow 是 AI 模块中的可视化/配置化任务编排能力，用于把多个脚本、Agent Runner、ASR 转录任务、通知发送任务串成一个可复用工作流。它的目标不是替代 Agent 自主规划，而是给用户一个简单、确定、可审计的编排层：用户可以声明每个节点的输入、输出、执行器和依赖关系，系统按拓扑顺序执行，并把每个节点产物沉淀为文件或文本，供后续节点选择性消费。

典型场景是音频文件驱动的复杂自动化：每日音频进入音频扫描与转录节点，该节点只负责把指定目录的新音频转成按日合成的 Daily Markdown 文本文件；后续一个或多个 Daily Agent Runner 节点读取这个每日文档，按不同 Prompt 生成行动项、客户问题、研发风险、会议纪要、周报素材或知识库条目等报告文件；脚本节点负责拆分、过滤、归档、同步到外部系统；通知节点把最终报告或摘要发送到飞书、微信等 IM 通道。

Workflow 应成为现有 ASR 定时任务模式的最终替代方案。当前 ASR 定时任务把“扫描目录并转录音频”和“Daily Agent 读取每日文档生成报告”绑定在一个模块里；Workflow 需要把它拆成两个独立节点，再允许用户在第二个节点之后 fan-out 出多个 Daily Agent、脚本和通知节点，从固定流水线升级为可编排的音频自动化平台。

## 用户目标验证清单

### 必须实现

- 在 AI 模块增加 Workflow 功能，支持用户自定义编排。
- Workflow 由多个节点组成，节点可以是脚本、Agent Runner 或 ASR 转录任务。
- 每个节点可指定输入和输出；输入可以来自用户显式配置，也可以来自上游节点输出。
- 节点输入和输出都支持文本内容与文件资源两种形态。
- 每个 Runner 节点可明确选择哪些资源、哪些上游输出、哪些自定义输入作为有效输入。
- 每个 Runner 节点可产出文档文件，并把文档路径/摘要作为后续节点输入。
- ASR 转录节点支持把每日音频转成打包好的 Daily 文件，作为后续复杂任务的统一输入。
- Workflow 能替换现有 ASR 定时任务：第一个节点扫描指定目录并把新音频转录/合成为每日文档，第二个节点把每日文档交给 Daily Agent Runner 生成报告文件。
- 一个转录节点后可连接多个 Daily Agent Runner 节点，不同 Runner/Prompt 输出到不同文件夹。
- Daily Agent 后可继续连接脚本节点和 IM 通知节点，例如把报告发到飞书或微信通道。
- 每个节点支持容错策略：无更新时跳过下游、失败时停止下游、按配置重试、按配置切换备用 Runner。
- 每次 Workflow 执行都必须落日志文件，能回溯每个节点输入、输出、状态、错误、重试和跳过原因。
- Workflow 执行过程可追踪，每个节点有独立状态、日志、输入快照和输出 artifacts。

### 必须不破坏

- 不改变现有 ASR Directory Task、Daily Agent、Agent Runner、ChatGPT/Codex/custom runner 的独立使用方式。
- 不绕过 Agent/Runner 进程隔离要求；每个 Runner 执行仍通过独立 worker 进程。
- 不让主进程执行 CPU 密集或长阻塞节点；主进程只负责编排、状态查询、输入输出索引和事件转发。
- 不把所有上游输出无差别塞给 Runner；默认只传递显式声明的输入，避免 token 膨胀和隐私越界。
- 不让脚本节点获得超出 Workflow 授权目录的文件访问能力。

### 必须真实验证

- 静态验证设计覆盖脚本、Runner、ASR 三类节点。
- 静态验证设计覆盖显式输入、上游文本输出、上游文件输出、资源选择和文档产出。
- 后续实现阶段需用真实 Bifrost 服务验证：ASR 日文件 -> 多 Runner 分流 -> 文档产出 -> 汇总节点。
- 后续实现阶段需验证 Runner 只收到声明输入，不收到未授权上游输出。
- 后续实现阶段需验证 ASR 定时任务兼容迁移：旧配置能映射为“转录节点 + Daily Agent 节点”的 Workflow。
- 后续实现阶段需验证无新增音频时转录节点输出 `no_update`，下游 Daily Agent/通知节点不会误执行。
- 后续实现阶段需验证 Daily Agent Runner 失败后的重试、备用 Runner fallback 和日志回溯。

## 核心概念

### Workflow Definition

Workflow Definition 是用户保存的编排模板，包含基本信息、参数、节点、边、默认资源策略和触发方式。

Workflow Definition 同时也是 Agent Runner 与 Bifrost CLI 共同使用的稳定协议。用户可以在 WebUI 里用 React Flow 画出来，也可以让 Agent Runner 根据自然语言生成同一份 JSON/YAML 协议，再通过 Bifrost CLI 创建、校验、预览和运行。WebUI 二次编辑时仍回写同一协议，避免“自然语言生成版本”和“可视化编辑版本”变成两套模型。

```json
{
  "id": "daily-audio-insight",
  "name": "Daily Audio Insight",
  "description": "把每日音频转录后分流成多个 AI 文档",
  "version": 3,
  "inputs": [
    {
      "name": "audio_dir",
      "type": "file_set",
      "required": true,
      "description": "当天音频目录"
    },
    {
      "name": "focus_topics",
      "type": "text",
      "required": false,
      "default": "行动项、客户问题、研发风险"
    }
  ],
  "outputs": [
    {
      "name": "final_report",
      "type": "document",
      "from": "summary.outputs.report"
    }
  ],
  "nodes": [],
  "edges": []
}
```

## Workflow 协议与自然语言创建

## 开源方案调研

本轮调研以可访问的官方文档、GitHub 仓库和项目文档为依据，不把任何一个外部系统作为直接替代品，而是拆解它们能给 Bifrost AI Workflow 提供的工程参考。资料核验时间为 2026-05-29。

### 调研资料来源

- React Flow / xyflow：官方 Save and Restore 示例（https://reactflow.dev/examples/misc/save-and-restore）说明 `toObject()` 可保存恢复画布；`ReactFlowJsonObject` 文档（https://reactflow.dev/api-reference/types/react-flow-json-object）说明可持久化 nodes、edges、viewport；xyflow GitHub（https://github.com/xyflow/xyflow）说明 React Flow / Svelte Flow 是 open source node-based UI libraries。
- Node-RED：官方 Admin API types 文档（https://nodered.org/docs/api/admin/types）说明 `/flows` 使用的 main flow format 包含节点 `id/type/x/y/wires`，编辑器可导入导出该结构。
- n8n：官方 export/import 文档（https://docs.n8n.io/workflows/export-import/）说明 workflow 以 JSON 导入导出；CLI 文档（https://docs.n8n.io/hosting/cli-commands/）提供 `export:workflow` / `import:workflow`；execution 文档覆盖失败执行排查入口。
- Langflow：官方 import/export 文档（https://docs.langflow.org/next/concepts-flows-import）说明 flow 可导出为 JSON、通过 API 上传/下载，并提示 API key/global variable 的导出边界；API 文档（https://docs.langflow.org/next/api-flows）提供 flow management 端点。
- Flowise：官方文档（https://docs.flowiseai.com/）说明 Chatflow/Agentflow、Tracing & Analytics、Human in the Loop，以及 API/CLI/SDK 能力。
- Dify：官方工作流概念文档（https://docs.dify.ai/en/guides/workflow/node/start）说明 app 可导出为 YAML DSL；Output 文档（https://docs.dify.ai/en/guides/workflow/node/end）说明输出变量和 workflow-as-tool 的返回边界。
- Temporal：官方文档入口（https://docs.temporal.io/）强调可靠应用恢复能力；V1 只借鉴 Event History、deterministic replay、Activity retry 的边界，不引入 Temporal runtime。
- Prefect：官方 states 文档（https://docs.prefect.io/latest/concepts/states）和 task 文档（https://docs.prefect.io/v3/concepts/tasks）说明 flow/task run 状态、AwaitingRetry/Retrying、task-level metrics/logs/state、retries、timeouts 与 cache。
- Airflow：官方 UI 文档（https://airflow.apache.org/docs/apache-airflow/stable/ui.html）说明 Grid View、Graph View、Runs、Task logs 和 retry/monitoring 调试视图；Dag Runs 文档（https://airflow.apache.org/docs/apache-airflow/stable/core-concepts/dag-run.html）说明可查看 task instance history 和每次 try 的 logs。
- Argo Workflows：官方 retry 文档（https://argo-workflows.readthedocs.io/en/latest/retries/）说明 `retryStrategy`、retry policies、conditional retries、backoff；artifact repository 文档（https://argo-workflows.readthedocs.io/en/release-3.7/configure-artifact-repository/）说明 workflow artifacts 需要配置 artifacts 存储。

| 项目 | 可参考能力 | 对 Bifrost 的取舍 |
| --- | --- | --- |
| React Flow / Xyflow（https://github.com/xyflow/xyflow、https://reactflow.dev） | 开源节点画布、custom nodes/edges、拖拽连线、`toObject` 保存恢复、JSON-compatible `ReactFlowJsonObject` | 采用为 WebUI DAG 画布。它只负责 UI 交互、布局、状态着色和保存 `nodes/edges/viewport`，不负责协议校验、DAG 语义、调度、权限、重试或日志。 |
| Node-RED（https://github.com/node-red/node-red、https://nodered.org/docs/api/admin/types） | 低代码事件流、节点 `id/type/x/y/z/wires`、编辑器导入导出、Admin API `/flows` 的完整 flow JSON | 参考节点协议的可读性和导入导出形态，但不采用扁平 `wires` 作为执行协议；Bifrost 需要显式 `spec.nodes/spec.edges/InputBinding`，避免 UI layout 与执行输入混在一起。 |
| n8n（https://github.com/n8n-io/n8n、https://docs.n8n.io/workflows/export-import/） | Workflow JSON 导入导出、CLI import/export、执行列表、失败重试、error workflow、节点级集成生态 | 参考 JSON 可审计、执行历史和失败重试体验；同时把凭据和 HTTP header 等敏感信息从导出协议中隔离为 secret ref，避免分享 Workflow 时泄漏。 |
| Langflow（https://github.com/langflow-ai/langflow、https://docs.langflow.org） | AI/Agent visual builder、flow JSON import/export、API 创建/导入导出、组件端口类型、运行时构建 DAG 并按依赖执行、flow logs | 参考 LLM 组件化、端口类型和“每个 workflow 可作为工具暴露”的思路；Bifrost 需要保留 Runner 进程隔离和 artifacts 文件边界，不把所有组件都内联成 Python 执行。 |
| Flowise（https://github.com/FlowiseAI/Flowise、https://docs.flowiseai.com） | 开源 AI Agent/LLM workflow visual builder、Chatflow/Agentflow、Tracing & Analytics、Human in the Loop、API/CLI/SDK | 参考 Agentflow 对多 Agent 和人工确认的表达；Bifrost 需要更严格的本地文件访问、Runner 输入白名单和 dry-run/apply 分离。 |
| Dify（https://github.com/langgenius/dify、https://docs.dify.ai） | AI workflow DSL、YAML 导入导出、变量引用、输出变量 schema、Workflow 与 Chatflow 区分、工作流工具化 | 参考 YAML DSL、变量 picker、输出 schema 和 workflow-as-tool；Bifrost 协议应明确 `apiVersion/kind/spec`，并让 OutputSpec 成为下游工具/Workflow 可消费的稳定 contract。 |
| Temporal（https://github.com/temporalio/temporal、https://docs.temporal.io） | Durable execution、event history、Activity retry、Workflow deterministic replay、长任务恢复 | 参考“编排层保持确定、外部副作用放到 Activity/Worker”的边界；Bifrost V1 不引入 Temporal 依赖，但 Runtime 应保留 events replay、attempt 日志和可恢复状态机。 |
| Prefect / Airflow / Argo Workflows（https://docs.prefect.io、https://airflow.apache.org、https://argo-workflows.readthedocs.io） | Flow/task state、参数校验、run UI、task logs、Graph/Grid View、retryStrategy、artifact/log 约定 | 参考状态机、重试策略、run 详情和任务日志组织；Bifrost 的节点 worker、attempt metadata、`no_update/skipped/failed` 状态应优先做成一等概念。 |

调研结论：

- 可视化 DAG 编辑和稳定执行协议必须分层。React Flow、Node-RED、Langflow 都证明 nodes/edges 适合保存和渲染图，但 Bifrost 不能把画布 JSON 直接当 runtime contract。
- 导入导出必须是 reviewable 的 JSON/YAML。n8n、Langflow、Dify 都把 workflow 文件作为迁移和分享的基础；Bifrost 应支持 `validate/preview/export`，并在导出时只输出 secret ref 和资源引用，不输出密钥明文。
- 运行历史、节点 attempt、错误分类和 retry/fallback 必须从 V1 协议就设计进去。Airflow、n8n、Prefect、Temporal 的共同经验是：没有可读日志和可恢复状态，复杂 Workflow 很快不可维护。
- Agent/LLM 工作流需要更强安全门禁。Langflow、Flowise、Dify 适合参考自然语言/可视化创建体验，但 Bifrost 必须默认最小授权、显式输入传递、dry-run 和用户确认，不能让 Agent 生成的协议直接获得本机文件或 IM 发送权限。

### 可借鉴能力

- 画布层采用 React Flow 的 node-based UI 能力：custom node、custom edge、handles、MiniMap、Controls、保存恢复和 `ReactFlowJsonObject`。Bifrost 只复用 UI state，不复用外部 runtime。
- 协议层借鉴 Dify YAML DSL、n8n JSON export/import、Langflow flow JSON 和 Node-RED flow JSON 的可导入导出经验，统一成 Bifrost 自有 `apiVersion/kind/metadata/spec/ui` 协议。
- Agent 创建链路借鉴 Langflow/Dify 的 AI workflow builder 体验，但强制引入 `draft -> validate -> preview -> apply`，把自然语言生成限制在 draft 阶段。
- 运行态借鉴 Temporal 的 Event History/source-of-truth 思路、Prefect 的 run state 与 retry state、Airflow 的 task try logs、Argo 的 retryStrategy/backoff/artifact repository。
- 调试体验借鉴 n8n execution history/error workflow、Airflow Graph/Grid View 和 Flowise tracing，要求每个节点都有 attempt、effective input preview、artifact、stdout/stderr、错误分类和跳过原因。
- 安全边界借鉴 Langflow 导出 API key 风险提示和 n8n credential/header 导出风险提示：导出的 Workflow 只能包含 secret ref、provider ref、resource ref，不能包含密钥明文或授权目录外绝对路径。

### 不采用边界

- 不把 React Flow 的 `nodes/edges` 直接作为运行协议。React Flow state 只用于坐标、viewport、折叠、样式和前端交互，Runtime 永远只信任 `spec`。
- 不嵌入 Node-RED/n8n/Langflow/Flowise/Dify 的 runtime。Bifrost 的目标是本地 Agent/Runner/ASR/IM Gateway 编排、文件访问控制和 artifacts 审计，不是通用集成平台。
- 不引入 Temporal/Prefect/Airflow/Argo 作为 V1 运行依赖。它们提供状态、重试、日志和 durable execution 参考，但会显著增加部署、权限、数据目录和调试复杂度。
- 不把 Agent 生成的脚本、IM 发送、外部网络、文件写入直接视为可信操作；这些能力必须经过 preview 风险展示、用户确认和资源策略校验。
- 不在 Workflow 协议里保存 credential 明文、OAuth token、IM webhook secret、Runner browser session、系统代理开关或其它 host-wide side effect。

## 技术选型依据

V1 采用“Bifrost 稳定协议 + React Flow WebUI + Bifrost Runtime”的组合：

1. 协议层由 Bifrost 定义：`apiVersion/kind/metadata/spec/ui/status`，支持 JSON/YAML、schema 校验、版本迁移、Git review 和 Agent typed tools patch。
2. UI 层采用 React Flow：负责图形编辑、自动布局、节点状态渲染、二次编辑和只读 run 图，不参与执行决策。
3. Runtime 层由 Bifrost 实现：负责 DAG 校验、拓扑调度、worker 隔离、input/output manifest、attempt、retry/fallback、权限、日志和 artifacts。
4. Agent Runner 通过 CLI 或 typed tools 写入同一协议：自然语言只生成 draft，不跳过 `validate`、`preview`、`dry-run` 和用户确认。

暂不直接嵌入 Node-RED/n8n/Langflow/Flowise/Dify 作为 runtime，原因是：

- 它们的节点生态和凭据模型与 Bifrost 本地代理、Runner、ASR、IM Gateway、File Access 策略不一致。
- Bifrost 的核心需求是本地 Agent/Runner/ASR worker 编排、受控文件访问和 artifacts 审计，不是通用 SaaS 集成平台。
- 引入外部 runtime 会放大部署、权限、数据目录、日志归属和用户调试成本。

## React Flow 可行性与边界

React Flow 可行性：

- `@xyflow/react` 支持自定义节点、边、handles、拖拽、缩放、mini map、controls 和状态管理，足够承载 Workflow 编辑器。
- 官方 Save and Restore 示例说明可以通过 React Flow instance `toObject()` 或本地 nodes/edges state 保存恢复；`ReactFlowJsonObject` 是 JSON-compatible，可写入 `ui.reactFlow`。
- Bifrost 可把 `spec.nodes/spec.edges` 转成 React Flow nodes/edges；没有 `ui.reactFlow` 时由后端或前端自动布局。
- Run 详情可以复用同一画布的 read-only 模式，把节点 outcome、attempt、fallback、logs/artifacts badge 渲染到节点 UI。

React Flow 不承担：

- 不校验 `apiVersion/kind/spec` schema。
- 不判断 DAG 是否有环、输入绑定是否存在、输出类型是否匹配。
- 不调度节点、不执行脚本、不启动 Runner、不跑 ASR。
- 不实现 retry/fallback/no_update/skip 语义。
- 不做 File Access、secret、IM target、网络权限校验。
- 不保存独立执行语义；`ui.reactFlow` 只能保存 position、viewport、折叠状态和展示样式。

因此 WebUI 保存流程必须是：

1. 用户在 React Flow 画布和右侧表单修改 Workflow。
2. 前端把画布改动转换为 `spec.nodes/spec.edges` 和 `ui.reactFlow`。
3. 调用后端 `workflow_validate`；失败时在节点和协议视图定位错误。
4. 调用 `workflow_preview` 展示 DAG 摘要、effective input、输出路径、权限风险和 dry-run 计划。
5. 用户确认后 `create/update` 保存。

## Agent Runner 自然语言创建 Workflow 的协议闭环

自然语言创建 Workflow 必须是可回放、可校验、可预览、可审计的闭环：

```text
用户自然语言
  -> workflow_schema_get
  -> workflow_draft_create
  -> workflow_validate
  -> workflow_preview
  -> 用户确认
  -> workflow_apply(dry_run=false)
  -> workflow_export / WebUI React Flow 二次编辑
  -> workflow_validate
```

闭环规则：

- `workflow_draft_create` 只返回 draft、诊断和无法确定的字段，不直接写入 definition。
- `workflow_validate` 必须覆盖 schema、DAG、节点类型、输入输出引用、资源策略、路径穿越、secret ref、IM target、Runner 输入显式声明。
- `workflow_preview` 必须返回 Markdown 摘要、React Flow 渲染数据、effective input preview、权限风险、输出路径预览、dry-run runbook。
- `workflow_apply` 默认 `dry_run=true`；只有当用户明确确认、preview 无阻塞风险、资源权限可满足时，才允许 `dry_run=false`。
- Agent 修改已有 Workflow 时必须先 `workflow_export`，基于当前版本做 patch，再 `validate/preview/update`，避免覆盖用户在 WebUI 中的二次编辑。
- Agent 不能把“用户想要的业务目标”直接转成脚本节点执行；涉及脚本、文件写入、IM 发送、外部网络、Runner 选择和 fallback 扩权时必须在 preview 里列出风险并要求确认。

### 协议目标

Workflow 必须先定义一套稳定协议，再在 CLI、WebUI、Agent 工具、导入导出之间复用。协议目标：

- 人可读：支持 JSON 和 YAML，适合在仓库里 code review。
- 机器可校验：每个字段有 schema、版本、默认值和迁移规则。
- Agent 可生成：Agent Runner 可以根据自然语言生成 Workflow draft。
- Web 可渲染：同一协议能渲染成 React Flow 节点图。
- 可往返编辑：React Flow 拖拽/连线/改配置后，仍能序列化回同一协议。
- 可审计：每次保存记录 version、author、source、diff summary。

### 协议版本

```yaml
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: daily-audio-insight
  name: Daily Audio Insight
  description: 每天扫描音频目录，转录并生成多份 Daily Agent 报告
  revision: 3
  createdBy: user:eden
  updatedBy: agent:codex
  source:
    type: natural_language_draft
    promptHash: sha256:...
  labels:
    domain: asr
spec:
  schemaRef: bifrost://schemas/ai-workflow/v1alpha1
  inputs: []
  triggers: []
  nodes: []
  edges: []
  outputs: []
  resourcePolicy:
    default: deny
  permissions:
    fileAccessRefs: []
    secretRefs: []
    imTargetRefs: []
ui:
  reactFlow:
    nodes: []
    edges: []
    viewport:
      x: 0
      y: 0
      zoom: 1
status:
  lastValidatedAt: null
  lastPreviewHash: null
```

核心规则：

- `apiVersion` 必填，V1 使用 `bifrost.ai.workflow/v1alpha1`。
- `kind` 固定为 `Workflow`。
- `metadata` 存储身份、名称、标签、描述、revision、author/source 和变更来源。
- `spec` 是执行语义的唯一事实源。
- `ui.reactFlow` 只保存布局、折叠状态、画布坐标、前端展示偏好；不得存储独立执行语义。
- Runtime 只信任 `spec`，React Flow 渲染层必须从 `spec` 派生。
- `status` 只保存校验/预览/最近运行摘要，可由 Runtime 重建；导入导出时默认可省略。
- `spec.schemaRef` 指向内置 JSON Schema / Rust schema 版本；`validate` 输出必须带 schema version，方便 Agent 按错误修复。
- `metadata.revision` 使用乐观锁；CLI、typed tools 和 WebUI update 都必须携带 base revision，避免覆盖用户二次编辑。
- `spec.permissions` 只保存授权引用，不保存授权内容；secret、IM provider、File Access grant 在运行时解析。

### 稳定协议契约

Workflow 协议是 CLI、Agent typed tools、WebUI、API、导入导出和运行态共同使用的稳定 contract。协议兼容策略：

- `v1alpha1` 允许新增可选字段，但不得改变现有字段语义。
- 所有 enum 增加新值时必须让旧 Runtime 返回结构化 warning，而不是静默忽略。
- `spec.nodes[*].type` 必须对应已注册 NodeKind；未知节点类型只能在 `validate` 阶段返回 `unsupported_node_type`。
- 所有输入输出引用使用 `nodeId/output` 或 `artifactId`，不允许通过 UI edge id 推断执行语义。
- 所有路径字段使用 path template 或 resource ref，validate 必须拒绝 `..`、绝对路径越权、符号链接逃逸和未授权目录。
- `spec.resourcePolicy.default` 默认 `deny`；Runner、script、notification、asr 节点必须各自声明最小权限。
- `ui.reactFlow` 可以由 render API 丢弃并重建；如果重建后 `spec` 不变，协议 roundtrip 视为成功。
- `status`、run logs、attempt logs 不参与 definition revision；它们属于 run instance，不反向污染 definition。

### 协议导入导出与 Patch

- `export` 默认输出 YAML，适合 code review；`--format json` 用于工具链和测试。
- `export --include-ui=false` 可只导出 `apiVersion/kind/metadata/spec`，便于 Agent 修改执行语义。
- `export --redact` 必须移除或脱敏 secret ref display name、IM target display name 和本机敏感路径，只保留 stable ref。
- `update` 支持 full document，也支持 JSON Patch / merge patch；typed tools 推荐返回 patch，减少覆盖 WebUI 手工布局。
- `validate` 必须同时输出 `errors[]`、`warnings[]`、`autoFixes[]`、`requiresConfirmation[]`。
- `preview` 必须基于 validate 后的同一 draft hash；`apply` 必须校验 preview hash 未过期。

### Bifrost CLI 协议命令

为 Agent Runner 和用户提供 CLI 入口：

```bash
bifrost ai workflow validate <workflow.yaml>
bifrost ai workflow preview <workflow.yaml> --format markdown
bifrost ai workflow draft --from-prompt prompt.txt --output draft.yaml
bifrost ai workflow create <workflow.yaml>
bifrost ai workflow update <workflow_id> --file <workflow.yaml>
bifrost ai workflow patch <workflow_id> --patch patch.json
bifrost ai workflow export <workflow_id> --format yaml
bifrost ai workflow run <workflow_id> --input audio_dir=/Audio/2026-05-29
bifrost ai workflow logs <run_id>
bifrost ai workflow render <workflow.yaml> --format react-flow-json
```

Agent Runner 可以通过这些命令把自然语言生成的 Workflow 变成真实配置：

1. 根据用户自然语言生成 `Workflow` YAML draft。
2. 执行 `bifrost ai workflow validate draft.yaml` 获取 schema 错误。
3. 执行 `bifrost ai workflow preview draft.yaml --format markdown` 获取节点图摘要、effective inputs、资源边界和风险提示。
4. 执行 `bifrost ai workflow render draft.yaml --format react-flow-json` 生成可视化预览，必要时让用户确认节点图。
5. 根据校验/预览反馈自动修正 draft。
6. 经用户确认后执行 `bifrost ai workflow create draft.yaml`。
7. 如果用户继续自然语言修改，Agent 先 `export` 当前协议，编辑后 `validate` + `preview` + `patch/update`。

### Agent Runner 工具注入

除了直接调用 CLI，也可以给内置 Agent/Runner 注入 typed tools，降低自然语言搭建 Workflow 的成本：

| 工具 | 作用 |
| --- | --- |
| `workflow_schema_get` | 返回当前 Workflow 协议 schema、节点类型、字段说明和示例 |
| `workflow_draft_create` | 根据自然语言生成 draft，并返回 YAML/JSON 与诊断 |
| `workflow_validate` | 校验 draft，返回结构化错误、warning、自动修复建议 |
| `workflow_preview` | 返回 Markdown 摘要、React Flow 渲染数据、effective input preview |
| `workflow_apply` | 创建或更新 Workflow，必须支持 dry-run 和用户确认 |
| `workflow_export` | 导出现有 Workflow，供 Agent 二次修改 |
| `workflow_patch_propose` | 基于用户自然语言和当前 revision 生成 JSON Patch / merge patch，不直接覆盖整份 definition |
| `workflow_render` | 把 `spec` 转成 React Flow nodes/edges/viewport，供 Chat/WebUI 展示与用户确认 |
| `workflow_run` | 启动 Workflow run，返回 run id 和日志入口 |
| `workflow_logs` | 查询 run 日志、节点 attempt、失败/skip/no_update 原因 |

工具设计原则：

- Agent 只能通过协议和 API/CLI 创建 Workflow，不直接写内部数据目录。
- `workflow_apply` 默认 `dry_run=true`，除非用户明确确认。
- 工具返回的错误必须是结构化的，方便 Agent 自动修复 YAML。
- Agent prompt 中明确要求：不要把未声明资源塞进 Runner 输入；不要绕过 `validate` 和 `preview`。
- `workflow_draft_create` 必须返回 `assumptions[]` 和 `openQuestions[]`；缺少文件路径、IM target、Runner id、secret/provider ref 时不能擅自填真实资源。
- `workflow_apply` 必须携带 `baseRevision`、`previewHash`、`confirmedBy` 和 `riskAccepted[]`；缺一时只能 dry-run。
- `workflow_patch_propose` 只能针对当前导出的 definition 生成 patch；如果 base revision 变化，必须重新 export 后再 patch。
- typed tools 的返回结构需要同时适合 Agent 自修和 UI 展示：错误带 `path`、`code`、`message`、`severity`、`suggestedFix`。

typed tool 输入输出草案：

```json
{
  "tool": "workflow_preview",
  "input": {
    "draft": "apiVersion: bifrost.ai.workflow/v1alpha1\nkind: Workflow\n...",
    "format": "markdown",
    "includeReactFlow": true
  },
  "output": {
    "draftHash": "sha256:...",
    "blockingErrors": [],
    "warnings": [],
    "markdown": "## DAG\n...",
    "reactFlow": { "nodes": [], "edges": [], "viewport": { "x": 0, "y": 0, "zoom": 1 } },
    "effectiveInputs": [],
    "permissionRisks": [],
    "dryRunRunbook": []
  }
}
```

### 自然语言创建示例

用户输入：

```text
每天凌晨扫描 /Audio/Daily，把新音频转成 Daily 文档。
用 Codex 提取行动项，失败就换 Bifrost Agent。
用 ChatGPT Web 提取客户问题。
最后把两个报告合并，发到飞书群。
```

Agent 生成协议草案：

```yaml
apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: daily-audio-team-report
  name: Daily Audio Team Report
spec:
  inputs:
    - name: audio_dir
      type: file_set
      default:
        type: file_ref
        path: /Audio/Daily
  triggers:
    - type: cron
      expr: "0 2 * * *"
  nodes:
    - id: transcribe
      type: asr_transcription
      outputs:
        - name: daily_markdown
          type: document
        - name: no_update
          type: no_update
      noUpdatePolicy:
        skipDownstream: true
    - id: action_items
      type: runner
      runner:
        runnerId: codex
        fallbackRunners: [bifrost_agent]
      prompt: 提取行动项，输出 Markdown 表格。
      inputs:
        - name: daily
          source:
            type: node_output
            nodeId: transcribe
            output: daily_markdown
          as: file
      outputs:
        - name: report
          type: document
          pathTemplate: reports/action-items/{{run.date}}.md
    - id: customer_issues
      type: runner
      runner:
        runnerId: chatgpt_web
      prompt: 提取客户问题、承诺和风险。
      inputs:
        - name: daily
          source:
            type: node_output
            nodeId: transcribe
            output: daily_markdown
          as: file
      outputs:
        - name: report
          type: document
          pathTemplate: reports/customer-issues/{{run.date}}.md
    - id: merge_reports
      type: script
      inputs:
        - name: action_report
          source:
            type: node_output
            nodeId: action_items
            output: report
          as: document
        - name: customer_report
          source:
            type: node_output
            nodeId: customer_issues
            output: report
          as: document
      outputs:
        - name: report
          type: document
          pathTemplate: reports/summary/{{run.date}}.md
    - id: send_feishu
      type: notification
      channel:
        providerId: feishu-main
        targetMode: configured_target
      inputs:
        - name: report
          source:
            type: node_output
            nodeId: merge_reports
            output: report
          as: document
  edges:
    - from: transcribe
      to: action_items
    - from: transcribe
      to: customer_issues
    - from: action_items
      to: merge_reports
    - from: customer_issues
      to: merge_reports
    - from: merge_reports
      to: send_feishu
```

### 协议校验与自动修复

`validate` 至少检查：

- `apiVersion/kind` 是否匹配。
- node id 唯一、edge 引用存在、DAG 无环。
- 每个节点类型的必填字段存在。
- `node_output` 引用的 output 存在。
- output path template 不允许路径穿越。
- notification channel 必须引用已配置或可选择的 provider/target。
- Runner 输入必须显式声明，不能使用隐式全量上游输出。
- ASR 替代定时任务模板必须包含 `no_update` 下游跳过策略。

自动修复只允许做无歧义操作，例如补默认 `apiVersion`、补 UI layout、规范字段命名、生成缺失 edge；涉及 Runner 选择、文件路径、IM target、权限扩大的修复必须要求用户确认。

### Workflow Run

Workflow Run 是某次执行实例，记录输入绑定、节点状态、产物索引、事件流和错误信息。Run 必须可恢复、可取消、可查看历史。

Run 状态：

- `queued`：已创建，等待执行。
- `running`：至少一个节点在执行。
- `waiting_input`：节点需要用户补充输入或选择资源。
- `succeeded`：所有必需节点完成。
- `failed`：必需节点失败且没有 fallback。
- `cancelled`：用户取消。
- `partial`：可选节点失败但 Workflow 已产出可用结果。

### Node

节点是最小执行单元，V1 支持四类：

| 节点类型 | 用途 | 执行域 | 产物 |
| --- | --- | --- | --- |
| `script` | 文本/文件转换、过滤、打包、外部同步 | 独立脚本进程或 sandbox worker | stdout 文本、文件 artifacts、JSON manifest |
| `runner` | 调用内置 Agent、Codex Runner、ChatGPT Runner、自定义 Runner 生成文档 | 独立 Agent/Runner worker 进程 | Markdown/JSON 文档、附件、摘要 |
| `asr_transcription` | 扫描指定音频目录，把新音频转录并合成为 Daily 文本文件 | ASR task/ASR worker 进程 | Daily Markdown、segments JSON、转录 manifest、no-update marker |
| `notification` | 把上游报告/摘要发送到飞书、微信或其他 IM 通道 | IM provider worker / sender | delivery receipt、message url、失败诊断 |

后续可以扩展 `approval`、`condition`、`parallel_map`、`http`、`schedule` 等节点，但 V1 先保持简单。定时触发不是单独节点，而是 Workflow trigger；它负责按 cron/interval 启动 run，节点仍按 DAG 执行。

### Artifact

Artifact 是节点输出的统一载体，既可以是内联文本，也可以是文件引用。

```json
{
  "id": "artifact_daily_md",
  "kind": "file",
  "media_type": "text/markdown",
  "path": "workflow-runs/run-123/nodes/asr/daily/2026-05-29.md",
  "sha256": "...",
  "size_bytes": 128034,
  "summary": "2026-05-29 daily transcript, 7 audio files, 12 speakers",
  "created_by": "asr_daily",
  "created_at_ms": 1780000000000
}
```

Artifact 默认保存在 Bifrost 数据目录下：

```text
agent/workflows/
  definitions/<workflow_id>.json
  runs/<run_id>/
    run.json
    events.jsonl
    nodes/<node_id>/
      input_manifest.json
      output_manifest.json
      stdout.log
      stderr.log
      artifacts/...
```

## 节点输入输出模型

### 输入类型

节点输入统一建模为 `InputBinding`：

```json
{
  "name": "daily_transcript",
  "source": {
    "type": "node_output",
    "node_id": "asr_daily",
    "output": "daily_markdown"
  },
  "as": "file",
  "required": true,
  "selector": {
    "include": ["*.md"],
    "max_bytes": 2000000
  }
}
```

`source.type` 支持：

- `workflow_input`：来自 Workflow Run 启动参数。
- `node_output`：来自上游节点输出。
- `literal_text`：用户直接写入的文本。
- `literal_script`：用户直接写入的脚本内容，通常仅给 `script` 节点使用。
- `file_ref`：用户显式选择的本地文件或目录引用。
- `artifact_query`：从当前 run 或历史 run 的 artifacts 中按标签/日期/类型查询。

`as` 支持：

- `text`：以内联文本传入。
- `file`：以文件路径传入。
- `file_set`：以文件清单传入。
- `json`：以结构化 JSON 传入。
- `document`：以文档资源传入，通常是 Markdown/HTML/PDF 等。

### 输出类型

节点输出统一建模为 `OutputSpec`：

```json
{
  "name": "action_items_doc",
  "type": "document",
  "format": "markdown",
  "path_template": "docs/{{run.date}}/action-items.md",
  "required": true,
  "expose_to_downstream": true
}
```

输出支持：

- `text`：短文本结果，可直接传给下游。
- `document`：Markdown/HTML/PDF/纯文本报告。
- `file`：单个文件。
- `file_set`：多个文件组成的清单。
- `json`：结构化数据，适合脚本和条件节点消费。
- `artifact_bundle`：打包产物，例如 ASR Daily package。
- `no_update`：节点执行成功但没有新内容，常用于定时扫描后阻断下游。

### 节点结果语义

节点完成不只区分成功/失败，还要表达“是否值得继续往下游走”。V1 统一使用 `NodeOutcome`：

- `produced`：产生了新的业务输出，下游可继续执行。
- `no_update`：节点执行成功，但输入没有变化或没有新内容；默认跳过依赖该输出的下游节点。
- `skipped`：因上游 `no_update`、条件不满足、用户禁用或权限策略被跳过。
- `failed`：节点失败；按错误策略决定是否重试、fallback、停止下游或继续 partial。
- `cancelled`：用户取消或 Workflow run 被取消。

`no_update` 是替换 ASR 定时任务的关键语义：定时触发可以照常运行 Workflow，但如果音频扫描节点发现目录里没有新增音频，节点应记录 `no_update` 日志并结束本次 run，不应继续触发 Daily Agent、脚本或通知节点。

### 默认输入传递规则

为了让编排简单但不失控，V1 采用以下默认规则：

1. 如果节点没有声明任何输入，默认使用所有直接上游节点中 `expose_to_downstream=true` 的主输出。
2. 如果节点声明了输入，只传递声明的输入，不自动附带其他上游输出。
3. Runner 节点必须在 UI 中展示最终有效输入清单，包括文本片段、文件路径、大小、来源节点和是否会进入模型上下文。
4. 大文件默认以文件引用方式传递，由 Runner prompt 只注入摘要、路径和读取说明；只有用户显式选择“inline text”时才内联。
5. 敏感文件或超出 Workflow 授权目录的文件不能传入节点。

## 节点类型设计

### Script 节点

Script 节点用于确定性处理，适合做拆分、正则提取、格式转换、目录归档、上传同步。

```json
{
  "id": "split_daily",
  "type": "script",
  "name": "Split Daily Transcript",
  "script": {
    "language": "python",
    "source": {
      "type": "inline",
      "content": "import json, pathlib\n..."
    },
    "entrypoint": "main.py",
    "timeout_ms": 120000
  },
  "inputs": [
    {
      "name": "daily_md",
      "source": { "type": "node_output", "node_id": "asr_daily", "output": "daily_markdown" },
      "as": "file"
    }
  ],
  "outputs": [
    { "name": "customer_segments", "type": "json" },
    { "name": "engineering_segments", "type": "json" }
  ]
}
```

执行契约：

- Runtime 为每次节点执行创建独立工作目录。
- 输入通过 `input_manifest.json` 和环境变量注入。
- 脚本 stdout 可作为 `outputs.text`，也可以写 `output_manifest.json` 声明多个 artifacts。
- 默认禁止访问 Workflow 授权目录外路径；后续可接入现有 File Access grant。
- 脚本超时、非零退出码、输出 manifest 非法都进入节点失败。

### Runner / Daily Agent 节点

Runner 节点用于非确定性/智能处理，目标可以是内置 Bifrost Agent、Codex Runner、ChatGPT Web Runner 或自定义 CLI Runner。Daily Agent 不再是 ASR 定时任务里的内嵌后处理，而是 Runner 节点的一种预设：输入通常是 `daily_markdown`，用户选择 Runner、Prompt、输出目录和失败策略，节点产出一个报告文件。

```json
{
  "id": "extract_actions",
  "type": "runner",
  "name": "Extract Action Items",
  "runner": {
    "runner_id": "codex",
    "adapter": "codex",
    "session_strategy": "per_workflow_node",
    "work_dir": "/Users/eden/work/reports",
    "fallback_runners": ["bifrost_agent", "chatgpt_web"]
  },
  "prompt": "请从输入的每日转录中提取行动项，输出 Markdown 表格。",
  "inputs": [
    {
      "name": "daily_transcript",
      "source": { "type": "node_output", "node_id": "asr_daily", "output": "daily_markdown" },
      "as": "file",
      "model_context": "summary_and_readable_file"
    },
    {
      "name": "focus_topics",
      "source": { "type": "workflow_input", "name": "focus_topics" },
      "as": "text",
      "model_context": "inline"
    }
  ],
  "outputs": [
    {
      "name": "report",
      "type": "document",
      "format": "markdown",
      "path_template": "reports/action-items/{{run.date}}.md"
    }
  ],
  "retry": {
    "max_attempts": 2,
    "backoff_ms": 30000,
    "fallback_runner_on_final_failure": true
  },
  "on_error": "fail_downstream"
}
```

Runner 输入策略：

- `inline`：直接把文本放入 prompt。
- `summary_only`：只注入 artifact 摘要。
- `summary_and_readable_file`：注入摘要、文件路径和读取要求，Runner 可用工具读取。
- `attachment`：作为附件/文件传给支持附件的 Runner。
- `excluded`：绑定存在但不进入模型，用于脚本后处理或审计。

Runner 输出策略：

- Runner 最终回复默认保存为 Markdown 文档 artifact。
- 如果 Runner 额外创建文件，worker 通过 `output_manifest.json` 或 run timeline 收集到 artifacts。
- 每个 Runner 节点都应产出 `summary`，便于下游不读取全文也能判断内容。
- Daily Agent 预设必须把最终报告写成文件，并把 `report_path`、`report_sha256`、`runner_id`、`prompt_version` 写入 output manifest。
- 同一个 Daily Markdown 可以 fan-out 到多个 Daily Agent 节点，每个节点使用不同 Prompt 和输出目录，例如 `reports/action-items/`、`reports/customer-issues/`、`reports/engineering-risks/`。

会话策略：

- `per_workflow_node`：每次节点执行新建独立 runner session，默认推荐。
- `reuse_by_workflow`：同一 Workflow Run 内同一 Runner 复用 session，适合多轮渐进处理。
- `external_thread_ref`：复用 ChatGPT/Codex 既有 thread/conversation，必须显式配置，避免上下文污染。

失败与 fallback：

- `max_attempts` 控制同一 Runner 的重试次数。
- `fallback_runners` 按顺序尝试备用 Runner；例如 Codex 失败后改用 Bifrost Agent，再失败改用 ChatGPT Web。
- fallback Runner 必须复用同一输入快照和 Prompt，并在日志里记录原 Runner 错误、fallback 原因和最终使用的 Runner。
- 如果 Daily Agent 节点失败且没有 fallback 成功，默认不继续执行依赖它报告的下游节点。

### ASR 转录节点

ASR 转录节点把音频文件或目录转成标准 Daily 文本文件。它只负责“从音频到文本”，不再内嵌 Daily Agent 后处理；这使得旧 ASR 定时任务可以拆成两个节点：

1. `asr_transcription`：扫描指定目录，识别新增音频，转录并合成每日 Markdown。
2. `runner` / Daily Agent：读取每日 Markdown，按 Prompt 生成报告文件。

```json
{
  "id": "asr_daily",
  "type": "asr_transcription",
  "name": "Transcribe Daily Audio",
  "asr": {
    "mode": "daily_package",
    "task_template_id": "default-daily-asr",
    "audio_grouping": "by_date",
    "scan_policy": "new_or_changed_audio_only",
    "speaker_diarization": true,
    "voiceprint_matching": true
  },
  "inputs": [
    {
      "name": "audio_dir",
      "source": { "type": "workflow_input", "name": "audio_dir" },
      "as": "file_set"
    }
  ],
  "outputs": [
    { "name": "daily_markdown", "type": "document", "format": "markdown" },
    { "name": "segments_json", "type": "json" },
    { "name": "transcription_manifest", "type": "json" },
    { "name": "no_update", "type": "no_update" }
  ],
  "no_update_policy": {
    "when": "no_new_audio_or_transcript_unchanged",
    "skip_downstream": true
  }
}
```

Daily package 内容：

- `daily/<date>.md`：适合 Runner 阅读的每日聚合 Markdown。
- `segments/<date>.json`：包含时间戳、speaker、source file、置信度等结构化片段。
- `speakers/<date>.json`：speaker diarization/voiceprint 匹配结果。
- `manifest.json`：输入音频 hash、模型配置、转录时间、产物路径、版本。
- `no_update.json`：当没有新增音频或最终 Daily Markdown hash 未变化时写入，包含扫描目录、扫描时间、已处理文件数、跳过原因。

ASR 节点可以复用现有 ASR Directory Task 运行能力，但 Workflow 视角只依赖标准输出 artifacts，不耦合具体 ASR 实现。后续迁移现有 ASR 定时任务时，旧的“目录 + 周期 + Daily Agent Runner + Prompt + 输出目录”配置应自动映射为 Workflow trigger、ASR 转录节点和 Daily Agent Runner 节点。

### Notification 节点

Notification 节点把上游文档、摘要或文件发送到 IM 通道，例如飞书、微信或未来更多 provider。

```json
{
  "id": "send_report_to_feishu",
  "type": "notification",
  "name": "Send Daily Report",
  "channel": {
    "provider_id": "feishu-main",
    "target_id": "oc_xxx",
    "target_mode": "configured_target"
  },
  "message_template": "今日音频报告已生成：{{inputs.report.summary}}",
  "inputs": [
    {
      "name": "report",
      "source": { "type": "node_output", "node_id": "summary", "output": "report" },
      "as": "document"
    }
  ],
  "outputs": [
    { "name": "delivery_receipt", "type": "json" }
  ],
  "retry": { "max_attempts": 3, "backoff_ms": 60000 }
}
```

Notification 节点必须记录 provider、target、消息 id、发送时间、失败原因和重试次数。它默认只在上游报告 `produced` 时执行；上游 `no_update` 或 `failed` 时不发送。

## DAG 编排模型

Workflow V1 使用有向无环图（DAG）：

- 节点通过 `edges` 或 `InputBinding.source.node_output` 建立依赖。
- Runtime 根据依赖拓扑执行；互不依赖的节点可以并行。
- 同一节点默认不重复执行；输入 hash 未变化且允许 cache 时可复用上次 artifacts。
- 节点失败时按 `required` 和 `on_error` 决定 Workflow 状态。

```json
{
  "edges": [
    { "from": "asr_daily", "to": "extract_actions" },
    { "from": "asr_daily", "to": "extract_customer_issues" },
    { "from": "extract_actions", "to": "summary" },
    { "from": "extract_customer_issues", "to": "summary" }
  ]
}
```

错误策略：

- `fail_workflow`：必需节点失败则整个 Workflow 失败。
- `fail_downstream`：当前节点失败后，依赖它输出的下游节点全部跳过，其他分支可继续。
- `continue_partial`：节点失败后继续执行不依赖它的分支。
- `use_fallback`：使用配置的 fallback 文本/文件/历史 artifact。
- `wait_user_input`：暂停等待用户补充输入或选择替代 artifact。
- `skip_downstream_on_no_update`：当前节点 `no_update` 时，下游节点标记为 `skipped`，run 可以以 `succeeded_no_update` 结束。

重试策略：

```json
{
  "retry": {
    "max_attempts": 3,
    "backoff_ms": 60000,
    "retry_on": ["timeout", "runner_error", "network_error"],
    "do_not_retry_on": ["input_missing", "permission_denied", "no_update"]
  }
}
```

节点每次 attempt 都要写入独立日志片段和 attempt metadata。重试不能覆盖上一次失败证据。

## 替换现有 ASR 定时任务模式

现有 ASR 定时任务可以被表达为一个最小 Workflow：

```text
Trigger: cron/interval

Node 1: transcribe_daily_audio (asr_transcription)
  input: audio_dir
  behavior: 扫描新增/变化音频 -> 转录 -> 合成 daily/<date>.md
  output produced: daily_markdown
  output no_update: no_update.json

Node 2: run_daily_agent (runner preset: Daily Agent)
  input: transcribe_daily_audio.daily_markdown
  config: runner_id + prompt + report output dir
  output: reports/<runner-or-purpose>/<date>-report.md
```

迁移规则：

- 旧 ASR task 的 `audio_dir`、schedule 周期和 enabled 状态映射到 Workflow input 与 trigger。
- 旧 ASR 转录配置映射到 `asr_transcription` 节点。
- 旧 Daily Agent 的 Runner、Prompt、report 目录映射到 `runner` 节点。
- 旧 processed state 迁移为 Workflow artifact manifest / node output hash，避免无变化时重复跑 Daily Agent。
- 旧 Run Records 迁移或兼容读取 Workflow Run 历史。

替换后增强能力：

- 一个 ASR 转录节点后可以连接多个 Daily Agent 节点，分别输出不同报告目录。
- Daily Agent 后可以继续接脚本节点做结构化提取、格式转换、同步归档。
- Daily Agent 或脚本后可以接 Notification 节点，把报告发送到 Feishu/Weixin。
- 任一节点都可以有重试、fallback、跳过下游和日志审计。

`no_update` 行为：

- 定时 Workflow 每次按计划启动。
- `asr_transcription` 扫描后发现没有新增音频，或合成后的 Daily Markdown hash 与上次相同，则输出 `no_update`。
- Runtime 将依赖该输出的 Daily Agent、脚本、通知节点标记为 `skipped`。
- Run 终态为 `succeeded_no_update`，并写明“没有新增内容，因此未执行下游”。

Daily Agent 失败行为：

- 如果配置了重试，先按 `retry.max_attempts` 重试同一 Runner。
- 如果配置了 `fallback_runners`，在同一输入快照上按顺序切换 Runner。
- 所有 attempt 都失败时，该节点 `failed`，依赖它报告的下游节点 `skipped_due_to_failed_dependency`。
- 如果这个 Daily Agent 分支不是必需分支，其他并行 Daily Agent 分支可继续并最终形成 `partial` run。

## 资源与输入选择

用户最核心的控制点是“Runner 到底能看到什么”。因此每个 Runner 节点必须有资源选择面板：

- Workflow 启动输入：例如音频目录、业务说明、关注主题。
- 上游节点输出：例如 ASR Daily Markdown、segments JSON、脚本拆分结果。
- 历史 artifacts：例如昨天的总结、上周报告、某个固定模板。
- 静态资源：例如 prompt 模板、团队名单、客户清单、术语表。

资源选择配置示例：

```json
{
  "resource_policy": {
    "default": "deny",
    "allowed_inputs": ["daily_transcript", "focus_topics", "team_roster"],
    "max_inline_bytes": 64000,
    "max_file_bytes": 20000000,
    "allow_tool_read": true,
    "allow_network": false
  }
}
```

原则：

- 默认最小授权，Runner 只收到声明资源。
- 所有进入模型的文本必须可预览、可审计。
- 文件路径传递必须受工作目录和 File Access policy 约束。
- 大文件优先以路径和摘要传递，避免上下文爆炸。
- 下游节点引用上游 artifact 时使用 artifact id，而不是裸绝对路径，Runtime 再解析成受控路径。

## UI 设计

AI 模块新增 `Workflow` 一级/二级入口，建议在 AI 页面下与 Agent、ASR、Skills 等能力并列。

### Workflow 列表

- 展示 Workflow 名称、最近运行状态、最近产物、触发方式、更新时间。
- 支持新建、复制、导入/导出 JSON、删除、手动运行。

### Workflow 编辑器

V1 使用开源 React Flow 渲染和编辑 Workflow DAG，同时保留右侧表单配置面板。React Flow 只负责节点图交互和布局，执行协议仍以 `spec` 为唯一事实源。

React Flow 能力：

- 从 Workflow 协议渲染节点和连线。
- 拖拽新增 `asr_transcription`、`runner`、`script`、`notification` 节点。
- 拖拽连线后自动生成或更新 `edges`。
- 点击节点在右侧表单编辑执行配置、输入绑定、输出规格、错误策略。
- 支持自动布局、手动布局、折叠/展开分支、节点状态着色。
- 支持 run 详情只读模式：按执行状态高亮节点，点击查看日志和 artifacts。

协议与 React Flow 的映射：

```text
Workflow spec.nodes[*] -> ReactFlowNode.data.semanticNode
Workflow spec.edges[*] -> ReactFlowEdge.data.semanticEdge
Workflow ui.reactFlow.nodes[*].position -> ReactFlowNode.position
Workflow ui.reactFlow.edges[*].style -> ReactFlowEdge.style
```

编辑规则：

- 节点执行语义只能写入 `spec.nodes` / `spec.edges`。
- `ui.reactFlow` 只保存坐标、缩放、折叠、展示样式。
- 删除节点时必须同步删除关联 edges 和下游输入引用，若会破坏 DAG，UI 要显示影响范围并要求确认。
- 从 Agent/CLI 创建的 Workflow 即使没有 `ui.reactFlow`，WebUI 也必须能自动布局渲染。
- WebUI 保存前必须调用同一套 `workflow_validate`，不能只依赖前端校验。

编辑器结构：

1. 基本信息：名称、描述、默认工作目录、默认资源策略。
2. 输入定义：新增文本、文件、文件集、JSON、日期等参数。
3. React Flow 画布：节点、边、状态、布局。
4. 节点配置面板：选择类型、执行器、输入绑定、输出规格、错误策略。
5. 预览：展示每个节点最终有效输入和可能产出的 artifacts。
6. 协议视图：展示 YAML/JSON，可复制、导出，也可在高级模式直接编辑后重新校验并渲染。

ASR 定时任务迁移入口：

- ASR 页面保留现有入口时，应提供“转换为 Workflow”或“使用 Workflow 编排”的引导。
- 新建音频 Workflow 时必须默认提供可编辑模板：`default-asr-transcription`（`定时转录目录 -> Daily Agent 报告`）。
- 模板默认创建两个节点：`transcribe_daily_audio(asr_transcription)` 和 `run_daily_agent(runner/Daily Agent)`。
- 模板通过 `GET /_bifrost/api/ai/workflows/templates` 暴露，并同时返回结构化 `workflow` 与可直接编辑/保存的 YAML `draft`；CLI 通过 `bifrost ai workflow templates` 和 `bifrost ai workflow template default-asr-transcription --output <file>` 暴露同一模板。
- WebUI Workflow 页面首次进入时加载默认 ASR 模板到草稿编辑器，用户可以直接修改 `audio_dir`、schedule、prompt、输出路径、Runner 输入和后续节点后再 validate / preview / apply / run。
- 用户可继续添加多个 Daily Agent、脚本、通知节点。

快速调试入口：

- Workflow 编辑器必须提供 Quick Debug 操作，面向“刚改完模板马上确认能否跑通”的场景。
- Quick Debug 按 `validate -> preview -> check apply -> save -> execute -> logs` 记录每一步状态，其中 `check apply` 只做不保存的安全检查，最终必须保存并执行完整 Workflow；失败时必须把失败步骤和错误摘要留在 Trace 面板，避免用户只能看到 toast。
- Quick Debug 与普通 Run 使用同一组可编辑调试输入，V1 至少暴露默认 ASR 模板需要的 `audio_dir`，后续扩展为根据 `spec.inputs` 自动生成表单。
- Quick Debug 不绕过后端校验；只有 validate 通过且 preview 无 blocking errors 时才保存和运行。
- React Flow 画布必须在默认模板加载和切换模板后自动预览渲染，让用户无需先阅读 YAML 也能理解任务流。

节点配置需要特别暴露：

- `asr_transcription`：扫描目录、周期触发、无更新判定、Daily 文档输出路径。
- Daily Agent Runner：Runner 列表、Prompt、输出目录、重试次数、备用 Runner。
- Notification：Provider、Target、消息模板、附件/文档发送方式。
- 每个节点：失败策略、`no_update` 下游行为、日志保留策略。

### Workflow Run 详情

- 顶部展示 run 状态、耗时、触发来源、输入参数。
- 节点时间线展示 queued/running/succeeded/failed、日志、Runner 进度事件。
- 节点时间线展示 `no_update`、`skipped`、`retrying`、`fallback_runner_started`、`skipped_due_to_failed_dependency` 等状态。
- Artifacts 面板按节点展示文档、文件、JSON，可下载/预览/复制路径。
- Runner 节点展示“实际传入模型的输入预览”，用于排查资源选择是否正确。
- Logs 面板按 run 和节点展示结构化日志文件路径、attempt 历史、错误摘要和跳过原因。

Run 详情使用同一个 React Flow 画布的只读模式：

- `produced/succeeded` 节点绿色。
- `no_update` 节点蓝灰色。
- `skipped` 节点灰色。
- `failed` 节点红色。
- `retrying/fallback` 节点展示 attempt badge。
- 点击节点打开对应 logs/artifacts/effective input preview。

WebUI 主题要求：所有颜色、边框、状态标签、节点状态线使用 CSS 变量，亮色/暗色主题都必须可读。

## API 设计

```http
GET    /_bifrost/api/ai/workflows
POST   /_bifrost/api/ai/workflows
GET    /_bifrost/api/ai/workflows/{workflow_id}
PUT    /_bifrost/api/ai/workflows/{workflow_id}
DELETE /_bifrost/api/ai/workflows/{workflow_id}
POST   /_bifrost/api/ai/workflows/validate
POST   /_bifrost/api/ai/workflows/preview
POST   /_bifrost/api/ai/workflows/render

POST   /_bifrost/api/ai/workflows/{workflow_id}/runs
GET    /_bifrost/api/ai/workflows/{workflow_id}/runs
GET    /_bifrost/api/ai/workflow-runs/{run_id}
GET    /_bifrost/api/ai/workflow-runs/{run_id}/events
GET    /_bifrost/api/ai/workflow-runs/{run_id}/logs
POST   /_bifrost/api/ai/workflow-runs/{run_id}/cancel
GET    /_bifrost/api/ai/workflow-runs/{run_id}/artifacts/{artifact_id}

POST   /_bifrost/api/ai/workflows/migrate/asr-task/{task_id}/preview
POST   /_bifrost/api/ai/workflows/migrate/asr-task/{task_id}/apply
```

`render` 返回 React Flow 可消费的数据：

```json
{
  "nodes": [
    {
      "id": "transcribe",
      "type": "asr_transcription",
      "position": { "x": 0, "y": 0 },
      "data": {
        "label": "Transcribe Daily Audio",
        "semanticNodeId": "transcribe",
        "status": "idle",
        "outputs": ["daily_markdown", "no_update"]
      }
    }
  ],
  "edges": []
}
```

`preview` 返回给 WebUI 和 Agent：

- DAG 摘要。
- React Flow 初始渲染数据。
- 每个节点 effective input preview。
- 资源权限风险。
- 输出路径预览。
- `no_update` / retry / fallback / notification 行为摘要。

Run 创建请求：

```json
{
  "inputs": {
    "audio_dir": { "type": "file_ref", "path": "/Users/eden/audio/2026-05-29" },
    "focus_topics": { "type": "text", "content": "行动项、客户问题、研发风险" }
  },
  "dry_run": false
}
```

Dry Run：

- 校验 DAG、输入绑定、资源权限、输出路径模板。
- 生成每个节点的 effective input preview。
- 不执行脚本、不启动 Runner、不跑 ASR。

## Runtime 架构

### 主进程职责

- Workflow definition CRUD。
- Workflow Run 状态机与持久化。
- DAG 调度与节点依赖判断。
- 输入/输出 manifest 管理。
- 子进程 worker 启停、事件转发、取消。
- API/SSE/WebSocket 状态查询。

主进程禁止直接执行节点业务逻辑，避免和代理服务抢 CPU。

### Worker 职责

- `script` 节点：脚本 worker 执行脚本、收集 stdout/stderr、写 output manifest。
- `runner` 节点：复用 Agent/Runner 进程隔离能力，每个节点 run 启动独立 worker 或按策略复用独立 worker。
- `asr_transcription` 节点：调用 ASR task/ASR worker，输出标准 Daily package。

Worker 统一通过 NDJSON 事件上报：

- `node_started`
- `node_progress`
- `node_artifact_created`
- `node_no_update`
- `node_retry_scheduled`
- `node_fallback_runner_started`
- `node_skipped`
- `node_succeeded`
- `node_failed`
- `node_cancelled`

### 状态一致性

- `events.jsonl` 是 run 事件审计日志。
- `run.json` 是当前状态快照，可由 events 重放恢复。
- 每个节点有独立 `input_manifest.json` 和 `output_manifest.json`。
- 每个节点有独立 `attempts/<attempt_no>/stdout.log`、`stderr.log`、`attempt.json`；fallback Runner 会生成新的 attempt。
- 节点开始前冻结输入快照，保证后续上游 artifact 被删除/覆盖也不影响本次 run 可审计性。
- `logs/index.json` 汇总 run 级日志、节点日志、attempt 日志和关键错误，供 WebUI 与 API 快速展示。

## 数据模型草案

```rust
struct WorkflowDefinition {
    id: String,
    name: String,
    description: Option<String>,
    version: u64,
    inputs: Vec<WorkflowInputSpec>,
    outputs: Vec<WorkflowOutputSpec>,
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
    resource_policy: WorkflowResourcePolicy,
    ui: WorkflowUiState,
}

enum WorkflowNodeKind {
    Script(ScriptNodeConfig),
    Runner(RunnerNodeConfig),
    AsrTranscription(AsrTranscriptionNodeConfig),
    Notification(NotificationNodeConfig),
}

struct WorkflowNode {
    id: String,
    name: String,
    kind: WorkflowNodeKind,
    inputs: Vec<InputBinding>,
    outputs: Vec<OutputSpec>,
    required: bool,
    on_error: NodeErrorPolicy,
}

enum InputSource {
    WorkflowInput { name: String },
    NodeOutput { node_id: String, output: String },
    LiteralText { content: String },
    LiteralScript { content: String },
    FileRef { path: String },
    ArtifactQuery { query: ArtifactQuery },
}

enum ArtifactKind {
    Text,
    Document,
    File,
    FileSet,
    Json,
    ArtifactBundle,
    NoUpdate,
}

enum NodeOutcome {
    Produced,
    NoUpdate,
    Skipped,
    Failed,
    Cancelled,
}

struct RetryPolicy {
    max_attempts: u32,
    backoff_ms: u64,
    fallback_runners: Vec<String>,
}

struct WorkflowUiState {
    react_flow: ReactFlowState,
}

struct ReactFlowState {
    nodes: Vec<ReactFlowNodeLayout>,
    edges: Vec<ReactFlowEdgeLayout>,
    viewport: Option<ReactFlowViewport>,
}
```

## 与现有模块关系

- Agent/Runner：Runner 节点复用 IM Gateway Runner 配置与 Agent worker/external-runner-worker 进程隔离。
- ASR：ASR 节点复用 ASR Directory Task 的转录能力，但把定时任务的 Daily Agent 后处理拆成独立 Runner 节点；旧 ASR 定时任务最终迁移为 Workflow trigger + ASR 转录节点 + Daily Agent Runner 节点。
- Skills：后续可把 Workflow 打包成 Skill，或让 Agent 通过工具创建/运行 Workflow。
- Schedule：后续可支持定时触发 Workflow，例如每天凌晨处理前一天音频。
- IM Gateway：Notification 节点可把 Workflow run 结果发送到 Feishu/Weixin，并支持通过 IM 触发。

## 权限与安全

- Workflow definition 中保存的文件路径必须经过 allowlist/File Access policy 校验。
- 脚本节点默认不继承主进程全部环境变量，只注入白名单环境和输入 manifest 路径。
- Runner 节点默认不获得未声明资源；prompt 中必须列出可读资源边界。
- ASR 节点只能读取启动输入绑定的音频文件集。
- Artifact 下载 API 必须校验 run 所属和路径穿越。
- 所有 secret 输入使用 secret ref，不直接写入 run events 或 input manifest 明文。

安全边界细化：

- 自然语言 draft 的权限默认全为 `request_only`，不能直接生成已授权的 `fileAccessRefs`、`secretRefs` 或 `imTargetRefs`。
- `workflow_preview` 必须把所有 host side effect 分组展示：读取本机目录、写入本机目录、启动 Runner、执行脚本、发送 IM、访问网络、复用浏览器登录态。
- `script` 节点默认 `allow_network=false`、`allow_shell=false`、`allow_write=false`；需要写文件时只能写 Runtime 分配的 node workdir 或显式授权目录。
- `runner` 节点不能隐式继承 Workflow 所在目录、父 Agent 会话上下文、IM 入站原消息附件或浏览器登录态；每类资源都要在 effective input preview 中出现。
- `notification` 节点必须引用已有 provider/target，Agent 不能凭自然语言创建新 webhook secret 或把消息发往未确认目标。
- `asr_transcription` 节点只能读取 `audio/*`、`video/*` 或显式允许的文件类型；扫描目录时必须遵守最大文件数、最大单文件大小、最大总字节数和符号链接策略。
- 导入外部 Workflow 文件时必须进入 untrusted 状态，只允许 validate/preview/render；用户确认并绑定本机资源前不得 run。
- Runtime 日志和 preview 对敏感路径做可配置脱敏，但磁盘审计文件必须保留 artifact id、hash、大小和授权 ref，保证可追踪。
- 所有节点执行前冻结 `input_manifest.json`；重试、fallback 和 resume 都使用同一输入快照，避免“第二次 attempt 看到不同文件”导致审计漂移。

## 可观测性

每个 run 记录：

- Workflow id/version。
- 触发来源：手动、schedule、IM、API、ASR task hook。
- 每个节点的开始/结束时间、耗时、退出码、runner id、ASR task id。
- 每个 artifact 的路径、hash、大小、media type、摘要。
- Runner effective input preview 和 token/context 使用量。
- 每个节点 attempt 的 stdout/stderr、结构化错误、重试次数、fallback Runner 切换记录。
- `no_update` 与 `skipped` 的原因：无新增音频、Daily 文档 hash 未变化、上游失败、条件不满足、用户禁用。
- 错误分类：输入缺失、权限拒绝、脚本失败、Runner 失败、ASR 失败、通知失败、取消、超时。

日志文件布局：

```text
runs/<run_id>/
  events.jsonl
  logs/
    index.json
    run.log
  nodes/<node_id>/
    input_manifest.json
    output_manifest.json
    attempts/1/attempt.json
    attempts/1/stdout.log
    attempts/1/stderr.log
    attempts/2/attempt.json
    attempts/2/stdout.log
    attempts/2/stderr.log
```

每次 Workflow 执行完成后，即使是 `succeeded_no_update` 或 `failed`，也必须能通过 Run 详情和磁盘日志回答：为什么跑、扫描了什么、是否有新内容、哪个节点执行/跳过、Runner 尝试了几次、最终产物在哪里。

## 典型示例：替换 ASR 定时任务的每日音频 Workflow

```text
Workflow inputs:
  audio_dir: /Audio/2026-05-29
  focus_topics: 行动项、客户问题、研发风险

Trigger:
  每天 02:00 执行

Nodes:
  1. transcribe_daily_audio (asr_transcription)
     input: audio_dir
     output produced: daily_markdown, segments_json, transcription_manifest
     output no_update: no_update.json, skip downstream

  2. action_items_agent (runner: Daily Agent / Codex)
     input: transcribe_daily_audio.daily_markdown
     prompt: 提取行动项
     output: reports/action-items/2026-05-29.md
     retry: same runner x2, fallback to Bifrost Agent

  3. customer_issues_agent (runner: Daily Agent / ChatGPT Web)
     input: transcribe_daily_audio.daily_markdown
     prompt: 提取客户问题和承诺
     output: reports/customer-issues/2026-05-29.md

  4. engineering_risks_agent (runner: Daily Agent / Bifrost Agent)
     input: transcribe_daily_audio.daily_markdown + focus_topics
     prompt: 提取研发风险
     output: reports/engineering-risks/2026-05-29.md

  5. normalize_reports (script)
     input: action_items_agent.report + customer_issues_agent.report + engineering_risks_agent.report
     output: reports/summary/2026-05-29.md

  6. send_to_feishu (notification)
     input: normalize_reports.report
     target: feishu-main / team channel
     output: delivery_receipt.json
```

这个例子体现：旧 ASR 定时任务被拆成“定时触发 + 转录节点 + Daily Agent Runner 节点”；同一 Daily Markdown 可以分流给多个 Runner；每个 Runner 只读取声明资源；每个 Runner 都产出文档；后续脚本和通知节点可以继续消费这些文档，而不需要重新读取全部原始音频或完整 transcript。

## 分阶段落地建议

### Phase 1：Definition + Dry Run + ASR 定时任务迁移预览

- Workflow CRUD。
- 节点 schema、DAG 校验、输入绑定校验。
- Dry Run effective input preview。
- ASR 定时任务迁移 preview：把旧配置展示为 trigger + `asr_transcription` + Daily Agent Runner。
- Run 状态和 artifacts 存储。

### Phase 2：最小可替代 ASR 定时任务

- `asr_transcription` 节点扫描指定目录并生成 Daily Markdown。
- Daily Agent Runner 节点读取 Daily Markdown 并产出报告文件。
- `no_update` 语义与下游 skip。
- 每次 run 落盘 events、run.log、node attempt logs。

### Phase 3：多 Runner fan-out 与 fallback

- 接入内置 Bifrost Agent、Codex Runner、ChatGPT Web Runner、自定义 Runner。
- Runner effective input preview。
- Runner 文档产出 artifact 化。
- Runner 节点 stop/cancel 与进程隔离。
- 多 Daily Agent 并行分流到不同输出目录。
- 同一 Runner 重试与 fallback Runner。

### Phase 4：脚本与通知节点

- Script 节点处理多个报告文件，输出汇总或结构化 JSON。
- Notification 节点发送飞书/微信消息。
- Notification 重试与发送回执 artifact。

### Phase 5：WebUI 编排与触发器

- AI Workflow 列表、编辑器、Run 详情。
- Schedule/IM/API 触发。
- 历史 artifacts 复用。

## 测试方案

### 单元测试

- `workflow_definition_rejects_cycle`：DAG 环检测。
- `workflow_input_binding_resolves_node_output_file`：上游文件输出绑定到下游输入。
- `workflow_input_binding_resolves_literal_text_and_script`：显式文本和脚本内容绑定。
- `runner_effective_input_excludes_undeclared_artifacts`：Runner 只收到声明资源。
- `workflow_artifact_manifest_rejects_path_traversal`：artifact 路径穿越拒绝。
- `asr_daily_node_outputs_daily_markdown_manifest`：ASR 节点输出 Daily Markdown 与 transcription manifest。
- `asr_daily_node_no_update_skips_downstream`：无新增音频或 Daily Markdown hash 不变时，下游 Daily Agent/通知节点跳过。
- `runner_retry_then_fallback_runner_preserves_input_snapshot`：Runner 重试和 fallback 使用同一输入快照并记录 attempt。
- `notification_node_uses_report_artifact_and_records_receipt`：通知节点消费报告 artifact 并记录发送回执。
- `legacy_asr_schedule_maps_to_workflow_nodes`：旧 ASR 定时任务配置可映射为 trigger + ASR 转录节点 + Daily Agent Runner 节点。
- `workflow_protocol_roundtrip_preserves_spec`：YAML/JSON 协议导入、React Flow 编辑、导出后执行语义不丢失。
- `workflow_render_generates_react_flow_layout_when_missing_ui_state`：Agent/CLI 创建的无 UI layout 协议可自动渲染。
- `workflow_agent_tool_validate_blocks_implicit_all_upstream_inputs`：Agent 工具生成协议时禁止 Runner 隐式消费全部上游输出。

### E2E 测试

- `test_ai_workflow_script_runner_chain.sh`：脚本节点生成文本/文件，External CLI 测试 Runner 消费指定文件并产出 Markdown。
- `test_ai_workflow_asr_daily_fanout.sh`：构造轻量音频/fixture 或 ASR 测试输入，验证 Daily Markdown 分流到多个 Daily Agent Runner，再由脚本汇总。
- `test_ai_workflow_runner_input_policy.sh`：上游有多个 artifacts，但 Runner 只收到声明输入。
- `test_ai_workflow_no_update_skips_downstream.sh`：第二次定时运行没有新增音频时，Daily Agent、脚本、通知节点不执行。
- `test_ai_workflow_runner_retry_fallback.sh`：主 Runner 失败后重试并切换备用 Runner，日志保留全部 attempt。
- `test_ai_workflow_notification_node.sh`：报告生成后发送到 IM provider 测试通道并记录 delivery receipt。
- `test_ai_workflow_cli_agent_protocol_roundtrip.sh`：Agent 生成 YAML，CLI validate/preview/create/export，WebUI React Flow render 后二次编辑，协议可再次 validate。
- `test_ai_workflow_react_flow_editing.sh`：React Flow 拖拽新增节点、连线、删除节点后，后端 schema 校验和 DAG 校验一致。

### 真实场景测试

- `human_tests/ai-workflow.md` 覆盖设计静态验收、ASR 定时任务替换、Workflow 协议、Agent Runner 自然语言创建、CLI validate/preview/create、React Flow 渲染与二次编辑、Dry Run 预期、每日音频 Workflow 端到端验收计划、无更新跳过、Runner 重试/fallback、通知节点和日志回溯。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：AI 模块 Workflow、自定义编排、替换 ASR 定时任务、转录节点 + Daily Agent 节点、多个 Daily Agent fan-out、脚本/通知后续节点、容错、重试、fallback、日志回溯、协议化、Agent Runner 通过 Bifrost CLI/工具自然语言创建、React Flow 渲染和二次编辑。
- Review 范围：本设计文档、`human_tests/ai-workflow.md`、`human_tests/readme.md`。
- 风险点：编排过复杂、默认输入传递过宽、ASR 节点与现有 ASR task 耦合过深、Runner 上下文膨胀、脚本权限边界不清、`no_update` 误跳过、fallback Runner 污染上下文、日志覆盖失败证据、React Flow UI state 与 spec 执行语义漂移、Agent 未经 validate/preview 直接 apply。
- 验证命令：`rg` 静态检查关键设计点。

### 第 2 轮

- 复查第 1 轮修正后的 diff 与 human_tests 索引。
- 再次检查用户补充需求是否全部落到设计：AI 模块入口、ASR 定时任务替换、每日音频、Daily Markdown、分流、多 Daily Agent 文档、脚本/通知节点、资源选择、容错、日志、协议、Agent 工具、CLI、React Flow。
- 复跑静态验收命令；如发现遗漏，追加第 3 轮。

## 校验要求

本轮仅设计方案，不实现代码：

- `rg -n "AI Workflow|asr_transcription|Runner 节点|Script 节点|Notification 节点|no_update|fallback Runner|ASR 定时任务|apiVersion|bifrost ai workflow|workflow_draft_create|React Flow" design/ai-workflow.md`
- `rg -n "TC-AIW-01|TC-AIW-02|TC-AIW-03|TC-AIW-04|TC-AIW-05|TC-AIW-06|TC-AIW-07|TC-AIW-08|TC-AIW-09|TC-AIW-10|TC-AIW-11|TC-AIW-12|TC-AIW-13|TC-AIW-14|TC-AIW-15" human_tests/ai-workflow.md`
- `rg -n "ai-workflow.md" human_tests/readme.md`

后续实现阶段还必须执行对应 Rust 单元测试、E2E、真实服务 human_tests、`cargo test --workspace --all-features`、clippy 和 local-ci。

## 文档更新要求

- 新增 `design/ai-workflow.md`。
- 新增 `human_tests/ai-workflow.md`。
- 更新 `human_tests/readme.md` 索引。

## 2026-06-02 发布化补齐说明

本轮实现把 Workflow 从“可保存/可预览/可记录 run”的 MVP 推进为可真实执行的发布候选：

- `asr_transcription` 节点接入现有 ASR Directory Task 真实执行链路，使用 Workflow 专属隐藏 task id 扫描用户配置的 `audio_dir`，生成 ASR run 结果、Daily Markdown 引用和 transcription manifest；当没有新音频或没有失败时返回 `no_update`。
- Runtime 按 DAG 拓扑顺序执行节点，`noUpdatePolicy.skipDownstream=true` 会把所有下游节点写成 `skipped`，并保留 input/output manifest、attempt stdout/stderr、events、run.log、logs/index.json。
- `runner` 节点接入 External CLI Runner 配置，支持 `runner` / `runnerId`、`fallbackRunner`、timeout、sessionKey、instructions 和显式 effective inputs；默认 ASR 模板显式使用 `runner: codex`，未显式声明 Runner 的自定义节点会读取 External CLI 默认 Runner，不再隐式落到 mock；fallback 保留 primary failure 证据。
- `script` 节点真实执行 `sh -c`，通过 `BIFROST_WORKFLOW_INPUT` 和 `BIFROST_WORKFLOW_OUTPUT` 传递输入/输出，产出 artifact 文件，不再只生成占位结果。
- `retryStrategy.maxAttempts` 进入运行时语义，每个 attempt 独立落盘 `attempt.json`、`stdout.log`、`stderr.log`，失败 attempt 不覆盖后续成功 attempt。
- Workflow schedule 入口已可运行：保存的 Workflow 若包含 enabled schedule trigger，后台 scheduler 会随 Admin 服务启动，计算 `nextRunAt` 并按 trigger inputs 执行完整 Workflow；`GET /api/ai/workflows/schedules` 可观察 workflowId、triggerIndex、last/next run 状态，E2E 已覆盖真实 schedule run 的日志、轨迹和 artifact。
- WebUI Quick Debug 保留 `Check Apply` 作为不保存的安全检查，但最终流程是 `validate -> preview -> check -> save -> execute -> logs`，执行完整 Workflow 并展示 run 轨迹摘要。
- CLI `bifrost ai workflow run` 已改为“执行 Workflow 并持久化轨迹”，不再描述为 dry-run run record。

仍需后续增强但不阻塞本轮发布候选的边界：

- Notification 节点不再只是记录 delivery request：默认会写入本地 `ai_workflow` 通知记录并产出 `notification_receipt.json`；当 `channel` 配置 provider/target 时会复用 IM Gateway `send_msg` 能力真实发送到飞书/微信等目标，并在 metadata 中保留 message id / receipt 或失败诊断。
- Schedule cron parser 目前覆盖 `*/N * * * *`、`N * * * *`、`M H * * *` 等常见本地模式；复杂 cron、timezone 精确换算和错过触发补偿可在后续增强。
- 内置 `bifrost_agent` Runner 仍建议通过 External CLI Runner/Agent worker 统一隔离执行；若产品需要专门内置 runner id，可在 runner node 增加显式 adapter。
