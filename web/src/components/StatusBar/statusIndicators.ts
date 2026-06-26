import type { TlsConfig } from "../../api/config";

export interface TlsInterceptionIndicator {
  active: boolean;
  text: string;
  detail: string;
  state: "unknown" | "limited" | "full";
}

export function getTlsInterceptionIndicator(
  config: TlsConfig | null,
): TlsInterceptionIndicator {
  if (!config) {
    return {
      active: false,
      text: "Unknown",
      detail: "TLS interception status has not loaded yet",
      state: "unknown",
    };
  }

  if (config.enable_tls_interception) {
    return {
      active: true,
      text: "Full On",
      detail: "HTTPS interception is globally enabled for all TLS traffic",
      state: "full",
    };
  }

  const scopedCount =
    config.intercept_include.length +
    config.app_intercept_include.length +
    (config.ip_intercept_include || []).length;

  if (scopedCount > 0) {
    return {
      active: false,
      text: "Scoped",
      detail: `${scopedCount} domain, app, or IP rule${scopedCount === 1 ? "" : "s"} can still enable HTTPS interception`,
      state: "limited",
    };
  }

  return {
    active: false,
    text: "Off",
    detail: "Global HTTPS interception is disabled",
    state: "limited",
  };
}
