import { Tag, Switch, Button, Space, theme, Tooltip, Popover } from "antd";
import {
  DeleteOutlined,
  ExportOutlined,
  ImportOutlined,
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  FilterOutlined,
} from "@ant-design/icons";
import type { ToolbarFilters } from "../../types";

interface ToolbarProps {
  filters: ToolbarFilters;
  onClearAll: () => void;
  onFilterChange: (filters: ToolbarFilters) => void;
  systemProxyEnabled?: boolean;
  systemProxySupported?: boolean;
  systemProxyLoading?: boolean;
  onSystemProxyToggle?: (enabled: boolean) => void;
  filterPanelCollapsed?: boolean;
  onFilterPanelToggle?: () => void;
  detailPanelCollapsed?: boolean;
  onDetailPanelToggle?: () => void;
  detailDetached?: boolean;
  onAttachDetailWindow?: () => void;
  breakpointEnabled?: boolean;
  breakpointLoading?: boolean;
  onBreakpointToggle?: (enabled: boolean) => void;
}

const filterGroups = {
  rule: ["Hit Rule"],
  protocol: ["HTTP", "HTTPS", "WS", "WSS", "H3"],
  type: ["JSON", "Form", "XML", "JS", "CSS", "Font", "Doc", "Media", "SSE"],
  status: ["1xx", "2xx", "3xx", "4xx", "5xx", "error"],
  imported: ["Imported"],
};

export default function Toolbar({
  filters,
  onClearAll,
  onFilterChange,
  systemProxyEnabled,
  systemProxySupported = true,
  systemProxyLoading,
  onSystemProxyToggle,
  filterPanelCollapsed,
  onFilterPanelToggle,
  detailPanelCollapsed,
  onDetailPanelToggle,
  detailDetached,
  onAttachDetailWindow,
  breakpointEnabled = false,
  breakpointLoading,
  onBreakpointToggle,
}: ToolbarProps) {
  const { token } = theme.useToken();

  const handleTagClick = (group: keyof ToolbarFilters, tag: string) => {
    const currentTags = filters[group];
    const newTags = currentTags.includes(tag) ? [] : [tag];
    onFilterChange({ ...filters, [group]: newTags });
  };

  const renderFilterGroup = (group: keyof ToolbarFilters, tags: string[]) => (
    <Space size={4} wrap style={{ marginRight: 8 }}>
      {tags.map((tag) => (
        <Tag.CheckableTag
          key={tag}
          checked={filters[group].includes(tag)}
          onChange={() => handleTagClick(group, tag)}
          style={{ marginInlineEnd: 0, fontSize: 12 }}
        >
          {tag}
        </Tag.CheckableTag>
      ))}
    </Space>
  );

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "6px 12px",
        background: token.colorBgContainer,
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        flexShrink: 0,
      }}
    >
      <Space size={4}>
        <Tooltip
          title={
            filterPanelCollapsed ? "Show filter panel" : "Hide filter panel"
          }
        >
          <Button
            type="text"
            size="small"
            icon={<FilterOutlined />}
            onClick={onFilterPanelToggle}
            style={{
              color: filterPanelCollapsed
                ? token.colorTextSecondary
                : token.colorPrimary,
            }}
          />
        </Tooltip>
        <div
          style={{
            width: 1,
            height: 16,
            backgroundColor: token.colorBorderSecondary,
            margin: "0 4px",
          }}
        />
        <Tooltip title="Clear all traffic">
          <Button
            type="text"
            size="small"
            icon={<DeleteOutlined />}
            onClick={onClearAll}
            data-testid="toolbar-clear-all"
          />
        </Tooltip>
        <div
          style={{
            width: 1,
            height: 16,
            backgroundColor: token.colorBorderSecondary,
            margin: "0 4px",
          }}
        />
      </Space>

      <div
        style={{
          display: "flex",
          alignItems: "center",
          flexWrap: "wrap",
          gap: 4,
          flex: 1,
          marginLeft: 8,
        }}
      >
        {renderFilterGroup("rule", filterGroups.rule)}
        <div
          style={{
            width: 1,
            height: 14,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        {renderFilterGroup("protocol", filterGroups.protocol)}
        <div
          style={{
            width: 1,
            height: 14,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        {renderFilterGroup("type", filterGroups.type)}
        <div
          style={{
            width: 1,
            height: 14,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        {renderFilterGroup("status", filterGroups.status)}
        <div
          style={{
            width: 1,
            height: 14,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        {renderFilterGroup("imported", filterGroups.imported)}
      </div>

      <Space size={8}>
        <Popover
          trigger="hover"
          content={
            <div style={{ maxWidth: 320, fontSize: 12, lineHeight: 1.5 }}>
              <div style={{ fontWeight: 600, marginBottom: 6 }}>
                Breakpoint global gate
              </div>
              <div>
                On: matched rules using <code>breakpoint://request</code> or{" "}
                <code>breakpoint://response</code> can pause traffic.
              </div>
              <div style={{ marginTop: 6 }}>
                Off: no new traffic pauses, and pending breakpoint requests are
                released.
              </div>
              <div style={{ marginTop: 6 }}>
                This switch alone does not pause traffic. Choose request,
                response, or both in the rule value.
              </div>
            </div>
          }
        >
          <span
            data-testid="toolbar-breakpoint-help"
            style={{
              color: token.colorTextSecondary,
              cursor: "help",
              fontSize: 12,
            }}
          >
            Breakpoint
          </span>
        </Popover>
        <Switch
          size="small"
          checked={breakpointEnabled}
          loading={breakpointLoading}
          onChange={onBreakpointToggle}
          data-testid="toolbar-breakpoint-toggle"
        />
        <div
          style={{
            width: 1,
            height: 16,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        <span style={{ color: token.colorTextSecondary, fontSize: 12 }}>
          System Proxy
        </span>
        {systemProxySupported ? (
          <Switch
            size="small"
            checked={systemProxyEnabled}
            loading={systemProxyLoading}
            onChange={onSystemProxyToggle}
          />
        ) : (
          <Tooltip title="System proxy is not supported on this platform">
            <Switch size="small" disabled />
          </Tooltip>
        )}
        <div
          style={{
            width: 1,
            height: 16,
            backgroundColor: token.colorBorderSecondary,
          }}
        />
        <Tooltip
          title={
            detailDetached
              ? "Attach detail window"
              : detailPanelCollapsed
                ? "Show detail panel"
                : "Hide detail panel"
          }
        >
          <Button
            type="text"
            size="small"
            icon={
              detailDetached ? (
                <ImportOutlined />
              ) : detailPanelCollapsed ? (
                <MenuUnfoldOutlined />
              ) : (
                <MenuFoldOutlined />
              )
            }
            onClick={detailDetached ? onAttachDetailWindow : onDetailPanelToggle}
            data-testid="toolbar-detail-toggle"
          />
        </Tooltip>
        {detailDetached ? (
          <Tooltip title="Detail is detached to a separate window">
            <ExportOutlined
              style={{ color: token.colorPrimary, fontSize: 14 }}
            />
          </Tooltip>
        ) : null}
      </Space>
    </div>
  );
}
