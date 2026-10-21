import { useMemo, useRef } from "react";
import {
  Descriptions,
  Typography,
  theme,
  Tag,
  Collapse,
  ConfigProvider,
} from "antd";
import dayjs from "dayjs";
import type {
  TrafficRecord,
  SessionTargetSearchState,
  MatchedRule,
  RequestTiming,
  SocketStatus,
} from "../../../../types";
import {
  formatDurationDetailed,
  getEffectiveDurationMs,
  isLiveStreamingTraffic,
} from "../../../../utils/duration";
import { useLiveNow } from "../../../../hooks/useLiveNow";
import { useMarkSearch } from "../../hooks/useMarkSearch";
import AppIcon from "../../../AppIcon";
import { useI18n, type TFunction } from "../../../../i18n";

const { Text, Paragraph } = Typography;

interface OverviewProps {
  record: TrafficRecord;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
}

const STATUS_CODES: Record<number, string> = {
  100: "Continue",
  101: "Switching Protocols",
  200: "OK",
  201: "Created",
  202: "Accepted",
  204: "No Content",
  206: "Partial Content",
  301: "Moved Permanently",
  302: "Found",
  303: "See Other",
  304: "Not Modified",
  307: "Temporary Redirect",
  308: "Permanent Redirect",
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  405: "Method Not Allowed",
  408: "Request Timeout",
  409: "Conflict",
  413: "Payload Too Large",
  414: "URI Too Long",
  415: "Unsupported Media Type",
  429: "Too Many Requests",
  500: "Internal Server Error",
  501: "Not Implemented",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

const formatSize = (bytes: number) => {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
};

const shouldUseSocketSize = (record: TrafficRecord): boolean => {
  if (!(record.is_websocket || record.is_sse || record.is_tunnel)) {
    return false;
  }
  if (!record.socket_status) {
    return false;
  }
  const totalSocketBytes =
    record.socket_status.send_bytes + record.socket_status.receive_bytes;
  return record.socket_status.is_open || totalSocketBytes > 0;
};

const RuleCard = ({ rule, index }: { rule: MatchedRule; index: number }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const source = rule.rule_name
    ? `${rule.rule_name}${rule.line ? `:${rule.line}` : ""}`
    : t("common.unknown");

  return (
    <div
      style={{
        padding: 6,
        marginBottom: 4,
        backgroundColor: token.colorBgLayout,
        borderRadius: 4,
        border: `1px solid ${token.colorBorderSecondary}`,
        fontSize: 12,
      }}
    >
      <div style={{ marginBottom: 2 }}>
        <Tag color="blue" style={{ fontSize: 11 }}>
          #{index + 1}
        </Tag>
        <Text strong style={{ fontSize: 12 }}>
          {source}
        </Text>
      </div>
      <div style={{ marginBottom: 1 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("trafficDetail.overview.ruleProtocol")}{" "}
        </Text>
        <Tag color="green" style={{ fontSize: 11 }}>
          {rule.protocol}
        </Tag>
      </div>
      <div style={{ marginBottom: 1 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("trafficDetail.overview.rulePattern")}{" "}
        </Text>
        <Text code style={{ fontSize: 11 }}>
          {rule.pattern}
        </Text>
      </div>
      <div style={{ marginBottom: 1 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("trafficDetail.overview.ruleValue")}{" "}
        </Text>
        <Text code style={{ fontSize: 11 }}>
          {rule.value || t("trafficDetail.overview.ruleEmpty")}
        </Text>
      </div>
      {rule.raw && (
        <div>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {t("trafficDetail.overview.ruleRaw")}
          </Text>
          <pre
            style={{
              fontFamily: "monospace",
              fontSize: 11,
              padding: "2px 6px",
              borderRadius: 4,
              margin: "2px 0 0 0",
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              backgroundColor: token.colorBgContainer,
            }}
          >
            {rule.raw}
          </pre>
        </div>
      )}
    </div>
  );
};

const TimingBar = ({ timing }: { timing: RequestTiming }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const total = timing.total_ms || 1;
  const firstByteWaitMs =
    timing.first_byte_ms !== undefined
      ? Math.max(
          timing.first_byte_ms -
            (timing.dns_ms ?? 0) -
            (timing.connect_ms ?? 0) -
            (timing.tls_ms ?? 0) -
            (timing.send_ms ?? 0),
          0,
        )
      : timing.wait_ms;

  const phases = [
    { key: "dns", label: t("trafficDetail.overview.phaseDns"), value: timing.dns_ms, color: "#8884d8" },
    {
      key: "connect",
      label: t("trafficDetail.overview.phaseConnect"),
      value: timing.connect_ms,
      color: "#82ca9d",
    },
    { key: "tls", label: t("trafficDetail.overview.phaseTls"), value: timing.tls_ms, color: "#ffc658" },
    { key: "send", label: t("trafficDetail.overview.phaseSend"), value: timing.send_ms, color: "#ff7300" },
    {
      key: "wait",
      label: t("trafficDetail.overview.phaseWait"),
      value: firstByteWaitMs,
      color: "#00C49F",
    },
    {
      key: "receive",
      label: t("trafficDetail.overview.phaseReceive"),
      value: timing.receive_ms,
      color: "#0088FE",
    },
  ].filter((p) => p.value !== undefined && p.value > 0);

  const stageDetails = [
    { key: "dns", label: t("trafficDetail.overview.phaseDns"), value: timing.dns_ms },
    { key: "connect", label: t("trafficDetail.overview.phaseConnect"), value: timing.connect_ms },
    { key: "tls", label: t("trafficDetail.overview.phaseTls"), value: timing.tls_ms },
    { key: "send", label: t("trafficDetail.overview.phaseSend"), value: timing.send_ms },
    { key: "first-byte", label: t("trafficDetail.overview.phaseFirstByteToClient"), value: timing.first_byte_ms },
    { key: "wait-byte", label: t("trafficDetail.overview.phaseWait"), value: firstByteWaitMs },
    {
      key: "receive",
      label: t("trafficDetail.overview.phaseReceive"),
      value: timing.receive_ms,
    },
    { key: "total", label: t("trafficDetail.overview.phaseTotal"), value: timing.total_ms },
  ].filter((stage) => stage.value !== undefined && stage.value > 0);

  return (
    <div style={{ marginTop: 2, display: "flex", flexDirection: "column", gap: 8 }}>
      <div
        style={{
          display: "flex",
          height: 14,
          borderRadius: 4,
          overflow: "hidden",
          backgroundColor: token.colorBgLayout,
        }}
      >
        {phases.map((phase) => (
          <div
            key={phase.key}
            style={{
              width: `${((phase.value || 0) / total) * 100}%`,
              backgroundColor: phase.color,
              minWidth: phase.value ? 2 : 0,
            }}
            title={`${phase.label}: ${phase.value}ms`}
          />
        ))}
      </div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 2 }}>
        {phases.map((phase) => (
          <div
            key={phase.key}
            style={{ display: "flex", alignItems: "center", gap: 3 }}
          >
            <div
              style={{
                width: 8,
                height: 8,
                borderRadius: 2,
                backgroundColor: phase.color,
              }}
            />
            <Text type="secondary" style={{ fontSize: 11 }}>
              {phase.label}: {phase.value}ms
            </Text>
          </div>
        ))}
      </div>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "minmax(170px, 220px) 1fr",
          gap: "4px 12px",
        }}
      >
        {stageDetails.map((stage) => (
          <div
            key={stage.key}
            style={{
              display: "contents",
            }}
          >
            <Text type="secondary" style={{ fontSize: 12 }}>
              {stage.label}
            </Text>
            <Text style={{ fontSize: 12 }}>
              {formatDurationDetailed(stage.value)}
            </Text>
          </div>
        ))}
      </div>
    </div>
  );
};

