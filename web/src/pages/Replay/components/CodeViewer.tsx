import { useMemo, useCallback, type CSSProperties } from "react";
import { Button, Tooltip, message, Segmented, Select, theme } from "antd";
import {
  CopyOutlined,
  FormatPainterOutlined,
} from "@ant-design/icons";
import hljs from "highlight.js/lib/core";
import { useI18n } from "../../../i18n";
import json from "highlight.js/lib/languages/json";
import xml from "highlight.js/lib/languages/xml";
import javascript from "highlight.js/lib/languages/javascript";
import { copyToClipboard } from "../../../utils/clipboard";
import css from "highlight.js/lib/languages/css";
import plaintext from "highlight.js/lib/languages/plaintext";
import "../../../styles/hljs-github-theme.css";
import { useReplayStore, type ResponseViewMode, type ResponseContentType } from "../../../stores/useReplayStore";

hljs.registerLanguage("json", json);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("css", css);
hljs.registerLanguage("plaintext", plaintext);

type ViewMode = ResponseViewMode;
type ContentType = ResponseContentType;

interface CodeViewerProps {
  content: string;
  contentType?: ContentType;
  showToolbar?: boolean;
  maxHeight?: number | string;
  showLineNumbers?: boolean;
}

function detectContentType(content: string): ContentType {
  const trimmed = content.trim();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      JSON.parse(trimmed);
      return "json";
    } catch {
      return "text";
    }
  }
  if (trimmed.startsWith("<?xml") || trimmed.startsWith("<") && trimmed.endsWith(">")) {
    if (trimmed.toLowerCase().includes("<!doctype html") || trimmed.toLowerCase().includes("<html")) {
      return "html";
    }
    return "xml";
  }
  return "text";
}

function formatContent(content: string, contentType: ContentType): string {
  if (contentType === "json") {
    try {
      const parsed = JSON.parse(content);
      return JSON.stringify(parsed, null, 2);
    } catch {
      return content;
    }
  }
  return content;
}

