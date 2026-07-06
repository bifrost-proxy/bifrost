import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useShallow } from "zustand/react/shallow";
import { CheckOutlined, CopyOutlined } from "@ant-design/icons";
import { getActiveSummary, type ActiveRuleItem } from "../../api/rules";
import {
  getTemporaryPortActiveSummary,
  getTemporaryPorts,
  type TemporaryPortActiveSummary,
  type TemporaryPortBinding,
  type TemporaryPortRuleSetRef,
} from "../../api/ports";
import { useMetricsStore } from "../../stores/useMetricsStore";
import {
  isSystemProxyLiveEnabledByBifrost,
  useProxyStore,
} from "../../stores/useProxyStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import { copyToClipboard } from "../../utils/clipboard";
import styles from "./index.module.css";

interface ActivityStat {
  label: string;
  value: string;
  caption: string;
  color: string;
}

const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
};

const formatRate = (bytesPerSecond: number): string => `${formatBytes(bytesPerSecond)}/s`;

const countLines = (content: string): number => {
  if (!content.trim()) return 0;
  return content.split(/\r?\n/).length;
};

const ruleLabel = (rule: ActiveRuleItem): string =>
  rule.group_name ? `${rule.group_name} / ${rule.name}` : rule.name;

const formatRuleRef = (ref: TemporaryPortRuleSetRef): string => {
  switch (ref.type) {
    case "local_rule":
      return ref.name;
    case "group_rule":
      return `${ref.group_id}/${ref.name}`;
    case "rule_file":
      return ref.path;
    case "inline_rule":
      return ref.content.split(/\r?\n/).find((line) => line.trim()) || "inline rule";
    default:
      return "unknown";
  }
};

