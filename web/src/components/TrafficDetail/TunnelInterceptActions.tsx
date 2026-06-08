import { useCallback, useEffect, useMemo } from "react";
import { AppstoreOutlined, LockOutlined, SafetyCertificateOutlined } from "@ant-design/icons";
import { Button, message, Modal, Typography, theme } from "antd";
import { useTlsConfigStore } from "../../stores/useTlsConfigStore";
import { useWhitelistStore } from "../../stores/useWhitelistStore";
import {
  showTlsWhitelistChangeSuccess,
  TLS_RECONNECT_NOTICE,
} from "../../utils/tlsInterceptionNotice";
import { isClientIpWhitelisted, isLoopbackClientIp } from "./clientIp";

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
  emptyText = "No response data for tunnel connection",
}: TunnelInterceptActionsProps) {
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
      message.error("No host found for this request");
      return;
    }

    Modal.confirm({
      title: "Add Domain to Intercept List",
      content: `Add "${host}" to TLS intercept list? This will enable HTTPS inspection for this domain and disconnect existing tunnel connections.`,
      okText: "Add",
      cancelText: "Cancel",
      onOk: async () => {
        const success = await addDomainToIntercept(host);
        if (success) {
          showTlsWhitelistChangeSuccess(`Added "${host}" to intercept list`);
        } else {
          message.error("Failed to add domain to intercept list");
        }
      },
    });
  }, [addDomainToIntercept, host]);

  const handleAddApp = useCallback(() => {
    if (!clientApp) {
      message.error("No app found for this request");
      return;
    }

    Modal.confirm({
      title: "Add App to Intercept List",
      content: `Add "${clientApp}" to app intercept list? This will enable HTTPS inspection for this app.`,
      okText: "Add",
      cancelText: "Cancel",
      onOk: async () => {
        const success = await addAppToIntercept(clientApp);
        if (success) {
          showTlsWhitelistChangeSuccess(`Added "${clientApp}" to app intercept list`);
        } else {
          message.error("Failed to add app to intercept list");
        }
      },
    });
  }, [addAppToIntercept, clientApp]);

  const handleAddIp = useCallback(() => {
    if (!clientIp) {
      message.error("No client IP found for this request");
      return;
    }

    Modal.confirm({
      title: "Add Client IP to Intercept List",
      content: `Add "${clientIp}" to IP TLS intercept list? This will enable HTTPS inspection for traffic from this client IP.`,
      okText: "Add",
      cancelText: "Cancel",
      onOk: async () => {
        const success = await addIpToIntercept(clientIp);
        if (success) {
          showTlsWhitelistChangeSuccess(`Added "${clientIp}" to IP intercept list`);
        } else {
          message.error("Failed to add client IP to intercept list");
        }
      },
    });
  }, [addIpToIntercept, clientIp]);

  const handleAddClientWhitelist = useCallback(() => {
    if (!clientIp) {
      message.error("No client IP found for this request");
      return;
    }

    Modal.confirm({
      title: "Add Client to Whitelist",
      content: `Add "${clientIp}" to the proxy access whitelist? This allows this non-local device to use Bifrost when access control requires whitelisted clients.`,
      okText: "Add",
      cancelText: "Cancel",
      onOk: async () => {
        const success = await addToWhitelist(clientIp);
        if (success) {
          message.success(`Added "${clientIp}" to client whitelist`);
        } else {
          message.error("Failed to add client to whitelist");
        }
      },
    });
  }, [addToWhitelist, clientIp]);

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
                Intercept this domain
              </Button>
            )}
            {showAppButton && (
              <Button
                type="primary"
                icon={<AppstoreOutlined />}
                onClick={handleAddApp}
                size="large"
              >
                Intercept this app
              </Button>
            )}
            {showIpInterceptButton && (
              <Button
                type="primary"
                icon={<LockOutlined />}
                onClick={handleAddIp}
                size="large"
              >
                Intercept this client
              </Button>
            )}
            {showClientWhitelistButton && (
              <Button
                icon={<SafetyCertificateOutlined />}
                onClick={handleAddClientWhitelist}
                size="large"
              >
                Allow this client
              </Button>
            )}
          </div>
          {hasTlsButton && (
            <Text
              type="secondary"
              style={{ maxWidth: 560, textAlign: "center", padding: "0 16px" }}
            >
              {TLS_RECONNECT_NOTICE}
            </Text>
          )}
        </>
      ) : (
        <div style={{ color: token.colorTextSecondary }}>{emptyText}</div>
      )}
    </>
  );
}
