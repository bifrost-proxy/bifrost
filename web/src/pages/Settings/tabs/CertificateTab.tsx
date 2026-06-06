import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Alert,
  Button,
  Card,
  Checkbox,
  Col,
  Divider,
  Image,
  Input,
  List,
  Row,
  Select,
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
  getIosWifiProxyConfigQRCodeUrl,
  getIosWifiProxyConfigUrl,
  getCertInfo,
  getMobileDevices,
  installLocalCa,
  installIosWifiProxyProfile,
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
import {
  getSharedIosWifiProxySsid,
  setSharedIosWifiProxySsid,
  subscribeSharedIosWifiProxySsid,
} from "../../../utils/iosWifiProxySsid";
import iosStep1Image from "../../../assets/ios/ios_1.png";
import iosStep2Image from "../../../assets/ios/ios_2.png";
import iosStep3Image from "../../../assets/ios/ios_3.png";
import iosStep4Image from "../../../assets/ios/ios_4.png";
import iosStep5Image from "../../../assets/ios/ios_5.png";
import iosStep6Image from "../../../assets/ios/ios_6.png";
import iosStep7Image from "../../../assets/ios/ios_7.png";
import iosQrStep1Image from "../../../assets/ios/ios_qr_1.jpeg";
import iosQrStep2Image from "../../../assets/ios/ios_qr_2.jpeg";

const { Text, Paragraph } = Typography;

const LOCAL_CA_TRUST_POLL_ATTEMPTS = 20;
const LOCAL_CA_TRUST_POLL_INTERVAL_MS = 1000;
const MANAGED_WIFI_PROFILE_RISK_TEXT =
  "Experimental profile note: Bifrost's iOS Wi-Fi proxy profile uses Apple's managed Wi-Fi payload. It does not contain a Wi-Fi password, but uninstalling the profile can remove that managed Wi-Fi network entry from iOS. Manual Wi-Fi proxy setup is the safe cleanup path.";

const CERTIFICATE_SECTION_IDS = [
  "certificate-trust-probe",
  "certificate-local-install",
  "certificate-mobile-ios",
  "certificate-mobile-android",
  "certificate-downloads",
] as const;
type CertificateSectionId = (typeof CERTIFICATE_SECTION_IDS)[number];

const IOS_INSTALL_STEPS = [
  {
    key: "ios_1",
    image: iosStep1Image,
    title: "Choose iPhone for the profile",
    description:
      "When iOS asks which device should receive the profile, choose iPhone. Configurator targets the selected USB device for you.",
  },
  {
    key: "ios_2",
    image: iosStep2Image,
    title: "Open Settings after the profile is ready",
    description:
      "If iOS says the profile was downloaded, tap Close and open Settings. If Configurator opens the install screen directly, continue there.",
  },
  {
    key: "ios_3",
    image: iosStep3Image,
    title: "Open the downloaded profile in Settings",
    description:
      "Open Settings and tap Downloaded Profile. This starts the actual Bifrost CA profile installation.",
  },
  {
    key: "ios_4",
    image: iosStep4Image,
    title: "Install the Bifrost CA profile",
    description:
      "On the Bifrost CA profile screen, tap Install in the top-right corner.",
  },
  {
    key: "ios_5",
    image: iosStep5Image,
    title: "Accept the unsigned profile warning",
    description:
      "iOS warns that the profile is unsigned because Bifrost creates a local CA. Tap Install again if you trust this Bifrost instance.",
  },
  {
    key: "ios_6",
    image: iosStep6Image,
    title: "Go to About after profile installation",
    description:
      "After the profile is installed, return to Settings > General and open About. Installation alone does not enable HTTPS trust.",
  },
  {
    key: "ios_7",
    image: iosStep7Image,
    title: "Enable full trust for Bifrost CA",
    description:
      "Open Certificate Trust Settings and turn on full trust for Bifrost CA. HTTPS inspection works only after this step.",
  },
];

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

