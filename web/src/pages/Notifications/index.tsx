import { useEffect, useCallback, useState, useMemo, type CSSProperties } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  Tabs,
  Table,
  Tag,
  Button,
  Space,
  Empty,
  Badge,
  Typography,
  Tooltip,
  theme,
  Segmented,
} from 'antd';
import {
  BellOutlined,
  CheckCircleOutlined,
  SafetyCertificateOutlined,
  WarningOutlined,
  EyeOutlined,
  CheckOutlined,
  CopyOutlined,
  ReloadOutlined,
} from '@ant-design/icons';
import { useNotificationStore } from '../../stores/useNotificationStore';
import { useTlsConfigStore } from '../../stores/useTlsConfigStore';
import type { NotificationRecord, ClientTrustSummary } from '../../api/notifications';
import { isMacDesktopShell } from '../../runtime';
import { useI18n, type TFunction } from '../../i18n';

const { Text } = Typography;
type NotificationFilterStatus = 'all' | 'read' | 'unread';
type NotificationFilterMap = Record<'all' | 'tls_trust_change' | 'pending_authorization', NotificationFilterStatus>;

function formatTimestamp(ts: number): string {
  if (!ts) return '-';
  const d = new Date(ts * 1000);
  return d.toLocaleString();
}

function trustStatusTag(
  status: { status: string; reason?: string; confidence?: number },
  t: TFunction,
) {
  switch (status.status) {
    case 'trusted':
      return <Tag color="success">{t('notify.trust.trusted')}</Tag>;
    case 'not_trusted':
      return <Tag color="error">{t('notify.trust.notTrusted')}</Tag>;
    case 'possibly_not_trusted':
      return (
        <Tooltip title={t('notify.trust.possiblyNotTrustedTip')}>
          <Tag color="blue">{t('notify.trust.possiblyNotTrusted')}</Tag>
        </Tooltip>
      );
    case 'likely_untrusted':
      return (
        <Tag color="warning">
          {t('notify.trust.likelyUntrusted', {
            percent: ((status.confidence ?? 0) * 100).toFixed(0),
          })}
        </Tag>
      );
    default:
      return <Tag>{t('common.unknown')}</Tag>;
  }
}

function notificationTypeTag(type: string, t: TFunction) {
  switch (type) {
    case 'tls_trust_change':
      return (
        <Tag icon={<SafetyCertificateOutlined />} color="orange">
          {t('notify.tabs.tlsTrust')}
        </Tag>
      );
    case 'pending_authorization':
      return (
        <Tag icon={<WarningOutlined />} color="blue">
          {t('notify.tabs.authorization')}
        </Tag>
      );
    default:
      return <Tag>{type}</Tag>;
  }
}

function statusTag(status: string, t: TFunction) {
  switch (status) {
    case 'unread':
      return <Badge status="processing" text={t('notify.status.unread')} />;
    case 'read':
      return <Badge status="default" text={t('notify.status.read')} />;
    case 'dismissed':
      return <Badge status="default" text={t('notify.status.dismissed')} />;
    default:
      return <Badge status="default" text={status} />;
  }
}

function CopyableCell({ text }: { text: string | null }) {
  const [copied, setCopied] = useState(false);
  if (!text) return <span>-</span>;
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation();
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <Tooltip title={text} mouseEnterDelay={0.3}>
      <div className="copyable-cell">
        <span className="copyable-cell-text">{text}</span>
        <span className="copyable-cell-btn" onClick={handleCopy}>
          {copied ? <CheckOutlined style={{ color: '#52c41a' }} /> : <CopyOutlined />}
        </span>
      </div>
    </Tooltip>
  );
}

