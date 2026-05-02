import { useEffect, useState } from "react";
import { Alert, Button, Form, Input, Modal, Space, Typography, message } from "antd";
import { getSkill, patchSkill, testSkill } from "../../../../api/agent-skills";
import type { JsonValue, SkillRecord, SkillTestReport } from "./types";

const { Text } = Typography;

type Props = {
  record: SkillRecord | null;
  open: boolean;
  onClose: () => void;
  onSaved: (record: SkillRecord) => void;
};

type FormValues = {
  skill_md: string;
  test_inputs: string;
};

export default function SkillEditor({ record, open, onClose, onSaved }: Props) {
  const [form] = Form.useForm<FormValues>();
  const [saving, setSaving] = useState(false);
  const [testReport, setTestReport] = useState<SkillTestReport | null>(null);

  useEffect(() => {
    if (!record || !open) {
      return;
    }
    getSkill(record.name).then((detail) => {
      form.setFieldsValue({ skill_md: detail.skill_md, test_inputs: "{}" });
    });
  }, [form, open, record]);

  const save = async () => {
    if (!record) {
      return;
    }
    const values = await form.validateFields();
    setSaving(true);
    try {
      const result = await patchSkill(record.name, {
        manifest_overrides: record.manifest,
        skill_md: values.skill_md,
      });
      if (result.record) {
        onSaved(result.record);
      }
      message.success("Skill saved");
      onClose();
    } finally {
      setSaving(false);
    }
  };

  const runTest = async () => {
    if (!record) {
      return;
    }
    const inputs = JSON.parse(form.getFieldValue("test_inputs") || "{}") as JsonValue;
    const report = await testSkill(record.name, inputs);
    setTestReport(report);
  };

  return (
    <Modal
      title={record ? `Edit ${record.name}` : "Edit Skill"}
      open={open}
      onCancel={onClose}
      width={860}
      footer={
        <Space>
          <Button onClick={runTest}>Run Test</Button>
          <Button type="primary" onClick={save} loading={saving}>
            Save
          </Button>
        </Space>
      }
    >
      {record ? (
        <Space direction="vertical" style={{ width: "100%" }}>
          <Text type="secondary">Checksum: {record.checksum || "pending"}</Text>
          <Form form={form} layout="vertical">
            <Form.Item name="skill_md" label="SKILL.md" rules={[{ required: true }]}>
              <Input.TextArea rows={14} />
            </Form.Item>
            <Form.Item name="test_inputs" label="Test Inputs">
              <Input.TextArea rows={5} />
            </Form.Item>
          </Form>
          {testReport ? (
            <Alert
              type={testReport.exit_code === 0 ? "success" : "warning"}
              message={`Exit ${testReport.exit_code ?? "unknown"} in ${testReport.duration_ms}ms`}
              description={<Text code>{testReport.stdout || testReport.stderr}</Text>}
            />
          ) : null}
        </Space>
      ) : null}
    </Modal>
  );
}
