import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Col,
  Input,
  List,
  Row,
  Select,
  Space,
  Tag,
  Typography,
  message,
} from "antd";
import { CopyOutlined, QrcodeOutlined } from "@ant-design/icons";
import {
  createTrustProbeSession,
  getCertInfo,
  updateTrustProbeSessionWifiSsid,
  type CertInfo,
  type MobileDevice,
  type MobileDevicesResponse,
  type TrustProbeDevice,
  type TrustProbeSession,
  type TrustProbeStatus,
} from "../../api/cert";
import { normalizeApiErrorMessage } from "../../api/client";
import { pushService, type SettingsScope } from "../../services/pushService";
import {
  getSharedIosWifiProxySsid,
  setSharedIosWifiProxySsid,
  subscribeSharedIosWifiProxySsid,
} from "../../utils/iosWifiProxySsid";

const { Text, Paragraph } = Typography;

const MANAGED_WIFI_PROFILE_RISK_TEXT =
  "Experimental profile note: Bifrost's iOS Wi-Fi proxy profile uses Apple's managed Wi-Fi payload. It does not contain a Wi-Fi password, but uninstalling the profile can remove that managed Wi-Fi network entry from iOS. Manual Wi-Fi proxy setup is the safe cleanup path.";

interface AvailabilityCheckPanelProps {
  certInfo?: CertInfo | null;
  active?: boolean;
  autoCreate?: boolean;
  compact?: boolean;
  showEvents?: boolean;
  mobileInfo?: MobileDevicesResponse | null;
  testIdPrefix?: string;
}

function trustProbeStatusLabel(status: TrustProbeStatus) {
  switch (status) {
    case "created":
      return "Waiting for scan";
    case "page_opened":
      return "Device opened page";
    case "network_reachable":
      return "Network reachable";
    case "tls_trusted":
      return "CA trusted";
    case "tls_failed":
      return "Trust failed";
    case "network_failed":
      return "Probe unreachable";
    case "expired":
      return "Expired";
    default:
      return "Unknown";
  }
}

function trustProbeStatusColor(status: TrustProbeStatus) {
  switch (status) {
    case "tls_trusted":
      return "green";
    case "network_reachable":
    case "page_opened":
      return "blue";
    case "tls_failed":
    case "network_failed":
    case "expired":
      return "red";
    case "created":
    default:
      return "default";
  }
}

function shortDeviceId(deviceId: string) {
  if (deviceId.length <= 18) {
    return deviceId;
  }
  return `${deviceId.slice(0, 8)}...${deviceId.slice(-6)}`;
}

function connectedMobileDevices(info?: MobileDevicesResponse | null): MobileDevice[] {
  if (!info) {
    return [];
  }
  return [...info.ios.devices, ...info.android.devices].filter(
    (device) => device.status === "connected",
  );
}

function mobileDeviceDisplayName(device: MobileDevice) {
  return device.name?.trim() || device.id;
}

function mobileDeviceCertificateLabel(device: MobileDevice): string {
  switch (device.certificate_status?.state) {
    case "installed":
      return device.certificate_status.trusted ? "CA trusted" : "CA installed";
    case "pushed_to_device":
      return "CA pushed";
    case "not_installed":
      return "CA not installed";
    case "unknown":
    default:
      return "CA unknown";
  }
}

function mobileDeviceCertificateColor(device: MobileDevice): string {
  switch (device.certificate_status?.state) {
    case "installed":
      return device.certificate_status.trusted ? "green" : "orange";
    case "pushed_to_device":
      return "blue";
    case "not_installed":
      return "red";
    case "unknown":
    default:
      return "default";
  }
}

function DeviceStatusTags({ device }: { device: TrustProbeDevice }) {
  return (
    <Space wrap size={[4, 4]}>
      <Tag color={device.opened ? "green" : "default"}>
        Page {device.opened ? "opened" : "waiting"}
      </Tag>
      <Tag color={device.networkReachable ? "green" : "default"}>
        Network {device.networkReachable ? "ok" : "pending"}
      </Tag>
      <Tag
        color={device.tlsTrusted ? "green" : device.status === "tls_failed" ? "red" : "default"}
      >
        Trust{" "}
        {device.tlsTrusted ? "passed" : device.status === "tls_failed" ? "failed" : "pending"}
      </Tag>
      <Tag
        color={
          device.proxyAccessAllowed
            ? "green"
            : device.proxyAccessStatus === "pending"
              ? "orange"
              : "default"
        }
      >
        Access {device.proxyAccessStatus ?? "pending"}
      </Tag>
      <Tag
        color={
          device.proxyConfigured
            ? "green"
            : device.proxyConfigurationMessage
              ? "red"
              : "default"
        }
      >
        Proxy{" "}
        {device.proxyConfigured
          ? "configured"
          : device.proxyConfigurationMessage
            ? "missing"
            : "pending"}
      </Tag>
    </Space>
  );
}

