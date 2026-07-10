import {
  useEffect,
  useCallback,
  useRef,
  useMemo,
  useDeferredValue,
  useState,
  type CSSProperties,
} from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Alert, Button, message, theme } from "antd";
import { useShallow } from "zustand/react/shallow";
import {
  useTrafficStore,
  applyTrafficRecordsMutationToFilteredRecords,
  filterRecords,
  type PanelFilters,
} from "../../stores/useTrafficStore";
import {
  isSystemProxyLiveEnabledByBifrost,
  useProxyStore,
} from "../../stores/useProxyStore";
import { useBreakpointStore } from "../../stores/useBreakpointStore";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import { useTrafficDetailWindowStore } from "../../stores/useTrafficDetailWindowStore";
import { useSearchStore } from "../../stores/useSearchStore";
import VirtualTrafficTable from "../../components/TrafficTable/VirtualTrafficTable";
import TrafficDetail from "../../components/TrafficDetail";
import Toolbar from "../../components/Toolbar";
import FilterBar from "../../components/FilterBar";
import ThreeSplitPane from "../../components/ThreeSplitPane";
import FilterPanel from "../../components/FilterPanel";
import SearchMode from "../../components/SearchMode";
import {
  decodeJsonFromQueryParam,
  encodeJsonForQueryParam,
} from "../../utils/urlState";
import { buildAppRouteUrl, isMacDesktopShell } from "../../runtime";
import { getPerformanceConfig } from "../../api/config";
import pushService from "../../services/pushService";
import type {
  TrafficSummary,
  FilterCondition,
  ToolbarFilters,
  SearchScope,
} from "../../types";

const FILTER_PARAM = "filter";
const TOOLBAR_PARAM = "toolbar";
const PANEL_PARAM = "panel";
const SEARCH_PARAM = "search";

const serializeFilters = (filters: FilterCondition[]): string => {
  if (filters.length === 0) return "";
  return encodeJsonForQueryParam(filters);
};

const deserializeFilters = (str: string): FilterCondition[] => {
  if (!str) return [];
  const value = decodeJsonFromQueryParam<unknown>(str);
  if (!Array.isArray(value)) return [];
  return value
    .filter((v): v is Record<string, unknown> => !!v && typeof v === "object")
    .map((v) => ({
      id: typeof v.id === "string" ? v.id : "",
      field: typeof v.field === "string" ? v.field : "",
      operator: typeof v.operator === "string" ? v.operator : "",
      value: typeof v.value === "string" ? v.value : "",
      enabled: typeof v.enabled === "boolean" ? v.enabled : true,
    }))
    .filter((v) => v.id && v.field && v.operator);
};

const serializeToolbar = (toolbar: ToolbarFilters): string => {
  const hasFilters =
    toolbar.rule.length > 0 ||
    toolbar.protocol.length > 0 ||
    toolbar.type.length > 0 ||
    toolbar.status.length > 0 ||
    toolbar.imported.length > 0;
  if (!hasFilters) return "";
  return encodeJsonForQueryParam(toolbar);
};

const deserializeToolbar = (str: string): ToolbarFilters | null => {
  if (!str) return null;
  const value = decodeJsonFromQueryParam<unknown>(str);
  if (!value || typeof value !== "object") return null;
  const v = value as Record<string, unknown>;
  const toStringArray = (input: unknown): string[] =>
    Array.isArray(input)
      ? input.filter((x): x is string => typeof x === "string")
      : [];
  return {
    rule: toStringArray(v.rule),
    protocol: toStringArray(v.protocol),
    type: toStringArray(v.type),
    status: toStringArray(v.status),
    imported: toStringArray(v.imported),
  };
};

