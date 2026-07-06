import { Alert, Descriptions, List, Modal, Space, Tag, Typography } from "antd";
import TrafficDetail from "../TrafficDetail";
import type { NetworkPreviewDetail, PreviewResponse } from "../../api/bifrost-file";
import type { TrafficRecord } from "../../types";

interface BifrostFilePreviewPanelProps {
  filename: string;
  preview: PreviewResponse;
}

export function confirmBifrostFileImport(
  filename: string,
  preview: PreviewResponse,
): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    Modal.confirm({
      title: `Preview ${filename}`,
      width: preview.network?.single_record ? 1040 : 760,
      okText: "Import",
      cancelText: "Cancel",
      centered: true,
      content: <BifrostFilePreviewPanel filename={filename} preview={preview} />,
      onOk: () => {
        settled = true;
        resolve(true);
      },
      onCancel: () => {
        settled = true;
        resolve(false);
      },
      afterClose: () => {
        if (!settled) {
          resolve(false);
        }
      },
    });
  });
}

function BifrostFilePreviewPanel({ filename, preview }: BifrostFilePreviewPanelProps) {
  if (preview.rules) {
    return (
      <div style={{ marginTop: 12 }}>
        <Descriptions size="small" column={1} bordered>
          <Descriptions.Item label="File">{filename}</Descriptions.Item>
          <Descriptions.Item label="Rule name">{preview.rules.name}</Descriptions.Item>
          <Descriptions.Item label="Status">
            <Tag color={preview.rules.enabled ? "green" : "default"}>
              {preview.rules.enabled ? "Enabled" : "Disabled"}
            </Tag>
          </Descriptions.Item>
          <Descriptions.Item label="Lines">{preview.rules.line_count}</Descriptions.Item>
          {preview.rules.description ? (
            <Descriptions.Item label="Description">
              {preview.rules.description}
            </Descriptions.Item>
          ) : null}
        </Descriptions>
        <Typography.Text strong style={{ display: "block", marginTop: 16 }}>
          Rule content
        </Typography.Text>
        <pre
          style={{
            maxHeight: 320,
            overflow: "auto",
            marginTop: 8,
            padding: 12,
            border: "1px solid var(--ant-color-border)",
            borderRadius: 6,
            background: "var(--ant-color-fill-tertiary)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {preview.rules.content || "(empty rule)"}
        </pre>
      </div>
    );
  }

  if (preview.network) {
    if (preview.network.single_record) {
      return <SingleNetworkPreview detail={preview.network.single_record} />;
    }

    return (
      <div style={{ marginTop: 12 }}>
        <Alert
          type="info"
          showIcon
          message={`${preview.network.record_count} request(s) will be imported into Network`}
          description={
            preview.network.hosts.length > 0 ? (
              <Space size={[4, 4]} wrap style={{ marginTop: 8 }}>
                {preview.network.hosts.map((host) => (
                  <Tag key={host}>{host}</Tag>
                ))}
              </Space>
            ) : (
              "No host information found in this package."
            )
          }
        />
        <List
          size="small"
          style={{ marginTop: 16, maxHeight: 360, overflow: "auto" }}
          dataSource={preview.network.records}
          renderItem={(record) => (
            <List.Item>
              <Space direction="vertical" size={2} style={{ width: "100%" }}>
                <Space size={8} wrap>
                  <Tag color="blue">{record.method}</Tag>
                  <Tag color={record.status >= 400 ? "red" : "green"}>{record.status}</Tag>
                  <Typography.Text strong>{record.host || "(unknown host)"}</Typography.Text>
                  {record.client_app ? <Tag>{record.client_app}</Tag> : null}
                </Space>
                <Typography.Text type="secondary" ellipsis>
                  {record.url}
                </Typography.Text>
              </Space>
            </List.Item>
          )}
        />
      </div>
    );
  }

  return (
    <Alert
      style={{ marginTop: 12 }}
      type="info"
      showIcon
      message={`${preview.file_type} package`}
      description={`${preview.item_count ?? 0} item(s) will be imported.`}
    />
  );
}

function SingleNetworkPreview({ detail }: { detail: NetworkPreviewDetail }) {
  const record = normalizePreviewTrafficRecord(detail);
  return (
    <div style={{ marginTop: 12 }}>
      <Alert
        type="info"
        showIcon
        message="1 request will be imported into Network"
        description="Review the request detail below, then confirm to import."
      />
      <div
        style={{
          height: 560,
          marginTop: 12,
          border: "1px solid var(--ant-color-border)",
          borderRadius: 6,
          overflow: "hidden",
        }}
      >
        <TrafficDetail
          record={record}
          requestBody={detail.request_body ?? null}
          responseBody={detail.response_body ?? null}
        />
      </div>
    </div>
  );
}

function normalizePreviewTrafficRecord(detail: NetworkPreviewDetail): TrafficRecord {
  const record = detail.record as Partial<TrafficRecord> & {
    original_response_headers?: [string, string][] | null;
  };
  const matchedRules = record.matched_rules ?? null;
  const timestamp = record.timestamp ?? Date.now();
  return {
    id: record.id ?? "preview",
    sequence: record.sequence ?? 0,
    timestamp,
    method: record.method ?? "GET",
    url: record.url ?? "",
    status: record.status ?? 0,
    content_type: record.content_type ?? null,
    request_content_type: record.request_content_type ?? null,
    request_size: record.request_size ?? (detail.request_body?.length ?? 0),
    response_size: record.response_size ?? (detail.response_body?.length ?? 0),
    upload_bytes: record.upload_bytes ?? (detail.request_body?.length ?? 0),
    download_bytes: record.download_bytes ?? (detail.response_body?.length ?? 0),
    duration_ms: record.duration_ms ?? 0,
    listener_port: record.listener_port ?? 0,
    host: record.host ?? "",
    path: record.path ?? "",
    protocol: record.protocol ?? "",
    client_ip: record.client_ip ?? "imported",
    client_app: record.client_app ?? "Bifrost Import",
    client_pid: record.client_pid,
    has_rule_hit: record.has_rule_hit ?? Boolean(matchedRules?.length),
    matched_rule_count: matchedRules?.length ?? 0,
    matched_protocols: matchedRules?.map((rule) => rule.protocol) ?? [],
    is_websocket: record.is_websocket ?? false,
    is_sse: record.is_sse ?? false,
    is_h3: record.is_h3 ?? false,
    is_tunnel: record.is_tunnel ?? false,
    frame_count: record.frame_count ?? 0,
    socket_status: record.socket_status ?? null,
    start_time: new Date(timestamp).toISOString(),
    end_time: null,
    request_headers: record.request_headers ?? null,
    response_headers: record.response_headers ?? record.original_response_headers ?? null,
    request_body: detail.request_body ?? null,
    response_body: detail.response_body ?? null,
    request_body_ref: record.request_body_ref ?? null,
    response_body_ref: record.response_body_ref ?? null,
    raw_request_body_ref: record.raw_request_body_ref ?? null,
    raw_response_body_ref: record.raw_response_body_ref ?? null,
    matched_rules: matchedRules,
    timing: record.timing ?? null,
    actual_url: record.actual_url ?? null,
    actual_host: record.actual_host ?? null,
    original_request_headers: record.original_request_headers ?? null,
    original_response_headers: record.original_response_headers ?? null,
    req_script_results: record.req_script_results ?? null,
    res_script_results: record.res_script_results ?? null,
    decode_req_script_results: record.decode_req_script_results ?? null,
    decode_res_script_results: record.decode_res_script_results ?? null,
  };
}
