import { useCallback, useEffect, useMemo, useState } from "react";
import { Alert, Button, Empty, Spin, theme } from "antd";
import { ArrowLeftOutlined, ImportOutlined, ReloadOutlined } from "@ant-design/icons";
import { useNavigate, useSearchParams } from "react-router-dom";
import TrafficDetail from "../../components/TrafficDetail";
import {
  getTrafficDetail,
  getRequestBody,
  getResponseBody,
  getRequestBodyContent,
  getResponseBodyContent,
} from "../../api/traffic";
import { normalizeApiErrorMessage } from "../../api/client";
import type { TrafficRecord } from "../../types";
import type { TrafficBodyContent } from "../../api/traffic";
import { useTrafficStore } from "../../stores/useTrafficStore";
import { useTrafficDetailWindowStore } from "../../stores/useTrafficDetailWindowStore";
import { isDesktopShell } from "../../runtime";
import { closeDesktopTrafficDetailWindow } from "../../desktop/tauri";
import { useI18n } from "../../i18n";

export default function TrafficDetailPage() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const { token } = theme.useToken();
  const [record, setRecord] = useState<TrafficRecord | null>(null);
  const [requestBody, setRequestBody] = useState<string | null>(null);
  const [responseBody, setResponseBody] = useState<string | null>(null);
  const [requestRawBody, setRequestRawBody] = useState<TrafficBodyContent | null>(null);
  const [responseRawBody, setResponseRawBody] = useState<TrafficBodyContent | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const selectedId = useTrafficStore((state) => state.selectedId);
  const detailDetached = useTrafficDetailWindowStore((state) => state.detached);
  const attachDetailWindow = useTrafficDetailWindowStore((state) => state.attach);
  const detachedMode = searchParams.get("detached") === "1";
  const popupId = searchParams.get("popupId")?.trim() || null;

  const urlId = searchParams.get("id")?.trim() || "";
  const recordId = detachedMode
    ? selectedId?.trim() || urlId
    : urlId;

  const closeDetachedWindow = useCallback(() => {
    if (isDesktopShell()) {
      void closeDesktopTrafficDetailWindow().catch((closeError) => {
        console.error("Failed to close native traffic detail window", closeError);
      });
      return;
    }
    window.close();
  }, []);

  const handleAttachBack = useCallback(() => {
    attachDetailWindow();
    closeDetachedWindow();
  }, [attachDetailWindow, closeDetachedWindow]);

  useEffect(() => {
    if (!detachedMode || !recordId) {
      return;
    }

    const nextParams = new URLSearchParams(searchParams);
    if (nextParams.get("id") === recordId) {
      return;
    }
    nextParams.set("id", recordId);
    setSearchParams(nextParams, { replace: true });
  }, [detachedMode, recordId, searchParams, setSearchParams]);

  useEffect(() => {
    if (!detachedMode || detailDetached) {
      return;
    }
    closeDetachedWindow();
  }, [closeDetachedWindow, detachedMode, detailDetached]);

  useEffect(() => {
    if (!detachedMode || !popupId) {
      return;
    }

    const handleBeforeUnload = () => {
      const currentState = useTrafficDetailWindowStore.getState();
      if (currentState.detached && currentState.popupId === popupId) {
        currentState.attach();
      }
    };

    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
    };
  }, [detachedMode, popupId]);

  const fetchDetail = useCallback(async () => {
    if (!recordId) {
      setRecord(null);
      setRequestBody(null);
      setResponseBody(null);
      setRequestRawBody(null);
      setResponseRawBody(null);
      setError(t("trafficDetail.page.missingId"));
      return;
    }

    setLoading(true);
    setError(null);

    try {
      const [detail, nextRequestBody, nextResponseBody] = await Promise.all([
        getTrafficDetail(recordId),
        getRequestBody(recordId),
        getResponseBody(recordId),
      ]);
      const [nextRequestRawBody, nextResponseRawBody] = await Promise.all([
        detail.raw_request_body_ref
          ? getRequestBodyContent(recordId, { raw: true, encoding: "base64" }).catch(() => null)
          : Promise.resolve(null),
        detail.raw_response_body_ref
          ? getResponseBodyContent(recordId, { raw: true, encoding: "base64" }).catch(() => null)
          : Promise.resolve(null),
      ]);
      setRecord(detail);
      setRequestBody(nextRequestBody);
      setResponseBody(nextResponseBody);
      setRequestRawBody(nextRequestRawBody);
      setResponseRawBody(nextResponseRawBody);
    } catch (nextError) {
      setRecord(null);
      setRequestBody(null);
      setResponseBody(null);
      setRequestRawBody(null);
      setResponseRawBody(null);
      setError(normalizeApiErrorMessage(nextError, t("trafficDetail.page.loadFailed")));
    } finally {
      setLoading(false);
    }
  }, [recordId, t]);

  useEffect(() => {
    void fetchDetail();
  }, [fetchDetail]);

  const styles = useMemo(
    () => ({
      page: {
        height: "100vh",
        display: "flex",
        flexDirection: "column" as const,
        overflow: "hidden",
        backgroundColor: token.colorBgContainer,
      },
      toolbar: {
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 12,
        padding: "2px 12px",
        minHeight: 32,
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        background: token.colorBgContainer,
      },
      toolbarLeft: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        minWidth: 0,
        fontSize: 12,
      },
      detailWrapper: {
        flex: 1,
        minHeight: 0,
        padding: 4,
        backgroundColor: token.colorBgContainer,
        overflow: "hidden",
      },
      centered: {
        flex: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 24,
      },
    }),
    [token],
  );

  return (
    <div style={styles.page}>
      <div style={styles.toolbar}>
        <div style={styles.toolbarLeft}>
          {detachedMode ? (
            <Button
              size="small"
              icon={<ImportOutlined />}
              onClick={handleAttachBack}
              data-testid="traffic-detail-attach-back"
              style={{ fontSize: 12 }}
            >
              {t("trafficDetail.page.attachBack")}
            </Button>
          ) : (
            <Button
              size="small"
              icon={<ArrowLeftOutlined />}
              onClick={() => navigate("/traffic")}
              style={{ fontSize: 12 }}
            >
              {t("trafficDetail.page.backToNetwork")}
            </Button>
          )}
          <span style={{ color: token.colorTextSecondary, fontSize: 12 }}>
            {recordId ? t("trafficDetail.page.requestId", { id: recordId }) : t("trafficDetail.page.title")}
          </span>
        </div>
        <Button
          size="small"
          icon={<ReloadOutlined />}
          onClick={() => void fetchDetail()}
          loading={loading}
          disabled={!recordId}
          style={{ fontSize: 12 }}
        >
          {t("common.refresh")}
        </Button>
      </div>

      {!recordId ? (
        <div style={styles.centered}>
          <Empty
            description={
              detachedMode
                ? t("trafficDetail.page.selectInMainWindow")
                : t("trafficDetail.page.missingId")
            }
          />
        </div>
      ) : error && !record ? (
        <div style={styles.centered}>
          <Alert type="error" showIcon message={error} />
        </div>
      ) : loading && !record ? (
        <div style={styles.centered}>
          <Spin size="large" />
        </div>
      ) : (
        <div style={styles.detailWrapper}>
          <TrafficDetail
            record={record}
            requestBody={requestBody}
            responseBody={responseBody}
            requestRawBody={requestRawBody}
            responseRawBody={responseRawBody}
            loading={loading}
            error={error}
            onResponseBodyChange={(body) => {
              setResponseBody(body);
            }}
          />
        </div>
      )}
    </div>
  );
}
