import { useMemo, useCallback, useEffect, type CSSProperties } from "react";
import { Tag, Typography, Empty, Spin, theme } from "antd";
import { CloseCircleOutlined } from "@ant-design/icons";
import {
  useReplayStore,
  type ResponsePanelTab,
} from "../../../stores/useReplayStore";
import type {
  SessionTargetSearchState,
  DisplayFormat,
  RecordContentType,
} from "../../../types";
import { Panel } from "../../../components/TrafficDetail/Panel";
import { HeaderView } from "../../../components/TrafficDetail/panes/Header";
import { Body } from "../../../components/TrafficDetail/panes/Body";
import { CookieView } from "../../../components/TrafficDetail/panes/Cookie";
import {
  getContentTypeFromHeader,
  isImageContentType,
} from "../../../components/TrafficDetail/helper/contentType";
import MessagesPanel from "./MessagesPanel";
import { getResponseBodyContentUrl } from "../../../api/traffic";
import { formatDurationDetailed } from "../../../utils/duration";
import { useI18n } from "../../../i18n";

const { Text } = Typography;

const staticStyles: Record<string, CSSProperties> = {
  emptyState: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: "100%",
    minHeight: 150,
  },
  errorState: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: 12,
    height: "100%",
    minHeight: 150,
    padding: 40,
  },
  emptyBody: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: "100%",
    minHeight: 100,
  },
};

