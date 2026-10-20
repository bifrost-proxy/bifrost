import { useCallback, useEffect, useMemo } from "react";
import { AppstoreOutlined, LockOutlined, SafetyCertificateOutlined } from "@ant-design/icons";
import { Button, message, Modal, Typography, theme } from "antd";
import { useTlsConfigStore } from "../../stores/useTlsConfigStore";
import { useWhitelistStore } from "../../stores/useWhitelistStore";
import { showTlsWhitelistChangeSuccess } from "../../utils/tlsInterceptionNotice";
import { isClientIpWhitelisted, isLoopbackClientIp } from "./clientIp";
import { useI18n } from "../../i18n";

const { Text } = Typography;

interface TunnelInterceptActionsProps {
  isTunnel?: boolean;
  host?: string;
  clientApp?: string;
  clientIp?: string;
  emptyText?: string;
}

export function TunnelInterceptActions({
  isTunnel,
  host,
  clientApp,
  clientIp,
  emptyText,
}: TunnelInterceptActionsProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const {
    config: tlsConfig,
    fetchConfig: fetchTlsConfig,
    addDomainToIntercept,
    addAppToIntercept,
    addIpToIntercept,
    isDomainInIntercept,
    isAppInIntercept,
    isIpInIntercept,
  } = useTlsConfigStore();
  const {
    status: whitelistStatus,
    fetchStatus: fetchWhitelistStatus,
    addToWhitelist,
  } = useWhitelistStore();

  useEffect(() => {
    if (!isTunnel) return;
    if (!tlsConfig) {
      void fetchTlsConfig();
    }
    if (!whitelistStatus) {
      void fetchWhitelistStatus();
    }
  }, [fetchTlsConfig, fetchWhitelistStatus, isTunnel, tlsConfig, whitelistStatus]);

  const showDomainButton = !!host && !isDomainInIntercept(host);
  const showAppButton = !!clientApp && !isAppInIntercept(clientApp);
  const showIpInterceptButton = !!clientIp && !isIpInIntercept(clientIp);
  const showClientWhitelistButton = useMemo(() => {
    if (!clientIp || isLoopbackClientIp(clientIp)) return false;
    return !isClientIpWhitelisted(clientIp, whitelistStatus?.whitelist);
  }, [clientIp, whitelistStatus?.whitelist]);

  const handleAddDomain = useCallback(() => {
    if (!host) {
      message.error(t("trafficDetail.tunnel.noHost"));
      return;
    }

    Modal.confirm({
      title: t("trafficDetail.tunnel.addDomainTitle"),
      content: t("trafficDetail.tunnel.addDomainContent", { host }),
      okText: t("common.add"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        const success = await addDomainToIntercept(host);
        if (success) {
          showTlsWhitelistChangeSuccess(t("trafficDetail.tunnel.addDomainSuccess", { host }));
        } else {
          message.error(t("trafficDetail.tunnel.addDomainFailed"));
        }
      },
    });
  }, [addDomainToIntercept, host, t]);

  const handleAddApp = useCallback(() => {
    if (!clientApp) {
      message.error(t("trafficDetail.tunnel.noApp"));
      return;
    }

    Modal.confirm({
      title: t("trafficDetail.tunnel.addAppTitle"),
      content: t("trafficDetail.tunnel.addAppContent", { app: clientApp }),
      okText: t("common.add"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        const success = await addAppToIntercept(clientApp);
        if (success) {
          showTlsWhitelistChangeSuccess(t("trafficDetail.tunnel.addAppSuccess", { app: clientApp }));
        } else {
          message.error(t("trafficDetail.tunnel.addAppFailed"));
        }
      },
    });
  }, [addAppToIntercept, clientApp, t]);

  const handleAddIp = useCallback(() => {
    if (!clientIp) {
      message.error(t("trafficDetail.tunnel.noClientIp"));
      return;
    }

    Modal.confirm({
      title: t("trafficDetail.tunnel.addIpTitle"),
      content: t("trafficDetail.tunnel.addIpContent", { ip: clientIp }),
      okText: t("common.add"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        const success = await addIpToIntercept(clientIp);
        if (success) {
          showTlsWhitelistChangeSuccess(t("trafficDetail.tunnel.addIpSuccess", { ip: clientIp }));
        } else {
          message.error(t("trafficDetail.tunnel.addIpFailed"));
        }
      },
    });
  }, [addIpToIntercept, clientIp, t]);

  const handleAddClientWhitelist = useCallback(() => {
    if (!clientIp) {
      message.error(t("trafficDetail.tunnel.noClientIp"));
      return;
    }

    Modal.confirm({
      title: t("trafficDetail.tunnel.addWhitelistTitle"),
      content: t("trafficDetail.tunnel.addWhitelistContent", { ip: clientIp }),
      okText: t("common.add"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        const success = await addToWhitelist(clientIp);
        if (success) {
          message.success(t("trafficDetail.tunnel.addWhitelistSuccess", { ip: clientIp }));
        } else {
          message.error(t("trafficDetail.tunnel.addWhitelistFailed"));
        }
      },
    });
  }, [addToWhitelist, clientIp, t]);

  const hasTlsButton = showDomainButton || showAppButton || showIpInterceptButton;
  const hasAnyButton = hasTlsButton || showClientWhitelistButton;

  return (
    <>
      {hasAnyButton ? (
        <>
          <div style={{ display: "flex", gap: 12, flexWrap: "wrap", justifyContent: "center" }}>
            {showDomainButton && (
              <Button
                type="primary"
                icon={<LockOutlined />}
                onClick={handleAddDomain}
                size="large"
              >
                {t("trafficDetail.tunnel.interceptDomain")}
              </Button>
            )}
            {showAppButton && (
              <Button
                type="primary"
                icon={<AppstoreOutlined />}
                onClick={handleAddApp}
                size="large"
              >
                {t("trafficDetail.tunnel.interceptApp")}
              </Button>
            )}
            {showIpInterceptButton && (
              <Button
                type="primary"
                icon={<LockOutlined />}
                onClick={handleAddIp}
                size="large"
              >
                {t("trafficDetail.tunnel.interceptClient")}
              </Button>
            )}
            {showClientWhitelistButton && (
              <Button
                icon={<SafetyCertificateOutlined />}
                onClick={handleAddClientWhitelist}
                size="large"
              >
                {t("trafficDetail.tunnel.allowClient")}
              </Button>
            )}
          </div>
          {hasTlsButton && (
            <Text
              type="secondary"
              style={{ maxWidth: 560, textAlign: "center", padding: "0 16px" }}
            >
              {t("shared.tls.reconnectNotice")}
            </Text>
          )}
        </>
      ) : (
        <div style={{ color: token.colorTextSecondary }}>{emptyText ?? t("trafficDetail.tunnel.emptyText")}</div>
      )}
    </>
  );
}
