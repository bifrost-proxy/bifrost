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

const { Text } = Typography;
type NotificationFilterStatus = 'all' | 'read' | 'unread';
type NotificationFilterMap = Record<'all' | 'tls_trust_change' | 'pending_authorization', NotificationFilterStatus>;

function formatTimestamp(ts: number): string {
  if (!ts) return '-';
  const d = new Date(ts * 1000);
  return d.toLocaleString();
}

function trustStatusTag(status: { status: string; reason?: string; confidence?: number }) {
  switch (status.status) {
    case 'trusted':
      return <Tag color="success">Trusted</Tag>;
    case 'not_trusted':
      return <Tag color="error">Not Trusted</Tag>;
    case 'possibly_not_trusted':
      return (
        <Tooltip title="Only one definitive failure detected — needs more data to confirm">
          <Tag color="blue">Possibly Not Trusted</Tag>
        </Tooltip>
      );
    case 'likely_untrusted':
      return (
        <Tag color="warning">
          Likely Untrusted ({((status.confidence ?? 0) * 100).toFixed(0)}%)
        </Tag>
      );
    default:
      return <Tag>Unknown</Tag>;
  }
}

function notificationTypeTag(type: string) {
  switch (type) {
    case 'tls_trust_change':
      return (
        <Tag icon={<SafetyCertificateOutlined />} color="orange">
          TLS Trust
        </Tag>
      );
    case 'pending_authorization':
      return (
        <Tag icon={<WarningOutlined />} color="blue">
          Authorization
        </Tag>
      );
    default:
      return <Tag>{type}</Tag>;
  }
}

function statusTag(status: string) {
  switch (status) {
    case 'unread':
      return <Badge status="processing" text="Unread" />;
    case 'read':
      return <Badge status="default" text="Read" />;
    case 'dismissed':
      return <Badge status="default" text="Dismissed" />;
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
      title: 'Type',
      dataIndex: 'notification_type',
      key: 'type',
      width: 130,
      render: (type: string) => notificationTypeTag(type),
    },
    {
      title: 'Domain',
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
      title: 'Title',
      dataIndex: 'title',
      key: 'title',
      width: 260,
      render: (title: string) => <CopyableCell text={title} />,
    },
    {
      title: 'Message',
      dataIndex: 'message',
      key: 'message',
      width: 300,
      render: (msg: string) => <CopyableCell text={msg} />,
    },
    {
      title: 'Status',
      dataIndex: 'status',
      key: 'status',
      width: 100,
      render: (s: string) => statusTag(s),
    },
    {
      title: 'Time',
      dataIndex: 'created_at',
      key: 'created_at',
      width: 170,
      render: (ts: number) => <Text type="secondary">{formatTimestamp(ts)}</Text>,
    },
    {
      title: 'Actions',
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
            return <Tag color="green">Passthrough ✓</Tag>;
          }
          if (record.action_taken === 'ignored' || record.status === 'dismissed') {
            return <Tag>Ignored</Tag>;
          }
          if (domain) {
            return (
              <Space size="small">
                <Button
                  size="small"
                  type="primary"
                  onClick={() => handlePassthrough(record.id, domain!)}
                >
                  Passthrough
                </Button>
                <Button size="small" onClick={() => handleIgnore(record.id)}>
                  Ignore
                </Button>
              </Space>
            );
          }
        }
        return (
          <Space size="small">
            {record.status === 'unread' && (
              <Tooltip title="Mark as read">
                <Button
                  type="text"
                  size="small"
                  icon={<EyeOutlined />}
                  onClick={() => handleUpdateStatus(record.id, 'read')}
                />
              </Tooltip>
            )}
            {record.status !== 'dismissed' && (
              <Tooltip title="Dismiss">
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
          <Text type="secondary">{total} notification(s)</Text>
          <Segmented<NotificationFilterStatus>
            data-testid={`notifications-status-filter-${tabKey}`}
            value={statusFilter}
            onChange={(value) => onStatusFilterChange(value)}
            options={[
              { label: 'All', value: 'all' },
              { label: 'Read', value: 'read' },
              { label: 'Unread', value: 'unread' },
            ]}
          />
        </Space>
        <Space size="small">
          {pendingTlsItems.length > 0 && (
            <>
              <Button size="small" type="primary" onClick={handleBatchPassthrough}>
                Passthrough All ({pendingTlsItems.length})
              </Button>
              <Button size="small" onClick={handleBatchIgnore}>
                Ignore All ({pendingTlsItems.length})
              </Button>
            </>
          )}
          <Button
            size="small"
            icon={<CheckCircleOutlined />}
            onClick={() => handleMarkAllRead(tabKey === 'all' ? undefined : tabKey, statusFilter)}
          >
            Mark All Read
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
        locale={{ emptyText: <Empty description="No notifications" /> }}
      />
    </div>
  );
}

function ClientTrustTable() {
  const { clientTrust, untrustedCount, fetchClientTrust } = useNotificationStore();

  useEffect(() => {
    fetchClientTrust();
  }, [fetchClientTrust]);

  const columns = [
    {
      title: 'Identifier',
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
      title: 'Trust Status',
      key: 'trust_status',
      render: (_: unknown, record: ClientTrustSummary) => trustStatusTag(record.trust_status),
    },
    {
      title: 'Success',
      dataIndex: 'handshake_success',
      key: 'success',
      width: 80,
      render: (v: number) => <Text type="success">{v}</Text>,
    },
    {
      title: 'Fail (Untrust)',
      key: 'fail_untrust',
      width: 140,
      render: (_: unknown, record: ClientTrustSummary) => {
        const total = record.handshake_fail_untrust;
        const d = record.handshake_fail_definite_untrust;
        const p = record.handshake_fail_probable_untrust;
        if (d !== undefined && p !== undefined) {
          return total > 0 ? (
            <Tooltip title={`Definite: ${d}, Probable: ${p}`}>
              <Text type="danger">{total}</Text>
            </Tooltip>
          ) : <Text>{total}</Text>;
        }
        return total > 0 ? <Text type="danger">{total}</Text> : <Text>{total}</Text>;
      },
    },
    {
      title: 'Fail (Other)',
      dataIndex: 'handshake_fail_other',
      key: 'fail_other',
      width: 100,
    },
    {
      title: 'Last Failure',
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
      title: 'Last Seen',
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
          <Text type="secondary">{clientTrust.length} client(s)</Text>
          {untrustedCount > 0 && (
            <Tag color="warning">{untrustedCount} untrusted</Tag>
          )}
        </Space>
        <Button size="small" icon={<ReloadOutlined />} onClick={fetchClientTrust}>
          Refresh
        </Button>
      </div>
      <Table
        dataSource={clientTrust}
        columns={columns}
        rowKey={(r) => `${r.identifier_type}-${r.identifier}`}
        size="small"
        scroll={{ x: 900 }}
        pagination={{ pageSize: 10, showSizeChanger: false }}
        locale={{ emptyText: <Empty description="No client trust data" /> }}
      />
    </div>
  );
}

const VALID_TABS = ['all', 'tls_trust_change', 'pending_authorization', 'client_trust'];

export default function Notifications() {
  const { token } = theme.useToken();
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
    padding: '16px 20px 0',
    background: token.colorBgLayout,
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
                All Notifications
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
                TLS Trust
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
                Authorization
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
                Client Trust Status
              </Space>
            ),
            children: <ClientTrustTable />,
          },
        ]}
      />
    </div>
  );
}
