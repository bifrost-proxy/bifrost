import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent,
} from "react";
import { useSearchParams } from "react-router-dom";
import {
  Alert,
  Button,
  Card,
  Col,
  Divider,
  Image,
  List,
  Row,
  Space,
  Steps,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  AndroidOutlined,
  AppleOutlined,
  CheckOutlined,
  CloseOutlined,
  DownloadOutlined,
  ExclamationCircleOutlined,
  MobileOutlined,
  QrcodeOutlined,
  ReloadOutlined,
  SafetyCertificateOutlined,
  SendOutlined,
} from "@ant-design/icons";
import {
  getIosMobileConfigQRCodeUrl,
  getIosMobileConfigUrl,
  getCertInfo,
  getMobileDevices,
  installLocalCa,
  installMobileCa,
  refreshMobileDevices,
  type CertInfo,
  type DeviceCertificateState,
  type InstallSession,
  type MobileDevice,
  type MobileDevicesResponse,
} from "../../../api/cert";
import { normalizeApiErrorMessage } from "../../../api/client";
import AvailabilityCheckPanel from "../../../components/AvailabilityCheckPanel";
import { pushService, type SettingsScope } from "../../../services/pushService";
import iosStep1Image from "../../../assets/ios/ios_1.png";
import iosStep2Image from "../../../assets/ios/ios_2.png";
import iosStep3Image from "../../../assets/ios/ios_3.png";
import iosStep4Image from "../../../assets/ios/ios_4.png";
import iosStep5Image from "../../../assets/ios/ios_5.png";
import iosStep6Image from "../../../assets/ios/ios_6.png";
import iosStep7Image from "../../../assets/ios/ios_7.png";
import iosQrStep1Image from "../../../assets/ios/ios_qr_1.jpeg";
import iosQrStep2Image from "../../../assets/ios/ios_qr_2.jpeg";
import {
  getLocale,
  useI18n,
  type TFunction,
  type TranslationKey,
} from "../../../i18n";
import {
  localizeBackendMessage,
  useBackendMessage,
} from "../../../i18n/backendMessages";

const { Text, Paragraph } = Typography;

const LOCAL_CA_TRUST_POLL_ATTEMPTS = 20;
const LOCAL_CA_TRUST_POLL_INTERVAL_MS = 1000;
const APPLE_CONFIGURATOR_APP_STORE_URL =
  "macappstore://itunes.apple.com/app/id1037126344";
const APPLE_CONFIGURATOR_WEB_URL =
  "https://apps.apple.com/us/app/apple-configurator/id1037126344";
const CERTIFICATE_SECTION_IDS = [
  "certificate-trust-probe",
  "certificate-local-install",
  "certificate-mobile-ios",
  "certificate-mobile-android",
  "certificate-downloads",
] as const;
type CertificateSectionId = (typeof CERTIFICATE_SECTION_IDS)[number];

const IOS_INSTALL_STEPS: {
  key: string;
  image: string;
  titleKey: TranslationKey;
  descriptionKey: TranslationKey;
}[] = [
  {
    key: "ios_1",
    image: iosStep1Image,
    titleKey: "settings.cert.iosStep1Title",
    descriptionKey: "settings.cert.iosStep1Description",
  },
  {
    key: "ios_2",
    image: iosStep2Image,
    titleKey: "settings.cert.iosStep2Title",
    descriptionKey: "settings.cert.iosStep2Description",
  },
  {
    key: "ios_3",
    image: iosStep3Image,
    titleKey: "settings.cert.iosStep3Title",
    descriptionKey: "settings.cert.iosStep3Description",
  },
  {
    key: "ios_4",
    image: iosStep4Image,
    titleKey: "settings.cert.iosStep4Title",
    descriptionKey: "settings.cert.iosStep4Description",
  },
  {
    key: "ios_5",
    image: iosStep5Image,
    titleKey: "settings.cert.iosStep5Title",
    descriptionKey: "settings.cert.iosStep5Description",
  },
  {
    key: "ios_6",
    image: iosStep6Image,
    titleKey: "settings.cert.iosStep6Title",
    descriptionKey: "settings.cert.iosStep6Description",
  },
  {
    key: "ios_7",
    image: iosStep7Image,
    titleKey: "settings.cert.iosStep7Title",
    descriptionKey: "settings.cert.iosStep7Description",
  },
];

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

const IOS_QR_DELIVERY_STEPS: {
  key: string;
  image: string;
  titleKey: TranslationKey;
  descriptionKey: TranslationKey;
}[] = [
  {
    key: "ios_qr_1",
    image: iosQrStep1Image,
    titleKey: "settings.cert.iosQrStep1Title",
    descriptionKey: "settings.cert.iosQrStep1Description",
  },
  {
    key: "ios_qr_2",
    image: iosQrStep2Image,
    titleKey: "settings.cert.iosQrStep2Title",
    descriptionKey: "settings.cert.iosQrStep2Description",
  },
];

export interface CertificateTabProps {
  certInfo: CertInfo | null;
  getCertDownloadUrl: () => string;
  getCertQRCodeUrl: (ip?: string) => string;
  onCertInfoChange?: (info: CertInfo) => void;
}