export default function Activity() {
  const navigate = useNavigate();
  const requestIdRef = useRef(0);
  const mergedCodeRef = useRef<HTMLPreElement>(null);
  const [activeRules, setActiveRules] = useState<ActiveRuleItem[]>([]);
  const [mergedContent, setMergedContent] = useState("");
  const [selectedRule, setSelectedRule] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [temporaryPorts, setTemporaryPorts] = useState<TemporaryPortBinding[]>([]);
  const [temporaryPortSummaries, setTemporaryPortSummaries] = useState<
    Record<number, TemporaryPortActiveSummary>
  >({});

  const metrics = useMetricsStore((state) => state.current);
  const overview = useMetricsStore((state) => state.overview);
  const fetchOverview = useMetricsStore((state) => state.fetchOverview);
  const systemProxy = useProxyStore((state) => state.systemProxy);
  const fetchSystemProxy = useProxyStore((state) => state.fetchSystemProxy);
  const traffic = useTrafficStore(
    useShallow((state) => ({
      records: state.records,
      serverTotal: state.serverTotal,
      availableClientApps: state.availableClientApps,
      clientAppCounts: state.clientAppCounts,
    })),
  );

  const refreshActiveSummary = useCallback(() => {
    const requestId = ++requestIdRef.current;
    getActiveSummary()
      .then((summary) => {
        if (requestId !== requestIdRef.current) return;
        setActiveRules(summary.rules ?? []);
        setMergedContent(summary.merged_content ?? "");
        setSelectedRule((current) => {
          if (current && summary.rules.some((rule) => ruleLabel(rule) === current)) {
            return current;
          }
          return summary.rules[0] ? ruleLabel(summary.rules[0]) : null;
        });
      })
      .catch(() => {
        if (requestId !== requestIdRef.current) return;
        setActiveRules([]);
        setMergedContent("");
        setSelectedRule(null);
      });
  }, []);

  const refreshTemporaryPorts = useCallback(() => {
    getTemporaryPorts()
      .then(async (ports) => {
        const settled = await Promise.allSettled(
          ports.map(
            async (port) =>
              [port.port, await getTemporaryPortActiveSummary(port.port)] as const,
          ),
        );
        const nextSummaries: Record<number, TemporaryPortActiveSummary> = {};
        for (const result of settled) {
          if (result.status === "fulfilled") {
            const [port, summary] = result.value;
            nextSummaries[port] = summary;
          }
        }
        setTemporaryPorts(ports);
        setTemporaryPortSummaries(nextSummaries);
      })
      .catch(() => {
        setTemporaryPorts([]);
        setTemporaryPortSummaries({});
      });
  }, []);

  useEffect(() => {
    refreshActiveSummary();
    const timer = window.setInterval(refreshActiveSummary, 5000);
    return () => window.clearInterval(timer);
  }, [refreshActiveSummary]);

  useEffect(() => {
    refreshTemporaryPorts();
    const timer = window.setInterval(refreshTemporaryPorts, 5000);
    return () => window.clearInterval(timer);
  }, [refreshTemporaryPorts]);

  useEffect(() => {
    void fetchOverview();
    void fetchSystemProxy();
  }, [fetchOverview, fetchSystemProxy]);

  const handleCopyMergedRules = useCallback(async () => {
    const selection = window.getSelection();
    const selectedText =
      selection &&
      mergedCodeRef.current &&
      selection.rangeCount > 0 &&
      mergedCodeRef.current.contains(selection.anchorNode) &&
      mergedCodeRef.current.contains(selection.focusNode)
        ? selection.toString()
        : "";
    const text = selectedText.trim() || mergedContent.trim();
    if (!text) return;
    const ok = await copyToClipboard(text);
    if (!ok) return;
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  }, [mergedContent]);

  const appDistribution = useMemo(() => {
    const entries = Array.from(traffic.clientAppCounts.entries())
      .map(([name, count]) => ({ name, count }))
      .sort((left, right) => right.count - left.count || left.name.localeCompare(right.name))
      .slice(0, 24);
    const max = Math.max(1, ...entries.map((entry) => entry.count));
    return entries.map((entry) => ({
      ...entry,
      percent: Math.max(1, (entry.count / max) * 100),
    }));
  }, [traffic.clientAppCounts]);

  const metricSnapshot = metrics ?? overview?.metrics ?? null;
  const liveConnections =
    metricSnapshot?.active_connections ??
    traffic.records.filter(
      (record) => record.socket_status?.is_open || record.status === 0,
    ).length;
  const totalRequests =
    metricSnapshot?.total_requests ??
    (traffic.serverTotal > 0 ? traffic.serverTotal : traffic.records.length);
  const totalUpload = metricSnapshot?.bytes_sent ?? traffic.records.reduce((sum, record) => sum + (record.upload_bytes ?? record.request_size), 0);
  const totalDownload = metricSnapshot?.bytes_received ?? traffic.records.reduce((sum, record) => sum + (record.download_bytes ?? record.response_size), 0);
  const rulesTotal = overview?.rules.total ?? activeRules.reduce((sum, rule) => sum + rule.rule_count, 0);
  const rulesEnabled = overview?.rules.enabled ?? activeRules.length;
  const serverPort = overview?.server.port ?? 9900;

  const systemProxyEnabled = systemProxy ? isSystemProxyLiveEnabledByBifrost(systemProxy) : false;
  const serviceState = systemProxyEnabled ? "Enabled" : "Disabled";
  const serviceCaption = `http://${systemProxy?.host || "127.0.0.1"}:${systemProxy?.port || serverPort}`;

  const stats: ActivityStat[] = [
    {
      label: "Active Connections",
      value: liveConnections.toLocaleString(),
      caption: `${traffic.availableClientApps.length.toLocaleString()} apps`,
      color: "#ff8a2a",
    },
    {
      label: "Upload",
      value: formatRate(metricSnapshot?.bytes_sent_rate ?? 0),
      caption: formatBytes(totalUpload),
      color: "#6256ff",
    },
    {
      label: "Download",
      value: formatRate(metricSnapshot?.bytes_received_rate ?? 0),
      caption: formatBytes(totalDownload),
      color: "#14b8d5",
    },
    {
      label: "Requests",
      value: totalRequests.toLocaleString(),
      caption: `${(metricSnapshot?.qps ?? 0).toFixed(2)} QPS`,
      color: "#32c766",
    },
    {
      label: "Rules",
      value: `${rulesEnabled}/${Math.max(rulesTotal, rulesEnabled)}`,
      caption: "Current rule set",
      color: "#cf3be9",
    },
    {
      label: "System Proxy",
      value: serviceState,
      caption: serviceCaption,
      color: systemProxyEnabled ? "#32c766" : "#168cff",
    },
  ];

  return (
    <main className={styles.activityPage} data-testid="activity-page">
      <div className={styles.activityShell}>
        <h1 className={styles.pageTitle}>Activity</h1>

        <section className={styles.statGrid} aria-label="Activity metrics">
          {stats.map((stat) => (
            <article className={styles.statCard} key={stat.label} data-testid="activity-stat-card">
              <div className={styles.statHeader}>
                <span>{stat.label}</span>
                <span className={styles.statDot} style={{ color: stat.color, backgroundColor: stat.color }} />
              </div>
              <div
                className={`${styles.statValue} ${
                  stat.value.length >= 9 ? styles.statValueCompact : ""
                }`}
                title={stat.value}
              >
                {stat.value}
              </div>
              <div className={styles.statCaption}>{stat.caption}</div>
            </article>
          ))}
        </section>

        <section className={`${styles.panel} ${styles.rulesPanel}`} data-testid="activity-rules-panel">
          <div className={styles.panelHeader}>
            <div className={styles.activeRulesColumn}>
              <h2 className={styles.panelTitle}>Active Rule Analysis</h2>
              <div className={styles.panelSubtitle}>Rule sets currently used by the proxy port</div>
            </div>
            <button
              type="button"
              className={styles.activeBadge}
              onClick={() => navigate("/rules")}
              data-testid="activity-rules-open-button"
            >
              <span className={styles.statDot} style={{ color: "#32c766", backgroundColor: "#32c766" }} />
              {activeRules.length.toLocaleString()} active
            </button>
          </div>

          <div className={styles.rulesLayout}>
            <div>
              <div className={styles.sectionLabel}>Active Rules</div>
              {activeRules.length > 0 ? (
                <div className={styles.ruleList}>
                  {activeRules.map((rule) => {
                    const label = ruleLabel(rule);
                    const selected = selectedRule === label;
                    return (
                      <button
                        type="button"
                        key={`${rule.group_id ?? "local"}:${rule.name}`}
                        className={`${styles.rulePill} ${selected ? styles.rulePillActive : ""}`}
                        onClick={() => setSelectedRule(label)}
                        onDoubleClick={() => {
                          const params = new URLSearchParams();
                          if (rule.group_id) params.set("group", rule.group_id);
                          params.set("rule", rule.name);
                          navigate({ pathname: "/rules", search: `?${params.toString()}` });
                        }}
                        data-testid="activity-rule-pill"
                      >
                        <div className={styles.rulePillName}>
                          <span className={styles.statDot} style={{ color: "#168cff", backgroundColor: "#168cff" }} />
                          <span>{rule.name}</span>
                        </div>
                        <div className={styles.rulePillMeta}>
                          {rule.rule_count.toLocaleString()} entries
                          {rule.group_name ? ` · ${rule.group_name}` : ""}
                        </div>
                      </button>
                    );
                  })}
                </div>
              ) : (
                <div className={styles.emptyState}>No active rules</div>
              )}
            </div>

            <div className={styles.mergedColumn}>
              <div className={styles.mergedHeader}>
                <div className={styles.sectionLabel}>Merged Rules</div>
                <div className={styles.mergedActions}>
                  <button
                    type="button"
                    className={styles.copyButton}
                    onClick={handleCopyMergedRules}
                    data-testid="activity-copy-merged-rules"
                    aria-label="Copy merged rules"
                    title="Copy selected text, or all merged rules when no text is selected"
                  >
                    {copied ? <CheckOutlined /> : <CopyOutlined />}
                  </button>
                  <div className={styles.lineCount}>{countLines(mergedContent).toLocaleString()} lines</div>
                </div>
              </div>
              <pre ref={mergedCodeRef} className={styles.mergedCode} data-testid="activity-merged-rules">
                {mergedContent.trim() || "# No active rules"}
              </pre>
            </div>
          </div>
        </section>

        {temporaryPorts.length > 0 ? (
          <section className={`${styles.panel} ${styles.temporaryPortsPanel}`} data-testid="activity-temporary-ports-panel">
            <div className={styles.distributionHeader}>
              <div>
                <h2 className={styles.panelTitle}>Temporary Ports</h2>
                <div className={styles.panelSubtitle}>Port-scoped listeners and their enabled rule details</div>
              </div>
              <div className={styles.distributionMode}>{temporaryPorts.length.toLocaleString()} active</div>
            </div>
            <div className={styles.temporaryPortGrid}>
              {temporaryPorts.map((port) => {
                const summary = temporaryPortSummaries[port.port];
                return (
                  <article
                    className={styles.temporaryPortCard}
                    key={port.port}
                    data-testid={`activity-temporary-port-card-${port.port}`}
                  >
                    <div className={styles.tempPortHeader}>
                      <div>
                        <div className={styles.tempPortAddress}>
                          {port.host}:{port.port}
                        </div>
                        {port.name ? (
                          <div className={styles.tempPortName}>{port.name}</div>
                        ) : null}
                      </div>
                      <span className={styles.tempPortStatus} data-status={port.status}>
                        {port.status}
                      </span>
                    </div>
                    <div className={styles.tempPortMetaGrid}>
                      <div>
                        <div className={styles.sectionLabel}>Bound Rules</div>
                        <div className={styles.ruleChipList}>
                          {port.rule_refs.length > 0 ? (
                            port.rule_refs.map((ref, index) => (
                              <span className={styles.ruleChip} key={`${ref.type}-${index}`} title={formatRuleRef(ref)}>
                                {formatRuleRef(ref)}
                              </span>
                            ))
                          ) : (
                            <span className={styles.mutedText}>No bound rule sets</span>
                          )}
                        </div>
                      </div>
                      <div>
                        <div className={styles.sectionLabel}>Active Rules</div>
                        <div className={styles.ruleChipList}>
                          {summary?.rules.length ? (
                            summary.rules.map((rule) => (
                              <span
                                className={styles.ruleChip}
                                key={`${rule.group_id || "local"}-${rule.name}`}
                                title={rule.group_name ? `${rule.group_name}/${rule.name}` : rule.name}
                              >
                                {rule.group_name ? `${rule.group_name}/` : ""}
                                {rule.name} · {rule.rule_count}
                              </span>
                            ))
                          ) : (
                            <span className={styles.mutedText}>No active rules resolved</span>
                          )}
                        </div>
                      </div>
                    </div>
                    {port.missing_refs.length > 0 ? (
                      <div className={styles.missingRules}>
                        Missing: {port.missing_refs.map(formatRuleRef).join(", ")}
                      </div>
                    ) : null}
                    <div className={styles.tempMergedHeader}>
                      <div className={styles.sectionLabel}>Merged Rules</div>
                      <div className={styles.lineCount}>
                        {countLines(summary?.merged_content ?? "").toLocaleString()} lines
                      </div>
                    </div>
                    <pre className={styles.tempMergedCode} data-testid={`activity-temporary-port-merged-${port.port}`}>
                      {summary?.merged_content?.trim() || "# No active rules"}
                    </pre>
                  </article>
                );
              })}
            </div>
          </section>
        ) : null}

        <section className={styles.panel} data-testid="activity-distribution-panel">
          <div className={styles.distributionHeader}>
            <h2 className={styles.panelTitle}>Traffic Distribution</h2>
            <div className={styles.distributionMode}>By application</div>
          </div>
          {appDistribution.length > 0 ? (
            <div className={styles.barList}>
              {appDistribution.map((entry) => (
                <div className={styles.barRow} key={entry.name} data-testid="activity-app-row">
                  <div className={styles.barLabel} title={entry.name}>
                    {entry.name}
                  </div>
                  <div className={styles.barTrack}>
                    <div
                      className={styles.barFill}
                      style={{ width: `${entry.percent}%` }}
                    />
                  </div>
                  <div className={styles.barValue}>{entry.count.toLocaleString()}</div>
                </div>
              ))}
            </div>
          ) : (
            <div className={styles.emptyState}>No application traffic yet</div>
          )}
        </section>
      </div>
    </main>
  );
}
