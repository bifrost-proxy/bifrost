import { useCallback, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import {
  ArrowLeftOutlined,
  BranchesOutlined,
  CaretDownOutlined,
  CaretRightOutlined,
  CodeOutlined,
  DatabaseOutlined,
  EditOutlined,
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
  const [expandedDomKeys, setExpandedDomKeys] = useState<Set<string>>(new Set());
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
  const selectedNode = useMemo(() => {
    const tree = snapshot?.dom_tree ?? null;
    if (!tree || selectedNodeId == null) return null;
    return findDomNode(tree, selectedNodeId);
  }, [selectedNodeId, snapshot?.dom_tree]);

  useEffect(() => {
    const tree = snapshot?.dom_tree ?? null;
    if (!tree) return;
    setExpandedDomKeys((previous) => {
      if (previous.size) return previous;
      return collectDefaultExpandedDomKeys(tree);
    });
  }, [snapshot?.dom_tree]);

  const openPage = async (page: DebugPage) => {
    setSelectedPageId(page.page_id);
    setSnapshot(null);
    setConsoleResult("");
    setSelectedNodeId(null);
    setExpandedDomKeys(new Set());
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
              setExpandedDomKeys(new Set());
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
                    selectedNode={selectedNode}
                    expandedKeys={expandedDomKeys}
                    onHighlight={highlightDomNode}
                    onToggle={(key) => {
                      setExpandedDomKeys((previous) => {
                        const next = new Set(previous);
                        if (next.has(key)) {
                          next.delete(key);
                        } else {
                          next.add(key);
                        }
                        return next;
                      });
                    }}
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
                    onStartEdit={(nextArea, key, value) => {
                      setStorageArea(nextArea);
                      setStorageKey(key);
                      setStorageValue(value);
                    }}
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
  selectedNode,
  expandedKeys,
  onHighlight,
  onToggle,
}: {
  node: DebugDomNode | null;
  html: string;
  selectedNodeId: number | null;
  selectedNode: DebugDomNode | null;
  expandedKeys: Set<string>;
  onHighlight: (nodeId: number) => void;
  onToggle: (key: string) => void;
}) {
  if (node) {
    return (
      <div style={elementsSplitStyle}>
        <div data-testid="devtools-elements-tree" style={treePaneStyle}>
          {renderDomNode(node, "0", selectedNodeId, expandedKeys, onHighlight, onToggle)}
        </div>
        <ElementInspector node={selectedNode} />
      </div>
    );
  }
  if (html) {
    return <pre style={codeBlockStyle}>{html}</pre>;
  }
  return <Empty description="No DOM snapshot yet" />;
}

function renderDomNode(
  node: DebugDomNode,
  path: string,
  selectedNodeId: number | null,
  expandedKeys: Set<string>,
  onHighlight: (nodeId: number) => void,
  onToggle: (key: string) => void,
): React.ReactNode {
  const children = Array.isArray(node.children) ? node.children : [];
  const key = domNodeKey(node, path);
  const nodeId = node.nodeId;
  const selected = nodeId != null && nodeId === selectedNodeId;
  const hasChildren = children.length > 0 && !isVoidDomNode(node);
  const expanded = !hasChildren || expandedKeys.has(key);
  const isElement = node.nodeType === 1;
  const isText = node.nodeType === 3;
  return (
    <div key={key} style={domBranchStyle}>
      <div
        style={{
          ...domLineStyle,
          background: selected ? "#dbeafe" : "transparent",
        }}
      >
        <button
          type="button"
          data-testid="devtools-dom-disclosure"
          aria-label={expanded ? "Collapse node" : "Expand node"}
          disabled={!hasChildren}
          onClick={(event) => {
            event.stopPropagation();
            if (hasChildren) onToggle(key);
          }}
          style={{
            ...domDisclosureStyle,
            visibility: hasChildren ? "visible" : "hidden",
          }}
        >
          {expanded ? <CaretDownOutlined /> : <CaretRightOutlined />}
        </button>
        {nodeId != null ? (
          <button
            type="button"
            data-testid="devtools-dom-node"
            onClick={() => {
              if (hasChildren) onToggle(key);
              onHighlight(nodeId);
            }}
            style={{
              ...domNodeButtonStyle,
              borderColor: selected ? "#60a5fa" : "transparent",
            }}
          >
            <DomNodePreview node={node} expanded={expanded} />
          </button>
        ) : (
          <span style={domNodeStaticStyle}>
            {isElement ? <DomNodePreview node={node} expanded={expanded} /> : null}
            {isText ? <span style={domTextNodeStyle}>{formatTextNode(node.nodeValue)}</span> : null}
            {!isElement && !isText ? <span style={domTextNodeStyle}>{domNodeDisplayName(node)}</span> : null}
          </span>
        )}
      </div>
      {hasChildren && expanded ? (
        <div style={domChildrenStyle}>
          {children.slice(0, 250).map((child, index) =>
            renderDomNode(child, `${path}.${index}`, selectedNodeId, expandedKeys, onHighlight, onToggle),
          )}
          {children.length > 250 ? <Text type="secondary">... {children.length - 250} more nodes</Text> : null}
          {isElement ? (
            <div style={domClosingTagStyle}>
              <span>&lt;/</span>
              <span style={domTagStyle}>{domNodeDisplayName(node)}</span>
              <span>&gt;</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function DomNodePreview({ node, expanded }: { node: DebugDomNode; expanded: boolean }) {
  const name = domNodeDisplayName(node);
  const attrs = domAttributes(node.attributes);
  const children = Array.isArray(node.children) ? node.children : [];
  const showEllipsis = children.length > 0 && !expanded && !isVoidDomNode(node);
  if (node.nodeType !== 1) {
    return <span style={domTextNodeStyle}>{formatTextNode(node.nodeValue)}</span>;
  }
  return (
    <span style={domPreviewStyle}>
      <span>&lt;</span>
      <span style={domTagStyle}>{name}</span>
      {attrs.slice(0, 8).map(([key, value]) => (
        <span key={`${key}-${value}`}>
          {" "}
          <span style={domAttrNameStyle}>{key}</span>
          {value !== "" ? (
            <>
              <span>=&quot;</span>
              <span style={domAttrValueStyle}>{value}</span>
              <span>&quot;</span>
            </>
          ) : null}
        </span>
      ))}
      {attrs.length > 8 ? <span style={domMutedStyle}> ...</span> : null}
      <span>{isVoidDomNode(node) ? " /" : ""}&gt;</span>
      {showEllipsis ? <span style={domMutedStyle}>...</span> : null}
      {!expanded && !isVoidDomNode(node) && children.length === 0 ? (
        <>
          <span>&lt;/</span>
          <span style={domTagStyle}>{name}</span>
          <span>&gt;</span>
        </>
      ) : null}
    </span>
  );
}

function ElementInspector({ node }: { node: DebugDomNode | null }) {
  if (!node) {
    return (
      <aside data-testid="devtools-elements-sidebar" style={inspectorPaneStyle}>
        <Empty description="Select an element" />
      </aside>
    );
  }
  const attrs = domAttributes(node.attributes);
  return (
    <aside data-testid="devtools-elements-sidebar" style={inspectorPaneStyle}>
      <Space direction="vertical" size={12} style={{ width: "100%" }}>
        <div>
          <Text type="secondary">Selected</Text>
          <div style={inspectorNodeTitleStyle}>
            <span style={domTagStyle}>{domNodeDisplayName(node)}</span>
            {node.nodeId != null ? <Text type="secondary">#{node.nodeId}</Text> : null}
          </div>
        </div>
        <div>
          <Text strong>Attributes</Text>
          {attrs.length ? (
            <div style={attributeGridStyle}>
              {attrs.map(([key, value]) => (
                <div key={key} style={attributeRowStyle}>
                  <Text code ellipsis title={key}>{key}</Text>
                  <Text ellipsis title={value}>{value || "true"}</Text>
                </div>
              ))}
            </div>
          ) : (
            <Text type="secondary" style={{ display: "block", marginTop: 8 }}>No attributes</Text>
          )}
        </div>
        {typeof node.nodeValue === "string" && node.nodeValue.trim() ? (
          <div>
            <Text strong>Text</Text>
            <pre style={{ ...codeBlockStyle, marginTop: 8 }}>{node.nodeValue.trim()}</pre>
          </div>
        ) : null}
      </Space>
    </aside>
  );
}

function domAttributes(attributes: DebugDomNode["attributes"]): Array<[string, string]> {
  if (!attributes) return [];
  if (Array.isArray(attributes)) {
    const pairs = [];
    for (let index = 0; index < attributes.length; index += 2) {
      pairs.push([String(attributes[index]), String(attributes[index + 1] ?? "")] as [string, string]);
    }
    return pairs;
  }
  return Object.entries(attributes)
    .map(([key, value]) => [key, String(value)] as [string, string]);
}

function domNodeDisplayName(node: DebugDomNode): string {
  return String(node.nodeName ?? node["name"] ?? "node").toLowerCase();
}

function domNodeKey(node: DebugDomNode, path: string): string {
  return node.nodeId != null ? `node:${node.nodeId}` : `path:${path}`;
}

function isVoidDomNode(node: DebugDomNode): boolean {
  return ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"].includes(domNodeDisplayName(node));
}

function formatTextNode(value: unknown): string {
  const text = typeof value === "string" ? value : "";
  const trimmed = text.replace(/\s+/g, " ").trim();
  return trimmed ? `"${trimmed.slice(0, 160)}"` : "";
}

function findDomNode(node: DebugDomNode, nodeId: number): DebugDomNode | null {
  if (node.nodeId === nodeId) return node;
  for (const child of node.children ?? []) {
    const found = findDomNode(child, nodeId);
    if (found) return found;
  }
  return null;
}

function collectDefaultExpandedDomKeys(root: DebugDomNode): Set<string> {
  const keys = new Set<string>();
  const walk = (node: DebugDomNode, path: string, depth: number) => {
    if (depth > 2) return;
    if ((node.children ?? []).length > 0 && !isVoidDomNode(node)) {
      keys.add(domNodeKey(node, path));
    }
    (node.children ?? []).slice(0, 12).forEach((child, index) => walk(child, `${path}.${index}`, depth + 1));
  };
  walk(root, "0", 0);
  return keys;
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
  onStartEdit,
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
  onStartEdit: (area: string, key: string, value: string) => void;
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
      <KeyValueList title="Cookies" area="cookie" rows={storage.cookies} onStartEdit={onStartEdit} />
      <KeyValueList title="Local Storage" area="local_storage" rows={storage.local_storage} onStartEdit={onStartEdit} />
      <KeyValueList title="Session Storage" area="session_storage" rows={storage.session_storage} onStartEdit={onStartEdit} />
    </Space>
  );
}

function KeyValueList({
  title,
  area,
  rows,
  onStartEdit,
}: {
  title: string;
  area: string;
  rows: Array<[string, string]>;
  onStartEdit: (area: string, key: string, value: string) => void;
}) {
  return (
    <div>
      <Title level={5} style={{ margin: "0 0 8px" }}>{title}</Title>
      {rows.length ? (
        <div style={kvTableStyle}>
          {rows.map(([key, value]) => (
            <div key={`${title}-${key}`} style={kvRowStyle}>
              <Text code ellipsis title={key}>{key}</Text>
              <Text ellipsis title={value}>{value}</Text>
              <Button
                size="small"
                type="text"
                icon={<EditOutlined />}
                aria-label={`Edit ${key}`}
                onClick={() => onStartEdit(area, key, value)}
              />
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

const elementsSplitStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(460px, 1fr) minmax(240px, 340px)",
  minWidth: 760,
  minHeight: 420,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
};

const treePaneStyle: CSSProperties = {
  minWidth: 0,
  overflow: "auto",
  padding: "8px 0 8px 8px",
  background: "#fff",
  borderRight: "1px solid #e7edf5",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 12,
  lineHeight: 1.55,
};

const domLineStyle: CSSProperties = {
  minHeight: 21,
  display: "flex",
  alignItems: "center",
  minWidth: "max-content",
  paddingRight: 8,
};

const domBranchStyle: CSSProperties = {
  minWidth: "max-content",
};

const domChildrenStyle: CSSProperties = {
  marginLeft: 14,
};

const domDisclosureStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  width: 16,
  minWidth: 16,
  height: 18,
  padding: 0,
  border: 0,
  background: "transparent",
  color: "#6b7280",
  cursor: "pointer",
  fontSize: 10,
};

const domNodeButtonStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  display: "inline-flex",
  alignItems: "center",
  minHeight: 20,
  padding: "1px 4px",
  border: "1px solid transparent",
  borderRadius: 3,
  background: "transparent",
  color: "inherit",
  textAlign: "left",
  cursor: "pointer",
};

const domNodeStaticStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  minHeight: 20,
  padding: "1px 4px",
};

const domPreviewStyle: CSSProperties = {
  whiteSpace: "nowrap",
};

const domTagStyle: CSSProperties = {
  color: "#881280",
};

const domAttrNameStyle: CSSProperties = {
  color: "#994500",
};

const domAttrValueStyle: CSSProperties = {
  color: "#1a1aa6",
};

const domTextNodeStyle: CSSProperties = {
  color: "#111827",
  whiteSpace: "nowrap",
};

const domMutedStyle: CSSProperties = {
  color: "#6b7280",
};

const domClosingTagStyle: CSSProperties = {
  minHeight: 21,
  paddingLeft: 20,
  color: "#111827",
  whiteSpace: "nowrap",
};

const inspectorPaneStyle: CSSProperties = {
  minWidth: 0,
  overflow: "auto",
  padding: 12,
  background: "#f8fafc",
};

const inspectorNodeTitleStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  minWidth: 0,
  marginTop: 4,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 13,
};

const attributeGridStyle: CSSProperties = {
  display: "grid",
  marginTop: 8,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
  background: "#fff",
};

const attributeRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(90px, 120px) minmax(120px, 1fr)",
  gap: 8,
  minWidth: 0,
  padding: "7px 8px",
  borderTop: "1px solid #e7edf5",
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
  gridTemplateColumns: "minmax(160px, 260px) minmax(240px, 1fr) 40px",
  alignItems: "center",
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
