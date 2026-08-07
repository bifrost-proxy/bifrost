import { useMemo, useRef, useState } from "react";
import { Button, Input, Table, Typography, theme, ConfigProvider, Space, Radio, Tag, Tooltip } from "antd";
import { DeleteOutlined, PlusOutlined } from "@ant-design/icons";
import type { ColumnsType } from "antd/es/table";
import type { SessionTargetSearchState } from "../../../../types";
import { useThemeStore } from "../../../../stores/useThemeStore";
import { useMarkSearch } from "../../hooks/useMarkSearch";
import { TunnelInterceptActions } from "../../TunnelInterceptActions";
import { areHeadersEqual, buildHeaderDiff } from "./diff";
import type { HeaderDiffItem } from "./diff";

const { Text } = Typography;

interface HeaderViewProps {
  headers: [string, string][] | null;
  originalHeaders?: [string, string][] | null;
  flow?: "request" | "response";
  testIdPrefix?: string;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  isTunnel?: boolean;
  host?: string;
  clientApp?: string;
  clientIp?: string;
  editable?: boolean;
  onHeadersChange?: (headers: [string, string][]) => void;
}

export const HeaderView = ({
  headers,
  originalHeaders,
  flow = "response",
  testIdPrefix = "header-view",
  searchValue,
  onSearch,
  isTunnel,
  host,
  clientApp,
  clientIp,
  editable = false,
  onHeadersChange,
}: HeaderViewProps) => {
  const { token } = theme.useToken();
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);
  const tableRef = useRef<HTMLDivElement>(null);
  const [viewMode, setViewMode] = useState<'current' | 'original'>('current');

  const showOriginalTab = headers != null && !!originalHeaders && !areHeadersEqual(headers, originalHeaders);
  const hasModifications = showOriginalTab;
  const effectiveHeaders = headers ?? originalHeaders ?? null;
  const resolvedViewMode = useMemo(() => {
    if (viewMode === 'original' && !showOriginalTab) {
      return 'current';
    }
    return viewMode;
  }, [showOriginalTab, viewMode]);

  const displayHeaders = useMemo(() => {
    if (resolvedViewMode === 'original' && originalHeaders) {
      return originalHeaders;
    }
    return effectiveHeaders;
  }, [resolvedViewMode, effectiveHeaders, originalHeaders]);

  const diffResult = useMemo(() => {
    if (!showOriginalTab || !headers || !originalHeaders) return null;
    return buildHeaderDiff(headers, originalHeaders);
  }, [headers, originalHeaders, showOriginalTab]);

  const dataSource = useMemo<HeaderDiffItem[]>(() => {
    if (!displayHeaders) return [];

    if (resolvedViewMode === 'current' && diffResult) {
      return diffResult.items;
    }

    return [...displayHeaders]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, value], index) => ({
        key: String(index),
        name,
        value,
        diffType: 'unchanged' as const,
      }));
  }, [diffResult, displayHeaders, resolvedViewMode]);

  const filteredData = useMemo(() => {
    if (!searchValue.value) return dataSource;
    const searchLower = searchValue.value.toLowerCase();
    return dataSource.filter(
      (item) =>
        item.name.toLowerCase().includes(searchLower) ||
        item.value.toLowerCase().includes(searchLower),
    );
  }, [dataSource, searchValue.value]);

  const protocolToken = useMemo(() => theme.getDesignToken({
    algorithm: resolvedTheme === "dark" ? theme.darkAlgorithm : theme.defaultAlgorithm,
  }), [resolvedTheme]);
  const diffColors = useMemo(() => ({
    added: { bg: token.colorSuccessBg, text: token.colorSuccess },
    modified: { bg: token.colorWarningBg, text: token.colorWarningText },
    deleted: { bg: token.colorErrorBg, text: token.colorError },
    protocol: { bg: protocolToken.colorInfoBg, text: protocolToken.colorInfo },
  }), [protocolToken, token]);

  useMarkSearch(searchValue, () => tableRef.current, onSearch);

  if (editable) {
    const editableHeaders = headers ?? [];
    const updateHeader = (index: number, field: 0 | 1, value: string) => {
      const next = editableHeaders.map(([name, headerValue]) => [name, headerValue] as [string, string]);
      next[index][field] = value;
      onHeadersChange?.(next);
    };
    return (
      <div ref={tableRef} data-testid={`${testIdPrefix}-editor`}>
        <Space direction="vertical" size={4} style={{ width: "100%" }}>
          {editableHeaders.map(([name, value], index) => {
            const nameValid = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name);
            return (
              <Space.Compact key={index} style={{ width: "100%" }}>
                <Input
                  value={name}
                  status={nameValid ? undefined : "error"}
                  aria-label={`Header ${index + 1} name`}
                  data-testid={`${testIdPrefix}-name-${index}`}
                  onChange={(event) => updateHeader(index, 0, event.target.value)}
                  placeholder="Header name"
                  style={{ width: 190, fontFamily: "monospace", fontSize: 12 }}
                />
                <Input
                  value={value}
                  aria-label={`Header ${index + 1} value`}
                  data-testid={`${testIdPrefix}-value-${index}`}
                  onChange={(event) => updateHeader(index, 1, event.target.value)}
                  placeholder="Header value"
                  style={{ flex: 1, fontFamily: "monospace", fontSize: 12 }}
                />
                <Tooltip title="Delete header">
                  <Button
                    aria-label={`Delete header ${index + 1}`}
                    data-testid={`${testIdPrefix}-delete-${index}`}
                    icon={<DeleteOutlined />}
                    onClick={() =>
                      onHeadersChange?.(editableHeaders.filter((_, itemIndex) => itemIndex !== index))
                    }
                  />
                </Tooltip>
              </Space.Compact>
            );
          })}
          <Button
            size="small"
            icon={<PlusOutlined />}
            data-testid={`${testIdPrefix}-add`}
            onClick={() => onHeadersChange?.([...editableHeaders, ["", ""]])}
          >
            Add header
          </Button>
        </Space>
      </div>
    );
  }

  const configuredChange = (record: HeaderDiffItem) =>
    !!record.changeSource && record.changeSource === "configured";
  const protocolChange = (record: HeaderDiffItem) =>
    record.changeSource === "protocol";
  const rowColors = (record: HeaderDiffItem) =>
    protocolChange(record) ? diffColors.protocol : diffColors[record.diffType as keyof typeof diffColors];

  const columns: ColumnsType<HeaderDiffItem> = [
    {
      title: "Name",
      dataIndex: "name",
      key: "name",
      width: 180,
      render: (text: string, record: HeaderDiffItem) => (
        <Text
          strong
          style={{
            fontFamily: "monospace",
            fontSize: 12,
            textDecoration: record.diffType === 'deleted' && configuredChange(record) ? 'line-through' : undefined,
            color: record.diffType && record.diffType !== 'unchanged'
              ? rowColors(record).text
              : undefined,
          }}
        >
          {text}
        </Text>
      ),
    },
    {
      title: "Value",
      dataIndex: "value",
      key: "value",
      render: (text: string, record: HeaderDiffItem) => (
        <div>
          <Text
            style={{
              fontFamily: "monospace",
              fontSize: 12,
              textDecoration: record.diffType === 'deleted' && configuredChange(record) ? 'line-through' : undefined,
              color: record.diffType && record.diffType !== 'unchanged'
                ? rowColors(record).text
                : undefined,
            }}
            copyable={record.diffType !== 'deleted' || protocolChange(record) ? { text } : undefined}
          >
            {text}
          </Text>
          {record.diffType === 'modified' && record.originalValue && (
            <div>
              <Text
                type="secondary"
                style={{
                  fontFamily: "monospace",
                  fontSize: 11,
                  textDecoration: 'line-through',
                  opacity: 0.6,
                }}
              >
                {record.originalValue}
              </Text>
            </div>
          )}
          {protocolChange(record) && (
            <Tooltip title="Bifrost removes hop-by-hop headers while forwarding for protocol compatibility. No rule, script, or breakpoint change is implied.">
              <Tag color="blue" style={{ marginLeft: 8 }} data-testid={`${testIdPrefix}-protocol-badge`}>
                Protocol handling
              </Tag>
            </Tooltip>
          )}
        </div>
      ),
    },
  ];

  if ((!effectiveHeaders || effectiveHeaders.length === 0) && !hasModifications) {
    if (isTunnel) {
      return (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexDirection: "column",
            gap: 16,
            minHeight: 200,
            backgroundColor: token.colorBgLayout,
            borderRadius: 4,
          }}
        >
          <TunnelInterceptActions
            isTunnel={isTunnel}
            host={host}
            clientApp={clientApp}
            clientIp={clientIp}
            emptyText="No headers"
          />
        </div>
      );
    }
    return (
      <Text type="secondary" style={{ padding: 8, display: "block" }}>
        No headers
      </Text>
    );
  }

  const currentLabel = flow === "response" ? "Sent to client" : "Sent upstream";
  const originalLabel = flow === "response" ? "Upstream original" : "Client original";

  return (
    <div ref={tableRef}>
      {hasModifications && (
        <div style={{ marginBottom: 8 }}>
          <Space wrap>
            <Radio.Group
              value={resolvedViewMode}
              onChange={(e) => setViewMode(e.target.value)}
              size="small"
              data-testid={`${testIdPrefix}-mode-tabs`}
            >
              <Radio.Button value="current" data-testid={`${testIdPrefix}-tab-current`}>
                {currentLabel}
              </Radio.Button>
              {showOriginalTab && (
                <Radio.Button value="original" data-testid={`${testIdPrefix}-tab-original`}>
                  {originalLabel}
                </Radio.Button>
              )}
            </Radio.Group>
            {diffResult && (
              <>
                <Tag color={diffResult.summary.configured > 0 ? "red" : undefined} data-testid={`${testIdPrefix}-configured-summary`}>
                  Configured changes: {diffResult.summary.configured}
                </Tag>
                <Tag color={diffResult.summary.protocol > 0 ? "blue" : undefined} data-testid={`${testIdPrefix}-protocol-summary`}>
                  Protocol handling: {diffResult.summary.protocol}
                </Tag>
              </>
            )}
          </Space>
        </div>
      )}
      <ConfigProvider
        theme={{
          components: {
            Table: {
              cellPaddingBlockSM: 2,
              cellPaddingInlineSM: 4,
            },
          },
        }}
      >
        <Table
          dataSource={filteredData}
          columns={columns}
          pagination={false}
          size="small"
          onRow={(record: HeaderDiffItem) => {
            if (!record.diffType || record.diffType === 'unchanged') return {};
            return {
              style: {
                backgroundColor: rowColors(record).bg,
              },
            };
          }}
          style={{
            backgroundColor: token.colorBgLayout,
            borderRadius: 4,
          }}
        />
      </ConfigProvider>
    </div>
  );
};
