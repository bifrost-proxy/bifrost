import {
  ArrowLeftOutlined,
  ArrowRightOutlined,
  FieldTimeOutlined,
  MessageOutlined,
  ReloadOutlined,
  RobotOutlined,
  SoundOutlined,
} from "@ant-design/icons";
import {
  Alert,
  Button,
  Grid,
  Input,
  Select,
  Skeleton,
  Space,
  Table,
  Tag,
  Typography,
  theme,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import {
  Link,
  Navigate,
  Route,
  Routes,
  useLocation,
  useSearchParams,
} from "react-router-dom";
import {
  getAgentRunSummaries,
  type AgentRunSummaryItem,
  type AgentRunSummaryResponse,
  type AgentRunSummaryStatus,
} from "../../api/agentSummaries";
import { getAsrCapabilities, listAsrTasks } from "../../api/asr";
import {
  getExternalCliConfig,
  getProviderStatus,
  listProviders,
} from "../../api/imGateway";
import ASR from "../ASR";
import AgentTab from "../Settings/tabs/AgentTab";
import ImGatewayTab from "../Settings/tabs/ImGatewayTab";
import { resolveLegacyAiRedirect } from "./aiLayout";
import {
  formatRunDuration,
  liveRunDuration,
  getRunSourceLabels,
  getRunStatusLabels,
  runSourceLabel,
} from "./runSummary";
import { useI18n } from "../../i18n";
import styles from "./index.module.css";

const { Text, Title, Paragraph } = Typography;
const { useBreakpoint } = Grid;
interface ModuleMetric {
  label: string;
  value: ReactNode;
}

interface ModuleCardProps {
  to: string;
  testId: string;
  icon: ReactNode;
  title: string;
  description: string;
  metrics: ModuleMetric[];
  loading?: boolean;
  error?: boolean;
  badge?: ReactNode;
}

function ModuleCard({
  to,
  testId,
  icon,
  title,
  description,
  metrics,
  loading,
  error,
  badge,
}: ModuleCardProps) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const cssVars = {
    "--ai-card-bg": token.colorBgContainer,
    "--ai-card-border": token.colorBorderSecondary,
    "--ai-card-hover-border": token.colorPrimaryBorder,
    "--ai-card-hover-shadow": token.boxShadowTertiary,
    "--ai-card-icon-bg": token.colorPrimaryBg,
    "--ai-card-icon-color": token.colorPrimary,
    "--ai-card-title": token.colorText,
    "--ai-card-description": token.colorTextSecondary,
    "--ai-card-metric-bg": token.colorFillQuaternary,
    "--ai-card-metric-label": token.colorTextTertiary,
    "--ai-card-metric-value": token.colorText,
  } as CSSProperties;

  return (
    <Link
      to={to}
      className={styles.moduleCard}
      style={cssVars}
      data-testid={testId}
      aria-label={t("ai.hub.openModule", { title })}
    >
      <div className={styles.moduleCardHeader}>
        <span className={styles.moduleIcon} aria-hidden="true">
          {icon}
        </span>
        <span className={styles.moduleBadge}>{badge}</span>
        <ArrowRightOutlined className={styles.moduleArrow} aria-hidden="true" />
      </div>
      <div>
        <div className={styles.moduleTitle}>{title}</div>
        <div className={styles.moduleDescription}>{description}</div>
      </div>
      <div className={styles.moduleMetrics} aria-label={t("ai.hub.moduleSummary", { title })}>
        {loading ? (
          <Skeleton active paragraph={{ rows: 1 }} title={false} />
        ) : error ? (
          <Text type="secondary">{t("ai.hub.summaryUnavailable")}</Text>
        ) : (
          metrics.map((metric) => (
            <div className={styles.moduleMetric} key={metric.label}>
              <span className={styles.moduleMetricValue}>{metric.value}</span>
              <span className={styles.moduleMetricLabel}>{metric.label}</span>
            </div>
          ))
        )}
      </div>
    </Link>
  );
}

interface HubSnapshot {
  loading: boolean;
  asr?: { available: boolean; taskCount: number; runningCount: number };
  channels?: { enabledCount: number; connectedCount: number };
  agents?: { enabledRunnerCount: number; defaultRunner: string | null };
  runs?: AgentRunSummaryResponse["summary"];
  errors: Set<"asr" | "channels" | "agents" | "runs">;
}

