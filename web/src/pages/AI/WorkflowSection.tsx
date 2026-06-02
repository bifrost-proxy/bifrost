import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Alert,
  Button,
  Card,
  Empty,
  Input,
  List,
  Segmented,
  Space,
  Tabs,
  Tag,
  Typography,
  message,
  theme,
} from "antd";
import {
  BugOutlined,
  CodeOutlined,
  EditOutlined,
  EyeOutlined,
  HistoryOutlined,
  PlayCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  SaveOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { Background, Controls, MiniMap, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  applyAiWorkflowDraft,
  getAiWorkflow,
  getAiWorkflowRun,
  listAiWorkflowRuns,
  listAiWorkflowTemplates,
  listAiWorkflows,
  previewAiWorkflow,
  runAiWorkflow,
  validateAiWorkflow,
  workflowToDraft,
  type WorkflowDocument,
  type WorkflowPreview,
  type WorkflowRun,
  type WorkflowSummary,
  type WorkflowTemplate,
} from "../../api/aiWorkflow";

const { Text, Title, Paragraph } = Typography;

type DebugStep = {
  title: string;
  status: "success" | "error" | "info";
  detail: string;
};

type WorkflowNodeData = {
  label?: string;
  kind?: string;
  outputs?: Array<{ name?: string; type?: string; pathTemplate?: string }>;
};

type DetailTab = "editor" | "runs" | "debug";
type EditorMode = "visual" | "code";

const SAMPLE_WORKFLOW = `apiVersion: bifrost.ai.workflow/v1alpha1
kind: Workflow
metadata:
  id: default-asr-transcription
  name: Default ASR Transcription Workflow
  description: Scan an audio directory, transcribe new files, then generate a Daily Agent report.
  labels:
    bifrost.io/template: default-asr
spec:
  schemaRef: bifrost://schemas/ai-workflow/v1alpha1
  resourcePolicy:
    default: deny
  triggers:
    - type: schedule
      enabled: false
      cron: "0 8 * * *"
      timezone: Asia/Shanghai
      description: Enable and adjust this schedule after choosing the audio directory.
  inputs:
    - name: audio_dir
      type: file_set
      required: true
      description: Directory containing audio files to transcribe.
    - name: focus_topics
      type: text
      required: false
      default: "行动项、客户问题、研发风险"
      description: Topics that the Daily Agent should emphasize in the final report.
  nodes:
    - id: transcribe_daily_audio
      type: asr_transcription
      inputs:
        - name: audio_dir
          source:
            type: workflow_input
            name: audio_dir
          as: file_set
      noUpdatePolicy:
        skipDownstream: true
      outputs:
        - name: daily_markdown
          type: document
          pathTemplate: daily/{{run.date}}.md
        - name: transcription_manifest
          type: json
          pathTemplate: manifests/{{run.date}}.json
    - id: run_daily_agent
      type: runner
      runner: codex
      prompt: Read the Daily Markdown and generate a concise report with action items, risks, and follow-ups.
      retryStrategy:
        maxAttempts: 2
      inputs:
        - name: daily_markdown
          source:
            type: node_output
            nodeId: transcribe_daily_audio
            output: daily_markdown
          as: document
        - name: focus_topics
          source:
            type: workflow_input
            name: focus_topics
          as: text
      outputs:
        - name: daily_report
          type: document
          pathTemplate: reports/daily-agent/{{run.date}}.md
  edges:
    - from: transcribe_daily_audio
      to: run_daily_agent
  outputs:
    - name: final_report
      type: document
      from: run_daily_agent.outputs.daily_report
`;

function parseWorkflowId(draft: string): string | undefined {
  const yamlId = draft.match(/^\s*id:\s*([^\n]+)/m)?.[1]?.trim().replace(/^[ '"]|[ '"]$/g, "");
  if (yamlId) {
    return yamlId;
  }
  return draft.match(/^\s*"id"\s*:\s*"([^"]+)"/m)?.[1]?.trim();
}

function workflowDisplayName(workflow?: WorkflowDocument, fallback = "Untitled Workflow") {
  return workflow?.metadata?.name || workflow?.metadata?.id || fallback;
}

function getNodeColor(kind: string, token: ReturnType<typeof theme.useToken>["token"]): string {
  if (kind === "asr_transcription") {
    return token.colorPrimary;
  }
  if (kind === "runner") {
    return token.colorInfo;
  }
  if (kind === "script") {
    return token.colorWarning;
  }
  if (kind === "notification") {
    return token.colorSuccess;
  }
  return token.colorBorder;
}

function toFlowNodes(preview: WorkflowPreview | null, token: ReturnType<typeof theme.useToken>["token"]): Node[] {
  return (preview?.reactFlow.nodes ?? []).map((node) => {
    const data = (node.data ?? {}) as WorkflowNodeData;
    const kind = data.kind ?? "workflow";
    const outputCount = Array.isArray(data.outputs) ? data.outputs.length : 0;
    return {
      ...node,
      id: String(node.id ?? data.label ?? kind),
      type: "default",
      position: {
        x: Number((node.position as { x?: number } | undefined)?.x ?? 0),
        y: Number((node.position as { y?: number } | undefined)?.y ?? 0),
      },
      data: {
        label: `${data.label ?? node.id ?? "node"}\n${kind}\noutputs: ${outputCount}`,
      },
      style: {
        border: `2px solid ${getNodeColor(kind, token)}`,
        background: token.colorBgContainer,
        color: token.colorText,
        borderRadius: 10,
        minWidth: 180,
        whiteSpace: "pre-line",
        boxShadow: token.boxShadowTertiary,
      },
    } as Node;
  });
}

function toFlowEdges(preview: WorkflowPreview | null): Edge[] {
  return (preview?.reactFlow.edges ?? []).map((edge) => ({
    ...edge,
    id: String(edge.id ?? `${edge.source}-${edge.target}`),
    source: String(edge.source ?? ""),
    target: String(edge.target ?? ""),
    animated: true,
  })) as Edge[];
}

function WorkflowGraph({ preview }: { preview: WorkflowPreview | null }) {
  const { token } = theme.useToken();
  const flowNodes = useMemo(() => toFlowNodes(preview, token), [preview, token]);
  const flowEdges = useMemo(() => toFlowEdges(preview), [preview]);

  return (
    <div
      data-testid="ai-workflow-reactflow-preview"
      style={{
        height: "100%",
        minHeight: 420,
        border: `1px solid ${token.colorBorderSecondary}`,
        borderRadius: 10,
        overflow: "hidden",
        background: token.colorBgContainer,
      }}
    >
      {preview ? (
        <ReactFlow nodes={flowNodes} edges={flowEdges} fitView minZoom={0.3} maxZoom={1.8}>
          <MiniMap pannable zoomable />
          <Controls />
          <Background />
        </ReactFlow>
      ) : (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description="Validate the Workflow to render the graph"
          style={{ marginTop: 150 }}
        />
      )}
    </div>
  );
}

function PreviewPanel({ preview }: { preview: WorkflowPreview | null }) {
  if (!preview) {
    return <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Validate a draft to see preview details" />;
  }
  return (
    <Space direction="vertical" size={8} style={{ width: "100%" }}>
      <Text type="secondary">Draft hash: {preview.draftHash}</Text>
      {preview.blockingErrors.length > 0 ? (
        <Alert
          type="error"
          message={`${preview.blockingErrors.length} blocking issue(s)`}
          description={preview.blockingErrors.map((item) => `${item.path}: ${item.message}`).join("\n")}
        />
      ) : (
        <Alert type="success" message="Preview has no blocking errors" />
      )}
      <pre style={{ whiteSpace: "pre-wrap", margin: 0 }}>{preview.markdown}</pre>
    </Space>
  );
}

export default function WorkflowSection() {
  const { token } = theme.useToken();
  const [searchParams, setSearchParams] = useSearchParams();
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [draft, setDraft] = useState(SAMPLE_WORKFLOW);
  const [selectedWorkflow, setSelectedWorkflow] = useState<WorkflowDocument>();
  const [selectedTemplate, setSelectedTemplate] = useState<WorkflowTemplate>();
  const [preview, setPreview] = useState<WorkflowPreview | null>(null);
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [loading, setLoading] = useState(false);
  const [lastRun, setLastRun] = useState<WorkflowRun>();
  const [debugSteps, setDebugSteps] = useState<DebugStep[]>([]);
  const [debugAudioDir, setDebugAudioDir] = useState("./human-tests/audio");
  const [editorMode, setEditorMode] = useState<EditorMode>("visual");
  const [detailTab, setDetailTab] = useState<DetailTab>("editor");
  const didLoadDefaultTemplate = useRef(false);

  const detailWorkflowId = searchParams.get("workflowId");
  const draftWorkflowId = useMemo(() => parseWorkflowId(draft), [draft]);
  const isDetailMode = Boolean(detailWorkflowId || selectedTemplate || selectedWorkflow);

  const updateWorkflowRoute = useCallback(
    (workflowId?: string | null) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-workflow");
          next.delete("asrTask");
          if (workflowId) {
            next.set("workflowId", workflowId);
          } else {
            next.delete("workflowId");
          }
          return next;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const refreshLists = useCallback(async () => {
    const [workflowResponse, templateResponse] = await Promise.all([
      listAiWorkflows(),
      listAiWorkflowTemplates(),
    ]);
    setWorkflows(workflowResponse.workflows);
    setTemplates(templateResponse.templates);
    const defaultTemplate = templateResponse.templates.find((item) => item.id === "default-asr-transcription") ?? templateResponse.templates[0];
    if (defaultTemplate && !didLoadDefaultTemplate.current && !detailWorkflowId) {
      didLoadDefaultTemplate.current = true;
      setDraft(defaultTemplate.draft);
      const nextPreview = await previewAiWorkflow(defaultTemplate.draft).catch(() => null);
      if (nextPreview) {
        setPreview(nextPreview);
      }
    }
  }, [detailWorkflowId]);

  const refreshRuns = useCallback(async (workflowId: string) => {
    const response = await listAiWorkflowRuns(workflowId);
    setRuns(response.runs);
  }, []);

  const loadWorkflow = useCallback(
    async (workflowId: string) => {
      setLoading(true);
      try {
        const response = await getAiWorkflow(workflowId);
        const nextDraft = workflowToDraft(response.workflow);
        setSelectedWorkflow(response.workflow);
        setSelectedTemplate(undefined);
        setDraft(nextDraft);
        setLastRun(undefined);
        setDebugSteps([]);
        const nextPreview = await previewAiWorkflow(nextDraft).catch(() => null);
        setPreview(nextPreview);
        await refreshRuns(workflowId).catch(() => setRuns([]));
      } catch (error) {
        message.error(`Failed to load workflow: ${error instanceof Error ? error.message : String(error)}`);
      } finally {
        setLoading(false);
      }
    },
    [refreshRuns],
  );

  useEffect(() => {
    void refreshLists().catch(() => {
      setWorkflows([]);
      setTemplates([]);
    });
  }, [refreshLists]);

  useEffect(() => {
    if (detailWorkflowId) {
      void loadWorkflow(detailWorkflowId);
    }
  }, [detailWorkflowId, loadWorkflow]);

  const handleBackToList = useCallback(() => {
    setSelectedWorkflow(undefined);
    setSelectedTemplate(undefined);
    setPreview(null);
    setRuns([]);
    setLastRun(undefined);
    setDebugSteps([]);
    updateWorkflowRoute(null);
  }, [updateWorkflowRoute]);

  const handleValidatePreview = useCallback(async () => {
    setLoading(true);
    try {
      const [validation, nextPreview] = await Promise.all([
        validateAiWorkflow(draft),
        previewAiWorkflow(draft),
      ]);
      setPreview(nextPreview);
      if (validation.valid) {
        message.success("Workflow draft is valid");
      } else {
        message.error(`Workflow has ${validation.errors.length} blocking issue(s)`);
      }
    } finally {
      setLoading(false);
    }
  }, [draft]);

  const handleCheckApply = useCallback(async () => {
    setLoading(true);
    try {
      const response = await applyAiWorkflowDraft(draft, { dryRun: true });
      if (response.preview) {
        setPreview(response.preview);
      }
      message.success("Apply check completed without saving");
    } finally {
      setLoading(false);
    }
  }, [draft]);

  const handleSave = useCallback(async () => {
    setLoading(true);
    try {
      const response = await applyAiWorkflowDraft(draft, { dryRun: false });
      await refreshLists();
      if (response.workflow) {
        setSelectedWorkflow(response.workflow);
        setSelectedTemplate(undefined);
        updateWorkflowRoute(response.workflow.metadata?.id ?? draftWorkflowId ?? null);
      }
      message.success("Workflow saved in backend");
    } finally {
      setLoading(false);
    }
  }, [draft, draftWorkflowId, refreshLists, updateWorkflowRoute]);

  const handleRun = useCallback(async () => {
    if (!draftWorkflowId) {
      message.error("Workflow metadata.id is required before running");
      return;
    }
    setLoading(true);
    try {
      await applyAiWorkflowDraft(draft, { dryRun: false });
      const response = await runAiWorkflow(draftWorkflowId, { audio_dir: debugAudioDir });
      const persisted = await getAiWorkflowRun(draftWorkflowId, response.run.id).catch(() => response);
      setLastRun(persisted.run);
      await refreshRuns(draftWorkflowId).catch(() => setRuns([persisted.run]));
      message.success(`Workflow executed: ${response.run.id} (${response.run.status})`);
    } finally {
      setLoading(false);
    }
  }, [debugAudioDir, draft, draftWorkflowId, refreshRuns]);

  const appendDebugStep = useCallback((step: DebugStep) => {
    setDebugSteps((current) => [...current, step]);
  }, []);

  const handleUseTemplate = useCallback(async (template: WorkflowTemplate) => {
    setSelectedTemplate(template);
    setSelectedWorkflow(undefined);
    setDraft(template.draft);
    setPreview(null);
    setRuns([]);
    setLastRun(undefined);
    setDebugSteps([]);
    setDetailTab("editor");
    updateWorkflowRoute(null);
    setLoading(true);
    try {
      const nextPreview = await previewAiWorkflow(template.draft);
      setPreview(nextPreview);
      message.success(`Loaded template: ${template.name}`);
    } catch (error) {
      message.warning(`Loaded template without preview: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [updateWorkflowRoute]);

  const handleCreateFromTemplate = useCallback(() => {
    const template = templates.find((item) => item.id === "default-asr-transcription") ?? templates[0];
    if (template) {
      void handleUseTemplate(template);
      return;
    }
    setSelectedTemplate(undefined);
    setSelectedWorkflow(undefined);
    setDraft(SAMPLE_WORKFLOW);
    setPreview(null);
    setRuns([]);
    setDetailTab("editor");
    updateWorkflowRoute(null);
  }, [handleUseTemplate, templates, updateWorkflowRoute]);

  const handleOpenWorkflow = useCallback((workflowId: string, tab: DetailTab = "editor") => {
    setDetailTab(tab);
    updateWorkflowRoute(workflowId);
  }, [updateWorkflowRoute]);

  const handleQuickDebug = useCallback(async () => {
    setLoading(true);
    setDebugSteps([]);
    try {
      const nextSteps: DebugStep[] = [];
      const pushStep = (step: DebugStep) => {
        nextSteps.push(step);
        setDebugSteps([...nextSteps]);
      };

      const validation = await validateAiWorkflow(draft).catch((error: unknown) => {
        pushStep({
          title: "Validate",
          status: "error",
          detail: error instanceof Error ? error.message : String(error),
        });
        throw error;
      });
      pushStep({
        title: "Validate",
        status: validation.valid ? "success" : "error",
        detail: validation.valid
          ? `valid=true, warnings=${validation.warnings.length}`
          : validation.errors.map((item) => `${item.path}: ${item.message}`).join("\n"),
      });

      const nextPreview = await previewAiWorkflow(draft);
      setPreview(nextPreview);
      pushStep({
        title: "Preview",
        status: nextPreview.blockingErrors.length === 0 ? "success" : "error",
        detail: `nodes=${nextPreview.reactFlow.nodes.length}, edges=${nextPreview.reactFlow.edges.length}, effectiveInputs=${nextPreview.effectiveInputs.length}`,
      });

      const applyCheck = await applyAiWorkflowDraft(draft, { dryRun: true });
      pushStep({
        title: "Apply Check",
        status: applyCheck.preview?.blockingErrors.length ? "error" : "success",
        detail: applyCheck.preview
          ? `draftHash=${applyCheck.preview.draftHash}, executionPlan=${applyCheck.preview.dryRunRunbook.length}`
          : "apply check response did not include preview",
      });

      if (validation.valid && nextPreview.blockingErrors.length === 0) {
        const saved = await applyAiWorkflowDraft(draft, { dryRun: false });
        setSelectedWorkflow(saved.workflow);
        setSelectedTemplate(undefined);
        pushStep({
          title: "Save",
          status: "success",
          detail: `workflow=${draftWorkflowId ?? "<unknown>"}`,
        });
        await refreshLists();
      }

      if (draftWorkflowId && validation.valid && nextPreview.blockingErrors.length === 0) {
        const response = await runAiWorkflow(draftWorkflowId, { audio_dir: debugAudioDir });
        const persisted = await getAiWorkflowRun(draftWorkflowId, response.run.id).catch(() => response);
        setLastRun(persisted.run);
        await refreshRuns(draftWorkflowId).catch(() => setRuns([persisted.run]));
        pushStep({
          title: "Run + Logs",
          status: persisted.run.status === "success" ? "success" : "info",
          detail: `run=${persisted.run.id}, status=${persisted.run.status}, events=${persisted.run.events.length}, nodes=${persisted.run.nodeStates.length}`,
        });
      }
      message.success("Workflow quick debug executed the full workflow");
    } catch (error) {
      appendDebugStep({
        title: "Quick Debug Failed",
        status: "error",
        detail: error instanceof Error ? error.message : String(error),
      });
      message.error("Workflow quick debug failed");
    } finally {
      setLoading(false);
    }
  }, [appendDebugStep, debugAudioDir, draft, draftWorkflowId, refreshLists, refreshRuns]);

  const renderListPage = () => (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Card>
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Space style={{ width: "100%", justifyContent: "space-between" }} align="start" wrap>
            <Space direction="vertical" size={4}>
              <Title level={4} style={{ margin: 0 }}>AI Workflow</Title>
              <Paragraph type="secondary" style={{ margin: 0 }}>
                Manage reusable ASR, Runner, script, and notification workflows. Open an item to edit graph/code, inspect runs, or debug execution.
              </Paragraph>
            </Space>
            <Space wrap>
              <Button icon={<ReloadOutlined />} onClick={refreshLists} loading={loading}>Refresh</Button>
              <Button type="primary" icon={<PlusOutlined />} onClick={handleCreateFromTemplate}>New Workflow</Button>
            </Space>
          </Space>
          <Space wrap>
            <Tag color="blue">v1alpha1</Tag>
            <Tag>List first</Tag>
            <Tag>Detail tabs</Tag>
            <Tag>ASR template</Tag>
          </Space>
        </Space>
      </Card>

      <Card title="Workflow List" extra={<Text type="secondary">{workflows.length} saved</Text>}>
        {workflows.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description="No saved workflows yet. Create one from the default ASR template."
          />
        ) : (
          <List
            data-testid="ai-workflow-list"
            dataSource={workflows}
            renderItem={(item) => (
              <List.Item
                actions={[
                  <Button key="open" type="link" onClick={() => handleOpenWorkflow(item.id)}>
                    Details
                  </Button>,
                  <Button key="debug" type="link" onClick={() => {
                    handleOpenWorkflow(item.id, "debug");
                  }}>
                    Debug
                  </Button>,
                ]}
              >
                <List.Item.Meta
                  avatar={<UnorderedListOutlined style={{ color: token.colorPrimary, fontSize: 18 }} />}
                  title={<Button type="link" style={{ padding: 0 }} onClick={() => handleOpenWorkflow(item.id)}>{item.name || item.id}</Button>}
                  description={
                    <Space direction="vertical" size={2}>
                      <Text type="secondary">{item.id} · rev {item.revision} · {item.nodeCount} nodes · {item.edgeCount} edges</Text>
                      <Text type="secondary">Updated {item.updatedAt}</Text>
                    </Space>
                  }
                />
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card title="Workflow Templates" extra={<Text type="secondary">Use a template to create a new workflow</Text>}>
        {templates.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No templates loaded" />
        ) : (
          <List
            data-testid="ai-workflow-template-list"
            dataSource={templates}
            renderItem={(item) => (
              <List.Item
                actions={[
                  <Button key="use" size="small" onClick={() => handleUseTemplate(item)}>
                    Use Template
                  </Button>,
                ]}
              >
                <Space direction="vertical" size={4}>
                  <Space wrap>
                    <Text strong>{item.name}</Text>
                    {item.tags.map((tag) => <Tag key={tag}>{tag}</Tag>)}
                  </Space>
                  <Text type="secondary">{item.description}</Text>
                </Space>
              </List.Item>
            )}
          />
        )}
      </Card>
    </Space>
  );

  const renderEditorTab = () => (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      <Card>
        <Space style={{ width: "100%", justifyContent: "space-between" }} wrap>
          <Segmented
            data-testid="ai-workflow-editor-mode"
            value={editorMode}
            onChange={(value) => setEditorMode(value as EditorMode)}
            options={[
              { label: "Visual Workflow", value: "visual", icon: <EyeOutlined /> },
              { label: "Code Config", value: "code", icon: <CodeOutlined /> },
            ]}
          />
          <Space wrap>
            <Button onClick={handleValidatePreview} loading={loading} icon={<ReloadOutlined />}>Validate & Preview</Button>
            <Button onClick={handleCheckApply} loading={loading} icon={<SaveOutlined />}>Check Apply</Button>
            <Button type="primary" onClick={handleSave} loading={loading}>Save</Button>
          </Space>
        </Space>
      </Card>

      {editorMode === "visual" ? (
        <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1fr) 360px", gap: 16, minHeight: 520 }}>
          <WorkflowGraph preview={preview} />
          <Card title="Preview Summary" style={{ height: "100%", overflow: "auto" }}>
            <PreviewPanel preview={preview} />
          </Card>
        </div>
      ) : (
        <Card title="Workflow Configuration">
          <Input.TextArea
            data-testid="ai-workflow-draft-editor"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            spellCheck={false}
            autoSize={{ minRows: 24, maxRows: 42 }}
            style={{ fontFamily: "var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace)" }}
          />
        </Card>
      )}
    </Space>
  );

  const renderRunsTab = () => (
    <Card title="Execution Records" extra={<Button size="small" onClick={() => draftWorkflowId && refreshRuns(draftWorkflowId)}>Refresh</Button>}>
      {runs.length === 0 ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No execution records yet" />
      ) : (
        <List
          data-testid="ai-workflow-run-list"
          dataSource={runs}
          renderItem={(run) => (
            <List.Item onClick={() => setLastRun(run)} style={{ cursor: "pointer" }}>
              <Space direction="vertical" size={4} style={{ width: "100%" }}>
                <Space wrap>
                  <Tag color={run.status === "success" ? "green" : run.status === "failed" ? "red" : "blue"}>{run.status}</Tag>
                  <Text strong>{run.id}</Text>
                  <Text type="secondary">rev {run.workflowRevision}</Text>
                </Space>
                <Text type="secondary">{run.createdAt} → {run.finishedAt ?? "running"}</Text>
                <Text type="secondary">{run.nodeStates.length} node(s), {run.events.length} event(s), artifacts: {run.artifactsDir}</Text>
              </Space>
            </List.Item>
          )}
        />
      )}
      {lastRun ? (
        <Card size="small" title="Selected Run" style={{ marginTop: 16 }}>
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Alert
              type={lastRun.status === "success" ? "success" : lastRun.status === "failed" ? "error" : "info"}
              message={`${lastRun.id} · ${lastRun.status}`}
              description={`${lastRun.nodeStates.length} node(s), ${lastRun.events.length} event(s), artifacts: ${lastRun.artifactsDir}`}
            />
            <List
              size="small"
              dataSource={lastRun.nodeStates as Array<Record<string, unknown>>}
              renderItem={(item) => (
                <List.Item>
                  <Space direction="vertical" size={2} style={{ width: "100%" }}>
                    <Text strong>{String(item.nodeId || "")}</Text>
                    <Text type="secondary">
                      {String(item.kind || "")} · {String(item.status || "")} · attempt {String(item.attempt || 0)} · artifacts {Array.isArray(item.artifacts) ? item.artifacts.length : 0}
                    </Text>
                    {typeof item.attemptLogPath === "string" ? <Text type="secondary" copyable>{item.attemptLogPath}</Text> : null}
                  </Space>
                </List.Item>
              )}
            />
          </Space>
        </Card>
      ) : null}
    </Card>
  );

  const renderDebugTab = () => (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Card title="Debug Run" extra={
        <Space wrap>
          <Button onClick={handleRun} loading={loading} icon={<PlayCircleOutlined />}>Run Full Workflow</Button>
          <Button type="primary" ghost onClick={handleQuickDebug} loading={loading} icon={<BugOutlined />}>Quick Debug</Button>
        </Space>
      }>
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Text type="secondary">Debug inputs used by Run and Quick Debug</Text>
          <Input
            data-testid="ai-workflow-debug-audio-dir"
            addonBefore="audio_dir"
            value={debugAudioDir}
            onChange={(event) => setDebugAudioDir(event.target.value)}
            placeholder="./human-tests/audio"
          />
        </Space>
      </Card>
      <Card title="Quick Debug Trace">
        {debugSteps.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Run Quick Debug to see validate → preview → check → save → execute → logs" />
        ) : (
          <List
            size="small"
            dataSource={debugSteps}
            renderItem={(item) => (
              <List.Item>
                <Space direction="vertical" size={2} style={{ width: "100%" }}>
                  <Space>
                    <Tag color={item.status === "success" ? "green" : item.status === "error" ? "red" : "blue"}>{item.status}</Tag>
                    <Text strong>{item.title}</Text>
                  </Space>
                  <Text type="secondary" style={{ whiteSpace: "pre-wrap" }}>{item.detail}</Text>
                </Space>
              </List.Item>
            )}
          />
        )}
      </Card>
    </Space>
  );

  const renderDetailPage = () => (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Card>
        <Space style={{ width: "100%", justifyContent: "space-between" }} align="start" wrap>
          <Space direction="vertical" size={4}>
            <Button type="link" style={{ padding: 0 }} onClick={handleBackToList}>← Back to Workflow list</Button>
            <Space wrap>
              <Title level={4} style={{ margin: 0 }}>{workflowDisplayName(selectedWorkflow, selectedTemplate?.name ?? "New Workflow")}</Title>
              {selectedTemplate ? <Tag color="purple">template</Tag> : null}
              {draftWorkflowId ? <Tag>{draftWorkflowId}</Tag> : null}
            </Space>
            <Text type="secondary">
              Edit the graph or code config, review execution records, and run isolated debug executions in separate tabs.
            </Text>
          </Space>
        </Space>
      </Card>

      <Tabs
        data-testid="ai-workflow-detail-tabs"
        activeKey={detailTab}
        onChange={(key) => setDetailTab(key as DetailTab)}
        items={[
          { key: "editor", label: <span><EditOutlined /> Editor</span>, children: renderEditorTab() },
          { key: "runs", label: <span><HistoryOutlined /> Execution Records</span>, children: renderRunsTab() },
          { key: "debug", label: <span><BugOutlined /> Debug Run</span>, children: renderDebugTab() },
        ]}
      />
    </Space>
  );

  return (
    <div
      data-testid="ai-workflow-section"
      style={{
        height: "100%",
        overflow: "auto",
        padding: 16,
        background: token.colorBgLayout,
      }}
    >
      {isDetailMode ? renderDetailPage() : renderListPage()}
    </div>
  );
}