export default function CodeViewer({
  content,
  contentType: propContentType,
  showToolbar = true,
  maxHeight,
  showLineNumbers = true,
}: CodeViewerProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const { uiState, updateUIState } = useReplayStore();
  const viewMode = uiState.responseViewMode;
  const selectedType = uiState.responseContentType;

  const setViewMode = useCallback((mode: ViewMode) => {
    updateUIState({ responseViewMode: mode });
  }, [updateUIState]);

  const setSelectedType = useCallback((type: ContentType | null) => {
    updateUIState({ responseContentType: type });
  }, [updateUIState]);

  const contentType = useMemo(() => {
    return selectedType || propContentType || detectContentType(content);
  }, [content, propContentType, selectedType]);

  const displayContent = useMemo(() => {
    if (viewMode === "raw") {
      return content;
    }
    return formatContent(content, contentType);
  }, [content, contentType, viewMode]);

  const highlighted = useMemo(() => {
    if (!displayContent || viewMode === "preview") return "";
    try {
      const lang = contentType === "text" ? "plaintext" : contentType;
      if (displayContent.length > 500 * 1024) {
        return displayContent;
      }
      const result = hljs.highlight(displayContent, { language: lang });
      return result.value;
    } catch {
      return displayContent;
    }
  }, [displayContent, contentType, viewMode]);

  const lines = useMemo(() => {
    if (!highlighted) return [];
    return highlighted.split("\n");
  }, [highlighted]);

  const handleCopy = useCallback(async () => {
    try {
      await copyToClipboard(content);
      message.success(t("replay.code.copiedToClipboard"));
    } catch {
      message.error(t("replay.code.copyFailed"));
    }
  }, [content, t]);

  const styles: Record<string, CSSProperties> = {
    container: {
      display: 'flex',
      flexDirection: 'column',
      height: '100%',
      overflow: 'hidden',
    },
    toolbar: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '4px 8px',
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
      backgroundColor: token.colorBgLayout,
      gap: 12,
    },
    toolbarLeft: {
      display: 'flex',
      alignItems: 'center',
      gap: 12,
    },
    toolbarRight: {
      display: 'flex',
      alignItems: 'center',
      gap: 4,
    },
    codeContainer: {
      flex: 1,
      overflow: 'auto',
      backgroundColor: token.colorBgLayout,
      maxHeight: maxHeight || 'auto',
    },
    codeWrapper: {
      display: 'flex',
      minHeight: '100%',
    },
    lineNumbers: {
      flexShrink: 0,
      padding: '8px 0',
      backgroundColor: token.colorBgContainer,
      borderRight: `1px solid ${token.colorBorderSecondary}`,
      userSelect: 'none',
      minWidth: 40,
      textAlign: 'right',
    },
    lineNumber: {
      padding: '0 8px',
      fontFamily: 'Monaco, Menlo, Ubuntu Mono, Consolas, monospace',
      fontSize: 12,
      lineHeight: '20px',
      color: token.colorTextTertiary,
    },
    code: {
      flex: 1,
      margin: 0,
      padding: 8,
      fontFamily: 'Monaco, Menlo, Ubuntu Mono, Consolas, monospace',
      fontSize: 12,
      lineHeight: '20px',
      overflowX: 'auto',
      backgroundColor: 'transparent',
      color: token.colorText,
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-all',
    },
    codeLine: {
      minHeight: 20,
    },
    previewFrame: {
      width: '100%',
      height: '100%',
      minHeight: 300,
      border: 'none',
      backgroundColor: '#fff',
    },
    previewFallback: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      height: '100%',
      minHeight: 200,
      color: token.colorTextTertiary,
      fontSize: 12,
    },
    empty: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      padding: 40,
      color: token.colorTextTertiary,
      fontSize: 12,
      backgroundColor: token.colorBgLayout,
      borderRadius: 4,
    },
  };

  const renderPreview = () => {
    if (contentType === "html") {
      return (
        <iframe
          srcDoc={content}
          style={styles.previewFrame}
          sandbox="allow-same-origin"
          title={t("replay.code.preview")}
        />
      );
    }
    return (
      <div style={styles.previewFallback}>
        {t("replay.code.previewUnavailable")}
      </div>
    );
  };

  if (!content) {
    return (
      <div style={styles.empty}>
        {t("replay.code.noContent")}
      </div>
    );
  }

  return (
    <div style={styles.container}>
      {showToolbar && (
        <div style={styles.toolbar}>
          <div style={styles.toolbarLeft}>
            <Segmented
              size="small"
              value={viewMode}
              onChange={(v) => setViewMode(v as ViewMode)}
              options={[
                { label: t("replay.code.pretty"), value: "pretty" },
                { label: t("replay.code.raw"), value: "raw" },
                { label: t("replay.code.preview"), value: "preview" },
              ]}
            />
            <Select
              size="small"
              value={contentType}
              onChange={setSelectedType}
              style={{ width: 100 }}
              options={[
                { label: "JSON", value: "json" },
                { label: "XML", value: "xml" },
                { label: "HTML", value: "html" },
                { label: "JavaScript", value: "javascript" },
                { label: "CSS", value: "css" },
                { label: t("replay.code.text"), value: "text" },
              ]}
            />
          </div>
          <div style={styles.toolbarRight}>
            {contentType === "json" && (
              <Tooltip title={viewMode === "pretty" ? t("replay.code.raw") : t("replay.code.format")}>
                <Button
                  type="text"
                  size="small"
                  icon={<FormatPainterOutlined />}
                  onClick={() => setViewMode(viewMode === "pretty" ? "raw" : "pretty")}
                  style={{ opacity: viewMode === "pretty" ? 1 : 0.6 }}
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
          </div>
        </div>
      )}

      <div style={styles.codeContainer}>
        {viewMode === "preview" ? (
          renderPreview()
        ) : (
          <div style={styles.codeWrapper}>
            {showLineNumbers && (
              <div style={styles.lineNumbers}>
                {lines.map((_, index) => (
                  <div key={index} style={styles.lineNumber}>
                    {index + 1}
                  </div>
                ))}
              </div>
            )}
            <pre style={styles.code}>
              <code className="hljs">
                {lines.map((line, index) => (
                  <div
                    key={index}
                    style={styles.codeLine}
                    dangerouslySetInnerHTML={{ __html: line || " " }}
                  />
                ))}
              </code>
            </pre>
          </div>
        )}
      </div>
    </div>
  );
}
