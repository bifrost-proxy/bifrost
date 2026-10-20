import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Alert, Card, Spin, Typography } from "antd";
import {
  getSyncStatus,
  saveSyncSession,
  type SyncStatus,
} from "../../api/sync";
import { t as translate, useI18n } from "../../i18n";

function getTokenFromHash(): string | null {
  const { hash } = window.location;
  const queryIndex = hash.indexOf("?");
  if (queryIndex === -1) {
    return null;
  }

  const hashQuery = hash.slice(queryIndex + 1);
  return new URLSearchParams(hashQuery).get("token");
}

function extractToken(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") {
    return null;
  }

  const directToken =
    "token" in payload && typeof payload.token === "string"
      ? payload.token
      : null;
  if (directToken) {
    return directToken;
  }

  const data =
    "data" in payload && payload.data && typeof payload.data === "object"
      ? payload.data
      : null;
  if (!data) {
    return null;
  }

  return "token" in data && typeof data.token === "string" ? data.token : null;
}

async function exchangeTokenFromRemote(remoteBaseUrl: string): Promise<string> {
  const normalizedBaseUrl = remoteBaseUrl.replace(/\/+$/, "");
  const response = await fetch(`${normalizedBaseUrl}/v4/sso/check`, {
    method: "GET",
    credentials: "include",
    headers: {
      Accept: "application/json",
    },
  });

  const payload = await response.json().catch(() => null);
  const token = extractToken(payload);
  if (!response.ok || !token) {
    throw new Error(translate("app.syncLogin.noToken"));
  }

  return token;
}

type SyncLoginPhase = "loading" | "success" | "error";

export default function SyncLogin() {
  const [searchParams] = useSearchParams();
  const { t } = useI18n();
  const token = useMemo(
    () => searchParams.get("token") || getTokenFromHash(),
    [searchParams],
  );
  const [phase, setPhase] = useState<SyncLoginPhase>("loading");
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    const completeLogin = async () => {
      try {
        const syncStatus = await getSyncStatus();
        const resolvedToken =
          token || (await exchangeTokenFromRemote(syncStatus.remote_base_url));
        const nextStatus = await saveSyncSession(resolvedToken);
        if (cancelled) {
          return;
        }

        setStatus(nextStatus);
        setPhase("success");
        window.opener?.postMessage(
          {
            type: "bifrost-sync-login-complete",
            status: nextStatus,
            redirect_to: "/",
          },
          window.location.origin,
        );
        window.setTimeout(() => window.location.replace("/"), 300);
      } catch (error) {
        if (cancelled) {
          return;
        }
        setPhase("error");
        setErrorMessage(
          error instanceof Error && error.message
            ? error.message
            : translate("app.syncLogin.retryHint"),
        );
      }
    };

    void completeLogin();

    return () => {
      cancelled = true;
    };
  }, [token]);

  return (
    <div
      style={{
        minHeight: "100vh",
        display: "grid",
        placeItems: "center",
        padding: 24,
        background: "linear-gradient(180deg, #f4f7fb 0%, #eef2f8 100%)",
      }}
    >
      <Card style={{ width: "100%", maxWidth: 460 }}>
        <Typography.Title level={3}>{t("app.syncLogin.title")}</Typography.Title>
        {phase === "loading" ? (
          <div style={{ display: "grid", placeItems: "center", padding: "12px 0" }}>
            <Spin size="large" />
          </div>
        ) : null}
        {phase === "success" ? (
          <Alert
            showIcon
            type="success"
            message={t("app.syncLogin.completed")}
            description={t("app.syncLogin.signedInAs", {
              user: status?.user?.user_id || t("app.syncLogin.remoteUser"),
            })}
          />
        ) : null}
        {phase === "error" ? (
          <Alert
            showIcon
            type="error"
            message={t("app.syncLogin.failed")}
            description={errorMessage || t("app.syncLogin.retryHint")}
          />
        ) : null}
      </Card>
    </div>
  );
}
