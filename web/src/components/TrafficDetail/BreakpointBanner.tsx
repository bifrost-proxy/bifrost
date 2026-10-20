import { useCallback, useMemo, useState } from "react";
import {
  Button,
  Select,
  Input,
  InputNumber,
  Space,
  Tag,
  Typography,
  message,
  theme,
} from "antd";
import { CaretRightOutlined, CheckOutlined } from "@ant-design/icons";
import type { PausedBreakpoint } from "../../stores/useBreakpointStore";
import { useBreakpointStore } from "../../stores/useBreakpointStore";
import {
  breakpointBodyBytes,
  convertBreakpointBody,
} from "../../utils/breakpointBody";
import { isFinalBreakpointStatus } from "../../utils/breakpointValidation";
import { useLiveNow } from "../../hooks/useLiveNow";
import { useI18n } from "../../i18n";

const { Text } = Typography;

export function BreakpointBanner({ paused }: { paused: PausedBreakpoint }) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const now = useLiveNow(true, 250);
  const updateMetadata = useBreakpointStore(
    (state) => state.updatePausedMetadata,
  );
  const updateBodyEncoding = useBreakpointStore(
    (state) => state.updateBodyEncoding,
  );
  const resume = useBreakpointStore((state) => state.resume);
  const [submitting, setSubmitting] = useState(false);
  const remainingMs = Math.max(0, paused.localDeadlineAtMs - now);
  const remainingLabel = useMemo(
    () => `${(remainingMs / 1000).toFixed(1)}s`,
    [remainingMs],
  );

  const handleResume = useCallback(
    async (applyEdits: boolean) => {
      if (applyEdits) {
        try {
          if (
            !paused.bodyOmitted &&
            breakpointBodyBytes(paused.body, paused.bodyEncoding).length >
              paused.maxBodyBytes
          )
            throw new Error(
              t("trafficDetail.breakpoint.bodyExceeds", {
                bytes: paused.maxBodyBytes,
              }),
            );
        } catch (error) {
          message.error(
            error instanceof Error
              ? error.message
              : t("trafficDetail.breakpoint.invalidBody"),
          );
          return;
        }
        const invalidHeader = paused.headers.find(
          ([name, value]) =>
            !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name) ||
            /[\r\n]/.test(value),
        );
        if (invalidHeader) {
          message.error(
            t("trafficDetail.breakpoint.invalidHeader", {
              name: invalidHeader[0] || t("trafficDetail.breakpoint.emptyName"),
            }),
          );
          return;
        }
        if (paused.phase === "request") {
          if (
            !paused.method ||
            !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(paused.method)
          ) {
            message.error(t("trafficDetail.breakpoint.invalidMethod"));
            return;
          }
          try {
            const url = new URL(paused.url ?? "");
            if (url.protocol !== "http:" && url.protocol !== "https:")
              throw new Error();
          } catch {
            message.error(t("trafficDetail.breakpoint.invalidUrl"));
            return;
          }
        } else if (!isFinalBreakpointStatus(paused.status)) {
          message.error(t("trafficDetail.breakpoint.invalidStatus"));
          return;
        }
      }
      setSubmitting(true);
      const ok = await resume(paused.requestId, paused.phase, applyEdits);
      setSubmitting(false);
      if (ok) {
        message.success(
          applyEdits
            ? t("trafficDetail.breakpoint.editsApplied")
            : t("trafficDetail.breakpoint.resumedUnchanged"),
        );
      } else {
        message.error(
          useBreakpointStore.getState().resumeError ??
            t("trafficDetail.breakpoint.noLongerPending"),
        );
      }
    },
    [paused, resume, t],
  );

  return (
    <div
      data-testid="breakpoint-editor-banner"
      data-phase={paused.phase}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "6px 8px",
        background: token.colorWarningBg,
        borderBottom: `1px solid ${token.colorWarningBorder}`,
        flexWrap: "wrap",
      }}
    >
      <Tag
        color="warning"
        style={{ margin: 0 }}
        data-testid="breakpoint-phase-badge"
      >
        {t("trafficDetail.breakpoint.badge", {
          phase:
            paused.phase === "request"
              ? t("trafficDetail.breakpoint.phaseRequest")
              : t("trafficDetail.breakpoint.phaseResponse"),
        })}
      </Tag>
      <Text
        type="secondary"
        style={{ fontSize: 12 }}
        data-testid="breakpoint-countdown"
      >
        {t("trafficDetail.breakpoint.autoResumeIn", { time: remainingLabel })}
      </Text>
      {paused.contentEncoding ? (
        <Tag style={{ margin: 0 }} data-testid="breakpoint-content-encoding">
          {paused.bodyRepresentation === "raw"
            ? t("trafficDetail.breakpoint.encodingRaw")
            : t("trafficDetail.breakpoint.encodingDecoded")}{" "}
          {paused.contentEncoding}
        </Tag>
      ) : null}
      {paused.phase === "response" &&
      (paused.method === "HEAD" ||
        (paused.status !== undefined &&
          (paused.status === 204 || paused.status === 304))) ? (
        <Text type="warning" data-testid="breakpoint-bodyless-warning">
          {t("trafficDetail.breakpoint.bodylessWarning")}
        </Text>
      ) : null}
      {paused.bodyOmitted ? (
        <Text
          type="warning"
          style={{ fontSize: 12 }}
          data-testid="breakpoint-body-omitted"
        >
          {t("trafficDetail.breakpoint.bodyOmitted")}
        </Text>
      ) : null}
      {paused.phase === "request" ? (
        <>
          <Input
            size="small"
            value={paused.method}
            aria-label={t("trafficDetail.breakpoint.ariaMethod")}
            data-testid="breakpoint-method-input"
            onChange={(event) =>
              updateMetadata(paused.requestId, paused.phase, {
                method: event.target.value,
              })
            }
            style={{ width: 84, fontFamily: "monospace" }}
          />
          <Input
            size="small"
            value={paused.url}
            aria-label={t("trafficDetail.breakpoint.ariaUrl")}
            data-testid="breakpoint-url-input"
            onChange={(event) =>
              updateMetadata(paused.requestId, paused.phase, {
                url: event.target.value,
              })
            }
            style={{ minWidth: 260, flex: 1, fontFamily: "monospace" }}
          />
        </>
      ) : (
        <InputNumber
          size="small"
          min={100}
          max={599}
          value={paused.status}
          aria-label={t("trafficDetail.breakpoint.ariaStatus")}
          data-testid="breakpoint-status-input"
          onChange={(value) =>
            updateMetadata(paused.requestId, paused.phase, {
              status: typeof value === "number" ? value : paused.originalStatus,
            })
          }
          style={{ width: 92 }}
        />
      )}
      {!paused.bodyOmitted ? (
        <Select
          size="small"
          aria-label={t("trafficDetail.breakpoint.ariaBodyEncoding")}
          data-testid="breakpoint-body-encoding"
          value={paused.bodyEncoding}
          options={[
            { label: "UTF-8", value: "utf8" },
            { label: "Base64", value: "base64" },
          ]}
          onChange={(encoding: "utf8" | "base64") => {
            try {
              updateBodyEncoding(
                paused.requestId,
                paused.phase,
                encoding,
                convertBreakpointBody(
                  paused.body,
                  paused.bodyEncoding,
                  encoding,
                ),
              );
            } catch {
              message.error(t("trafficDetail.breakpoint.notValidUtf8"));
            }
          }}
        />
      ) : null}
      <Text type="secondary" style={{ fontSize: 12 }}>
        {t("trafficDetail.breakpoint.bodyLimit", {
          bytes: paused.maxBodyBytes.toLocaleString(),
        })}
        {paused.bodyRepresentation === "raw"
          ? t("trafficDetail.breakpoint.rawBytesSuffix")
          : ""}
      </Text>
      <Space.Compact>
        <Button
          size="small"
          icon={<CaretRightOutlined />}
          disabled={submitting}
          data-testid="breakpoint-resume-unchanged"
          onClick={() => void handleResume(false)}
        >
          {t("trafficDetail.breakpoint.resumeUnchanged")}
        </Button>
        <Button
          size="small"
          type="primary"
          icon={<CheckOutlined />}
          loading={submitting}
          data-testid="breakpoint-apply-resume"
          onClick={() => void handleResume(true)}
        >
          {t("trafficDetail.breakpoint.applyResume")}
        </Button>
      </Space.Compact>
    </div>
  );
}
