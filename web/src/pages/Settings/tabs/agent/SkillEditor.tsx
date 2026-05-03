import { useEffect, useState } from "react";
import { Button, Form, Modal, Space, message } from "antd";
import { patchSkill } from "../../../../api/agent-skills";
import type { SkillRecord } from "./types";
import {
  buildSkillMd,
  ManifestFormSection,
  ScriptEditorSection,
  TestPanel,
  type SkillFormValues,
} from "./SkillCreatorWizard";

type Props = {
  record: SkillRecord | null;
  open: boolean;
  onClose: () => void;
  onSaved: (record: SkillRecord) => void;
};

export default function SkillEditor({ record, open, onClose, onSaved }: Props) {
  const [form] = Form.useForm<SkillFormValues>();
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!record || !open) {
      return;
    }
    form.setFieldsValue({
      name: record.name,
      version: record.manifest.version,
      description: record.description,
      scope: record.manifest.scope,
      slash_command: record.manifest.slash_command || undefined,
      trigger_keywords: "",
      entrypoint_kind: record.manifest.entrypoint.kind,
      script:
        record.manifest.entrypoint.kind === "inline"
          ? record.manifest.entrypoint.instructions_md
          : "",
      shell:
        record.manifest.entrypoint.kind === "shell"
          ? record.manifest.entrypoint.shell
          : "bash",
      inputs_schema: JSON.stringify(record.manifest.inputs_schema || { type: "object" }, null, 2),
      test_inputs: "{}",
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
          version: values.version,
          description: values.description,
          scope: values.scope,
          slash_command: values.slash_command || null,
          entrypoint:
            values.entrypoint_kind === "inline"
              ? { kind: "inline", instructions_md: values.script }
              : values.entrypoint_kind === "shell"
                ? { kind: "shell", script: "scripts/run.sh", shell: values.shell }
                : values.entrypoint_kind === "python"
                  ? { kind: "python", script: "scripts/run.py", python: null }
                  : { kind: "node", script: "scripts/run.js" },
        },
        skill_md: buildSkillMd(
          {
            ...record.manifest,
            name: values.name,
            version: values.version,
            description: values.description,
            scope: values.scope,
          },
          values,
        ),
        assets:
          values.entrypoint_kind === "inline"
            ? []
            : [
                {
                  path:
                    values.entrypoint_kind === "shell"
                      ? "scripts/run.sh"
                      : values.entrypoint_kind === "python"
                        ? "scripts/run.py"
                        : "scripts/run.js",
                  content: values.script,
                },
              ],
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
          <ManifestFormSection />
          <ScriptEditorSection />
          <TestPanel testReport={null} />
        </Form>
      ) : null}
    </Modal>
  );
}