export default function Traffic() {
  const { token } = theme.useToken();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [superPerformanceMode, setSuperPerformanceMode] = useState(false);
  const detachedPopupRef = useRef<Window | null>(null);

  const records = useTrafficStore((state) => state.records);
  const recordsMutation = useTrafficStore((state) => state.recordsMutation);
  const hasMore = useTrafficStore((state) => state.hasMore);
  const toolbarFilters = useTrafficStore((state) => state.toolbarFilters);
  const filterConditions = useTrafficStore((state) => state.filterConditions);
  const autoScroll = useTrafficStore((state) => state.autoScroll);
  const newRecordsCount = useTrafficStore((state) => state.newRecordsCount);
  const scrollTop = useTrafficStore((state) => state.scrollTop);
  const selectedId = useTrafficStore((state) => state.selectedId);
  const clientInfo = useTrafficStore(
    useShallow((state) => ({
      apps: state.availableClientApps,
      accounts: state.availableAccountNames,
      ips: state.availableClientIps,
      proxyPorts: state.availableProxyPorts,
      domains: state.availableDomains,
      appCounts: state.clientAppCounts,
      accountCounts: state.accountNameCounts,
      ipCounts: state.clientIpCounts,
      proxyPortCounts: state.proxyPortCounts,
      domainCounts: state.domainCounts,
    })),
  );
  const { currentRecord, requestBody, responseBody, requestRawBody, responseRawBody, detailLoading, detailError } =
    useTrafficStore(
      useShallow((state) => ({
        currentRecord: state.currentRecord,
        requestBody: state.requestBody,
        responseBody: state.responseBody,
        requestRawBody: state.requestRawBody,
        responseRawBody: state.responseRawBody,
        detailLoading: state.detailLoading,
        detailError: state.detailError,
      })),
    );

  const {
    fetchTrafficDetail,
    clearTraffic,
    setToolbarFilters,
    setFilterConditions,
    setAutoScroll,
    clearNewRecordsCount,
    initFromUrl,
    setScrollTop,
    setSelectedId,
  } = useTrafficStore(
    useShallow((state) => ({
      fetchTrafficDetail: state.fetchTrafficDetail,
      clearTraffic: state.clearTraffic,
      setToolbarFilters: state.setToolbarFilters,
      setFilterConditions: state.setFilterConditions,
      setAutoScroll: state.setAutoScroll,
      clearNewRecordsCount: state.clearNewRecordsCount,
      initFromUrl: state.initFromUrl,
      setScrollTop: state.setScrollTop,
      setSelectedId: state.setSelectedId,
    })),
  );

  const showFilterBar = true;
  const systemProxy = useProxyStore((state) => state.systemProxy);
  const systemProxyLoading = useProxyStore((state) => state.loading);
  const toggleSystemProxy = useProxyStore((state) => state.toggleSystemProxy);

  const breakpointEnabled = useBreakpointStore((state) => state.enabled);
  const breakpointLoading = useBreakpointStore((state) => state.loading);
  const toggleBreakpoint = useBreakpointStore(
    (state) => state.toggleEnabled,
  );

  useEffect(() => {
    useBreakpointStore.getState().connectPush();
    useBreakpointStore.getState().fetchSettings();
  }, []);

  useEffect(() => {
    let cancelled = false;
    const refreshPerformanceMode = async () => {
      try {
        const config = await getPerformanceConfig();
        if (!cancelled) {
          setSuperPerformanceMode(config.traffic.super_performance_mode);
        }
      } catch {
        if (!cancelled) {
          setSuperPerformanceMode(false);
        }
      }
    };

    void refreshPerformanceMode();
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: Array.from(
        new Set([
          ...(pushService.getSubscription().settings_scopes ?? []),
          "performance_config" as const,
        ]),
      ),
    });
    const unsubscribe = pushService.onSettingsUpdate((data) => {
      if (data.scope === "performance_config") {
        const maybeConfig = data.data as { traffic?: { super_performance_mode?: boolean } };
        if (typeof maybeConfig.traffic?.super_performance_mode === "boolean") {
          setSuperPerformanceMode(maybeConfig.traffic.super_performance_mode);
        } else {
          void refreshPerformanceMode();
        }
      }
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const filterPanelCollapsed = useFilterPanelStore(
    (state) => state.panelCollapsed,
  );
  const setFilterPanelCollapsed = useFilterPanelStore(
    (state) => state.setPanelCollapsed,
  );
  const filterPanelWidth = useFilterPanelStore((state) => state.panelWidth);
  const setFilterPanelWidth = useFilterPanelStore(
    (state) => state.setPanelWidth,
  );
  const detailPanelCollapsed = useFilterPanelStore(
    (state) => state.detailPanelCollapsed,
  );
  const setDetailPanelCollapsed = useFilterPanelStore(
    (state) => state.setDetailPanelCollapsed,
  );
  const selectedClientIps = useFilterPanelStore(
    (state) => state.selectedClientIps,
  );
  const selectedProxyPorts = useFilterPanelStore(
    (state) => state.selectedProxyPorts,
  );
  const selectedClientApps = useFilterPanelStore(
    (state) => state.selectedClientApps,
  );
  const selectedAccountNames = useFilterPanelStore(
    (state) => state.selectedAccountNames,
  );
  const selectedDomains = useFilterPanelStore((state) => state.selectedDomains);
  const setSelectedClientIps = useFilterPanelStore(
    (state) => state.setSelectedClientIps,
  );
  const setSelectedProxyPorts = useFilterPanelStore(
    (state) => state.setSelectedProxyPorts,
  );
  const setSelectedClientApps = useFilterPanelStore(
    (state) => state.setSelectedClientApps,
  );
  const setSelectedAccountNames = useFilterPanelStore(
    (state) => state.setSelectedAccountNames,
  );
  const setSelectedDomains = useFilterPanelStore(
    (state) => state.setSelectedDomains,
  );
  const filterPanelInitialized = useFilterPanelStore(
    (state) => state.initialized,
  );
  const detailDetached = useTrafficDetailWindowStore((state) => state.detached);
  const detachDetailWindow = useTrafficDetailWindowStore((state) => state.detach);
  const attachDetailWindow = useTrafficDetailWindowStore((state) => state.attach);

  const searchMode = useSearchStore((state) => state.mode);
  const setSearchMode = useSearchStore((state) => state.setMode);
  const searchKeyword = useSearchStore((state) => state.keyword);
  const setSearchKeyword = useSearchStore((state) => state.setKeyword);
  const searchScope = useSearchStore((state) => state.scope);
  const setSearchScope = useSearchStore((state) => state.setScope);

  const pendingUrlUpdateRef = useRef<Record<string, string>>({});

  const isDefaultSearchScope = useCallback((scope: SearchScope) => {
    return (
      scope.all === true &&
      scope.request_body === false &&
      scope.response_body === false &&
      scope.request_headers === false &&
      scope.response_headers === false &&
      scope.url === false &&
      scope.websocket_messages === false &&
      scope.sse_events === false
    );
  }, []);

  const serializePanel = useCallback(() => {
    const hasAny =
      selectedClientIps.length > 0 ||
      selectedProxyPorts.length > 0 ||
      selectedClientApps.length > 0 ||
      selectedAccountNames.length > 0 ||
      selectedDomains.length > 0;
    if (!hasAny) return "";
    return encodeJsonForQueryParam({
      clientIps: selectedClientIps,
      proxyPorts: selectedProxyPorts,
      clientApps: selectedClientApps,
      accountNames: selectedAccountNames,
      domains: selectedDomains,
    });
  }, [selectedAccountNames, selectedClientApps, selectedClientIps, selectedDomains, selectedProxyPorts]);

  const deserializePanel = useCallback((str: string) => {
    const toStringArray = (input: unknown): string[] =>
      Array.isArray(input)
        ? input.filter(
            (x): x is string => typeof x === "string" && x.length > 0,
          )
        : [];
    const value = decodeJsonFromQueryParam<unknown>(str || "");
    if (!value || typeof value !== "object") {
      return { clientIps: [], proxyPorts: [], clientApps: [], accountNames: [], domains: [] };
    }
    const v = value as Record<string, unknown>;
    return {
      clientIps: toStringArray(v.clientIps),
      proxyPorts: toStringArray(v.proxyPorts),
      clientApps: toStringArray(v.clientApps),
      accountNames: toStringArray(v.accountNames),
      domains: toStringArray(v.domains),
    };
  }, []);

  const serializeSearch = useCallback(() => {
    const shouldPersist =
      searchMode === "search" ||
      searchKeyword.trim().length > 0 ||
      !isDefaultSearchScope(searchScope);
    if (!shouldPersist) return "";
    return encodeJsonForQueryParam({
      mode: searchMode,
      keyword: searchKeyword,
      scope: searchScope,
    });
  }, [isDefaultSearchScope, searchKeyword, searchMode, searchScope]);

  const deserializeSearch = useCallback((str: string) => {
    const value = decodeJsonFromQueryParam<unknown>(str || "");
    if (!value || typeof value !== "object") return null;
    const v = value as Record<string, unknown>;
    const mode: "normal" | "search" = v.mode === "search" ? "search" : "normal";
    const keyword = typeof v.keyword === "string" ? v.keyword : "";
    const scopeValue = v.scope;
    if (!scopeValue || typeof scopeValue !== "object") {
      return { mode, keyword, scope: null as SearchScope | null };
    }
    const s = scopeValue as Record<string, unknown>;
    const scope: SearchScope = {
      request_body: s.request_body === true,
      response_body: s.response_body === true,
      request_headers: s.request_headers === true,
      response_headers: s.response_headers === true,
      url: s.url === true,
      websocket_messages: s.websocket_messages === true,
      sse_events: s.sse_events === true,
      all: s.all !== false,
    };
    return { mode, keyword, scope };
  }, []);

  const handleSystemProxyToggle = useCallback(
    async (enabled: boolean) => {
      const success = await toggleSystemProxy(enabled);
      if (success) {
        message.success(
          enabled ? "System proxy enabled" : "System proxy disabled",
        );
      } else {
        const proxyError = useProxyStore.getState().error;
        message.error(proxyError || "Failed to toggle system proxy");
      }
    },
    [toggleSystemProxy],
  );

  useEffect(() => {
    const pending = pendingUrlUpdateRef.current;
    const pendingKeys = Object.keys(pending);
    if (
      pendingKeys.length > 0 &&
      pendingKeys.every((k) => (searchParams.get(k) || "") === pending[k])
    ) {
      pendingUrlUpdateRef.current = {};
      return;
    }

    const hasAnyStateParam = [
      FILTER_PARAM,
      TOOLBAR_PARAM,
      PANEL_PARAM,
      SEARCH_PARAM,
    ].some((k) => searchParams.has(k));
    if (!hasAnyStateParam) {
      return;
    }

    const filterParam = searchParams.get(FILTER_PARAM) || "";
    const toolbarParam = searchParams.get(TOOLBAR_PARAM) || "";
    const panelParam = searchParams.get(PANEL_PARAM) || "";
    const searchParam = searchParams.get(SEARCH_PARAM) || "";

    const filtersFromUrl = deserializeFilters(filterParam);
    const toolbarFromUrl = deserializeToolbar(toolbarParam);
    initFromUrl(filtersFromUrl, toolbarFromUrl);

    const panelFromUrl = deserializePanel(panelParam);
    setSelectedClientIps(panelFromUrl.clientIps);
    setSelectedProxyPorts(panelFromUrl.proxyPorts);
    setSelectedClientApps(panelFromUrl.clientApps);
    setSelectedAccountNames(panelFromUrl.accountNames);
    setSelectedDomains(panelFromUrl.domains);

    const searchFromUrl = deserializeSearch(searchParam);
    if (!searchFromUrl) {
      setSearchMode("normal");
      setSearchKeyword("");
      setSearchScope({ all: true });
      return;
    }

    setSearchMode(searchFromUrl.mode);
    setSearchKeyword(searchFromUrl.keyword);
    if (!searchFromUrl.scope) {
      setSearchScope({ all: true });
      return;
    }
    if (searchFromUrl.scope.all === true) {
      setSearchScope({ all: true });
      return;
    }
    setSearchScope(searchFromUrl.scope);
  }, [
    deserializePanel,
    deserializeSearch,
    initFromUrl,
    searchParams,
    setSearchKeyword,
    setSearchMode,
    setSearchScope,
    setSelectedClientApps,
    setSelectedAccountNames,
    setSelectedClientIps,
    setSelectedProxyPorts,
    setSelectedDomains,
  ]);

  useEffect(() => {
    const filterStr = serializeFilters(filterConditions);
    const toolbarStr = serializeToolbar(toolbarFilters);
    const panelStr = serializePanel();
    const searchStr = serializeSearch();
    const currentFilterStr = searchParams.get(FILTER_PARAM) || "";
    const currentToolbarStr = searchParams.get(TOOLBAR_PARAM) || "";
    const currentPanelStr = searchParams.get(PANEL_PARAM) || "";
    const currentSearchStr = searchParams.get(SEARCH_PARAM) || "";

    if (
      filterStr === currentFilterStr &&
      toolbarStr === currentToolbarStr &&
      panelStr === currentPanelStr &&
      searchStr === currentSearchStr
    ) {
      return;
    }

    pendingUrlUpdateRef.current = {
      [FILTER_PARAM]: filterStr,
      [TOOLBAR_PARAM]: toolbarStr,
      [PANEL_PARAM]: panelStr,
      [SEARCH_PARAM]: searchStr,
    };
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (filterStr) {
          next.set(FILTER_PARAM, filterStr);
        } else {
          next.delete(FILTER_PARAM);
        }

        if (toolbarStr) {
          next.set(TOOLBAR_PARAM, toolbarStr);
        } else {
          next.delete(TOOLBAR_PARAM);
        }

        if (panelStr) {
          next.set(PANEL_PARAM, panelStr);
        } else {
          next.delete(PANEL_PARAM);
        }

        if (searchStr) {
          next.set(SEARCH_PARAM, searchStr);
        } else {
          next.delete(SEARCH_PARAM);
        }

        return next;
      },
      { replace: true },
    );
  }, [
    filterConditions,
    searchKeyword,
    searchMode,
    searchScope,
    selectedClientApps,
    selectedAccountNames,
    selectedClientIps,
    selectedProxyPorts,
    selectedDomains,
    serializePanel,
    serializeSearch,
    setSearchParams,
    searchParams,
    toolbarFilters,
  ]);

  const lastAutoFetchSelectedIdRef = useRef<string | null>(null);
  const previousDetachedRef = useRef(detailDetached);

  useEffect(() => {
    if (!selectedId) {
      lastAutoFetchSelectedIdRef.current = null;
      return;
    }
    if (lastAutoFetchSelectedIdRef.current === selectedId) {
      return;
    }
    if (currentRecord?.id === selectedId) {
      lastAutoFetchSelectedIdRef.current = selectedId;
      return;
    }
    lastAutoFetchSelectedIdRef.current = selectedId;
    fetchTrafficDetail(selectedId);
  }, [currentRecord?.id, fetchTrafficDetail, selectedId]);

  useEffect(() => {
    if (detailDetached) {
      if (!detailPanelCollapsed) {
        setDetailPanelCollapsed(true);
      }
    } else if (previousDetachedRef.current) {
      setDetailPanelCollapsed(false);
    }

    previousDetachedRef.current = detailDetached;
  }, [detailDetached, detailPanelCollapsed, setDetailPanelCollapsed]);

  useEffect(() => {
    if (!detailDetached) {
      return;
    }

    const timer = window.setInterval(() => {
      const popup = detachedPopupRef.current;
      if (!popup || popup.closed) {
        detachedPopupRef.current = null;
        attachDetailWindow();
      }
    }, 400);

    return () => {
      window.clearInterval(timer);
    };
  }, [attachDetailWindow, detailDetached]);

  const handleSelect = useCallback(
    (record: TrafficSummary) => {
      setSelectedId(record.id);
    },
    [setSelectedId],
  );

  const handleClearAll = useCallback(async () => {
    const success = await clearTraffic();
    if (success) {
      message.success("Traffic cleared");
      setSelectedId(undefined);
    }
  }, [clearTraffic, setSelectedId]);

  const handleFilterConditionsChange = useCallback(
    (conditions: FilterCondition[]) => {
      setFilterConditions(conditions);
    },
    [setFilterConditions],
  );

  const handleDetailPanelToggle = useCallback(() => {
    setDetailPanelCollapsed(!detailPanelCollapsed);
  }, [detailPanelCollapsed, setDetailPanelCollapsed]);

  const handleFilterPanelToggle = useCallback(() => {
    setFilterPanelCollapsed(!filterPanelCollapsed);
  }, [filterPanelCollapsed, setFilterPanelCollapsed]);

  const handleDoubleClick = useCallback(
    (record: TrafficSummary) => {
      setSelectedId(record.id);
      if (detailPanelCollapsed) {
        setDetailPanelCollapsed(false);
      }
    },
    [detailPanelCollapsed, setDetailPanelCollapsed, setSelectedId],
  );

  const handleOpenDetailInNewWindow = useCallback((record: TrafficSummary) => {
    setSelectedId(record.id);

    const popupId =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `traffic-detail-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
    const url = buildAppRouteUrl(
      `/traffic/detail?detached=1&popupId=${encodeURIComponent(popupId)}&id=${encodeURIComponent(record.id)}`,
    );

    const existingPopup = detachedPopupRef.current;
    if (existingPopup && !existingPopup.closed) {
      detachDetailWindow(popupId);
      existingPopup.location.href = url;
      existingPopup.focus();
      return;
    }

    const popup = window.open(
      url,
      "_blank",
      "popup=yes,width=1440,height=900",
    );
    if (!popup) {
      message.error("Failed to open detail window");
      return;
    }

    detachedPopupRef.current = popup;
    detachDetailWindow(popupId);
    popup.focus();
  }, [detachDetailWindow, setSelectedId]);

  const handleAttachDetailWindow = useCallback(() => {
    attachDetailWindow();
    detachedPopupRef.current?.close();
    detachedPopupRef.current = null;
  }, [attachDetailWindow]);

  const handleScrollPositionChange = useCallback(
    (isAtBottom: boolean) => {
      setAutoScroll(isAtBottom);
    },
    [setAutoScroll],
  );

  const handleScrollToBottom = useCallback(() => {
    clearNewRecordsCount();
  }, [clearNewRecordsCount]);

  const handleScrollTopChange = useCallback(
    (newScrollTop: number) => {
      setScrollTop(newScrollTop);
    },
    [setScrollTop],
  );

  const handleSearchModeToggle = useCallback(() => {
    setSearchMode(searchMode === "search" ? "normal" : "search");
  }, [searchMode, setSearchMode]);

  const handleOpenPerformanceSettings = useCallback(() => {
    navigate("/settings?tab=performance&highlight=super-performance-mode");
  }, [navigate]);

  const panelFilters = useMemo<PanelFilters>(
    () => ({
      clientIps: selectedClientIps,
      proxyPorts: selectedProxyPorts,
      clientApps: selectedClientApps,
      accountNames: selectedAccountNames,
      domains: selectedDomains,
    }),
    [selectedClientIps, selectedProxyPorts, selectedClientApps, selectedAccountNames, selectedDomains],
  );

  const deferredToolbarFilters = useDeferredValue(toolbarFilters);
  const deferredFilterConditions = useDeferredValue(filterConditions);
  const deferredPanelFilters = useDeferredValue(panelFilters);
  const [filteredRecords, setFilteredRecords] = useState<TrafficSummary[]>([]);
  const appliedMutationVersionRef = useRef(-1);

  useEffect(() => {
    const needsFullRefilter =
      recordsMutation.reset ||
      recordsMutation.version < appliedMutationVersionRef.current;

    if (needsFullRefilter) {
      appliedMutationVersionRef.current = recordsMutation.version;
      setFilteredRecords(
        filterRecords(
          records,
          deferredToolbarFilters,
          deferredFilterConditions,
          deferredPanelFilters,
        ),
      );
      return;
    }

    if (recordsMutation.version === appliedMutationVersionRef.current) {
      return;
    }

    appliedMutationVersionRef.current = recordsMutation.version;
    setFilteredRecords((current) =>
      applyTrafficRecordsMutationToFilteredRecords(
        current,
        recordsMutation,
        deferredToolbarFilters,
        deferredFilterConditions,
        deferredPanelFilters,
      ),
    );
  }, [recordsMutation]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    setFilteredRecords(
      filterRecords(
        records,
        deferredToolbarFilters,
        deferredFilterConditions,
        deferredPanelFilters,
      ),
    );
  }, [deferredFilterConditions, deferredPanelFilters, deferredToolbarFilters, records]);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => {
      const macDesktopShell = isMacDesktopShell();
      return {
        container: {
          display: "flex",
          flexDirection: "column",
          height: "100%",
          overflow: "hidden",
          backgroundColor: macDesktopShell ? "transparent" : token.colorBgContainer,
        },
        filterBarWrapper: {
          padding: "8px 16px",
          backgroundColor: token.colorBgContainer,
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
        },
        mainContent: {
          flex: 1,
          overflow: "hidden",
          backgroundColor: macDesktopShell ? "transparent" : token.colorBgContainer,
        },
        centerWrapper: {
          display: "flex",
          flexDirection: "column",
          height: "100%",
          overflow: "hidden",
        },
        tableWrapper: {
          flex: 1,
          minHeight: 0,
          position: "relative",
          backgroundColor: token.colorBgContainer,
        },
        superPerformanceOverlay: {
          position: "absolute",
          inset: 0,
          zIndex: 5,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: 24,
          background: `${token.colorBgContainer}cc`,
          backdropFilter: "blur(12px) saturate(1.2)",
          WebkitBackdropFilter: "blur(12px) saturate(1.2)",
        },
        superPerformanceAlert: {
          maxWidth: 620,
          boxShadow: token.boxShadowSecondary,
        },
        detailWrapper: {
          height: "100%",
          padding: 4,
          backgroundColor: token.colorBgContainer,
          overflow: "auto",
        },
      };
    },
    [token],
  );

  const renderCenter = () => (
    <div style={styles.centerWrapper}>
      {showFilterBar && (
        <div style={styles.filterBarWrapper}>
          <FilterBar
            filters={filterConditions}
            onFiltersChange={handleFilterConditionsChange}
            availableClientApps={clientInfo.apps}
            availableClientIps={clientInfo.ips}
            onSearchModeToggle={handleSearchModeToggle}
            isSearchMode={searchMode === "search"}
          />
        </div>
      )}
      <div style={styles.tableWrapper}>
        {searchMode === "search" ? (
          <SearchMode
            onSelect={handleSelect}
            onDoubleClick={handleDoubleClick}
            selectedId={selectedId}
          />
        ) : (
          <VirtualTrafficTable
            data={filteredRecords}
            onSelect={handleSelect}
            onDoubleClick={handleDoubleClick}
            selectedId={selectedId}
            selectedIds={selectedIds}
            onSelectedIdsChange={setSelectedIds}
            hasMore={hasMore}
            autoScroll={autoScroll}
            onScrollPositionChange={handleScrollPositionChange}
            newRecordsCount={newRecordsCount}
            onScrollToBottom={handleScrollToBottom}
            initialScrollTop={scrollTop}
            onScrollTopChange={handleScrollTopChange}
          />
        )}
        {superPerformanceMode && (
          <div
            data-testid="traffic-super-performance-overlay"
            style={styles.superPerformanceOverlay}
          >
            <Alert
              type="warning"
              showIcon
              message="Super performance mode is enabled"
              description="Bifrost is processing proxy rules without storing traffic records, bodies, WebSocket frames, or traffic database updates. Turn it off to use Network recording and traffic inspection again."
              style={styles.superPerformanceAlert}
              action={
                <Button
                  type="primary"
                  onClick={handleOpenPerformanceSettings}
                  data-testid="traffic-super-performance-disable-button"
                >
                  Open Performance Settings
                </Button>
              }
            />
          </div>
        )}
      </div>
    </div>
  );

  const renderDetail = () => (
    <div style={styles.detailWrapper}>
      <TrafficDetail
        record={currentRecord}
        requestBody={requestBody}
        responseBody={responseBody}
        requestRawBody={requestRawBody}
        responseRawBody={responseRawBody}
        loading={detailLoading}
        error={detailError}
        onOpenInNewWindow={handleOpenDetailInNewWindow}
        onSelectById={setSelectedId}
      />
    </div>
  );

  const renderFilterPanel = () => (
    <FilterPanel
      availableClientIps={clientInfo.ips}
      availableProxyPorts={clientInfo.proxyPorts}
      availableClientApps={clientInfo.apps}
      availableAccountNames={clientInfo.accounts}
      availableDomains={clientInfo.domains}
      clientIpCounts={clientInfo.ipCounts}
      proxyPortCounts={clientInfo.proxyPortCounts}
      clientAppCounts={clientInfo.appCounts}
      accountNameCounts={clientInfo.accountCounts}
      domainCounts={clientInfo.domainCounts}
    />
  );

  return (
    <div style={styles.container}>
      <Toolbar
        filters={toolbarFilters}
        onClearAll={handleClearAll}
        onFilterChange={setToolbarFilters}
        systemProxyEnabled={
          systemProxy ? isSystemProxyLiveEnabledByBifrost(systemProxy) : false
        }
        systemProxySupported={systemProxy?.supported}
        systemProxyLoading={systemProxyLoading}
        onSystemProxyToggle={handleSystemProxyToggle}
        filterPanelCollapsed={filterPanelCollapsed}
        onFilterPanelToggle={handleFilterPanelToggle}
        detailPanelCollapsed={detailPanelCollapsed}
        onDetailPanelToggle={handleDetailPanelToggle}
        detailDetached={detailDetached}
        onAttachDetailWindow={handleAttachDetailWindow}
        breakpointEnabled={breakpointEnabled}
        breakpointLoading={breakpointLoading}
        onBreakpointToggle={toggleBreakpoint}
      />

      <div style={styles.mainContent}>
        {filterPanelInitialized ? (
          <ThreeSplitPane
            left={renderFilterPanel()}
            center={renderCenter()}
            right={renderDetail()}
            leftWidth={filterPanelWidth}
            minLeftWidth={180}
            maxLeftWidth={350}
            minCenterWidth={400}
            minRightWidth={350}
            leftCollapsed={filterPanelCollapsed}
            rightCollapsed={detailPanelCollapsed}
            onLeftWidthChange={setFilterPanelWidth}
            keepRightMountedWhenCollapsed
          />
        ) : (
          <div style={{ flex: 1 }} />
        )}
      </div>
    </div>
  );
}
