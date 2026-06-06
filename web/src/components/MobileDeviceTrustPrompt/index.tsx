import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { Alert, Button, List, Modal, Space, Tag, Typography, message } from "antd";
import { AndroidOutlined, AppleOutlined, MobileOutlined, SendOutlined } from "@ant-design/icons";
import {
  installMobileCa,
  type InstallMode,
  type MobileDevice,
  type MobileDevicesResponse,
} from "../../api/cert";
import { normalizeApiErrorMessage } from "../../api/client";
import { pushService, type SettingsScope } from "../../services/pushService";

const { Text } = Typography;

function deviceDisplayName(device: MobileDevice): string {
  return device.name?.trim() || device.id;
}

function certificateStateLabel(device: MobileDevice): string | null {
  switch (device.certificate_status?.state) {
    case "installed":
      return "CA installed";
    case "pushed_to_device":
      return "CA pushed";
    case "not_installed":
      return "CA not installed";
    case "unknown":
      return "CA unknown";
    default:
      return null;
  }
}

function connectedDevices(info: MobileDevicesResponse | null): MobileDevice[] {
  if (!info) {
    return [];
  }
  return [...info.android.devices, ...info.ios.devices].filter(
    (device) => device.status === "connected",
  );
}

function withSettingsScope(scope: SettingsScope): SettingsScope[] {
  return Array.from(
    new Set([...(pushService.getSubscription().settings_scopes ?? []), scope]),
  );
}

function mobileDevicesFromPushData(data: unknown): MobileDevicesResponse | null {
  if (!data || typeof data !== "object") {
    return null;
  }
  return data as MobileDevicesResponse;
}