const SocketStatusCard = ({
  status,
  isWebSocket,
}: {
  status: SocketStatus;
  isWebSocket: boolean;
}) => {
  const { t } = useI18n();
  const { token } = theme.useToken();

  return (
    <div
      className="compact-socket-status"
      style={{
        padding: 6,
        backgroundColor: token.colorBgLayout,
        borderRadius: 4,
        border: `1px solid ${token.colorBorderSecondary}`,
        fontSize: 12,
      }}
    >
      <div style={{ marginBottom: 2 }}>
        <Tag
          color={status.is_open ? "green" : "default"}
          style={{ fontSize: 11 }}
        >
          {status.is_open ? t("trafficDetail.overview.connected") : t("trafficDetail.overview.closed")}
        </Tag>
        <Tag color="blue" style={{ fontSize: 11 }}>
          {isWebSocket ? "WebSocket" : "SSE"}
        </Tag>
      </div>
      <ConfigProvider
        theme={{
          components: {
            Descriptions: {
              itemPaddingBottom: 0,
              padding: 2,
              paddingSM: 2,
              paddingXS: 2,
              fontSize: 12,
              titleMarginBottom: 4,
            },
          },
        }}
      >
        <Descriptions
          column={2}
          size="small"
          bordered
          labelStyle={{ fontSize: 12 }}
          contentStyle={{ fontSize: 12 }}
        >
          <Descriptions.Item label={t("trafficDetail.overview.sendCount")}>
            {status.send_count}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.receiveCount")}>
            {status.receive_count}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.sendBytes")}>
            {formatSize(status.send_bytes)}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.receiveBytes")}>
            {formatSize(status.receive_bytes)}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.frameCount")}>
            {status.frame_count}
          </Descriptions.Item>
          {status.close_code !== undefined && (
            <Descriptions.Item label={t("trafficDetail.overview.closeCode")}>
              {status.close_code}
            </Descriptions.Item>
          )}
          {status.close_reason && (
            <Descriptions.Item label={t("trafficDetail.overview.closeReason")} span={2}>
              {status.close_reason}
            </Descriptions.Item>
          )}
        </Descriptions>
      </ConfigProvider>
    </div>
  );
};

