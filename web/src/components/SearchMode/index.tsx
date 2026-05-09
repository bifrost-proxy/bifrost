import { useCallback, useMemo, type CSSProperties } from "react";
import {
  Input,
  Button,
  Checkbox,
  Empty,
  Spin,
  Typography,
  Space,
  theme,
} from "antd";
import {
  SearchOutlined,
  CloseOutlined,
  LoadingOutlined,
  StopOutlined,
} from "@ant-design/icons";
import { useSearchStore, compactToSummary } from "../../stores/useSearchStore";
import {
  isFilterConditionApplicable,
  useTrafficStore,
} from "../../stores/useTrafficStore";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import type {
  SearchFilters,
  SearchResultItem,
  TrafficSummary,
} from "../../types";
import SearchResultsList from "./SearchResultsList";

const { Text } = Typography;

interface SearchModeProps {
  onSelect: (record: TrafficSummary) => void;
  onDoubleClick: (record: TrafficSummary) => void;
  selectedId?: string;
}

export default function SearchMode({
  onSelect,
  onDoubleClick,
  selectedId,
}: SearchModeProps) {
  const { token } = theme.useToken();

  const {
    keyword,
    scope,
    results,
    totalSearched,
    totalMatched,
    hasMore,
    isSearching,
    isLoadingMore,
    setKeyword,
    setScope,
    search,
    loadMore,
    cancelSearch,
    setMode,
  } = useSearchStore();

  const toolbarFilters = useTrafficStore((state) => state.toolbarFilters);
  const filterConditions = useTrafficStore((state) => state.filterConditions);
  const selectedClientIps = useFilterPanelStore(
    (state) => state.selectedClientIps,
  );
  const selectedClientApps = useFilterPanelStore(
    (state) => state.selectedClientApps,
  );
  const selectedDomains = useFilterPanelStore((state) => state.selectedDomains);

  const buildFilters = useCallback((): SearchFilters => {
    const importedApps =
      toolbarFilters.imported.length > 0 ? ["Bifrost Import"] : [];
    const allClientApps = [
      ...new Set([...selectedClientApps, ...importedApps]),
    ];

    return {
      protocols: toolbarFilters.protocol,
      status_ranges: toolbarFilters.status,
      content_types: toolbarFilters.type,
      has_rule_hit: toolbarFilters.rule.length > 0 ? true : undefined,
      conditions: filterConditions
        .filter(isFilterConditionApplicable)
        .map(({ field, operator, value }) => ({
          field,
          operator,
          value,
        })),
      client_ips: selectedClientIps,
      client_apps: allClientApps,
      domains: selectedDomains,
    };
  }, [
    filterConditions,
    toolbarFilters,
    selectedClientIps,
    selectedClientApps,
    selectedDomains,
  ]);

  const handleSearch = useCallback(() => {
    if (
      keyword.trim() ||
      filterConditions.some(isFilterConditionApplicable) ||
      selectedClientIps.length > 0 ||
      selectedClientApps.length > 0 ||
      selectedDomains.length > 0 ||
      toolbarFilters.protocol.length > 0 ||
      toolbarFilters.status.length > 0 ||
      toolbarFilters.type.length > 0 ||
      toolbarFilters.rule.length > 0 ||
      toolbarFilters.imported.length > 0
    ) {
      search(buildFilters());
    }
  }, [
    keyword,
    filterConditions,
    selectedClientIps,
    selectedClientApps,
    selectedDomains,
    toolbarFilters,
    search,
    buildFilters,
  ]);

  const canSearch =
    keyword.trim().length > 0 ||
    filterConditions.some(isFilterConditionApplicable) ||
    selectedClientIps.length > 0 ||
    selectedClientApps.length > 0 ||
    selectedDomains.length > 0 ||
    toolbarFilters.protocol.length > 0 ||
    toolbarFilters.status.length > 0 ||
    toolbarFilters.type.length > 0 ||
    toolbarFilters.rule.length > 0 ||
    toolbarFilters.imported.length > 0;

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        handleSearch();
      }
    },
    [handleSearch],
  );

  const handleLoadMore = useCallback(() => {
    loadMore(buildFilters());
  }, [loadMore, buildFilters]);

  const handleExitSearch = useCallback(() => {
    setMode("normal");
  }, [setMode]);

  const handleCancelSearch = useCallback(() => {
    cancelSearch();
  }, [cancelSearch]);

  const handleResultSelect = useCallback(
    (item: SearchResultItem) => {
      const summary = compactToSummary(item.record);
      onSelect(summary);
    },
    [onSelect],
  );

  const handleResultDoubleClick = useCallback(
    (item: SearchResultItem) => {
      const summary = compactToSummary(item.record);
      onDoubleClick(summary);
    },
    [onDoubleClick],
  );

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
      },
      header: {
        padding: "12px 16px",
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: token.colorBgContainer,
      },
      searchRow: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        marginBottom: 8,
      },
      scopeRow: {
        display: "flex",
        alignItems: "center",
        gap: 12,
        flexWrap: "wrap",
      },
      scopeLabel: {
        color: token.colorTextSecondary,
        fontSize: 12,
      },
      results: {
        flex: 1,
        overflow: "hidden",
      },
      statsRow: {
        padding: "8px 16px",
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: token.colorBgLayout,
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
      },
      emptyWrapper: {
        flex: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      },
    }),
    [token],
  );

  const scopeOptions = [
    { key: "all", label: "All" },
    { key: "url", label: "URL" },
    { key: "request_headers", label: "Req Headers" },
    { key: "response_headers", label: "Res Headers" },
    { key: "request_body", label: "Req Body" },
    { key: "response_body", label: "Res Body" },
    { key: "websocket_messages", label: "WS Messages" },
    { key: "sse_events", label: "SSE Events" },
  ];

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.searchRow}>
          <Input
            placeholder="Enter keyword to search all content..."
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
            onKeyDown={handleKeyDown}
            prefix={
              <SearchOutlined style={{ color: token.colorTextSecondary }} />
            }
            suffix={
              keyword ? (
                <CloseOutlined
                  onClick={() => setKeyword("")}
                  style={{ color: token.colorTextSecondary, cursor: "pointer" }}
                />
              ) : null
            }
            style={{ flex: 1 }}
          />
          <Button
            type="primary"
            onClick={handleSearch}
            icon={<SearchOutlined />}
            disabled={!canSearch}
          >
            Search
          </Button>
          {(isSearching || isLoadingMore) && (
            <Button
              onClick={handleCancelSearch}
              icon={<StopOutlined />}
              danger
            >
              Stop
            </Button>
          )}
          <Button onClick={handleExitSearch}>Exit</Button>
        </div>
        <div style={styles.scopeRow}>
          <span style={styles.scopeLabel}>Search in:</span>
          {scopeOptions.map((opt) => (
            <Checkbox
              key={opt.key}
              checked={
                opt.key === "all"
                  ? scope.all
                  : scope[opt.key as keyof typeof scope]
              }
              onChange={(e) => {
                if (opt.key === "all") {
                  setScope({ all: e.target.checked });
                } else {
                  setScope({ [opt.key]: e.target.checked });
                }
              }}
            >
              {opt.label}
            </Checkbox>
          ))}
        </div>
      </div>

      {results.length > 0 && (
        <div style={styles.statsRow}>
          <Space>
            <Text type="secondary">
              Found <Text strong>{totalMatched}</Text> matches
            </Text>
            <Text type="secondary">(searched {totalSearched} records)</Text>
            {isSearching && (
              <Space size={6}>
                <LoadingOutlined style={{ color: token.colorTextSecondary }} />
                <Text type="secondary">Searching...</Text>
              </Space>
            )}
          </Space>
          {hasMore && (
            <Button
              size="small"
              onClick={handleLoadMore}
              loading={isLoadingMore}
            >
              Load More
            </Button>
          )}
        </div>
      )}

      <div style={styles.results}>
        {isSearching && results.length === 0 ? (
          <div style={styles.emptyWrapper}>
            <Space direction="vertical" align="center" size={8}>
              <Spin indicator={<LoadingOutlined spin />} tip="Searching..." />
              <Text type="secondary">searched {totalSearched} records</Text>
              <Button onClick={handleCancelSearch} icon={<StopOutlined />} danger>
                Stop
              </Button>
            </Space>
          </div>
        ) : results.length === 0 ? (
          <div style={styles.emptyWrapper}>
            <Empty
              description={
                keyword.trim()
                  ? "No results found. Try a different keyword."
                  : "Enter a keyword to search all traffic content."
              }
            />
          </div>
        ) : (
          <SearchResultsList
            results={results}
            keyword={keyword}
            selectedId={selectedId}
            onSelect={handleResultSelect}
            onDoubleClick={handleResultDoubleClick}
            onLoadMore={handleLoadMore}
            hasMore={hasMore}
            isLoadingMore={isLoadingMore}
          />
        )}
      </div>
    </div>
  );
}
