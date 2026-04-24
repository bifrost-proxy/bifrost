import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Descriptions,
  Modal,
  Select,
  Space,
  Switch,
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
  type RemoteShellSet,
} from "../../api/remoteInvoke";

const { Text } = Typography;

interface PairingRequestModalProps {
  visible: boolean;
  pairing: PairingRequest | null;
  shellConfig?: RemoteShellSet | null;
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
  shellConfig,
  onClose,
}: PairingRequestModalProps) {
  const [loading, setLoading] = useState(false);
  const [accessMode, setAccessMode] = useState<"query" | "selected" | "all">("query");
  const [selectedPolicies, setSelectedPolicies] = useState<string[]>([]);
  const [stdinAllowed, setStdinAllowed] = useState(false);
  const [interactiveAllowed, setInteractiveAllowed] = useState(false);

  const enabledPolicies = useMemo(
    () => (shellConfig?.policies ?? []).filter((policy) => policy.enabled),
    [shellConfig],
  );

  useEffect(() => {
    if (enabledPolicies.length > 0) {
      setSelectedPolicies([enabledPolicies[0].id]);
    } else {
      setSelectedPolicies([]);
    }
    setAccessMode(enabledPolicies.length > 0 ? "selected" : "query");
    setStdinAllowed(false);
    setInteractiveAllowed(false);
  }, [pairing?.pairing_id, enabledPolicies]);

  if (!pairing) return null;

  const handleApprove = async (mode: GrantMode) => {
    setLoading(true);
    try {
      const input =
        accessMode === "query"
          ? {
              grant_mode: mode,
              grant_scope: "remote_query" as const,
            }
          : {
              grant_mode: mode,
              grant_scope: interactiveAllowed
                ? ("remote_shell_interactive" as const)
                : ("remote_shell_exec" as const),
              policy_binding:
                accessMode === "all"
                  ? { mode: "all" }
                  : { mode: "selected", policy_ids: selectedPolicies },
              interactive_allowed: interactiveAllowed,
              stdin_allowed: stdinAllowed,
            };
      await approvePairing(pairing.pairing_id, input);
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
            Access Decision
          </Text>
          {enabledPolicies.length === 0 ? (
            <Alert
              showIcon
              type="warning"
              style={{ marginBottom: 12 }}
              message="No enabled shell policy found on this device"
              description="Only read-only query access can be granted until you configure and enable at least one Shell Access policy."
            />
          ) : null}
          <Space direction="vertical" size={12} style={{ width: "100%", marginBottom: 16 }}>
            <Select
              value={accessMode}
              onChange={(value) => setAccessMode(value)}
              options={[
                {
                  value: "query",
                  label: "Read-only queries",
                },
                {
                  value: "selected",
                  label: "Selected shell policies",
                  disabled: enabledPolicies.length === 0,
                },
                {
                  value: "all",
                  label: "All enabled shell policies",
                  disabled: enabledPolicies.length === 0,
                },
              ]}
            />
            <Select
              mode="multiple"
              placeholder="Choose shell policies for this caller"
              disabled={accessMode !== "selected"}
              value={selectedPolicies}
              onChange={setSelectedPolicies}
              options={enabledPolicies.map((policy) => ({
                value: policy.id,
                label: `${policy.name} (${policy.id})`,
              }))}
            />
            <Space>
              <Text type="secondary">Allow stdin</Text>
              <Switch
                checked={stdinAllowed}
                disabled={accessMode === "query"}
                onChange={setStdinAllowed}
              />
              <Text type="secondary">Allow interactive shell</Text>
              <Switch
                checked={interactiveAllowed}
                disabled={accessMode === "query"}
                onChange={setInteractiveAllowed}
              />
            </Space>
          </Space>
          <Text strong style={{ display: "block", marginBottom: 8 }}>
            Grant Duration
          </Text>
          <Space wrap>
            {GRANT_OPTIONS.map((opt) => (
              <Button
                key={opt.value}
                type={opt.value === "once" ? "primary" : "default"}
                icon={<CheckOutlined />}
                loading={loading}
                disabled={
                  accessMode === "selected" && selectedPolicies.length === 0
                }
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
