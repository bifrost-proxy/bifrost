import { useCallback, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Badge,
  Button,
  Dropdown,
  List,
  Modal,
  Space,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import {
  CheckOutlined,
  CloseOutlined,
  EyeInvisibleOutlined,
  SettingOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import { usePairingRequestStore } from "../../stores/usePairingRequestStore";
import type { GrantMode, PairingRequest } from "../../api/remoteInvoke";
import { useI18n, type TranslationKey } from "../../i18n";

const { Text } = Typography;

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

const GRANT_MENU_ITEMS: { key: GrantMode; labelKey: TranslationKey }[] = [
  { key: "once", labelKey: "app.pairing.grantOnce" },
  { key: "30m", labelKey: "app.pairing.grant30m" },
  { key: "1h", labelKey: "app.pairing.grant1h" },
  { key: "1d", labelKey: "app.pairing.grant1d" },
  { key: "permanent", labelKey: "app.pairing.grantPermanent" },
];

function PairingItem({
  pairing,
  onApprove,
  onReject,
  onIgnore,
}: {
  pairing: PairingRequest;
  onApprove: (pairingId: string, mode: GrantMode) => Promise<void>;
  onReject: (pairingId: string) => Promise<void>;
  onIgnore: (pairingId: string) => void;
}) {
  const [loading, setLoading] = useState(false);
  const { t } = useI18n();

  const handleApprove = async (mode: GrantMode) => {
    setLoading(true);
    try {
      await onApprove(pairing.pairing_id, mode);
    } finally {
      setLoading(false);
    }
  };

  const handleReject = async () => {
    setLoading(true);
    try {
      await onReject(pairing.pairing_id);
    } finally {
      setLoading(false);
    }
  };

  return (
    <List.Item>
      <div style={{ width: "100%" }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            flexWrap: "wrap",
          }}
        >
          <Tooltip title={pairing.caller_info.fingerprint}>
            <Tag color="blue" style={{ fontFamily: "monospace", fontSize: 11 }}>
              {formatFingerprint(pairing.caller_info.fingerprint)}
            </Tag>
          </Tooltip>
          <Text strong>
            {pairing.caller_info.display_name || t("app.pairing.unknownCaller")}
          </Text>
          {pairing.caller_info.source_ip && (
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("app.pairing.fromIp", { ip: pairing.caller_info.source_ip })}
            </Text>
          )}
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            marginTop: 8,
          }}
        >
          <Space size={4}>
            <Tag color="geekblue">
              {pairing.command_summary.command_preview || pairing.command.command}
            </Tag>
            {pairing.caller_info.platform && (
              <Text type="secondary" style={{ fontSize: 11 }}>
                {pairing.caller_info.platform}
              </Text>
            )}
          </Space>
          <Space size={4} style={{ flexShrink: 0 }}>
            <Dropdown
              menu={{
                items: GRANT_MENU_ITEMS.map((item) => ({
                  key: item.key,
                  label: t(item.labelKey),
                })),
                onClick: ({ key }) => handleApprove(key as GrantMode),
              }}
              trigger={["click"]}
            >
              <Button
                type="primary"
                size="small"
                icon={<CheckOutlined />}
                loading={loading}
                data-testid={`pairing-approve-${pairing.pairing_id}`}
              >
                {t("app.pairing.authorize")}
              </Button>
            </Dropdown>
            <Button
              danger
              size="small"
              icon={<CloseOutlined />}
              loading={loading}
              onClick={handleReject}
              data-testid={`pairing-reject-${pairing.pairing_id}`}
            >
              {t("app.pairing.reject")}
            </Button>
            <Button
              size="small"
              icon={<EyeInvisibleOutlined />}
              onClick={() => onIgnore(pairing.pairing_id)}
              data-testid={`pairing-ignore-${pairing.pairing_id}`}
            >
              {t("app.pairing.dismiss")}
            </Button>
          </Space>
        </div>
      </div>
    </List.Item>
  );
}

export default function PairingApprovalModal() {
  const pendingList = usePairingRequestStore((s) => s.pendingList);
  const ignoredIds = usePairingRequestStore((s) => s.ignoredIds);
  const storeApprove = usePairingRequestStore((s) => s.approvePairing);
  const storeReject = usePairingRequestStore((s) => s.rejectPairing);
  const storeIgnore = usePairingRequestStore((s) => s.ignorePairing);
  const storeIgnoreAll = usePairingRequestStore((s) => s.ignoreAll);
  const navigate = useNavigate();
  const { t } = useI18n();

  const activePairings = useMemo(
    () => pendingList.filter((p) => !ignoredIds.includes(p.pairing_id)),
    [pendingList, ignoredIds],
  );

  const handleApprove = useCallback(
    async (pairingId: string, grantMode: GrantMode) => {
      const ok = await storeApprove(pairingId, grantMode);
      if (ok) {
        message.success(t("app.pairing.authorizationGranted"));
      } else {
        message.warning(t("app.pairing.requestExpired"));
      }
    },
    [storeApprove, t],
  );

  const handleReject = useCallback(
    async (pairingId: string) => {
      const ok = await storeReject(pairingId);
      if (ok) {
        message.success(t("app.pairing.requestRejected"));
      } else {
        message.warning(t("app.pairing.requestExpired"));
      }
    },
    [storeReject, t],
  );

  const handleIgnore = useCallback(
    (pairingId: string) => {
      storeIgnore(pairingId);
    },
    [storeIgnore],
  );

  const handleClose = useCallback(() => {
    storeIgnoreAll();
  }, [storeIgnoreAll]);

  const handleGoToSettings = useCallback(() => {
    storeIgnoreAll();
    navigate("/settings?tab=remote-invoke");
  }, [storeIgnoreAll, navigate]);

  if (activePairings.length === 0) {
    return null;
  }

  return (
    <Modal
      open
      title={
        <Space>
          <WarningOutlined style={{ color: "#faad14" }} />
          <span>{t("app.pairing.approvalTitle")}</span>
          <Badge
            count={activePairings.length}
            style={{ backgroundColor: "#faad14" }}
          />
        </Space>
      }
      footer={
        <Space>
          <Button
            icon={<EyeInvisibleOutlined />}
            onClick={handleClose}
            data-testid="pairing-modal-dismiss-all"
          >
            {t("app.pairing.dismissAll")}
          </Button>
          <Button
            icon={<SettingOutlined />}
            onClick={handleGoToSettings}
            data-testid="pairing-modal-settings"
          >
            {t("app.pairing.remoteInvokeSettings")}
          </Button>
        </Space>
      }
      onCancel={handleClose}
      centered
      width={720}
      zIndex={999}
      data-testid="pairing-approval-modal"
    >
      <div data-testid="pairing-approval-modal-content">
        <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
          {t("app.pairing.approvalDescription")}
        </Text>
        <List
          size="small"
          dataSource={activePairings}
          locale={{ emptyText: t("app.pairing.noPendingRequests") }}
          renderItem={(item) => (
            <PairingItem
              key={item.pairing_id}
              pairing={item}
              onApprove={handleApprove}
              onReject={handleReject}
              onIgnore={handleIgnore}
            />
          )}
        />
      </div>
    </Modal>
  );
}
