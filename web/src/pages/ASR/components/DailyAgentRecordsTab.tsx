import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Card, Empty, message, Select, Space, Table, Tag, Typography } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import type {
  AsrDailyAgentProcessedDocument,
  AsrDailyAgentRunsResponse,
} from "../../../api/asr";
import { getDailyAgentRuns } from "../../../api/asr";

const { Text } = Typography;
const DAILY_AGENT_RECORDS_TABLE_SCROLL_X = 1080;

interface DailyAgentRecordsTabProps {
  taskId: string;
  onOpenReport?: (date: string, agentId?: string) => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function DailyAgentRecordsTab({
  taskId,
  onOpenReport,
}: DailyAgentRecordsTabProps) {
  const [runsData, setRunsData] = useState<AsrDailyAgentRunsResponse | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [agentFilter, setAgentFilter] = useState<string | undefined>();
  const [dateFilter, setDateFilter] = useState<string | undefined>();
  const [runnerFilter, setRunnerFilter] = useState<string | undefined>();

  const fetchRuns = useCallback(async () => {
    setLoading(true);
    try {
      setRunsData(await getDailyAgentRuns(taskId));
    } catch (error: unknown) {
      message.error(
        `Failed to load Daily Agent records: ${errorMessage(error)}`,
      );
    } finally {
      setLoading(false);
    }
  }, [taskId]);

  useEffect(() => {
    fetchRuns();
  }, [fetchRuns]);

  useEffect(() => {
    setAgentFilter(undefined);
    setDateFilter(undefined);
    setRunnerFilter(undefined);
  }, [taskId]);

  const processedDocuments = useMemo(
    () =>
      [...(runsData?.processed_documents ?? [])].sort((a, b) => {
        const dateOrder = b.date.localeCompare(a.date);
        if (dateOrder !== 0) {
          return dateOrder;
        }
        return (b.processed_at_ms ?? 0) - (a.processed_at_ms ?? 0);
      }),
    [runsData],
  );

  const filterOptions = useMemo(() => {
    const agents = new Map<string, string>();
    const dates = new Set<string>();
    const runners = new Set<string>();

    for (const record of processedDocuments) {
      const agentId = record.agent_id || "daily_report";
      agents.set(agentId, record.agent_name || agentId);
      if (record.date) {
        dates.add(record.date);
      }
      if (record.runner) {
        runners.add(record.runner);
      }
    }

    return {
      agents: [...agents.entries()]
        .sort((a, b) => a[1].localeCompare(b[1]))
        .map(([value, label]) => ({
          value,
          label: label === value ? label : `${label} (${value})`,
        })),
      dates: [...dates]
        .sort((a, b) => b.localeCompare(a))
        .map((value) => ({ value, label: value })),
      runners: [...runners].sort().map((value) => ({ value, label: value })),
    };
  }, [processedDocuments]);

  const filteredDocuments = useMemo(
    () =>
      processedDocuments.filter((record) => {
        if (
          agentFilter &&
          (record.agent_id || "daily_report") !== agentFilter
        ) {
          return false;
        }
        if (dateFilter && record.date !== dateFilter) {
          return false;
        }
        if (runnerFilter && record.runner !== runnerFilter) {
          return false;
        }
        return true;
      }),
    [agentFilter, dateFilter, processedDocuments, runnerFilter],
  );

  return (
    <Card
      size="small"
      title="Run Results"
      style={{ width: "100%", minWidth: 0 }}
      loading={loading && !runsData}
      extra={
        <Button icon={<ReloadOutlined />} onClick={fetchRuns} loading={loading}>
          Refresh
        </Button>
      }
    >
      {runsData && processedDocuments.length > 0 ? (
        <Space
          direction="vertical"
          size={12}
          style={{ width: "100%", minWidth: 0 }}
        >
          <Space size={8} wrap>
            <Select
              allowClear
              showSearch
              placeholder="Agent"
              style={{ width: 220 }}
              value={agentFilter}
              options={filterOptions.agents}
              optionFilterProp="label"
              onChange={setAgentFilter}
              data-testid="asr-daily-agent-records-agent-filter"
            />
            <Select
              allowClear
              showSearch
              placeholder="Date"
              style={{ width: 160 }}
              value={dateFilter}
              options={filterOptions.dates}
              optionFilterProp="label"
              onChange={setDateFilter}
              data-testid="asr-daily-agent-records-date-filter"
            />
            <Select
              allowClear
              showSearch
              placeholder="Runner"
              style={{ width: 160 }}
              value={runnerFilter}
              options={filterOptions.runners}
              optionFilterProp="label"
              onChange={setRunnerFilter}
              data-testid="asr-daily-agent-records-runner-filter"
            />
          </Space>
          <div
            data-testid="asr-daily-agent-run-results-table"
            style={{ width: "100%", minWidth: 0, overflow: "hidden" }}
          >
            <Table<AsrDailyAgentProcessedDocument>
              rowKey={(record) =>
                [
                  record.agent_id || "daily_report",
                  record.date,
                  record.last_run_id ||
                    record.report_path ||
                    record.output_dir ||
                    "",
                ].join(":")
              }
              size="small"
              tableLayout="fixed"
              scroll={{ x: DAILY_AGENT_RECORDS_TABLE_SCROLL_X }}
              dataSource={filteredDocuments}
              pagination={{ pageSize: 10, hideOnSinglePage: true }}
              columns={[
                { title: "Date", dataIndex: "date", width: 120 },
                {
                  title: "Agent",
                  dataIndex: "agent_name",
                  width: 140,
                  render: (value, record) => (
                    <Tag>{value || record.agent_id}</Tag>
                  ),
                },
                {
                  title: "Output",
                  dataIndex: "output_dir",
                  width: 120,
                  render: (value) => <Text code>{value || "report"}</Text>,
                },
                {
                  title: "Processed At",
                  dataIndex: "processed_at_ms",
                  width: 180,
                  render: (value) =>
                    value ? new Date(value).toLocaleString() : "-",
                },
                {
                  title: "SHA256",
                  dataIndex: "source_sha256",
                  width: 100,
                  render: (value) => (
                    <Text code style={{ fontSize: 10 }}>
                      {value?.slice(0, 8)}
                    </Text>
                  ),
                },
                {
                  title: "Size",
                  dataIndex: "source_len_bytes",
                  width: 80,
                  render: (value: number) =>
                    value < 1024
                      ? `${value} B`
                      : value < 1024 * 1024
                        ? `${(value / 1024).toFixed(1)} KB`
                        : `${(value / 1024 / 1024).toFixed(1)} MB`,
                },
                {
                  title: "Runner",
                  dataIndex: "runner",
                  width: 100,
                  render: (value) => <Tag>{value}</Tag>,
                },
                {
                  title: "Report",
                  dataIndex: "report_path",
                  width: 220,
                  ellipsis: true,
                  render: (value, record) =>
                    value ? (
                      <Button
                        type="link"
                        size="small"
                        data-testid={`asr-daily-agent-report-link-${record.agent_id}-${record.date}`}
                        style={{ padding: 0, height: "auto", fontSize: 11 }}
                        onClick={() =>
                          onOpenReport?.(record.date, record.agent_id)
                        }
                      >
                        {value.split("/").pop()}
                      </Button>
                    ) : (
                      "-"
                    ),
                },
              ]}
              locale={{ emptyText: "No matching Daily Agent records" }}
            />
          </div>
        </Space>
      ) : (
        <Empty description="No Daily Agent records yet" />
      )}
    </Card>
  );
}