export default function MobileDeviceTrustPrompt() {
  const navigate = useNavigate();
  const location = useLocation();
  const [mobileInfo, setMobileInfo] = useState<MobileDevicesResponse | null>(null);
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [dismissedDeviceIds, setDismissedDeviceIds] = useState<Set<string>>(() => new Set());
  const [installingDeviceId, setInstallingDeviceId] = useState<string | null>(null);
  const [suppressPrompt, setSuppressPrompt] = useState(false);

  const devices = useMemo(() => connectedDevices(mobileInfo), [mobileInfo]);
  const visibleDevices = useMemo(
    () => devices.filter((device) => !dismissedDeviceIds.has(device.id)),
    [devices, dismissedDeviceIds],
  );
  const selectedDevice =
    visibleDevices.find((device) => device.id === selectedDeviceId) ?? visibleDevices[0] ?? null;
  const browserPath = typeof window === "undefined" ? "" : window.location.pathname;
  const browserSearch = typeof window === "undefined" ? "" : window.location.search;
  const isCertificateSettingsPage =
    `${location.pathname} ${browserPath}`.includes("/settings") &&
    (new URLSearchParams(location.search).get("tab") === "certificate" ||
      new URLSearchParams(browserSearch).get("tab") === "certificate");
  const promptOpen = visibleDevices.length > 0 && !isCertificateSettingsPage && !suppressPrompt;

  useEffect(() => {
    setSuppressPrompt(isCertificateSettingsPage);
  }, [isCertificateSettingsPage]);

  useEffect(() => {
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: withSettingsScope("mobile_devices"),
    });
    const unsubscribe = pushService.onSettingsUpdate((update) => {
      if (update.scope !== "mobile_devices" || installingDeviceId) {
        return;
      }
      const info = mobileDevicesFromPushData(update.data);
      if (!info) {
        return;
      }
      setMobileInfo(info);
      const candidates = connectedDevices(info).filter(
        (device) => !dismissedDeviceIds.has(device.id),
      );
      setSelectedDeviceId((current) =>
        current && candidates.some((device) => device.id === current)
          ? current
          : candidates[0]?.id ?? null,
      );
    });
    return () => {
      unsubscribe();
      pushService.disconnectIfIdle();
    };
  }, [dismissedDeviceIds, installingDeviceId]);

  const dismissVisibleDevices = useCallback(() => {
    setDismissedDeviceIds((prev) => {
      const next = new Set(prev);
      visibleDevices.forEach((device) => next.add(device.id));
      return next;
    });
    setSelectedDeviceId(null);
    setSuppressPrompt(true);
  }, [visibleDevices]);

  const handleOpenGuide = useCallback(() => {
    if (!selectedDevice) {
      return;
    }
    const deviceId = selectedDevice.id;
    setDismissedDeviceIds((prev) => {
      const next = new Set(prev);
      visibleDevices.forEach((device) => next.add(device.id));
      return next;
    });
    setSelectedDeviceId(null);

    const params = new URLSearchParams({
      tab: "certificate",
      mobile_device: deviceId,
      mobile_platform: selectedDevice.platform,
    });
    const target = `/settings?${params.toString()}`;
    if (`${location.pathname}${location.search}` !== target) {
      navigate(target);
    }
  }, [location.pathname, location.search, navigate, selectedDevice, visibleDevices]);

  const handleInstallSelected = useCallback(async () => {
    if (!selectedDevice) {
      return;
    }

    const mode: InstallMode =
      selectedDevice.platform === "ios" ? "managed_auto_trust" : "normal_guide";
    setInstallingDeviceId(selectedDevice.id);
    try {
      const session = await installMobileCa(selectedDevice.id, mode);
      if (session.completed && !session.requires_user_confirmation) {
        message.success("Bifrost started the certificate install flow for the selected device.");
      } else if (session.requires_user_confirmation) {
        message.warning("Finish the confirmation on the selected phone.");
      } else {
        message.warning(session.summary);
      }
      handleOpenGuide();
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Failed to install CA on selected device"));
    } finally {
      setInstallingDeviceId(null);
    }
  }, [handleOpenGuide, selectedDevice]);

  const selectedInstallDisabled =
    !selectedDevice ||
    selectedDevice.status !== "connected" ||
    (selectedDevice.platform === "ios" &&
      !mobileInfo?.ios.configurator.cfgutil_available);
  const selectedInstallTooltip =
    selectedDevice?.platform === "ios" && !mobileInfo?.ios.configurator.cfgutil_available
      ? "Install Apple Configurator on this Mac first."
      : selectedDevice?.platform === "ios"
        ? "Install the profile on this selected iPhone/iPad through Apple Configurator."
        : "Push the CA to this selected Android device and open the installer.";

  return (
    <Modal
      open={promptOpen}
      title={
        visibleDevices.length > 1
          ? `Install Bifrost CA on one of ${visibleDevices.length} connected devices?`
          : selectedDevice?.platform === "ios"
            ? "Install Bifrost CA profile on connected iPhone?"
            : "Install Bifrost CA on connected Android device?"
      }
      okText="Open Certificate Setup"
      cancelText="Not now"
      onOk={handleOpenGuide}
      onCancel={dismissVisibleDevices}
      footer={(_, { OkBtn, CancelBtn }) => (
        <Space wrap>
          <CancelBtn />
          <Button
            icon={<SendOutlined />}
            loading={installingDeviceId === selectedDevice?.id}
            disabled={selectedInstallDisabled}
            onClick={() => void handleInstallSelected()}
            title={selectedInstallTooltip}
          >
            Install Selected
          </Button>
          <OkBtn />
        </Space>
      )}
      data-testid="global-mobile-device-trust-modal"
    >
      <Space direction="vertical" style={{ width: "100%" }} size="middle">
        <Alert
          type="info"
          showIcon
          icon={
            selectedDevice?.platform === "ios" ? (
              <AppleOutlined />
            ) : selectedDevice?.platform === "android" ? (
              <AndroidOutlined />
            ) : (
              <MobileOutlined />
            )
          }
          message="Bifrost detected connected mobile devices"
          description={
            visibleDevices.length > 1
              ? "Select the device that should receive the Bifrost CA. You can install directly from this dialog or open the Certificate settings page with that device highlighted."
              : selectedDevice?.platform === "ios"
                ? "Install directly through Apple Configurator when available, or open Certificate settings for the iOS profile and QR fallback."
                : "Install directly through ADB, or open Certificate settings to review the Android guided install steps."
          }
        />
        <List
          size="small"
          dataSource={visibleDevices}
          renderItem={(device) => {
            const selected = selectedDevice?.id === device.id;
            const caStateLabel = certificateStateLabel(device);
            return (
              <List.Item
                onClick={() => setSelectedDeviceId(device.id)}
                style={{
                  cursor: "pointer",
                  borderRadius: 6,
                  paddingInline: 8,
                  background: selected ? "var(--ant-color-primary-bg)" : undefined,
                  border: selected
                    ? "1px solid var(--ant-color-primary)"
                    : "1px solid transparent",
                }}
              >
                <List.Item.Meta
                  avatar={device.platform === "ios" ? <AppleOutlined /> : <AndroidOutlined />}
                  title={
                    <Space wrap>
                      <Text>{deviceDisplayName(device)}</Text>
                      <Tag color={device.platform === "ios" ? "blue" : "green"}>
                        {device.platform}
                      </Tag>
                      {caStateLabel ? <Tag>{caStateLabel}</Tag> : null}
                      <Tag color={device.capability === "managed_auto_trust" ? "purple" : "gold"}>
                        {device.capability === "managed_auto_trust"
                          ? "Configurator"
                          : "Guided"}
                      </Tag>
                    </Space>
                  }
                  description={
                    <Space direction="vertical" size={2}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        ID: <Text code>{device.id}</Text>
                        {device.managed_install_target ? (
                          <>
                            {" "}
                            ECID: <Text code>{device.managed_install_target}</Text>
                          </>
                        ) : null}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {device.status_message}
                      </Text>
                      {device.certificate_status ? (
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {device.certificate_status.message}
                        </Text>
                      ) : null}
                    </Space>
                  }
                />
              </List.Item>
            );
          }}
        />
        <Text type="secondary">
          Personal phones still require phone-side confirmation before the CA is installed or
          trusted. Configurator targets the selected iOS device by ECID.
        </Text>
      </Space>
    </Modal>
  );
}
