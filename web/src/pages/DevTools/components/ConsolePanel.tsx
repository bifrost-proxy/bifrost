import type { CSSProperties } from "react";
import { useState } from "react";
import { ArrowsAltOutlined, CopyOutlined, PlayCircleOutlined, RightOutlined } from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import { Button, Empty, Input, message, Modal, Space, Tag, Tooltip } from "antd";
import type { DebugConsoleValue, DevtoolsSnapshot } from "../../../api/devtools";
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
  ], searchQuery, (entry) => `${entry.kind} ${entry.level} ${entry.text} ${entry.raw ?? ""} ${entry.args?.map((arg) => arg.raw ?? arg.preview ?? "").join(" ") ?? ""} ${formatConsoleTime(entry.at_ms)}`).sort((left, right) => left.at_ms - right.at_ms);

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
  args?: DebugConsoleValue[];
  raw?: string | null;
};

type ConsoleDisplayEntry = ConsoleUiEntry | (DevtoolsSnapshot["console"][number] & { kind: "page" });

function ConsoleRow({ entry, searchQuery }: { entry: ConsoleDisplayEntry; searchQuery: string }) {
  const level = entry.kind === "input" ? "input" : entry.level || "log";
  const style = consoleRowStyleForLevel(level, entry.kind);
  const args = entry.args?.length ? entry.args : null;
  const raw = entry.raw || args?.map((arg) => arg.raw || arg.preview || "").join(" ") || entry.text;
  return (
    <div data-testid={`devtools-console-row-${level}`} style={style}>
      <span style={consolePromptStyle}>{entry.kind === "input" ? ">" : ""}</span>
      <Tag style={consoleLevelTagStyle} color={consoleLevelColor(level)}>{level}</Tag>
      <span data-testid="devtools-console-row-time" style={consoleTimeStyle}>{formatConsoleTime(entry.at_ms)}</span>
      <div style={consoleMessageCellStyle}>
        <div style={consoleMessageStyle}>
          {args ? (
            args.map((arg, index) => (
              <ConsoleValueView
                key={`${index}-${arg.type}-${arg.preview}`}
                value={arg}
                searchQuery={searchQuery}
                depth={0}
              />
            ))
          ) : (
            <HighlightedText text={entry.text} query={searchQuery} />
          )}
        </div>
        <Tooltip title="Copy raw console content">
          <Button
            data-testid="devtools-console-copy-raw"
            aria-label="Copy raw console content"
            size="small"
            type="text"
            icon={<CopyOutlined />}
            style={consoleCopyButtonStyle}
            onClick={() => copyConsoleRaw(raw)}
          />
        </Tooltip>
      </div>
    </div>
  );
}

