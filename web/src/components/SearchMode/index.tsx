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
import { compactToSummary, useSearchStore } from "../../stores/useSearchStore";
import {
  isFilterConditionApplicable,
  useTrafficStore,
} from "../../stores/useTrafficStore";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import type { SearchResultItem, TrafficSummary } from "../../types";
import SearchResultsList from "./SearchResultsList";
import { buildSearchFilters } from "./searchFilters";
import { useI18n } from "../../i18n";

const { Text } = Typography;

interface SearchModeProps {
  onSelect: (record: TrafficSummary) => void;
  onDoubleClick: (record: TrafficSummary) => void;
  selectedId?: string;
  pendingOnly?: boolean;
  breakpointPhases?: Map<string, "request" | "response">;
}

export default function SearchMode({
  onSelect,
  onDoubleClick,
  selectedId,
  breakpointPhases,
  pendingOnly = false,
}: SearchModeProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();

  const {
    keyword,
    submittedKeyword,
    scope,
    results,
    totalSearched,
    totalMatched,
    hasMore,
    nextCursor,
    activeSearchKey,
    searchId,
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
  const selectedAccountNames = useFilterPanelStore(
    (state) => state.selectedAccountNames,
  );
  const selectedDomains = useFilterPanelStore((state) => state.selectedDomains);

  const buildFilters = buildSearchFilters;

  const handleSearch = useCallback(() => {
    if (
      keyword.trim() ||
      filterConditions.some(isFilterConditionApplicable) ||
      selectedClientIps.length > 0 ||
      selectedClientApps.length > 0 ||
      selectedAccountNames.length > 0 ||
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
    selectedAccountNames,
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
    selectedAccountNames.length > 0 ||
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
    { key: "all", label: t("common.all") },
    { key: "url", label: "URL" },
    { key: "request_headers", label: t("traffic.search.scopeReqHeaders") },
    { key: "response_headers", label: t("traffic.search.scopeResHeaders") },
    { key: "request_body", label: t("traffic.search.scopeReqBody") },
    { key: "response_body", label: t("traffic.search.scopeResBody") },
    { key: "websocket_messages", label: t("traffic.search.scopeWsMessages") },
    { key: "sse_events", label: t("traffic.search.scopeSseEvents") },
  ];

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.searchRow}>
          <Input
            placeholder={t("traffic.search.placeholder")}
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
            data-testid="search-mode-submit"
            type="primary"
            onClick={handleSearch}
            icon={<SearchOutlined />}
            disabled={!canSearch}
          >
            {t("common.search")}
          </Button>
          {(isSearching || isLoadingMore) && (
            <Button onClick={handleCancelSearch} icon={<StopOutlined />} danger>
              {t("common.stop")}
            </Button>
          )}
          <Button onClick={handleExitSearch}>{t("traffic.search.exit")}</Button>
        </div>
        <div style={styles.scopeRow}>
          <span style={styles.scopeLabel}>{t("traffic.search.searchIn")}</span>
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
              {t("traffic.search.foundPrefix")}{" "}
              <Text strong>{totalMatched}</Text>{" "}
              {t("traffic.search.foundSuffix")}
            </Text>
            <Text type="secondary">
              {t("traffic.search.searchedRecordsParen", {
                count: totalSearched,
              })}
            </Text>
            {isSearching && (
              <Space size={6}>
                <LoadingOutlined style={{ color: token.colorTextSecondary }} />
                <Text type="secondary">{t("traffic.search.searching")}</Text>
              </Space>
            )}
          </Space>
          {hasMore && (
            <Button
              size="small"
              onClick={handleLoadMore}
              loading={isLoadingMore}
            >
              {t("traffic.search.loadMore")}
            </Button>
          )}
        </div>
      )}

      <div style={styles.results}>
        {isSearching && results.length === 0 ? (
          <div style={styles.emptyWrapper}>
            <Space direction="vertical" align="center" size={8}>
              <Spin
                indicator={<LoadingOutlined spin />}
                tip={t("traffic.search.searching")}
              />
              <Text type="secondary">
                {t("traffic.search.searchedRecords", { count: totalSearched })}
              </Text>
              <Button
                onClick={handleCancelSearch}
                icon={<StopOutlined />}
                danger
              >
                {t("common.stop")}
              </Button>
            </Space>
          </div>
        ) : results.length === 0 && !hasMore ? (
          <div style={styles.emptyWrapper}>
            <Empty
              description={
                keyword.trim()
                  ? t("traffic.search.noResults")
                  : t("traffic.search.enterKeyword")
              }
            />
          </div>
        ) : (
          <SearchResultsList
            results={
              pendingOnly
                ? results.filter((item) =>
                    breakpointPhases?.has(item.record.id),
                  )
                : results
            }
            keyword={submittedKeyword}
            paginationKey={
              activeSearchKey && nextCursor !== null
                ? JSON.stringify([activeSearchKey, searchId, nextCursor])
                : null
            }
            selectedId={selectedId}
            breakpointPhases={breakpointPhases}
            onSelect={handleResultSelect}
            onDoubleClick={handleResultDoubleClick}
            onLoadMore={handleLoadMore}
            hasMore={hasMore}
            isLoadingMore={isLoadingMore || isSearching}
          />
        )}
      </div>
    </div>
  );
}