type TimelineSegment = {
  key: string;
  label: string;
  value: number;
  color: string;
};

type TimelineMoment = {
  key: string;
  label: string;
  timestamp: number;
  offsetMs: number;
  active?: boolean;
};

const buildTimelineData = (
  record: TrafficRecord,
  durationMs: number,
  t: TFunction,
): { segments: TimelineSegment[]; moments: TimelineMoment[] } => {
  const timing = record.timing;
  const isOpen = record.socket_status?.is_open === true;
  const transferLabel =
    record.is_sse || record.is_websocket || record.is_tunnel
      ? isOpen
        ? t("trafficDetail.overview.streaming")
        : t("trafficDetail.overview.streamingUntilClose")
      : t("trafficDetail.overview.receivingResponseBody");
  const firstByteMs = timing?.first_byte_ms;

  const stageCandidates: Array<{
    key: string;
    label: string;
    value?: number;
    color: string;
    momentLabel?: string;
  }> = [
    {
      key: "dns",
      label: t("trafficDetail.overview.phaseDns"),
      value: timing?.dns_ms,
      color: "#8884d8",
      momentLabel: t("trafficDetail.overview.momentDnsResolved"),
    },
    {
      key: "connect",
      label: t("trafficDetail.overview.phaseConnect"),
      value: timing?.connect_ms,
      color: "#82ca9d",
      momentLabel: t("trafficDetail.overview.connected"),
    },
    {
      key: "tls",
      label: t("trafficDetail.overview.phaseTls"),
      value: timing?.tls_ms,
      color: "#ffc658",
      momentLabel: t("trafficDetail.overview.momentTlsReady"),
    },
    {
      key: "send",
      label: t("trafficDetail.overview.phaseSend"),
      value: timing?.send_ms,
      color: "#ff7300",
      momentLabel: t("trafficDetail.overview.phaseSend"),
    },
    {
      key: "wait",
      label: t("trafficDetail.overview.phaseWait"),
      value:
        firstByteMs !== undefined
          ? Math.max(
              firstByteMs -
                (timing?.dns_ms ?? 0) -
                (timing?.connect_ms ?? 0) -
                (timing?.tls_ms ?? 0) -
                (timing?.send_ms ?? 0),
              0,
            )
          : timing?.wait_ms,
      color: "#00C49F",
      momentLabel: t("trafficDetail.overview.momentFirstByte"),
    },
  ];

  const segments: TimelineSegment[] = [];
  const moments: TimelineMoment[] = [
    {
      key: "started",
      label: t("trafficDetail.overview.momentStarted"),
      timestamp: record.timestamp,
      offsetMs: 0,
    },
  ];

  let cumulativeMs = 0;
  for (const stage of stageCandidates) {
    const value = stage.value ?? 0;
    if (value <= 0) {
      continue;
    }
    segments.push({
      key: stage.key,
      label: stage.label,
      value,
      color: stage.color,
    });
    cumulativeMs += value;
    if (stage.momentLabel) {
      moments.push({
        key: `${stage.key}-moment`,
        label: stage.momentLabel,
        timestamp: record.timestamp + cumulativeMs,
        offsetMs: cumulativeMs,
      });
    }
  }

  const transferStartMs = firstByteMs ?? cumulativeMs;
  const transferMs = Math.max(durationMs - transferStartMs, 0);
  if (transferMs > 0) {
    segments.push({
      key: "transfer",
      label: transferLabel,
      value: transferMs,
      color: "#1677ff",
    });
  }
  if (segments.length === 0 && durationMs > 0) {
    segments.push({
      key: "total",
      label: t("trafficDetail.overview.requestProcessing"),
      value: durationMs,
      color: "#722ed1",
    });
  }

  if (transferMs > 0) {
    moments.push({
      key: "transfer-start",
      label:
        record.is_sse || record.is_websocket || record.is_tunnel
          ? t("trafficDetail.overview.momentStreamActive")
          : t("trafficDetail.overview.momentReceivingBody"),
      timestamp: record.timestamp + transferStartMs,
      offsetMs: transferStartMs,
    });
  }
  moments.push({
    key: isOpen ? "current" : "closed",
    label: isOpen ? t("trafficDetail.overview.momentCurrent") : t("trafficDetail.overview.closed"),
    timestamp: record.timestamp + durationMs,
    offsetMs: durationMs,
    active: isOpen,
  });

  return { segments, moments };
};