function NotificationsTable({
  tabKey,
  statusFilter,
  onStatusFilterChange,
}: {
  tabKey: 'all' | 'tls_trust_change' | 'pending_authorization';
  statusFilter: NotificationFilterStatus;
  onStatusFilterChange: (status: NotificationFilterStatus) => void;
}) {
  const { t } = useI18n();
  const { notifications, total, loading, handleUpdateStatus, handleMarkAllRead, activeTab } =
    useNotificationStore();
  const addDomainToPassthrough = useTlsConfigStore((s) => s.addDomainToPassthrough);
  const fetchTlsConfig = useTlsConfigStore((s) => s.fetchConfig);
  const fetchNotifications = useNotificationStore((s) => s.fetchNotifications);

  useEffect(() => {
    fetchTlsConfig();
  }, [fetchTlsConfig]);

  useEffect(() => {
    if (activeTab !== tabKey) {
      return;
    }
    fetchNotifications(tabKey, statusFilter);
  }, [activeTab, fetchNotifications, statusFilter, tabKey]);

  const handlePassthrough = useCallback(
    async (id: number, domain: string) => {
      const ok = await addDomainToPassthrough(domain);
      if (ok) {
        await handleUpdateStatus(id, 'read', 'passthrough', tabKey, statusFilter);
      }
    },
    [addDomainToPassthrough, handleUpdateStatus, statusFilter, tabKey],
  );

  const handleIgnore = useCallback(
    async (id: number) => {
      await handleUpdateStatus(id, 'dismissed', 'ignored', tabKey, statusFilter);
    },
    [handleUpdateStatus, statusFilter, tabKey],
  );

  const pendingTlsItems = useMemo(() => {
    return notifications
      .filter(
        (n) =>
          n.notification_type === 'tls_trust_change' &&
          n.action_taken !== 'passthrough' &&
          n.action_taken !== 'ignored' &&
          n.status !== 'dismissed',
      )
      .map((n) => {
        let domain: string | null = null;
        try {
          const meta = JSON.parse(n.metadata || '{}');
          domain = meta.domain || null;
        } catch {
          domain = null;
        }
        return { id: n.id, domain };
      })
      .filter((i): i is { id: number; domain: string } => i.domain !== null);
  }, [notifications]);

  const handleBatchPassthrough = useCallback(async () => {
    for (const item of pendingTlsItems) {
      await addDomainToPassthrough(item.domain);
      await handleUpdateStatus(item.id, 'read', 'passthrough', tabKey, statusFilter);
    }
  }, [pendingTlsItems, addDomainToPassthrough, handleUpdateStatus, statusFilter, tabKey]);

  const handleBatchIgnore = useCallback(async () => {
    for (const item of pendingTlsItems) {
      await handleUpdateStatus(item.id, 'dismissed', 'ignored', tabKey, statusFilter);
    }
  }, [pendingTlsItems, handleUpdateStatus, statusFilter, tabKey]);

  const columns = [
    {
      title: t('notify.table.type'),
      dataIndex: 'notification_type',
      key: 'type',
      width: 130,
      render: (type: string) => notificationTypeTag(type, t),
    },
    {
      title: t('notify.table.domain'),
      dataIndex: 'metadata',
      key: 'domain',
      width: 180,
      render: (metadata: string | null) => {
        if (!metadata) return '-';
        try {
          const parsed = JSON.parse(metadata);
          return <CopyableCell text={parsed.domain ?? null} />;
        } catch {
          return '-';
        }
      },
    },
    {
      title: t('notify.table.title'),
      dataIndex: 'title',
      key: 'title',
      width: 260,
      render: (title: string) => <CopyableCell text={title} />,
    },
    {
      title: t('notify.table.message'),
      dataIndex: 'message',
      key: 'message',
      width: 300,
      render: (msg: string) => <CopyableCell text={msg} />,
    },
    {
      title: t('common.status'),
      dataIndex: 'status',
      key: 'status',
      width: 100,
      render: (s: string) => statusTag(s, t),
    },
    {
      title: t('notify.table.time'),
      dataIndex: 'created_at',
      key: 'created_at',
      width: 170,
      render: (ts: number) => <Text type="secondary">{formatTimestamp(ts)}</Text>,
    },
    {
      title: t('common.actions'),
      key: 'actions',
      width: 200,
      fixed: 'right' as const,
      render: (_: unknown, record: NotificationRecord) => {
        if (record.notification_type === 'tls_trust_change') {
          let domain: string | null = null;
          try {
            const meta = JSON.parse(record.metadata || '{}');
            domain = meta.domain || null;
          } catch {
            domain = null;
          }

          if (record.action_taken === 'passthrough') {
            return <Tag color="green">{t('notify.table.passthroughDone')}</Tag>;
          }
          if (record.action_taken === 'ignored' || record.status === 'dismissed') {
            return <Tag>{t('notify.table.ignored')}</Tag>;
          }
          if (domain) {
            return (
              <Space size="small">
                <Button
                  size="small"
                  type="primary"
                  onClick={() => handlePassthrough(record.id, domain!)}
                >
                  {t('notify.table.passthrough')}
                </Button>
                <Button size="small" onClick={() => handleIgnore(record.id)}>
                  {t('notify.table.ignore')}
                </Button>
              </Space>
            );
          }
        }
        return (
          <Space size="small">
            {record.status === 'unread' && (
              <Tooltip title={t('notify.table.markAsRead')}>
                <Button
                  type="text"
                  size="small"
                  icon={<EyeOutlined />}
                  onClick={() => handleUpdateStatus(record.id, 'read')}
                />
              </Tooltip>
            )}
            {record.status !== 'dismissed' && (
              <Tooltip title={t('notify.table.dismiss')}>
                <Button
                  type="text"
                  size="small"
                  icon={<CheckOutlined />}
                  onClick={() => handleUpdateStatus(record.id, 'dismissed', 'dismissed')}
                />
              </Tooltip>
            )}
          </Space>
        );
      },
    },
  ];

  return (
    <div>
      <div style={{ marginBottom: 12, display: 'flex', justifyContent: 'space-between' }}>
        <Space align="center" size="middle">
          <Text type="secondary">{t('notify.table.count', { count: total })}</Text>
          <Segmented<NotificationFilterStatus>
            data-testid={`notifications-status-filter-${tabKey}`}
            value={statusFilter}
            onChange={(value) => onStatusFilterChange(value)}
            options={[
              { label: t('common.all'), value: 'all' },
              { label: t('notify.status.read'), value: 'read' },
              { label: t('notify.status.unread'), value: 'unread' },
            ]}
          />
        </Space>
        <Space size="small">
          {pendingTlsItems.length > 0 && (
            <>
              <Button size="small" type="primary" onClick={handleBatchPassthrough}>
                {t('notify.table.passthroughAll', { count: pendingTlsItems.length })}
              </Button>
              <Button size="small" onClick={handleBatchIgnore}>
                {t('notify.table.ignoreAll', { count: pendingTlsItems.length })}
              </Button>
            </>
          )}
          <Button
            size="small"
            icon={<CheckCircleOutlined />}
            onClick={() => handleMarkAllRead(tabKey === 'all' ? undefined : tabKey, statusFilter)}
          >
            {t('notify.table.markAllRead')}
          </Button>
        </Space>
      </div>
      <Table
        dataSource={notifications}
        columns={columns}
        rowKey="id"
        loading={loading}
        size="small"
        scroll={{ x: 1340 }}
        pagination={{ pageSize: 10, showSizeChanger: false }}
        locale={{ emptyText: <Empty description={t('notify.table.empty')} /> }}
      />
    </div>
  );
}

