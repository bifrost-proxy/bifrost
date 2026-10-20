import { useCallback } from "react";
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
} from "antd";
import {
  CheckOutlined,
  StopOutlined,
  ClearOutlined,
  SafetyCertificateOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import { usePendingIpTlsStore } from "../../stores/usePendingIpTlsStore";
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

export default function PendingIpTlsModal() {
  const pendingList = usePendingIpTlsStore((s) => s.pendingList);
  const pendingCount = usePendingIpTlsStore((s) => s.pendingCount);
  const approvePending = usePendingIpTlsStore((s) => s.approvePending);
  const skipPending = usePendingIpTlsStore((s) => s.skipPending);
  const clearPending = usePendingIpTlsStore((s) => s.clearPending);
  const navigate = useNavigate();
  const { t } = useI18n();

  const handleApprove = useCallback(
    async (ip: string) => {
      const ok = await approvePending(ip);
      if (ok) {
        message.success(t("app.pendingIpTls.enabled", { ip }));
      } else {
        message.error(t("app.pendingIpTls.enableFailed", { ip }));
      }
    },
    [approvePending, t],
  );

  const handleSkip = useCallback(
    async (ip: string) => {
      const ok = await skipPending(ip);
      if (ok) {
        message.success(t("app.pendingIpTls.skipped", { ip }));
      } else {
        message.error(t("app.pendingIpTls.skipFailed", { ip }));
      }
    },
    [skipPending, t],
  );

  const handleClearAll = useCallback(async () => {
    const ok = await clearPending();
    if (ok) {
      message.success(t("app.pendingIpTls.cleared"));
    } else {
      message.error(t("app.pendingIpTls.clearFailed"));
    }
  }, [clearPending, t]);

  const handleGoToSettings = useCallback(() => {
    navigate("/settings?tab=tls");
  }, [navigate]);

  return (
    <Modal
      open={pendingCount > 0}
      title={
        <Space>
          <SafetyCertificateOutlined style={{ color: "#1890ff" }} />
          <span>{t("app.pendingIpTls.title")}</span>
          <Badge
            count={pendingCount}
            style={{ backgroundColor: "#1890ff" }}
          />
        </Space>
      }
      footer={
        <Space>
          {pendingList.length > 0 && (
            <Popconfirm
              title={t("app.pendingIpTls.clearConfirmTitle")}
              description={t("app.pendingIpTls.clearConfirmDescription")}
              onConfirm={handleClearAll}
              okText={t("common.yes")}
              cancelText={t("common.no")}
            >
              <Button icon={<ClearOutlined />}>
                {t("app.pending.clearAll")}
              </Button>
            </Popconfirm>
          )}
          <Button
            icon={<SettingOutlined />}
            onClick={handleGoToSettings}
          >
            {t("app.pendingIpTls.tlsSettings")}
          </Button>
        </Space>
      }
      closable={false}
      maskClosable={false}
      keyboard={false}
      centered
      width={560}
      zIndex={999}
    >
      <div>
        <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
          {t("app.pendingIpTls.description")}
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
                >
                  {t("app.pendingIpTls.enableTls")}
                </Button>,
                <Button
                  key="skip"
                  size="small"
                  icon={<StopOutlined />}
                  onClick={() => handleSkip(item.ip)}
                >
                  {t("app.pendingIpTls.skip")}
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
