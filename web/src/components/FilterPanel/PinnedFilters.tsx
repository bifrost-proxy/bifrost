import { useMemo, useState, type CSSProperties } from "react";
import { theme, Tooltip } from "antd";
import { CloseOutlined, CheckOutlined } from "@ant-design/icons";
import { useFilterPanelStore, isFilterSelected } from "../../stores/useFilterPanelStore";
import AppIcon from "../AppIcon";

interface PinnedFiltersProps {
  clientIpCounts: Map<string, number>;
  proxyPortCounts: Map<string, number>;
  clientAppCounts: Map<string, number>;
  accountNameCounts: Map<string, number>;
  domainCounts: Map<string, number>;
}

export default function PinnedFilters({
  clientIpCounts,
  proxyPortCounts,
  clientAppCounts,
  accountNameCounts,
  domainCounts,
}: PinnedFiltersProps) {
  const {
    pinnedFilters,
    removePinnedFilter,
    togglePinnedFilter,
  } = useFilterPanelStore();

  const filterState = useFilterPanelStore();

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        flexDirection: "column",
      },
    }),
    []
  );

  if (pinnedFilters.length === 0) {
    return null;
  }

  return (
    <div style={styles.container}>
      {pinnedFilters.map((filter) => {
        const isSelected = isFilterSelected(filterState, filter.type, filter.value);
        return (
          <PinnedFilterItem
            key={filter.id}
            label={filter.label}
            type={filter.type}
            value={filter.value}
            selected={isSelected}
            onToggle={() => togglePinnedFilter(filter.id)}
            onRemove={() => removePinnedFilter(filter.id)}
            count={
              filter.type === "client_ip"
                ? (clientIpCounts.get(filter.value) ?? 0)
                : filter.type === "proxy_port"
                  ? (proxyPortCounts.get(filter.value) ?? 0)
                : filter.type === "client_app"
                  ? (clientAppCounts.get(filter.value) ?? 0)
                : filter.type === "account_name"
                  ? (accountNameCounts.get(filter.value) ?? 0)
                  : (domainCounts.get(filter.value) ?? 0)
            }
          />
        );
      })}
    </div>
  );
}

interface PinnedFilterItemProps {
  label: string;
  type: string;
  value: string;
  selected: boolean;
  onToggle: () => void;
  onRemove: () => void;
  count: number;
}

function PinnedFilterItem({
  label,
  type,
  value,
  selected,
  onToggle,
  onRemove,
  count,
}: PinnedFilterItemProps) {
  const { token } = theme.useToken();
  const [isHovering, setIsHovering] = useState(false);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "5px 8px 5px 20px",
        cursor: "pointer",
        userSelect: "none",
        backgroundColor: selected ? token.colorPrimaryBg : "transparent",
        borderLeft: selected ? `2px solid ${token.colorPrimary}` : "2px solid transparent",
        transition: "all 0.15s",
      },
      containerHover: {
        backgroundColor: selected ? token.colorPrimaryBg : token.colorBgTextHover,
      },
      pinIcon: {
        fontSize: 12,
        flexShrink: 0,
      },
      icon: {
        flexShrink: 0,
      },
      label: {
        flex: 1,
        fontSize: 12,
        color: selected ? token.colorPrimary : token.colorText,
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap" as const,
      },
      count: {
        fontSize: 11,
        color: selected ? token.colorPrimary : token.colorTextSecondary,
        flexShrink: 0,
        minWidth: 24,
        textAlign: "right" as const,
        fontVariantNumeric: "tabular-nums",
      },
      typeTag: {
        fontSize: 10,
        color: token.colorTextSecondary,
        backgroundColor: token.colorBgTextHover,
        padding: "1px 4px",
        borderRadius: 2,
        flexShrink: 0,
      },
      checkIcon: {
        fontSize: 12,
        color: token.colorPrimary,
        flexShrink: 0,
      },
      closeBtn: {
        fontSize: 10,
        color: token.colorTextSecondary,
        padding: 2,
        borderRadius: 4,
        opacity: isHovering ? 1 : 0,
        transition: "opacity 0.15s",
        cursor: "pointer",
        flexShrink: 0,
      },
    }),
    [token, selected, isHovering]
  );

  const getTypeLabel = (filterType: string) => {
    switch (filterType) {
      case "client_ip":
        return "IP";
      case "proxy_port":
        return "Port";
      case "client_app":
        return "App";
      case "account_name":
        return "Account";
      case "domain":
        return "Host";
      default:
        return filterType;
    }
  };

  const renderIcon = () => {
    if (type === "client_app") {
      return <AppIcon appName={value} size={16} />;
    }
    return <span style={styles.pinIcon}>📌</span>;
  };

  return (
    <div
      style={{
        ...styles.container,
        ...(isHovering ? styles.containerHover : {}),
      }}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={() => setIsHovering(false)}
      onClick={onToggle}
    >
      <span style={styles.icon}>{renderIcon()}</span>
      <Tooltip title={label} placement="right" mouseEnterDelay={0.5}>
        <span style={styles.label}>{label}</span>
      </Tooltip>
      <span
        style={styles.count}
        data-testid={`pinned-filter-count-${type}`}
        aria-label={`${label} count`}
      >
        {count.toLocaleString()}
      </span>
      <span style={styles.typeTag}>{getTypeLabel(type)}</span>
      {selected && <CheckOutlined style={styles.checkIcon} />}
      <Tooltip title="Unpin">
        <span
          style={styles.closeBtn}
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor = token.colorErrorBg;
            e.currentTarget.style.color = token.colorError;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
            e.currentTarget.style.color = token.colorTextSecondary;
          }}
        >
          <CloseOutlined />
        </span>
      </Tooltip>
    </div>
  );
}
