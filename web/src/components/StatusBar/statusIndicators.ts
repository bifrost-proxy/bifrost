import type { TlsConfig } from "../../api/config";
import { t as globalT, type TFunction } from "../../i18n";

export interface TlsInterceptionIndicator {
  active: boolean;
  text: string;
  detail: string;
  state: "unknown" | "limited" | "full";
}

export function getTlsInterceptionIndicator(
  config: TlsConfig | null,
  t: TFunction = globalT,
): TlsInterceptionIndicator {
  if (!config) {
    return {
      active: false,
      text: t("common.unknown"),
      detail: t("app.statusBar.tlsNotLoaded"),
      state: "unknown",
    };
  }

  if (config.enable_tls_interception) {
    return {
      active: true,
      text: t("app.statusBar.tlsFullOn"),
      detail: t("app.statusBar.tlsFullOnDetail"),
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
      text: t("app.statusBar.tlsScoped"),
      detail: t(
        scopedCount === 1
          ? "app.statusBar.tlsScopedDetailOne"
          : "app.statusBar.tlsScopedDetailOther",
        { count: scopedCount },
      ),
      state: "limited",
    };
  }

  return {
    active: false,
    text: t("common.off"),
    detail: t("app.statusBar.tlsOffDetail"),
    state: "limited",
  };
}