function AIHubPage() {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<HubSnapshot>({
    loading: true,
    errors: new Set(),
  });

  const loadSnapshot = useCallback(async (silent = false) => {
    if (!silent) {
      setSnapshot((current) => ({ ...current, loading: true }));
    }
    const [asrResult, channelResult, agentResult, runResult] =
      await Promise.allSettled([
        Promise.all([getAsrCapabilities(), listAsrTasks()]),
        listProviders().then(async (providers) => {
          const enabled = providers.filter((provider) => provider.enabled);
          const statuses = await Promise.allSettled(
            enabled.map((provider) => getProviderStatus(provider.id)),
          );
          return {
            enabledCount: enabled.length,
            connectedCount: statuses.filter(
              (result) =>
                result.status === "fulfilled" &&
                result.value.state === "connected",
            ).length,
          };
        }),
        getExternalCliConfig(),
        getAgentRunSummaries({ limit: 1 }),
      ]);
    const errors = new Set<"asr" | "channels" | "agents" | "runs">();
    if (asrResult.status === "rejected") errors.add("asr");
    if (channelResult.status === "rejected") errors.add("channels");
    if (agentResult.status === "rejected") errors.add("agents");
    if (runResult.status === "rejected") errors.add("runs");
    setSnapshot({
      loading: false,
      errors,
      asr:
        asrResult.status === "fulfilled"
          ? {
              available:
                asrResult.value[0].qwen3_asr.enabled &&
                !asrResult.value[0].qwen3_asr.hidden,
              taskCount: asrResult.value[1].length,
              runningCount: asrResult.value[1].filter(
                (task) => task.summary.running,
              ).length,
            }
          : undefined,
      channels:
        channelResult.status === "fulfilled" ? channelResult.value : undefined,
      agents:
        agentResult.status === "fulfilled"
          ? {
              enabledRunnerCount: Object.values(
                agentResult.value.runners || {},
              ).filter((runner) => runner.enabled !== false).length,
              defaultRunner: agentResult.value.defaultRunnerId || null,
            }
          : undefined,
      runs:
        runResult.status === "fulfilled" ? runResult.value.summary : undefined,
    });
  }, []);

  useEffect(() => {
    void loadSnapshot();
    const poll = window.setInterval(() => void loadSnapshot(true), 15_000);
    return () => window.clearInterval(poll);
  }, [loadSnapshot]);

  const activeRunnerText =
    snapshot.runs?.active_runners
      .map((runner) => `${runner.runner_id} × ${runner.count}`)
      .join(", ") || t("ai.hub.none");

  return (
    <main
      className={styles.hubPage}
      data-testid="ai-module-hub"
      style={{ background: token.colorBgLayout }}
    >
      <div className={styles.hubContent} data-testid="ai-hub-content">
        <div className={styles.hubHeading}>
          <div>
            <Title level={2} style={{ margin: 0 }}>
              {t("ai.hub.title")}
            </Title>
            <Paragraph type="secondary" style={{ margin: "8px 0 0" }}>
              {t("ai.hub.description")}
            </Paragraph>
          </div>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => void loadSnapshot()}
            loading={snapshot.loading}
          >
            {t("ai.hub.refreshSummaries")}
          </Button>
        </div>

        <div className={styles.moduleGrid}>
          <ModuleCard
            to="/ai/asr"
            testId="ai-module-card-asr"
            icon={<SoundOutlined />}
            title={t("ai.module.asr.title")}
            description={t("ai.module.asr.cardDescription")}
            loading={snapshot.loading}
            error={snapshot.errors.has("asr")}
            badge={
              snapshot.asr && (
                <Tag color={snapshot.asr.available ? "success" : "default"}>
                  {snapshot.asr.available
                    ? t("ai.hub.available")
                    : t("ai.hub.unavailable")}
                </Tag>
              )
            }
            metrics={[
              {
                label: t("ai.hub.metric.tasks"),
                value: snapshot.asr?.taskCount ?? 0,
              },
              {
                label: t("ai.hub.metric.running"),
                value: snapshot.asr?.runningCount ?? 0,
              },
            ]}
          />
          <ModuleCard
            to="/ai/channels"
            testId="ai-module-card-channels"
            icon={<MessageOutlined />}
            title={t("ai.module.channels.title")}
            description={t("ai.module.channels.cardDescription")}
            loading={snapshot.loading}
            error={snapshot.errors.has("channels")}
            metrics={[
              {
                label: t("ai.hub.metric.enabled"),
                value: snapshot.channels?.enabledCount ?? 0,
              },
              {
                label: t("ai.hub.metric.connected"),
                value: snapshot.channels?.connectedCount ?? 0,
              },
            ]}
          />
          <ModuleCard
            to="/ai/agents"
            testId="ai-module-card-agents"
            icon={<RobotOutlined />}
            title={t("ai.module.agents.title")}
            description={t("ai.module.agents.description")}
            loading={snapshot.loading}
            error={snapshot.errors.has("agents")}
            metrics={[
              {
                label: t("ai.hub.metric.availableRunners"),
                value: snapshot.agents?.enabledRunnerCount ?? 0,
              },
              {
                label: t("ai.hub.metric.defaultRunner"),
                value: snapshot.agents
                  ? (snapshot.agents.defaultRunner ?? t("ai.hub.notSet"))
                  : "—",
              },
            ]}
          />
          <ModuleCard
            to="/ai/runs"
            testId="ai-module-card-runs"
            icon={<FieldTimeOutlined />}
            title={t("ai.module.runs.title")}
            description={t("ai.module.runs.cardDescription")}
            loading={snapshot.loading}
            error={snapshot.errors.has("runs")}
            badge={
              snapshot.runs && snapshot.runs.running_count > 0 ? (
                <Tag color="processing">
                  {t("ai.hub.runningCount", {
                    count: snapshot.runs.running_count,
                  })}
                </Tag>
              ) : undefined
            }
            metrics={[
              {
                label: t("ai.hub.metric.running"),
                value: snapshot.runs?.running_count ?? 0,
              },
              {
                label: t("ai.hub.metric.totalRuns"),
                value: snapshot.runs?.total_count ?? 0,
              },
              {
                label: t("ai.hub.metric.activeRunners"),
                value: activeRunnerText,
              },
            ]}
          />
        </div>
      </div>
    </main>
  );
}

