import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Form,
  Input,
  List,
  Modal,
  Popconfirm,
  Row,
  Space,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import {
  ApiOutlined,
  CopyOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  EyeOutlined,
  HistoryOutlined,
  KeyOutlined,
  ReloadOutlined,
  SafetyOutlined,
  ScanOutlined,
  StopOutlined,
} from "@ant-design/icons";
import {
  createRemoteInvokeSshKey,
  enterDiscoveryMode,
  exitDiscoveryMode,
  getClientIdentity,
  getRemoteInvokeStatus,
  getRemoteInvokeSshKey,
  getRemoteInvokeSshPrivateKey,
  listCalls,
  listGrants,
  refreshPairCode,
  resetRemoteInvokeSshKey,
  revokeGrant,
  revokeRemoteInvokeSshKey,
  type CreateRemoteInvokeSshKeyInput,
  type Call,
  type ClientIdentity,
  type DiscoverySession,
  type Grant,
  type GrantMode,
  type RemoteInvokeSshCallerInfo,
  type RemoteInvokeSshKeyRecord,
  type RemoteInvokeSshKeySecretPayload,
  type RemoteInvokeStatus,
} from "../../../api/remoteInvoke";
import { isConnectionIssueError, isNotFoundError } from "../../../api/client";
import { copyToClipboard } from "../../../utils/clipboard";
import { usePairingRequestStore } from "../../../stores/usePairingRequestStore";
import PairingRequestModal from "../../../components/PairingRequestModal";
import type { PairingRequest } from "../../../api/remoteInvoke";

const { Text, Title } = Typography;
const { TextArea } = Input;

const SSH_GRANT_MODE: { label: string; value: GrantMode } = {
  label: "Permanent",
  value: "permanent",
};

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

function formatArgsPreview(argsJson?: string | null): string {
  if (!argsJson) return "";
  try {
    const obj = JSON.parse(argsJson);
    return Object.entries(obj)
      .filter(([, v]) => v != null)
      .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join(" ");
  } catch {
    return argsJson.length > 60 ? argsJson.slice(0, 60) + "…" : argsJson;
  }
}

