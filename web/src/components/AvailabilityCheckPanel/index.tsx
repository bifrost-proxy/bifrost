import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Col,
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
  type CertInfo,
  type DeviceStatus,
  type MobileDevice,
  type MobileDevicesResponse,
  type TrustProbeDevice,
  type TrustProbeProxyAccessStatus,
  type TrustProbeSession,
} from "../../api/cert";
import { normalizeApiErrorMessage } from "../../api/client";
import { pushService, type SettingsScope } from "../../services/pushService";
import { buildBackendUrl } from "../../runtime";
import { useI18n, type TFunction, type TranslationKey } from "../../i18n";

const { Text, Paragraph } = Typography;

interface AvailabilityCheckPanelProps {
  certInfo?: CertInfo | null;
  active?: boolean;
  autoCreate?: boolean;
  compact?: boolean;
  mobileInfo?: MobileDevicesResponse | null;
  testIdPrefix?: string;
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

const DEVICE_STATUS_LABEL_KEYS: Record<DeviceStatus, TranslationKey> = {
  connected: "notify.check.deviceConnected",
  unauthorized: "notify.check.deviceUnauthorized",
  offline: "notify.check.deviceOffline",
  unsupported: "notify.check.deviceUnsupported",
};

const ACCESS_STATUS_LABEL_KEYS: Record<TrustProbeProxyAccessStatus, TranslationKey> = {
  allowed: "notify.check.accessAllowed",
  pending: "notify.check.accessPending",
  denied: "notify.check.accessDenied",
  unavailable: "notify.check.accessUnavailable",
};

function mobileDeviceCertificateLabel(device: MobileDevice, t: TFunction): string {
  switch (device.certificate_status?.state) {
    case "installed":
      return device.certificate_status.trusted
        ? t("notify.check.caTrusted")
        : t("notify.check.caInstalled");
    case "pushed_to_device":
      return t("notify.check.caPushed");
    case "not_installed":
      return t("notify.check.caNotInstalled");
    case "unknown":
    default:
      return t("notify.check.caUnknown");
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
  const { t } = useI18n();
  return (
    <Space wrap size={[4, 4]}>
      <Tag color={device.opened ? "green" : "default"}>
        {device.opened ? t("notify.check.pageOpened") : t("notify.check.pageWaiting")}
      </Tag>
      <Tag color={device.networkReachable ? "green" : "default"}>
        {device.networkReachable ? t("notify.check.networkOk") : t("notify.check.networkPending")}
      </Tag>
      <Tag
        color={device.tlsTrusted ? "green" : device.status === "tls_failed" ? "red" : "default"}
      >
        {device.tlsTrusted
          ? t("notify.check.httpsPassed")
          : device.status === "tls_failed"
            ? t("notify.check.httpsFailed")
            : t("notify.check.httpsPending")}
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
        {t(
          ACCESS_STATUS_LABEL_KEYS[device.proxyAccessStatus ?? "pending"] ??
            "notify.check.accessPending",
        )}
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
        {device.proxyConfigured
          ? t("notify.check.proxyConfigured")
          : device.proxyConfigurationMessage
            ? t("notify.check.proxyMissing")
            : t("notify.check.proxyPending")}
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

function backendResourceUrl(url: string): string {
  try {
    const parsed = new URL(url, window.location.origin);
    return buildBackendUrl(`${parsed.pathname}${parsed.search}${parsed.hash}`);
  } catch {
    return url;
  }
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
  mobileInfo,
  testIdPrefix = "availability-check",
}: AvailabilityCheckPanelProps) {
  const { t } = useI18n();
  const [loadedCertInfo, setLoadedCertInfo] = useState<CertInfo | null>(null);
  const [trustProbeHost, setTrustProbeHost] = useState<string>("");
  const [trustProbeSession, setTrustProbeSession] = useState<TrustProbeSession | null>(null);
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

  const handleCreateTrustProbe = useCallback(
    async (silent = false) => {
      if (!trustProbeHost) {
        if (!silent) {
          message.warning(t("notify.check.selectHostFirst"));
        }
        return;
      }
      setCreatingTrustProbe(true);
      try {
        const session = await createTrustProbeSession(trustProbeHost);
        setTrustProbeSession(session);
        if (!silent) {
          message.success(t("notify.check.created"));
        }
      } catch (error) {
        if (!silent) {
          message.error(normalizeApiErrorMessage(error, t("notify.check.createFailed")));
        }
      } finally {
        setCreatingTrustProbe(false);
      }
    },
    [trustProbeHost, t],
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
      message.success(t("notify.check.linkCopied"));
    } catch {
      message.warning(t("notify.check.copyManually"));
    }
  }, [trustProbeSession?.landingUrl, t]);

  const qrSize = compact ? 112 : 180;
  const detectedMobileDevices = connectedMobileDevices(mobileInfo);

  return (
    <Space direction="vertical" style={{ width: "100%" }} size={compact ? "small" : "middle"}>
      <Alert
        type="info"
        showIcon
        message={compact ? t("notify.check.compactTitle") : t("notify.check.title")}
        description={
          compact ? t("notify.check.compactDesc") : t("notify.check.desc")
        }
      />

      {!compact ? (
        <div data-testid={`${testIdPrefix}-target-devices`}>
          <Space direction="vertical" size={6} style={{ width: "100%" }}>
            <Space wrap size={[8, 4]}>
              <Text strong style={{ fontSize: 13 }}>
                {t("notify.check.devicesToCheck")}
              </Text>
              <Tag color={detectedMobileDevices.length ? "blue" : "default"}>
                {detectedMobileDevices.length}
              </Tag>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {t("notify.check.devicesToCheckHint")}
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
                        <Tag color="green">
                          {DEVICE_STATUS_LABEL_KEYS[device.status]
                            ? t(DEVICE_STATUS_LABEL_KEYS[device.status])
                            : device.status}
                        </Tag>
                        <Tag color={mobileDeviceCertificateColor(device)}>
                          {mobileDeviceCertificateLabel(device, t)}
                        </Tag>
                      </Space>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {t("notify.check.deviceHint")}
                      </Text>
                    </Space>
                  </List.Item>
                )}
              />
            ) : (
              <Alert
                type="info"
                showIcon
                message={t("notify.check.noPhoneTitle")}
                description={t("notify.check.noPhoneDesc")}
              />
            )}
          </Space>
        </div>
      ) : null}

      <Row gutter={[12, 12]} align="bottom">
        <Col xs={24} sm={compact || autoCreate ? 24 : 12} md={compact ? 24 : autoCreate ? 12 : 10}>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {t("notify.check.localAddress")}
          </Text>
          <Select
            style={{ width: "100%", marginTop: 4 }}
            value={trustProbeHost || undefined}
            placeholder={t("notify.check.selectLanIp")}
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
              {trustProbeSession ? t("notify.check.regenerate") : t("notify.check.generate")}
            </Button>
          </Col>
        ) : null}
      </Row>

      {effectiveCertInfo?.sha256_fingerprint && !compact ? (
        <Paragraph type="secondary" style={{ fontSize: 12, marginBottom: 0 }}>
          {t("notify.check.fingerprint")}<Text code>{effectiveCertInfo.sha256_fingerprint}</Text>
        </Paragraph>
      ) : null}

      {trustProbeSession ? (
        <Row gutter={[16, 16]} align="top" data-testid={`${testIdPrefix}-session`}>
          <Col xs={24} sm={compact ? 24 : 10} md={compact ? 24 : 8}>
            <div style={{ textAlign: compact ? "left" : "center" }}>
              <img
                src={backendResourceUrl(trustProbeSession.qrCodeUrl)}
                alt={t("notify.check.qrAlt")}
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
                  {t("notify.check.openPage")}
                </a>
                <Button
                  size="small"
                  icon={<CopyOutlined />}
                  onClick={() => void copyLandingUrl()}
                  data-testid={`${testIdPrefix}-copy`}
                >
                  {t("notify.check.copyLink")}
                </Button>
              </Space>
            </div>
          </Col>
          <Col xs={24} sm={compact ? 24 : 14} md={compact ? 24 : 16}>
            <Space direction="vertical" style={{ width: "100%" }} size={compact ? "small" : "middle"}>
              {trustProbeSession.devices?.length ? (
                <List
                  size="small"
                  header={
                    <Space size={6}>
                      <Text strong style={{ fontSize: 12 }}>
                        {t("notify.check.connectedDevices")}
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
                            <Tag color={device.platformHint.startsWith("ios") ? "purple" : "blue"}>
                              {device.platformHint}
                            </Tag>
                          ) : null}
                          {device.clientIp ? (
                            <Text code style={{ fontSize: 12 }}>
                              {device.clientIp}
                            </Text>
                          ) : null}
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {t("notify.check.lastSeenLower", {
                              time: new Date(device.lastSeen).toLocaleTimeString(),
                            })}
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
                  {t("notify.check.noDeviceYet")}
                </Text>
              )}

              {!trustProbeSession.proxyConfigured && trustProbeSession.proxyConfigurationMessage ? (
                <Alert
                  type="warning"
                  showIcon
                  message={t("notify.check.proxyNotConfiguredTitle")}
                  description={
                    <Space direction="vertical" size={4}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {trustProbeSession.proxyConfigurationMessage}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {t("notify.check.proxyManualPrefix")}{" "}
                        <Text code>
                          {trustProbeSession.host}:{trustProbeSession.adminPort}
                        </Text>
                        {t("notify.check.proxyManualSuffix")}
                      </Text>
                    </Space>
                  }
                />
              ) : null}

              {trustProbeSession.status === "tls_trusted" ? (
                <Alert
                  type="success"
                  showIcon
                  message={t("notify.check.httpsPassedTitle")}
                  description={
                    <Space direction="vertical" size={4}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {t("notify.check.httpsPassedDesc")}
                      </Text>
                      <Text>
                        {t("notify.check.proxyLabel")}{" "}
                        <Text code>
                          {trustProbeSession.host}:{trustProbeSession.adminPort}
                        </Text>
                      </Text>
                      <a href={trustProbeSession.proxyQrCodeUrl} target="_blank" rel="noreferrer">
                        {t("notify.check.openProxyQr")}
                      </a>
                    </Space>
                  }
                />
              ) : trustProbeSession.status === "tls_failed" ? (
                <Alert
                  type="warning"
                  showIcon
                  message={t("notify.check.httpsFailedTitle")}
                  description={t("notify.check.httpsFailedDesc")}
                />
              ) : trustProbeSession.status === "network_failed" ? (
                <Alert
                  type="error"
                  showIcon
                  message={t("notify.check.networkFailedTitle")}
                  description={t("notify.check.networkFailedDesc")}
                />
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {t("notify.check.waitingHint")}
                </Text>
              )}

              {trustProbeSession.lastError ? (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {t("notify.check.lastError", { error: trustProbeSession.lastError })}
                </Text>
              ) : null}

            </Space>
          </Col>
        </Row>
      ) : null}
    </Space>
  );
}
