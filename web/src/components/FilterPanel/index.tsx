import { useMemo, useEffect, type CSSProperties } from "react";
import { theme, Button, Empty, Tooltip, Input } from "antd";
import { ClearOutlined, SearchOutlined, CloseCircleFilled } from "@ant-design/icons";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import { useTlsConfigStore } from "../../stores/useTlsConfigStore";
import FilterSection from "./FilterSection";
import PinnedFilters from "./PinnedFilters";
import FilterItem from "./FilterItem";
import AppIcon from "../AppIcon";
import { isMacDesktopShell } from "../../runtime";

interface FilterPanelProps {
  availableClientIps: string[];
  availableClientApps: string[];
  availableDomains: string[];
  clientIpCounts: Map<string, number>;
  clientAppCounts: Map<string, number>;
  domainCounts: Map<string, number>;
}

export default function FilterPanel({
  availableClientIps,
  availableClientApps,
  availableDomains,
  clientIpCounts,
  clientAppCounts,
  domainCounts,
}: FilterPanelProps) {
  const { token } = theme.useToken();
  const panelBackground = isMacDesktopShell() ? "transparent" : token.colorBgContainer;
  const headerBackground = isMacDesktopShell() ? "transparent" : token.colorBgLayout;
  const { fetchConfig } = useTlsConfigStore();

  useEffect(() => {
    fetchConfig();
  }, [fetchConfig]);

  const {
    pinnedFilters,
    selectedClientIps,
    selectedClientApps,
    selectedDomains,
    collapsedSections,
    searchKeyword,
    toggleClientIp,
    toggleClientApp,
    toggleDomain,
    addPinnedFilter,
    setCollapsedSection,
    clearAllSelections,
    setSearchKeyword,
  } = useFilterPanelStore();

  const hasSelections =
    selectedClientIps.length > 0 ||
    selectedClientApps.length > 0 ||
    selectedDomains.length > 0;

  const selectionSummary = useMemo(() => {
    const parts: string[] = [];
    if (selectedClientApps.length > 0) {
      parts.push(`App ${selectedClientApps.length}`);
    }
    if (selectedDomains.length > 0) {
      parts.push(`Domain ${selectedDomains.length}`);
    }
    if (selectedClientIps.length > 0) {
      parts.push(`IP ${selectedClientIps.length}`);
    }
    return parts.join(" · ");
  }, [selectedClientApps.length, selectedClientIps.length, selectedDomains.length]);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        flexDirection: "column",
        height: "100%",
        minHeight: 0,
        backgroundColor: panelBackground,
        borderRight: `1px solid ${token.colorBorderSecondary}`,
      },
      header: {
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "8px 12px",
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: headerBackground,
        flexShrink: 0,
        gap: 8,
      },
      searchWrapper: {
        padding: "6px 8px",
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: panelBackground,
        flexShrink: 0,
      },
      title: {
        fontSize: 13,
        fontWeight: 600,
        color: token.colorText,
        margin: 0,
      },
      titleGroup: {
        display: "flex",
        alignItems: "baseline",
        gap: 8,
        minWidth: 0,
        flex: 1,
      },
      summary: {
        fontSize: 11,
        color: token.colorTextSecondary,
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis",
      },
      content: {
        flex: 1,
        minHeight: 0,
        overflowY: "auto",
        overflowX: "hidden",
        padding: "4px 0",
      },
      emptyText: {
        color: token.colorTextSecondary,
        fontSize: 12,
        padding: "8px 12px",
      },
    }),
    [headerBackground, panelBackground, token]
  );

  const sortedClientIps = useMemo(() => {
    return [...availableClientIps].sort((a, b) => {
      if (a === "127.0.0.1") return -1;
      if (b === "127.0.0.1") return 1;
      if (a.startsWith("192.168.") && !b.startsWith("192.168.")) return -1;
      if (!a.startsWith("192.168.") && b.startsWith("192.168.")) return 1;
      return a.localeCompare(b);
    });
  }, [availableClientIps]);

  const sortedClientApps = useMemo(() => {
    return [...availableClientApps].sort((a, b) => a.localeCompare(b));
  }, [availableClientApps]);

  const sortedDomains = useMemo(() => {
    return [...availableDomains].sort((a, b) => a.localeCompare(b));
  }, [availableDomains]);

  const getIpLabel = (ip: string) => {
    if (ip === "127.0.0.1") return "Local (127.0.0.1)";
    return ip;
  };

  const filteredClientIps = useMemo(() => {
    if (!searchKeyword.trim()) return sortedClientIps;
    const keyword = searchKeyword.toLowerCase();
    return sortedClientIps.filter((ip) => getIpLabel(ip).toLowerCase().includes(keyword));
  }, [sortedClientIps, searchKeyword]);

  const filteredClientApps = useMemo(() => {
    if (!searchKeyword.trim()) return sortedClientApps;
    const keyword = searchKeyword.toLowerCase();
    return sortedClientApps.filter((app) => app.toLowerCase().includes(keyword));
  }, [sortedClientApps, searchKeyword]);

  const filteredDomains = useMemo(() => {
    if (!searchKeyword.trim()) return sortedDomains;
    const keyword = searchKeyword.toLowerCase();
    return sortedDomains.filter((domain) => domain.toLowerCase().includes(keyword));
  }, [sortedDomains, searchKeyword]);

  const hasSearchResults = filteredClientIps.length > 0 || filteredClientApps.length > 0 || filteredDomains.length > 0;
  const isSearching = searchKeyword.trim().length > 0;

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.titleGroup}>
          <span style={styles.title}>Filters</span>
          {selectionSummary && <span style={styles.summary}>{selectionSummary}</span>}
        </div>
        <Tooltip title="Clear all selections">
          <Button
            type="text"
            size="small"
            icon={<ClearOutlined />}
            onClick={clearAllSelections}
            style={{
              visibility: hasSelections ? "visible" : "hidden",
            }}
          />
        </Tooltip>
      </div>
      <div style={styles.searchWrapper}>
        <Input
          placeholder="Search filters..."
          prefix={<SearchOutlined style={{ color: token.colorTextSecondary }} />}
          suffix={
            searchKeyword && (
              <CloseCircleFilled
                style={{ color: token.colorTextQuaternary, cursor: "pointer" }}
                onClick={() => setSearchKeyword("")}
              />
            )
          }
          value={searchKeyword}
          onChange={(e) => setSearchKeyword(e.target.value)}
          allowClear={false}
          size="small"
          style={{ borderRadius: 6 }}
        />
      </div>
      <div style={styles.content}>
        {pinnedFilters.length > 0 && !isSearching && (
          <FilterSection
            title="Pinned"
            icon="📌"
            collapsed={collapsedSections.pinned}
            onToggle={() => setCollapsedSection("pinned", !collapsedSections.pinned)}
          >
            <PinnedFilters
              clientIpCounts={clientIpCounts}
              clientAppCounts={clientAppCounts}
              domainCounts={domainCounts}
            />
          </FilterSection>
        )}

        {isSearching && !hasSearchResults && (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={`No filters matching "${searchKeyword}"`}
            style={{ margin: "24px 0" }}
          />
        )}

        {(!isSearching || filteredClientIps.length > 0) && (
          <FilterSection
            title="Client IP"
            collapsed={isSearching ? false : collapsedSections.clientIp}
            onToggle={() => setCollapsedSection("clientIp", !collapsedSections.clientIp)}
            count={isSearching ? filteredClientIps.length : sortedClientIps.length}
          >
            {filteredClientIps.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No clients"
                style={{ margin: "12px 0" }}
              />
            ) : (
              filteredClientIps.map((ip) => (
                <FilterItem
                  key={ip}
                  label={getIpLabel(ip)}
                  value={ip}
                  type="client_ip"
                  selected={selectedClientIps.includes(ip)}
                  onSelect={() => toggleClientIp(ip)}
                  onPin={() =>
                    addPinnedFilter({
                      type: "client_ip",
                      value: ip,
                      label: getIpLabel(ip),
                    })
                  }
                  count={clientIpCounts.get(ip) ?? 0}
                  searchKeyword={searchKeyword}
                />
              ))
            )}
          </FilterSection>
        )}

        {(!isSearching || filteredClientApps.length > 0) && (
          <FilterSection
            title="Applications"
            collapsed={isSearching ? false : collapsedSections.clientApp}
            onToggle={() => setCollapsedSection("clientApp", !collapsedSections.clientApp)}
            count={isSearching ? filteredClientApps.length : sortedClientApps.length}
          >
            {filteredClientApps.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No applications"
                style={{ margin: "12px 0" }}
              />
            ) : (
              filteredClientApps.map((app) => (
                <FilterItem
                  key={app}
                  label={app}
                  value={app}
                  type="client_app"
                  selected={selectedClientApps.includes(app)}
                  onSelect={() => toggleClientApp(app)}
                  onPin={() =>
                    addPinnedFilter({
                      type: "client_app",
                      value: app,
                      label: app,
                    })
                  }
                  count={clientAppCounts.get(app) ?? 0}
                  icon={<AppIcon appName={app} size={16} />}
                  searchKeyword={searchKeyword}
                />
              ))
            )}
          </FilterSection>
        )}

        {(!isSearching || filteredDomains.length > 0) && (
          <FilterSection
            title="Domains"
            collapsed={isSearching ? false : collapsedSections.domain}
            onToggle={() => setCollapsedSection("domain", !collapsedSections.domain)}
            count={isSearching ? filteredDomains.length : sortedDomains.length}
          >
            {filteredDomains.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No domains"
                style={{ margin: "12px 0" }}
              />
            ) : (
              filteredDomains.map((domain) => (
                <FilterItem
                  key={domain}
                  label={domain}
                  value={domain}
                  type="domain"
                  selected={selectedDomains.includes(domain)}
                  onSelect={() => toggleDomain(domain)}
                  onPin={() =>
                    addPinnedFilter({
                      type: "domain",
                      value: domain,
                      label: domain,
                    })
                  }
                  count={domainCounts.get(domain) ?? 0}
                  searchKeyword={searchKeyword}
                />
              ))
            )}
          </FilterSection>
        )}
      </div>
    </div>
  );
}