function AIDetailPage({
  title,
  description,
  children,
  scroll = false,
}: {
  title: string;
  description: string;
  children: ReactNode;
  scroll?: boolean;
}) {
  const { token } = theme.useToken();
  const { t } = useI18n();
  return (
    <main
      className={styles.detailPage}
      style={
        {
          background: token.colorBgLayout,
          "--ai-bg-container": token.colorBgContainer,
          "--ai-border-secondary": token.colorBorderSecondary,
          "--ai-text": token.colorText,
          "--ai-text-secondary": token.colorTextSecondary,
          "--ai-text-tertiary": token.colorTextTertiary,
        } as CSSProperties
      }
      data-testid="ai-detail-page"
    >
      <header
        className={styles.detailHeader}
        style={{
          background: token.colorBgContainer,
          borderColor: token.colorBorderSecondary,
        }}
      >
        <div
          className={styles.detailHeaderContent}
          data-testid="ai-detail-content"
        >
          <div className={styles.detailBreadcrumb}>
            <Link
              className={styles.backLink}
              to="/ai"
              data-testid="ai-home-link"
            >
              <ArrowLeftOutlined /> {t("ai.detail.home")}
            </Link>
            <Text type="secondary" className={styles.detailCrumbCurrent}>
              / {title}
            </Text>
          </div>
          <div>
            <Title level={3} style={{ margin: 0 }}>
              {title}
            </Title>
            <Text type="secondary">{description}</Text>
          </div>
        </div>
      </header>
      <div
        className={`${styles.detailBody} ${scroll ? styles.detailBodyScroll : ""}`}
        data-testid="ai-detail-body"
      >
        {children}
      </div>
    </main>
  );
}

