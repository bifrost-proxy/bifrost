import type { CSSProperties } from "react";
import { useState } from "react";
import { ArrowsAltOutlined, PlayCircleOutlined } from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import { Button, Empty, Input, Modal, Space, Tag } from "antd";
import type { DevtoolsSnapshot } from "../../../api/devtools";
import { HighlightedText, filterBySearch } from "./shared";

const { TextArea } = Input;

export function ConsoleView({
  messages,
  entries,
  searchQuery,
  expression,
  running,
  onExpressionChange,
  onRun,
}: {
  messages: DevtoolsSnapshot["console"];
  entries: ConsoleUiEntry[];
  searchQuery: string;
  expression: string;
  running: boolean;
  onExpressionChange: (value: string) => void;
  onRun: () => void;
}) {
  const [editorOpen, setEditorOpen] = useState(false);
  const rows = filterBySearch([
    ...messages.map((entry) => ({ kind: "page" as const, ...entry })),
    ...entries,
  ], searchQuery, (entry) => `${entry.kind} ${entry.level} ${entry.text} ${formatConsoleTime(entry.at_ms)}`).sort((left, right) => left.at_ms - right.at_ms);

  return (
    <div style={consoleShellStyle}>
      <div data-testid="devtools-console-log" style={consoleLogStyle}>
        {rows.length ? (
          rows.map((entry, index) => (
            <ConsoleRow key={`${entry.kind}-${entry.at_ms}-${index}`} entry={entry} searchQuery={searchQuery} />
          ))
        ) : (
          <Empty description="No console messages yet" />
        )}
      </div>
      <div style={consoleInputBarStyle}>
        <div style={consoleInputWrapStyle}>
          <TextArea
            data-testid="devtools-console-input"
            value={expression}
            autoSize={{ minRows: 1, maxRows: 6 }}
            onChange={(event) => onExpressionChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                onRun();
              }
            }}
            placeholder="Run JavaScript in the remote page"
            style={consoleInputStyle}
          />
          <Button
            data-testid="devtools-console-expand-editor"
            aria-label="Open fullscreen JavaScript editor"
            icon={<ArrowsAltOutlined />}
            onClick={() => setEditorOpen(true)}
          />
        </div>
        <Button
          data-testid="devtools-console-run"
          type="primary"
          icon={<PlayCircleOutlined />}
          loading={running}
          onClick={onRun}
        >
          Run
        </Button>
      </div>
      <Modal
        title="JavaScript Console"
        open={editorOpen}
        width="min(1040px, 94vw)"
        styles={{ body: fullEditorBodyStyle }}
        onCancel={() => setEditorOpen(false)}
        footer={(
          <Space>
            <Button onClick={() => setEditorOpen(false)}>Close</Button>
            <Button
              data-testid="devtools-console-fullscreen-run"
              type="primary"
              icon={<PlayCircleOutlined />}
              loading={running}
              onClick={() => {
                onRun();
                setEditorOpen(false);
              }}
            >
              Run
            </Button>
          </Space>
        )}
      >
        <div data-testid="devtools-console-fullscreen-editor" style={fullEditorShellStyle}>
          <Editor
            height="100%"
            language="javascript"
            theme="light"
            value={expression}
            onChange={(value) => onExpressionChange(value ?? "")}
            options={{
              minimap: { enabled: false },
              fontSize: 13,
              lineNumbers: "on",
              scrollBeyondLastLine: false,
              automaticLayout: true,
              tabSize: 2,
              wordWrap: "on",
              padding: { top: 10, bottom: 10 },
            }}
          />
        </div>
      </Modal>
    </div>
  );
}

export type ConsoleUiEntry = {
  kind: "input" | "result";
  level: string;
  text: string;
  at_ms: number;
};

type ConsoleDisplayEntry = ConsoleUiEntry | (DevtoolsSnapshot["console"][number] & { kind: "page" });

function ConsoleRow({ entry, searchQuery }: { entry: ConsoleDisplayEntry; searchQuery: string }) {
  const level = entry.kind === "input" ? "input" : entry.level || "log";
  const style = consoleRowStyleForLevel(level, entry.kind);
  return (
    <div data-testid={`devtools-console-row-${level}`} style={style}>
      <span style={consolePromptStyle}>{entry.kind === "input" ? ">" : ""}</span>
      <Tag style={consoleLevelTagStyle} color={consoleLevelColor(level)}>{level}</Tag>
      <span data-testid="devtools-console-row-time" style={consoleTimeStyle}>{formatConsoleTime(entry.at_ms)}</span>
      <pre style={consoleMessageStyle}><HighlightedText text={entry.text} query={searchQuery} /></pre>
    </div>
  );
}

function formatConsoleTime(atMs: number): string {
  if (!Number.isFinite(atMs) || atMs <= 0) return "--:--:--.---";
  const date = new Date(atMs);
  const pad = (value: number, size = 2) => String(value).padStart(size, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

function consoleRowStyleForLevel(level: string, kind: ConsoleDisplayEntry["kind"]): CSSProperties {
  const normalized = level.toLowerCase();
  const color =
    kind === "input" ? "#f8fafc"
      : normalized === "error" ? "#fff1f0"
        : normalized === "warn" || normalized === "warning" ? "#fffbe6"
          : normalized === "debug" ? "#f5f3ff"
            : normalized === "result" ? "#f0fdf4"
              : "#fff";
  return {
    ...consoleRowStyle,
    background: color,
  };
}

function consoleLevelColor(level: string): string {
  const normalized = level.toLowerCase();
  if (normalized === "error") return "red";
  if (normalized === "warn" || normalized === "warning") return "gold";
  if (normalized === "debug") return "purple";
  if (normalized === "input") return "blue";
  if (normalized === "result") return "green";
  return "default";
}



export function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  return JSON.stringify(value, null, 2);
}



const consoleShellStyle: CSSProperties = {
  display: "grid",
  gridTemplateRows: "minmax(260px, 1fr) auto",
  height: "100%",
  minHeight: 0,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
  background: "#fff",
};

const consoleLogStyle: CSSProperties = {
  display: "grid",
  alignContent: "start",
  overflow: "auto",
  minHeight: 0,
};

const consoleRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "18px 58px 72px minmax(0, 1fr)",
  alignItems: "flex-start",
  gap: 6,
  padding: "7px 10px",
  borderBottom: "1px solid #edf2f7",
  minWidth: 520,
};

const consoleInputBarStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) auto",
  alignItems: "end",
  gap: 8,
  padding: 8,
  borderTop: "1px solid #d9e2ef",
  background: "#fbfdff",
};

const consoleInputWrapStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) auto",
  alignItems: "stretch",
  gap: 6,
};

const consoleInputStyle: CSSProperties = {
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
};

const fullEditorBodyStyle: CSSProperties = {
  padding: 0,
};

const fullEditorShellStyle: CSSProperties = {
  height: "min(70vh, 680px)",
  minHeight: 420,
  borderTop: "1px solid #e5edf7",
  borderBottom: "1px solid #e5edf7",
};

const consolePromptStyle: CSSProperties = {
  color: "#6b7280",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
};

const consoleLevelTagStyle: CSSProperties = {
  marginInlineEnd: 0,
  textTransform: "lowercase",
  textAlign: "center",
};

const consoleTimeStyle: CSSProperties = {
  color: "#9ca3af",
  fontSize: 10,
  lineHeight: "22px",
  fontVariantNumeric: "tabular-nums",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  userSelect: "text",
};

const consoleMessageStyle: CSSProperties = {
  margin: 0,
  whiteSpace: "pre-wrap",
  overflowWrap: "anywhere",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
};
