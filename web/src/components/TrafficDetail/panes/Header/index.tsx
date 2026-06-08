import { useMemo, useRef, useState } from "react";
import { Table, Typography, theme, ConfigProvider, Space, Radio } from "antd";
import type { ColumnsType } from "antd/es/table";
import type { SessionTargetSearchState } from "../../../../types";
import { useMarkSearch } from "../../hooks/useMarkSearch";
import { TunnelInterceptActions } from "../../TunnelInterceptActions";

const { Text } = Typography;

interface HeaderViewProps {
  headers: [string, string][] | null;
  originalHeaders?: [string, string][] | null;
  testIdPrefix?: string;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  isTunnel?: boolean;
  host?: string;
  clientApp?: string;
  clientIp?: string;
}

type DiffType = 'added' | 'modified' | 'deleted' | 'unchanged';

interface HeaderItem {
  key: string;
  name: string;
  value: string;
  diffType?: DiffType;
  originalValue?: string;
}

const areHeadersEqual = (
  left: [string, string][] | null | undefined,
  right: [string, string][] | null | undefined,
): boolean => {
  if (left === right) {
    return true;
  }
  if (!left || !right) {
    return !left && !right;
  }
  if (left.length !== right.length) {
    return false;
  }
  return left.every(([leftName, leftValue], index) => {
    const [rightName, rightValue] = right[index] ?? [];
    return leftName === rightName && leftValue === rightValue;
  });
};

export const HeaderView = ({
  headers,
  originalHeaders,
  testIdPrefix = "header-view",
  searchValue,
  onSearch,
  isTunnel,
  host,
  clientApp,
  clientIp,
}: HeaderViewProps) => {
  const { token } = theme.useToken();
  const tableRef = useRef<HTMLDivElement>(null);
  const [viewMode, setViewMode] = useState<'current' | 'original'>('current');

  const showOriginalTab = !!headers && headers.length > 0 && !!originalHeaders && !areHeadersEqual(headers, originalHeaders);
  const hasModifications = showOriginalTab;
  const effectiveHeaders = (headers && headers.length > 0) ? headers : originalHeaders ?? null;
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

  const dataSource = useMemo<HeaderItem[]>(() => {
    if (!displayHeaders) return [];

    const sorted = [...displayHeaders]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, value], index) => ({
        key: String(index),
        name,
        value,
      }));

    const baseHeaders = showOriginalTab ? originalHeaders : null;
    if (resolvedViewMode !== 'current' || !baseHeaders) {
      return sorted;
    }

    const origMap = new Map<string, string[]>();
    for (const [k, v] of baseHeaders) {
      const lower = k.toLowerCase();
      const arr = origMap.get(lower);
      if (arr) {
        arr.push(v);
      } else {
        origMap.set(lower, [v]);
      }
    }

    const currentKeyCount = new Map<string, number>();
    for (const [k] of displayHeaders) {
      const lower = k.toLowerCase();
      currentKeyCount.set(lower, (currentKeyCount.get(lower) ?? 0) + 1);
    }

    const usedOrigIndex = new Map<string, number>();

    const active: HeaderItem[] = sorted.map((item) => {
      const lowerName = item.name.toLowerCase();
      const origValues = origMap.get(lowerName);
      if (!origValues || origValues.length === 0) {
        return { ...item, diffType: 'added' as DiffType };
      }
      const idx = usedOrigIndex.get(lowerName) ?? 0;
      usedOrigIndex.set(lowerName, idx + 1);
      if (idx < origValues.length) {
        const origVal = origValues[idx];
        if (origVal !== item.value) {
          return { ...item, diffType: 'modified' as DiffType, originalValue: origVal };
        }
        return { ...item, diffType: 'unchanged' as DiffType };
      }
      return { ...item, diffType: 'added' as DiffType };
    });

    const deleted: HeaderItem[] = [];
    for (const [k, values] of origMap) {
      const currentCount = currentKeyCount.get(k) ?? 0;
      if (currentCount < values.length) {
        for (let i = currentCount; i < values.length; i++) {
          deleted.push({
            key: `deleted-${deleted.length}`,
            name: baseHeaders.find(([n]) => n.toLowerCase() === k)?.[0] ?? k,
            value: values[i],
            diffType: 'deleted',
          });
        }
      }
    }
    deleted.sort((a, b) => a.name.localeCompare(b.name));

    return [...active, ...deleted];
  }, [displayHeaders, resolvedViewMode, showOriginalTab, originalHeaders]);

  const filteredData = useMemo(() => {
    if (!searchValue.value) return dataSource;
    const searchLower = searchValue.value.toLowerCase();
    return dataSource.filter(
      (item) =>
        item.name.toLowerCase().includes(searchLower) ||
        item.value.toLowerCase().includes(searchLower),
    );
  }, [dataSource, searchValue.value]);

  useMarkSearch(searchValue, () => tableRef.current, onSearch);

  const diffColors = useMemo(() => ({
    added: { bg: token.colorSuccessBg, text: token.colorSuccess },
    modified: { bg: token.colorWarningBg, text: token.colorWarningText },
    deleted: { bg: token.colorErrorBg, text: token.colorError },
  }), [token]);

  const columns: ColumnsType<HeaderItem> = [
    {
      title: "Name",
      dataIndex: "name",
      key: "name",
      width: 180,
      render: (text: string, record: HeaderItem) => (
        <Text
          strong
          style={{
            fontFamily: "monospace",
            fontSize: 12,
            textDecoration: record.diffType === 'deleted' ? 'line-through' : undefined,
            color: record.diffType && record.diffType !== 'unchanged'
              ? diffColors[record.diffType].text
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
      render: (text: string, record: HeaderItem) => (
        <div>
          <Text
            style={{
              fontFamily: "monospace",
              fontSize: 12,
              textDecoration: record.diffType === 'deleted' ? 'line-through' : undefined,
              color: record.diffType && record.diffType !== 'unchanged'
                ? diffColors[record.diffType].text
                : undefined,
            }}
            copyable={record.diffType !== 'deleted' ? { text } : undefined}
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
        </div>
      ),
    },
  ];

  if (!effectiveHeaders || effectiveHeaders.length === 0) {
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

  return (
    <div ref={tableRef}>
      {hasModifications && (
        <div style={{ marginBottom: 8 }}>
          <Space>
            <Radio.Group
              value={resolvedViewMode}
              onChange={(e) => setViewMode(e.target.value)}
              size="small"
              data-testid={`${testIdPrefix}-mode-tabs`}
            >
              <Radio.Button value="current" data-testid={`${testIdPrefix}-tab-current`}>
                Current
              </Radio.Button>
              {showOriginalTab && (
                <Radio.Button value="original" data-testid={`${testIdPrefix}-tab-original`}>
                  Original
                </Radio.Button>
              )}
            </Radio.Group>
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
          onRow={(record: HeaderItem) => {
            if (!record.diffType || record.diffType === 'unchanged') return {};
            return {
              style: {
                backgroundColor: diffColors[record.diffType].bg,
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