const TimelineBreakdown = ({
  record,
  durationMs,
}: {
  record: TrafficRecord;
  durationMs: number;
}) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const { segments, moments } = useMemo(
    () => buildTimelineData(record, durationMs, t),
    [record, durationMs, t],
  );
  const total = Math.max(durationMs, 1);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
      <div
        style={{
          display: "flex",
          height: 12,
          borderRadius: 999,
          overflow: "hidden",
          backgroundColor: token.colorBgLayout,
        }}
      >
        {segments.map((segment) => (
          <div
            key={segment.key}
            style={{
              width: `${(segment.value / total) * 100}%`,
              backgroundColor: segment.color,
              minWidth: segment.value > 0 ? 4 : 0,
            }}
            title={`${segment.label}: ${formatDurationDetailed(segment.value)}`}
          />
        ))}
      </div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
        {segments.map((segment) => (
          <div
            key={segment.key}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              padding: "4px 8px",
              borderRadius: 999,
              backgroundColor: token.colorBgLayout,
            }}
          >
            <div
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                backgroundColor: segment.color,
              }}
            />
            <Text type="secondary" style={{ fontSize: 11 }}>
              {segment.label}: {formatDurationDetailed(segment.value)}
            </Text>
          </div>
        ))}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {moments.map((moment) => (
          <div
            key={moment.key}
            style={{
              display: "grid",
              gridTemplateColumns: "20px minmax(120px, 160px) 1fr",
              alignItems: "center",
              gap: 8,
            }}
          >
            <div
              style={{
                width: 10,
                height: 10,
                borderRadius: "50%",
                backgroundColor: moment.active ? token.colorPrimary : "#52c41a",
              }}
            />
            <Text strong style={{ fontSize: 12 }}>
              {moment.label}
            </Text>
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                gap: 12,
                flexWrap: "wrap",
              }}
            >
              <Text type="secondary" style={{ fontSize: 12 }}>
                {dayjs(moment.timestamp).format("YYYY-MM-DD HH:mm:ss.SSS")}
              </Text>
              <Text type="secondary" style={{ fontSize: 12 }}>
                +{formatDurationDetailed(moment.offsetMs)}
              </Text>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};

