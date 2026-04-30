import {
  ArrowLeftOutlined,
  AimOutlined,
  BranchesOutlined,
  CodeOutlined,
  CopyOutlined,
  DatabaseOutlined,
  GlobalOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import type { CSSProperties, ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Empty, Input, Space, Tabs, Tag, Tooltip, Typography, message, theme } from "antd";
import { useNavigate, useParams } from "react-router-dom";
import { getRequestBody, getResponseBody, getTrafficDetail } from "../../api/traffic";
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
import type { TrafficRecord } from "../../types";
import { useTrafficStore } from "../../stores/useTrafficStore";
import TrafficDetail from "../../components/TrafficDetail";
import { ConsoleView, consoleValueFromRuntimeResult, type ConsoleUiEntry } from "./components/ConsolePanel";
import { DomTree, collectDefaultExpandedDomKeys, findDomNodePathById, findFirstDomSearchMatch } from "./components/ElementsPanel";
import { NetworkList } from "./components/NetworkPanel";
import { StorageView } from "./components/StoragePanel";
import { tabSearchLabel } from "./components/shared";
import "./index.css";

const { Text, Title } = Typography;

export default function DevTools() {
  const navigate = useNavigate();
  const { pageId: routePageId } = useParams<{ pageId?: string }>();
  const { token } = theme.useToken();
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
  const [elementInspecting, setElementInspecting] = useState(false);
  const [sessionWsReady, setSessionWsReady] = useState(false);
  const [expandedDomKeys, setExpandedDomKeys] = useState<Set<string>>(new Set());
  const [storageArea, setStorageArea] = useState("local_storage");
  const [storageKey, setStorageKey] = useState("");
  const [storageValue, setStorageValue] = useState("");
  const [storageSaving, setStorageSaving] = useState(false);
  const [storageEditingKey, setStorageEditingKey] = useState<string | null>(null);
  const [activeToolTab, setActiveToolTab] = useState("elements");
  const [panelSearch, setPanelSearch] = useState("");
  const [urlCopyVisible, setUrlCopyVisible] = useState(false);
  const [networkTrafficId, setNetworkTrafficId] = useState<string | null>(null);
  const [networkRecord, setNetworkRecord] = useState<TrafficRecord | null>(null);
  const [networkRequestBody, setNetworkRequestBody] = useState<string | null>(null);
  const [networkResponseBody, setNetworkResponseBody] = useState<string | null>(null);
  const [networkDetailLoading, setNetworkDetailLoading] = useState(false);
  const [networkDetailError, setNetworkDetailError] = useState<string | null>(null);
  const [networkDetailEvent, setNetworkDetailEvent] = useState<DebugNetworkEvent | null>(null);
  const networkDetailRequestRef = useRef(0);
  const domTreeRef = useRef(snapshot?.dom_tree ?? null);
  const openingPageIdRef = useRef<string | null>(null);
  const elementInspectingRef = useRef(false);
  const devtoolsThemeVars = useMemo(
    () => ({
      "--devtools-bg": token.colorBgLayout,
      "--devtools-surface": token.colorBgContainer,
      "--devtools-surface-alt": token.colorFillQuaternary,
      "--devtools-surface-elevated": token.colorBgElevated,
      "--devtools-border": token.colorBorderSecondary,
      "--devtools-border-strong": token.colorBorder,
      "--devtools-text": token.colorText,
      "--devtools-text-secondary": token.colorTextSecondary,
      "--devtools-text-tertiary": token.colorTextTertiary,
      "--devtools-selected-bg": token.colorPrimaryBg,
      "--devtools-selected-border": token.colorPrimaryBorder,
      "--devtools-search-bg": token.colorWarningBg,
      "--devtools-search-border": token.colorWarningBorder,
      "--devtools-code-tag": token.colorPrimaryText,
      "--devtools-code-attr": token.colorWarningText,
      "--devtools-code-string": token.colorErrorText,
      "--devtools-code-number": token.colorInfoText,
      "--devtools-code-boolean": token.colorPrimaryText,
      "--devtools-console-input-bg": token.colorFillQuaternary,
      "--devtools-console-error-bg": token.colorErrorBg,
      "--devtools-console-warning-bg": token.colorWarningBg,
      "--devtools-console-debug-bg": token.colorPrimaryBg,
      "--devtools-console-result-bg": token.colorSuccessBg,
      "--devtools-shadow": token.boxShadowTertiary,
    }) as CSSProperties,
    [token],
  );

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
    domTreeRef.current = snapshot?.dom_tree ?? null;
  }, [snapshot?.dom_tree]);

  useEffect(() => {
    elementInspectingRef.current = elementInspecting;
  }, [elementInspecting]);

  const selectDomNodeFromTarget = useCallback((nodeId: number) => {
    setSelectedNodeId(nodeId);
    const domTree = domTreeRef.current;
    if (domTree) {
      const match = findDomNodePathById(domTree, nodeId);
      if (match) {
        setExpandedDomKeys((previous) => new Set([...previous, ...match.expandedKeys]));
      }
    }
    window.setTimeout(() => {
      document
        .querySelector('[data-testid="devtools-dom-node"][data-selected="true"]')
        ?.scrollIntoView({ block: "center", inline: "nearest" });
    }, 80);
  }, []);

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
            if (routePageId && routePageId !== liveMessage.snapshot.page.page_id) {
              navigate(`/devtools/${encodeURIComponent(liveMessage.snapshot.page.page_id)}`, { replace: true });
            }
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
        } else if (liveMessage.type === "node_selected") {
          elementInspectingRef.current = false;
          setElementInspecting(false);
          selectDomNodeFromTarget(liveMessage.node_id);
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
      setSessionWsReady(true);
    };
    socket.onclose = () => {
      setSessionWsReady(false);
      setSession((current) =>
        current && current.session_id === sessionId
          ? { ...current, state: "disconnected" }
          : current,
      );
    };
    return () => socket.close();
  }, [navigate, requestCurrentTabRefresh, routePageId, selectDomNodeFromTarget, session?.session_id]);

  useEffect(() => {
    if (!session || !sessionWsReady) return;
    void requestCurrentTabRefresh(session.session_id, activeToolTab);
  }, [activeToolTab, requestCurrentTabRefresh, session?.session_id, sessionWsReady]);

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
  const networkEvents = snapshot?.network ?? [];

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

  const resetDetailState = () => {
    setSelectedPageId(null);
    setSession(null);
    setSessionWsReady(false);
    setSnapshot(null);
    setSelectedNodeId(null);
    setExpandedDomKeys(new Set());
    clearNetworkDetail();
  };

  const openPage = async (page: DebugPage, options: { replaceRoute?: boolean; updateRoute?: boolean } = {}) => {
    if (openingPageIdRef.current === page.page_id) return;
    openingPageIdRef.current = page.page_id;
    if (options.updateRoute !== false) {
      navigate(`/devtools/${encodeURIComponent(page.page_id)}`, { replace: Boolean(options.replaceRoute) });
    }
    setSelectedPageId(page.page_id);
    setSessionWsReady(false);
    setSnapshot(null);
    setConsoleEntries([]);
    setSelectedNodeId(null);
    setExpandedDomKeys(new Set());
    clearNetworkDetail();
    if (options.updateRoute !== false) {
      setActiveToolTab("elements");
    }
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
      openingPageIdRef.current = null;
    }
  };

  useEffect(() => {
    if (!routePageId) {
      if (selectedPageId) resetDetailState();
      return;
    }
    if (selectedPageId === routePageId || loading) return;
    const page = pages.find((item) => item.page_id === routePageId);
    if (page) {
      void openPage(page, { replaceRoute: true, updateRoute: false });
    }
  }, [loading, pages, routePageId, selectedPageId]);

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

  const startElementInspect = async () => {
    if (!session) return;
    setActiveToolTab("elements");
    elementInspectingRef.current = true;
    setElementInspecting(true);
    try {
      await sendDevtoolsCommand(session.session_id, "dom.inspect", {});
      message.info("Click an element in the target page to select it");
    } catch (error) {
      elementInspectingRef.current = false;
      setElementInspecting(false);
      message.error(error instanceof Error ? error.message : "Failed to start element picker");
    }
  };

  const handleToolTabChange = useCallback(
    (key: string) => {
      setActiveToolTab(key);
      if (session) {
        void requestCurrentTabRefresh(session.session_id, key);
      }
      if (key === "cookie" || key === "local_storage" || key === "session_storage") {
        setStorageArea(key);
        setStorageEditingKey(null);
      }
    },
    [requestCurrentTabRefresh, session],
  );

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

  const clearNetworkDetail = () => {
    networkDetailRequestRef.current += 1;
    setNetworkTrafficId(null);
    setNetworkRecord(null);
    setNetworkRequestBody(null);
    setNetworkResponseBody(null);
    setNetworkDetailError(null);
    setNetworkDetailEvent(null);
    setNetworkDetailLoading(false);
  };

  const openTrafficRecord = () => {
    if (!selectedTrafficId) return;
    setTrafficSelectedId(selectedTrafficId);
    navigate("/traffic");
  };

  const openNetworkTrafficById = async (trafficId: string) => {
    const requestId = networkDetailRequestRef.current + 1;
    networkDetailRequestRef.current = requestId;
    setNetworkTrafficId(trafficId);
    setNetworkRecord(null);
    setNetworkRequestBody(null);
    setNetworkResponseBody(null);
    setNetworkDetailError(null);
    setNetworkDetailLoading(true);
    try {
      const record = await getTrafficDetail(trafficId);
      if (networkDetailRequestRef.current !== requestId) return;
      setNetworkRecord(record);
      const [requestResult, responseResult] = await Promise.allSettled([
        getRequestBody(trafficId),
        getResponseBody(trafficId),
      ]);
      if (networkDetailRequestRef.current !== requestId) return;
      setNetworkRequestBody(requestResult.status === "fulfilled" ? requestResult.value : null);
      setNetworkResponseBody(responseResult.status === "fulfilled" ? responseResult.value : null);
    } catch (error) {
      if (networkDetailRequestRef.current !== requestId) return;
      const errorText =
        error instanceof Error
          ? error.message
          : "Traffic detail not found. It may have been deleted or captured only as a CONNECT tunnel.";
      setNetworkDetailError(errorText);
      message.warning(errorText);
    } finally {
      if (networkDetailRequestRef.current === requestId) {
        setNetworkDetailLoading(false);
      }
    }
  };

  const openNetworkTrafficRecord = async (event: DebugNetworkEvent) => {
    setNetworkDetailEvent(event);
    let trafficId: string | null = null;
    try {
      trafficId =
        event.traffic_id || (event.client_req_id ? await findTrafficForDevtoolsRequest(event.client_req_id) : null);
    } catch (error) {
      setNetworkTrafficId(null);
      setNetworkRecord(null);
      setNetworkRequestBody(null);
      setNetworkResponseBody(null);
      setNetworkDetailError(
        error instanceof Error
          ? error.message
          : "No matching Traffic record. It may have been deleted or captured only as a CONNECT tunnel.",
      );
      return;
    }
    if (!trafficId) {
      setNetworkTrafficId(null);
      setNetworkRecord(null);
      setNetworkRequestBody(null);
      setNetworkResponseBody(null);
      setNetworkDetailError("No matching Traffic record. It may have been deleted or captured only as a CONNECT tunnel.");
      return;
    }
    await openNetworkTrafficById(trafficId);
  };

  if (!selectedPage) {
    return (
      <div style={{ ...pageShellStyle, ...devtoolsThemeVars }}>
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
                    <Text type="secondary">
                      {page.state === "fallback_attached" ? "Attached" : "Online"}
                      {page.mode === "control" ? " · Control enabled" : ""}
                    </Text>
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
    <div data-testid="devtools-detail" style={{ ...detailShellStyle, ...devtoolsThemeVars }}>
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
              resetDetailState();
              navigate("/devtools");
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

        <Tabs
          className="devtools-workspace-tabs"
          data-testid="devtools-custom-workspace"
          style={workspaceStyle}
          destroyOnHidden
          activeKey={activeToolTab}
          onChange={handleToolTabChange}
          onTabClick={handleToolTabChange}
          tabBarExtraContent={{
            right: (
              <Space size={8}>
                {activeToolTab === "elements" ? (
                  <Tooltip title="Select an element from the target page">
                    <Button
                      icon={<AimOutlined />}
                      loading={elementInspecting}
                      data-testid="devtools-elements-inspect"
                      onClick={() => {
                        void startElementInspect();
                      }}
                    />
                  </Tooltip>
                ) : null}
                <Input
                  allowClear
                  data-testid="devtools-panel-search"
                  value={panelSearch}
                  onChange={(event) => setPanelSearch(event.target.value)}
                  placeholder={`Search ${tabSearchLabel(activeToolTab)}`}
                  style={panelSearchStyle}
                />
              </Space>
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
                <section
                  data-testid="devtools-network-panel"
                  style={networkDetailEvent || networkTrafficId || networkDetailLoading || networkDetailError ? networkPanelStyle : panelStyle}
                >
                  <NetworkList
                    events={networkEvents}
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
                  {networkDetailEvent || networkTrafficId || networkDetailLoading || networkDetailError ? (
                    <div data-testid="devtools-network-detail" style={networkDetailStyle}>
                      {networkRecord || networkDetailLoading ? (
                        <TrafficDetail
                          record={networkRecord}
                          requestBody={networkRequestBody}
                          responseBody={networkResponseBody}
                          loading={networkDetailLoading}
                          error={networkDetailError}
                          onSelectById={(id) => {
                            void openNetworkTrafficById(id);
                          }}
                        />
                      ) : (
                        <NetworkEventDetail event={networkDetailEvent} warning={networkDetailError} />
                      )}
                    </div>
                  ) : null}
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

function NetworkEventDetail({
  event,
  warning,
}: {
  event: DebugNetworkEvent | null;
  warning?: string | null;
}) {
  if (!event) {
    return (
      <div style={networkEventDetailEmptyStyle}>
        <Empty description="Select a network request" />
      </div>
    );
  }
  const parsed = parseNetworkEventUrl(event.url);
  const rows: Array<[string, string]> = [
    ["URL", event.url || "-"],
    ["Method", event.method || "-"],
    ["Status", event.status == null ? "-" : String(event.status)],
    ["Type", event.resource_type || "-"],
    ["From Cache", event.from_cache == null ? "-" : event.from_cache ? "yes" : "no"],
    ["Host", parsed.host],
    ["Path", parsed.path],
    ["Protocol", parsed.protocol],
    ["Client Request ID", event.client_req_id || "-"],
    ["Traffic ID", event.traffic_id || "-"],
    ["Traffic Match", warning || "No matching Traffic record. It may have been deleted or captured only as a CONNECT tunnel."],
    ["Time", formatNetworkEventTime(event.at_ms)],
  ];
  return (
    <div style={networkEventDetailShellStyle}>
      <div style={networkEventDetailHeaderStyle}>
        <Text strong>Network Detail</Text>
        <Tag>{event.method || "GET"}</Tag>
      </div>
      {warning ? <Alert type="warning" showIcon message={warning} style={{ marginBottom: 10 }} /> : null}
      <div data-testid="devtools-network-fallback-detail" style={networkEventDetailGridStyle}>
        {rows.map(([label, value]) => (
          <div key={label} style={networkEventDetailRowStyle}>
            <Text type="secondary" style={networkEventDetailLabelStyle}>{label}</Text>
            <Text copyable={label === "URL" || label === "Client Request ID"} style={networkEventDetailValueStyle}>
              {value}
            </Text>
          </div>
        ))}
      </div>
      <NetworkPairSection title="Query" pairs={event.query_params} />
      <NetworkPairSection title="Request Headers" pairs={event.request_headers} />
      <NetworkPairSection title="Response Headers" pairs={event.response_headers} />
    </div>
  );
}

function NetworkPairSection({ title, pairs }: { title: string; pairs?: Array<[string, string]> }) {
  const visiblePairs = pairs?.filter(([key]) => key) ?? [];
  return (
    <div data-testid={`devtools-network-${title.toLowerCase().replace(/\s+/g, "-")}`} style={networkPairSectionStyle}>
      <Text strong>{title}</Text>
      {visiblePairs.length ? (
        <div style={networkPairGridStyle}>
          {visiblePairs.map(([key, value], index) => (
            <div key={`${key}-${index}`} style={networkPairRowStyle}>
              <Text code style={networkPairNameStyle}>{key}</Text>
              <Text copyable style={networkPairValueStyle}>{value}</Text>
            </div>
          ))}
        </div>
      ) : (
        <Text type="secondary">No captured {title.toLowerCase()}</Text>
      )}
    </div>
  );
}

function parseNetworkEventUrl(url: string): { host: string; path: string; protocol: string } {
  try {
    const parsed = new URL(url);
    return {
      host: parsed.host || "-",
      path: `${parsed.pathname || "/"}${parsed.search || ""}`,
      protocol: parsed.protocol.replace(":", "") || "-",
    };
  } catch {
    return { host: "-", path: url || "-", protocol: "-" };
  }
}

function formatNetworkEventTime(atMs: number): string {
  if (!Number.isFinite(atMs) || atMs <= 0) return "-";
  const date = new Date(atMs);
  const pad = (value: number, size = 2) => String(value).padStart(size, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
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
  const scope = typeof incoming.scope === "string" ? incoming.scope : null;
  const metadataOnly =
    previous &&
    incoming.page &&
    Array.isArray(incoming.console) &&
    incoming.console.length === 0 &&
    Array.isArray(incoming.network) &&
    incoming.network.length === 0 &&
    Object.prototype.hasOwnProperty.call(incoming, "dom_snapshot") &&
    incoming.dom_snapshot == null &&
    Object.prototype.hasOwnProperty.call(incoming, "dom_tree") &&
    incoming.dom_tree == null &&
    Object.prototype.hasOwnProperty.call(incoming, "storage") &&
    incoming.storage == null;
  if (metadataOnly) {
    return { ...base, page: incoming.page ?? base.page };
  }
  const shouldReplace = (
    field: "console" | "network" | "storage" | "dom_snapshot" | "dom_tree",
    matchingScopes: string[],
  ) => {
    if (!Object.prototype.hasOwnProperty.call(incoming, field)) return false;
    if (!previous || !scope || scope === "full" || matchingScopes.includes(scope)) return true;
    const value = incoming[field];
    if (Array.isArray(value)) return value.length > 0;
    return value != null;
  };
  return {
    page: incoming.page ?? base.page,
    scope: incoming.scope ?? base.scope,
    console: shouldReplace("console", ["console"])
      ? (incoming.console ?? [])
      : base.console,
    network: shouldReplace("network", ["network"])
      ? (incoming.network ?? [])
      : base.network,
    storage: shouldReplace("storage", ["storage"])
      ? (incoming.storage ?? null)
      : base.storage,
    dom_snapshot: shouldReplace("dom_snapshot", ["elements"])
      ? (incoming.dom_snapshot ?? null)
      : base.dom_snapshot,
    dom_tree: shouldReplace("dom_tree", ["elements"])
      ? (incoming.dom_tree ?? null)
      : base.dom_tree,
  };
}

const pageShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: 20,
  overflow: "auto",
  background: "var(--devtools-bg)",
  color: "var(--devtools-text)",
};

const detailShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: "10px 20px 12px",
  overflow: "hidden",
  background: "var(--devtools-bg)",
  color: "var(--devtools-text)",
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
  border: "1px solid var(--devtools-border)",
  borderRadius: 8,
  background: "var(--devtools-surface)",
  color: "var(--devtools-text)",
  textAlign: "left",
  cursor: "pointer",
  boxShadow: "var(--devtools-shadow)",
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
  background: "var(--devtools-surface)",
  border: "1px solid var(--devtools-border)",
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

const networkPanelStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  minWidth: 0,
  display: "grid",
  gridTemplateColumns: "minmax(360px, 1fr) minmax(380px, 44%)",
  gap: 10,
};

const networkDetailStyle: CSSProperties = {
  minHeight: 0,
  minWidth: 0,
  overflow: "hidden",
  border: "1px solid var(--devtools-border)",
  borderRadius: 8,
  background: "var(--devtools-surface)",
};

const networkEventDetailEmptyStyle: CSSProperties = {
  height: "100%",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const networkEventDetailShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  overflow: "auto",
  padding: 12,
};

const networkEventDetailHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 8,
  marginBottom: 10,
};

const networkEventDetailGridStyle: CSSProperties = {
  display: "grid",
  gap: 0,
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  overflow: "hidden",
};

const networkEventDetailRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "132px minmax(0, 1fr)",
  gap: 10,
  padding: "8px 10px",
  borderBottom: "1px solid var(--devtools-border)",
  minWidth: 0,
};

const networkEventDetailLabelStyle: CSSProperties = {
  fontSize: 12,
};

const networkEventDetailValueStyle: CSSProperties = {
  minWidth: 0,
  fontSize: 12,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  wordBreak: "break-all",
};

const networkPairSectionStyle: CSSProperties = {
  marginTop: 12,
  display: "grid",
  gap: 8,
};

const networkPairGridStyle: CSSProperties = {
  display: "grid",
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  overflow: "hidden",
};

const networkPairRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(120px, 34%) minmax(0, 1fr)",
  gap: 10,
  padding: "7px 10px",
  borderBottom: "1px solid var(--devtools-border)",
  minWidth: 0,
};

const networkPairNameStyle: CSSProperties = {
  minWidth: 0,
  fontSize: 12,
  whiteSpace: "normal",
  overflowWrap: "anywhere",
};

const networkPairValueStyle: CSSProperties = {
  minWidth: 0,
  fontSize: 12,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  whiteSpace: "pre-wrap",
  overflowWrap: "anywhere",
};
