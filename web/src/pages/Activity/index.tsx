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
import RuleEffectivenessCode from "../../components/RuleEffectivenessCode";
import { useI18n, type TFunction } from "../../i18n";
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

const formatRuleRef = (t: TFunction, ref: TemporaryPortRuleSetRef): string => {
  switch (ref.type) {
    case "local_rule":
      return ref.name;
    case "group_rule":
      return `${ref.group_id}/${ref.name}`;
    case "rule_file":
      return ref.path;
    case "inline_rule":
      return ref.content.split(/\r?\n/).find((line) => line.trim()) || t("activity.inlineRule");
    default:
      return t("activity.unknownRuleRef");
  }
};

export default function Activity() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const requestIdRef = useRef(0);
  const mergedCodeRef = useRef<HTMLPreElement>(null);
  const [activeRules, setActiveRules] = useState<ActiveRuleItem[]>([]);
  const [mergedContent, setMergedContent] = useState("");
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
      availableClientApps: state.availableClientApps,
      clientAppCounts: state.clientAppCounts,
      statisticsTotal: state.trafficStatisticsTotal,
      statisticsLoaded: state.trafficStatisticsLoaded,
    })),
  );

  const refreshActiveSummary = useCallback(() => {
    const requestId = ++requestIdRef.current;
    getActiveSummary()
      .then((summary) => {
        if (requestId !== requestIdRef.current) return;
        setActiveRules(summary.rules ?? []);
        setMergedContent(summary.merged_content ?? "");
      })
      .catch(() => {
        if (requestId !== requestIdRef.current) return;
        setActiveRules([]);
        setMergedContent("");
      });
  }, []);

  const navigateToRule = useCallback(
    (rule: ActiveRuleItem) => {
      const params = new URLSearchParams();
      if (rule.group_id) params.set("group", rule.group_id);
      params.set("rule", rule.name);
      navigate({ pathname: "/rules", search: `?${params.toString()}` });
    },
    [navigate],
  );

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
    (traffic.statisticsLoaded ? traffic.statisticsTotal : undefined) ??
    metricSnapshot?.total_requests ??
    traffic.records.length;
  const totalUpload = metricSnapshot?.bytes_sent ?? traffic.records.reduce((sum, record) => sum + (record.upload_bytes ?? record.request_size), 0);
  const totalDownload = metricSnapshot?.bytes_received ?? traffic.records.reduce((sum, record) => sum + (record.download_bytes ?? record.response_size), 0);
  const rulesTotal = overview?.rules.total ?? activeRules.reduce((sum, rule) => sum + rule.rule_count, 0);
  const rulesEnabled = overview?.rules.enabled ?? activeRules.length;
  const serverPort = overview?.server.port ?? 9900;

  const systemProxyEnabled = systemProxy ? isSystemProxyLiveEnabledByBifrost(systemProxy) : false;
  const serviceState = systemProxyEnabled ? t("common.enabled") : t("common.disabled");
  const serviceCaption = `http://${systemProxy?.host || "127.0.0.1"}:${systemProxy?.port || serverPort}`;

  const stats: ActivityStat[] = [
    {
      label: t("activity.statActiveConnections"),
      value: liveConnections.toLocaleString(),
      caption: t("activity.statApps", {
        count: traffic.availableClientApps.length.toLocaleString(),
      }),
      color: "#ff8a2a",
    },
    {
      label: t("activity.statUpload"),
      value: formatRate(metricSnapshot?.bytes_sent_rate ?? 0),
      caption: formatBytes(totalUpload),
      color: "#6256ff",
    },
    {
      label: t("activity.statDownload"),
      value: formatRate(metricSnapshot?.bytes_received_rate ?? 0),
      caption: formatBytes(totalDownload),
      color: "#14b8d5",
    },
    {
      label: t("activity.statRequests"),
      value: totalRequests.toLocaleString(),
      caption: `${(metricSnapshot?.qps ?? 0).toFixed(2)} QPS`,
      color: "#32c766",
    },
    {
      label: t("activity.statRules"),
      value: `${rulesEnabled}/${Math.max(rulesTotal, rulesEnabled)}`,
      caption: t("activity.statCurrentRuleSet"),
      color: "#cf3be9",
    },
    {
      label: t("activity.statSystemProxy"),
      value: serviceState,
      caption: serviceCaption,
      color: systemProxyEnabled ? "#32c766" : "#168cff",
    },
  ];

  return (
    <main className={styles.activityPage} data-testid="activity-page">
      <div className={styles.activityShell}>
        <h1 className={styles.pageTitle}>{t("activity.title")}</h1>

        <section className={styles.statGrid} aria-label={t("activity.metricsAria")}>
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
              <h2 className={styles.panelTitle}>{t("activity.activeRuleAnalysis")}</h2>
              <div className={styles.panelSubtitle}>{t("activity.activeRuleAnalysisSubtitle")}</div>
            </div>
            <button
              type="button"
              className={styles.activeBadge}
              onClick={() => navigate("/rules")}
              data-testid="activity-rules-open-button"
            >
              <span className={styles.statDot} style={{ color: "#32c766", backgroundColor: "#32c766" }} />
              {t("activity.activeCount", { count: activeRules.length.toLocaleString() })}
            </button>
          </div>

          <div className={styles.rulesLayout}>
            <div className={styles.mergedColumn}>
              <div className={styles.mergedHeader}>
                <div className={styles.sectionLabel}>{t("activity.mergedRules")}</div>
                <div className={styles.mergedActions}>
                  <button
                    type="button"
                    className={styles.copyButton}
                    onClick={handleCopyMergedRules}
                    data-testid="activity-copy-merged-rules"
                    aria-label={t("activity.copyMergedRules")}
                    title={t("activity.copyMergedRulesTooltip")}
                  >
                    {copied ? <CheckOutlined /> : <CopyOutlined />}
                  </button>
                  <div className={styles.lineCount}>
                    {t("activity.lineCount", { count: countLines(mergedContent).toLocaleString() })}
                  </div>
                </div>
              </div>
              {activeRules.length > 0 ? (
                <div className={styles.ruleList} aria-label={t("activity.activeRulesAria")}>
                  {activeRules.map((rule) => (
                    <button
                      type="button"
                      key={`${rule.group_id ?? "local"}:${rule.name}`}
                      className={styles.rulePill}
                      onClick={() => navigateToRule(rule)}
                      data-testid="activity-rule-pill"
                      data-rule-name={rule.name}
                      title={rule.group_name ? `${rule.group_name} / ${rule.name}` : rule.name}
                    >
                      <span className={styles.rulePillName}>{rule.name}</span>
                      <span className={styles.rulePillMeta}>
                        {t("activity.entryCount", { count: rule.rule_count.toLocaleString() })}
                        {rule.group_name ? ` · ${rule.group_name}` : ""}
                      </span>
                    </button>
                  ))}
                </div>
              ) : (
                <div className={styles.emptyState}>{t("activity.noActiveRules")}</div>
              )}
              <RuleEffectivenessCode
                ref={mergedCodeRef}
                className={styles.mergedCode}
                content={mergedContent}
                data-testid="activity-merged-rules"
              />
            </div>
          </div>
        </section>

        {temporaryPorts.length > 0 ? (
          <section className={`${styles.panel} ${styles.temporaryPortsPanel}`} data-testid="activity-temporary-ports-panel">
            <div className={styles.distributionHeader}>
              <div>
                <h2 className={styles.panelTitle}>{t("activity.temporaryPorts")}</h2>
                <div className={styles.panelSubtitle}>{t("activity.temporaryPortsSubtitle")}</div>
              </div>
              <div className={styles.distributionMode}>
                {t("activity.activeCount", { count: temporaryPorts.length.toLocaleString() })}
              </div>
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
                        <div className={styles.sectionLabel}>{t("activity.boundRules")}</div>
                        <div className={styles.ruleChipList}>
                          {port.rule_refs.length > 0 ? (
                            port.rule_refs.map((ref, index) => (
                              <span className={styles.ruleChip} key={`${ref.type}-${index}`} title={formatRuleRef(t, ref)}>
                                {formatRuleRef(t, ref)}
                              </span>
                            ))
                          ) : (
                            <span className={styles.mutedText}>{t("activity.noBoundRuleSets")}</span>
                          )}
                        </div>
                      </div>
                      <div>
                        <div className={styles.sectionLabel}>{t("activity.activeRules")}</div>
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
                            <span className={styles.mutedText}>{t("activity.noActiveRulesResolved")}</span>
                          )}
                        </div>
                      </div>
                    </div>
                    {port.missing_refs.length > 0 ? (
                      <div className={styles.missingRules}>
                        {t("activity.missing", {
                          refs: port.missing_refs.map((ref) => formatRuleRef(t, ref)).join(", "),
                        })}
                      </div>
                    ) : null}
                    <div className={styles.tempMergedHeader}>
                      <div className={styles.sectionLabel}>{t("activity.mergedRules")}</div>
                      <div className={styles.lineCount}>
                        {t("activity.lineCount", {
                          count: countLines(summary?.merged_content ?? "").toLocaleString(),
                        })}
                      </div>
                    </div>
                    <RuleEffectivenessCode
                      className={styles.tempMergedCode}
                      content={summary?.merged_content ?? ""}
                      data-testid={`activity-temporary-port-merged-${port.port}`}
                    />
                  </article>
                );
              })}
            </div>
          </section>
        ) : null}

        <section className={styles.panel} data-testid="activity-distribution-panel">
          <div className={styles.distributionHeader}>
            <h2 className={styles.panelTitle}>{t("activity.trafficDistribution")}</h2>
            <div className={styles.distributionMode}>{t("activity.byApplication")}</div>
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
            <div className={styles.emptyState}>{t("activity.noAppTraffic")}</div>
          )}
        </section>
      </div>
    </main>
  );
}
