import {
  ArrowLeftOutlined,
  BranchesOutlined,
  CodeOutlined,
  CopyOutlined,
  DatabaseOutlined,
  GlobalOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import type { CSSProperties, ReactNode } from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Alert, Button, Empty, Input, Space, Tabs, Tag, Tooltip, Typography, message } from "antd";
import { useNavigate } from "react-router-dom";
import {
  buildDevtoolsSessionWsUrl,
  findTrafficForDevtoolsRequest,
  listDevtoolsPages,
  openDevtoolsSession,
  requestDevtoolsSnapshotRefresh,
  sendDevtoolsCommand,
  type DevtoolsLiveMessage,
  type DebugNetworkEvent,
  type DebugPage,
  type DebugSession,
  type DevtoolsSnapshot,
} from "../../api/devtools";
import { useTrafficStore } from "../../stores/useTrafficStore";
import { ConsoleView, consoleValueFromRuntimeResult, type ConsoleUiEntry } from "./components/ConsolePanel";
import { DomTree, collectDefaultExpandedDomKeys, findFirstDomSearchMatch } from "./components/ElementsPanel";
import { NetworkList } from "./components/NetworkPanel";
import { StorageView } from "./components/StoragePanel";
import { tabSearchLabel } from "./components/shared";
import "./index.css";

const { Text, Title } = Typography;