export const Overview = ({ record, searchValue, onSearch }: OverviewProps) => {
  const { t } = useI18n();
  const wrapperRef = useRef<HTMLDivElement>(null);
  const isLiveDuration = isLiveStreamingTraffic(record);
  const liveNow = useLiveNow(isLiveDuration);
  const durationMs = getEffectiveDurationMs(record, liveNow);

  useMarkSearch(searchValue, () => wrapperRef.current, onSearch);

  const collapseItems = useMemo(() => {
    const items = [];

    if (record.timing) {
      items.push({
        key: "timing",
        label: <Text strong>{t("trafficDetail.overview.timingDetails")}</Text>,
        children: <TimingBar timing={record.timing} />,
      });
    }

    if (
      (record.is_websocket || record.is_sse || record.is_tunnel) &&
      record.socket_status
    ) {
      const connectionType = record.is_websocket
        ? "WebSocket"
        : record.is_sse
          ? "SSE"
          : t("trafficDetail.overview.tunnel");
      items.push({
        key: "socket",
        label: (
          <Text strong>
            {t("trafficDetail.overview.connectionStatus", { type: connectionType })}
            <Tag
              color={record.socket_status.is_open ? "green" : "default"}
              style={{ marginLeft: 8 }}
            >
              {record.socket_status.is_open ? t("trafficDetail.overview.connected") : t("trafficDetail.overview.closed")}
            </Tag>
          </Text>
        ),
        children: (
          <SocketStatusCard
            status={record.socket_status}
            isWebSocket={record.is_websocket || false}
          />
        ),
      });
    }

    if (record.matched_rules && record.matched_rules.length > 0) {
      items.push({
        key: "rules",
        label: (
          <Text strong>
            {t("trafficDetail.overview.matchedRules")} <Tag color="blue">{record.matched_rules.length}</Tag>
          </Text>
        ),
        children: (
          <div>
            {record.matched_rules.map((rule, index) => (
              <RuleCard key={index} rule={rule} index={index} />
            ))}
          </div>
        ),
      });
    }

    return items;
  }, [record, t]);

  const connectionType = useMemo(() => {
    if (record.is_websocket) return "WebSocket";
    if (record.is_sse) return "SSE";
    if (record.is_tunnel) return t("trafficDetail.overview.tunnel");
    return null;
  }, [record.is_websocket, record.is_sse, record.is_tunnel, t]);

  const isH3 = record.is_h3 || record.protocol === "h3";
  const useSocketSize = shouldUseSocketSize(record);
  const socketStatus = record.socket_status ?? undefined;

  return (
    <div ref={wrapperRef} style={{ fontSize: 12 }}>
      <ConfigProvider
        theme={{
          components: {
            Descriptions: {
              itemPaddingBottom: 0,
              padding: 2,
              paddingSM: 2,
              paddingXS: 2,
              fontSize: 12,
              titleMarginBottom: 4,
            },
          },
        }}
      >
        <Descriptions
          column={1}
          size="small"
          bordered
          style={{ marginBottom: 4 }}
          labelStyle={{ width: 120, fontWeight: 500, fontSize: 12 }}
          contentStyle={{ fontSize: 12 }}
        >
          <Descriptions.Item label="URL">
            <Paragraph
              style={{ margin: 0, maxWidth: "100%" }}
              ellipsis={{ rows: 2, expandable: true }}
              copyable
            >
              {record.url}
            </Paragraph>
            {record.actual_url && (
              <div style={{ marginTop: 4 }}>
                <Tag color="orange" style={{ fontSize: 11, marginRight: 4 }}>
                  {t("trafficDetail.overview.actual")}
                </Tag>
                <Paragraph
                  style={{ margin: 0, maxWidth: "100%", display: "inline" }}
                  ellipsis={{ rows: 2, expandable: true }}
                  copyable
                >
                  {record.actual_url}
                </Paragraph>
              </div>
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.method")}>
            <Tag color="blue">{record.method}</Tag>
            {connectionType && (
              <Tag color="purple" style={{ marginLeft: 4 }}>
                {connectionType}
              </Tag>
            )}
            {isH3 && (
              <Tag color="purple" style={{ marginLeft: 4 }}>
                HTTP/3
              </Tag>
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.status")}>
            <Tag
              color={
                record.status >= 400
                  ? "red"
                  : record.status >= 300
                    ? "orange"
                    : "green"
              }
            >
              {record.status} {STATUS_CODES[record.status] || ""}
            </Tag>
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.protocol")}>
            {record.protocol}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.proxyPort")}>
            {record.listener_port ? (
              <Tag style={{ margin: 0 }}>{record.listener_port}</Tag>
            ) : (
              "-"
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.host")}>
            {record.host}
            {record.actual_host && (
              <span style={{ marginLeft: 8 }}>
                <Tag color="orange" style={{ fontSize: 11, marginRight: 4 }}>
                  {t("trafficDetail.overview.actual")}
                </Tag>
                {record.actual_host}
              </span>
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.path")}>{record.path}</Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.contentType")}>
            {record.content_type || "-"}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.client")}>
            {record.client_app ? (
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  flexWrap: "wrap",
                }}
              >
                <AppIcon appName={record.client_app} size={18} />
                <Tag color="cyan" style={{ margin: 0 }}>
                  {record.client_app}
                </Tag>
                {record.client_pid && (
                  <Text type="secondary">PID: {record.client_pid}</Text>
                )}
                {record.client_ip && (
                  <Text type="secondary">IP: {record.client_ip}</Text>
                )}
              </div>
            ) : (
              record.client_ip || "-"
            )}
          </Descriptions.Item>
        </Descriptions>

        <Descriptions
          title={t("trafficDetail.overview.size")}
          column={2}
          size="small"
          bordered
          style={{ marginBottom: 4 }}
          labelStyle={{ width: 120, fontWeight: 500, fontSize: 12 }}
          contentStyle={{ fontSize: 12 }}
        >
          <Descriptions.Item label={t("trafficDetail.overview.requestSize")}>
            {useSocketSize && socketStatus
              ? formatSize(socketStatus.send_bytes)
              : formatSize(record.request_size)}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.responseSize")}>
            {useSocketSize && socketStatus
              ? formatSize(socketStatus.receive_bytes)
              : formatSize(record.response_size)}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.totalSize")}>
            {useSocketSize && socketStatus
              ? formatSize(
                  socketStatus.send_bytes + socketStatus.receive_bytes,
                )
              : formatSize(record.request_size + record.response_size)}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.duration")}>
            {formatDurationDetailed(durationMs)}
          </Descriptions.Item>
        </Descriptions>

        <Descriptions
          title={t("trafficDetail.overview.timeline")}
          column={1}
          size="small"
          bordered
          style={{ marginBottom: 4 }}
          labelStyle={{ width: 120, fontWeight: 500, fontSize: 12 }}
          contentStyle={{ fontSize: 12 }}
        >
          <Descriptions.Item label={t("trafficDetail.overview.phases")}>
            <TimelineBreakdown record={record} durationMs={durationMs} />
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.startedAt")}>
            {dayjs(record.timestamp).format("YYYY-MM-DD HH:mm:ss.SSS")}
          </Descriptions.Item>
          <Descriptions.Item label={t("trafficDetail.overview.endedAt")}>
            {record.socket_status?.is_open
              ? t("trafficDetail.overview.inProgress")
              : dayjs(record.timestamp + durationMs).format(
                  "YYYY-MM-DD HH:mm:ss.SSS",
                )}
          </Descriptions.Item>
        </Descriptions>

        {collapseItems.length > 0 && (
          <Collapse
            items={collapseItems}
            defaultActiveKey={["timing", "socket", "rules"]}
            size="small"
            style={{ marginTop: 4, fontSize: 12 }}
          />
        )}
      </ConfigProvider>
    </div>
  );
};
