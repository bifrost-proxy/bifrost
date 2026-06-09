import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import { useNavigate } from "react-router-dom";
import { Badge, Button, Card, Space, Tag, Typography, theme } from "antd";
import { BellOutlined, CloseOutlined, MobileOutlined, RightOutlined } from "@ant-design/icons";
import type { TrustProbeDevice, TrustProbeSession } from "../../api/cert";
import { pushService, type SettingsScope } from "../../services/pushService";
import {
  NOTIFICATION_BUBBLE_DEFAULT_TOP,
  NOTIFICATION_BUBBLE_MARGIN,
  NOTIFICATION_BUBBLE_SIZE,
  NOTIFICATION_PANEL_WIDTH,
  clampNotificationBubblePosition,
  clampNotificationPanelPosition,
  defaultNotificationBubblePosition,
  type FloatingPosition,
} from "./position";
import "./index.css";

const { Text } = Typography;
const DRAG_CLICK_SUPPRESSION_DISTANCE = 4;

interface DragState {
  pointerId: number;
  offsetX: number;
  offsetY: number;
  startX: number;
  startY: number;
  captured: boolean;
  hasMoved: boolean;
}

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
  const rootRef = useRef<HTMLDivElement | null>(null);
  const dragStateRef = useRef<DragState | null>(null);
  const suppressNextClickRef = useRef(false);
  const suppressClickTimerRef = useRef<number | null>(null);
  const [sessions, setSessions] = useState<TrustProbeSession[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [dismissedSignature, setDismissedSignature] = useState<string | null>(null);
  const [draggedPosition, setDraggedPosition] = useState<FloatingPosition | null>(null);

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

  useEffect(
    () => () => {
      if (suppressClickTimerRef.current) {
        window.clearTimeout(suppressClickTimerRef.current);
      }
    },
    [],
  );

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

  useEffect(() => {
    if (!draggedPosition) {
      return;
    }
    const handleResize = () => {
      setDraggedPosition((current) =>
        current
          ? clampNotificationBubblePosition(current, {
              width: window.innerWidth,
              height: window.innerHeight,
            })
          : null,
      );
    };
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [draggedPosition]);

  const handlePointerDown = useCallback((event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || expanded) {
      return;
    }
    const root = rootRef.current;
    if (!root) {
      return;
    }
    const rect = root.getBoundingClientRect();
    dragStateRef.current = {
      pointerId: event.pointerId,
      offsetX: event.clientX - rect.left,
      offsetY: event.clientY - rect.top,
      startX: event.clientX,
      startY: event.clientY,
      captured: false,
      hasMoved: false,
    };
  }, [expanded]);

  const handlePointerMove = useCallback((event: PointerEvent<HTMLDivElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }
    const deltaX = event.clientX - dragState.startX;
    const deltaY = event.clientY - dragState.startY;
    if (Math.hypot(deltaX, deltaY) > DRAG_CLICK_SUPPRESSION_DISTANCE) {
      dragState.hasMoved = true;
    }
    if (!dragState.hasMoved) {
      return;
    }
    if (!dragState.captured) {
      rootRef.current?.setPointerCapture(event.pointerId);
      dragState.captured = true;
    }
    setDraggedPosition(
      clampNotificationBubblePosition(
        {
          left: event.clientX - dragState.offsetX,
          top: event.clientY - dragState.offsetY,
        },
        {
          width: window.innerWidth,
          height: window.innerHeight,
        },
      ),
    );
  }, []);

  const finishDrag = useCallback((event: PointerEvent<HTMLDivElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }
    if (dragState.hasMoved) {
      suppressNextClickRef.current = true;
      if (suppressClickTimerRef.current) {
        window.clearTimeout(suppressClickTimerRef.current);
      }
      suppressClickTimerRef.current = window.setTimeout(() => {
        suppressNextClickRef.current = false;
        suppressClickTimerRef.current = null;
      }, 0);
    }
    dragStateRef.current = null;
    const root = rootRef.current;
    if (dragState.captured && root?.hasPointerCapture(event.pointerId)) {
      root.releasePointerCapture(event.pointerId);
    }
  }, []);

  const handleBubbleClick = useCallback(() => {
    if (suppressNextClickRef.current) {
      suppressNextClickRef.current = false;
      return;
    }
    setExpanded(true);
  }, []);

  if (!visible) {
    return null;
  }

  const activePosition =
    draggedPosition ??
    (typeof window === "undefined"
      ? {
          left:
            NOTIFICATION_PANEL_WIDTH - NOTIFICATION_BUBBLE_SIZE - NOTIFICATION_BUBBLE_MARGIN,
          top: NOTIFICATION_BUBBLE_DEFAULT_TOP,
        }
      : defaultNotificationBubblePosition({
          width: window.innerWidth,
          height: window.innerHeight,
        }));
  const panelPosition =
    typeof window === "undefined"
      ? activePosition
      : clampNotificationPanelPosition(activePosition, {
          width: window.innerWidth,
          height: window.innerHeight,
        });

  return (
    <div
      ref={rootRef}
      data-testid="availability-check-notification-center"
      className={[
        "availability-check-notification-center",
        !expanded ? "availability-check-notification-center--collapsed" : "",
        !expanded && devices.length > 0 ? "availability-check-notification-center--alerting" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
      style={{
        position: "fixed",
        top: expanded ? panelPosition.top : activePosition.top,
        left: expanded ? panelPosition.left : activePosition.left,
        zIndex: 980,
        maxWidth: NOTIFICATION_PANEL_WIDTH,
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
            onClick={handleBubbleClick}
            data-testid="availability-check-notification-bubble"
            className="availability-check-notification-bubble"
            aria-label={`${devices.length} availability check notification${devices.length === 1 ? "" : "s"}`}
            style={{
              width: NOTIFICATION_BUBBLE_SIZE,
              height: NOTIFICATION_BUBBLE_SIZE,
              boxShadow: token.boxShadowSecondary,
            }}
          />
        </Badge>
      )}
    </div>
  );
}
