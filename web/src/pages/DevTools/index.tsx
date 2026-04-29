import { useCallback, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import {
  ArrowLeftOutlined,
  BranchesOutlined,
  CodeOutlined,
  DatabaseOutlined,
  GlobalOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { Alert, Button, Empty, Input, Space, Tabs, Tag, Typography, message } from "antd";
import {
  getDevtoolsSnapshot,
  listDevtoolsPages,
  openDevtoolsSession,
  sendDevtoolsCommand,
  type DebugDomNode,
  type DebugNetworkEvent,
  type DebugPage,
  type DebugSession,
  type DebugStorageSnapshot,
  type DevtoolsSnapshot,
} from "../../api/devtools";

const { Text, Title } = Typography;

export default function DevTools() {
  const [pages, setPages] = useState<DebugPage[]>([]);
  const [query, setQuery] = useState("");
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [session, setSession] = useState<DebugSession | null>(null);
  const [snapshot, setSnapshot] = useState<DevtoolsSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [snapshotLoading, setSnapshotLoading] = useState(false);
  const [consoleExpression, setConsoleExpression] = useState("document.title");
  const [consoleResult, setConsoleResult] = useState<string>("");
  const [consoleRunning, setConsoleRunning] = useState(false);
  const [selectedNodeId, setSelectedNodeId] = useState<number | null>(null);
  const [storageArea, setStorageArea] = useState("local_storage");
  const [storageKey, setStorageKey] = useState("");
  const [storageValue, setStorageValue] = useState("");
  const [storageSaving, setStorageSaving] = useState(false);

  const refreshPages = useCallback(async () => {
    const next = await listDevtoolsPages(true);
    setPages(next);
  }, []);

  const refreshSnapshot = useCallback(async (sessionId: string) => {
    setSnapshotLoading(true);
    try {
      setSnapshot(await getDevtoolsSnapshot(sessionId));
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to refresh DevTools data");
    } finally {
      setSnapshotLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshPages();
    const timer = window.setInterval(() => {
      void refreshPages();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [refreshPages]);

  useEffect(() => {
    if (!session) return;
    void refreshSnapshot(session.session_id);
    const timer = window.setInterval(() => {
      void refreshSnapshot(session.session_id);
    }, 1500);
    return () => window.clearInterval(timer);
  }, [refreshSnapshot, session]);

  const filteredPages = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return pages;
    return pages.filter((page) =>
      [page.title, page.url, page.adapter, page.state]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(needle)),
    );
  }, [pages, query]);

  const selectedPage = pages.find((page) => page.page_id === selectedPageId) ?? snapshot?.page ?? null;

  const openPage = async (page: DebugPage) => {
    setSelectedPageId(page.page_id);
    setSnapshot(null);
    setConsoleResult("");
    setSelectedNodeId(null);
    setLoading(true);
    try {
      const nextSession = await openDevtoolsSession(page.page_id);
      setSession(nextSession);
      await refreshSnapshot(nextSession.session_id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to open DevTools session");
    } finally {
      setLoading(false);
    }
  };

  const runConsoleExpression = async () => {
    if (!session) return;
    const expression = consoleExpression.trim();
    if (!expression) return;
    setConsoleRunning(true);
    setConsoleResult("");
    try {
      const response = await sendDevtoolsCommand(session.session_id, "runtime.evaluate", {
        expression,
      });
      setConsoleResult(formatValue(response.result));
      await refreshSnapshot(session.session_id);
    } catch (error) {
      setConsoleResult(error instanceof Error ? error.message : String(error));
    } finally {
      setConsoleRunning(false);
    }
  };

  const highlightDomNode = async (nodeId: number) => {
    if (!session) return;
    try {
      setSelectedNodeId(nodeId);
      await sendDevtoolsCommand(session.session_id, "dom.highlight", { node_id: nodeId });
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to highlight element");
    }
  };

  const updateStorageValue = async () => {
    if (!session) return;
    if (!storageKey.trim()) {
      message.warning("Storage key is required");
      return;
    }
    setStorageSaving(true);
    try {
      await sendDevtoolsCommand(session.session_id, "storage.set", {
        area: storageArea,
        key: storageKey,
        value: storageValue,
      });
      await refreshSnapshot(session.session_id);
      message.success("Storage updated");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to update storage");
    } finally {
      setStorageSaving(false);
    }
  };

  if (!selectedPage) {
    return (
      <div style={pageShellStyle}>
        <Space direction="vertical" size={16} style={{ width: "100%", minHeight: 0 }}>
          <div style={listHeaderStyle}>
            <Space direction="vertical" size={4} style={{ minWidth: 0 }}>
              <Title level={3} style={{ margin: 0 }}>DevTools</Title>
              <Text type="secondary">Online pages that matched a devtools:// rule</Text>
            </Space>
            <Space.Compact style={{ width: "min(520px, 100%)" }}>
              <Input.Search
                allowClear
                placeholder="Search online pages"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onSearch={() => void refreshPages()}
              />
              <Button icon={<ReloadOutlined />} onClick={() => void refreshPages()}>
                Refresh Pages
              </Button>
            </Space.Compact>
          </div>

          <div data-testid="devtools-page-list" style={cardGridStyle}>
            {filteredPages.length === 0 ? (
              <div style={emptyListStyle}>
                <Empty description="No online pages" />
              </div>
            ) : (
              filteredPages.map((page) => (
                <button
                  key={page.page_id}
                  type="button"
                  data-testid="devtools-page-card"
                  onClick={() => void openPage(page)}
                  style={pageCardStyle}
                >
                  <Space direction="vertical" size={10} style={{ width: "100%", minWidth: 0 }}>
                    <Space direction="vertical" size={2} style={{ width: "100%", minWidth: 0 }}>
                      <Text strong ellipsis style={{ maxWidth: "100%", fontSize: 16 }}>
                        {page.title || "(untitled)"}
                      </Text>
                      <Text type="secondary" ellipsis style={{ maxWidth: "100%" }}>
                        {page.url}
                      </Text>
                    </Space>
                    <Space wrap size={6}>
                      <Tag color="blue">{page.adapter}</Tag>
                      <Tag color="gold">{page.fidelity}</Tag>
                      <Tag>{page.state}</Tag>
                      <Tag>{page.mode}</Tag>
                    </Space>
                  </Space>
                </button>
              ))
            )}
          </div>
        </Space>
      </div>
    );
  }

  return (
    <div data-testid="devtools-detail" style={detailShellStyle}>
      <Space direction="vertical" size={12} style={{ width: "100%", height: "100%", minHeight: 0 }}>
        <div style={detailHeaderStyle}>
          <Button
            size="small"
            data-testid="devtools-back"
            icon={<ArrowLeftOutlined />}
            onClick={() => {
              setSelectedPageId(null);
              setSession(null);
              setSnapshot(null);
              setSelectedNodeId(null);
            }}
          >
            Back
          </Button>
          <Space size={8} style={{ minWidth: 0, flex: 1 }}>
            <Text strong ellipsis style={{ maxWidth: 220, fontSize: 16 }}>
              {selectedPage.title || "(untitled)"}
            </Text>
            <Text type="secondary" ellipsis style={{ maxWidth: "min(720px, 55vw)" }}>
              {selectedPage.url}
            </Text>
          </Space>
          <Button
            size="small"
            data-testid="devtools-refresh"
            icon={<ReloadOutlined />}
            loading={loading || snapshotLoading}
            onClick={() => {
              if (session) {
                void refreshSnapshot(session.session_id);
              } else {
                void openPage(selectedPage);
              }
            }}
          />
          <Tag>{session?.state ?? selectedPage.state}</Tag>
        </div>

        {selectedPage.status_reason ? (
          <Alert type="warning" showIcon message={selectedPage.status_reason} />
        ) : null}

        <div style={summaryStripStyle}>
          <InfoItem label="Adapter" value={selectedPage.adapter} />
          <InfoItem label="Mode" value={selectedPage.mode} />
          <InfoItem label="Rule" value={selectedPage.matched_rule?.pattern ?? "-"} />
          <InfoItem label="Traffic" value={(selectedPage.traffic_ids ?? []).slice(-3).join(", ") || "-"} />
        </div>

        <Tabs
          data-testid="devtools-custom-workspace"
          style={workspaceStyle}
          items={[
            {
              key: "elements",
              label: <TabLabel icon={<BranchesOutlined />} text="Elements" />,
              children: (
                <section data-testid="devtools-elements-panel" style={panelStyle}>
                  <DomTree
                    node={snapshot?.dom_tree ?? null}
                    html={snapshot?.dom_snapshot ?? ""}
                    selectedNodeId={selectedNodeId}
                    onHighlight={highlightDomNode}
                  />
                </section>
              ),
            },
            {
              key: "network",
              label: <TabLabel icon={<GlobalOutlined />} text="Network" />,
              children: (
                <section data-testid="devtools-network-panel" style={panelStyle}>
                  <NetworkList events={snapshot?.network ?? []} />
                </section>
              ),
            },
            {
              key: "storage",
              label: <TabLabel icon={<DatabaseOutlined />} text="Storage" />,
              children: (
                <section data-testid="devtools-storage-panel" style={panelStyle}>
                  <StorageView
                    mode={selectedPage.mode}
                    storage={snapshot?.storage ?? null}
                    area={storageArea}
                    storageKey={storageKey}
                    storageValue={storageValue}
                    saving={storageSaving}
                    onAreaChange={setStorageArea}
                    onKeyChange={setStorageKey}
                    onValueChange={setStorageValue}
                    onSave={() => void updateStorageValue()}
                  />
                </section>
              ),
            },
            {
              key: "console",
              label: <TabLabel icon={<CodeOutlined />} text="Console" />,
              children: (
                <section data-testid="devtools-console-panel" style={panelStyle}>
                  <ConsoleView
                    mode={selectedPage.mode}
                    messages={snapshot?.console ?? []}
                    expression={consoleExpression}
                    result={consoleResult}
                    running={consoleRunning}
                    onExpressionChange={setConsoleExpression}
                    onRun={() => void runConsoleExpression()}
                  />
                </section>
              ),
            },
          ]}
        />
      </Space>
    </div>
  );
}

function TabLabel({ icon, text }: { icon: ReactNode; text: string }) {
  return (
    <Space size={6}>
      {icon}
      <span>{text}</span>
    </Space>
  );
}

function InfoItem({ label, value }: { label: string; value: string }) {
  return (
    <div style={infoItemStyle}>
      <Text type="secondary">{label}</Text>
      <Text ellipsis style={{ maxWidth: "100%" }}>{value}</Text>
    </div>
  );
}

function DomTree({
  node,
  html,
  selectedNodeId,
  onHighlight,
}: {
  node: DebugDomNode | null;
  html: string;
  selectedNodeId: number | null;
  onHighlight: (nodeId: number) => void;
}) {
  if (node) {
    return <div style={treeStyle}>{renderDomNode(node, 0, selectedNodeId, onHighlight)}</div>;
  }
  if (html) {
    return <pre style={codeBlockStyle}>{html}</pre>;
  }
  return <Empty description="No DOM snapshot yet" />;
}

function renderDomNode(
  node: DebugDomNode,
  depth: number,
  selectedNodeId: number | null,
  onHighlight: (nodeId: number) => void,
): React.ReactNode {
  const children = Array.isArray(node.children) ? node.children : [];
  const label = domNodeLabel(node);
  const nodeId = node.nodeId;
  const selected = nodeId != null && nodeId === selectedNodeId;
  return (
    <div key={`${label}-${depth}-${children.length}`} style={{ marginLeft: depth === 0 ? 0 : 14 }}>
      <div style={domLineStyle}>
        {nodeId != null ? (
          <button
            type="button"
            data-testid="devtools-dom-node"
            onClick={() => onHighlight(nodeId)}
            style={{
              ...domNodeButtonStyle,
              background: selected ? "#e6f4ff" : "transparent",
              borderColor: selected ? "#91caff" : "transparent",
            }}
          >
            <Text code>{label}</Text>
          </button>
        ) : (
          <Text code>{label}</Text>
        )}
      </div>
      {children.slice(0, 250).map((child, index) => (
        <div key={`${label}-${index}`}>
          {renderDomNode(child, depth + 1, selectedNodeId, onHighlight)}
        </div>
      ))}
      {children.length > 250 ? <Text type="secondary">... {children.length - 250} more nodes</Text> : null}
    </div>
  );
}

function domNodeLabel(node: DebugDomNode): string {
  const name = String(node.nodeName ?? node["name"] ?? "node").toLowerCase();
  const attrs = formatAttributes(node.attributes);
  const value = typeof node.nodeValue === "string" && node.nodeValue.trim()
    ? ` ${node.nodeValue.trim().slice(0, 80)}`
    : "";
  return `<${name}${attrs}>${value}`;
}

function formatAttributes(attributes: DebugDomNode["attributes"]): string {
  if (!attributes) return "";
  if (Array.isArray(attributes)) {
    const pairs = [];
    for (let index = 0; index < attributes.length; index += 2) {
      pairs.push(`${attributes[index]}="${attributes[index + 1] ?? ""}"`);
    }
    return pairs.length ? ` ${pairs.slice(0, 8).join(" ")}` : "";
  }
  return Object.entries(attributes)
    .slice(0, 8)
    .map(([key, value]) => `${key}="${value}"`)
    .join(" ")
    .replace(/^(.+)/, " $1");
}

function NetworkList({ events }: { events: DebugNetworkEvent[] }) {
  if (!events.length) return <Empty description="No network events yet" />;
  return (
    <div style={tableStyle}>
      <div style={tableHeaderStyle}>
        <Text strong>Method</Text>
        <Text strong>Status</Text>
        <Text strong>Type</Text>
        <Text strong>URL</Text>
      </div>
      {events.slice().reverse().map((event, index) => (
        <div key={`${event.url}-${event.at_ms}-${index}`} style={tableRowStyle}>
          <Text code>{event.method || "GET"}</Text>
          <Text>{event.status ?? "-"}</Text>
          <Text>{event.resource_type || "resource"}</Text>
          <Text ellipsis title={event.url}>{event.url}</Text>
        </div>
      ))}
    </div>
  );
}

function StorageView({
  mode,
  storage,
  area,
  storageKey,
  storageValue,
  saving,
  onAreaChange,
  onKeyChange,
  onValueChange,
  onSave,
}: {
  mode: DebugPage["mode"];
  storage: DebugStorageSnapshot | null;
  area: string;
  storageKey: string;
  storageValue: string;
  saving: boolean;
  onAreaChange: (value: string) => void;
  onKeyChange: (value: string) => void;
  onValueChange: (value: string) => void;
  onSave: () => void;
}) {
  if (!storage) return <Empty description="No storage snapshot yet" />;
  return (
    <Space direction="vertical" size={14} style={{ width: "100%" }}>
      <div style={storageEditorStyle}>
        {mode !== "control" ? (
          <Alert type="info" showIcon message="Storage editing requires mode=control." />
        ) : null}
        <Space.Compact style={{ width: "100%" }}>
          <select
            data-testid="devtools-storage-area"
            value={area}
            onChange={(event) => onAreaChange(event.target.value)}
            style={selectStyle}
            disabled={mode !== "control"}
          >
            <option value="cookie">Cookie</option>
            <option value="local_storage">Local Storage</option>
            <option value="session_storage">Session Storage</option>
          </select>
          <Input
            data-testid="devtools-storage-key"
            value={storageKey}
            onChange={(event) => onKeyChange(event.target.value)}
            placeholder="Key"
            disabled={mode !== "control"}
          />
          <Input
            data-testid="devtools-storage-value"
            value={storageValue}
            onChange={(event) => onValueChange(event.target.value)}
            placeholder="Value"
            disabled={mode !== "control"}
          />
          <Button
            data-testid="devtools-storage-save"
            type="primary"
            loading={saving}
            disabled={mode !== "control"}
            onClick={onSave}
          >
            Save
          </Button>
        </Space.Compact>
      </div>
      <KeyValueList title="Cookies" rows={storage.cookies} />
      <KeyValueList title="Local Storage" rows={storage.local_storage} />
      <KeyValueList title="Session Storage" rows={storage.session_storage} />
    </Space>
  );
}

function KeyValueList({ title, rows }: { title: string; rows: Array<[string, string]> }) {
  return (
    <div>
      <Title level={5} style={{ margin: "0 0 8px" }}>{title}</Title>
      {rows.length ? (
        <div style={kvTableStyle}>
          {rows.map(([key, value]) => (
            <div key={`${title}-${key}`} style={kvRowStyle}>
              <Text code ellipsis title={key}>{key}</Text>
              <Text ellipsis title={value}>{value}</Text>
            </div>
          ))}
        </div>
      ) : (
        <Text type="secondary">Empty</Text>
      )}
    </div>
  );
}

function ConsoleView({
  mode,
  messages,
  expression,
  result,
  running,
  onExpressionChange,
  onRun,
}: {
  mode: DebugPage["mode"];
  messages: DevtoolsSnapshot["console"];
  expression: string;
  result: string;
  running: boolean;
  onExpressionChange: (value: string) => void;
  onRun: () => void;
}) {
  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {mode !== "control" ? (
        <Alert type="info" showIcon message="This page is read-only. Console evaluation requires mode=control." />
      ) : null}
      <Space.Compact style={{ width: "100%" }}>
        <Input
          data-testid="devtools-console-input"
          value={expression}
          onChange={(event) => onExpressionChange(event.target.value)}
          onPressEnter={onRun}
          placeholder="Run JavaScript in the remote page"
        />
        <Button
          data-testid="devtools-console-run"
          type="primary"
          icon={<PlayCircleOutlined />}
          loading={running}
          disabled={mode !== "control"}
          onClick={onRun}
        >
          Run
        </Button>
      </Space.Compact>
      {result ? <pre data-testid="devtools-console-result" style={codeBlockStyle}>{result}</pre> : null}
      <div style={consoleLogStyle}>
        {messages.length ? (
          messages.slice().reverse().map((entry, index) => (
            <div key={`${entry.at_ms}-${index}`} style={consoleRowStyle}>
              <Tag>{entry.level}</Tag>
              <Text>{entry.text}</Text>
            </div>
          ))
        ) : (
          <Empty description="No console messages yet" />
        )}
      </div>
    </Space>
  );
}

function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  return JSON.stringify(value, null, 2);
}

const pageShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: 20,
  overflow: "auto",
  background: "#f7f9fc",
};

const detailShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: "10px 20px 12px",
  overflow: "auto",
  background: "#f7f9fc",
};

const listHeaderStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "flex-start",
  gap: 16,
  flexWrap: "wrap",
};

const cardGridStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))",
  gap: 14,
};

const pageCardStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  display: "block",
  width: "100%",
  minHeight: 136,
  padding: 16,
  border: "1px solid #d9e2ef",
  borderRadius: 8,
  background: "#fff",
  color: "inherit",
  textAlign: "left",
  cursor: "pointer",
  boxShadow: "0 1px 2px rgba(15, 23, 42, 0.04)",
};

const emptyListStyle: CSSProperties = {
  gridColumn: "1 / -1",
  minHeight: 320,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const detailHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 12,
  minWidth: 0,
  minHeight: 28,
};

const summaryStripStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))",
  gap: 10,
};

const infoItemStyle: CSSProperties = {
  minWidth: 0,
  padding: "8px 10px",
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  background: "#fff",
  display: "flex",
  flexDirection: "column",
  gap: 2,
};

const workspaceStyle: CSSProperties = {
  flex: 1,
  minHeight: 0,
  background: "#fff",
  border: "1px solid #d9e2ef",
  borderRadius: 8,
  padding: "0 12px 12px",
};

const panelStyle: CSSProperties = {
  minHeight: 420,
  maxHeight: "calc(100vh - 230px)",
  overflow: "auto",
};