function ClientTrustTable() {
  const { t } = useI18n();
  const { clientTrust, untrustedCount, fetchClientTrust } = useNotificationStore();

  useEffect(() => {
    fetchClientTrust();
  }, [fetchClientTrust]);

  const columns = [
    {
      title: t('notify.client.identifier'),
      dataIndex: 'identifier',
      key: 'identifier',
      render: (id: string, record: ClientTrustSummary) => (
        <Space>
          <Tag>{record.identifier_type.toUpperCase()}</Tag>
          <Text copyable={{ text: id }}>{id}</Text>
        </Space>
      ),
    },
    {
      title: t('notify.client.trustStatus'),
      key: 'trust_status',
      render: (_: unknown, record: ClientTrustSummary) => trustStatusTag(record.trust_status, t),
    },
    {
      title: t('notify.client.success'),
      dataIndex: 'handshake_success',
      key: 'success',
      width: 80,
      render: (v: number) => <Text type="success">{v}</Text>,
    },
    {
      title: t('notify.client.failUntrust'),
      key: 'fail_untrust',
      width: 140,
      render: (_: unknown, record: ClientTrustSummary) => {
        const total = record.handshake_fail_untrust;
        const d = record.handshake_fail_definite_untrust;
        const p = record.handshake_fail_probable_untrust;
        if (d !== undefined && p !== undefined) {
          return total > 0 ? (
            <Tooltip title={t('notify.client.failUntrustTip', { definite: d, probable: p })}>
              <Text type="danger">{total}</Text>
            </Tooltip>
          ) : <Text>{total}</Text>;
        }
        return total > 0 ? <Text type="danger">{total}</Text> : <Text>{total}</Text>;
      },
    },
    {
      title: t('notify.client.failOther'),
      dataIndex: 'handshake_fail_other',
      key: 'fail_other',
      width: 100,
    },
    {
      title: t('notify.client.lastFailure'),
      key: 'last_failure',
      render: (_: unknown, record: ClientTrustSummary) =>
        record.last_failure_domain ? (
          <Tooltip title={record.last_failure_reason}>
            <Text type="secondary">{record.last_failure_domain}</Text>
          </Tooltip>
        ) : (
          '-'
        ),
    },
    {
      title: t('notify.client.lastSeen'),
      dataIndex: 'last_seen',
      key: 'last_seen',
      width: 180,
      render: (ts: number) => <Text type="secondary">{formatTimestamp(ts)}</Text>,
    },
  ];

  return (
    <div>
      <div style={{ marginBottom: 12, display: 'flex', justifyContent: 'space-between' }}>
        <Space>
          <Text type="secondary">
            {t('notify.client.count', { count: clientTrust.length })}
          </Text>
          {untrustedCount > 0 && (
            <Tag color="warning">
              {t('notify.client.untrustedCount', { count: untrustedCount })}
            </Tag>
          )}
        </Space>
        <Button size="small" icon={<ReloadOutlined />} onClick={fetchClientTrust}>
          {t('common.refresh')}
        </Button>
      </div>
      <Table
        dataSource={clientTrust}
        columns={columns}
        rowKey={(r) => `${r.identifier_type}-${r.identifier}`}
        size="small"
        scroll={{ x: 900 }}
        pagination={{ pageSize: 10, showSizeChanger: false }}
        locale={{ emptyText: <Empty description={t('notify.client.empty')} /> }}
      />
    </div>
  );
}