function findScrollableParent(element: HTMLElement): HTMLElement | null {
  let parent = element.parentElement;
  while (parent) {
    const style = window.getComputedStyle(parent);
    const canScroll =
      /(auto|scroll|overlay)/.test(style.overflowY) &&
      parent.scrollHeight > parent.clientHeight + 1;
    if (canScroll) {
      return parent;
    }
    parent = parent.parentElement;
  }
  return null;
}

function scrollMobileDeviceTargetIntoView(target: HTMLElement) {
  const scrollParent = findScrollableParent(target);
  if (!scrollParent) {
    target.scrollIntoView({
      block: "center",
      behavior: "smooth",
      inline: "nearest",
    });
    return;
  }

  const parentRect = scrollParent.getBoundingClientRect();
  const targetRect = target.getBoundingClientRect();
  const top =
    scrollParent.scrollTop +
    targetRect.top -
    parentRect.top -
    (scrollParent.clientHeight - targetRect.height) / 2;
  const nextTop = Math.max(0, top);

  scrollParent.scrollTop = nextTop;
  scrollParent.scrollTo({
    top: nextTop,
    behavior: "smooth",
  });
}

function scrollCertificateSectionIntoView(target: HTMLElement) {
  const scrollParent = findScrollableParent(target);
  if (!scrollParent) {
    target.scrollIntoView({
      block: "start",
      behavior: "smooth",
      inline: "nearest",
    });
    return;
  }

  const parentRect = scrollParent.getBoundingClientRect();
  const targetRect = target.getBoundingClientRect();
  const nextTop = Math.max(
    0,
    scrollParent.scrollTop + targetRect.top - parentRect.top - 8,
  );
  scrollParent.scrollTop = nextTop;
  scrollParent.scrollTo({
    top: nextTop,
    behavior: "smooth",
  });
}

function mobileDeviceTitle(device: { id: string; name?: string | null }) {
  return device.name?.trim() || device.id;
}

function certificateStateLabel(state: DeviceCertificateState, t: TFunction) {
  switch (state) {
    case "installed":
      return t("settings.cert.caInstalled");
    case "pushed_to_device":
      return t("settings.cert.caPushed");
    case "not_installed":
      return t("settings.cert.caNotInstalled");
    case "unknown":
    default:
      return t("settings.cert.caUnknown");
  }
}

function certificateStateColor(state: DeviceCertificateState) {
  switch (state) {
    case "installed":
      return "green";
    case "pushed_to_device":
      return "gold";
    case "not_installed":
      return "red";
    case "unknown":
    default:
      return "default";
  }
}

