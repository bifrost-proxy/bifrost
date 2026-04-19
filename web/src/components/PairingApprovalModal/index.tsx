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

const { Text } = Typography;

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

const GRANT_MENU_ITEMS: { key: GrantMode; label: string }[] = [
  { key: "once", label: "This Time Only" },
  { key: "30m", label: "30 Minutes" },
  { key: "1h", label: "1 Hour" },
  { key: "1d", label: "1 Day" },
  { key: "permanent", label: "Permanent" },
];

function PairingItem({
  pairing,
  onApprove,
  onReject,
  onIgnore,
}: {
  pairing: PairingRequest;
  onApprove: (pairingId: string, mode: GrantMode) => void;
  onReject: (pairingId: string) => void;
  onIgnore: (pairingId: string) => void;
}) {
  const [loading, setLoading] = useState(false);

  const handleApprove = async (mode: GrantMode) => {
    setLoading(true);
    onApprove(pairing.pairing_id, mode);
  };

  const handleReject = () => {
    setLoading(true);
    onReject(pairing.pairing_id);
  };

  return (
    <List.Item
      actions={[
        <Dropdown
          key="approve"
          menu={{
            items: GRANT_MENU_ITEMS.map((item) => ({
              key: item.key,
              label: item.label,
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
            Authorize
          </Button>
        </Dropdown>,
        <Button
          key="reject"
          danger
          size="small"
          icon={<CloseOutlined />}
          loading={loading}
          onClick={handleReject}
          data-testid={`pairing-reject-${pairing.pairing_id}`}
        >
          Reject
        </Button>,
        <Button
          key="ignore"
          size="small"
          icon={<EyeInvisibleOutlined />}
          onClick={() => onIgnore(pairing.pairing_id)}
          data-testid={`pairing-ignore-${pairing.pairing_id}`}
        >
          Dismiss
        </Button>,
      ]}
    >
      <List.Item.Meta
        avatar={
          <Tooltip title={pairing.caller_info.fingerprint}>
            <Tag color="blue" style={{ fontFamily: "monospace", fontSize: 11 }}>
              {formatFingerprint(pairing.caller_info.fingerprint)}
            </Tag>
          </Tooltip>
        }
        title={
          <Space size={4} wrap>
            <Text strong>
              {pairing.caller_info.display_name || "Unknown Caller"}
            </Text>
            {pairing.caller_info.source_ip && (
              <Text type="secondary" style={{ fontSize: 12 }}>
                from {pairing.caller_info.source_ip}
              </Text>
            )}
          </Space>
        }
        description={
          <Space size={4} wrap>
            <Tag color="geekblue">
              {pairing.command_summary.command_preview || pairing.command.command}
            </Tag>
            {pairing.caller_info.platform && (
              <Text type="secondary" style={{ fontSize: 11 }}>
                {pairing.caller_info.platform}
              </Text>
            )}
          </Space>
        }
      />
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

  const activePairings = useMemo(
    () => pendingList.filter((p) => !ignoredIds.includes(p.pairing_id)),
    [pendingList, ignoredIds],
  );

  const handleApprove = useCallback(
    async (pairingId: string, grantMode: GrantMode) => {
      const ok = await storeApprove(pairingId, grantMode);
      if (ok) {
        message.success("Authorization granted");
      } else {
        message.error("Failed to authorize");
      }
    },
    [storeApprove],
  );

  const handleReject = useCallback(
    async (pairingId: string) => {
      const ok = await storeReject(pairingId);
      if (ok) {
        message.success("Request rejected");
      } else {
        message.error("Failed to reject");
      }
    },
    [storeReject],
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
          <span>Remote Command Authorization</span>
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
            Dismiss All
          </Button>
          <Button
            icon={<SettingOutlined />}
            onClick={handleGoToSettings}
            data-testid="pairing-modal-settings"
          >
            Remote Invoke Settings
          </Button>
        </Space>
      }
      onCancel={handleClose}
      centered
      width={620}
      zIndex={999}
      data-testid="pairing-approval-modal"
    >
      <div data-testid="pairing-approval-modal-content">
        <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
          A remote caller is requesting permission to execute commands on this
          Bifrost instance. Review the details below and choose to authorize,
          reject, or dismiss each request.
        </Text>
        <List
          size="small"
          dataSource={activePairings}
          locale={{ emptyText: "No pending requests" }}
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
