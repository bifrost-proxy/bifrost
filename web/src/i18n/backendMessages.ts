import { useCallback } from "react";
import { useLocaleStore, type Locale } from "./store";

/**
 * Some device / certificate status text is produced by the Rust backend
 * (crates/bifrost-device, crates/bifrost-admin/src/handlers/cert.rs and
 * mobile_devices.rs) and returned in English. The backend has no locale
 * negotiation, so known messages are localized here for display. Unknown
 * text is returned unchanged.
 */
const ZH_CN_EXACT: Record<string, string> = {
  // Android (ADB)
  "ADB is not available. Install Android Platform Tools or configure BIFROST_ADB_PATH to detect USB Android devices.":
    "ADB 不可用。请安装 Android Platform Tools，或配置 BIFROST_ADB_PATH 以检测 USB 连接的 Android 设备。",
  "ADB is available, but no Android USB devices were reported.":
    "ADB 可用，但未检测到通过 USB 连接的 Android 设备。",
  "Ready to push the Bifrost CA and open the Android certificate installer. The phone still requires user confirmation.":
    "可以推送 Bifrost CA 证书并打开 Android 证书安装器，仍需在手机上手动确认。",
  "Unlock the phone and allow USB debugging for this computer, then refresh devices.":
    "请解锁手机并允许这台电脑进行 USB 调试，然后刷新设备列表。",
  "The device is offline. Reconnect USB or restart ADB.":
    "设备已离线，请重新连接 USB 或重启 ADB。",
  "Current Bifrost CA is installed in Android's user certificate store. Android 7+ apps may still ignore user CAs unless their Network Security Config allows them.":
    "当前 Bifrost CA 证书已安装到 Android 用户证书存储。Android 7 及以上的应用默认可能不信任用户证书，除非其 Network Security Config 允许。",
  "Current Bifrost CA file is on the phone, but it was not found in the readable Android user certificate store. Finish the certificate installer on the phone.":
    "当前 Bifrost CA 证书文件已在手机上，但未在可读取的 Android 用户证书存储中找到。请在手机上完成证书安装。",
  "Bifrost could read Android's user certificate store and did not find the current Bifrost CA.":
    "Bifrost 已读取 Android 用户证书存储，未找到当前的 Bifrost CA 证书。",
  "Current Bifrost CA file is on the phone. Ordinary Android ADB cannot verify the private user certificate store, so finish the installer and confirm on the phone.":
    "当前 Bifrost CA 证书文件已在手机上。普通 ADB 无法校验私有的用户证书存储，请在手机上完成安装并确认。",
  "Bifrost can see this Android device, but ordinary ADB cannot verify whether the CA is installed. Rooted/emulator test devices can expose the user certificate store for verification.":
    "Bifrost 已识别这台 Android 设备，但普通 ADB 无法校验 CA 证书是否已安装。已 root 的测试机或模拟器可以开放用户证书存储用于校验。",
  "Bifrost pushed the CA certificate and opened Android's installer flow. Confirm installation and trust on the phone.":
    "Bifrost 已推送 CA 证书并打开 Android 安装流程，请在手机上确认安装并信任。",
  "Bifrost could not complete the Android installer handoff. Review the failed step and finish installation manually if needed.":
    "Bifrost 未能完成 Android 安装流程的交接。请查看失败的步骤，必要时手动完成安装。",
  // iOS (Apple Configurator)
  "No iPhone or iPad USB devices detected.":
    "未检测到通过 USB 连接的 iPhone 或 iPad。",
  "Apple Configurator cfgutil is available. Supervised devices can receive profiles automatically; unsupervised devices may still require Trust, unlock, or onscreen confirmation.":
    "Apple Configurator cfgutil 可用。受监管设备可自动接收描述文件；非受监管设备可能仍需在手机上信任、解锁或确认。",
  "Apple Configurator cfgutil was not found. Install Apple Configurator from the Mac App Store to enable computer-side iPhone profile installation.":
    "未找到 Apple Configurator cfgutil。请从 Mac App Store 安装 Apple Configurator，以便从电脑端为 iPhone 安装描述文件。",
  "Apple Configurator installation is supported on macOS only.":
    "仅 macOS 支持通过 Apple Configurator 安装。",
  "Apple Configurator installed the Bifrost profile. Certificate payloads installed through Configurator are trusted for SSL/TLS automatically.":
    "Apple Configurator 已安装 Bifrost 描述文件。通过 Configurator 安装的证书会自动被 SSL/TLS 信任。",
  "Apple Configurator opened the Bifrost profile install flow on the iPhone. Finish the confirmation on the phone; certificate payloads installed through Configurator are trusted for SSL/TLS automatically.":
    "Apple Configurator 已在 iPhone 上打开 Bifrost 描述文件安装流程。请在手机上完成确认；通过 Configurator 安装的证书会自动被 SSL/TLS 信任。",
  "Apple Configurator could not install the Bifrost profile. Unlock the iPhone, tap Trust for this Mac if prompted, then retry.":
    "Apple Configurator 无法安装 Bifrost 描述文件。请解锁 iPhone，如有提示请点击「信任此电脑」，然后重试。",
  "Detected over USB. Download the iOS profile, install it on the phone, then enable full trust in Certificate Trust Settings.":
    "已通过 USB 检测到。请下载 iOS 描述文件并在手机上安装，然后在「证书信任设置」中开启完全信任。",
  "Detected over USB through Apple Configurator. Select this device to install the Bifrost CA profile with cfgutil; unsupervised devices may still require iPhone confirmation.":
    "已通过 Apple Configurator 经 USB 检测到。选择此设备即可用 cfgutil 安装 Bifrost CA 描述文件；非受监管设备可能仍需在 iPhone 上确认。",
  // Mobile device notices
  "Mobile device discovery is unavailable.": "移动设备发现不可用。",
  "Personal Android and iOS devices require final confirmation on the phone before the CA is installed or fully trusted.":
    "个人 Android 和 iOS 设备需要在手机上最终确认，CA 证书才会被安装或完全信任。",
  "Managed Android/iOS devices can support automatic trust only through Device Owner/Profile Owner, MDM, Apple Configurator, or equivalent fleet tooling.":
    "受管理的 Android/iOS 设备只能通过 Device Owner/Profile Owner、MDM、Apple Configurator 或同类设备管理工具实现自动信任。",
  // Local CA trust status
  "Not installed": "未安装",
  "Check failed": "检查失败",
  "Installed, not trusted": "已安装，未信任",
  "Installed and trusted": "已安装并信任",
  "CA certificate file is missing, so system trust is not configured.":
    "CA 证书文件缺失，系统信任未配置。",
  "CA certificate is not installed in the system trust store.":
    "CA 证书未安装到系统信任存储。",
  "CA certificate is installed, but the system does not trust it yet.":
    "CA 证书已安装，但系统尚未信任。",
  "CA certificate is installed and trusted by the system.":
    "CA 证书已安装，且已被系统信任。",
};

