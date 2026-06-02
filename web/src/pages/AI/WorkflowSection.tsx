import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Card, Empty, Input, List, Space, Tag, Typography, message, theme } from "antd";
import { BugOutlined, PlayCircleOutlined, ReloadOutlined, SaveOutlined } from "@ant-design/icons";
import { Background, Controls, MiniMap, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  applyAiWorkflowDraft,
  getAiWorkflowRun,
  listAiWorkflowTemplates,
  listAiWorkflows,
  previewAiWorkflow,
  runAiWorkflow,
  validateAiWorkflow,
  type WorkflowRun,
  type WorkflowPreview,
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
  return draft.match(/^\s*id:\s*([^\n]+)/m)?.[1]?.trim().replace(/^['"]|['"]$/g, "");
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

export default function WorkflowSection() {
  const { token } = theme.useToken();
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [draft, setDraft] = useState(SAMPLE_WORKFLOW);
  const [preview, setPreview] = useState<WorkflowPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastRun, setLastRun] = useState<WorkflowRun>();
  const [debugSteps, setDebugSteps] = useState<DebugStep[]>([]);
  const [debugAudioDir, setDebugAudioDir] = useState("./human-tests/audio");
  const didLoadDefaultTemplate = useRef(false);

  const draftWorkflowId = useMemo(() => parseWorkflowId(draft), [draft]);
  const flowNodes = useMemo(() => toFlowNodes(preview, token), [preview, token]);
  const flowEdges = useMemo(() => toFlowEdges(preview), [preview]);

  const refresh = useCallback(async () => {
    const [workflowResponse, templateResponse] = await Promise.all([
      listAiWorkflows(),
      listAiWorkflowTemplates(),
    ]);
    setWorkflows(workflowResponse.workflows);
    setTemplates(templateResponse.templates);
    const defaultTemplate = templateResponse.templates.find((item) => item.id === "default-asr-transcription") ?? templateResponse.templates[0];
    if (defaultTemplate && !didLoadDefaultTemplate.current) {
      const draftForPreview = defaultTemplate.draft;
      setDraft((currentDraft) => {
        didLoadDefaultTemplate.current = true;
        return currentDraft === SAMPLE_WORKFLOW && draftForPreview !== SAMPLE_WORKFLOW
          ? draftForPreview
          : currentDraft;
      });
      const nextPreview = await previewAiWorkflow(draftForPreview).catch(() => null);
      if (nextPreview) {
        setPreview(nextPreview);
      }
    }
  }, []);

  useEffect(() => {
    void refresh().catch(() => setWorkflows([]));
  }, [refresh]);

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
      await applyAiWorkflowDraft(draft, { dryRun: false });
      await refresh();
      message.success("Workflow saved in backend");
    } finally {
      setLoading(false);
    }
  }, [draft, refresh]);

  const handleRun = useCallback(async () => {
    if (!draftWorkflowId) {
      message.error("Workflow metadata.id is required before running");
      return;
    }
    setLoading(true);
    try {
      const response = await runAiWorkflow(draftWorkflowId, { audio_dir: debugAudioDir });
      const persisted = await getAiWorkflowRun(draftWorkflowId, response.run.id).catch(() => response);
      setLastRun(persisted.run);
      message.success(`Workflow executed: ${response.run.id} (${response.run.status})`);
    } finally {
      setLoading(false);
    }
  }, [debugAudioDir, draftWorkflowId]);

  const appendDebugStep = useCallback((step: DebugStep) => {
    setDebugSteps((current) => [...current, step]);
  }, []);

  const handleUseTemplate = useCallback(async (template: WorkflowTemplate) => {
    setDraft(template.draft);
    setPreview(null);
    setLastRun(undefined);
    setDebugSteps([]);
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
  }, []);

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
        await applyAiWorkflowDraft(draft, { dryRun: false });
        pushStep({
          title: "Save",
          status: "success",
          detail: `workflow=${draftWorkflowId ?? "<unknown>"}`,
        });
        await refresh();
      }

      if (draftWorkflowId && validation.valid && nextPreview.blockingErrors.length === 0) {
        const response = await runAiWorkflow(draftWorkflowId, { audio_dir: debugAudioDir });
        const persisted = await getAiWorkflowRun(draftWorkflowId, response.run.id).catch(() => response);
        setLastRun(persisted.run);
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
  }, [appendDebugStep, debugAudioDir, draft, draftWorkflowId, refresh]);

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
      <Space direction="vertical" size={16} style={{ width: "100%" }}>
        <Card>
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Title level={4} style={{ margin: 0 }}>AI Workflow</Title>
            <Paragraph type="secondary" style={{ margin: 0 }}>
              Build reviewable Workflow definitions for ASR, Runner, script, and IM notification nodes. The default draft is an editable ASR transcription workflow template that can replace the current scheduled ASR task flow after configuration.
            </Paragraph>
            <Space wrap>
              <Tag color="blue">v1alpha1</Tag>
              <Tag>script</Tag>
              <Tag>runner</Tag>
              <Tag>asr_transcription</Tag>
              <Tag>notification</Tag>
            </Space>
          </Space>
        </Card>

        <div style={{ display: "grid", gridTemplateColumns: "minmax(360px, 1fr) 360px", gap: 16 }}>
          <Card
            title="Workflow Draft"
            extra={
              <Space>
                <Button onClick={handleValidatePreview} loading={loading} icon={<ReloadOutlined />}>
                  Validate & Preview
                </Button>
                <Button onClick={handleCheckApply} loading={loading} icon={<SaveOutlined />}>
                  Check Apply
                </Button>
                <Button onClick={handleSave} loading={loading}>
                  Save
                </Button>
                <Button type="primary" onClick={handleRun} loading={loading} icon={<PlayCircleOutlined />}>
                  Run
                </Button>
                <Button type="primary" ghost onClick={handleQuickDebug} loading={loading} icon={<BugOutlined />}>
                  Quick Debug
                </Button>
              </Space>
            }
          >
            <div style={{ display: "grid", gridTemplateRows: "360px minmax(360px, auto)", gap: 12 }}>
              <div
                data-testid="ai-workflow-reactflow-preview"
                style={{
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
                    description="Validate or Quick Debug to render the Workflow graph"
                    style={{ marginTop: 120 }}
                  />
                )}
              </div>
              <Input.TextArea
                data-testid="ai-workflow-draft-editor"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                spellCheck={false}
                autoSize={{ minRows: 18, maxRows: 30 }}
                style={{ fontFamily: "var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace)" }}
              />
            </div>
          </Card>

          <Space direction="vertical" size={16} style={{ width: "100%" }}>
            <Card title="Workflow Templates">
              {templates.length === 0 ? (
                <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No templates loaded" />
              ) : (
                <List
                  size="small"
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

            <Card title="Saved Workflows" extra={<Button size="small" onClick={refresh}>Refresh</Button>}>
              {workflows.length === 0 ? (
                <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No saved workflows" />
              ) : (
                <List
                  size="small"
                  dataSource={workflows}
                  renderItem={(item) => (
                    <List.Item>
                      <Space direction="vertical" size={2}>
                        <Text strong>{item.name || item.id}</Text>
                        <Text type="secondary">{item.id} · rev {item.revision} · {item.nodeCount} nodes</Text>
                      </Space>
                    </List.Item>
                  )}
                />
              )}
            </Card>

            <Card title="Preview">
              {preview ? (
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
              ) : (
                <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Validate a draft to see preview" />
              )}
            </Card>

            <Card title="Quick Debug Trace">
              <Space direction="vertical" size={8} style={{ width: "100%", marginBottom: 12 }}>
                <Text type="secondary">Debug inputs used by Run and Quick Debug</Text>
                <Input
                  addonBefore="audio_dir"
                  value={debugAudioDir}
                  onChange={(event) => setDebugAudioDir(event.target.value)}
                  placeholder="./human-tests/audio"
                />
              </Space>
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

            {lastRun ? (
              <Card title="Last Run">
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
          </Space>
        </div>
      </Space>
    </div>
  );
}