function mobileDeviceDescription(device: MobileDevice, extra?: string) {
  return (
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
        {extra
          ? `${localizeBackendMessage(getLocale(), device.status_message)} ${extra}`
          : localizeBackendMessage(getLocale(), device.status_message)}
      </Text>
      {device.certificate_status ? (
        <Text type="secondary" style={{ fontSize: 12 }}>
          {localizeBackendMessage(
            getLocale(),
            device.certificate_status.message,
          )}
        </Text>
      ) : null}
    </Space>
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

export default function CertificateTab({
  certInfo,
  getCertDownloadUrl,
  getCertQRCodeUrl,
  onCertInfoChange,
}: CertificateTabProps) {
  const { t } = useI18n();
  const bm = useBackendMessage();
  const { token } = theme.useToken();
  const [searchParams] = useSearchParams();
  const highlightedDeviceId = searchParams.get("mobile_device");
  const highlightedPlatform = searchParams.get("mobile_platform");
  const autoScrolledTargetRef = useRef<string | null>(null);
  const [mobileInfo, setMobileInfo] = useState<MobileDevicesResponse | null>(
    null,
  );
  const [loadingDevices, setLoadingDevices] = useState(false);
  const [installingDeviceId, setInstallingDeviceId] = useState<string | null>(
    null,
  );
  const [
    installingIosConfiguratorDeviceId,
    setInstallingIosConfiguratorDeviceId,
  ] = useState<string | null>(null);
  const [installSession, setInstallSession] = useState<InstallSession | null>(
    null,
  );
  const [installingLocalCa, setInstallingLocalCa] = useState(false);
  const [activeSectionId, setActiveSectionId] = useState<string>(() => {
    if (typeof window === "undefined") {
      return CERTIFICATE_SECTION_IDS[0];
    }
    const hashSectionId = window.location.hash.replace(/^#/, "");
    return CERTIFICATE_SECTION_IDS.includes(
      hashSectionId as CertificateSectionId,
    )
      ? hashSectionId
      : CERTIFICATE_SECTION_IDS[0];
  });
  const certStatus = certInfo?.status ?? "unknown";
  const certStatusLabel =
    bm(certInfo?.status_label) ?? t("settings.cert.checkFailed");
  const certStatusColor =
    certStatus === "installed_and_trusted"
      ? "green"
      : certStatus === "installed_not_trusted"
        ? "orange"
        : certStatus === "not_installed"
          ? "red"
          : "default";
  const certStatusIcon =
    certStatus === "installed_and_trusted" ? (
      <CheckOutlined />
    ) : (
      <CloseOutlined />
    );

  const loadMobileDevices = useCallback(
    async (refresh = false, silent = false) => {
      if (!silent) {
        setLoadingDevices(true);
      }
      try {
        const info = refresh
          ? await refreshMobileDevices()
          : await getMobileDevices();
        setMobileInfo(info);
      } catch (error) {
        if (!silent) {
          message.error(
            normalizeApiErrorMessage(
              error,
              t("settings.cert.detectDevicesFailed"),
            ),
          );
        }
      } finally {
        if (!silent) {
          setLoadingDevices(false);
        }
      }
    },
    [t],
  );

  useEffect(() => {
    void loadMobileDevices(false);
  }, [loadMobileDevices]);

  useEffect(() => {
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: withSettingsScope("mobile_devices"),
    });
    const unsubscribe = pushService.onSettingsUpdate((update) => {
      if (update.scope !== "mobile_devices") {
        return;
      }
      const next = mobileDevicesFromPushData(update.data);
      if (next) {
        setMobileInfo(next);
      }
    });
    return () => {
      unsubscribe();
      pushService.disconnectIfIdle();
    };
  }, []);

  useEffect(() => {
    let frameId: number | null = null;

    const readHashSection = () => {
      const hashSectionId = window.location.hash.replace(/^#/, "");
      if (
        CERTIFICATE_SECTION_IDS.includes(hashSectionId as CertificateSectionId)
      ) {
        setActiveSectionId(hashSectionId);
        const target = document.getElementById(hashSectionId);
        if (target) {
          scrollCertificateSectionIntoView(target);
        }
      }
    };

    const updateActiveSection = () => {
      const firstSection = document.getElementById(CERTIFICATE_SECTION_IDS[0]);
      const scrollParent = firstSection
        ? findScrollableParent(firstSection)
        : null;
      const parentTop = scrollParent?.getBoundingClientRect().top ?? 0;
      const hashSectionId = window.location.hash.replace(/^#/, "");
      if (
        CERTIFICATE_SECTION_IDS.includes(hashSectionId as CertificateSectionId)
      ) {
        const hashSection = document.getElementById(hashSectionId);
        if (hashSection) {
          const hashSectionTop =
            hashSection.getBoundingClientRect().top - parentTop;
          const containerHeight =
            scrollParent?.clientHeight ?? window.innerHeight;
          if (
            hashSectionTop >= -24 &&
            hashSectionTop <= containerHeight - 120
          ) {
            setActiveSectionId(hashSectionId);
            return;
          }
        }
      }

      let nextSectionId: CertificateSectionId = CERTIFICATE_SECTION_IDS[0];

      for (const sectionId of CERTIFICATE_SECTION_IDS) {
        const section = document.getElementById(sectionId);
        if (!section) {
          continue;
        }
        const sectionTop = section.getBoundingClientRect().top - parentTop;
        if (sectionTop <= 120) {
          nextSectionId = sectionId;
        }
      }

      setActiveSectionId(nextSectionId);
    };

    const scheduleUpdate = () => {
      if (frameId !== null) {
        return;
      }
      frameId = window.requestAnimationFrame(() => {
        frameId = null;
        updateActiveSection();
      });
    };

    const firstSection = document.getElementById(CERTIFICATE_SECTION_IDS[0]);
    const scrollParent = firstSection
      ? findScrollableParent(firstSection)
      : null;
    readHashSection();
    scheduleUpdate();
    scrollParent?.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("hashchange", readHashSection);
    window.addEventListener("popstate", readHashSection);

    return () => {
      if (frameId !== null) {
        window.cancelAnimationFrame(frameId);
      }
      scrollParent?.removeEventListener("scroll", scheduleUpdate);
      window.removeEventListener("scroll", scheduleUpdate);
      window.removeEventListener("hashchange", readHashSection);
      window.removeEventListener("popstate", readHashSection);
    };
  }, []);

  useEffect(() => {
    if (!highlightedDeviceId || !mobileInfo) {
      return;
    }

    const targetKey = `${highlightedPlatform ?? "any"}:${highlightedDeviceId}`;
    if (autoScrolledTargetRef.current === targetKey) {
      return;
    }

    const idSelector = `[data-mobile-device-id="${CSS.escape(highlightedDeviceId)}"]`;
    const selector = highlightedPlatform
      ? `${idSelector}[data-mobile-device-platform="${CSS.escape(highlightedPlatform)}"]`
      : idSelector;
    const initialTarget = document.querySelector<HTMLElement>(selector);
    if (!initialTarget) {
      return;
    }

    autoScrolledTargetRef.current = targetKey;
    const timers = [0, 160, 420, 900].map((delay) =>
      window.setTimeout(() => {
        const target = document.querySelector<HTMLElement>(selector);
        if (!target) {
          return;
        }
        scrollMobileDeviceTargetIntoView(target);
        target
          .querySelector<HTMLElement>("[data-mobile-install-cta='true']")
          ?.focus({ preventScroll: true });
      }, delay),
    );

    return () => {
      timers.forEach((timer) => window.clearTimeout(timer));
    };
  }, [highlightedDeviceId, highlightedPlatform, mobileInfo]);

  const isHighlightedDevice = useCallback(
    (deviceId: string, platform: "android" | "ios") =>
      highlightedDeviceId === deviceId &&
      (!highlightedPlatform || highlightedPlatform === platform),
    [highlightedDeviceId, highlightedPlatform],
  );

  const highlightedDeviceStyle = {
    border: `1px solid ${token.colorPrimary}`,
    borderRadius: token.borderRadius,
    background: token.colorPrimaryBg,
    boxShadow: `0 0 0 3px ${token.colorPrimaryBgHover}`,
    paddingInline: 8,
  };

  const handleInstallLocalCa = useCallback(async () => {
    setInstallingLocalCa(true);
    try {
      let nextInfo = await installLocalCa();
      onCertInfoChange?.(nextInfo);

      for (
        let attempt = 0;
        !nextInfo.trusted && attempt < LOCAL_CA_TRUST_POLL_ATTEMPTS;
        attempt += 1
      ) {
        await sleep(LOCAL_CA_TRUST_POLL_INTERVAL_MS);
        nextInfo = await getCertInfo();
        onCertInfoChange?.(nextInfo);
      }

      if (nextInfo.trusted) {
        message.success(t("settings.cert.localTrusted"));
        return;
      }

      message.warning(
        `${bm(nextInfo.status_message)} ${t("settings.cert.localTrustPending")}`,
      );
    } catch (error) {
      message.error(
        normalizeApiErrorMessage(error, t("settings.cert.localInstallFailed")),
      );
    } finally {
      setInstallingLocalCa(false);
    }
  }, [bm, onCertInfoChange, t]);

  const handleInstallAndroid = useCallback(
    async (deviceId: string) => {
      setInstallingDeviceId(deviceId);
      setInstallSession(null);
      try {
        const session = await installMobileCa(deviceId);
        setInstallSession(session);
        await loadMobileDevices(true, true);
        message.success(t("settings.cert.androidPushed"));
      } catch (error) {
        message.error(
          normalizeApiErrorMessage(
            error,
            t("settings.cert.androidInstallFailed"),
          ),
        );
      } finally {
        setInstallingDeviceId(null);
      }
    },
    [loadMobileDevices, t],
  );

  const handleInstallIosConfigurator = useCallback(
    async (deviceId: string) => {
      setInstallingIosConfiguratorDeviceId(deviceId);
      setInstallSession(null);
      try {
        const session = await installMobileCa(deviceId, "managed_auto_trust");
        setInstallSession(session);
        if (session.completed && !session.requires_user_confirmation) {
          message.success(t("settings.cert.configuratorInstalled"));
        } else if (session.requires_user_confirmation) {
          message.warning(t("settings.cert.configuratorConfirm"));
        } else {
          message.warning(t("settings.cert.configuratorIncomplete"));
        }
      } catch (error) {
        message.error(
          normalizeApiErrorMessage(
            error,
            t("settings.cert.configuratorFailed"),
          ),
        );
      } finally {
        setInstallingIosConfiguratorDeviceId(null);
      }
    },
    [t],
  );

  const navigateToCertificateSection = useCallback((sectionId: string) => {
    const target = document.getElementById(sectionId);
    if (!target) {
      return;
    }
    setActiveSectionId(sectionId);
    scrollCertificateSectionIntoView(target);
    if (window.location.hash !== `#${sectionId}`) {
      window.history.pushState(null, "", `#${sectionId}`);
    }
  }, []);

  const handleSectionNavClick = useCallback(
    (event: MouseEvent<HTMLAnchorElement>, sectionId: string) => {
      event.preventDefault();
      navigateToCertificateSection(sectionId);
    },
    [navigateToCertificateSection],
  );

  useEffect(() => {
    const handleCapturedSectionClick = (event: globalThis.MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      const link = target?.closest<HTMLAnchorElement>(
        "[data-certificate-section-link='true']",
      );
      const sectionId = link?.dataset.certificateSectionId;
      if (!sectionId) {
        return;
      }
      event.preventDefault();
      navigateToCertificateSection(sectionId);
    };

    document.addEventListener("click", handleCapturedSectionClick, true);
    return () =>
      document.removeEventListener("click", handleCapturedSectionClick, true);
  }, [navigateToCertificateSection]);

  const navItems = [
    {
      href: "certificate-trust-probe",
      icon: <MobileOutlined />,
      label: t("settings.cert.navAvailability"),
    },
    {
      href: "certificate-local-install",
      icon: <SafetyCertificateOutlined />,
      label: t("settings.cert.navLocalInstall"),
    },
    {
      href: "certificate-mobile-ios",
      icon: <AppleOutlined />,
      label: t("settings.cert.navIos"),
    },
    {
      href: "certificate-mobile-android",
      icon: <AndroidOutlined />,
      label: t("settings.cert.navAndroid"),
    },
    {
      href: "certificate-downloads",
      icon: <QrcodeOutlined />,
      label: t("settings.cert.navDownloads"),
    },
  ];

  return (
    <Row gutter={[16, 16]} data-testid="settings-certificate-tab">
      <Col xs={24} md={6} lg={5} xl={4}>
        <div style={{ position: "sticky", top: 8 }}>
          <Card title={t("settings.cert.setupTitle")} size="small">
            <Space direction="vertical" style={{ width: "100%" }} size={4}>
              {navItems.map((item) =>
                (() => {
                  const active = activeSectionId === item.href;
                  return (
                    <a
                      key={item.href}
                      href={`#${item.href}`}
                      onClick={(event) =>
                        handleSectionNavClick(event, item.href)
                      }
                      data-certificate-section-link="true"
                      data-certificate-section-id={item.href}
                      aria-current={active ? "location" : undefined}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        minHeight: 34,
                        padding: "6px 8px",
                        borderRadius: token.borderRadiusSM,
                        border: `1px solid ${
                          active ? token.colorPrimaryBorder : "transparent"
                        }`,
                        background: active
                          ? token.colorPrimaryBg
                          : "transparent",
                        color: active ? token.colorPrimary : token.colorText,
                        fontWeight: active ? 600 : 400,
                        boxShadow: active
                          ? `inset 3px 0 0 ${token.colorPrimary}`
                          : "none",
                        transition:
                          "background-color 120ms ease, border-color 120ms ease, color 120ms ease, box-shadow 120ms ease",
                      }}
                    >
                      {item.icon}
                      <span>{item.label}</span>
                    </a>
                  );
                })(),
              )}
            </Space>
          </Card>
        </div>
      </Col>

      <Col xs={24} md={18} lg={19} xl={20}>
        <Space direction="vertical" style={{ width: "100%" }} size="middle">
          <Card
            id="certificate-trust-probe"
            title={
              <Space>
                <MobileOutlined />
                <span>{t("settings.cert.availabilityTitle")}</span>
              </Space>
            }
            size="small"
          >
            <AvailabilityCheckPanel
              certInfo={certInfo}
              mobileInfo={mobileInfo}
              autoCreate
              testIdPrefix="settings-trust-probe"
            />
          </Card>

          <Card
            id="certificate-local-install"
            title={
              <Space>
                <SafetyCertificateOutlined />
                <span>{t("settings.cert.localTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              <Row justify="space-between" align="middle">
                <Col>
                  <Text>{t("settings.cert.status")}</Text>
                </Col>
                <Col>
                  <Tag color={certStatusColor} icon={certStatusIcon}>
                    {certStatusLabel}
                  </Tag>
                </Col>
              </Row>

              <Divider style={{ margin: "8px 0" }} />

              <Text type="secondary" style={{ fontSize: 12 }}>
                {bm(certInfo?.status_message) ??
                  t("settings.cert.statusUnknown")}
              </Text>

              <Text type="secondary" style={{ fontSize: 12 }}>
                {t("settings.cert.localHint")}
              </Text>

              {certStatus !== "installed_and_trusted" ? (
                <Tooltip
                  title={
                    certInfo?.available
                      ? t("settings.cert.localInstallTooltip")
                      : t("settings.cert.caFileUnavailable")
                  }
                >
                  <Button
                    type="primary"
                    icon={<SafetyCertificateOutlined />}
                    loading={installingLocalCa}
                    disabled={!certInfo?.available}
                    onClick={() => void handleInstallLocalCa()}
                    data-testid="settings-certificate-local-install"
                  >
                    {certStatus === "installed_not_trusted"
                      ? t("settings.cert.trustCa")
                      : t("settings.cert.installAndTrustCa")}
                  </Button>
                </Tooltip>
              ) : null}

              {certInfo?.sha256_fingerprint ? (
                <Paragraph
                  type="secondary"
                  style={{ fontSize: 12, marginBottom: 0 }}
                >
                  {t("settings.cert.fingerprint")}{" "}
                  <Text code>{certInfo.sha256_fingerprint}</Text>
                </Paragraph>
              ) : null}
            </Space>
          </Card>

          <Card
            id="certificate-mobile-ios"
            title={
              <Space>
                <AppleOutlined />
                <span>{t("settings.cert.iosTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              <Alert
                type="info"
                showIcon
                icon={<MobileOutlined />}
                message={t("settings.cert.iosFlowTitle")}
                description={t("settings.cert.iosFlowDescription")}
              />

              <Steps
                size="small"
                direction="vertical"
                current={-1}
                items={[
                  {
                    title: t("settings.cert.iosOverviewSendTitle"),
                    description: t("settings.cert.iosOverviewSendDescription"),
                  },
                  {
                    title: t("settings.cert.iosOverviewInstallTitle"),
                    description: t(
                      "settings.cert.iosOverviewInstallDescription",
                    ),
                  },
                  {
                    title: t("settings.cert.iosOverviewTrustSettingsTitle"),
                    description: t(
                      "settings.cert.iosOverviewTrustSettingsDescription",
                    ),
                  },
                  {
                    title: t("settings.cert.iosOverviewFullTrustTitle"),
                    description: t(
                      "settings.cert.iosOverviewFullTrustDescription",
                    ),
                  },
                ]}
              />

              <Text strong>{t("settings.cert.iosStepChoose")}</Text>
              <Alert
                type={
                  mobileInfo?.ios.configurator.cfgutil_available
                    ? "success"
                    : "info"
                }
                showIcon
                message={t("settings.cert.configuratorTitle")}
                description={
                  <Space direction="vertical" size={4}>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {bm(mobileInfo?.ios.configurator.message) ??
                        t("settings.cert.configuratorDefaultMessage")}
                    </Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.cert.configuratorUsbHint")}
                    </Text>
                    {!mobileInfo?.ios.configurator.cfgutil_available ? (
                      <Space
                        wrap
                        size={8}
                        data-testid="settings-mobile-ios-configurator-missing"
                      >
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {t("settings.cert.configuratorMissing")}
                        </Text>
                        <Button
                          size="small"
                          href={APPLE_CONFIGURATOR_APP_STORE_URL}
                          target="_blank"
                          rel="noreferrer"
                          data-testid="settings-mobile-ios-configurator-app-store"
                        >
                          {t("settings.cert.openAppStore")}
                        </Button>
                        <a
                          href={APPLE_CONFIGURATOR_WEB_URL}
                          target="_blank"
                          rel="noreferrer"
                          data-testid="settings-mobile-ios-configurator-web-store"
                        >
                          {t("settings.cert.webFallback")}
                        </a>
                      </Space>
                    ) : null}
                  </Space>
                }
              />
              <Button
                icon={<ReloadOutlined />}
                onClick={() => void loadMobileDevices(true)}
                loading={loadingDevices}
                data-testid="settings-mobile-detect-ios"
              >
                {t("settings.cert.refreshDevices")}
              </Button>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {bm(mobileInfo?.ios.message) ??
                  t("settings.cert.iosDevicesHint")}
              </Text>
              <List
                size="small"
                locale={{
                  emptyText: t("settings.cert.iosNoDevices"),
                }}
                dataSource={mobileInfo?.ios.devices ?? []}
                renderItem={(device) => {
                  const highlighted = isHighlightedDevice(device.id, "ios");
                  return (
                    <List.Item
                      className={
                        highlighted ? "mobile-device-target-card" : undefined
                      }
                      data-mobile-device-id={device.id}
                      data-mobile-device-platform="ios"
                      style={highlighted ? highlightedDeviceStyle : undefined}
                    >
                      <Space
                        direction="vertical"
                        size="small"
                        style={{ width: "100%" }}
                      >
                        <List.Item.Meta
                          title={
                            <Space wrap>
                              <Text>{mobileDeviceTitle(device)}</Text>
                              <Tag color="blue">{device.status}</Tag>
                              <Tag color="purple">
                                {t("settings.cert.advanced")}
                              </Tag>
                            </Space>
                          }
                          description={mobileDeviceDescription(
                            device,
                            t("settings.cert.iosDeviceExtra"),
                          )}
                        />
                        <Space
                          direction="vertical"
                          size={6}
                          style={{ width: "100%", maxWidth: 260 }}
                        >
                          <Tooltip
                            title={
                              mobileInfo?.ios.configurator.cfgutil_available
                                ? t("settings.cert.configuratorInstallTooltip")
                                : t("settings.cert.configuratorInstallFirst")
                            }
                          >
                            <Button
                              icon={<SendOutlined />}
                              size="small"
                              className={
                                highlighted
                                  ? "mobile-device-install-cta-pulse"
                                  : undefined
                              }
                              disabled={
                                !certInfo?.available ||
                                !mobileInfo?.ios.configurator
                                  .cfgutil_available ||
                                device.status !== "connected"
                              }
                              loading={
                                installingIosConfiguratorDeviceId === device.id
                              }
                              onClick={() =>
                                void handleInstallIosConfigurator(device.id)
                              }
                              data-mobile-install-cta="true"
                              data-testid="settings-mobile-install-ios-configurator"
                              style={{ width: "100%" }}
                            >
                              {t("settings.cert.configuratorInstall")}
                            </Button>
                          </Tooltip>
                          {!mobileInfo?.ios.configurator.cfgutil_available ? (
                            <Text
                              type="secondary"
                              style={{ fontSize: 12 }}
                              data-testid="settings-mobile-ios-configurator-disabled-reason"
                            >
                              {t("settings.cert.configuratorDisabledReason")}
                            </Text>
                          ) : null}
                        </Space>
                      </Space>
                    </List.Item>
                  );
                }}
              />
              {installSession?.platform === "ios" ? (
                <Alert
                  type={
                    installSession.completed &&
                    !installSession.requires_user_confirmation
                      ? "success"
                      : "warning"
                  }
                  showIcon
                  message={bm(installSession.summary)}
                  description={
                    <List
                      size="small"
                      dataSource={installSession.steps}
                      renderItem={(step) => (
                        <List.Item>
                          <Space>
                            {step.success ? (
                              <CheckOutlined />
                            ) : (
                              <ExclamationCircleOutlined />
                            )}
                            <Text>{step.name}</Text>
                            <Text type="secondary">{bm(step.message)}</Text>
                          </Space>
                        </List.Item>
                      )}
                    />
                  }
                  data-testid="settings-mobile-ios-install-session"
                />
              ) : null}
              <Divider style={{ margin: "4px 0" }} />
              <Alert
                type="warning"
                showIcon
                message={t("settings.cert.manualTitle")}
                description={t("settings.cert.manualDescription")}
              />
              <Row
                gutter={[12, 12]}
                data-testid="settings-mobile-ios-qr-delivery-guide"
              >
                {IOS_QR_DELIVERY_STEPS.map((step) => (
                  <Col xs={24} sm={12} key={step.key}>
                    <figure
                      style={{
                        margin: 0,
                        border: `1px solid ${token.colorBorderSecondary}`,
                        borderRadius: token.borderRadius,
                        padding: 8,
                        background: token.colorBgContainer,
                        height: "100%",
                      }}
                      data-testid={`settings-mobile-ios-step-${step.key}`}
                    >
                      <Image
                        src={step.image}
                        alt={`${step.key}: ${t(step.titleKey)}`}
                        width="100%"
                        height={260}
                        style={{
                          objectFit: "contain",
                          background: token.colorFillAlter,
                          borderRadius: token.borderRadiusSM,
                        }}
                        preview={{
                          mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                        }}
                      />
                      <figcaption style={{ marginTop: 8 }}>
                        <Space direction="vertical" size={2}>
                          <Text strong style={{ fontSize: 12 }}>
                            {step.key}: {t(step.titleKey)}
                          </Text>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {t(step.descriptionKey)}
                          </Text>
                        </Space>
                      </figcaption>
                    </figure>
                  </Col>
                ))}
              </Row>
              <Button
                icon={<DownloadOutlined />}
                href={getIosMobileConfigUrl()}
                download="bifrost-ca.mobileconfig"
                disabled={!certInfo?.available}
                data-testid="settings-mobile-ios-profile"
              >
                {t("settings.cert.downloadIosProfile")}
              </Button>
              {certInfo?.available ? (
                <Row
                  gutter={[16, 16]}
                  data-testid="settings-mobile-ios-profile-qr-list"
                >
                  {(certInfo.local_ips.length > 0
                    ? certInfo.local_ips
                    : [""]
                  ).map((ip, index) => (
                    <Col key={ip || index}>
                      <div style={{ textAlign: "center", width: 180 }}>
                        <Space
                          direction="vertical"
                          size={2}
                          style={{ marginBottom: 8 }}
                        >
                          <Text strong style={{ fontSize: 12 }}>
                            {t("settings.cert.qrInstallProfile")}
                          </Text>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {t("settings.cert.qrScanHint")}
                          </Text>
                        </Space>
                        <Image
                          src={getIosMobileConfigQRCodeUrl(ip || undefined)}
                          alt={t("settings.cert.iosProfileQrAlt", {
                            host: ip || t("settings.cert.currentHost"),
                          })}
                          width={120}
                          height={120}
                          preview={{
                            mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                          }}
                          fallback="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mN8/+F9PQAJpAN4pokyXwAAAABJRU5ErkJggg=="
                          data-testid={
                            index === 0
                              ? "settings-mobile-ios-qrcode"
                              : `settings-mobile-ios-qrcode-${ip}`
                          }
                        />
                        {ip ? (
                          <div style={{ marginTop: 4 }}>
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              {t("settings.cert.profileFrom", { ip })}
                            </Text>
                          </div>
                        ) : null}
                      </div>
                    </Col>
                  ))}
                </Row>
              ) : null}
              <Divider style={{ margin: "4px 0" }} />
              <div data-testid="settings-mobile-ios-manual-guide">
                <Space direction="vertical" size={8} style={{ width: "100%" }}>
                  <Text strong>{t("settings.cert.iosStepFinish")}</Text>
                  <Row gutter={[12, 12]}>
                    {IOS_INSTALL_STEPS.map((step) => (
                      <Col xs={24} sm={12} xl={8} key={step.key}>
                        <figure
                          style={{
                            margin: 0,
                            border: `1px solid ${token.colorBorderSecondary}`,
                            borderRadius: token.borderRadius,
                            padding: 8,
                            background: token.colorBgContainer,
                            height: "100%",
                          }}
                          data-testid={`settings-mobile-ios-step-${step.key}`}
                        >
                          <Image
                            src={step.image}
                            alt={`${step.key}: ${t(step.titleKey)}`}
                            width="100%"
                            height={260}
                            style={{
                              objectFit: "contain",
                              background: token.colorFillAlter,
                              borderRadius: token.borderRadiusSM,
                            }}
                            preview={{
                              mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                            }}
                          />
                          <figcaption style={{ marginTop: 8 }}>
                            <Space direction="vertical" size={2}>
                              <Text strong style={{ fontSize: 12 }}>
                                {step.key}: {t(step.titleKey)}
                              </Text>
                              <Text type="secondary" style={{ fontSize: 12 }}>
                                {t(step.descriptionKey)}
                              </Text>
                            </Space>
                          </figcaption>
                        </figure>
                      </Col>
                    ))}
                  </Row>
                </Space>
              </div>
            </Space>
          </Card>

          <Card
            id="certificate-mobile-android"
            title={
              <Space>
                <AndroidOutlined />
                <span>{t("settings.cert.androidTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              <Alert
                type="info"
                showIcon
                icon={<MobileOutlined />}
                message={t("settings.cert.androidFlowTitle")}
                description={t("settings.cert.androidFlowDescription")}
              />
              <Button
                icon={<ReloadOutlined />}
                onClick={() => void loadMobileDevices(true)}
                loading={loadingDevices}
                data-testid="settings-mobile-detect-android"
              >
                {t("settings.cert.detectDevices")}
              </Button>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {bm(mobileInfo?.android.message) ??
                  t("settings.cert.androidDevicesHint")}
              </Text>
              <List
                size="small"
                locale={{ emptyText: t("settings.cert.androidNoDevices") }}
                dataSource={mobileInfo?.android.devices ?? []}
                renderItem={(device) => {
                  const highlighted = isHighlightedDevice(device.id, "android");
                  return (
                    <List.Item
                      className={
                        highlighted ? "mobile-device-target-card" : undefined
                      }
                      data-mobile-device-id={device.id}
                      data-mobile-device-platform="android"
                      style={highlighted ? highlightedDeviceStyle : undefined}
                    >
                      <Space
                        direction="vertical"
                        size="small"
                        style={{ width: "100%" }}
                      >
                        <List.Item.Meta
                          title={
                            <Space wrap>
                              <Text>{mobileDeviceTitle(device)}</Text>
                              <Tag
                                color={
                                  device.status === "connected"
                                    ? "green"
                                    : device.status === "unauthorized"
                                      ? "orange"
                                      : "default"
                                }
                              >
                                {device.status}
                              </Tag>
                              {device.certificate_status ? (
                                <Tag
                                  color={certificateStateColor(
                                    device.certificate_status.state,
                                  )}
                                >
                                  {certificateStateLabel(
                                    device.certificate_status.state,
                                    t,
                                  )}
                                </Tag>
                              ) : null}
                            </Space>
                          }
                          description={mobileDeviceDescription(device)}
                        />
                        <div style={{ width: "100%", maxWidth: 260 }}>
                          <Tooltip
                            title={
                              device.status === "connected"
                                ? t("settings.cert.androidInstallTooltip")
                                : t("settings.cert.androidResolveStatus")
                            }
                          >
                            <Button
                              icon={<SendOutlined />}
                              size="small"
                              className={
                                highlighted
                                  ? "mobile-device-install-cta-pulse"
                                  : undefined
                              }
                              disabled={
                                !certInfo?.available ||
                                device.status !== "connected"
                              }
                              loading={installingDeviceId === device.id}
                              onClick={() =>
                                void handleInstallAndroid(device.id)
                              }
                              data-mobile-install-cta="true"
                              data-testid="settings-mobile-install-android"
                              style={{ width: "100%" }}
                            >
                              {t("settings.cert.install")}
                            </Button>
                          </Tooltip>
                        </div>
                      </Space>
                    </List.Item>
                  );
                }}
              />
              {installSession?.platform === "android" ? (
                <Alert
                  type={installSession.completed ? "success" : "warning"}
                  showIcon
                  message={bm(installSession.summary)}
                  description={
                    <List
                      size="small"
                      dataSource={installSession.steps}
                      renderItem={(step) => (
                        <List.Item>
                          <Space>
                            {step.success ? (
                              <CheckOutlined />
                            ) : (
                              <ExclamationCircleOutlined />
                            )}
                            <Text>{step.name}</Text>
                            <Text type="secondary">{bm(step.message)}</Text>
                          </Space>
                        </List.Item>
                      )}
                    />
                  }
                  data-testid="settings-mobile-install-session"
                />
              ) : null}
            </Space>
          </Card>

          <Card
            id="certificate-downloads"
            title={
              <Space>
                <QrcodeOutlined />
                <span>{t("settings.cert.downloadsTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              <Button
                type="primary"
                icon={<DownloadOutlined />}
                href={getCertDownloadUrl()}
                download="bifrost-ca.crt"
                disabled={!certInfo?.available}
                data-testid="settings-certificate-download"
              >
                {t("settings.cert.downloadCa")}
              </Button>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {certInfo?.available
                  ? t("settings.cert.downloadHint")
                  : t("settings.cert.downloadUnavailable")}
              </Text>
              {certInfo?.available && certInfo.download_urls.length > 0 ? (
                <>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("settings.cert.qrRemainHint")}
                  </Text>
                  <Row gutter={[16, 16]} justify="start">
                    {certInfo.download_urls.map((url, index) => {
                      const ip = certInfo.local_ips[index] || "";
                      return (
                        <Col key={ip || index}>
                          <div style={{ textAlign: "center" }}>
                            <Image
                              src={getCertQRCodeUrl(ip || undefined)}
                              alt={t("settings.cert.certQrAlt", { ip })}
                              width={120}
                              height={120}
                              preview={{
                                mask: (
                                  <QrcodeOutlined style={{ fontSize: 20 }} />
                                ),
                              }}
                              fallback="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mN8/+F9PQAJpAN4pokyXwAAAABJRU5ErkJggg=="
                              data-testid={
                                index === 0
                                  ? "settings-certificate-qrcode"
                                  : `settings-certificate-qrcode-${ip}`
                              }
                            />
                            <div style={{ marginTop: 4 }}>
                              <a
                                href={url}
                                target="_blank"
                                rel="noreferrer"
                                style={{ fontSize: 12 }}
                              >
                                {url}
                              </a>
                            </div>
                          </div>
                        </Col>
                      );
                    })}
                  </Row>
                </>
              ) : (
                <Text type="secondary">
                  {certInfo?.available
                    ? t("settings.cert.noDownloadUrls")
                    : t("settings.cert.qrUnavailable")}
                </Text>
              )}
            </Space>
          </Card>
          <div aria-hidden="true" style={{ height: "50vh", minHeight: 240 }} />
        </Space>
      </Col>
    </Row>
  );
}
