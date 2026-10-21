import { useMemo, useRef } from 'react';
import { Button, Input, Space, Table, Typography, theme, ConfigProvider } from 'antd';
import { DeleteOutlined, PlusOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import type { SessionTargetSearchState } from '../../../../types';
import { useMarkSearch } from '../../hooks/useMarkSearch';
import { useI18n } from '../../../../i18n';

const { Text } = Typography;

interface QueryViewProps {
  url: string;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  editable?: boolean;
  onUrlChange?: (url: string) => void;
}

interface QueryItem {
  key: string;
  name: string;
  value: string;
}

export const QueryView = ({
  url,
  searchValue,
  onSearch,
  editable = false,
  onUrlChange,
}: QueryViewProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const tableRef = useRef<HTMLDivElement>(null);

  const dataSource = useMemo<QueryItem[]>(() => {
    try {
      const urlObj = new URL(url);
      const items: QueryItem[] = [];
      let index = 0;
      urlObj.searchParams.forEach((value, name) => {
        items.push({
          key: String(index++),
          name,
          value,
        });
      });
      return items;
    } catch {
      return [];
    }
  }, [url]);

  const filteredData = useMemo(() => {
    if (!searchValue.value) return dataSource;
    const searchLower = searchValue.value.toLowerCase();
    return dataSource.filter(
      (item) =>
        item.name.toLowerCase().includes(searchLower) ||
        item.value.toLowerCase().includes(searchLower)
    );
  }, [dataSource, searchValue.value]);

  useMarkSearch(searchValue, () => tableRef.current, onSearch);

  const updateUrl = (items: QueryItem[]) => {
    try {
      const next = new URL(url);
      next.search = '';
      for (const item of items) next.searchParams.append(item.name, item.value);
      onUrlChange?.(next.toString());
    } catch {
      // The request URL editor surfaces invalid absolute URLs separately.
    }
  };

  if (editable) {
    return (
      <div ref={tableRef} data-testid="breakpoint-query-editor">
        <Space direction="vertical" size={4} style={{ width: '100%' }}>
          {dataSource.map((item, index) => (
            <Space.Compact key={item.key} style={{ width: '100%' }}>
              <Input
                value={item.name}
                aria-label={t('trafficDetail.query.ariaName', { index: index + 1 })}
                data-testid={`breakpoint-query-name-${index}`}
                onChange={(event) => {
                  const next = dataSource.map((entry) => ({ ...entry }));
                  next[index].name = event.target.value;
                  updateUrl(next);
                }}
                style={{ width: 180, fontFamily: 'monospace', fontSize: 12 }}
              />
              <Input
                value={item.value}
                aria-label={t('trafficDetail.query.ariaValue', { index: index + 1 })}
                data-testid={`breakpoint-query-value-${index}`}
                onChange={(event) => {
                  const next = dataSource.map((entry) => ({ ...entry }));
                  next[index].value = event.target.value;
                  updateUrl(next);
                }}
                style={{ flex: 1, fontFamily: 'monospace', fontSize: 12 }}
              />
              <Button
                icon={<DeleteOutlined />}
                aria-label={t('trafficDetail.query.ariaDelete', { index: index + 1 })}
                data-testid={`breakpoint-query-delete-${index}`}
                onClick={() => updateUrl(dataSource.filter((_, itemIndex) => itemIndex !== index))}
              />
            </Space.Compact>
          ))}
          <Button
            size="small"
            icon={<PlusOutlined />}
            data-testid="breakpoint-query-add"
            onClick={() =>
              updateUrl([...dataSource, { key: String(dataSource.length), name: '', value: '' }])
            }
          >
            {t('trafficDetail.query.addParam')}
          </Button>
        </Space>
      </div>
    );
  }

  const columns: ColumnsType<QueryItem> = [
    {
      title: t('common.name'),
      dataIndex: 'name',
      key: 'name',
      width: 180,
      render: (text: string) => (
        <Text strong style={{ fontFamily: 'monospace', fontSize: 12 }}>
          {text}
        </Text>
      ),
    },
    {
      title: t('common.value'),
      dataIndex: 'value',
      key: 'value',
      render: (text: string) => (
        <Text style={{ fontFamily: 'monospace', fontSize: 12 }} copyable={{ text }}>
          {text}
        </Text>
      ),
    },
  ];

  if (dataSource.length === 0) {
    return (
      <Text type="secondary" style={{ padding: 8, display: 'block' }}>
        {t('trafficDetail.query.empty')}
      </Text>
    );
  }

  return (
    <div ref={tableRef}>
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
          style={{
            backgroundColor: token.colorBgLayout,
            borderRadius: 4,
          }}
        />
      </ConfigProvider>
    </div>
  );
};
