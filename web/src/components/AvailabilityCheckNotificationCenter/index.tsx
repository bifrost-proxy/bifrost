import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Badge, Button, Card, Space, Tag, Typography, theme } from "antd";
import { BellOutlined, CloseOutlined, MobileOutlined, RightOutlined } from "@ant-design/icons";
import type { TrustProbeDevice, TrustProbeSession } from "../../api/cert";
import { pushService, type SettingsScope } from "../../services/pushService";

const { Text } = Typography;

function deviceLabel(device: TrustProbeDevice) {
  return device.platformHint || device.clientIp || device.deviceId;
}

function deviceStatus(device: TrustProbeDevice) {
  if (device.tlsTrusted) {
    return { label: "Browser HTTPS passed", color: "green" };
  }
  if (device.status === "tls_failed") {
    return { label: "Browser HTTPS failed", color: "orange" };
  }
  if (device.networkReachable) {
    return { label: "Checking HTTPS", color: "blue" };
  }
  if (device.opened) {
    return { label: "Opened page", color: "blue" };
  }
  return { label: "Waiting", color: "default" };
}

function notificationSignature(sessions: TrustProbeSession[]) {
  return sessions
    .flatMap((session) =>
      session.devices.map((device) => `${session.sessionId}:${device.deviceId}:${device.firstSeen}`),
    )
    .sort()
    .join("|");
}

function withSettingsScope(scope: SettingsScope): SettingsScope[] {
  return Array.from(
    new Set([...(pushService.getSubscription().settings_scopes ?? []), scope]),
  );
}

function trustProbeSessionsFromPushData(data: unknown): TrustProbeSession[] {
  return Array.isArray(data) ? (data as TrustProbeSession[]) : [];
}

export default function AvailabilityCheckNotificationCenter() {
  const { token } = theme.useToken();
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<TrustProbeSession[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [dismissedSignature, setDismissedSignature] = useState<string | null>(null);

  useEffect(() => {
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: withSettingsScope("trust_probe"),
    });
    const unsubscribe = pushService.onSettingsUpdate((update) => {
      if (update.scope !== "trust_probe") {
        return;
      }
      setSessions(
        trustProbeSessionsFromPushData(update.data).filter((session) => session.devices.length > 0),
      );
    });
    return () => {
      unsubscribe();
      pushService.disconnectIfIdle();
    };
  }, []);

  const devices = useMemo(
    () =>
      sessions
        .flatMap((session) =>
          session.devices.map((device) => ({
            session,
            device,
          })),
        )
        .sort((a, b) => Date.parse(b.device.lastSeen) - Date.parse(a.device.lastSeen)),
    [sessions],
  );
  const signature = notificationSignature(sessions);
  const visible = devices.length > 0 && signature !== dismissedSignature;

  const openCertificatePage = useCallback(() => {
    setExpanded(false);
    navigate({
      pathname: "/settings",
      search: "?tab=certificate",
      hash: "#certificate-trust-probe",
    });
  }, [navigate]);

  const dismiss = useCallback(() => {
    setExpanded(false);
    setDismissedSignature(signature);
  }, [signature]);

  if (!visible) {
    return null;
  }

  return (
    <div
      data-testid="availability-check-notification-center"
      style={{
        position: "fixed",
        top: 14,
        right: 18,
        zIndex: 980,
        maxWidth: 360,
      }}
    >
      {expanded ? (
        <Card
          size="small"
          title={
            <Space size={8}>
              <MobileOutlined />
              <span>Availability Check</span>
              <Badge count={devices.length} size="small" />
            </Space>
          }
          extra={
            <Button
              aria-label="Close availability check notifications"
              icon={<CloseOutlined />}
              size="small"
              type="text"
              onClick={dismiss}
              data-testid="availability-check-notification-close"
            />
          }
          style={{
            width: 360,
            boxShadow: token.boxShadowSecondary,
            borderColor: token.colorBorderSecondary,
          }}
        >
          <Space direction="vertical" size="small" style={{ width: "100%" }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              A device opened the availability check page. Review proxy access, browser HTTPS
              probe, and proxy configuration from the certificate page.
            </Text>
            {devices.slice(0, 4).map(({ session, device }) => {
              const status = deviceStatus(device);
              return (
                <button
                  key={`${session.sessionId}:${device.deviceId}`}
                  type="button"
                  onClick={openCertificatePage}
                  data-testid="availability-check-notification-item"
                  style={{
                    width: "100%",
                    border: `1px solid ${token.colorBorderSecondary}`,
                    background: token.colorBgContainer,
                    color: token.colorText,
                    borderRadius: 6,
                    padding: "8px 10px",
                    cursor: "pointer",
                    textAlign: "left",
                  }}
                >
                  <Space direction="vertical" size={4} style={{ width: "100%" }}>
                    <Space wrap size={[6, 4]}>
                      <Text strong>{deviceLabel(device)}</Text>
                      {device.clientIp ? <Text code>{device.clientIp}</Text> : null}
                      <Tag color={status.color}>{status.label}</Tag>
                    </Space>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Last seen {new Date(device.lastSeen).toLocaleTimeString()}
                    </Text>
                  </Space>
                </button>
              );
            })}
            <Button
              type="primary"
              block
              icon={<RightOutlined />}
              onClick={openCertificatePage}
              data-testid="availability-check-notification-open"
            >
              Open Availability Check
            </Button>
          </Space>
        </Card>
      ) : (
        <Badge count={devices.length} size="small">
          <Button
            shape="circle"
            type="primary"
            icon={<BellOutlined />}
            onClick={() => setExpanded(true)}
            data-testid="availability-check-notification-bubble"
            style={{
              width: 42,
              height: 42,
              boxShadow: token.boxShadowSecondary,
            }}
          />
        </Badge>
      )}
    </div>
  );
}