function preserveTrustProbeUrls(
  next: TrustProbeSession,
  previous: TrustProbeSession,
): TrustProbeSession {
  return {
    ...next,
    landingUrl: previous.landingUrl,
    qrCodeUrl: previous.qrCodeUrl,
    caDownloadUrl: previous.caDownloadUrl,
    proxyQrCodeUrl: previous.proxyQrCodeUrl,
  };
}

function withSettingsScope(scope: SettingsScope): SettingsScope[] {
  return Array.from(
    new Set([...(pushService.getSubscription().settings_scopes ?? []), scope]),
  );
}

function trustProbeSessionsFromPushData(data: unknown): TrustProbeSession[] {
  return Array.isArray(data) ? (data as TrustProbeSession[]) : [];
}

export default function AvailabilityCheckPanel({
  certInfo,
  active = true,
  autoCreate = false,
  compact = false,
  showEvents = true,
  mobileInfo,
  testIdPrefix = "availability-check",
}: AvailabilityCheckPanelProps) {
  const [loadedCertInfo, setLoadedCertInfo] = useState<CertInfo | null>(null);
  const [trustProbeHost, setTrustProbeHost] = useState<string>("");
  const [trustProbeSession, setTrustProbeSession] = useState<TrustProbeSession | null>(null);
  const [wifiSsidDraft, setWifiSsidDraft] = useState(() => getSharedIosWifiProxySsid());
  const [updatingWifiSsid, setUpdatingWifiSsid] = useState(false);
  const [creatingTrustProbe, setCreatingTrustProbe] = useState(false);
  const autoCreatedHostRef = useRef<string | null>(null);
  const effectiveCertInfo = certInfo ?? loadedCertInfo;

  useEffect(() => {
    if (!active || certInfo || loadedCertInfo) {
      return;
    }

    let cancelled = false;
    void getCertInfo()
      .then((info) => {
        if (!cancelled) {
          setLoadedCertInfo(info);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setLoadedCertInfo(null);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [active, certInfo, loadedCertInfo]);

  useEffect(() => {
    if (trustProbeHost || !effectiveCertInfo?.local_ips?.length) {
      return;
    }
    setTrustProbeHost(effectiveCertInfo.local_ips[0]);
  }, [effectiveCertInfo?.local_ips, trustProbeHost]);

  useEffect(() => {
    const sessionId = trustProbeSession?.sessionId;
    if (!sessionId || trustProbeSession.status === "expired") {
      return;
    }
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: withSettingsScope("trust_probe"),
    });
    const unsubscribe = pushService.onSettingsUpdate((update) => {
      if (update.scope !== "trust_probe") {
        return;
      }
      const next = trustProbeSessionsFromPushData(update.data).find(
        (session) => session.sessionId === sessionId,
      );
      if (!next) {
        return;
      }
      setTrustProbeSession((current) => {
        if (!current || current.sessionId !== next.sessionId) {
          return next;
        }
        return preserveTrustProbeUrls(next, current);
      });
    });
    return () => {
      unsubscribe();
      pushService.disconnectIfIdle();
    };
  }, [trustProbeSession?.sessionId, trustProbeSession?.status]);

  useEffect(() => {
    const sessionSsid = trustProbeSession?.suggestedWifiSsid?.trim();
    setWifiSsidDraft(sessionSsid || getSharedIosWifiProxySsid());
  }, [trustProbeSession?.sessionId, trustProbeSession?.suggestedWifiSsid]);

  const applyWifiSsidToCurrentSession = useCallback(
    async (ssid: string, silent = false) => {
      const normalized = ssid.trim();
      if (!trustProbeSession || !normalized) {
        return;
      }
      if (trustProbeSession.suggestedWifiSsid?.trim() === normalized) {
        return;
      }
      try {
        const next = await updateTrustProbeSessionWifiSsid(
          trustProbeSession.sessionId,
          normalized,
        );
        setTrustProbeSession((current) => {
          if (!current || current.sessionId !== next.sessionId) {
            return next;
          }
          return preserveTrustProbeUrls(next, current);
        });
      } catch (error) {
        if (!silent) {
          message.error(normalizeApiErrorMessage(error, "Failed to update Wi-Fi name"));
        }
      }
    },
    [trustProbeSession],
  );

  useEffect(() => {
    return subscribeSharedIosWifiProxySsid((ssid) => {
      setWifiSsidDraft(ssid);
      void applyWifiSsidToCurrentSession(ssid, true);
    });
  }, [applyWifiSsidToCurrentSession]);

  useEffect(() => {
    const sharedSsid = getSharedIosWifiProxySsid();
    if (sharedSsid) {
      void applyWifiSsidToCurrentSession(sharedSsid, true);
    }
  }, [applyWifiSsidToCurrentSession, trustProbeSession?.sessionId]);

  const handleCreateTrustProbe = useCallback(
    async (silent = false) => {
      if (!trustProbeHost) {
        if (!silent) {
          message.warning("Select a local network address first.");
        }
        return;
      }
      setCreatingTrustProbe(true);
      try {
        const session = await createTrustProbeSession(trustProbeHost);
        const sharedSsid = getSharedIosWifiProxySsid() || wifiSsidDraft.trim();
        if (sharedSsid && session.suggestedWifiSsid?.trim() !== sharedSsid) {
          const next = await updateTrustProbeSessionWifiSsid(session.sessionId, sharedSsid);
          setTrustProbeSession(preserveTrustProbeUrls(next, session));
        } else {
          setTrustProbeSession(session);
        }
        if (!silent) {
          message.success("Availability check link and QR code are ready.");
        }
      } catch (error) {
        if (!silent) {
          message.error(normalizeApiErrorMessage(error, "Failed to create availability check"));
        }
      } finally {
        setCreatingTrustProbe(false);
      }
    },
    [trustProbeHost, wifiSsidDraft],
  );

  useEffect(() => {
    if (
      !active ||
      !autoCreate ||
      !trustProbeHost ||
      creatingTrustProbe ||
      trustProbeSession?.host === trustProbeHost ||
      autoCreatedHostRef.current === trustProbeHost
    ) {
      return;
    }
    autoCreatedHostRef.current = trustProbeHost;
    void handleCreateTrustProbe(true);
  }, [
    active,
    autoCreate,
    creatingTrustProbe,
    handleCreateTrustProbe,
    trustProbeHost,
    trustProbeSession?.host,
  ]);

  const copyLandingUrl = useCallback(async () => {
    if (!trustProbeSession?.landingUrl) {
      return;
    }
    try {
      await navigator.clipboard.writeText(trustProbeSession.landingUrl);
      message.success("Availability check link copied.");
    } catch {
      message.warning("Select and copy the link manually.");
    }
  }, [trustProbeSession?.landingUrl]);

  const handleUpdateWifiSsid = useCallback(async () => {
    if (!trustProbeSession) {
      return;
    }
    const ssid = wifiSsidDraft.trim();
    if (!ssid) {
      message.warning("Enter the Wi-Fi name shown on the target device first.");
      return;
    }
    setUpdatingWifiSsid(true);
    try {
      const next = await updateTrustProbeSessionWifiSsid(trustProbeSession.sessionId, ssid);
      setTrustProbeSession((current) => {
        if (!current || current.sessionId !== next.sessionId) {
          return next;
        }
        return preserveTrustProbeUrls(next, current);
      });
      setSharedIosWifiProxySsid(ssid);
      message.success("Wi-Fi name sent to the availability check page.");
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Failed to update Wi-Fi name"));
    } finally {
      setUpdatingWifiSsid(false);
    }
  }, [trustProbeSession, wifiSsidDraft]);

  const qrSize = compact ? 112 : 180;
  const detectedMobileDevices = connectedMobileDevices(mobileInfo);

  return (
    <Space direction="vertical" style={{ width: "100%" }} size={compact ? "small" : "middle"}>
      <Alert
        type="info"
        showIcon
        message={compact ? "Use Availability Check when this device cannot use the proxy" : "Check whether a phone can use Bifrost"}
        description={
          compact
            ? "Scan the QR code or open the link from the device. Bifrost will check proxy authorization, probe reachability, and HTTPS CA trust."
            : "Share the link or scan the QR code from the target device. The page checks proxy authorization, probe port reachability, and a real HTTPS request signed by the current Bifrost CA."
        }
      />

      {!compact ? (
        <div data-testid={`${testIdPrefix}-target-devices`}>
          <Space direction="vertical" size={6} style={{ width: "100%" }}>
            <Space wrap size={[8, 4]}>
              <Text strong style={{ fontSize: 13 }}>
                Devices to check
              </Text>
              <Tag color={detectedMobileDevices.length ? "blue" : "default"}>
                {detectedMobileDevices.length}
              </Tag>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Connected phones are shown here before you scan the availability link.
              </Text>
            </Space>
            {detectedMobileDevices.length ? (
              <List
                size="small"
                dataSource={detectedMobileDevices}
                data-testid={`${testIdPrefix}-target-device-list`}
                renderItem={(device) => (
                  <List.Item>
                    <Space direction="vertical" size={4} style={{ width: "100%" }}>
                      <Space wrap size={[8, 4]}>
                        <Text strong>{mobileDeviceDisplayName(device)}</Text>
                        <Tag color={device.platform === "ios" ? "purple" : "blue"}>
                          {device.platform === "ios" ? "iOS" : "Android"}
                        </Tag>
                        <Tag color="green">{device.status}</Tag>
                        <Tag color={mobileDeviceCertificateColor(device)}>
                          {mobileDeviceCertificateLabel(device)}
                        </Tag>
                      </Space>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        Open the link or scan the QR code below from this device. Bifrost will
                        track the browser with a local device ID after the check page opens.
                      </Text>
                    </Space>
                  </List.Item>
                )}
              />
            ) : (
              <Alert
                type="info"
                showIcon
                message="No connected phone detected yet"
                description="Connect an iPhone or Android device over USB, or share the availability link with any phone on the same LAN. Devices that open the check page will appear in the live status list."
              />
            )}
          </Space>
        </div>
      ) : null}

      <Row gutter={[12, 12]} align="bottom">
        <Col xs={24} sm={compact || autoCreate ? 24 : 12} md={compact ? 24 : autoCreate ? 12 : 10}>
          <Text type="secondary" style={{ fontSize: 12 }}>
            Local network address
          </Text>
          <Select
            style={{ width: "100%", marginTop: 4 }}
            value={trustProbeHost || undefined}
            placeholder="Select a LAN IP"
            onChange={(value) => {
              setTrustProbeHost(value);
              setTrustProbeSession(null);
              autoCreatedHostRef.current = null;
            }}
            options={(effectiveCertInfo?.local_ips ?? []).map((ip) => ({
              value: ip,
              label: ip,
            }))}
            disabled={!effectiveCertInfo?.available || !effectiveCertInfo?.local_ips?.length}
            data-testid={`${testIdPrefix}-host`}
          />
        </Col>
        {!autoCreate ? (
          <Col xs={24} sm={compact ? 24 : 12} md={compact ? 24 : 14}>
            <Button
              type={compact ? "default" : "primary"}
              icon={<QrcodeOutlined />}
              loading={creatingTrustProbe}
              disabled={!effectiveCertInfo?.available || !trustProbeHost}
              onClick={() => void handleCreateTrustProbe()}
              data-testid={`${testIdPrefix}-create`}
            >
              {trustProbeSession ? "Regenerate Availability Check" : "Generate Availability Check"}
            </Button>
          </Col>
        ) : null}
      </Row>

      {effectiveCertInfo?.sha256_fingerprint && !compact ? (
        <Paragraph type="secondary" style={{ fontSize: 12, marginBottom: 0 }}>
          CA SHA-256 fingerprint: <Text code>{effectiveCertInfo.sha256_fingerprint}</Text>
        </Paragraph>
      ) : null}

      {trustProbeSession ? (
        <Row gutter={[16, 16]} align="top" data-testid={`${testIdPrefix}-session`}>
          <Col xs={24} sm={compact ? 24 : 10} md={compact ? 24 : 8}>
            <div style={{ textAlign: compact ? "left" : "center" }}>
              <img
                src={trustProbeSession.qrCodeUrl}
                alt="Availability Check QR Code"
                width={qrSize}
                height={qrSize}
                style={{
                  width: qrSize,
                  height: qrSize,
                  padding: 8,
                  background: "var(--color-bg-container)",
                  border: "1px solid var(--color-border)",
                  borderRadius: 4,
                  display: "block",
                  objectFit: "contain",
                  margin: compact ? 0 : "0 auto",
                }}
                data-testid={`${testIdPrefix}-qrcode`}
              />
              <Space direction="vertical" size={2} style={{ marginTop: 6, width: "100%" }}>
                <a
                  href={trustProbeSession.landingUrl}
                  target="_blank"
                  rel="noreferrer"
                  data-testid={`${testIdPrefix}-link`}
                >
                  Open availability check page
                </a>
                <Button
                  size="small"
                  icon={<CopyOutlined />}
                  onClick={() => void copyLandingUrl()}
                  data-testid={`${testIdPrefix}-copy`}
                >
                  Copy link
                </Button>
              </Space>
            </div>
          </Col>
          <Col xs={24} sm={compact ? 24 : 14} md={compact ? 24 : 16}>
            <Space direction="vertical" style={{ width: "100%" }} size={compact ? "small" : "middle"}>
              <Space wrap>
                <Tag color={trustProbeStatusColor(trustProbeSession.status)}>
                  {trustProbeStatusLabel(trustProbeSession.status)}
                </Tag>
                <Tag color={trustProbeSession.opened ? "green" : "default"}>
                  Page {trustProbeSession.opened ? "opened" : "waiting"}
                </Tag>
                <Tag color={trustProbeSession.networkReachable ? "green" : "default"}>
                  Probe port {trustProbeSession.networkReachable ? "reachable" : "pending"}
                </Tag>
                <Tag color={trustProbeSession.tlsTrusted ? "green" : "default"}>
                  HTTPS trust {trustProbeSession.tlsTrusted ? "passed" : "pending"}
                </Tag>
                <Tag color={trustProbeSession.proxyAccessAllowed ? "green" : "default"}>
                  Proxy access{" "}
                  {trustProbeSession.proxyAccessStatus === "pending"
                    ? "needs approval"
                    : trustProbeSession.proxyAccessStatus ?? "pending"}
                </Tag>
                <Tag
                  color={
                    trustProbeSession.proxyConfigured
                      ? "green"
                      : trustProbeSession.proxyConfigurationMessage
                        ? "red"
                        : "default"
                  }
                >
                  Proxy config{" "}
                  {trustProbeSession.proxyConfigured
                    ? "detected"
                    : trustProbeSession.proxyConfigurationMessage
                      ? "missing"
                      : "pending"}
                </Tag>
              </Space>

              {trustProbeSession.devices?.length ? (
                <List
                  size="small"
                  header={
                    <Space size={6}>
                      <Text strong style={{ fontSize: 12 }}>
                        Connected devices
                      </Text>
                      <Tag>{trustProbeSession.devices.length}</Tag>
                    </Space>
                  }
                  dataSource={trustProbeSession.devices}
                  data-testid={`${testIdPrefix}-devices`}
                  renderItem={(device) => (
                    <List.Item>
                      <Space direction="vertical" size={4} style={{ width: "100%" }}>
                        <Space wrap size={[8, 4]}>
                          <Text strong style={{ fontSize: 12 }}>
                            {shortDeviceId(device.deviceId)}
                          </Text>
                          {device.platformHint ? (
                            <Tag color={device.platformHint === "ios" ? "purple" : "blue"}>
                              {device.platformHint}
                            </Tag>
                          ) : null}
                          {device.clientIp ? (
                            <Text code style={{ fontSize: 12 }}>
                              {device.clientIp}
                            </Text>
                          ) : null}
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            last seen {new Date(device.lastSeen).toLocaleTimeString()}
                          </Text>
                        </Space>
                        <DeviceStatusTags device={device} />
                        {device.lastError ? (
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {device.lastError}
                          </Text>
                        ) : null}
                      </Space>
                    </List.Item>
                  )}
                />
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  No device has opened this availability check link yet. Multiple devices can use
                  the same fixed link; Bifrost will track each browser by a local device ID.
                </Text>
              )}

              {!trustProbeSession.proxyConfigured && trustProbeSession.proxyConfigurationMessage ? (
                <Alert
                  type="warning"
                  showIcon
                  message="Proxy is not configured on this device yet"
                  description={
                    <Space direction="vertical" size={4}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {trustProbeSession.proxyConfigurationMessage}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        On iPhone, use the Availability Check page to download the iOS Wi-Fi Proxy
                        Profile, or manually set the current Wi-Fi HTTP Proxy to{" "}
                        <Text code>
                          {trustProbeSession.host}:{trustProbeSession.adminPort}
                        </Text>
                        .
                      </Text>
                    </Space>
                  }
                />
              ) : null}

              {trustProbeSession.status === "tls_trusted" ? (
                <Alert
                  type="success"
                  showIcon
                  message="Current device browser trusts Bifrost CA"
                  description={
                    <Space direction="vertical" size={4}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        This confirms a real HTTPS handshake with a certificate signed by Bifrost
                        CA. Some apps can still bypass system trust with certificate pinning or
                        custom TLS policy.
                      </Text>
                      <Text>
                        Proxy:{" "}
                        <Text code>
                          {trustProbeSession.host}:{trustProbeSession.adminPort}
                        </Text>
                      </Text>
                      <a href={trustProbeSession.proxyQrCodeUrl} target="_blank" rel="noreferrer">
                        Open proxy QR code
                      </a>
                    </Space>
                  }
                />
              ) : trustProbeSession.status === "tls_failed" ? (
                <Alert
                  type="warning"
                  showIcon
                  message="HTTPS trust check failed"
                  description="The device reached the probe port, but the browser could not complete the HTTPS handshake. Install the CA, enable full trust on iOS, check device time, then fully restart the browser and retry. Some browsers keep old certificate trust decisions until restart."
                />
              ) : trustProbeSession.status === "network_failed" ? (
                <Alert
                  type="error"
                  showIcon
                  message="Probe port is not reachable"
                  description="The phone opened the landing page but could not reach the probe port. Check firewall rules, local network isolation, and the selected IP address."
                />
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Scan the QR code from the target device. Bifrost updates this status over the
                  live connection when the device opens the page or completes a check step.
                </Text>
              )}

              {trustProbeSession.lastError ? (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Last error: {trustProbeSession.lastError}
                </Text>
              ) : null}

              <div>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Wi-Fi name for iOS proxy profile
                </Text>
                <Row gutter={[8, 8]} align="middle" style={{ marginTop: 4 }}>
                  <Col xs={24} md={compact ? 24 : 14}>
                    <Input
                      value={wifiSsidDraft}
                      placeholder="Exact Wi-Fi name shown on the phone"
                      onChange={(event) => setWifiSsidDraft(event.target.value)}
                      onPressEnter={() => void handleUpdateWifiSsid()}
                      data-testid={`${testIdPrefix}-wifi-ssid`}
                    />
                  </Col>
                  <Col xs={24} md={compact ? 24 : 10}>
                    <Button
                      loading={updatingWifiSsid}
                      onClick={() => void handleUpdateWifiSsid()}
                      data-testid={`${testIdPrefix}-wifi-ssid-update`}
                    >
                      Send Wi-Fi Name
                    </Button>
                  </Col>
                </Row>
                {trustProbeSession.suggestedWifiSsidMessage ? (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {trustProbeSession.suggestedWifiSsidMessage}
                  </Text>
                ) : null}
                <Text type="secondary" style={{ display: "block", fontSize: 12, marginTop: 4 }}>
                  Saved Wi-Fi names are shared with other Availability Check panels. Phone pages
                  keep the same session open and update their profile link automatically.
                </Text>
                <Alert
                  type="warning"
                  showIcon
                  message="Experimental managed Wi-Fi profile"
                  description={MANAGED_WIFI_PROFILE_RISK_TEXT}
                  style={{ marginTop: 8 }}
                />
              </div>

              {showEvents ? (
                <List
                  size="small"
                  dataSource={trustProbeSession.events.slice(-5).reverse()}
                  locale={{ emptyText: "No probe events yet" }}
                  renderItem={(event) => (
                    <List.Item>
                      <Space direction="vertical" size={0}>
                        <Text style={{ fontSize: 12 }}>{event.type}</Text>
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {new Date(event.at).toLocaleTimeString()}
                          {event.message ? ` - ${event.message}` : ""}
                        </Text>
                      </Space>
                    </List.Item>
                  )}
                />
              ) : null}
            </Space>
          </Col>
        </Row>
      ) : null}
    </Space>
  );
}
