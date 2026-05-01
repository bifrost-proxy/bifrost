/**
 * Skills Section - Display loaded skills from workspace and global directories
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Col,
  Empty,
  Row,
  Space,
  Table,
  Tooltip,
  Typography,
} from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import { get } from "../../../../api/client";
import { BASE, type SkillInfo } from "./types";

const { Text } = Typography;

export default function SkillsSection() {
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [workDir, setWorkDir] = useState<string>("");

  const fetchSkills = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{
        skills: SkillInfo[];
        work_dir: string;
        home_dir: string;
      }>(`${BASE}/agent/skills`);
      setSkills(data.skills || []);
      setWorkDir(data.work_dir || "");
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSkills();
  }, [fetchSkills]);

  const repoSkills = skills.filter((s) => s.scope === "Repo");
  const userSkills = skills.filter((s) => s.scope === "User");
  const systemSkills = skills.filter((s) => s.scope === "System");

  const columns = [
    {
      title: "Name",
      dataIndex: "name",
      key: "name",
      render: (val: string) => <Text strong>{val}</Text>,
    },
    {
      title: "Description",
      dataIndex: "short_description",
      key: "description",
      ellipsis: true,
      render: (val: string | undefined, record: SkillInfo) => val || record.description,
    },
    {
      title: "Path",
      dataIndex: "path",
      key: "path",
      ellipsis: true,
      render: (val: string) => (
        <Tooltip title={val}>
          <Text code style={{ fontSize: 10 }}>
            {val}
          </Text>
        </Tooltip>
      ),
    },
  ];

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {skills.length} skill{skills.length !== 1 ? "s" : ""} loaded
          </Text>
        </Col>
        <Col>
          <Button
            icon={<ReloadOutlined />}
            size="small"
            onClick={fetchSkills}
            loading={loading}
          >
            Refresh
          </Button>
        </Col>
      </Row>

      {repoSkills.length > 0 && (
        <div>
          <Text type="secondary" style={{ fontSize: 11 }}>
            Workspace ({workDir}/.agents/skills/)
          </Text>
          <Table
            dataSource={repoSkills}
            columns={columns}
            rowKey="name"
            size="small"
            pagination={false}
            style={{ marginTop: 4 }}
          />
        </div>
      )}

      {userSkills.length > 0 && (
        <div>
          <Text type="secondary" style={{ fontSize: 11 }}>
            Global (~/.agents/skills/)
          </Text>
          <Table
            dataSource={userSkills}
            columns={columns}
            rowKey="name"
            size="small"
            pagination={false}
            style={{ marginTop: 4 }}
          />
        </div>
      )}

      {systemSkills.length > 0 && (
        <div>
          <Text type="secondary" style={{ fontSize: 11 }}>
            System
          </Text>
          <Table
            dataSource={systemSkills}
            columns={columns}
            rowKey="name"
            size="small"
            pagination={false}
            style={{ marginTop: 4 }}
          />
        </div>
      )}

      {skills.length === 0 && !loading && (
        <Empty description="No skills loaded" />
      )}
    </Space>
  );
}
