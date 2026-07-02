import { useMemo, useRef, useState, type ChangeEvent, type CSSProperties } from "react";
import {
  Alert,
  Button,
  Col,
  Descriptions,
  Empty,
  Input,
  Row,
  Space,
  Statistic,
  Table,
  Tabs,
  Tag,
  Timeline,
  Typography,
  message,
  theme,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import { CopyOutlined, ImportOutlined, PlayCircleOutlined } from "@ant-design/icons";
import {
  explainSurgeProfile,
  importSurgeProfile,
  type CompatibilityItem,
  type ExplainReport,
  type ProfileResource,
  type RuntimeRule,
  type SurgeImportResponse,
} from "../../api/profile";
import { normalizeApiErrorMessage } from "../../api/client";

const SAMPLE_PROFILE = `[General]
dns-server = 8.8.8.8

[Host]
api.example.com = 203.0.113.10

[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT

[MITM]
hostname = %APPEND% *.example.com, -private.example.com

[URL Rewrite]
^https://rewrite\\.example/path https://target.example/path 302

[Rule]
DOMAIN,api.example.com,DIRECT
DOMAIN-SUFFIX,example.com,Proxy
FINAL,DIRECT
`;

const SUPPORT_LABELS: Record<string, { label: string; color: string }> = {
  FullySupported: { label: "Fully supported", color: "green" },
  TranslatedWithBehaviorNote: { label: "Behavior note", color: "gold" },
  NeedsManualReview: { label: "Manual review", color: "orange" },
  NotSupportedYet: { label: "Not supported", color: "red" },
};

function supportTag(level: string) {
  const meta = SUPPORT_LABELS[level] ?? { label: level, color: "default" };
  return <Tag color={meta.color}>{meta.label}</Tag>;
}

function lineLabel(line?: number | null) {
  return line ? `line ${line}` : "runtime";
}

export default function Profile() {
  const { token } = theme.useToken();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [sourceLabel, setSourceLabel] = useState("surge.conf");
  const [content, setContent] = useState(SAMPLE_PROFILE);
  const [explainUrl, setExplainUrl] = useState("https://api.example.com/path");
  const [importResult, setImportResult] = useState<SurgeImportResponse | null>(null);
  const [explainReport, setExplainReport] = useState<ExplainReport | null>(null);
  const [loadingImport, setLoadingImport] = useState(false);
  const [loadingExplain, setLoadingExplain] = useState(false);

  const styles: Record<string, CSSProperties> = {
    page: {
      minHeight: "100%",
      padding: 20,
      display: "flex",
      flexDirection: "column",
      gap: 16,
    },
    header: {
      display: "flex",
      justifyContent: "space-between",
      gap: 16,
      alignItems: "center",
    },
    titleBlock: {
      minWidth: 0,
    },
    surface: {
      border: `1px solid ${token.colorBorderSecondary}`,
      borderRadius: 8,
      background: token.colorBgContainer,
      padding: 16,
    },
    editor: {
      fontFamily:
        "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace",
      fontSize: 12,
      lineHeight: 1.55,
    },
    table: {
      border: `1px solid ${token.colorBorderSecondary}`,
      borderRadius: 8,
      overflow: "hidden",
    },
    preview: {
      margin: 0,
      minHeight: 360,
      maxHeight: 540,
      overflow: "auto",
      padding: 12,
      border: `1px solid ${token.colorBorderSecondary}`,
      borderRadius: 8,
      background: token.colorFillQuaternary,
      whiteSpace: "pre-wrap",
      wordBreak: "break-word",
      fontSize: 12,
      lineHeight: 1.55,
    },
  };

  const summary = importResult?.compatibility.summary;
  const activeExplain = explainReport ?? importResult?.explain ?? null;

  const compatibilityColumns: ColumnsType<CompatibilityItem> = useMemo(
    () => [
      {
        title: "Status",
        dataIndex: "level",
        width: 150,
        render: (level: string) => supportTag(level),
      },
      { title: "Line", dataIndex: "line", width: 82 },
      { title: "Section", dataIndex: "section", width: 130 },
      { title: "Capability", dataIndex: "capability", width: 170 },
      {
        title: "Decision",
        dataIndex: "message",
        render: (messageText: string, item) => (
          <Space direction="vertical" size={2}>
            <span>{messageText}</span>
            {item.suggestion ? (
              <Typography.Text type="secondary">{item.suggestion}</Typography.Text>
            ) : null}
          </Space>
        ),
      },
    ],
    [],
  );

  const resourceColumns: ColumnsType<ProfileResource> = useMemo(
    () => [
      { title: "Kind", dataIndex: "kind", width: 140 },
      {
        title: "Reference",
        dataIndex: "reference",
        ellipsis: true,
      },
      { title: "Status", dataIndex: "status", width: 110 },
      { title: "Items", dataIndex: "item_count", width: 86 },
      {
        title: "Cache",
        width: 120,
        render: (_, resource) => (resource.loaded_from_cache ? "cache-hit" : "fresh"),
      },
    ],
    [],
  );

  const ruleColumns: ColumnsType<RuntimeRule> = useMemo(
    () => [
      { title: "Line", width: 82, render: (_, rule) => rule.source.line },
      { title: "Type", dataIndex: "rule_type", width: 140 },
      {
        title: "Value",
        render: (_, rule) => rule.value || "<none>",
        ellipsis: true,
      },
      { title: "Policy", dataIndex: "policy", width: 150 },
      { title: "Origin", dataIndex: "origin", width: 150 },
    ],
    [],
  );

  const runImport = async () => {
    if (!content.trim()) {
      message.warning("Paste or load a Surge profile first.");
      return;
    }
    setLoadingImport(true);
    try {
      const response = await importSurgeProfile({
        content,
        source_label: sourceLabel,
        explain_url: explainUrl.trim() || undefined,
      });
      setImportResult(response);
      setExplainReport(response.explain ?? null);
      message.success("Surge profile analyzed.");
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Failed to analyze profile"));
    } finally {
      setLoadingImport(false);
    }
  };

  const runExplain = async () => {
    if (!content.trim() || !explainUrl.trim()) {
      message.warning("Profile content and URL are required.");
      return;
    }
    setLoadingExplain(true);
    try {
      const response = await explainSurgeProfile({
        content,
        url: explainUrl,
        source_label: sourceLabel,
      });
      setExplainReport(response.report);
      message.success("Decision timeline refreshed.");
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Failed to explain request"));
    } finally {
      setLoadingExplain(false);
    }
  };

  const loadFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (!file) {
      return;
    }
    setSourceLabel(file.name);
    setContent(await file.text());
    event.target.value = "";
  };

  const copyPreview = async () => {
    const preview = importResult?.conversion_preview.content;
    if (!preview) {
      return;
    }
    await navigator.clipboard.writeText(preview);
    message.success("Preview copied.");
  };

  return (
    <div style={styles.page}>
      <div style={styles.header}>
        <div style={styles.titleBlock}>
          <Typography.Title level={3} style={{ margin: 0 }}>
            Profile
          </Typography.Title>
          <Typography.Text type="secondary">
            Bring an existing Surge profile into a dry-run workbench before activating anything.
          </Typography.Text>
        </div>
        <Space wrap>
          <input
            ref={fileInputRef}
            type="file"
            accept=".conf,.dconf,.sgmodule,text/plain"
            style={{ display: "none" }}
            onChange={loadFile}
          />
          <Button icon={<ImportOutlined />} onClick={() => fileInputRef.current?.click()}>
            Load
          </Button>
          <Button type="primary" icon={<PlayCircleOutlined />} loading={loadingImport} onClick={runImport}>
            Analyze
          </Button>
        </Space>
      </div>

      <Row gutter={[16, 16]} align="stretch">
        <Col xs={24} xl={10}>
          <div style={{ ...styles.surface, height: "100%" }}>
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Input
                value={sourceLabel}
                onChange={(event) => setSourceLabel(event.target.value)}
                placeholder="profile source label"
              />
              <Input.Search
                value={explainUrl}
                onChange={(event) => setExplainUrl(event.target.value)}
                onSearch={runExplain}
                enterButton="Explain"
                loading={loadingExplain}
                placeholder="https://example.com/path"
              />
              <Input.TextArea
                value={content}
                onChange={(event) => setContent(event.target.value)}
                autoSize={{ minRows: 24, maxRows: 38 }}
                style={styles.editor}
                spellCheck={false}
              />
            </Space>
          </div>
        </Col>
        <Col xs={24} xl={14}>
          <Space direction="vertical" size={16} style={{ width: "100%" }}>
            <div style={styles.surface}>
              {summary ? (
                <Row gutter={[12, 12]}>
                  <Col xs={12} lg={6}>
                    <Statistic title="Fully Supported" value={summary.fully_supported} />
                  </Col>
                  <Col xs={12} lg={6}>
                    <Statistic title="Behavior Notes" value={summary.translated_with_behavior_note} />
                  </Col>
                  <Col xs={12} lg={6}>
                    <Statistic title="Manual Review" value={summary.needs_manual_review} />
                  </Col>
                  <Col xs={12} lg={6}>
                    <Statistic title="Not Supported" value={summary.not_supported_yet} />
                  </Col>
                </Row>
              ) : (
                <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Analyze a profile to see compatibility, runtime plan, and conversion preview." />
              )}
            </div>

            <Tabs
              items={[
                {
                  key: "compatibility",
                  label: "Compatibility",
                  children: importResult ? (
                    <Table
                      rowKey={(item) => `${item.line}-${item.section}-${item.capability}`}
                      columns={compatibilityColumns}
                      dataSource={importResult.compatibility.items}
                      pagination={{ pageSize: 8, hideOnSinglePage: true }}
                      size="small"
                      style={styles.table}
                    />
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
                  ),
                },
                {
                  key: "runtime",
                  label: "Runtime Plan",
                  children: importResult ? (
                    <Space direction="vertical" size={12} style={{ width: "100%" }}>
                      <Descriptions size="small" bordered column={{ xs: 1, md: 3 }}>
                        <Descriptions.Item label="Mode">{importResult.runtime_plan.mode}</Descriptions.Item>
                        <Descriptions.Item label="Rules">{importResult.runtime_plan.rules.length}</Descriptions.Item>
                        <Descriptions.Item label="Policy Groups">{importResult.runtime_plan.policy_groups.length}</Descriptions.Item>
                        <Descriptions.Item label="Proxies">{importResult.runtime_plan.proxies.length}</Descriptions.Item>
                        <Descriptions.Item label="DNS">{importResult.runtime_plan.dns.length}</Descriptions.Item>
                        <Descriptions.Item label="HTTP Pipeline">{importResult.runtime_plan.http_pipeline.length}</Descriptions.Item>
                      </Descriptions>
                      <Table
                        rowKey={(rule) => `${rule.source.line}-${rule.rule_type}-${rule.origin}`}
                        columns={ruleColumns}
                        dataSource={importResult.runtime_plan.rules}
                        pagination={{ pageSize: 8, hideOnSinglePage: true }}
                        size="small"
                        style={styles.table}
                      />
                    </Space>
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
                  ),
                },
                {
                  key: "explain",
                  label: "Explain",
                  children: activeExplain ? (
                    <Space direction="vertical" size={12} style={{ width: "100%" }}>
                      <Alert
                        showIcon
                        type="info"
                        message={
                          activeExplain.target_policy
                            ? `Matched ${activeExplain.matched_rule?.rule_type ?? "rule"} -> ${activeExplain.target_policy}`
                            : "No rule matched this request"
                        }
                        description={activeExplain.policy_decision?.chain.join(" -> ") || activeExplain.mitm_decision.reason}
                      />
                      <Timeline
                        items={activeExplain.timeline.map((step) => ({
                          children: (
                            <Space direction="vertical" size={0}>
                              <Typography.Text strong>{`${step.stage} · ${lineLabel(step.line)}`}</Typography.Text>
                              <Typography.Text>{step.message}</Typography.Text>
                            </Space>
                          ),
                        }))}
                      />
                    </Space>
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
                  ),
                },
                {
                  key: "resources",
                  label: "Resources",
                  children: importResult ? (
                    <Table
                      rowKey={(resource) => `${resource.kind}-${resource.source_line}-${resource.reference}`}
                      columns={resourceColumns}
                      dataSource={importResult.resources}
                      pagination={{ pageSize: 8, hideOnSinglePage: true }}
                      size="small"
                      style={styles.table}
                    />
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
                  ),
                },
                {
                  key: "preview",
                  label: "Native Preview",
                  children: importResult ? (
                    <Space direction="vertical" size={8} style={{ width: "100%" }}>
                      <Button icon={<CopyOutlined />} onClick={copyPreview}>
                        Copy
                      </Button>
                      <pre style={styles.preview}>{importResult.conversion_preview.content}</pre>
                    </Space>
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
                  ),
                },
              ]}
            />
          </Space>
        </Col>
      </Row>
    </div>
  );
}
