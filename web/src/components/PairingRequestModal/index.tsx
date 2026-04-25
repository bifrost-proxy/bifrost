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
  Radio,
} from "antd";
import {
  CheckOutlined,
  CloseOutlined,
  WarningOutlined,
  ThunderboltOutlined,
  FileOutlined,
  CodeOutlined,
  SearchOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import {
  approvePairing,
  rejectPairing,
  type GrantMode,
  type PairingRequest,
  type RemoteShellSet,
  type FileAccessScope,
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

type AccessPreset = "query" | "full" | "shell_only" | "file_only" | "custom";

export default function PairingRequestModal({
  visible,
  pairing,
  shellConfig,
  onClose,
}: PairingRequestModalProps) {
  const [loading, setLoading] = useState(false);
  const [preset, setPreset] = useState<AccessPreset>("full");

  // Custom mode fine-grained controls
  const [shellMode, setShellMode] = useState<"none" | "selected" | "all">("all");
  const [selectedPolicies, setSelectedPolicies] = useState<string[]>([]);
  const [stdinAllowed, setStdinAllowed] = useState(false);
  const [interactiveAllowed, setInteractiveAllowed] = useState(false);
  const [fileAccess, setFileAccess] = useState<FileAccessScope>("none");

  const enabledPolicies = useMemo(
    () => (shellConfig?.policies ?? []).filter((policy) => policy.enabled),
    [shellConfig],
  );

  const hasShellPolicies = enabledPolicies.length > 0;

  useEffect(() => {
    if (enabledPolicies.length > 0) {
      setSelectedPolicies([enabledPolicies[0].id]);
    } else {
      setSelectedPolicies([]);
    }
    setPreset(hasShellPolicies ? "full" : "query");
    setShellMode("all");
    setStdinAllowed(false);
    setInteractiveAllowed(false);
    setFileAccess("none");
  }, [pairing?.pairing_id, enabledPolicies, hasShellPolicies]);

  if (!pairing) return null;

  // Resolve preset to actual grant parameters
  const resolveParams = () => {
    switch (preset) {
      case "query":
        return {
          grant_scope: "remote_query" as const,
          file_access: "none" as const,
        };
      case "full":
        return {
          grant_scope: "remote_shell_interactive" as const,
          file_access: "read_write" as const,
          policy_binding: { mode: "all" as const },
          interactive_allowed: true,
          stdin_allowed: true,
        };
      case "shell_only":
        return {
          grant_scope: "remote_shell_interactive" as const,
          file_access: "none" as const,
          policy_binding: { mode: "all" as const },
          interactive_allowed: true,
          stdin_allowed: true,
        };
      case "file_only":
        return {
          grant_scope: "remote_query" as const,
          file_access: "read_write" as const,
        };
      case "custom":
        if (shellMode === "none") {
          return {
            grant_scope: "remote_query" as const,
            file_access: fileAccess,
          };
        }
        return {
          grant_scope: interactiveAllowed
            ? ("remote_shell_interactive" as const)
            : ("remote_shell_exec" as const),
          file_access: fileAccess,
          policy_binding:
            shellMode === "all"
              ? { mode: "all" as const }
              : { mode: "selected" as const, policy_ids: selectedPolicies },
          interactive_allowed: interactiveAllowed,
          stdin_allowed: stdinAllowed,
        };
    }
  };

  const handleApprove = async (mode: GrantMode) => {
    setLoading(true);
    try {
      const params = resolveParams();
      await approvePairing(pairing.pairing_id, {
        grant_mode: mode,
        ...params,
      });
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

  const isApproveDisabled =
    preset === "custom" &&
    shellMode === "selected" &&
    selectedPolicies.length === 0;

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
            Access Level
          </Text>
          {!hasShellPolicies && (
            <Alert
              showIcon
              type="warning"
              style={{ marginBottom: 12 }}
              message="No enabled shell policy found on this device"
              description="Shell-related presets are unavailable. Configure Shell Access policies first, or use Query/File only."
            />
          )}
          <Radio.Group
            value={preset}
            onChange={(e) => setPreset(e.target.value)}
            style={{ width: "100%", marginBottom: 16 }}
          >
            <Space direction="vertical" style={{ width: "100%" }} size={4}>
              <Radio value="full" disabled={!hasShellPolicies}>
                <Space>
                  <ThunderboltOutlined />
                  <span>Full Access</span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Shell + File + Interactive
                  </Text>
                </Space>
              </Radio>
              <Radio value="shell_only" disabled={!hasShellPolicies}>
                <Space>
                  <CodeOutlined />
                  <span>Shell Only</span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Execute commands, no file access
                  </Text>
                </Space>
              </Radio>
              <Radio value="file_only">
                <Space>
                  <FileOutlined />
                  <span>File Only</span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Read & write files, no shell
                  </Text>
                </Space>
              </Radio>
              <Radio value="query">
                <Space>
                  <SearchOutlined />
                  <span>Query Only</span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Read-only queries
                  </Text>
                </Space>
              </Radio>
              <Radio value="custom">
                <Space>
                  <SettingOutlined />
                  <span>Custom</span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Configure individually
                  </Text>
                </Space>
              </Radio>
            </Space>
          </Radio.Group>

          {preset === "custom" && (
            <div
              style={{
                padding: 12,
                background: "var(--ant-color-bg-container-disabled, #fafafa)",
                borderRadius: 8,
                marginBottom: 16,
              }}
            >
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>Shell Access</Text>
                  <Select
                    value={shellMode}
                    onChange={setShellMode}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: "No shell access" },
                      {
                        value: "selected",
                        label: "Selected shell policies",
                        disabled: !hasShellPolicies,
                      },
                      {
                        value: "all",
                        label: "All enabled shell policies",
                        disabled: !hasShellPolicies,
                      },
                    ]}
                  />
                </div>
                {shellMode === "selected" && (
                  <Select
                    mode="multiple"
                    placeholder="Choose shell policies"
                    value={selectedPolicies}
                    onChange={setSelectedPolicies}
                    style={{ width: "100%" }}
                    options={enabledPolicies.map((policy) => ({
                      value: policy.id,
                      label: `${policy.name} (${policy.id})`,
                    }))}
                  />
                )}
                {shellMode !== "none" && (
                  <Space>
                    <Text type="secondary">Allow stdin</Text>
                    <Switch
                      checked={stdinAllowed}
                      onChange={setStdinAllowed}
                    />
                    <Text type="secondary">Interactive</Text>
                    <Switch
                      checked={interactiveAllowed}
                      onChange={setInteractiveAllowed}
                    />
                  </Space>
                )}
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>File Access</Text>
                  <Select
                    value={fileAccess}
                    onChange={setFileAccess}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: "No file access" },
                      { value: "read", label: "Read only" },
                      { value: "read_write", label: "Read & Write" },
                    ]}
                  />
                </div>
              </Space>
            </div>
          )}

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
                disabled={isApproveDisabled}
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
