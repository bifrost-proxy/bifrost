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
          grant_scope: "remote_shell_exec" as const,
          file_access: "read_write" as const,
          policy_binding: { mode: "all" as const },
          interactive_allowed: false,
          stdin_allowed: false,
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
          <span>A remote device wants to connect</span>
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
            How much access to grant
          </Text>
          <Text type="secondary" style={{ display: "block", marginBottom: 8, fontSize: 12 }}>
            Pick the smallest scope that lets the remote agent do its job.
          </Text>
          {!hasShellPolicies && (
            <Alert
              showIcon
              type="warning"
              style={{ marginBottom: 12 }}
              message="No command groups enabled on this device"
              description="This device has no command groups that agents are allowed to run. The shell-related presets below are disabled. You can still grant Files-only or Read-only access, or open Shell access settings to add a group first."
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
                  <span><b>Full trust</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Run any allowed command, read &amp; write files, open interactive terminals
                  </Text>
                </Space>
              </Radio>
              <Radio value="shell_only" disabled={!hasShellPolicies}>
                <Space>
                  <CodeOutlined />
                  <span><b>Run commands &amp; read/write files</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Execute allowed commands and edit files. No interactive shells.
                  </Text>
                </Space>
              </Radio>
              <Radio value="file_only">
                <Space>
                  <FileOutlined />
                  <span><b>Files only</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Read &amp; write files. Cannot run any shell commands.
                  </Text>
                </Space>
              </Radio>
              <Radio value="query">
                <Space>
                  <SearchOutlined />
                  <span><b>Read-only watch</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Can see status and traffic. No commands, no file writes.
                  </Text>
                </Space>
              </Radio>
              <Radio value="custom">
                <Space>
                  <SettingOutlined />
                  <span><b>Custom</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Pick shell + file + terminal access individually
                  </Text>
                </Space>
              </Radio>
            </Space>
          </Radio.Group>

          <Alert
            showIcon
            type={
              preset === "full"
                ? "warning"
                : preset === "query"
                ? "success"
                : "info"
            }
            style={{ marginBottom: 16 }}
            message="Preview — what this device will be able to do"
            description={
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {(() => {
                  const bullets: string[] = [];
                  const shellAllowed =
                    preset === "full" ||
                    preset === "shell_only" ||
                    (preset === "custom" && shellMode !== "none");
                  const fileScope =
                    preset === "full" || preset === "shell_only"
                      ? "read_write"
                      : preset === "file_only"
                      ? "read_write"
                      : preset === "query"
                      ? "none"
                      : fileAccess;
                  const isInteractive =
                    preset === "full" || (preset === "custom" && interactiveAllowed);
                  const acceptsStdin =
                    preset === "full" || (preset === "custom" && stdinAllowed);

                  if (shellAllowed) {
                    if (preset === "custom" && shellMode === "selected") {
                      const names = enabledPolicies
                        .filter((p) => selectedPolicies.includes(p.id))
                        .map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? "Run commands from: " + names.join(", ")
                          : "Run commands from: (none selected yet)"
                      );
                    } else {
                      const names = enabledPolicies.map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? "Run commands from any of: " + names.join(", ")
                          : "No command groups enabled on this device"
                      );
                    }
                    bullets.push(
                      isInteractive
                        ? "Can open an interactive terminal"
                        : acceptsStdin
                        ? "Can send stdin to commands, but no interactive terminal"
                        : "No stdin, no interactive terminal"
                    );
                  } else {
                    bullets.push("Cannot run any shell commands");
                  }

                  if (fileScope === "read_write") {
                    bullets.push("Can read and write files on this device");
                  } else if (fileScope === "read") {
                    bullets.push("Can read files but cannot modify them");
                  } else {
                    bullets.push("No file access");
                  }

                  bullets.push("Can always see status and inspect traffic records");
                  return bullets.map((b, i) => <li key={i}>{b}</li>);
                })()}
              </ul>
            }
          />

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
                  <Text type="secondary" style={{ fontSize: 12 }}>What commands can this device run?</Text>
                  <Select
                    value={shellMode}
                    onChange={setShellMode}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: "No commands at all" },
                      {
                        value: "selected",
                        label: "Only commands from selected groups",
                        disabled: !hasShellPolicies,
                      },
                      {
                        value: "all",
                        label: "Any command from enabled groups",
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
                    <Text type="secondary">Allow stdin (piped input)</Text>
                    <Switch
                      checked={stdinAllowed}
                      onChange={setStdinAllowed}
                    />
                    <Text type="secondary">Interactive terminal</Text>
                    <Switch
                      checked={interactiveAllowed}
                      onChange={setInteractiveAllowed}
                    />
                  </Space>
                )}
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>What file access to grant?</Text>
                  <Select
                    value={fileAccess}
                    onChange={setFileAccess}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: "No file access (cannot read or write files)" },
                      { value: "read", label: "Read-only (can open files, cannot modify)" },
                      { value: "read_write", label: "Read & write (can modify files)" },
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
