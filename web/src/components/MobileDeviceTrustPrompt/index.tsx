import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import {
  Alert,
  Button,
  List,
  Modal,
  Space,
  Tag,
  Typography,
  message,
} from "antd";
import {
  AndroidOutlined,
  AppleOutlined,
  MobileOutlined,
  SendOutlined,
} from "@ant-design/icons";
import {
  installMobileCa,
  type InstallMode,
  type MobileDevice,
  type MobileDevicesResponse,
} from "../../api/cert";
import { normalizeApiErrorMessage } from "../../api/client";
import { pushService, type SettingsScope } from "../../services/pushService";
import { useI18n, type TFunction } from "../../i18n";
import { useBackendMessage } from "../../i18n/backendMessages";

const { Text } = Typography;

function deviceDisplayName(device: MobileDevice): string {
  return device.name?.trim() || device.id;
}

function certificateStateLabel(
  t: TFunction,
  device: MobileDevice,
): string | null {
  switch (device.certificate_status?.state) {
    case "installed":
      return t("app.mobileTrust.caInstalled");
    case "pushed_to_device":
      return t("app.mobileTrust.caPushed");
    case "not_installed":
      return t("app.mobileTrust.caNotInstalled");
    case "unknown":
      return t("app.mobileTrust.caUnknown");
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

function mobileDevicesFromPushData(
  data: unknown,
): MobileDevicesResponse | null {
  if (!data || typeof data !== "object") {
    return null;
  }
  return data as MobileDevicesResponse;
}

export default function MobileDeviceTrustPrompt() {
  const navigate = useNavigate();
  const location = useLocation();
  const { t } = useI18n();
  const bm = useBackendMessage();
  const [mobileInfo, setMobileInfo] = useState<MobileDevicesResponse | null>(
    null,
  );
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [dismissedDeviceIds, setDismissedDeviceIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [installingDeviceId, setInstallingDeviceId] = useState<string | null>(
    null,
  );
  const [suppressPrompt, setSuppressPrompt] = useState(false);

  const devices = useMemo(() => connectedDevices(mobileInfo), [mobileInfo]);
  const visibleDevices = useMemo(
    () => devices.filter((device) => !dismissedDeviceIds.has(device.id)),
    [devices, dismissedDeviceIds],
  );
  const selectedDevice =
    visibleDevices.find((device) => device.id === selectedDeviceId) ??
    visibleDevices[0] ??
    null;
  const browserPath =
    typeof window === "undefined" ? "" : window.location.pathname;
  const browserSearch =
    typeof window === "undefined" ? "" : window.location.search;
  const isCertificateSettingsPage =
    `${location.pathname} ${browserPath}`.includes("/settings") &&
    (new URLSearchParams(location.search).get("tab") === "certificate" ||
      new URLSearchParams(browserSearch).get("tab") === "certificate");
  const promptOpen =
    visibleDevices.length > 0 && !isCertificateSettingsPage && !suppressPrompt;

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
          : (candidates[0]?.id ?? null),
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
  }, [
    location.pathname,
    location.search,
    navigate,
    selectedDevice,
    visibleDevices,
  ]);

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
        message.success(t("app.mobileTrust.installStarted"));
      } else if (session.requires_user_confirmation) {
        message.warning(t("app.mobileTrust.finishOnPhone"));
      } else {
        message.warning(bm(session.summary));
      }
      handleOpenGuide();
    } catch (error) {
      message.error(
        normalizeApiErrorMessage(error, t("app.mobileTrust.installFailed")),
      );
    } finally {
      setInstallingDeviceId(null);
    }
  }, [bm, handleOpenGuide, selectedDevice, t]);

  const selectedInstallDisabled =
    !selectedDevice ||
    selectedDevice.status !== "connected" ||
    (selectedDevice.platform === "ios" &&
      !mobileInfo?.ios.configurator.cfgutil_available);
  const selectedInstallTooltip =
    selectedDevice?.platform === "ios" &&
    !mobileInfo?.ios.configurator.cfgutil_available
      ? t("app.mobileTrust.installConfiguratorFirst")
      : selectedDevice?.platform === "ios"
        ? t("app.mobileTrust.installIosTooltip")
        : t("app.mobileTrust.installAndroidTooltip");

  return (
    <Modal
      open={promptOpen}
      title={
        visibleDevices.length > 1
          ? t("app.mobileTrust.titleMultiple", { count: visibleDevices.length })
          : selectedDevice?.platform === "ios"
            ? t("app.mobileTrust.titleIos")
            : t("app.mobileTrust.titleAndroid")
      }
      okText={t("app.mobileTrust.openCertificateSetup")}
      cancelText={t("app.mobileTrust.notNow")}
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
            {t("app.mobileTrust.installSelected")}
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
          message={t("app.mobileTrust.detected")}
          description={
            visibleDevices.length > 1
              ? t("app.mobileTrust.descriptionMultiple")
              : selectedDevice?.platform === "ios"
                ? t("app.mobileTrust.descriptionIos")
                : t("app.mobileTrust.descriptionAndroid")
          }
        />
        <List
          size="small"
          dataSource={visibleDevices}
          renderItem={(device) => {
            const selected = selectedDevice?.id === device.id;
            const caStateLabel = certificateStateLabel(t, device);
            return (
              <List.Item
                onClick={() => setSelectedDeviceId(device.id)}
                style={{
                  cursor: "pointer",
                  borderRadius: 6,
                  paddingInline: 8,
                  background: selected
                    ? "var(--ant-color-primary-bg)"
                    : undefined,
                  border: selected
                    ? "1px solid var(--ant-color-primary)"
                    : "1px solid transparent",
                }}
              >
                <List.Item.Meta
                  avatar={
                    device.platform === "ios" ? (
                      <AppleOutlined />
                    ) : (
                      <AndroidOutlined />
                    )
                  }
                  title={
                    <Space wrap>
                      <Text>{deviceDisplayName(device)}</Text>
                      <Tag color={device.platform === "ios" ? "blue" : "green"}>
                        {device.platform}
                      </Tag>
                      {caStateLabel ? <Tag>{caStateLabel}</Tag> : null}
                      <Tag
                        color={
                          device.capability === "managed_auto_trust"
                            ? "purple"
                            : "gold"
                        }
                      >
                        {device.capability === "managed_auto_trust"
                          ? "Configurator"
                          : t("app.mobileTrust.guided")}
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
                            ECID:{" "}
                            <Text code>{device.managed_install_target}</Text>
                          </>
                        ) : null}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {bm(device.status_message)}
                      </Text>
                      {device.certificate_status ? (
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {bm(device.certificate_status.message)}
                        </Text>
                      ) : null}
                    </Space>
                  }
                />
              </List.Item>
            );
          }}
        />
        <Text type="secondary">{t("app.mobileTrust.footnote")}</Text>
      </Space>
    </Modal>
  );
}