function ConsoleValueView({
  value,
  searchQuery,
  depth,
  label,
}: {
  value: DebugConsoleValue;
  searchQuery: string;
  depth: number;
  label?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const children = value.properties ?? [];
  const expandable = children.length > 0;
  const type = value.subtype || value.type || "unknown";
  const primitive = !expandable && (type === "string" || type === "number" || type === "boolean" || type === "null" || type === "undefined" || type === "bigint");
  const preview = valuePreview(value);
  return (
    <span style={valueShellStyle(depth)}>
      {expandable ? (
        <button
          type="button"
          data-testid="devtools-console-expand-value"
          aria-label={expanded ? "Collapse console value" : "Expand console value"}
          style={valueToggleStyle}
          onClick={() => setExpanded((next) => !next)}
        >
          <RightOutlined style={{ fontSize: 10, transform: expanded ? "rotate(90deg)" : undefined }} />
        </button>
      ) : (
        <span style={valueTogglePlaceholderStyle} />
      )}
      {label ? <span style={propertyNameStyle}>{label}: </span> : null}
      <span style={primitive ? primitiveValueStyle(type) : objectPreviewStyle(type)}>
        <HighlightedText text={preview} query={searchQuery} />
      </span>
      {expanded ? (
        <span style={childrenWrapStyle}>
          {children.map((child) => (
            <ConsoleValueView
              key={`${child.name}-${child.value.type}-${child.value.preview}`}
              value={child.value}
              searchQuery={searchQuery}
              depth={depth + 1}
              label={child.name}
            />
          ))}
          {value.overflow ? <span style={overflowStyle}>... {value.overflow} more</span> : null}
        </span>
      ) : null}
    </span>
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

export function consoleValueFromRuntimeResult(value: unknown): DebugConsoleValue {
  if (!value || typeof value !== "object") {
    return consoleValueFromPlain(value);
  }
  const payload = value as { result?: unknown; type?: string; subtype?: string; value?: unknown; description?: string };
  const object = payload.result && typeof payload.result === "object"
    ? payload.result as { type?: string; subtype?: string; value?: unknown; description?: string }
    : payload;
  if ("value" in object) return consoleValueFromPlain(object.value);
  return {
    type: object.type || "object",
    subtype: object.subtype,
    preview: object.description || formatValue(value),
    raw: object.description || formatValue(value),
  };
}

function consoleValueFromPlain(value: unknown): DebugConsoleValue {
  if (value === null) return { type: "null", value, preview: "null", raw: "null" };
  if (value === undefined) return { type: "undefined", preview: "undefined", raw: "undefined" };
  const type = typeof value;
  if (type === "string") return { type, value, preview: JSON.stringify(value), raw: value as string };
  if (type === "number" || type === "boolean") return { type, value, preview: String(value), raw: String(value) };
  try {
    return { type: "object", preview: JSON.stringify(value), raw: JSON.stringify(value, null, 2) };
  } catch {
    return { type: "object", preview: String(value), raw: String(value) };
  }
}

function valuePreview(value: DebugConsoleValue): string {
  if (value.preview) return value.preview;
  if (typeof value.value === "string") return JSON.stringify(value.value);
  if (value.value !== undefined) return String(value.value);
  return value.type || "unknown";
}

async function copyConsoleRaw(raw: string) {
  try {
    await navigator.clipboard.writeText(raw);
    message.success("Copied");
  } catch {
    message.error("Copy failed");
  }
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

const consoleMessageCellStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) auto",
  gap: 8,
  minWidth: 0,
};

const consoleMessageStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "baseline",
  gap: "3px 8px",
  minWidth: 0,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
};

const consoleCopyButtonStyle: CSSProperties = {
  opacity: 0.68,
};

function valueShellStyle(depth: number): CSSProperties {
  return {
    display: depth === 0 ? "inline-flex" : "flex",
    flexDirection: "column",
    alignItems: "flex-start",
    minWidth: 0,
    marginLeft: depth === 0 ? 0 : 14,
    lineHeight: "22px",
  };
}

const valueToggleStyle: CSSProperties = {
  border: 0,
  background: "transparent",
  color: "#6b7280",
  padding: 0,
  width: 14,
  height: 18,
  lineHeight: "18px",
  cursor: "pointer",
};

const valueTogglePlaceholderStyle: CSSProperties = {
  display: "inline-block",
  width: 14,
};

const childrenWrapStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  width: "100%",
  marginTop: 2,
};

const propertyNameStyle: CSSProperties = {
  color: "#7c3aed",
};

const overflowStyle: CSSProperties = {
  color: "#9ca3af",
  marginLeft: 28,
};

function objectPreviewStyle(type: string): CSSProperties {
  return {
    color: type === "array" ? "#1f4fbf" : "#111827",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  };
}

function primitiveValueStyle(type: string): CSSProperties {
  const color =
    type === "string" ? "#c41d1d"
      : type === "number" || type === "bigint" ? "#1d4ed8"
        : type === "boolean" ? "#7c3aed"
          : "#6b7280";
  return {
    color,
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  };
}
