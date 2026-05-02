import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Empty,
  Input,
  Popconfirm,
  Segmented,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import {
  DeleteOutlined,
  EditOutlined,
  ImportOutlined,
  PlusOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import {
  deleteSkill,
  listSkills,
  patchSkill,
} from "../../../../api/agent-skills";
import type { SkillRecord, SkillScope } from "./types";
import SkillCreatorWizard from "./SkillCreatorWizard";
import SkillEditor from "./SkillEditor";

const { Text } = Typography;

type Filter = "all" | "enabled" | SkillScope;

export default function SkillsSection() {
  const [skills, setSkills] = useState<SkillRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [wizardOpen, setWizardOpen] = useState(false);
  const [editing, setEditing] = useState<SkillRecord | null>(null);

  const fetchSkills = useCallback(async () => {
    setLoading(true);
    try {
      const data = await listSkills();
      setSkills(data.skills || []);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSkills();
  }, [fetchSkills]);

  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return skills.filter((skill) => {
      if (filter === "enabled" && !skill.enabled) {
        return false;
      }
      if (filter !== "all" && filter !== "enabled" && skill.effective_scope !== filter) {
        return false;
      }
      if (!normalized) {
        return true;
      }
      return (
        skill.name.toLowerCase().includes(normalized) ||
        skill.description.toLowerCase().includes(normalized) ||
        (skill.manifest?.slash_command ?? "").toLowerCase().includes(normalized)
      );
    });
  }, [filter, query, skills]);

  const updateRecord = (record: SkillRecord) => {
    setSkills((current) => {
      const rest = current.filter((item) => item.name !== record.name);
      return [record, ...rest];
    });
  };

  const toggleEnabled = async (record: SkillRecord, enabled: boolean) => {
    const result = await patchSkill(record.name, { enabled });
    if (result.record) {
      updateRecord(result.record);
    } else {
      fetchSkills();
    }
  };

  const remove = async (record: SkillRecord) => {
    await deleteSkill(record.name, record.effective_scope);
    setSkills((current) => current.filter((item) => item.name !== record.name));
  };

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Space wrap style={{ width: "100%", justifyContent: "space-between" }}>
        <Space wrap>
          <Segmented
            size="small"
            value={filter}
            onChange={(value) => setFilter(value as Filter)}
            options={[
              { label: "All", value: "all" },
              { label: "Enabled", value: "enabled" },
              { label: "Repo", value: "repo" },
              { label: "User", value: "user" },
              { label: "System", value: "system" },
            ]}
          />
          <Input.Search
            allowClear
            size="small"
            placeholder="Search skills"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            style={{ width: 220 }}
          />
        </Space>
        <Space>
          <Tooltip title="Import .skill">
            <Button size="small" icon={<ImportOutlined />} disabled />
          </Tooltip>
          <Button size="small" icon={<ReloadOutlined />} onClick={fetchSkills} loading={loading}>
            Refresh
          </Button>
          <Button size="small" type="primary" icon={<PlusOutlined />} onClick={() => setWizardOpen(true)}>
            New Skill
          </Button>
        </Space>
      </Space>

      <Table
        dataSource={filtered}
        rowKey={(record) => `${record.effective_scope}:${record.name}`}
        loading={loading}
        size="small"
        pagination={false}
        locale={{ emptyText: <Empty description="No skills" /> }}
        columns={[
          {
            title: "",
            width: 52,
            render: (_: unknown, record) => {
              const isSystem = record.effective_scope === "system";
              return (
                <Tooltip title={isSystem ? "System skills cannot be disabled" : undefined}>
                  <Switch
                    size="small"
                    checked={record.enabled}
                    disabled={isSystem}
                    onChange={(checked) => toggleEnabled(record, checked)}
                  />
                </Tooltip>
              );
            },
          },
          {
            title: "Skill",
            render: (_: unknown, record) => (
              <Space direction="vertical" size={0}>
                <Space>
                  <Text strong>{record.name}</Text>
                  <Tag>v{record.version}</Tag>
                  <Tag color={scopeColor(record.effective_scope)}>{record.effective_scope}</Tag>
                  {record.manifest?.slash_command ? <Tag>{record.manifest.slash_command}</Tag> : null}
                </Space>
                <Text type="secondary">{record.description}</Text>
              </Space>
            ),
          },
          {
            title: "Actions",
            width: 120,
            render: (_: unknown, record) => {
              const isSystem = record.effective_scope === "system";
              return (
                <Space>
                  <Tooltip title="Edit">
                    <Button size="small" icon={<EditOutlined />} onClick={() => setEditing(record)} />
                  </Tooltip>
                  {isSystem ? (
                    <Tooltip title="System skills cannot be deleted">
                      <Button size="small" danger icon={<DeleteOutlined />} disabled />
                    </Tooltip>
                  ) : (
                    <Popconfirm title="Delete skill?" onConfirm={() => remove(record)}>
                      <Button size="small" danger icon={<DeleteOutlined />} />
                    </Popconfirm>
                  )}
                </Space>
              );
            },
          },
        ]}
      />

      <SkillCreatorWizard
        open={wizardOpen}
        onClose={() => setWizardOpen(false)}
        onSaved={updateRecord}
      />
      <SkillEditor
        record={editing}
        open={Boolean(editing)}
        onClose={() => setEditing(null)}
        onSaved={updateRecord}
      />
    </Space>
  );
}

function scopeColor(scope: SkillScope) {
  if (scope === "repo") {
    return "blue";
  }
  if (scope === "user") {
    return "green";
  }
  return "purple";
}
