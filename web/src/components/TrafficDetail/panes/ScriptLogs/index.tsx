import { useState, useCallback } from "react";
import { Typography, Tag, Space, Empty, theme, Modal, Spin } from "antd";
import {
  CheckCircleOutlined,
  CloseCircleOutlined,
  ClockCircleOutlined,
  CodeOutlined,
} from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import type { ScriptExecutionResult, ScriptLogEntry } from "../../../../types";
import { scriptsApi, type ScriptType } from "../../../../api/scripts";
import { useThemeStore } from "../../../../stores/useThemeStore";
import { useI18n, type TFunction } from "../../../../i18n";

const { Text } = Typography;

interface ScriptLogsPaneProps {
  results: ScriptExecutionResult[];
}

function LogLevel({ level }: { level: ScriptLogEntry["level"] }) {
  const colors: Record<string, string> = {
    debug: "default",
    info: "blue",
    warn: "orange",
    error: "red",
  };
  return (
    <Tag
      color={colors[level]}
      style={{ fontSize: 10, padding: "0 4px", lineHeight: "16px" }}
    >
      {level.toUpperCase()}
    </Tag>
  );
}

const scriptTypeLabel = (type: string, t: TFunction): string => {
  if (type === "request") return t("trafficDetail.script.typeRequest");
  if (type === "response") return t("trafficDetail.script.typeResponse");
  return type;
};

interface ScriptResultCardProps {
  result: ScriptExecutionResult;
  onScriptClick: (scriptName: string, scriptType: ScriptType) => void;
}

function ScriptResultCard({ result, onScriptClick }: ScriptResultCardProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();

  const handleScriptNameClick = useCallback(() => {
    onScriptClick(result.script_name, result.script_type as ScriptType);
  }, [result.script_name, result.script_type, onScriptClick]);

  return (
    <div
      style={{
        marginBottom: 12,
        padding: 12,
        borderRadius: 6,
        border: `1px solid ${token.colorBorderSecondary}`,
        background: token.colorBgContainer,
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 8,
        }}
      >
        <Space>
          {result.success ? (
            <CheckCircleOutlined style={{ color: token.colorSuccess }} />
          ) : (
            <CloseCircleOutlined style={{ color: token.colorError }} />
          )}
          <Text
            strong
            style={{ cursor: "pointer", color: token.colorPrimary }}
            onClick={handleScriptNameClick}
          >
            <CodeOutlined style={{ marginRight: 4 }} />
            {result.script_name}
          </Text>
          <Tag color={result.script_type === "request" ? "blue" : "green"}>
            {scriptTypeLabel(result.script_type, t)}
          </Tag>
        </Space>
        <Space>
          <ClockCircleOutlined style={{ color: token.colorTextSecondary }} />
          <Text type="secondary">{result.duration_ms}ms</Text>
        </Space>
      </div>

      {result.error && (
        <div
          style={{
            marginBottom: 8,
            padding: 8,
            borderRadius: 4,
            background: token.colorErrorBg,
            border: `1px solid ${token.colorErrorBorder}`,
          }}
        >
          <Text type="danger" style={{ fontFamily: "monospace", fontSize: 12 }}>
            {result.error}
          </Text>
        </div>
      )}

      {result.logs.length > 0 ? (
        <div
          style={{
            maxHeight: 200,
            overflow: "auto",
            fontFamily: "monospace",
            fontSize: 12,
            padding: 8,
            borderRadius: 4,
            background: token.colorFillQuaternary,
          }}
        >
          {result.logs.map((log, i) => (
            <div
              key={i}
              style={{
                display: "flex",
                alignItems: "flex-start",
                marginBottom: 4,
                lineHeight: "20px",
              }}
            >
              <LogLevel level={log.level} />
              <Text
                code
                style={{
                  marginLeft: 4,
                  marginRight: 8,
                  fontSize: 10,
                  color: token.colorTextSecondary,
                }}
              >
                {new Date(log.timestamp).toLocaleTimeString()}
              </Text>
              <Text style={{ flex: 1, wordBreak: "break-all" }}>
                {log.message}
              </Text>
            </div>
          ))}
        </div>
      ) : (
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t("trafficDetail.script.noLogs")}
        </Text>
      )}
    </div>
  );
}

export default function ScriptLogsPane({ results }: ScriptLogsPaneProps) {
  const { t } = useI18n();
  const resolvedTheme = useThemeStore((s) => s.resolvedTheme);
  const [modalVisible, setModalVisible] = useState(false);
  const [scriptContent, setScriptContent] = useState<string | null>(null);
  const [scriptLoading, setScriptLoading] = useState(false);
  const [currentScript, setCurrentScript] = useState<{
    name: string;
    type: ScriptType;
  } | null>(null);

  const handleScriptClick = useCallback(
    async (scriptName: string, scriptType: ScriptType) => {
      setCurrentScript({ name: scriptName, type: scriptType });
      setModalVisible(true);
      setScriptLoading(true);
      setScriptContent(null);

      try {
        const detail = await scriptsApi.get(scriptType, scriptName);
        setScriptContent(detail.content);
      } catch {
        setScriptContent(
          `// ${t("trafficDetail.script.loadFailed", { name: scriptName })}`,
        );
      } finally {
        setScriptLoading(false);
      }
    },
    [t],
  );

  const handleModalClose = useCallback(() => {
    setModalVisible(false);
    setScriptContent(null);
    setCurrentScript(null);
  }, []);

  if (!results || results.length === 0) {
    return (
      <Empty
        description={t("trafficDetail.script.noResults")}
        style={{ padding: 40 }}
      />
    );
  }

  return (
    <div style={{ padding: 12 }}>
      {results.map((result, index) => (
        <ScriptResultCard
          key={`${result.script_name}-${index}`}
          result={result}
          onScriptClick={handleScriptClick}
        />
      ))}

      <Modal
        title={
          <Space>
            <CodeOutlined />
            <span>{currentScript?.name}</span>
            {currentScript && (
              <Tag color={currentScript.type === "request" ? "blue" : "green"}>
                {scriptTypeLabel(currentScript.type, t)}
              </Tag>
            )}
          </Space>
        }
        open={modalVisible}
        onCancel={handleModalClose}
        footer={null}
        width={800}
        styles={{ body: { padding: 0 } }}
      >
        {scriptLoading ? (
          <div
            style={{
              display: "flex",
              justifyContent: "center",
              alignItems: "center",
              height: 400,
            }}
          >
            <Spin />
          </div>
        ) : (
          <Editor
            height={400}
            language="javascript"
            theme={resolvedTheme === "dark" ? "vs-dark" : "light"}
            value={scriptContent || ""}
            options={{
              readOnly: true,
              minimap: { enabled: false },
              fontSize: 13,
              lineNumbers: "on",
              scrollBeyondLastLine: false,
              automaticLayout: true,
              tabSize: 2,
            }}
          />
        )}
      </Modal>
    </div>
  );
}
