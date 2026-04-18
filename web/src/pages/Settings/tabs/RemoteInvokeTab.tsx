import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  List,
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
  DisconnectOutlined,
  ReloadOutlined,
  ScanOutlined,
  StopOutlined,
} from "@ant-design/icons";
import {
  enterDiscoveryMode,
  exitDiscoveryMode,
  getClientIdentity,
  getPendingPairings,
  getRemoteInvokeStatus,
  refreshPairCode,
  type ClientIdentity,
  type DiscoverySession,
  type PairingRequest,
  type RemoteInvokeStatus,
} from "../../../api/remoteInvoke";
import { isConnectionIssueError } from "../../../api/client";
import { copyToClipboard } from "../../../utils/clipboard";
import PairingRequestModal from "../../../components/PairingRequestModal";

const { Text, Title } = Typography;

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

function formatCountdown(expiresAt: number): string {
  const remaining = Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
  if (remaining <= 0) return "Expired";
  const m = Math.floor(remaining / 60);
  const s = remaining % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
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
  const [loading, setLoading] = useState(false);
  const [discoveryLoading, setDiscoveryLoading] = useState(false);
  const [pendingPairings, setPendingPairings] = useState<PairingRequest[]>([]);
  const [countdown, setCountdown] = useState("");
  const [selectedPairing, setSelectedPairing] = useState<PairingRequest | null>(
    null,
  );
  const [modalVisible, setModalVisible] = useState(false);
  const pollRef = useRef<number | null>(null);

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

  const refreshPairings = useCallback(async () => {
    try {
      const res = await getPendingPairings();
      setPendingPairings(res.pairings ?? []);
    } catch {
      // ignore
    }
  }, []);

  useEffect(() => {
    void refresh();
    void refreshPairings();
  }, [refresh, refreshPairings]);

  useEffect(() => {
    pollRef.current = window.setInterval(() => {
      void refresh();
      void refreshPairings();
    }, 3000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [refresh, refreshPairings]);

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

  const handlePairingClick = (p: PairingRequest) => {
    setSelectedPairing(p);
    setModalVisible(true);
  };

  const handleModalClose = () => {
    setModalVisible(false);
    setSelectedPairing(null);
    void refresh();
    void refreshPairings();
  };

  const discoverySession: DiscoverySession | null =
    status?.discovery_session ?? null;
  const pairingList = pendingPairings;

  return (
    <div data-testid="settings-remote-invoke-tab">
      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Alert
            showIcon
            type="info"
            message="Remote Command Bridge"
            description="Allows authorized callers to execute read-only queries on this Bifrost instance via a relay server. Enter discovery mode and share the pair code to begin."
          />
        </Col>

        <Col xs={24} md={12}>
          <Card
            title={
              <Space>
                <ApiOutlined />
                <span>Connection Status</span>
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
          >
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
          </Card>
        </Col>

        <Col xs={24} md={12}>
          <Card
            title={
              <Space>
                <ScanOutlined />
                <span>Discovery Mode</span>
              </Space>
            }
            size="small"
          >
            {discoverySession ? (
              <Space direction="vertical" size={12} style={{ width: "100%" }}>
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
                style={{ width: "100%", textAlign: "center" }}
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
                  disabled={
                    status?.state?.toLowerCase() !== "connected"
                  }
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
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <Badge count={pairingList.length} offset={[8, 0]}>
                  <span>Pending Pairing Requests</span>
                </Badge>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshPairings()}
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
                        onClick={() => handlePairingClick(p)}
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
      </Row>

      <PairingRequestModal
        visible={modalVisible}
        pairing={selectedPairing}
        onClose={handleModalClose}
      />
    </div>
  );
}