function RunRecordsPage() {
  const screens = useBreakpoint();
  const { t, locale } = useI18n();
  const dateLocale = locale === "zh-CN" ? "zh-CN" : "en-US";
  const runStatusLabels = useMemo(() => getRunStatusLabels(t), [t]);
  const runSourceLabels = useMemo(() => getRunSourceLabels(t), [t]);
  const [searchParams, setSearchParams] = useSearchParams();
  const [response, setResponse] = useState<AgentRunSummaryResponse | null>(
    null,
  );
  const [items, setItems] = useState<AgentRunSummaryItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState(false);
  const [searchDraft, setSearchDraft] = useState(searchParams.get("q") || "");
  const [nowSeconds, setNowSeconds] = useState(() =>
    Math.floor(Date.now() / 1000),
  );
  const filterKey = searchParams.toString();

  const requestPage = useCallback(
    async (cursor?: string, append = false) => {
      if (append) {
        setLoadingMore(true);
      } else {
        setLoading(true);
      }
      setError(false);
      try {
        const filters = new URLSearchParams(filterKey);
        const data = await getAgentRunSummaries({
          q: filters.get("q") || undefined,
          status: filters.get("status") || undefined,
          runner: filters.get("runner") || undefined,
          source: filters.get("source") || undefined,
          cursor,
          limit: 30,
        });
        setResponse(data);
        setItems((current) =>
          append ? [...current, ...data.items] : data.items,
        );
      } catch {
        setError(true);
      } finally {
        setLoading(false);
        setLoadingMore(false);
      }
    },
    [filterKey],
  );

  useEffect(() => {
    setSearchDraft(new URLSearchParams(filterKey).get("q") || "");
    void requestPage();
    const poll = window.setInterval(() => void requestPage(), 15_000);
    return () => window.clearInterval(poll);
  }, [requestPage, filterKey]);

  useEffect(() => {
    const timer = window.setInterval(
      () => setNowSeconds(Math.floor(Date.now() / 1000)),
      1000,
    );
    return () => window.clearInterval(timer);
  }, []);

  const updateFilter = (key: string, value?: string) => {
    const next = new URLSearchParams(searchParams);
    if (value) {
      next.set(key, value);
    } else {
      next.delete(key);
    }
    setSearchParams(next, { replace: true });
  };

  const runnerOptions = useMemo(() => {
    const runners = new Set(items.map((item) => item.runner_id));
    response?.summary.active_runners.forEach((runner) =>
      runners.add(runner.runner_id),
    );
    return Array.from(runners)
      .sort()
      .map((runner) => ({ label: runner, value: runner }));
  }, [items, response]);

  const statusTag = (status: AgentRunSummaryStatus) => (
    <Tag
      color={
        status === "running"
          ? "processing"
          : status === "completed"
            ? "success"
            : status === "failed"
              ? "error"
              : "default"
      }
    >
      {runStatusLabels[status]}
    </Tag>
  );

  const columns: ColumnsType<AgentRunSummaryItem> = [
    {
      title: t("ai.runs.column.status"),
      dataIndex: "status",
      width: 92,
      render: statusTag,
    },
    {
      title: t("ai.runs.column.title"),
      dataIndex: "title",
      ellipsis: true,
      render: (title: string) => <Text strong>{title}</Text>,
    },
    { title: t("ai.runs.column.runner"), dataIndex: "runner_id", width: 140 },
    {
      title: t("ai.runs.column.duration"),
      width: 132,
      render: (_, item) =>
        formatRunDuration(liveRunDuration(item, nowSeconds)),
    },
    {
      title: t("ai.runs.column.userMessages"),
      dataIndex: "user_message_count",
      width: 104,
      render: (count: number) => count,
    },
    {
      title: t("ai.runs.column.source"),
      dataIndex: "source",
      width: 100,
      render: (source: string) => runSourceLabel(source, t),
    },
    {
      title: t("ai.runs.column.started"),
      dataIndex: "start_time",
      width: 180,
      render: (timestamp: number) =>
        timestamp
          ? new Date(timestamp * 1000).toLocaleString(dateLocale, {
              hour12: false,
            })
          : "—",
    },
  ];

  return (
    <AIDetailPage
      title={t("ai.module.runs.title")}
      description={t("ai.module.runs.description")}
      scroll
    >
      <div className={styles.runPage} data-testid="agent-run-summaries">
        <section className={styles.runOverview} aria-label={t("ai.runs.overview")}>
          <div>
            <span className={styles.overviewValue}>
              {response?.summary.running_count ?? "—"}
            </span>
            <span className={styles.overviewLabel}>
              {t("ai.runs.runningNow")}
            </span>
          </div>
          <div>
            <span className={styles.overviewValue}>
              {response?.summary.total_count ?? "—"}
            </span>
            <span className={styles.overviewLabel}>
              {t("ai.runs.matchingRuns")}
            </span>
          </div>
          <div className={styles.activeRunners}>
            <span className={styles.overviewLabel}>
              {t("ai.runs.activeRunners")}
            </span>
            <Space size={[4, 4]} wrap>
              {response?.summary.active_runners.length ? (
                response.summary.active_runners.map((runner) => (
                  <Tag key={runner.runner_id}>
                    {runner.runner_id} × {runner.count}
                  </Tag>
                ))
              ) : (
                <Text type="secondary">{t("ai.runs.noActiveThreads")}</Text>
              )}
            </Space>
          </div>
        </section>

        <section className={styles.runFilters} aria-label={t("ai.runs.filters")}>
          <Input.Search
            allowClear
            value={searchDraft}
            placeholder={t("ai.runs.searchPlaceholder")}
            onChange={(event) => setSearchDraft(event.target.value)}
            onSearch={(value) => updateFilter("q", value.trim() || undefined)}
            className={styles.searchInput}
          />
          <Select
            allowClear
            placeholder={t("ai.runs.allStatuses")}
            value={searchParams.get("status") || undefined}
            onChange={(value) => updateFilter("status", value)}
            options={Object.entries(runStatusLabels).map(
              ([value, label]) => ({
                value,
                label,
              }),
            )}
          />
          <Select
            allowClear
            showSearch
            placeholder={t("ai.runs.allRunners")}
            value={searchParams.get("runner") || undefined}
            onChange={(value) => updateFilter("runner", value)}
            options={runnerOptions}
          />
          <Select
            allowClear
            placeholder={t("ai.runs.allSources")}
            value={searchParams.get("source") || undefined}
            onChange={(value) => updateFilter("source", value)}
            options={Object.entries(runSourceLabels).map(
              ([value, label]) => ({
                value,
                label,
              }),
            )}
          />
          <Button icon={<ReloadOutlined />} onClick={() => void requestPage()}>
            {t("common.refresh")}
          </Button>
        </section>

        {error && (
          <Alert
            type="error"
            showIcon
            message={t("ai.runs.loadFailed")}
            action={
              <Button onClick={() => void requestPage()}>
                {t("common.retry")}
              </Button>
            }
          />
        )}

        {screens.md ? (
          <Table
            rowKey="session_key"
            dataSource={items}
            columns={columns}
            loading={loading}
            pagination={false}
            locale={{ emptyText: t("ai.runs.empty") }}
            data-testid="agent-run-summary-table"
          />
        ) : (
          <div
            className={styles.mobileRunList}
            data-testid="agent-run-summary-list"
          >
            {loading && !items.length ? (
              <Skeleton active paragraph={{ rows: 5 }} />
            ) : items.length ? (
              items.map((item) => (
                <article
                  className={styles.mobileRunCard}
                  key={item.session_key}
                >
                  <div className={styles.mobileRunTitle}>
                    <Text strong ellipsis>
                      {item.title}
                    </Text>
                    {statusTag(item.status)}
                  </div>
                  <dl className={styles.mobileRunFacts}>
                    <div>
                      <dt>{t("ai.runs.column.runner")}</dt>
                      <dd>{item.runner_id}</dd>
                    </div>
                    <div>
                      <dt>{t("ai.runs.column.duration")}</dt>
                      <dd>
                        {formatRunDuration(
                          liveRunDuration(item, nowSeconds),
                        )}
                      </dd>
                    </div>
                    <div>
                      <dt>{t("ai.runs.column.userMessages")}</dt>
                      <dd>{item.user_message_count}</dd>
                    </div>
                    <div>
                      <dt>{t("ai.runs.column.source")}</dt>
                      <dd>{runSourceLabel(item.source, t)}</dd>
                    </div>
                    <div>
                      <dt>{t("ai.runs.column.started")}</dt>
                      <dd>
                        {item.start_time
                          ? new Date(item.start_time * 1000).toLocaleString(
                              dateLocale,
                              {
                                hour12: false,
                              },
                            )
                          : "—"}
                      </dd>
                    </div>
                  </dl>
                </article>
              ))
            ) : (
              <Text type="secondary">{t("ai.runs.empty")}</Text>
            )}
          </div>
        )}

        {response?.next_cursor && (
          <div className={styles.loadMore}>
            <Button
              loading={loadingMore}
              onClick={() =>
                void requestPage(response.next_cursor || undefined, true)
              }
            >
              {t("ai.runs.loadMore")}
            </Button>
          </div>
        )}
      </div>
    </AIDetailPage>
  );
}

function LegacyAIEntry() {
  const [searchParams] = useSearchParams();
  const destination = resolveLegacyAiRedirect(searchParams);
  if (!destination) return <AIHubPage />;
  return <Navigate replace to={destination} />;
}

export default function AI() {
  const location = useLocation();
  const { t } = useI18n();
  return (
    <Routes location={location}>
      <Route index element={<LegacyAIEntry />} />
      <Route
        path="asr"
        element={
          <AIDetailPage
            title={t("ai.module.asr.title")}
            description={t("ai.module.asr.description")}
          >
            <ASR />
          </AIDetailPage>
        }
      />
      <Route
        path="channels"
        element={
          <AIDetailPage
            title={t("ai.module.channels.title")}
            description={t("ai.module.channels.description")}
          >
            <ImGatewayTab hideSectionNav cardGrid />
          </AIDetailPage>
        }
      />
      <Route
        path="agents"
        element={
          <AIDetailPage
            title={t("ai.module.agents.title")}
            description={t("ai.module.agents.description")}
          >
            <AgentTab />
          </AIDetailPage>
        }
      />
      <Route path="runs" element={<RunRecordsPage />} />
      <Route path="*" element={<Navigate to="/ai" replace />} />
    </Routes>
  );
}
