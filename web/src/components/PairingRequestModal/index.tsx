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
import { useI18n, type TranslationKey } from "../../i18n";

const { Text } = Typography;
const DEFAULT_SSH_KEY_SHELL_POLICY_ID = "ssh-key-full-access";

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

const GRANT_OPTIONS: { labelKey: TranslationKey; value: GrantMode; color: string }[] = [
  { labelKey: "app.pairing.grantOnce", value: "once", color: "default" },
  { labelKey: "app.pairingRequest.allow30m", value: "30m", color: "blue" },
  { labelKey: "app.pairingRequest.allow1h", value: "1h", color: "cyan" },
  { labelKey: "app.pairingRequest.allow1d", value: "1d", color: "green" },
  { labelKey: "app.pairingRequest.allowPermanently", value: "permanent", color: "gold" },
];

type AccessPreset = "query" | "full" | "shell_only" | "file_only" | "custom";

export default function PairingRequestModal({
  visible,
  pairing,
  shellConfig,
  onClose,
}: PairingRequestModalProps) {
  const { t } = useI18n();
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
    setPreset("full");
    setShellMode("all");
    setStdinAllowed(false);
    setInteractiveAllowed(false);
    setFileAccess("none");
  }, [pairing?.pairing_id, enabledPolicies]);

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
          policy_binding: { mode: "selected" as const, policy_ids: [DEFAULT_SSH_KEY_SHELL_POLICY_ID] },
          interactive_allowed: true,
          stdin_allowed: true,
        };
      case "shell_only":
        return {
          grant_scope: "remote_shell_exec" as const,
          file_access: "read_write" as const,
          policy_binding: { mode: "selected" as const, policy_ids: [DEFAULT_SSH_KEY_SHELL_POLICY_ID] },
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
      message.success(t("app.pairingRequest.approved"));
      onClose();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("app.pairingRequest.approveFailed"),
      );
    } finally {
      setLoading(false);
    }
  };

  const handleReject = async () => {
    setLoading(true);
    try {
      await rejectPairing(pairing.pairing_id);
      message.success(t("app.pairingRequest.rejected"));
      onClose();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("app.pairingRequest.rejectFailed"),
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
          <span>{t("app.pairingRequest.title")}</span>
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
          <Descriptions.Item label={t("app.pairingRequest.deviceFingerprint")}>
            <Text code style={{ fontFamily: "monospace" }}>
              {formatFingerprint(pairing.caller_info.fingerprint)}
            </Text>
          </Descriptions.Item>
          <Descriptions.Item label={t("app.pairingRequest.displayName")}>
            {pairing.caller_info.display_name || "-"}
          </Descriptions.Item>
          <Descriptions.Item label={t("app.pairingRequest.sourceIp")}>
            {pairing.caller_info.source_ip || "-"}
          </Descriptions.Item>
          <Descriptions.Item label={t("app.pairingRequest.platform")}>
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
          <Descriptions.Item label={t("app.pairingRequest.command")}>
            <Tag color="blue">
              {pairing.command_summary.command_preview || pairing.command.command}
            </Tag>
          </Descriptions.Item>
        </Descriptions>

        <Tag icon={<WarningOutlined />} color="warning">
          {t("app.pairingRequest.newDeviceWarning")}
        </Tag>

        <div>
          <Text strong style={{ display: "block", marginBottom: 8 }}>
            {t("app.pairingRequest.accessTitle")}
          </Text>
          <Text type="secondary" style={{ display: "block", marginBottom: 8, fontSize: 12 }}>
            {t("app.pairingRequest.accessHint")}
          </Text>
          {!hasShellPolicies && (
            <Alert
              showIcon
              type="warning"
              style={{ marginBottom: 12 }}
              message={t("app.pairingRequest.noCommandGroups")}
              description={t("app.pairingRequest.noCommandGroupsDescription")}
            />
          )}
          <Radio.Group
            value={preset}
            onChange={(e) => setPreset(e.target.value)}
            style={{ width: "100%", marginBottom: 16 }}
          >
            <Space direction="vertical" style={{ width: "100%" }} size={4}>
              <Radio value="full">
                <Space>
                  <ThunderboltOutlined />
                  <span><b>{t("app.pairingRequest.presetFull")}</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("app.pairingRequest.presetFullDescription")}
                  </Text>
                </Space>
              </Radio>
              <Radio value="shell_only">
                <Space>
                  <CodeOutlined />
                  <span><b>{t("app.pairingRequest.presetShell")}</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("app.pairingRequest.presetShellDescription")}
                  </Text>
                </Space>
              </Radio>
              <Radio value="file_only">
                <Space>
                  <FileOutlined />
                  <span><b>{t("app.pairingRequest.presetFile")}</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("app.pairingRequest.presetFileDescription")}
                  </Text>
                </Space>
              </Radio>
              <Radio value="query">
                <Space>
                  <SearchOutlined />
                  <span><b>{t("app.pairingRequest.presetQuery")}</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("app.pairingRequest.presetQueryDescription")}
                  </Text>
                </Space>
              </Radio>
              <Radio value="custom">
                <Space>
                  <SettingOutlined />
                  <span><b>{t("app.pairingRequest.presetCustom")}</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("app.pairingRequest.presetCustomDescription")}
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
            message={t("app.pairingRequest.previewTitle")}
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
                    if (preset === "full" || preset === "shell_only") {
                      bullets.push(t("app.pairingRequest.previewRunFromAny", { names: "Full Access" }));
                    } else if (preset === "custom" && shellMode === "selected") {
                      const names = enabledPolicies
                        .filter((p) => selectedPolicies.includes(p.id))
                        .map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? t("app.pairingRequest.previewRunFrom", { names: names.join(", ") })
                          : t("app.pairingRequest.previewRunFromNone")
                      );
                    } else {
                      const names = enabledPolicies.map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? t("app.pairingRequest.previewRunFromAny", { names: names.join(", ") })
                          : t("app.pairingRequest.noCommandGroups")
                      );
                    }
                    bullets.push(
                      isInteractive
                        ? t("app.pairingRequest.previewInteractive")
                        : acceptsStdin
                        ? t("app.pairingRequest.previewStdinOnly")
                        : t("app.pairingRequest.previewNoStdin")
                    );
                  } else {
                    bullets.push(t("app.pairingRequest.previewNoShell"));
                  }

                  if (fileScope === "read_write") {
                    bullets.push(t("app.pairingRequest.previewReadWrite"));
                  } else if (fileScope === "read") {
                    bullets.push(t("app.pairingRequest.previewReadOnly"));
                  } else {
                    bullets.push(t("app.pairingRequest.previewNoFileAccess"));
                  }

                  bullets.push(t("app.pairingRequest.previewAlwaysStatus"));
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
                  <Text type="secondary" style={{ fontSize: 12 }}>{t("app.pairingRequest.customShellQuestion")}</Text>
                  <Select
                    value={shellMode}
                    onChange={setShellMode}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: t("app.pairingRequest.shellNone") },
                      {
                        value: "selected",
                        label: t("app.pairingRequest.shellSelected"),
                        disabled: !hasShellPolicies,
                      },
                      {
                        value: "all",
                        label: t("app.pairingRequest.shellAll"),
                        disabled: !hasShellPolicies,
                      },
                    ]}
                  />
                </div>
                {shellMode === "selected" && (
                  <Select
                    mode="multiple"
                    placeholder={t("app.pairingRequest.choosePolicies")}
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
                    <Text type="secondary">{t("app.pairingRequest.allowStdin")}</Text>
                    <Switch
                      checked={stdinAllowed}
                      onChange={setStdinAllowed}
                    />
                    <Text type="secondary">{t("app.pairingRequest.interactiveTerminal")}</Text>
                    <Switch
                      checked={interactiveAllowed}
                      onChange={setInteractiveAllowed}
                    />
                  </Space>
                )}
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>{t("app.pairingRequest.customFileQuestion")}</Text>
                  <Select
                    value={fileAccess}
                    onChange={setFileAccess}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: t("app.pairingRequest.fileNone") },
                      { value: "read", label: t("app.pairingRequest.fileRead") },
                      { value: "read_write", label: t("app.pairingRequest.fileReadWrite") },
                    ]}
                  />
                </div>
              </Space>
            </div>
          )}

          <Text strong style={{ display: "block", marginBottom: 8 }}>
            {t("app.pairingRequest.grantDuration")}
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
                {t(opt.labelKey)}
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
          {t("app.pairing.reject")}
        </Button>
      </Space>
    </Modal>
  );
}