function formatBytes(bytes: number | null | undefined): string | null {
  if (bytes == null || bytes === 0) return null;
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

function formatTimestamp(value: string | number | null | undefined): string {
  if (value == null || value === "") return "-";
  const date = typeof value === "number" ? new Date(value) : new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function formatSshFingerprint(fingerprint?: string | null): string {
  if (!fingerprint) return "-";
  if (fingerprint.startsWith("SHA256:")) return fingerprint;
  return fingerprint.length > 48 ? `${fingerprint.slice(0, 24)}…` : fingerprint;
}

function formatCallerInfo(info: RemoteInvokeSshCallerInfo | null): string {
  if (!info) return "No SSH caller has connected yet.";

  const headline = [info.hostname, info.username && `as ${info.username}`]
    .filter(Boolean)
    .join(" ");
  const details = [info.source_ip ?? info.ip, info.platform]
    .filter(Boolean)
    .join(" · ");

  return [headline, details].filter(Boolean).join(" — ") || "No SSH caller has connected yet.";
}

function formatSshGrantMode(mode: GrantMode): string {
  return mode === "permanent" ? SSH_GRANT_MODE.label : mode;
}

function formatCountdown(expiresAt: number): string {
  const remaining = Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
  if (remaining <= 0) return "Expired";
  const m = Math.floor(remaining / 60);
  const s = remaining % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function getCallStatusColor(call: Call): string {
  switch (call.status) {
    case "completed":
      return call.exit_code === 0 ? "green" : "red";
    case "cancelled":
      return "orange";
    case "streaming":
    case "authorized":
    case "key_exchanged":
    case "pending":
    case "running":
      return "processing";
    case "failed":
    case "timeout":
      return "red";
    default:
      return "default";
  }
}

function renderStateTag(state: string) {
  switch (state.toLowerCase()) {
    case "connected":
      return <Tag color="green">Connected</Tag>;
    case "connecting":
    case "registering":
      return <Tag color="processing">Connecting</Tag>;
    case "reconnecting":
      return <Tag color="orange">Reconnecting</Tag>;
    case "disconnected":
      return <Tag color="default">Disconnected</Tag>;
    default:
      return <Tag>{state}</Tag>;
  }
}

export default function RemoteInvokeTab() {
  const [status, setStatus] = useState<RemoteInvokeStatus | null>(null);
  const [identity, setIdentity] = useState<ClientIdentity | null>(null);
  const [sshKey, setSshKey] = useState<RemoteInvokeSshKeyRecord | null>(null);
  const [sshApiAvailable, setSshApiAvailable] = useState(true);
  const [sshLoading, setSshLoading] = useState(false);
  const [sshAction, setSshAction] = useState<
    "create" | "download" | "reset" | "revoke" | null
  >(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [secretModal, setSecretModal] = useState<{
    title: string;
    description: string;
    payload: RemoteInvokeSshKeySecretPayload;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [discoveryLoading, setDiscoveryLoading] = useState(false);
  const [countdown, setCountdown] = useState("");
  const [grants, setGrants] = useState<Grant[]>([]);
  const [calls, setCalls] = useState<Call[]>([]);
  const pollRef = useRef<number | null>(null);
  const [sshForm] = Form.useForm<CreateRemoteInvokeSshKeyInput>();

  const [reviewPairing, setReviewPairing] = useState<PairingRequest | null>(null);
  const pendingPairings = usePairingRequestStore((s) => s.pendingList);
  const storeFetchPairings = usePairingRequestStore((s) => s.fetchPendingList);

  const refresh = useCallback(async () => {
    try {
      const [s, id] = await Promise.all([
        getRemoteInvokeStatus(),
        getClientIdentity(),
      ]);
      setStatus(s);
      setIdentity(id);
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch remote invoke status");
      }
    }
  }, []);

  const refreshGrants = useCallback(async () => {
    try {
      const res = await listGrants();
      const sorted = (res.grants ?? []).sort((a, b) => (b.created_at ?? 0) - (a.created_at ?? 0));
      setGrants(sorted);
    } catch {
      // ignore
    }
  }, []);

  const refreshCalls = useCallback(async () => {
    try {
      const res = await listCalls();
      const sorted = (res.calls ?? []).sort((a, b) => (b.started_at ?? 0) - (a.started_at ?? 0));
      setCalls(sorted);
    } catch {
      // ignore
    }
  }, []);

  const refreshSshKey = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setSshLoading(true);
      }

      try {
        const currentKey = await getRemoteInvokeSshKey();
        setSshApiAvailable(true);
        setSshKey(currentKey);
      } catch (e) {
        if (isNotFoundError(e)) {
          setSshApiAvailable(false);
          setSshKey(null);
          return;
        }
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : "Failed to load SSH key status",
          );
        }
      } finally {
        if (!silent) {
          setSshLoading(false);
        }
      }
    },
    [],
  );

  useEffect(() => {
    void refresh();
    void refreshGrants();
    void refreshCalls();
    void refreshSshKey();
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey]);

  useEffect(() => {
    pollRef.current = window.setInterval(() => {
      void refresh();
      void refreshGrants();
      void refreshCalls();
      if (sshApiAvailable) {
        void refreshSshKey({ silent: true });
      }
    }, 3000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey, sshApiAvailable]);

  useEffect(() => {
    const session = status?.discovery_session;
    if (!session) {
      setCountdown("");
      return;
    }
    const tick = () => {
      setCountdown(formatCountdown(session.expires_at));
    };
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [status?.discovery_session]);

  const handleEnterDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await enterDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success("Discovery mode enabled");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to enter discovery mode",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleExitDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      await exitDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: null } : prev,
      );
      message.success("Discovery mode disabled");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to exit discovery mode",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleRefreshCode = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await refreshPairCode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success("Pair code refreshed");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to refresh pair code",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleCopyCode = () => {
    const code = status?.discovery_session?.pair_code;
    if (code) {
      copyToClipboard(code);
      message.success("Pair code copied");
    }
  };

  const handleRevokeGrant = async (grantId: string) => {
    try {
      await revokeGrant(grantId);
      message.success("Grant revoked");
      void refreshGrants();
    } catch (e) {
      message.error(e instanceof Error ? e.message : "Failed to revoke grant");
    }
  };

  const openCreateModal = () => {
    sshForm.setFieldsValue({
      label: sshKey?.label ?? "",
      grant_mode: "permanent",
    });
    setEditorOpen(true);
  };

  const presentSecretPayload = (
    payload: RemoteInvokeSshKeySecretPayload,
    mode: "created" | "downloaded" | "reset",
  ) => {
    const titleMap = {
      created: "SSH key created",
      downloaded: "Bifrost key file",
      reset: "SSH key rotated",
    } as const;
    const descriptionMap = {
      created:
        "This Bifrost key file contains the device code and private key. Copy it now and store it securely.",
      downloaded:
        "Only trusted administrators should retrieve this file. Share it only with approved callers.",
      reset:
        "A new key pair is active now. Any previous SSH grants tied to the old key should be treated as revoked.",
    } as const;

    setSecretModal({
      title: titleMap[mode],
      description: descriptionMap[mode],
      payload,
    });
  };

  const handleSubmitSshForm = async () => {
    const values = await sshForm.validateFields();
    setSshAction("create");
    try {
      const payload = await createRemoteInvokeSshKey(values);
      presentSecretPayload(payload, "created");
      message.success("SSH key created");
      setEditorOpen(false);
      await refreshSshKey({ silent: true });
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to save SSH key settings",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleFetchPrivateKey = async () => {
    setSshAction("download");
    try {
      const payload = await getRemoteInvokeSshPrivateKey();
      presentSecretPayload(payload, "downloaded");
      message.success("SSH key file loaded");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to fetch key file",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleResetSshKey = async () => {
    setSshAction("reset");
    try {
      const payload = await resetRemoteInvokeSshKey();
      presentSecretPayload(payload, "reset");
      await refreshSshKey({ silent: true });
      void refreshGrants();
      message.success("SSH key rotated");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to rotate SSH key",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleRevokeSshKey = async () => {
    setSshAction("revoke");
    try {
      await revokeRemoteInvokeSshKey();
      setSshKey(null);
      void refreshGrants();
      message.success("SSH key revoked");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to revoke SSH key",
      );
    } finally {
      setSshAction(null);
    }
  };

  const discoverySession: DiscoverySession | null =
    status?.discovery_session ?? null;
  const pairingList = pendingPairings;

  return (
    <div data-testid="settings-remote-invoke-tab" style={{ paddingBottom: 20 }}>
      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Alert
            showIcon
            type="info"
            message="Remote Command Bridge"
            description="Allows authorized callers to execute read-only queries on this Bifrost instance via a relay server. Enter discovery mode and share the pair code to begin."
          />
        </Col>

        <Col xs={24} md={12} style={{ display: "flex" }}>
          <Card
            data-testid="settings-remote-invoke-status-card"
            title={
              <Space>
                <ApiOutlined />
                <span>Remote Status</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => {
                  setLoading(true);
                  void refresh().finally(() => setLoading(false));
                }}
                loading={loading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
            bodyStyle={{
              height: "100%",
              display: "flex",
              flexDirection: "column",
              gap: 16,
            }}
          >
            <div
              data-testid="settings-remote-invoke-connection-section"
              style={{
                paddingBottom: 16,
                borderBottom: "1px solid #f0f0f0",
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                Connection Status
              </Text>
              <Descriptions size="small" column={1}>
                <Descriptions.Item label="Relay Connection">
                  {status ? renderStateTag(status.state) : <Tag>Loading</Tag>}
                </Descriptions.Item>
                <Descriptions.Item label="Instance ID">
                  <Text code style={{ fontSize: 11 }}>
                    {identity?.instance_id ?? "-"}
                  </Text>
                </Descriptions.Item>
                <Descriptions.Item label="Device">
                  {identity?.device_name ?? "-"} ({identity?.platform ?? "-"})
                </Descriptions.Item>
                <Descriptions.Item label="Active Calls">
                  <Badge
                    count={status?.active_call_ids?.length ?? 0}
                    showZero
                    style={{
                      backgroundColor:
                        (status?.active_call_ids?.length ?? 0) > 0
                          ? "#52c41a"
                          : "#d9d9d9",
                    }}
                  />
                </Descriptions.Item>
              </Descriptions>
            </div>

            <div
              data-testid="settings-remote-invoke-discovery-section"
              style={{
                flex: 1,
                display: "flex",
                flexDirection: "column",
                minHeight: 180,
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                Discovery Mode
              </Text>
              {discoverySession ? (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 28,
                  }}
                >
                  <div style={{ textAlign: "center" }}>
                    <Title
                      level={2}
                      style={{
                        fontFamily: "monospace",
                        letterSpacing: "0.3em",
                        margin: 0,
                      }}
                    >
                      {discoverySession.pair_code}
                    </Title>
                    <Text type="secondary">
                      {countdown === "Expired" ? (
                        <Tag color="red">Expired</Tag>
                      ) : (
                        <>Expires in {countdown}</>
                      )}
                    </Text>
                  </div>
                  <Space wrap style={{ justifyContent: "center", width: "100%" }}>
                    <Button
                      icon={<CopyOutlined />}
                      onClick={handleCopyCode}
                      size="small"
                    >
                      Copy Code
                    </Button>
                    <Button
                      icon={<ReloadOutlined />}
                      onClick={handleRefreshCode}
                      loading={discoveryLoading}
                      size="small"
                    >
                      Refresh
                    </Button>
                    <Button
                      icon={<StopOutlined />}
                      onClick={handleExitDiscovery}
                      loading={discoveryLoading}
                      danger
                      size="small"
                    >
                      Exit Discovery
                    </Button>
                  </Space>
                </Space>
              ) : (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 20,
                    textAlign: "center",
                  }}
                >
                  <DisconnectOutlined
                    style={{ fontSize: 32, color: "#bfbfbf" }}
                  />
                  <Text type="secondary">
                    Not in discovery mode. Enter discovery to generate a pair
                    code.
                  </Text>
                  <Button
                    type="primary"
                    icon={<ScanOutlined />}
                    onClick={handleEnterDiscovery}
                    loading={discoveryLoading}
                    disabled={status?.state?.toLowerCase() !== "connected"}
                  >
                    Enter Discovery Mode
                  </Button>
                  {status?.state?.toLowerCase() !== "connected" && (
                    <Text type="warning" style={{ fontSize: 12 }}>
                      Relay must be connected before entering discovery mode.
                    </Text>
                  )}
                </Space>
              )}
            </div>
          </Card>
        </Col>

        <Col xs={24} md={12} style={{ display: "flex" }}>
          <Card
            data-testid="settings-remote-invoke-ssh-card"
            title={
              <Space>
                <KeyOutlined />
                <span>SSH Key</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshSshKey()}
                loading={sshLoading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
          >
            {!sshApiAvailable ? (
              <Alert
                showIcon
                type="warning"
                message="SSH key management is not available yet"
                description="This client does not expose the SSH key endpoints yet. Once the backend lands, this section will light up without further UI changes."
              />
            ) : sshKey ? (
              <Space direction="vertical" size={16} style={{ width: "100%" }}>
                <Descriptions
                  size="small"
                  column={1}
                  items={[
                    {
                      key: "label",
                      label: "Label",
                      children: sshKey.label,
                    },
                    {
                      key: "device_code",
                      label: "Device Code",
                      children: (
                        <Space size={8} wrap>
                          <Text code>{sshKey.device_code}</Text>
                          <Button
                            size="small"
                            icon={<CopyOutlined />}
                            onClick={() => {
                              copyToClipboard(sshKey.device_code);
                              message.success("Device code copied");
                            }}
                          >
                            Copy
                          </Button>
                        </Space>
                      ),
                    },
                    {
                      key: "fingerprint",
                      label: "Fingerprint",
                      children: (
                        <Text code style={{ fontSize: 11 }}>
                          {formatSshFingerprint(sshKey.ssh_key_fingerprint)}
                        </Text>
                      ),
                    },
                    {
                      key: "grant_mode",
                      label: "SSH Access",
                      children: `${formatSshGrantMode(sshKey.grant_mode)} until key revoke`,
                    },
                    {
                      key: "status",
                      label: "Status",
                      children: (
                        <Tag color={sshKey.status === "active" ? "green" : "default"}>
                          {sshKey.status}
                        </Tag>
                      ),
                    },
                    {
                      key: "last_used_at",
                      label: "Last Used",
                      children: formatTimestamp(sshKey.last_used_at),
                    },
                    {
                      key: "last_caller",
                      label: "Last Caller",
                      children: (
                        <Text type={sshKey.last_caller_info ? undefined : "secondary"}>
                          {formatCallerInfo(sshKey.last_caller_info)}
                        </Text>
                      ),
                    },
                  ]}
                />

                <Space wrap>
                  <Button
                    icon={<CopyOutlined />}
                    onClick={() => void handleFetchPrivateKey()}
                    loading={sshAction === "download"}
                  >
                    Copy Key File
                  </Button>
                  <Popconfirm
                    title="Rotate SSH key?"
                    description="This creates a new key pair, replaces the device code, and should revoke grants tied to the old key."
                    okText="Rotate"
                    cancelText="Cancel"
                    onConfirm={() => void handleResetSshKey()}
                  >
                    <Button loading={sshAction === "reset"}>
                      Reset Key
                    </Button>
                  </Popconfirm>
                  <Popconfirm
                    title="Revoke SSH key?"
                    description="SSH callers will lose access until a new key is created."
                    okText="Revoke"
                    cancelText="Cancel"
                    onConfirm={() => void handleRevokeSshKey()}
                  >
                    <Button
                      danger
                      icon={<DeleteOutlined />}
                      loading={sshAction === "revoke"}
                    >
                      Revoke Key
                    </Button>
                  </Popconfirm>
                </Space>
              </Space>
            ) : (
              <Space
                direction="vertical"
                size={12}
                style={{ width: "100%", textAlign: "center" }}
              >
                <KeyOutlined style={{ fontSize: 32, color: "#bfbfbf" }} />
                <Text type="secondary">
                  No active SSH key. Create one to provision long-lived callers without pair codes.
                </Text>
                <Button
                  type="primary"
                  onClick={openCreateModal}
                  loading={sshAction === "create"}
                >
                  Create SSH Key
                </Button>
              </Space>
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <span>Pending Pairing Requests</span>
                <Badge count={pairingList.length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void storeFetchPairings()}
              />
            }
            size="small"
          >
            {pairingList.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No pending pairing requests"
              />
            ) : (
              <List
                dataSource={pairingList}
                renderItem={(p) => (
                  <List.Item
                    actions={[
                      <Button
                        key="review"
                        type="primary"
                        size="small"
                        icon={<EyeOutlined />}
                        onClick={() => setReviewPairing(p)}
                      >
                        Review
                      </Button>,
                    ]}
                  >
                    <List.Item.Meta
                      avatar={
                        <Tooltip title={p.caller_info.fingerprint}>
                          <Tag color="blue" style={{ fontFamily: "monospace" }}>
                            {formatFingerprint(p.caller_info.fingerprint)}
                          </Tag>
                        </Tooltip>
                      }
                      title={
                        <Space>
                          <Text>
                            {p.caller_info.display_name || "Unknown Caller"}
                          </Text>
                          {p.caller_info.source_ip && (
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              from {p.caller_info.source_ip}
                            </Text>
                          )}
                        </Space>
                      }
                      description={
                        <Space size={4}>
                          <Tag>
                            {p.command_summary.command_preview || p.command.command}
                          </Tag>
                          {p.caller_info.platform && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {p.caller_info.platform}
                            </Text>
                          )}
                        </Space>
                      }
                    />
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>Grants</span>
                <Badge count={grants.filter((g) => g.status === "active").length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshGrants()}
              />
            }
            size="small"
          >
            {grants.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No active grants"
              />
            ) : (
              <List
                dataSource={grants}
                size="small"
                pagination={{ pageSize: 10, size: "small", hideOnSinglePage: true }}
                renderItem={(g) => (
                  <List.Item
                    actions={g.status === "removed" ? [] : [
                      <Popconfirm
                        key="revoke"
                        title="Revoke this grant?"
                        description="The caller will need to pair again."
                        onConfirm={() => void handleRevokeGrant(g.grant_id)}
                        okText="Revoke"
                        cancelText="Cancel"
                      >
                        <Button
                          danger
                          size="small"
                          icon={<DeleteOutlined />}
                        >
                          Revoke
                        </Button>
                      </Popconfirm>,
                    ]}
                  >
                    <List.Item.Meta
                      title={
                        <Space>
                          <Text>{g.caller_display_name || formatFingerprint(g.caller_fingerprint)}</Text>
                          <Tag color={g.status === "active" ? "green" : "default"}>
                            {g.status}
                          </Tag>
                          <Tag>{g.grant_mode}</Tag>
                        </Space>
                      }
                      description={
                        <Space size={4} wrap>
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            Used {g.use_count}x
                          </Text>
                          {g.last_used_at && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · last active {new Date(g.last_used_at).toLocaleString()}
                            </Text>
                          )}
                          {g.expires_at != null && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · expires {new Date(g.expires_at).toLocaleDateString()}
                            </Text>
                          )}
                          <Tooltip title={g.caller_fingerprint}>
                            <Text type="secondary" style={{ fontSize: 10, fontFamily: "monospace" }}>
                              {formatFingerprint(g.caller_fingerprint)}
                            </Text>
                          </Tooltip>
                        </Space>
                      }
                    />
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <HistoryOutlined />
                <span>Recent Calls</span>
                <Badge count={calls.length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshCalls()}
              />
            }
            size="small"
          >
            {calls.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No recent calls"
              />
            ) : (
              <List
                dataSource={calls}
                size="small"
                pagination={{ pageSize: 10, size: "small", hideOnSinglePage: true }}
                renderItem={(c) => {
                  const argsPreview = formatArgsPreview(c.command_summary?.masked_args_json);
                  const summaryPreview = c.command_summary?.command_preview?.trim();
                  const decryptedPreview = c.command?.command?.trim();
                  const routeOnlyPreview =
                    summaryPreview && c.command_kind && summaryPreview === c.command_kind;
                  const commandPreview =
                    (!routeOnlyPreview && summaryPreview) ||
                    decryptedPreview ||
                    summaryPreview ||
                    c.command_kind ||
                    "-";
                  const bytesLabel = formatBytes(c.bytes_out);
                  return (
                  <List.Item>
                    <List.Item.Meta
                      title={
                        <Space>
                          <Text code style={{ fontSize: 11 }}>
                            {commandPreview}
                          </Text>
                          {argsPreview && (
                            <Tooltip title={c.command_summary?.masked_args_json}>
                              <Text type="secondary" style={{ fontSize: 11, maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", display: "inline-block", verticalAlign: "middle" }}>
                                {argsPreview}
                              </Text>
                            </Tooltip>
                          )}
                          <Tag
                            color={getCallStatusColor(c)}
                          >
                            {c.status}
                            {c.exit_code !== undefined && c.exit_code !== null
                              ? ` (${c.exit_code})`
                              : ""}
                          </Tag>
                          {c.caller_display_name && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              by {c.caller_display_name}
                            </Text>
                          )}
                        </Space>
                      }
                      description={
                        <Space size={4}>
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            {new Date(c.started_at).toLocaleString()}
                          </Text>
                          {c.duration_ms != null && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · {c.duration_ms}ms
                            </Text>
                          )}
                          {bytesLabel && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · ↓ {bytesLabel}
                            </Text>
                          )}
                        </Space>
                      }
                    />
                  </List.Item>
                  );
                }}
              />
            )}
          </Card>
        </Col>
      </Row>
      <PairingRequestModal
        visible={reviewPairing !== null}
        pairing={reviewPairing}
        onClose={() => {
          setReviewPairing(null);
          void storeFetchPairings();
        }}
      />
      <Modal
        open={editorOpen}
        title="Create SSH key"
        okText="Create"
        cancelText="Cancel"
        onCancel={() => setEditorOpen(false)}
        onOk={() => void handleSubmitSshForm()}
        confirmLoading={sshAction === "create"}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message="Only one active SSH key is supported, and SSH grants stay active until rotation or revoke"
          description="Creating a new key should automatically revoke the previous SSH key and its grants on the backend. SSH mode ignores time-based grant TTL, so callers keep access until you rotate or revoke this SSH key."
        />
        <Form
          form={sshForm}
          layout="vertical"
          initialValues={{ label: "", grant_mode: "permanent" satisfies GrantMode }}
        >
          <Form.Item
            label="Label"
            name="label"
            rules={[
              { required: true, message: "Please enter a label" },
              { max: 80, message: "Label must be 80 characters or fewer" },
            ]}
          >
            <Input placeholder="CI Agent" maxLength={80} />
          </Form.Item>
          <Form.Item name="grant_mode" hidden>
            <Input />
          </Form.Item>
        </Form>
      </Modal>
      <Modal
        open={secretModal !== null}
        title={secretModal?.title}
        okText="Close"
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => setSecretModal(null)}
        onCancel={() => setSecretModal(null)}
        width={760}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert showIcon type="warning" message={secretModal?.description} />
          <Descriptions
            size="small"
            column={1}
            items={[
              {
                key: "device_code",
                label: "Device Code",
                children: <Text code>{secretModal?.payload.device_code ?? "-"}</Text>,
              },
              {
                key: "fingerprint",
                label: "Fingerprint",
                children: (
                  <Text code style={{ fontSize: 11 }}>
                    {formatSshFingerprint(secretModal?.payload.ssh_key_fingerprint)}
                  </Text>
                ),
              },
            ]}
          />
          <div>
            <Space style={{ marginBottom: 8 }}>
              <Text strong>Bifrost key file</Text>
              <Button
                size="small"
                icon={<CopyOutlined />}
                onClick={() => {
                  if (!secretModal?.payload.bifrost_key_file) return;
                  copyToClipboard(secretModal.payload.bifrost_key_file);
                  message.success("Key file copied");
                }}
              >
                Copy
              </Button>
            </Space>
            <TextArea
              readOnly
              autoSize={{ minRows: 8, maxRows: 14 }}
              value={secretModal?.payload.bifrost_key_file ?? ""}
            />
          </div>
          <div>
            <Text strong>CLI Example</Text>
            <div style={{ marginTop: 8 }}>
              <Text code>bifrost remote connect --ssh-key /path/to/bifrost.key</Text>
            </div>
          </div>
        </Space>
      </Modal>
    </div>
  );
}
