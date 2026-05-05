import { useEffect, useMemo, useState } from "react";
import { Button, Input, Modal, Space, Typography, theme } from "antd";
import { EditOutlined, EyeOutlined } from "@ant-design/icons";

const { Text, Paragraph } = Typography;
const { TextArea } = Input;

interface LongTextModalFieldProps {
  value?: string | null;
  onChange?: (value: string) => void;
  label: string;
  placeholder?: string;
  description?: string;
  readOnly?: boolean;
  testId?: string;
  copyPlaceholderLabel?: string;
}

function previewText(value: string | null | undefined, placeholder?: string): string {
  const normalized = value?.trim();
  if (normalized) return normalized;
  return placeholder || "Empty";
}

export default function LongTextModalField({
  value,
  onChange,
  label,
  placeholder,
  description,
  readOnly = false,
  testId,
  copyPlaceholderLabel,
}: LongTextModalFieldProps) {
  const { token } = theme.useToken();
  const normalizedValue = value ?? "";
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(normalizedValue);

  useEffect(() => {
    if (!open) {
      setDraft(normalizedValue);
    }
  }, [normalizedValue, open]);

  const preview = useMemo(
    () => previewText(normalizedValue, placeholder),
    [normalizedValue, placeholder],
  );

  const handleOpen = () => {
    setDraft(normalizedValue);
    setOpen(true);
  };

  const handleSave = () => {
    onChange?.(draft);
    setOpen(false);
  };

  return (
    <Space direction="vertical" size={6} style={{ width: "100%" }} data-testid={testId}>
      <Space style={{ width: "100%", justifyContent: "space-between" }} align="center">
        <Text>{label}</Text>
        <Button
          icon={readOnly ? <EyeOutlined /> : <EditOutlined />}
          onClick={handleOpen}
          size="small"
          data-testid={testId ? `${testId}-button` : undefined}
        >
          {readOnly ? "View" : "Edit"}
        </Button>
      </Space>
      <div
        style={{
          background: token.colorFillQuaternary,
          border: `1px solid ${token.colorBorderSecondary}`,
          borderRadius: 6,
          color: normalizedValue ? token.colorText : token.colorTextTertiary,
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
          fontSize: 12,
          lineHeight: 1.6,
          maxHeight: 88,
          overflow: "hidden",
          padding: "8px 10px",
          whiteSpace: "pre-wrap",
        }}
        data-testid={testId ? `${testId}-preview` : undefined}
      >
        {preview}
      </div>
      {description && (
        <Text type="secondary" style={{ fontSize: 12 }}>
          {description}
        </Text>
      )}
      <Modal
        title={label}
        open={open}
        onCancel={() => setOpen(false)}
        onOk={readOnly ? undefined : handleSave}
        footer={readOnly ? null : undefined}
        width={960}
        destroyOnHidden
      >
        {description && (
          <Paragraph type="secondary" style={{ marginTop: 0 }}>
            {description}
          </Paragraph>
        )}
        {!readOnly && copyPlaceholderLabel && placeholder && (
          <Button
            size="small"
            onClick={() => setDraft(placeholder)}
            style={{ marginBottom: 8 }}
            data-testid={testId ? `${testId}-copy-placeholder` : undefined}
          >
            {copyPlaceholderLabel}
          </Button>
        )}
        <TextArea
          value={readOnly ? normalizedValue : draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder={placeholder}
          readOnly={readOnly}
          rows={20}
          style={{
            fontFamily:
              "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
            fontSize: 12,
          }}
          data-testid={testId ? `${testId}-modal-textarea` : undefined}
        />
      </Modal>
    </Space>
  );
}
