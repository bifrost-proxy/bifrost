import { useState } from "react";
import {
  Button,
  Descriptions,
  Modal,
  Space,
  Tag,
  Typography,
  message,
} from "antd";
import {
  CheckOutlined,
  CloseOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import {
  approvePairing,
  rejectPairing,
  type GrantMode,
  type PairingRequest,
} from "../../api/remoteInvoke";

const { Text } = Typography;

interface PairingRequestModalProps {
  visible: boolean;
  pairing: PairingRequest | null;
  onClose: () => void;
}

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

const GRANT_OPTIONS: { label: string; value: GrantMode; color: string }[] = [
  { label: "This Time Only", value: "once", color: "default" },
  { label: "Allow 30m", value: "30m", color: "blue" },
  { label: "Allow 1h", value: "1h", color: "cyan" },
  { label: "Allow 1d", value: "1d", color: "green" },
  { label: "Allow Permanently", value: "permanent", color: "gold" },
];

export default function PairingRequestModal({
  visible,
  pairing,
  onClose,
}: PairingRequestModalProps) {
  const [loading, setLoading] = useState(false);

  if (!pairing) return null;

  const handleApprove = async (mode: GrantMode) => {
    setLoading(true);
    try {
      await approvePairing(pairing.pairing_id, mode);
      message.success("Pairing approved");
      onClose();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to approve pairing",
      );
    } finally {
      setLoading(false);
    }
  };

  const handleReject = async () => {
    setLoading(true);
    try {
      await rejectPairing(pairing.pairing_id);
      message.success("Pairing rejected");
      onClose();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to reject pairing",
      );
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal
      open={visible}
      title={
        <Space>
          <WarningOutlined style={{ color: "#faad14" }} />
          <span>Pairing Request</span>
        </Space>
      }
      onCancel={onClose}
      footer={null}
      centered
      width={560}
      data-testid="pairing-request-modal"
    >
      <Space direction="vertical" size={16} style={{ width: "100%" }}>
        <Descriptions size="small" column={1} bordered>
          <Descriptions.Item label="Device Fingerprint">
            <Text code style={{ fontFamily: "monospace" }}>
              {formatFingerprint(pairing.caller_info.fingerprint)}
            </Text>
          </Descriptions.Item>
          <Descriptions.Item label="Display Name">
            {pairing.caller_info.display_name || "-"}
          </Descriptions.Item>
          <Descriptions.Item label="Source IP">
            {pairing.caller_info.source_ip || "-"}
          </Descriptions.Item>
          <Descriptions.Item label="Platform">
            {pairing.caller_info.platform || "-"}
          </Descriptions.Item>
          <Descriptions.Item label="User Agent">
            <Text
              type="secondary"
              style={{ fontSize: 11, wordBreak: "break-all" }}
            >
              {pairing.caller_info.user_agent || "-"}
            </Text>
          </Descriptions.Item>
          <Descriptions.Item label="Command">
            <Tag color="blue">
              {pairing.command_summary.command_preview || pairing.command.command}
            </Tag>
          </Descriptions.Item>
        </Descriptions>

        <Tag icon={<WarningOutlined />} color="warning">
          New device — please confirm this is your own operation
        </Tag>

        <div>
          <Text strong style={{ display: "block", marginBottom: 8 }}>
            Choose authorization scope:
          </Text>
          <Space wrap>
            {GRANT_OPTIONS.map((opt) => (
              <Button
                key={opt.value}
                type={opt.value === "once" ? "primary" : "default"}
                icon={<CheckOutlined />}
                loading={loading}
                onClick={() => handleApprove(opt.value)}
                data-testid={`pairing-approve-${opt.value}`}
              >
                {opt.label}
              </Button>
            ))}
          </Space>
        </div>

        <Button
          danger
          icon={<CloseOutlined />}
          onClick={handleReject}
          loading={loading}
          block
          data-testid="pairing-reject"
        >
          Reject
        </Button>
      </Space>
    </Modal>
  );
}
