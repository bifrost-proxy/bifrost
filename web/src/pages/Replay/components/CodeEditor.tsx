import { useEffect, useRef, useCallback, type CSSProperties } from "react";
import { editor as MonacoEditor } from "monaco-editor";
import { theme, Button, Tooltip, Space, message } from "antd";
import { FormatPainterOutlined, CopyOutlined } from "@ant-design/icons";
import { useThemeStore } from "../../../stores/useThemeStore";
import { copyToClipboard } from "../../../utils/clipboard";
import { useI18n } from "../../../i18n";

interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  language?: "json" | "xml" | "plaintext";
  placeholder?: string;
  minHeight?: number;
}

function formatJSON(content: string): string {
  try {
    const parsed = JSON.parse(content);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return content;
  }
}

function formatXML(content: string): string {
  try {
    let formatted = "";
    let indent = 0;
    const pad = "  ";
    content.split(/>\s*</).forEach((node, index) => {
      if (node.match(/^\/\w/)) {
        indent--;
      }
      formatted += (index > 0 ? "\n" : "") + pad.repeat(Math.max(0, indent)) + (index > 0 ? "<" : "") + node + (index < content.split(/>\s*</).length - 1 ? ">" : "");
      if (node.match(/^<?\w[^>]*[^/]$/) && !node.startsWith("?") && !node.startsWith("!")) {
        indent++;
      }
    });
    return formatted;
  } catch {
    return content;
  }
}

export default function CodeEditor({
  value,
  onChange,
  language = "json",
  placeholder: placeholderProp,
  minHeight = 200,
}: CodeEditorProps) {
  const { t } = useI18n();
  const placeholder = placeholderProp ?? t("replay.code.placeholder");
  const { token } = theme.useToken();
  const { resolvedTheme } = useThemeStore();
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null);
  const isSettingValueRef = useRef(false);
  const onChangeRef = useRef(onChange);
  const initialValueRef = useRef(value);

  onChangeRef.current = onChange;

  useEffect(() => {
    if (!containerRef.current) return;

    const ed = MonacoEditor.create(containerRef.current, {
      value: initialValueRef.current,
      language,
      theme: resolvedTheme === "dark" ? "vs-dark" : "vs",
      minimap: { enabled: false },
      automaticLayout: true,
      scrollBeyondLastLine: false,
      fontSize: 12,
      lineHeight: 20,
      tabSize: 2,
      wordWrap: "on",
      lineNumbers: "on",
      folding: true,
      renderLineHighlight: "all",
      scrollbar: {
        vertical: "auto",
        horizontal: "auto",
        verticalScrollbarSize: 8,
        horizontalScrollbarSize: 8,
      },
      padding: { top: 8, bottom: 8 },
      placeholder,
    });

    ed.onDidChangeModelContent(() => {
      if (isSettingValueRef.current) return;
      const newValue = ed.getValue();
      onChangeRef.current(newValue);
    });

    editorRef.current = ed;

    return () => {
      ed.dispose();
      editorRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!editorRef.current) return;
    const currentValue = editorRef.current.getValue();
    if (currentValue !== value) {
      isSettingValueRef.current = true;
      editorRef.current.setValue(value);
      isSettingValueRef.current = false;
    }
  }, [value]);

  useEffect(() => {
    if (!editorRef.current) return;
    const model = editorRef.current.getModel();
    if (model) {
      MonacoEditor.setModelLanguage(model, language);
    }
  }, [language]);

  useEffect(() => {
    if (!editorRef.current) return;
    editorRef.current.updateOptions({
      theme: resolvedTheme === "dark" ? "vs-dark" : "vs",
    });
  }, [resolvedTheme]);

  useEffect(() => {
    editorRef.current?.updateOptions({ placeholder });
  }, [placeholder]);

  const handleFormat = useCallback(() => {
    if (!editorRef.current) return;
    const currentValue = editorRef.current.getValue();
    if (!currentValue.trim()) return;

    let formatted = currentValue;
    if (language === "json") {
      formatted = formatJSON(currentValue);
    } else if (language === "xml") {
      formatted = formatXML(currentValue);
    }

    if (formatted !== currentValue) {
      isSettingValueRef.current = true;
      editorRef.current.setValue(formatted);
      isSettingValueRef.current = false;
      onChangeRef.current(formatted);
      message.success(t("replay.code.formatted"));
    }
  }, [language, t]);

  const handleCopy = useCallback(async () => {
    if (!editorRef.current) return;
    const currentValue = editorRef.current.getValue();
    try {
      await copyToClipboard(currentValue);
      message.success(t("replay.code.copied"));
    } catch {
      message.error(t("replay.code.copyFailed"));
    }
  }, [t]);

  const styles: Record<string, CSSProperties> = {
    wrapper: {
      display: "flex",
      flexDirection: "column",
      height: "100%",
      minHeight,
      border: `1px solid ${token.colorBorderSecondary}`,
      borderRadius: 4,
      overflow: "hidden",
    },
    toolbar: {
      display: "flex",
      alignItems: "center",
      justifyContent: "flex-end",
      padding: "4px 8px",
      backgroundColor: token.colorBgLayout,
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
    },
    container: {
      flex: 1,
      width: "100%",
      overflow: "hidden",
    },
  };

  const canFormat = language === "json" || language === "xml";

  return (
    <div style={styles.wrapper}>
      <div style={styles.toolbar}>
        <Space size={4}>
          {canFormat && (
            <Tooltip title={t("replay.code.format")}>
              <Button
                type="text"
                size="small"
                icon={<FormatPainterOutlined />}
                onClick={handleFormat}
              />
            </Tooltip>
          )}
          <Tooltip title={t("common.copy")}>
            <Button
              type="text"
              size="small"
              icon={<CopyOutlined />}
              onClick={handleCopy}
            />
          </Tooltip>
        </Space>
      </div>
      <div ref={containerRef} style={styles.container} />
    </div>
  );
}