const IOS_QR_DELIVERY_STEPS = [
  {
    key: "ios_qr_1",
    image: iosQrStep1Image,
    title: "Scan the LAN profile QR code",
    description:
      "Open Camera on the iPhone, point it at Bifrost's LAN QR code, then tap the yellow link for the LAN address.",
  },
  {
    key: "ios_qr_2",
    image: iosQrStep2Image,
    title: "Allow the configuration profile download",
    description:
      "When iOS asks whether this website may download a configuration profile, tap Allow, then continue with the shared steps below.",
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
  const nextTop = Math.max(0, scrollParent.scrollTop + targetRect.top - parentRect.top - 8);
  scrollParent.scrollTop = nextTop;
  scrollParent.scrollTo({
    top: nextTop,
    behavior: "smooth",
  });
}

function mobileDeviceTitle(device: { id: string; name?: string | null }) {
  return device.name?.trim() || device.id;
}

function certificateStateLabel(state: DeviceCertificateState) {
  switch (state) {
    case "installed":
      return "CA installed";
    case "pushed_to_device":
      return "CA pushed";
    case "not_installed":
      return "CA not installed";
    case "unknown":
    default:
      return "CA unknown";
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
        {extra ? `${device.status_message} ${extra}` : device.status_message}
      </Text>
      {device.certificate_status ? (
        <Text type="secondary" style={{ fontSize: 12 }}>
          {device.certificate_status.message}
        </Text>
      ) : null}
    </Space>
  );
}

export default function CertificateTab({
  certInfo,
  getCertDownloadUrl,
  getCertQRCodeUrl,
  onCertInfoChange,
}: CertificateTabProps) {
  const { token } = theme.useToken();
  const [searchParams] = useSearchParams();
  const highlightedDeviceId = searchParams.get("mobile_device");
  const highlightedPlatform = searchParams.get("mobile_platform");
  const autoScrolledTargetRef = useRef<string | null>(null);
  const [mobileInfo, setMobileInfo] = useState<MobileDevicesResponse | null>(null);
  const [loadingDevices, setLoadingDevices] = useState(false);
  const [installingDeviceId, setInstallingDeviceId] = useState<string | null>(null);
  const [installingIosConfiguratorDeviceId, setInstallingIosConfiguratorDeviceId] = useState<
    string | null
  >(null);
  const [installingIosProxyDeviceId, setInstallingIosProxyDeviceId] = useState<string | null>(null);
  const [iosProxyHost, setIosProxyHost] = useState("");
  const [manualIosProxySsid, setManualIosProxySsid] = useState(() =>
    getSharedIosWifiProxySsid(),
  );
  const [ackManagedWifiProfileRisk, setAckManagedWifiProfileRisk] = useState(false);
  const [installSession, setInstallSession] = useState<InstallSession | null>(null);
  const [installingLocalCa, setInstallingLocalCa] = useState(false);
  const [activeSectionId, setActiveSectionId] = useState<string>(() => {
    if (typeof window === "undefined") {
      return CERTIFICATE_SECTION_IDS[0];
    }
    const hashSectionId = window.location.hash.replace(/^#/, "");
    return CERTIFICATE_SECTION_IDS.includes(hashSectionId as CertificateSectionId)
      ? hashSectionId
      : CERTIFICATE_SECTION_IDS[0];
  });
  const certStatus = certInfo?.status ?? "unknown";
  const detectedIosProxySsid = mobileInfo?.suggested_wifi_ssid?.trim() ?? "";
  const iosProxySsid = detectedIosProxySsid || manualIosProxySsid.trim();
  const certStatusLabel = certInfo?.status_label ?? "Check failed";
  const certStatusColor =
    certStatus === "installed_and_trusted"
      ? "green"
      : certStatus === "installed_not_trusted"
        ? "orange"
        : certStatus === "not_installed"
          ? "red"
          : "default";
  const certStatusIcon =
    certStatus === "installed_and_trusted" ? <CheckOutlined /> : <CloseOutlined />;

  const loadMobileDevices = useCallback(async (refresh = false, silent = false) => {
    if (!silent) {
      setLoadingDevices(true);
    }
    try {
      const info = refresh ? await refreshMobileDevices() : await getMobileDevices();
      setMobileInfo(info);
    } catch (error) {
      if (!silent) {
        message.error(normalizeApiErrorMessage(error, "Failed to detect mobile devices"));
      }
    } finally {
      if (!silent) {
        setLoadingDevices(false);
      }
    }
  }, []);

  useEffect(() => {
    void loadMobileDevices(false);
  }, [loadMobileDevices]);

  useEffect(() => {
    if (iosProxyHost || !certInfo?.local_ips?.length) {
      return;
    }
    setIosProxyHost(certInfo.local_ips[0]);
  }, [certInfo?.local_ips, iosProxyHost]);

  useEffect(() => {
    return subscribeSharedIosWifiProxySsid((ssid) => {
      if (!detectedIosProxySsid) {
        setManualIosProxySsid(ssid);
      }
    });
  }, [detectedIosProxySsid]);

  useEffect(() => {
    if (detectedIosProxySsid) {
      setSharedIosWifiProxySsid(detectedIosProxySsid);
    }
  }, [detectedIosProxySsid]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void loadMobileDevices(true, true);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [loadMobileDevices]);

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
      const scrollParent = firstSection ? findScrollableParent(firstSection) : null;
      const parentTop = scrollParent?.getBoundingClientRect().top ?? 0;
      const hashSectionId = window.location.hash.replace(/^#/, "");
      if (CERTIFICATE_SECTION_IDS.includes(hashSectionId as CertificateSectionId)) {
        const hashSection = document.getElementById(hashSectionId);
        if (hashSection) {
          const hashSectionTop = hashSection.getBoundingClientRect().top - parentTop;
          const containerHeight = scrollParent?.clientHeight ?? window.innerHeight;
          if (hashSectionTop >= -24 && hashSectionTop <= containerHeight - 120) {
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
    const scrollParent = firstSection ? findScrollableParent(firstSection) : null;
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
        message.success("Bifrost CA is installed and trusted on this computer.");
        return;
      }

      message.warning(
        `${nextInfo.status_message} If you just completed macOS authorization, Bifrost will refresh again when you reopen this page.`,
      );
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Failed to install local CA certificate"));
    } finally {
      setInstallingLocalCa(false);
    }
  }, [onCertInfoChange]);

  const handleInstallAndroid = useCallback(
    async (deviceId: string) => {
      setInstallingDeviceId(deviceId);
      setInstallSession(null);
      try {
        const session = await installMobileCa(deviceId);
        setInstallSession(session);
        await loadMobileDevices(true, true);
        message.success("CA pushed to Android. Finish the confirmation on the phone.");
      } catch (error) {
        message.error(normalizeApiErrorMessage(error, "Failed to start Android install flow"));
      } finally {
        setInstallingDeviceId(null);
      }
    },
    [loadMobileDevices],
  );

  const handleInstallIosConfigurator = useCallback(async (deviceId: string) => {
    setInstallingIosConfiguratorDeviceId(deviceId);
    setInstallSession(null);
    try {
      const session = await installMobileCa(deviceId, "managed_auto_trust");
      setInstallSession(session);
      if (session.completed && !session.requires_user_confirmation) {
        message.success("Apple Configurator installed the iOS profile.");
      } else if (session.requires_user_confirmation) {
        message.warning("Apple Configurator opened the install flow. Confirm it on the iPhone.");
      } else {
        message.warning("Apple Configurator could not complete the install flow.");
      }
    } catch (error) {
      message.error(
        normalizeApiErrorMessage(error, "Failed to install iOS profile with Apple Configurator"),
      );
    } finally {
      setInstallingIosConfiguratorDeviceId(null);
    }
  }, []);

  const handleInstallIosProxyProfile = useCallback(
    async (deviceId: string) => {
      const ssid = iosProxySsid.trim();
      if (!ssid) {
        message.warning(
          "Enter the exact Wi-Fi name shown on the iPhone before sending the proxy profile.",
        );
        return;
      }
      if (!iosProxyHost) {
        message.warning("Select the proxy address before sending the proxy profile.");
        return;
      }
      setInstallingIosProxyDeviceId(deviceId);
      setInstallSession(null);
      try {
        setSharedIosWifiProxySsid(ssid);
        const session = await installIosWifiProxyProfile(deviceId, ssid, iosProxyHost);
        setInstallSession(session);
        if (session.completed && !session.requires_user_confirmation) {
          message.success("Apple Configurator installed the iOS Wi-Fi proxy profile.");
        } else if (session.requires_user_confirmation) {
          message.warning(
            "Apple Configurator opened the proxy profile install flow. Confirm it on the iPhone.",
          );
        } else {
          message.warning("Apple Configurator could not complete the proxy profile install flow.");
        }
      } catch (error) {
        message.error(
          normalizeApiErrorMessage(
            error,
            "Failed to install iOS Wi-Fi proxy profile with Apple Configurator",
          ),
        );
      } finally {
        setInstallingIosProxyDeviceId(null);
      }
    },
    [iosProxyHost, iosProxySsid],
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
    },
    [],
  );

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
      const link = target?.closest<HTMLAnchorElement>("[data-certificate-section-link='true']");
      const sectionId = link?.dataset.certificateSectionId;
      if (!sectionId) {
        return;
      }
      event.preventDefault();
      navigateToCertificateSection(sectionId);
    };

    document.addEventListener("click", handleCapturedSectionClick, true);
    return () => document.removeEventListener("click", handleCapturedSectionClick, true);
  }, [navigateToCertificateSection]);

  const navItems = [
    {
      href: "certificate-trust-probe",
      icon: <MobileOutlined />,
      label: "Availability check",
    },
    {
      href: "certificate-local-install",
      icon: <SafetyCertificateOutlined />,
      label: "Local install",
    },
    {
      href: "certificate-mobile-ios",
      icon: <AppleOutlined />,
      label: "iOS devices",
    },
    {
      href: "certificate-mobile-android",
      icon: <AndroidOutlined />,
      label: "Android devices",
    },
    {
      href: "certificate-downloads",
      icon: <QrcodeOutlined />,
      label: "Certificate downloads",
    },
  ];

  return (
    <Row gutter={[16, 16]} data-testid="settings-certificate-tab">
      <Col xs={24} md={6} lg={5} xl={4}>
        <div style={{ position: "sticky", top: 8 }}>
          <Card title="Certificate Setup" size="small">
            <Space direction="vertical" style={{ width: "100%" }} size={4}>
              {navItems.map((item) => (
                (() => {
                  const active = activeSectionId === item.href;
                  return (
                    <a
                      key={item.href}
                      href={`#${item.href}`}
                      onClick={(event) => handleSectionNavClick(event, item.href)}
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
                        background: active ? token.colorPrimaryBg : "transparent",
                        color: active ? token.colorPrimary : token.colorText,
                        fontWeight: active ? 600 : 400,
                        boxShadow: active ? `inset 3px 0 0 ${token.colorPrimary}` : "none",
                        transition:
                          "background-color 120ms ease, border-color 120ms ease, color 120ms ease, box-shadow 120ms ease",
                      }}
                    >
                      {item.icon}
                      <span>{item.label}</span>
                    </a>
                  );
                })()
              ))}
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
                <span>Availability Check</span>
              </Space>
            }
            size="small"
          >
            <AvailabilityCheckPanel certInfo={certInfo} testIdPrefix="settings-trust-probe" />
          </Card>

        <Card
          id="certificate-local-install"
          title={
            <Space>
              <SafetyCertificateOutlined />
              <span>Local Certificate Install</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <Row justify="space-between" align="middle">
              <Col>
                <Text>Certificate Status</Text>
              </Col>
              <Col>
                <Tag color={certStatusColor} icon={certStatusIcon}>
                  {certStatusLabel}
                </Tag>
              </Col>
            </Row>

            <Divider style={{ margin: "8px 0" }} />

            <Text type="secondary" style={{ fontSize: 12 }}>
              {certInfo?.status_message ??
                "Unable to verify whether the CA certificate is installed and trusted."}
            </Text>

            <Text type="secondary" style={{ fontSize: 12 }}>
              Install and trust the Bifrost CA on this computer when you want desktop apps and
              browsers on this machine to trust certificates generated by Bifrost.
            </Text>

            {certStatus !== "installed_and_trusted" ? (
              <Tooltip
                title={
                  certInfo?.available
                    ? "Run the same local system trust install flow as `bifrost ca install`."
                    : "The CA certificate file is not available yet."
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
                  {certStatus === "installed_not_trusted" ? "Trust CA" : "Install and Trust CA"}
                </Button>
              </Tooltip>
            ) : null}

            {certInfo?.sha256_fingerprint ? (
              <Paragraph type="secondary" style={{ fontSize: 12, marginBottom: 0 }}>
                SHA-256 fingerprint: <Text code>{certInfo.sha256_fingerprint}</Text>
              </Paragraph>
            ) : null}
          </Space>
        </Card>

        <Card
          id="certificate-mobile-ios"
          title={
            <Space>
              <AppleOutlined />
              <span>iOS Mobile Installation</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <Alert
              type="info"
              showIcon
              icon={<MobileOutlined />}
              message="iOS uses one shared install and trust flow"
              description="First send the Bifrost CA profile to the iPhone, then complete the iOS Settings steps on the phone. Apple Configurator and QR/manual delivery only change how the profile reaches the phone."
            />

            <Steps
              size="small"
              direction="vertical"
              current={-1}
              items={[
                {
                  title: "Send the profile",
                  description:
                    "Use Configurator Install for a connected USB device, or scan the QR code / download the profile on the iPhone.",
                },
                {
                  title: "Install the profile in Settings",
                  description:
                    "Open Settings > Downloaded Profile, then install the Bifrost CA profile.",
                },
                {
                  title: "Open certificate trust settings",
                  description: "Go to Settings > General > About > Certificate Trust Settings.",
                },
                {
                  title: "Enable full trust",
                  description:
                    "Turn on full trust for Bifrost CA. This is the final step that enables SSL/TLS trust.",
                },
              ]}
            />

            <Text strong>1. Choose how to send the profile</Text>
            <Alert
              type="info"
              showIcon
              message="Recommended: configure Wi-Fi proxy manually"
              description="On ordinary iPhone, the safe cleanup path is Settings > Wi-Fi > current network > Configure Proxy > Manual, then later set it back to Off. The profile option below is experimental: iOS treats it as a managed Wi-Fi configuration, and removing the profile may also remove that Wi-Fi entry from the phone."
            />
            <Row gutter={[12, 12]} align="bottom">
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Wi-Fi network for proxy profile
                </Text>
                {detectedIosProxySsid ? (
                  <div style={{ marginTop: 4, minHeight: 32, display: "flex", alignItems: "center" }}>
                    <Text code data-testid="settings-mobile-ios-proxy-ssid">
                      {detectedIosProxySsid}
                    </Text>
                  </div>
                ) : (
                  <Input
                    value={manualIosProxySsid}
                    onChange={(event) => setManualIosProxySsid(event.target.value)}
                    onBlur={(event) => {
                      const ssid = event.target.value.trim();
                      if (ssid) {
                        setSharedIosWifiProxySsid(ssid);
                      }
                    }}
                    onPressEnter={(event) => {
                      const ssid = event.currentTarget.value.trim();
                      if (ssid) {
                        setSharedIosWifiProxySsid(ssid);
                        message.success("Wi-Fi name shared with Availability Check pages.");
                      }
                    }}
                    placeholder="Exact Wi-Fi name shown on the iPhone"
                    data-testid="settings-mobile-ios-proxy-ssid-input"
                    style={{ marginTop: 4 }}
                  />
                )}
              </Col>
              <Col xs={24} md={12}>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Proxy address
                </Text>
                <Select
                  value={iosProxyHost || undefined}
                  onChange={setIosProxyHost}
                  placeholder="Select this computer's LAN IP"
                  options={(certInfo?.local_ips ?? []).map((ip) => ({
                    value: ip,
                    label: `${ip}:${window.location.port || "9900"}`,
                  }))}
                  disabled={!certInfo?.local_ips?.length}
                  data-testid="settings-mobile-ios-proxy-host"
                  style={{ width: "100%", marginTop: 4 }}
                />
              </Col>
            </Row>
            {!detectedIosProxySsid ? (
              <Space direction="vertical" size={2}>
                <Text type="secondary" data-testid="settings-mobile-ios-proxy-ssid-missing">
                  {mobileInfo?.suggested_wifi_ssid_message ??
                    "Not detected. Enter the exact Wi-Fi name shown on the iPhone."}
                </Text>
                <a
                  href="x-apple.systempreferences:com.apple.preference.security?Privacy_LocationServices"
                  data-testid="settings-mobile-ios-location-services"
                >
                  Open macOS Location Services
                </a>
              </Space>
            ) : null}
            <Text
              type="secondary"
              style={{ display: "block", fontSize: 12 }}
              data-testid="settings-mobile-ios-ssid-sync-hint"
            >
              This Wi-Fi name is shared with open Availability Check panels. If the phone has the
              check page open, it polls the session once per second and updates the profile link.
            </Text>
            <Alert
              type="warning"
              showIcon
              message="Experimental managed Wi-Fi profile"
              description={MANAGED_WIFI_PROFILE_RISK_TEXT}
              data-testid="settings-mobile-ios-wifi-risk-near-ssid"
            />
            <Divider style={{ margin: "4px 0" }} />
            <Alert
              type={mobileInfo?.ios.configurator.cfgutil_available ? "success" : "info"}
              showIcon
              message="Send profile with Apple Configurator"
              description={
                <Space direction="vertical" size={4}>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {mobileInfo?.ios.configurator.message ??
                      "Install Apple Configurator on macOS to enable computer-side profile installation."}
                  </Text>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Connect the iPhone over USB, unlock it, tap Trust for this Mac if prompted,
                    then choose the device below. If the iPhone asks for confirmation, continue with
                    the same Settings steps below.
                  </Text>
                </Space>
              }
            />
            <Button
              icon={<ReloadOutlined />}
              onClick={() => void loadMobileDevices(true)}
              loading={loadingDevices}
              data-testid="settings-mobile-detect-ios"
            >
              Refresh Devices
            </Button>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {mobileInfo?.ios.message ?? "Connect an iPhone or iPad over USB to show it here."}
            </Text>
            <List
              size="small"
              locale={{
                emptyText: "No iPhone or iPad USB devices detected",
              }}
              dataSource={mobileInfo?.ios.devices ?? []}
              renderItem={(device) => {
                const highlighted = isHighlightedDevice(device.id, "ios");
                return (
                  <List.Item
                    className={highlighted ? "mobile-device-target-card" : undefined}
                    data-mobile-device-id={device.id}
                    data-mobile-device-platform="ios"
                    style={highlighted ? highlightedDeviceStyle : undefined}
                  >
                    <Space direction="vertical" size="small" style={{ width: "100%" }}>
                      <List.Item.Meta
                        title={
                          <Space wrap>
                            <Text>{mobileDeviceTitle(device)}</Text>
                            <Tag color="blue">{device.status}</Tag>
                            <Tag color="purple">advanced</Tag>
                          </Space>
                        }
                        description={mobileDeviceDescription(
                          device,
                          "For Configurator install, unlock the iPhone and trust this Mac first.",
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
                              ? "Install profile through Apple Configurator"
                              : "Install Apple Configurator on this Mac first"
                          }
                        >
                          <Button
                            icon={<SendOutlined />}
                            size="small"
                            className={highlighted ? "mobile-device-install-cta-pulse" : undefined}
                            disabled={
                              !certInfo?.available ||
                              !mobileInfo?.ios.configurator.cfgutil_available ||
                              device.status !== "connected"
                            }
                            loading={installingIosConfiguratorDeviceId === device.id}
                            onClick={() => void handleInstallIosConfigurator(device.id)}
                            data-mobile-install-cta="true"
                            data-testid="settings-mobile-install-ios-configurator"
                            style={{ width: "100%" }}
                          >
                            Configurator Install
                          </Button>
                        </Tooltip>
                        <Tooltip
                          title={
                            mobileInfo?.ios.configurator.cfgutil_available
                              ? "Send a Wi-Fi proxy profile through Apple Configurator"
                              : "Install Apple Configurator on this Mac first"
                          }
                        >
                          <Button
                            icon={<SendOutlined />}
                            size="small"
                            disabled={
                              !certInfo?.available ||
                              !mobileInfo?.ios.configurator.cfgutil_available ||
                              device.status !== "connected" ||
                              !iosProxySsid.trim() ||
                              !iosProxyHost ||
                              !ackManagedWifiProfileRisk
                            }
                            loading={installingIosProxyDeviceId === device.id}
                            onClick={() => void handleInstallIosProxyProfile(device.id)}
                            data-testid="settings-mobile-install-ios-proxy-config"
                            style={{ width: "100%" }}
                          >
                            Proxy Config
                          </Button>
                        </Tooltip>
                      </Space>
                    </Space>
                  </List.Item>
                );
              }}
            />
            {installSession?.platform === "ios" ? (
              <Alert
                type={
                  installSession.completed && !installSession.requires_user_confirmation
                    ? "success"
                    : "warning"
                }
                showIcon
                message={installSession.summary}
                description={
                  <List
                    size="small"
                    dataSource={installSession.steps}
                    renderItem={(step) => (
                      <List.Item>
                        <Space>
                          {step.success ? <CheckOutlined /> : <ExclamationCircleOutlined />}
                          <Text>{step.name}</Text>
                          <Text type="secondary">{step.message}</Text>
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
              message="Or send profile manually with QR or file"
              description="Scan the QR code with the iPhone Camera app, open the profile in Safari, AirDrop the file, or download it directly on the phone. After the profile reaches the iPhone, continue with the same Settings and trust steps below."
            />
            <Row gutter={[12, 12]} data-testid="settings-mobile-ios-qr-delivery-guide">
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
                      alt={`${step.key}: ${step.title}`}
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
                          {step.key}: {step.title}
                        </Text>
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {step.description}
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
              Download iOS Profile
            </Button>
            {iosProxySsid.trim() && iosProxyHost ? (
              <Alert
                type="warning"
                showIcon
                message="Experimental managed Wi-Fi profile"
                description={
                  <Space direction="vertical" size={6}>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      This profile uses Apple's managed Wi-Fi payload. It does not contain the Wi-Fi
                      password, but uninstalling the profile can remove the managed Wi-Fi network
                      entry from iOS. Use manual Wi-Fi proxy settings when you need clean removal.
                    </Text>
                    <Checkbox
                      checked={ackManagedWifiProfileRisk}
                      onChange={(event) => setAckManagedWifiProfileRisk(event.target.checked)}
                      data-testid="settings-mobile-ios-managed-wifi-risk"
                    >
                      I understand removing this profile may remove this Wi-Fi entry.
                    </Checkbox>
                  </Space>
                }
              />
            ) : null}
            {iosProxySsid.trim() && iosProxyHost ? (
              <Button
                icon={<DownloadOutlined />}
                href={getIosWifiProxyConfigUrl(iosProxySsid.trim(), iosProxyHost)}
                download="bifrost-ios-wifi-proxy.mobileconfig"
                disabled={!certInfo?.available || !ackManagedWifiProfileRisk}
                data-testid="settings-mobile-ios-wifi-proxy-profile"
              >
                Download Experimental Wi-Fi Proxy Profile
              </Button>
            ) : null}
            {certInfo?.available ? (
              <Row gutter={[16, 16]} data-testid="settings-mobile-ios-profile-qr-list">
                {(certInfo.local_ips.length > 0 ? certInfo.local_ips : [""]).map((ip, index) => (
                  <Col key={ip || index}>
                    <div style={{ textAlign: "center" }}>
                      <Image
                        src={getIosMobileConfigQRCodeUrl(ip || undefined)}
                        alt={`iOS Profile QR Code - ${ip || "current host"}`}
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
                            {ip}
                          </Text>
                        </div>
                      ) : null}
                    </div>
                  </Col>
                ))}
                {iosProxySsid.trim() && iosProxyHost ? (
                  <Col data-testid="settings-mobile-ios-wifi-proxy-qr-list">
                    <div style={{ textAlign: "center" }}>
                      {ackManagedWifiProfileRisk ? (
                        <Image
                          src={getIosWifiProxyConfigQRCodeUrl(iosProxySsid.trim(), iosProxyHost)}
                          alt={`iOS Wi-Fi Proxy Profile QR Code - ${iosProxyHost}`}
                          width={120}
                          height={120}
                          preview={{
                            mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                          }}
                          fallback="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mN8/+F9PQAJpAN4pokyXwAAAABJRU5ErkJggg=="
                          data-testid="settings-mobile-ios-wifi-proxy-qrcode"
                        />
                      ) : (
                        <div
                          style={{
                            width: 120,
                            height: 120,
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "center",
                            border: `1px solid ${token.colorBorderSecondary}`,
                            borderRadius: token.borderRadius,
                            color: token.colorTextSecondary,
                            fontSize: 12,
                            textAlign: "center",
                            padding: 8,
                          }}
                        >
                          Confirm risk to show QR
                        </div>
                      )}
                      <div style={{ marginTop: 4 }}>
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          Experimental Wi-Fi proxy profile: {iosProxySsid.trim()} to {iosProxyHost}:
                          {window.location.port || "9900"}
                        </Text>
                      </div>
                    </div>
                  </Col>
                ) : null}
              </Row>
            ) : null}
            <Divider style={{ margin: "4px 0" }} />
            <div data-testid="settings-mobile-ios-manual-guide">
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <Text strong>2. Finish the shared iOS Settings steps</Text>
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
                          alt={`${step.key}: ${step.title}`}
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
                              {step.key}: {step.title}
                            </Text>
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              {step.description}
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
              <span>Android Mobile Installation</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <Alert
              type="info"
              showIcon
              icon={<MobileOutlined />}
              message="Android uses USB guided install"
              description="Bifrost can detect ADB devices, push the CA file, and open the Android installer or security settings. The phone still needs user confirmation, and many Android 7+ apps only trust user CAs when their Network Security Config allows it."
            />
            <Button
              icon={<ReloadOutlined />}
              onClick={() => void loadMobileDevices(true)}
              loading={loadingDevices}
              data-testid="settings-mobile-detect-android"
            >
              Detect Devices
            </Button>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {mobileInfo?.android.message ??
                "Use ADB to detect USB Android devices and open the certificate installer."}
            </Text>
            <List
              size="small"
              locale={{ emptyText: "No Android USB devices detected" }}
              dataSource={mobileInfo?.android.devices ?? []}
              renderItem={(device) => {
                const highlighted = isHighlightedDevice(device.id, "android");
                return (
                  <List.Item
                    className={highlighted ? "mobile-device-target-card" : undefined}
                    data-mobile-device-id={device.id}
                    data-mobile-device-platform="android"
                    style={highlighted ? highlightedDeviceStyle : undefined}
                  >
                    <Space direction="vertical" size="small" style={{ width: "100%" }}>
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
                              <Tag color={certificateStateColor(device.certificate_status.state)}>
                                {certificateStateLabel(device.certificate_status.state)}
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
                              ? "Push CA and open Android installer"
                              : "Resolve the device status before installation"
                          }
                        >
                          <Button
                            icon={<SendOutlined />}
                            size="small"
                            className={highlighted ? "mobile-device-install-cta-pulse" : undefined}
                            disabled={!certInfo?.available || device.status !== "connected"}
                            loading={installingDeviceId === device.id}
                            onClick={() => void handleInstallAndroid(device.id)}
                            data-mobile-install-cta="true"
                            data-testid="settings-mobile-install-android"
                            style={{ width: "100%" }}
                          >
                            Install
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
                message={installSession.summary}
                description={
                  <List
                    size="small"
                    dataSource={installSession.steps}
                    renderItem={(step) => (
                      <List.Item>
                        <Space>
                          {step.success ? <CheckOutlined /> : <ExclamationCircleOutlined />}
                          <Text>{step.name}</Text>
                          <Text type="secondary">{step.message}</Text>
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
              <span>Certificate Downloads and QR Codes</span>
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
              Download CA Certificate
            </Button>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {certInfo?.available
                ? "Download the CA certificate file and install it as a trusted root CA on devices that support manual CA installation."
                : "The CA certificate file is not available yet, so it cannot be downloaded or trusted on this device."}
            </Text>
            {certInfo?.available && certInfo.download_urls.length > 0 ? (
              <>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Certificate file QR codes remain available for manual installation.
                </Text>
                <Row gutter={[16, 16]} justify="start">
                  {certInfo.download_urls.map((url, index) => {
                    const ip = certInfo.local_ips[index] || "";
                    return (
                      <Col key={ip || index}>
                        <div style={{ textAlign: "center" }}>
                          <Image
                            src={getCertQRCodeUrl(ip || undefined)}
                            alt={`Certificate QR Code - ${ip}`}
                            width={120}
                            height={120}
                            preview={{
                              mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
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
                {certInfo?.available ? "No download URLs available" : "QR code not available"}
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