export default function DevTools() {
  const navigate = useNavigate();
  const setTrafficSelectedId = useTrafficStore((state) => state.setSelectedId);
  const [pages, setPages] = useState<DebugPage[]>([]);
  const [query, setQuery] = useState("");
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [session, setSession] = useState<DebugSession | null>(null);
  const [snapshot, setSnapshot] = useState<DevtoolsSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [snapshotLoading, setSnapshotLoading] = useState(false);
  const [consoleExpression, setConsoleExpression] = useState("document.title");
  const [consoleEntries, setConsoleEntries] = useState<ConsoleUiEntry[]>([]);
  const [consoleRunning, setConsoleRunning] = useState(false);
  const [selectedNodeId, setSelectedNodeId] = useState<number | null>(null);
  const [expandedDomKeys, setExpandedDomKeys] = useState<Set<string>>(new Set());
  const [storageArea, setStorageArea] = useState("local_storage");
  const [storageKey, setStorageKey] = useState("");
  const [storageValue, setStorageValue] = useState("");
  const [storageSaving, setStorageSaving] = useState(false);
  const [storageEditingKey, setStorageEditingKey] = useState<string | null>(null);
  const [activeToolTab, setActiveToolTab] = useState("elements");
  const [panelSearch, setPanelSearch] = useState("");
  const [urlCopyVisible, setUrlCopyVisible] = useState(false);

  const refreshPages = useCallback(async () => {
    const next = await listDevtoolsPages(true);
    setPages(next);
  }, []);

  const refreshSnapshot = useCallback(async (sessionId: string, options: { full?: boolean } = {}) => {
    setSnapshotLoading(true);
    try {
      await requestDevtoolsSnapshotRefresh(sessionId, options.full ? "full" : activeToolTab);
      setSnapshot((previous) => previous ?? null);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to refresh DevTools data");
    } finally {
      setSnapshotLoading(false);
    }
  }, [activeToolTab]);

  const requestCurrentTabRefresh = useCallback(async (sessionId: string, tabKey: string) => {
    const scope =
      tabKey === "elements"
        ? "elements"
        : tabKey === "network"
          ? "network"
          : tabKey === "console"
            ? "console"
            : tabKey === "cookie" || tabKey === "local_storage" || tabKey === "session_storage"
              ? "storage"
              : "full";
    await requestDevtoolsSnapshotRefresh(sessionId, scope).catch(() => undefined);
  }, []);

  useEffect(() => {
    if (selectedPageId) return;
    void refreshPages();
    const timer = window.setInterval(() => {
      void refreshPages();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [refreshPages, selectedPageId]);

  useEffect(() => {
    if (!session) return;
    const sessionId = session.session_id;
    const socket = new WebSocket(buildDevtoolsSessionWsUrl(sessionId));
    socket.onmessage = (event) => {
      try {
        const liveMessage = JSON.parse(String(event.data)) as DevtoolsLiveMessage;
        if (liveMessage.type === "snapshot") {
          setSnapshot((previous) => mergeSnapshot(previous, liveMessage.snapshot));
          if (liveMessage.snapshot.page?.page_id) {
            setSelectedPageId((current) => (current === null ? current : liveMessage.snapshot.page.page_id));
            setSession((current) =>
              current && current.session_id === sessionId && current.page_id !== liveMessage.snapshot.page.page_id
                ? { ...current, page_id: liveMessage.snapshot.page.page_id }
                : current,
            );
          }
        } else if (liveMessage.type === "console") {
          setSnapshot((previous) =>
            previous
              ? { ...previous, console: [...previous.console, liveMessage.message].slice(-200) }
              : previous,
          );
        } else if (liveMessage.type === "network") {
          setSnapshot((previous) =>
            previous
              ? { ...previous, network: [...previous.network, liveMessage.event].slice(-500) }
              : previous,
          );
        } else if (liveMessage.type === "disconnected") {
          setSession((current) =>
            current && current.session_id === sessionId
              ? { ...current, state: "disconnected" }
              : current,
          );
          message.warning(liveMessage.reason || "Target page DevTools bridge disconnected");
        }
      } catch (error) {
        console.warn("Failed to parse DevTools live message", error);
      }
    };
    socket.onopen = () => {
      void requestCurrentTabRefresh(sessionId, "elements");
    };
    socket.onclose = () => {
      setSession((current) =>
        current && current.session_id === sessionId
          ? { ...current, state: "disconnected" }
          : current,
      );
    };
    return () => socket.close();
  }, [requestCurrentTabRefresh, session?.session_id]);

  useEffect(() => {
    if (!session) return;
    void requestCurrentTabRefresh(session.session_id, activeToolTab);
  }, [activeToolTab, requestCurrentTabRefresh, session?.session_id]);

  const filteredPages = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return pages;
    return pages.filter((page) =>
      [page.title, page.url, page.adapter, page.state]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(needle)),
    );
  }, [pages, query]);

  const selectedPage = selectedPageId
    ? pages.find((page) => page.page_id === selectedPageId) ?? snapshot?.page ?? null
    : null;
  const selectedTrafficId = selectedPage?.traffic_ids?.slice(-1)[0] ?? null;

  useEffect(() => {
    const tree = snapshot?.dom_tree ?? null;
    if (!tree) return;
    setExpandedDomKeys((previous) => {
      if (previous.size) return previous;
      return collectDefaultExpandedDomKeys(tree);
    });
  }, [snapshot?.dom_tree]);

  useEffect(() => {
    const tree = snapshot?.dom_tree ?? null;
    const needle = panelSearch.trim();
    if (activeToolTab !== "elements" || !tree || !needle) return;
    const match = findFirstDomSearchMatch(tree, needle);
    if (!match?.node.nodeId) return;
    setSelectedNodeId(match.node.nodeId);
    setExpandedDomKeys((previous) => {
      const next = new Set(previous);
      match.expandedKeys.forEach((key) => next.add(key));
      return next;
    });
  }, [activeToolTab, panelSearch, snapshot?.dom_tree]);

  const openPage = async (page: DebugPage) => {
    setSelectedPageId(page.page_id);
    setSnapshot(null);
    setConsoleEntries([]);
    setSelectedNodeId(null);
    setExpandedDomKeys(new Set());
    setActiveToolTab("elements");
    setPanelSearch("");
    setLoading(true);
    try {
      const nextSession = await openDevtoolsSession(page.page_id);
      setSession(nextSession);
      setSnapshot(emptySnapshot(page));
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
    const inputEntry: ConsoleUiEntry = {
      kind: "input",
      level: "input",
      text: expression,
      at_ms: Date.now(),
    };
    setConsoleEntries((entries) => [...entries, inputEntry]);
    setConsoleRunning(true);
    try {
      const response = await sendDevtoolsCommand(session.session_id, "runtime.evaluate", {
        expression,
      });
      const exceptionText = runtimeExceptionText(response.result);
      const resultValue = exceptionText ? undefined : consoleValueFromRuntimeResult(response.result);
      const resultText = exceptionText ?? resultValue?.raw ?? resultValue?.preview ?? "";
      setConsoleExpression("");
      setConsoleEntries((entries) => [
        ...entries,
        {
          kind: "result",
          level: exceptionText ? "error" : "result",
          text: resultText,
          args: resultValue ? [resultValue] : undefined,
          raw: resultValue?.raw ?? resultText,
          at_ms: Date.now(),
        },
      ]);
      await refreshSnapshot(session.session_id);
    } catch (error) {
      const errorText = error instanceof Error ? error.message : String(error);
      setConsoleEntries((entries) => [
        ...entries,
        { kind: "result", level: "error", text: errorText, at_ms: Date.now() },
      ]);
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
      if (storageEditingKey && storageEditingKey !== storageKey) {
        await sendDevtoolsCommand(session.session_id, "storage.delete", {
          area: storageArea,
          key: storageEditingKey,
        });
      }
      await refreshSnapshot(session.session_id);
      setStorageEditingKey(null);
      setStorageKey("");
      setStorageValue("");
      message.success("Storage updated");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to update storage");
    } finally {
      setStorageSaving(false);
    }
  };

  const deleteStorageValue = async (area: string, key: string) => {
    if (!session) return;
    setStorageSaving(true);
    try {
      await sendDevtoolsCommand(session.session_id, "storage.delete", { area, key });
      await refreshSnapshot(session.session_id);
      message.success("Storage deleted");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to delete storage");
    } finally {
      setStorageSaving(false);
    }
  };

  const copyStorageValue = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      message.success("Copied");
    } catch {
      message.error("Failed to copy");
    }
  };

  const copyTargetUrl = async () => {
    if (!selectedPage?.url) return;
    try {
      await navigator.clipboard.writeText(selectedPage.url);
      message.success("URL copied");
    } catch {
      message.error("Failed to copy URL");
    }
  };

  const openTrafficRecord = () => {
    if (!selectedTrafficId) return;
    setTrafficSelectedId(selectedTrafficId);
    navigate("/traffic");
  };

  const openNetworkTrafficRecord = async (event: DebugNetworkEvent) => {
    const trafficId =
      event.traffic_id || (event.client_req_id ? await findTrafficForDevtoolsRequest(event.client_req_id) : null);
    if (!trafficId) {
      message.warning("No matching Traffic record. It may have been deleted or captured only as a CONNECT tunnel.");
      return;
    }
    setTrafficSelectedId(trafficId);
    navigate("/traffic");
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
      <div
        style={{
          ...detailContentStyle,
          gridTemplateRows: selectedPage.status_reason
            ? "auto auto minmax(0, 1fr)"
            : "auto minmax(0, 1fr)",
        }}
      >
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
          <div style={titleClusterStyle}>
            <Space size={8} style={{ minWidth: 0 }}>
              <Text strong ellipsis style={{ maxWidth: 260, fontSize: 16 }}>
                {selectedPage.title || "(untitled)"}
              </Text>
              {selectedTrafficId ? (
                <Button
                  size="small"
                  type="link"
                  data-testid="devtools-traffic-link"
                  style={trafficLinkStyle}
                  onClick={openTrafficRecord}
                >
                  Traffic {selectedTrafficId}
                </Button>
              ) : null}
            </Space>
            <div
              data-testid="devtools-target-url"
              style={urlLineStyle}
              onMouseEnter={() => setUrlCopyVisible(true)}
              onMouseLeave={() => setUrlCopyVisible(false)}
            >
              <Text type="secondary" ellipsis style={{ minWidth: 0, maxWidth: "min(780px, 56vw)" }}>
                {selectedPage.url}
              </Text>
              <Tooltip title="Copy URL">
                <Button
                  size="small"
                  type="text"
                  aria-label="Copy target URL"
                  data-testid="devtools-copy-url"
                  icon={<CopyOutlined />}
                  style={{
                    ...urlCopyButtonStyle,
                    opacity: urlCopyVisible ? 1 : 0,
                  }}
                  onClick={copyTargetUrl}
                />
              </Tooltip>
            </div>
          </div>
          <Button
            size="small"
            data-testid="devtools-refresh"
            icon={<ReloadOutlined />}
            loading={loading || snapshotLoading}
            onClick={() => {
              if (session) {
                void refreshSnapshot(session.session_id, { full: true });
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

        <Tabs
          className="devtools-workspace-tabs"
          data-testid="devtools-custom-workspace"
          style={workspaceStyle}
          destroyOnHidden
          activeKey={activeToolTab}
          onChange={(key) => {
            setActiveToolTab(key);
            if (key === "cookie" || key === "local_storage" || key === "session_storage") {
              setStorageArea(key);
              setStorageEditingKey(null);
            }
          }}
          tabBarExtraContent={{
            right: (
              <Input
                allowClear
                data-testid="devtools-panel-search"
                value={panelSearch}
                onChange={(event) => setPanelSearch(event.target.value)}
                placeholder={`Search ${tabSearchLabel(activeToolTab)}`}
                style={panelSearchStyle}
              />
            ),
          }}
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
                    expandedKeys={expandedDomKeys}
                    searchQuery={panelSearch}
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
                  <NetworkList
                    events={snapshot?.network ?? []}
                    searchQuery={panelSearch}
                    onOpenTraffic={(event) => {
                      void openNetworkTrafficRecord(event).catch((error) => {
                        message.warning(
                          error instanceof Error
                            ? error.message
                            : "No matching Traffic record. It may have been deleted or captured only as a CONNECT tunnel.",
                        );
                      });
                    }}
                  />
                </section>
              ),
            },
            {
              key: "cookie",
              label: <TabLabel icon={<DatabaseOutlined />} text="Cookies" />,
              children: (
                <section data-testid="devtools-cookies-panel" style={panelStyle}>
                  <StorageView
                    storage={snapshot?.storage ?? null}
                    area="cookie"
                    searchQuery={panelSearch}
                    storageKey={storageKey}
                    storageValue={storageValue}
                    editingKey={storageArea === "cookie" ? storageEditingKey : null}
                    saving={storageSaving}
                    onKeyChange={setStorageKey}
                    onValueChange={setStorageValue}
                    onStartEdit={(nextArea, key, value) => {
                      setStorageArea(nextArea);
                      setStorageKey(key);
                      setStorageValue(value);
                      setStorageEditingKey(key);
                    }}
                    onStartAdd={(nextArea) => {
                      setStorageArea(nextArea);
                      setStorageKey("");
                      setStorageValue("");
                      setStorageEditingKey("");
                    }}
                    onCancelEdit={() => {
                      setStorageEditingKey(null);
                      setStorageKey("");
                      setStorageValue("");
                    }}
                    onCopy={copyStorageValue}
                    onDelete={(nextArea, key) => void deleteStorageValue(nextArea, key)}
                    onSave={() => void updateStorageValue()}
                  />
                </section>
              ),
            },
            {
              key: "local_storage",
              label: <TabLabel icon={<DatabaseOutlined />} text="LocalStorage" />,
              children: (
                <section data-testid="devtools-local-storage-panel" style={panelStyle}>
                  <StorageView
                    storage={snapshot?.storage ?? null}
                    area="local_storage"
                    searchQuery={panelSearch}
                    storageKey={storageKey}
                    storageValue={storageValue}
                    editingKey={storageArea === "local_storage" ? storageEditingKey : null}
                    saving={storageSaving}
                    onKeyChange={setStorageKey}
                    onValueChange={setStorageValue}
                    onStartEdit={(nextArea, key, value) => {
                      setStorageArea(nextArea);
                      setStorageKey(key);
                      setStorageValue(value);
                      setStorageEditingKey(key);
                    }}
                    onStartAdd={(nextArea) => {
                      setStorageArea(nextArea);
                      setStorageKey("");
                      setStorageValue("");
                      setStorageEditingKey("");
                    }}
                    onCancelEdit={() => {
                      setStorageEditingKey(null);
                      setStorageKey("");
                      setStorageValue("");
                    }}
                    onCopy={copyStorageValue}
                    onDelete={(nextArea, key) => void deleteStorageValue(nextArea, key)}
                    onSave={() => void updateStorageValue()}
                  />
                </section>
              ),
            },
            {
              key: "session_storage",
              label: <TabLabel icon={<DatabaseOutlined />} text="SessionStorage" />,
              children: (
                <section data-testid="devtools-session-storage-panel" style={panelStyle}>
                  <StorageView
                    storage={snapshot?.storage ?? null}
                    area="session_storage"
                    searchQuery={panelSearch}
                    storageKey={storageKey}
                    storageValue={storageValue}
                    editingKey={storageArea === "session_storage" ? storageEditingKey : null}
                    saving={storageSaving}
                    onKeyChange={setStorageKey}
                    onValueChange={setStorageValue}
                    onStartEdit={(nextArea, key, value) => {
                      setStorageArea(nextArea);
                      setStorageKey(key);
                      setStorageValue(value);
                      setStorageEditingKey(key);
                    }}
                    onStartAdd={(nextArea) => {
                      setStorageArea(nextArea);
                      setStorageKey("");
                      setStorageValue("");
                      setStorageEditingKey("");
                    }}
                    onCancelEdit={() => {
                      setStorageEditingKey(null);
                      setStorageKey("");
                      setStorageValue("");
                    }}
                    onCopy={copyStorageValue}
                    onDelete={(nextArea, key) => void deleteStorageValue(nextArea, key)}
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
                    messages={snapshot?.console ?? []}
                    entries={consoleEntries}
                    searchQuery={panelSearch}
                    expression={consoleExpression}
                    running={consoleRunning}
                    onExpressionChange={setConsoleExpression}
                    onRun={() => void runConsoleExpression()}
                  />
                </section>
              ),
            },
          ]}
        />
      </div>
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

