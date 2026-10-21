import { type ReactNode, useCallback, useMemo } from 'react';
import { Typography, Space, Divider, theme } from 'antd';
import { FilterOutlined, DownOutlined, UpOutlined } from '@ant-design/icons';
import type {
  SessionTargetSearchState,
  DisplayFormat,
  RecordContentType,
} from '../../../types';
import { Search } from '../Search';
import { BodyTypeMenu } from '../panes/Body/BodyTypeMenu';
import { useI18n } from '../../../i18n';

const { Text } = Typography;

interface TabConfig {
  key: string;
  label: string;
  children: ReactNode;
  enable?: boolean;
}

interface PanelProps {
  name: 'Request' | 'Response';
  tabs: TabConfig[];
  activeTab: string;
  onTabChange: (tab: string) => void;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  tabBarExtra?: ReactNode;
  displayFormat?: DisplayFormat;
  onDisplayFormatChange?: (format: string) => void;
  contentType?: RecordContentType;
  bodyData?: string | null;
  collapsed?: boolean;
  onCollapsedChange?: (collapsed: boolean) => void;
  keepAliveTabs?: string[];
  contentOverflow?: 'auto' | 'hidden';
  bodyFormatTabs?: string[];
}

export const Panel = ({
  name,
  tabs,
  activeTab,
  onTabChange,
  searchValue,
  onSearch,
  tabBarExtra,
  displayFormat,
  onDisplayFormatChange,
  contentType,
  bodyData,
  collapsed = false,
  onCollapsedChange,
  keepAliveTabs,
  contentOverflow = 'auto',
  bodyFormatTabs = ['Body'],
}: PanelProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();

  const enabledTabs = tabs.filter((tab) => tab.enable !== false);
  const keepAliveSet = useMemo(
    () => new Set(keepAliveTabs ?? []),
    [keepAliveTabs],
  );

  const handleToggleSearch = useCallback(() => {
    onSearch({ show: !searchValue.show });
  }, [onSearch, searchValue.show]);

  const handleToggleCollapsed = useCallback(() => {
    onCollapsedChange?.(!collapsed);
  }, [onCollapsedChange, collapsed]);

  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 12,
          minHeight: 28,
          padding: '2px 0',
          flexShrink: 0,
        }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 12,
            flex: 1,
            minWidth: 0,
          }}
        >
          <Text strong style={{ fontSize: 14 }}>
            {name === 'Request'
              ? t('trafficDetail.panel.request')
              : t('trafficDetail.panel.response')}
          </Text>
          <div
            style={{
              flex: 1,
              minWidth: 0,
              overflowX: 'auto',
              overflowY: 'hidden',
              scrollbarWidth: 'thin',
            }}
          >
            <div
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 16,
                minWidth: 'max-content',
                whiteSpace: 'nowrap',
              }}
            >
              {enabledTabs.map((tab) => (
                <Text
                  key={tab.key}
                  type={activeTab === tab.key ? undefined : 'secondary'}
                  style={{
                    cursor: 'pointer',
                    flexShrink: 0,
                    fontWeight: activeTab === tab.key ? 500 : 400,
                    color: activeTab === tab.key ? token.colorPrimary : undefined,
                  }}
                  onClick={() => onTabChange(tab.key)}
                  data-testid={`${name.toLowerCase()}-tab-${tab.key.toLowerCase()}`}
                >
                  {tab.label}
                </Text>
              ))}
            </div>
          </div>
        </div>

        <Space size="middle" align="center" style={{ flexShrink: 0 }}>
          {tabBarExtra}
          <Space size="small" align="center">
            {bodyFormatTabs.includes(activeTab) &&
              displayFormat &&
              onDisplayFormatChange &&
              contentType && (
                <BodyTypeMenu
                  value={displayFormat}
                  onChange={onDisplayFormatChange}
                  contentType={contentType}
                  data={bodyData}
                />
              )}
            <FilterOutlined
              onClick={handleToggleSearch}
              style={{
                cursor: 'pointer',
                color: searchValue.show ? token.colorPrimary : token.colorTextSecondary,
                fontSize: 14,
              }}
            />
            {collapsed ? (
              <DownOutlined
                onClick={handleToggleCollapsed}
                style={{
                  cursor: 'pointer',
                  color: token.colorTextSecondary,
                  fontSize: 14,
                }}
              />
            ) : (
              <UpOutlined
                onClick={handleToggleCollapsed}
                style={{
                  cursor: 'pointer',
                  color: token.colorTextSecondary,
                  fontSize: 14,
                }}
              />
            )}
          </Space>
        </Space>
      </div>

      <div
        style={{
          display: collapsed ? 'none' : 'flex',
          flex: 1,
          minHeight: 0,
          flexDirection: 'column',
        }}
      >
        <Divider style={{ margin: '0 0 4px 0' }} />

        <Search value={searchValue} onSearch={onSearch} />

        <div
          style={{
            flex: 1,
            minHeight: 0,
            overflow: contentOverflow,
          }}
        >
          {enabledTabs.map((tab) => {
            const isActive = tab.key === activeTab;
            if (!isActive && !keepAliveSet.has(tab.key)) {
              return null;
            }
            return (
              <div
                key={tab.key}
                style={{ display: isActive ? 'block' : 'none', height: '100%' }}
              >
                {tab.children}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
};
