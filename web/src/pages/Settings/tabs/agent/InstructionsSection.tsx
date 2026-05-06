/**
 * Instructions Section - Display AGENTS.md content from project root hierarchy
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Col,
  Empty,
  Row,
  Space,
  Typography,
  theme,
} from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import { get } from "../../../../api/client";
import { BASE } from "./types";

const { Text } = Typography;

export default function InstructionsSection() {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const { token } = theme.useToken();

  const fetchInstructions = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{
        content: string | null;
        work_dir: string;
      }>(`${BASE}/agent/instructions`);
      setContent(data.content);
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchInstructions();
  }, [fetchInstructions]);

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Text type="secondary" style={{ fontSize: 12 }}>
            AGENTS.md from project root hierarchy
          </Text>
        </Col>
        <Col>
          <Button
            icon={<ReloadOutlined />}
            size="small"
            onClick={fetchInstructions}
            loading={loading}
          >
            Refresh
          </Button>
        </Col>
      </Row>

      {content ? (
        <div
          style={{
            background: token.colorFillQuaternary,
            borderRadius: 6,
            padding: 12,
            maxHeight: 300,
            overflowY: "auto",
          }}
        >
          <pre style={{ margin: 0, fontSize: 11, whiteSpace: "pre-wrap", color: token.colorText }}>
            {content}
          </pre>
        </div>
      ) : (
        <Empty description="No AGENTS.md found" />
      )}
    </Space>
  );
}
