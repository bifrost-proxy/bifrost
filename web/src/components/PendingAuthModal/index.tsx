import { useCallback } from "react";
import {
  Badge,
  Button,
  List,
  Modal,
  Popconfirm,
  Space,
  Typography,
  message,
  theme,
} from "antd";
import {
  CheckOutlined,
  CloseOutlined,
  ClearOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import { usePendingAuthStore } from "../../stores/usePendingAuthStore";

const { Text } = Typography;

function formatTimeAgo(timestamp: number) {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestamp;
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

export default function PendingAuthModal() {
  const { token } = theme.useToken();
  const pendingList = usePendingAuthStore((s) => s.pendingList);
  const pendingCount = usePendingAuthStore((s) => s.pendingCount);
  const approvePending = usePendingAuthStore((s) => s.approvePending);
  const rejectPending = usePendingAuthStore((s) => s.rejectPending);
  const clearPending = usePendingAuthStore((s) => s.clearPending);

  const handleApprove = useCallback(
    async (ip: string) => {
      const ok = await approvePending(ip);
      if (ok) {
        message.success(`Approved ${ip}`);
      } else {
        message.error(`Failed to approve ${ip}`);
      }
    },
    [approvePending],
  );

  const handleReject = useCallback(
    async (ip: string) => {
      const ok = await rejectPending(ip);
      if (ok) {
        message.success(`Rejected ${ip}`);
      } else {
        message.error(`Failed to reject ${ip}`);
      }
    },
    [rejectPending],
  );

  const handleClearAll = useCallback(async () => {
    const ok = await clearPending();
    if (ok) {
      message.success("Cleared all pending authorizations");
    } else {
      message.error("Failed to clear pending authorizations");
    }
  }, [clearPending]);

  return (
    <Modal
      open={pendingCount > 0}
      title={
        <Space>
          <WarningOutlined style={{ color: token.colorWarning }} />
          <span>Pending Authorization Requests</span>
          <Badge
            count={pendingCount}
            style={{ backgroundColor: token.colorWarning }}
          />
        </Space>
      }
      footer={
        <Space>
          {pendingList.length > 0 && (
            <Popconfirm
              title="Clear all pending authorizations?"
              description="This will reject all pending requests."
              onConfirm={handleClearAll}
              okText="Yes"
              cancelText="No"
            >
              <Button
                icon={<ClearOutlined />}
                data-testid="pending-auth-modal-clear-all"
              >
                Clear All
              </Button>
            </Popconfirm>
          )}
        </Space>
      }
      closable={false}
      maskClosable={false}
      keyboard={false}
      centered
      width={560}
      zIndex={999}
      data-testid="pending-auth-modal"
    >
      <div data-testid="pending-auth-modal-content">
        <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
          The following devices are requesting proxy access. Allow trusted devices only.
        </Text>
        <List
          size="small"
          dataSource={pendingList}
          locale={{ emptyText: "No pending requests" }}
          renderItem={(item) => (
            <List.Item
              actions={[
                <Button
                  key="approve"
                  type="primary"
                  size="small"
                  icon={<CheckOutlined />}
                  onClick={() => handleApprove(item.ip)}
                  data-testid={`pending-auth-approve-${item.ip}`}
                >
                  Allow
                </Button>,
                <Button
                  key="reject"
                  danger
                  size="small"
                  icon={<CloseOutlined />}
                  onClick={() => handleReject(item.ip)}
                  data-testid={`pending-auth-reject-${item.ip}`}
                >
                  Deny
                </Button>,
              ]}
            >
              <List.Item.Meta
                title={<Text code>{item.ip}</Text>}
                description={
                  <Text type="secondary">
                    First seen: {formatTimeAgo(item.first_seen)} · Attempts:{" "}
                    {item.attempt_count}
                  </Text>
                }
              />
            </List.Item>
          )}
        />
      </div>
    </Modal>
  );
}