function getStatusText(status: number): string {
  const statusTexts: Record<number, string> = {
    200: "OK",
    201: "Created",
    204: "No Content",
    301: "Moved Permanently",
    302: "Found",
    304: "Not Modified",
    400: "Bad Request",
    401: "Unauthorized",
    403: "Forbidden",
    404: "Not Found",
    405: "Method Not Allowed",
    500: "Internal Server Error",
    502: "Bad Gateway",
    503: "Service Unavailable",
  };
  return statusTexts[status] || "";
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function getHeaderValue(
  headers: [string, string][] | null | undefined,
  name: string
): string | null {
  if (!headers) return null;
  const entry = headers.find(([key]) => key.toLowerCase() === name.toLowerCase());
  return entry?.[1] ?? null;
}

const hasSetCookies = (headers: [string, string][] | null): boolean => {
  if (!headers) return false;
  return headers.some(([name]) => name.toLowerCase() === "set-cookie");
};

const initialSearchState: SessionTargetSearchState = {
  value: "",
  total: 0,
  show: false,
};

interface MatchedRule {
  protocol: string;
  rule_name?: string;
  pattern: string;
  value: string;
  line?: number;
  raw?: string;
}

interface RuleCardProps {
  rule: MatchedRule;
  index: number;
}

function RuleCard({ rule, index }: RuleCardProps) {
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
          {t("replay.response.rule.protocol")}{" "}
        </Text>
        <Tag color="green" style={{ fontSize: 11 }}>
          {rule.protocol}
        </Tag>
      </div>
      <div style={{ marginBottom: 1 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("replay.response.rule.pattern")}{" "}
        </Text>
        <Text code style={{ fontSize: 11 }}>
          {rule.pattern}
        </Text>
      </div>
      <div style={{ marginBottom: 1 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("replay.response.rule.value")}{" "}
        </Text>
        <Text code style={{ fontSize: 11 }}>
          {rule.value || t("replay.response.rule.empty")}
        </Text>
      </div>
      {rule.raw && (
        <div>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {t("replay.response.rule.raw")}
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
}

interface MatchedRulesPaneProps {
  rules: MatchedRule[];
}

function MatchedRulesPane({ rules }: MatchedRulesPaneProps) {
  const { t } = useI18n();
  if (rules.length === 0) {
    return (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          height: "100%",
          minHeight: 100,
        }}
      >
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("replay.response.noRulesApplied")}
        </Text>
      </div>
    );
  }

  return (
    <div style={{ padding: 8 }}>
      {rules.map((rule, index) => (
        <RuleCard key={index} rule={rule} index={index} />
      ))}
    </div>
  );
}

export default function ResponsePanel() {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const {
    currentResponse,
    currentTrafficRecord,
    streamingConnection,
    executing,
    sseEvents,
    wsMessages,
    uiState,
    updateUIState,
  } = useReplayStore();

  const activeTab = uiState.responsePanelActiveTab;
  const searchState = uiState.responsePanelSearch || initialSearchState;
  const displayFormat = uiState.responsePanelDisplayFormat || "HighLight";

  const hasStreamingContent =
    streamingConnection || sseEvents.length > 0 || wsMessages.length > 0;

  const setActiveTab = useCallback(
    (tab: string) => {
      updateUIState({ responsePanelActiveTab: tab as ResponsePanelTab });
    },
    [updateUIState],
  );

  const setSearchState = useCallback(
    (v: Partial<SessionTargetSearchState>) => {
      const currentSearch =
        useReplayStore.getState().uiState.responsePanelSearch ||
        initialSearchState;
      updateUIState({
        responsePanelSearch: { ...currentSearch, ...v },
      });
    },
    [updateUIState],
  );

  const setDisplayFormat = useCallback(
    (format: string) => {
      updateUIState({
        responsePanelDisplayFormat: format as DisplayFormat,
      });
    },
    [updateUIState],
  );

  const status = currentResponse?.status || currentTrafficRecord?.status || 0;
  const duration =
    currentResponse?.duration_ms || currentTrafficRecord?.duration_ms || 0;
  const error = currentResponse?.error;

  const statusColor = useMemo(() => {
    if (!status) return "default";
    if (status >= 500) return "red";
    if (status >= 400) return "orange";
    if (status >= 300) return "blue";
    if (status >= 200) return "green";
    return "default";
  }, [status]);

  const responseHeaders = useMemo<[string, string][]>(() => {
    return (
      currentResponse?.headers || currentTrafficRecord?.response_headers || []
    );
  }, [currentResponse, currentTrafficRecord]);

  const responseBody = useMemo(() => {
    return currentResponse?.body || currentTrafficRecord?.response_body || null;
  }, [currentResponse, currentTrafficRecord]);

  const appliedRules = useMemo(() => {
    return (
      currentResponse?.applied_rules ||
      currentTrafficRecord?.matched_rules ||
      []
    );
  }, [currentResponse?.applied_rules, currentTrafficRecord?.matched_rules]);

  const isEmpty = !currentResponse && !currentTrafficRecord && !hasStreamingContent;
  const isExecutingPending =
    executing && !currentResponse && !currentTrafficRecord && !hasStreamingContent;

  const responseRawContentType = useMemo(() => {
    return (
      currentTrafficRecord?.content_type ||
      getHeaderValue(currentResponse?.headers, "content-type")
    );
  }, [currentResponse?.headers, currentTrafficRecord?.content_type]);

  const responseContentType = useMemo<RecordContentType>(() => {
    return getContentTypeFromHeader(responseRawContentType);
  }, [responseRawContentType]);

  const responseMediaSrc = useMemo(() => {
    const trafficId = currentTrafficRecord?.id || currentResponse?.traffic_id;
    return trafficId ? getResponseBodyContentUrl(trafficId) : null;
  }, [currentResponse?.traffic_id, currentTrafficRecord?.id]);

  const canPreviewImage = useMemo(() => {
    return (
      responseContentType === "Media" &&
      isImageContentType(responseRawContentType) &&
      !!responseMediaSrc
    );
  }, [responseContentType, responseMediaSrc, responseRawContentType]);

  useEffect(() => {
    const shouldUseMedia =
      responseContentType === "Media" &&
      isImageContentType(responseRawContentType);
    if (displayFormat !== (shouldUseMedia ? "Media" : "HighLight")) {
      updateUIState({
        responsePanelDisplayFormat: shouldUseMedia ? "Media" : "HighLight",
      });
    }
  }, [
    currentTrafficRecord?.id,
    currentResponse?.traffic_id,
    displayFormat,
    responseContentType,
    responseRawContentType,
    updateUIState,
  ]);

  const tabs = useMemo(() => {
    if (isExecutingPending) {
      return [
        {
          key: "Body",
          label: t("replay.response.tab.body"),
          children: (
            <div style={staticStyles.emptyState} data-testid="replay-response-executing">
              <Spin tip={t("replay.response.executing")} />
            </div>
          ),
        },
      ];
    }

    if (isEmpty) {
      return [
        {
          key: "Body",
          label: t("replay.response.tab.body"),
          children: (
            <div style={staticStyles.emptyState}>
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={t("replay.response.empty")}
              />
            </div>
          ),
        },
      ];
    }

    if (error) {
      return [
        {
          key: "Body",
          label: t("replay.response.tab.body"),
          children: (
            <div style={staticStyles.errorState}>
              <CloseCircleOutlined
                style={{ fontSize: 32, color: token.colorError }}
              />
              <Text
                type="danger"
                style={{ fontSize: 13, textAlign: "center", maxWidth: 400 }}
              >
                {error}
              </Text>
            </div>
          ),
        },
      ];
    }

    return [
      {
        key: "Body",
        label: t("replay.response.tab.body"),
        enable: !!responseBody || canPreviewImage,
        children: responseBody || canPreviewImage ? (
          <Body
            data={responseBody}
            contentType={responseContentType}
            rawContentType={responseRawContentType}
            mediaSrc={responseMediaSrc}
            searchValue={searchState}
            displayFormat={displayFormat}
            onSearch={setSearchState}
          />
        ) : (
          <div style={staticStyles.emptyBody}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("replay.response.noBody")}
            </Text>
          </div>
        ),
      },
      {
        key: "Header",
        label: t("replay.response.tab.header"),
        children: (
          <HeaderView
            headers={responseHeaders}
            searchValue={searchState}
            onSearch={setSearchState}
          />
        ),
      },
      {
        key: "Set-Cookie",
        label: "Set-Cookie",
        enable: hasSetCookies(responseHeaders),
        children: (
          <CookieView
            headers={responseHeaders}
            type="response"
            searchValue={searchState}
            onSearch={setSearchState}
          />
        ),
      },
      {
        key: "Matched Rules",
        label: t("replay.response.tab.matchedRules", { count: appliedRules.length }),
        enable: appliedRules.length > 0,
        children: <MatchedRulesPane rules={appliedRules} />,
      },
      ...(hasStreamingContent
        ? [
            {
              key: "Messages",
              label: t("replay.response.tab.messages", {
                count: sseEvents.length + wsMessages.length,
              }),
              children: <MessagesPanel />,
            },
          ]
        : []),
    ];
  }, [
    isExecutingPending,
    isEmpty,
    error,
    token.colorError,
    responseBody,
    canPreviewImage,
    responseContentType,
    responseMediaSrc,
    responseRawContentType,
    searchState,
    displayFormat,
    setSearchState,
    responseHeaders,
    appliedRules,
    hasStreamingContent,
    sseEvents.length,
    wsMessages.length,
    t,
  ]);

  useEffect(() => {
    if (isExecutingPending || isEmpty || error) {
      if (activeTab !== "Body") {
        setActiveTab("Body");
      }
      return;
    }

    const enabledTabs = tabs.filter((tab) => tab.enable !== false);
    const currentTabEnabled = enabledTabs.some((tab) => tab.key === activeTab);

    if (!currentTabEnabled && enabledTabs.length > 0) {
      setActiveTab(enabledTabs[0].key);
    }
  }, [isExecutingPending, isEmpty, error, tabs, activeTab, setActiveTab]);

  const styles: Record<string, CSSProperties> = {
    container: {
      display: "flex",
      flexDirection: "column",
      height: "100%",
      overflow: "hidden",
    },
    statusBar: {
      display: "flex",
      alignItems: "center",
      gap: 12,
    },
    statusItem: {
      display: "flex",
      alignItems: "center",
      gap: 4,
    },
    statusLabel: {
      fontSize: 12,
      color: token.colorTextSecondary,
    },
    statusValue: {
      fontSize: 12,
      fontWeight: 500,
      color: token.colorText,
    },
    content: {
      flex: 1,
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
      padding: "0 8px",
      minHeight: 0,
    },
  };

  const tabBarExtra =
    !isEmpty && !error && status > 0 ? (
      <div style={styles.statusBar}>
        <div style={styles.statusItem}>
          <span style={styles.statusLabel}>{t("replay.response.status")}</span>
          <Tag color={statusColor} style={{ margin: 0, fontSize: 12 }}>
            {status} {getStatusText(status)}
          </Tag>
        </div>
        <div style={styles.statusItem}>
          <span style={styles.statusLabel}>{t("replay.response.time")}</span>
          <span style={styles.statusValue}>{formatDurationDetailed(duration)}</span>
        </div>
        {responseBody && (
          <div style={styles.statusItem}>
            <span style={styles.statusLabel}>{t("replay.response.size")}</span>
            <span style={styles.statusValue}>{formatSize(responseBody.length)}</span>
          </div>
        )}
      </div>
    ) : null;

  return (
    <div style={styles.container}>
      <div style={styles.content}>
        <Panel
          name="Response"
          tabs={tabs}
          activeTab={activeTab}
          onTabChange={setActiveTab}
          searchValue={searchState}
          onSearch={setSearchState}
          tabBarExtra={tabBarExtra}
          displayFormat={displayFormat}
          onDisplayFormatChange={setDisplayFormat}
          contentType={responseContentType}
        />
      </div>
    </div>
  );
}
