import { message } from "antd";
import { t } from "../i18n";

/**
 * English copy kept for existing importers. Prefer `getTlsReconnectNotice()`
 * (or `t("shared.tls.reconnectNotice")`) so the text follows the UI language.
 */
export const TLS_RECONNECT_NOTICE =
  "Restart the target app and reopen the target domain to establish a new connection.";

export function getTlsReconnectNotice(): string {
  return t("shared.tls.reconnectNotice");
}

export function showTlsWhitelistChangeSuccess(content: string) {
  message.success({
    content: t("shared.tls.changeSuccess", {
      content,
      notice: getTlsReconnectNotice(),
    }),
    duration: 5,
  });
}
