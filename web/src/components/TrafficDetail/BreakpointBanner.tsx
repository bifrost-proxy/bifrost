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

const { Text } = Typography;

export function BreakpointBanner({ paused }: { paused: PausedBreakpoint }) {
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
            throw new Error(`Body exceeds ${paused.maxBodyBytes} bytes`);
        } catch (error) {
          message.error(
            error instanceof Error ? error.message : "Invalid body",
          );
          return;
        }
        const invalidHeader = paused.headers.find(
          ([name, value]) =>
            !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name) ||
            /[\r\n]/.test(value),
        );
        if (invalidHeader) {
          message.error(`Invalid header: ${invalidHeader[0] || "empty name"}`);
          return;
        }
        if (paused.phase === "request") {
          if (
            !paused.method ||
            !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(paused.method)
          ) {
            message.error("Enter a valid HTTP method");
            return;
          }
          try {
            const url = new URL(paused.url ?? "");
            if (url.protocol !== "http:" && url.protocol !== "https:")
              throw new Error();
          } catch {
            message.error("Enter an absolute HTTP(S) URL");
            return;
          }
        } else if (!isFinalBreakpointStatus(paused.status)) {
          message.error("Enter a final response status between 200 and 599");
          return;
        }
      }
      setSubmitting(true);
      const ok = await resume(paused.requestId, paused.phase, applyEdits);
      setSubmitting(false);
      if (ok) {
        message.success(
          applyEdits
            ? "Breakpoint edits applied"
            : "Breakpoint resumed unchanged",
        );
      } else {
        message.error(
          useBreakpointStore.getState().resumeError ??
            "Breakpoint is no longer pending; state has been refreshed",
        );
      }
    },
    [paused, resume],
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
        Breakpoint · {paused.phase === "request" ? "Request" : "Response"}
      </Tag>
      <Text
        type="secondary"
        style={{ fontSize: 12 }}
        data-testid="breakpoint-countdown"
      >
        Auto-resume in {remainingLabel}
      </Text>
      {paused.contentEncoding ? (
        <Tag style={{ margin: 0 }} data-testid="breakpoint-content-encoding">
          {paused.bodyRepresentation === "raw" ? "raw" : "decoded"}{" "}
          {paused.contentEncoding}
        </Tag>
      ) : null}
      {paused.phase === "response" &&
      (paused.method === "HEAD" ||
        (paused.status !== undefined &&
          (paused.status === 204 || paused.status === 304))) ? (
        <Text type="warning" data-testid="breakpoint-bodyless-warning">
          HTTP bodyless semantics: body edits are discarded for HEAD, 204 and
          304.
        </Text>
      ) : null}
      {paused.bodyOmitted ? (
        <Text
          type="warning"
          style={{ fontSize: 12 }}
          data-testid="breakpoint-body-omitted"
        >
          Body is unavailable for editing (safe size limit, streaming, or
          bodyless response); metadata and headers remain editable.
        </Text>
      ) : null}
      {paused.phase === "request" ? (
        <>
          <Input
            size="small"
            value={paused.method}
            aria-label="Breakpoint request method"
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
            aria-label="Breakpoint request URL"
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
          aria-label="Breakpoint response status"
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
          aria-label="Breakpoint body encoding"
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
              message.error(
                "These bytes are not valid UTF-8; keep Base64 to edit losslessly",
              );
            }
          }}
        />
      ) : null}
      <Text type="secondary" style={{ fontSize: 12 }}>
        Body limit: {paused.maxBodyBytes.toLocaleString()} bytes
        {paused.bodyRepresentation === "raw" ? " · raw bytes" : ""}
      </Text>
      <Space.Compact>
        <Button
          size="small"
          icon={<CaretRightOutlined />}
          disabled={submitting}
          data-testid="breakpoint-resume-unchanged"
          onClick={() => void handleResume(false)}
        >
          Resume unchanged
        </Button>
        <Button
          size="small"
          type="primary"
          icon={<CheckOutlined />}
          loading={submitting}
          data-testid="breakpoint-apply-resume"
          onClick={() => void handleResume(true)}
        >
          Apply & Resume
        </Button>
      </Space.Compact>
    </div>
  );
}
