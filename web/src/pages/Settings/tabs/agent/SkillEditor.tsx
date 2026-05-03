import { useEffect, useState } from "react";
import { Descriptions, Modal, Spin, Tag, Typography } from "antd";
import { getSkill } from "../../../../api/agent-skills";
import type { SkillRecord, SkillDetailResponse } from "./types";

const { Text, Paragraph } = Typography;

type Props = {
  record: SkillRecord | null;
  open: boolean;
  onClose: () => void;
};

export default function SkillDetailViewer({ record, open, onClose }: Props) {
  const [detail, setDetail] = useState<SkillDetailResponse | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!record || !open) {
      setDetail(null);
      return;
    }
    setLoading(true);
    getSkill(record.name)
      .then(setDetail)
      .finally(() => setLoading(false));
  }, [record, open]);

  const manifest = detail?.manifest ?? record?.manifest;
  const entrypoint = manifest?.entrypoint;

  return (
    <Modal
      title={record ? `Skill: ${record.name}` : "Skill Detail"}
      open={open}
      onCancel={onClose}
      width={840}
      footer={null}
    >
      {loading ? (
        <Spin style={{ display: "block", textAlign: "center", padding: 32 }} />
      ) : record && manifest ? (
        <>
          <Descriptions column={2} size="small" bordered style={{ marginBottom: 16 }}>
            <Descriptions.Item label="Name">{manifest.name}</Descriptions.Item>
            <Descriptions.Item label="Version">{manifest.version}</Descriptions.Item>
            <Descriptions.Item label="Scope">
              <Tag color={scopeColor(record.effective_scope)}>{record.effective_scope}</Tag>
            </Descriptions.Item>
            <Descriptions.Item label="Enabled">
              <Tag color={record.enabled ? "green" : "default"}>
                {record.enabled ? "Yes" : "No"}
              </Tag>
            </Descriptions.Item>
            {manifest.slash_command ? (
              <Descriptions.Item label="Slash Command" span={2}>
                <Text code>{manifest.slash_command}</Text>
              </Descriptions.Item>
            ) : null}
            <Descriptions.Item label="Description" span={2}>
              {manifest.description}
            </Descriptions.Item>
            <Descriptions.Item label="Entrypoint" span={2}>
              <Tag>{entrypoint?.kind ?? "unknown"}</Tag>
              {"shell" in (entrypoint ?? {}) &&
                (entrypoint as { shell?: string }).shell && (
                  <Tag>{(entrypoint as { shell: string }).shell}</Tag>
                )}
            </Descriptions.Item>
            {manifest.triggers?.length ? (
              <Descriptions.Item label="Triggers" span={2}>
                {manifest.triggers.map((t, i) => (
                  <Tag key={i}>
                    {t.kind}
                    {"any_of" in t ? `: ${(t as { any_of: string[] }).any_of.join(", ")}` : ""}
                    {"pattern" in t ? `: ${(t as { pattern: string }).pattern}` : ""}
                  </Tag>
                ))}
              </Descriptions.Item>
            ) : null}
            {manifest.allowed_tools?.length ? (
              <Descriptions.Item label="Allowed Tools" span={2}>
                {manifest.allowed_tools.map((t, i) => (
                  <Tag key={i}>{t.kind}{("op" in t) ? `.${(t as { op: string }).op}` : ""}</Tag>
                ))}
              </Descriptions.Item>
            ) : null}
            <Descriptions.Item label="Path" span={2}>
              <Text code style={{ fontSize: 12 }}>{record.path}</Text>
            </Descriptions.Item>
            <Descriptions.Item label="Checksum" span={2}>
              <Text code style={{ fontSize: 12 }}>{record.checksum || "-"}</Text>
            </Descriptions.Item>
          </Descriptions>

          {detail?.skill_md ? (
            <>
              <Text strong style={{ display: "block", marginBottom: 8 }}>SKILL.md</Text>
              <Paragraph>
                <pre style={{
                  background: "var(--ant-color-fill-tertiary, #f5f5f5)",
                  padding: 12,
                  borderRadius: 6,
                  fontSize: 12,
                  maxHeight: 400,
                  overflow: "auto",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}>{detail.skill_md}</pre>
              </Paragraph>
            </>
          ) : null}

          {entrypoint?.kind === "inline" && entrypoint.instructions_md ? (
            <>
              <Text strong style={{ display: "block", marginBottom: 8 }}>Instructions</Text>
              <Paragraph>
                <pre style={{
                  background: "var(--ant-color-fill-tertiary, #f5f5f5)",
                  padding: 12,
                  borderRadius: 6,
                  fontSize: 12,
                  maxHeight: 300,
                  overflow: "auto",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}>{entrypoint.instructions_md}</pre>
              </Paragraph>
            </>
          ) : null}
        </>
      ) : null}
    </Modal>
  );
}

function scopeColor(scope: string) {
  if (scope === "repo") return "blue";
  if (scope === "user") return "green";
  if (scope === "global") return "orange";
  return "purple";
}
