import { useEffect, useState } from "react";
import { Button, Form, Input, Modal, Space, message } from "antd";
import { patchSkill } from "../../../../api/agent-skills";
import type { SkillRecord } from "./types";

type Props = {
  record: SkillRecord | null;
  open: boolean;
  onClose: () => void;
  onSaved: (record: SkillRecord) => void;
};

type FormValues = {
  name: string;
  description: string;
};

export default function SkillEditor({ record, open, onClose, onSaved }: Props) {
  const [form] = Form.useForm<FormValues>();
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!record || !open) {
      return;
    }
    form.setFieldsValue({
      name: record.name,
      description: record.description,
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
        manifest_overrides: {
          ...record.manifest,
          name: values.name,
          description: values.description,
        },
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

  return (
    <Modal
      title={record ? `Edit ${record.name}` : "Edit Skill"}
      open={open}
      onCancel={onClose}
      width={560}
      footer={
        <Space>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="primary" onClick={save} loading={saving}>
            Save
          </Button>
        </Space>
      }
    >
      {record ? (
        <Form form={form} layout="vertical">
          <Form.Item name="name" label="Name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="description" label="Description" rules={[{ required: true, max: 1024 }]}>
            <Input.TextArea rows={4} />
          </Form.Item>
        </Form>
      ) : null}
    </Modal>
  );
}