const VALID_TABS = ['all', 'tls_trust_change', 'pending_authorization', 'client_trust'];

export default function Notifications() {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const [statusFilters, setStatusFilters] = useState<NotificationFilterMap>({
    all: 'unread',
    tls_trust_change: 'unread',
    pending_authorization: 'unread',
  });
  const { unreadCount, activeTab, setActiveTab, fetchClientTrust } = useNotificationStore();

  useEffect(() => {
    const urlTab = searchParams.get('tab');
    if (urlTab && VALID_TABS.includes(urlTab) && urlTab !== activeTab) {
      setActiveTab(urlTab);
    }
  }, [activeTab, searchParams, setActiveTab]);

  useEffect(() => {
    fetchClientTrust();
  }, [fetchClientTrust]);

  const handleTabChange = useCallback(
    (tab: string) => {
      setActiveTab(tab);
      setSearchParams({ tab }, { replace: true });
    },
    [setActiveTab, setSearchParams],
  );

  const handleStatusFilterChange = useCallback(
    (tabKey: keyof NotificationFilterMap, status: NotificationFilterStatus) => {
      setStatusFilters((prev) => ({ ...prev, [tabKey]: status }));
    },
    [],
  );

  const containerStyle: CSSProperties = {
    height: '100%',
    display: 'flex',
    flexDirection: 'column',
    overflow: 'hidden',
    padding: '0 20px 0',
    background: isMacDesktopShell() ? 'transparent' : token.colorBgLayout,
  };

  const tabContentStyle = `
    .notif-tabs .ant-tabs-content-holder {
      overflow: auto;
      padding-bottom: 16px;
    }
    .copyable-cell {
      display: flex;
      align-items: center;
      gap: 4px;
      overflow: hidden;
    }
    .copyable-cell-text {
      flex: 1;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .copyable-cell-btn {
      flex-shrink: 0;
      opacity: 0;
      cursor: pointer;
      color: rgba(0,0,0,0.45);
      font-size: 12px;
      transition: opacity 0.15s;
    }
    .copyable-cell:hover .copyable-cell-btn {
      opacity: 1;
    }
  `;

  return (
    <div style={containerStyle}>
      <style>{tabContentStyle}</style>
      <Tabs
        className="notif-tabs"
        style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}
        activeKey={activeTab}
        onChange={handleTabChange}
        items={[
          {
            key: 'all',
            label: (
              <Space>
                <BellOutlined />
                {t('notify.tabs.all')}
                {unreadCount > 0 && <Badge count={unreadCount} size="small" />}
              </Space>
            ),
            children: (
              <NotificationsTable
                tabKey="all"
                statusFilter={statusFilters.all}
                onStatusFilterChange={(status) => handleStatusFilterChange('all', status)}
              />
            ),
          },
          {
            key: 'tls_trust_change',
            label: (
              <Space>
                <SafetyCertificateOutlined />
                {t('notify.tabs.tlsTrust')}
              </Space>
            ),
            children: (
              <NotificationsTable
                tabKey="tls_trust_change"
                statusFilter={statusFilters.tls_trust_change}
                onStatusFilterChange={(status) =>
                  handleStatusFilterChange('tls_trust_change', status)
                }
              />
            ),
          },
          {
            key: 'pending_authorization',
            label: (
              <Space>
                <WarningOutlined />
                {t('notify.tabs.authorization')}
              </Space>
            ),
            children: (
              <NotificationsTable
                tabKey="pending_authorization"
                statusFilter={statusFilters.pending_authorization}
                onStatusFilterChange={(status) =>
                  handleStatusFilterChange('pending_authorization', status)
                }
              />
            ),
          },
          {
            key: 'client_trust',
            label: (
              <Space>
                <SafetyCertificateOutlined />
                {t('notify.tabs.clientTrust')}
              </Space>
            ),
            children: <ClientTrustTable />,
          },
        ]}
      />
    </div>
  );
}
