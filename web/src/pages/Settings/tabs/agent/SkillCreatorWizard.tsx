import { useMemo, useState } from "react";
import {
  Alert,
  Button,
  Checkbox,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Steps,
  Typography,
  message,
} from "antd";
import { createSkill, testSkill } from "../../../../api/agent-skills";
import type {
  Entrypoint,
  JsonValue,
  SkillCreateRequest,
  SkillManifest,
  SkillRecord,
  SkillScope,
  SkillTestReport,
  TriggerRule,
} from "./types";

const { Text } = Typography;

type FormValues = {
  name: string;
  version: string;
  description: string;
  scope: SkillScope;
  slash_command?: string;
  trigger_keywords?: string;
  entrypoint_kind: "inline" | "shell" | "python" | "node";
  script: string;
  shell: "bash" | "sh" | "zsh" | "power_shell";
  memory_read?: boolean;
  memory_write?: boolean;
  inputs_schema: string;
  test_inputs: string;
};

type Props = {
  open: boolean;
  onClose: () => void;
  onSaved: (record: SkillRecord) => void;
};

export default function SkillCreatorWizard({ open, onClose, onSaved }: Props) {
  const [form] = Form.useForm<FormValues>();
  const [step, setStep] = useState(0);
  const [saving, setSaving] = useState(false);
  const [testReport, setTestReport] = useState<SkillTestReport | null>(null);

  const initialValues = useMemo<FormValues>(
    () => ({
      name: "",
      version: "0.1.0",
      description: "",
      scope: "repo",
      entrypoint_kind: "inline",
      script: "",
      shell: "bash",
      inputs_schema: "{\n  \"type\": \"object\"\n}",
      test_inputs: "{}",
    }),
    [],
  );

  const buildRequest = async (): Promise<SkillCreateRequest> => {
    const values = await form.validateFields();
    const inputsSchema = parseJson(values.inputs_schema, "Inputs schema");
    const triggers: TriggerRule[] = [{ kind: "description_match" }];
    const keywords = values.trigger_keywords
      ?.split(",")
      .map((item) => item.trim())
      .filter(Boolean);
    if (keywords?.length) {
      triggers.push({ kind: "keyword", any_of: keywords });
    }
    if (values.slash_command) {
      triggers.push({ kind: "slash_command" });
    }
    const entrypoint = buildEntrypoint(values);
    const manifest: SkillManifest = {
      name: values.name,
      version: values.version,
      description: values.description,
      scope: values.scope,
      entrypoint,
      allowed_tools: [
        ...(values.memory_read ? [{ kind: "memory" as const, op: "read" as const }] : []),
        ...(values.memory_write ? [{ kind: "memory" as const, op: "write" as const }] : []),
      ],
      slash_command: values.slash_command || null,
      triggers,
      inputs_schema: inputsSchema,
      outputs_schema: null,
      metadata: {},
      created_by: { user: { id: "webui" } },
      created_at_unix: Math.floor(Date.now() / 1000),
      updated_at_unix: Math.floor(Date.now() / 1000),
      checksum: "",
      schema_version: 1,
    };
    return {
      manifest,
      skill_md: buildSkillMd(manifest, values),
      assets: buildAssets(values),
    };
  };

  const runTest = async () => {
    const request = await buildRequest();
    setSaving(true);
    try {
      const created = await createSkill(request);
      const inputs = parseJson(form.getFieldValue("test_inputs") || "{}", "Test inputs");
      const report = await testSkill(created.record.name, inputs);
      setTestReport(report);
      message.success("Skill test passed");
      onSaved(created.record);
    } finally {
      setSaving(false);
    }
  };

  const save = async () => {
    const request = await buildRequest();
    setSaving(true);
    try {
      const result = await createSkill(request);
      message.success("Skill saved");
      onSaved(result.record);
      onClose();
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      title="New Skill"
      open={open}
      onCancel={onClose}
      width={860}
      footer={
        <Space>
          <Button disabled={step === 0} onClick={() => setStep((value) => value - 1)}>
            Back
          </Button>
          {step < 3 ? (
            <Button type="primary" onClick={() => setStep((value) => value + 1)}>
              Next
            </Button>
          ) : (
            <>
              <Button onClick={runTest} loading={saving}>
                Run Test
              </Button>
              <Button type="primary" onClick={save} loading={saving}>
                Save
              </Button>
            </>
          )}
        </Space>
      }
    >
      <Steps
        size="small"
        current={step}
        items={[
          { title: "Metadata" },
          { title: "Entrypoint" },
          { title: "Tools" },
          { title: "Test" },
        ]}
        style={{ marginBottom: 16 }}
      />
      <Form form={form} layout="vertical" initialValues={initialValues}>
        <div style={{ display: step === 0 ? "block" : "none" }}>
          <Form.Item
            name="name"
            label="Name"
            rules={[
              { required: true },
              { pattern: /^[a-z][a-z0-9-]{0,63}$/, message: "Use kebab-case" },
            ]}
          >
            <Input placeholder="weather-lookup" />
          </Form.Item>
          <Form.Item name="description" label="Description" rules={[{ required: true, max: 1024 }]}>
            <Input.TextArea rows={3} />
          </Form.Item>
          <Space.Compact style={{ width: "100%" }}>
            <Form.Item name="version" label="Version" rules={[{ required: true }]} style={{ width: "33%" }}>
              <Input />
            </Form.Item>
            <Form.Item name="scope" label="Scope" rules={[{ required: true }]} style={{ width: "33%" }}>
              <Select
                options={[
                  { value: "repo", label: "Repo" },
                  { value: "user", label: "User" },
                  { value: "system", label: "System" },
                ]}
              />
            </Form.Item>
            <Form.Item
              name="slash_command"
              label="Slash Command"
              rules={[{ pattern: /^\/[a-z][a-z0-9-]{0,31}$/, message: "Use /kebab-case" }]}
              style={{ width: "34%" }}
            >
              <Input placeholder="/weather" />
            </Form.Item>
          </Space.Compact>
          <Form.Item name="trigger_keywords" label="Keywords">
            <Input placeholder="weather, 天气" />
          </Form.Item>
        </div>
        <div style={{ display: step === 1 ? "block" : "none" }}>
          <Form.Item name="entrypoint_kind" label="Kind" rules={[{ required: true }]}>
            <Select
              options={[
                { value: "inline", label: "Inline" },
                { value: "shell", label: "Shell" },
                { value: "python", label: "Python" },
                { value: "node", label: "Node" },
              ]}
            />
          </Form.Item>
          <Form.Item name="shell" label="Shell">
            <Select
              options={[
                { value: "bash", label: "Bash" },
                { value: "sh", label: "sh" },
                { value: "zsh", label: "zsh" },
              ]}
            />
          </Form.Item>
          <Form.Item name="script" label="Instructions or Script" rules={[{ required: true }]}>
            <Input.TextArea rows={10} />
          </Form.Item>
        </div>
        <div style={{ display: step === 2 ? "block" : "none" }}>
          <Space direction="vertical">
            <Form.Item name="memory_read" valuePropName="checked">
              <Checkbox>Allow memory.read</Checkbox>
            </Form.Item>
            <Form.Item name="memory_write" valuePropName="checked">
              <Checkbox>Allow memory.write</Checkbox>
            </Form.Item>
          </Space>
          <Form.Item name="inputs_schema" label="Inputs Schema" rules={[{ validator: validateJson }]}>
            <Input.TextArea rows={8} />
          </Form.Item>
        </div>
        <div style={{ display: step === 3 ? "block" : "none" }}>
          <Form.Item name="test_inputs" label="Test Inputs" rules={[{ validator: validateJson }]}>
            <Input.TextArea rows={6} />
          </Form.Item>
          {testReport ? (
            <Alert
              type={testReport.exit_code === 0 ? "success" : "warning"}
              message={`Exit ${testReport.exit_code ?? "unknown"} in ${testReport.duration_ms}ms`}
              description={<Text code>{testReport.stdout || testReport.stderr}</Text>}
            />
          ) : null}
        </div>
      </Form>
    </Modal>
  );
}

function buildEntrypoint(values: FormValues): Entrypoint {
  if (values.entrypoint_kind === "inline") {
    return { kind: "inline", instructions_md: values.script };
  }
  if (values.entrypoint_kind === "shell") {
    return { kind: "shell", script: "scripts/run.sh", shell: values.shell };
  }
  if (values.entrypoint_kind === "python") {
    return { kind: "python", script: "scripts/run.py", python: null };
  }
  return { kind: "node", script: "scripts/run.js" };
}

function buildAssets(values: FormValues) {
  if (values.entrypoint_kind === "inline") {
    return [];
  }
  const path =
    values.entrypoint_kind === "shell"
      ? "scripts/run.sh"
      : values.entrypoint_kind === "python"
        ? "scripts/run.py"
        : "scripts/run.js";
  return [{ path, content: values.script }];
}

function buildSkillMd(manifest: SkillManifest, values: FormValues): string {
  return `---\nname: ${manifest.name}\nversion: ${manifest.version}\ndescription: ${manifest.description}\nscope: ${manifest.scope}\n---\n\n# ${manifest.name}\n\n${values.script}\n`;
}

function parseJson(value: string, label: string): JsonValue {
  try {
    return JSON.parse(value) as JsonValue;
  } catch (error) {
    throw new Error(`${label}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

async function validateJson(_: unknown, value?: string) {
  if (!value) {
    return;
  }
  parseJson(value, "JSON");
}