function runtimeExceptionText(result: unknown): string | null {
  if (!result || typeof result !== "object") return null;
  const payload = result as {
    exception?: unknown;
    exceptionDetails?: {
      text?: unknown;
      exception?: {
        description?: unknown;
        value?: unknown;
      };
    };
  };
  const exception = payload.exception;
  if (typeof exception === "string" && exception) return exception;
  const description = payload.exceptionDetails?.exception?.description;
  if (typeof description === "string" && description) return description;
  const value = payload.exceptionDetails?.exception?.value;
  if (typeof value === "string" && value) return value;
  const text = payload.exceptionDetails?.text;
  return typeof text === "string" && text ? text : null;
}

function emptySnapshot(page: DebugPage): DevtoolsSnapshot {
  return {
    page,
    console: [],
    dom_snapshot: null,
    dom_tree: null,
    network: [],
    storage: null,
  };
}

function mergeSnapshot(previous: DevtoolsSnapshot | null, incoming: Partial<DevtoolsSnapshot>): DevtoolsSnapshot {
  const base = previous ?? emptySnapshot(incoming.page as DebugPage);
  return {
    page: incoming.page ?? base.page,
    console: Object.prototype.hasOwnProperty.call(incoming, "console")
      ? (incoming.console ?? [])
      : base.console,
    network: Object.prototype.hasOwnProperty.call(incoming, "network")
      ? (incoming.network ?? [])
      : base.network,
    storage: Object.prototype.hasOwnProperty.call(incoming, "storage")
      ? (incoming.storage ?? null)
      : base.storage,
    dom_snapshot: Object.prototype.hasOwnProperty.call(incoming, "dom_snapshot")
      ? (incoming.dom_snapshot ?? null)
      : base.dom_snapshot,
    dom_tree: Object.prototype.hasOwnProperty.call(incoming, "dom_tree")
      ? (incoming.dom_tree ?? null)
      : base.dom_tree,
  };
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
  overflow: "hidden",
  background: "#f7f9fc",
};

const detailContentStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  display: "grid",
  gap: 10,
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
  minHeight: 42,
};

const titleClusterStyle: CSSProperties = {
  flex: 1,
  minWidth: 0,
  display: "grid",
  gap: 2,
};

const urlLineStyle: CSSProperties = {
  minWidth: 0,
  display: "inline-grid",
  gridTemplateColumns: "minmax(0, auto) 28px",
  alignItems: "center",
  columnGap: 2,
  width: "fit-content",
  maxWidth: "100%",
};

const urlCopyButtonStyle: CSSProperties = {
  transition: "opacity 0.15s ease",
};

const trafficLinkStyle: CSSProperties = {
  padding: 0,
  height: 22,
};

const workspaceStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  minWidth: 0,
  background: "#fff",
  border: "1px solid #d9e2ef",
  borderRadius: 8,
  padding: "0 12px 12px",
};

const panelSearchStyle: CSSProperties = {
  width: "100%",
  maxWidth: 420,
};


const panelStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  minWidth: 0,
  overflow: "auto",
};







