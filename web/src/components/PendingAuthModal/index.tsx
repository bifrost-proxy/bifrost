import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
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
  SettingOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import { usePendingAuthStore } from "../../stores/usePendingAuthStore";
import { useI18n, type TFunction } from "../../i18n";

const { Text } = Typography;

function formatTimeAgo(t: TFunction, timestamp: number) {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestamp;
  if (diff < 60) return t("app.timeAgo.seconds", { n: diff });
  if (diff < 3600) return t("app.timeAgo.minutes", { n: Math.floor(diff / 60) });
  if (diff < 86400) return t("app.timeAgo.hours", { n: Math.floor(diff / 3600) });
  return t("app.timeAgo.days", { n: Math.floor(diff / 86400) });
}

export default function PendingAuthModal() {
  const navigate = useNavigate();
  const { token } = theme.useToken();
  const { t } = useI18n();
  const pendingList = usePendingAuthStore((s) => s.pendingList);
  const pendingCount = usePendingAuthStore((s) => s.pendingCount);
  const approvePending = usePendingAuthStore((s) => s.approvePending);
  const rejectPending = usePendingAuthStore((s) => s.rejectPending);
  const clearPending = usePendingAuthStore((s) => s.clearPending);
  const pendingIdentityKey = useMemo(
    () => pendingList.map((item) => item.ip).sort().join("|"),
    [pendingList],
  );
  const [dismissedIdentityKey, setDismissedIdentityKey] = useState<string | null>(null);
  const modalOpen =
    pendingCount > 0 &&
    pendingIdentityKey.length > 0 &&
    dismissedIdentityKey !== pendingIdentityKey;

  useEffect(() => {
    if (pendingCount === 0) {
      setDismissedIdentityKey(null);
    }
  }, [pendingCount]);

  const dismissCurrentPending = useCallback(() => {
    if (pendingIdentityKey.length > 0) {
      setDismissedIdentityKey(pendingIdentityKey);
    }
  }, [pendingIdentityKey]);

  const handleApprove = useCallback(
    async (ip: string) => {
      dismissCurrentPending();
      const ok = await approvePending(ip);
      if (ok) {
        message.success(t("app.pendingAuth.approved", { ip }));
      } else {
        message.error(t("app.pendingAuth.approveFailed", { ip }));
      }
    },
    [approvePending, dismissCurrentPending, t],
  );

  const handleReject = useCallback(
    async (ip: string) => {
      dismissCurrentPending();
      const ok = await rejectPending(ip);
      if (ok) {
        message.success(t("app.pendingAuth.rejected", { ip }));
      } else {
        message.error(t("app.pendingAuth.rejectFailed", { ip }));
      }
    },
    [rejectPending, dismissCurrentPending, t],
  );

  const handleClearAll = useCallback(async () => {
    dismissCurrentPending();
    const ok = await clearPending();
    if (ok) {
      message.success(t("app.pendingAuth.cleared"));
    } else {
      message.error(t("app.pendingAuth.clearFailed"));
    }
  }, [clearPending, dismissCurrentPending, t]);

  const handleOpenAccessControl = useCallback(() => {
    dismissCurrentPending();
    navigate("/settings?tab=access");
  }, [dismissCurrentPending, navigate]);

  return (
    <Modal
      open={modalOpen}
      title={
        <Space>
          <WarningOutlined style={{ color: token.colorWarning }} />
          <span>{t("app.pendingAuth.title")}</span>
          <Badge
            count={pendingCount}
            style={{ backgroundColor: token.colorWarning }}
          />
        </Space>
      }
      footer={
        <Space>
          <Button
            icon={<SettingOutlined />}
            onClick={handleOpenAccessControl}
            data-testid="pending-auth-modal-settings"
          >
            {t("common.settings")}
          </Button>
          {pendingList.length > 0 && (
            <Popconfirm
              title={t("app.pendingAuth.clearConfirmTitle")}
              description={t("app.pendingAuth.clearConfirmDescription")}
              onConfirm={handleClearAll}
              okText={t("common.yes")}
              cancelText={t("common.no")}
            >
              <Button
                icon={<ClearOutlined />}
                data-testid="pending-auth-modal-clear-all"
              >
                {t("app.pending.clearAll")}
              </Button>
            </Popconfirm>
          )}
        </Space>
      }
      onCancel={dismissCurrentPending}
      closable
      maskClosable={false}
      keyboard={false}
      centered
      width={560}
      zIndex={999}
      data-testid="pending-auth-modal"
    >
      <div data-testid="pending-auth-modal-content">
        <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
          {t("app.pendingAuth.description")}
        </Text>
        <List
          size="small"
          dataSource={pendingList}
          locale={{ emptyText: t("app.pairing.noPendingRequests") }}
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
                  {t("app.pendingAuth.allow")}
                </Button>,
                <Button
                  key="reject"
                  danger
                  size="small"
                  icon={<CloseOutlined />}
                  onClick={() => handleReject(item.ip)}
                  data-testid={`pending-auth-reject-${item.ip}`}
                >
                  {t("app.pendingAuth.deny")}
                </Button>,
              ]}
            >
              <List.Item.Meta
                title={<Text code>{item.ip}</Text>}
                description={
                  <Text type="secondary">
                    {t("app.pending.firstSeenAttempts", {
                      time: formatTimeAgo(t, item.first_seen),
                      count: item.attempt_count,
                    })}
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