const treeStyle: CSSProperties = {
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 12,
  lineHeight: 1.6,
};

const domLineStyle: CSSProperties = {
  minHeight: 22,
  display: "flex",
  alignItems: "center",
};

const domNodeButtonStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  width: "100%",
  padding: "1px 4px",
  border: "1px solid transparent",
  borderRadius: 4,
  color: "inherit",
  textAlign: "left",
  cursor: "pointer",
};

const codeBlockStyle: CSSProperties = {
  margin: 0,
  padding: 12,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  background: "#f8fafc",
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

const tableStyle: CSSProperties = {
  display: "grid",
  gap: 0,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
};

const tableHeaderStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "96px 80px 130px minmax(260px, 1fr)",
  gap: 8,
  padding: "8px 10px",
  background: "#eef4fb",
};

const tableRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "96px 80px 130px minmax(260px, 1fr)",
  gap: 8,
  padding: "8px 10px",
  borderTop: "1px solid #e7edf5",
  minWidth: 560,
};

const kvTableStyle: CSSProperties = {
  display: "grid",
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
};

const storageEditorStyle: CSSProperties = {
  display: "grid",
  gap: 8,
  padding: 10,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  background: "#f8fafc",
};

const selectStyle: CSSProperties = {
  width: 150,
  minWidth: 150,
  border: "1px solid #d9d9d9",
  borderRadius: "6px 0 0 6px",
  padding: "0 10px",
  background: "#fff",
};

const kvRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(160px, 260px) minmax(240px, 1fr)",
  gap: 10,
  padding: "8px 10px",
  borderTop: "1px solid #e7edf5",
  minWidth: 420,
};

const consoleLogStyle: CSSProperties = {
  display: "grid",
  gap: 8,
};

const consoleRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "flex-start",
  gap: 8,
  padding: "8px 10px",
  border: "1px solid #e7edf5",
  borderRadius: 6,
};