const ZH_CN_PATTERNS: [RegExp, (...groups: string[]) => string][] = [
  [
    /^ADB found (\d+) Android device\(s\)\.$/,
    (n) => `ADB 发现 ${n} 台 Android 设备。`,
  ],
  [
    /^Detected (\d+) iOS USB device\(s\)\.$/,
    (n) => `检测到 ${n} 台通过 USB 连接的 iOS 设备。`,
  ],
  [/^ADB failed to list devices: (.*)$/s, (e) => `ADB 获取设备列表失败：${e}`],
  [/^ADB failed to start: (.*)$/s, (e) => `ADB 启动失败：${e}`],
  [
    /^ADB reported unsupported device status: (.*)$/s,
    (s) => `ADB 返回了不支持的设备状态：${s}`,
  ],
  [
    /^Could not read the local Bifrost CA certificate: (.*)$/s,
    (e) => `无法读取本地 Bifrost CA 证书：${e}`,
  ],
  [
    /^Could not parse the local Bifrost CA certificate: (.*)$/s,
    (e) => `无法解析本地 Bifrost CA 证书：${e}`,
  ],
  [
    /^Unable to verify whether the CA certificate is trusted: (.*)$/s,
    (e) => `无法确认 CA 证书是否已被信任：${e}`,
  ],
];

export function localizeBackendMessage(locale: Locale, text: string): string;
export function localizeBackendMessage(
  locale: Locale,
  text: string | null | undefined,
): string | null | undefined;
export function localizeBackendMessage(
  locale: Locale,
  text: string | null | undefined,
) {
  if (!text || locale !== "zh-CN") return text;
  const exact = ZH_CN_EXACT[text];
  if (exact) return exact;
  for (const [pattern, render] of ZH_CN_PATTERNS) {
    const match = pattern.exec(text);
    if (match) return render(...match.slice(1));
  }
  return text;
}

/** Hook form: re-renders with the current locale. */
export function useBackendMessage() {
  const locale = useLocaleStore((state) => state.locale);
  return useCallback(
    <T extends string | null | undefined>(text: T) =>
      localizeBackendMessage(locale, text) as T,
    [locale],
  );
}
