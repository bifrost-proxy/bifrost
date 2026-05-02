import type { CSSProperties, ReactNode } from "react";
import { useState } from "react";
import { ArrowsAltOutlined, CopyOutlined, PlayCircleOutlined, RightOutlined } from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import { Button, Empty, Input, message, Modal, Space, Tag, Tooltip } from "antd";
import type { DebugConsoleValue, DevtoolsSnapshot } from "../../../api/devtools";
import { useThemeStore } from "../../../stores/useThemeStore";
import { HighlightedText } from "./shared";
import { filterBySearch } from "./sharedUtils";

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
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);
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
            theme={resolvedTheme === "dark" ? "vs-dark" : "light"}
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
  const formattedParts = args ? formatConsoleMessageArgs(args) : null;
  return (
    <div data-testid={`devtools-console-row-${level}`} style={style}>
      <span style={consolePromptStyle}>{entry.kind === "input" ? ">" : ""}</span>
      <Tag style={consoleLevelTagStyle} color={consoleLevelColor(level)}>{level}</Tag>
      <span data-testid="devtools-console-row-time" style={consoleTimeStyle}>{formatConsoleTime(entry.at_ms)}</span>
      <div style={consoleMessageCellStyle}>
        <div style={consoleMessageStyleForLevel(level)}>
          {args ? (
            formattedParts
              ? formattedParts.map((part, index) => renderFormattedConsolePart(part, index, searchQuery))
              : args.map((arg, index) => (
                <ConsoleValueView
                  key={`${index}-${arg.type}-${arg.preview}`}
                  value={arg}
                  searchQuery={searchQuery}
                  depth={0}
                  topLevel
                  level={level}
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

type ConsoleFormattedPart =
  | { kind: "text"; text: string; style?: CSSProperties }
  | { kind: "value"; value: DebugConsoleValue };

function formatConsoleMessageArgs(args: DebugConsoleValue[]): ConsoleFormattedPart[] | null {
  if (!args.length || args[0].type !== "string") return null;
  const template = consolePlainString(args[0]);
  if (!template.includes("%")) return null;
  const parts: ConsoleFormattedPart[] = [];
  let argIndex = 1;
  let textBuffer = "";
  let currentStyle: CSSProperties | undefined;
  const flushText = () => {
    if (!textBuffer) return;
    parts.push({ kind: "text", text: textBuffer, style: currentStyle });
    textBuffer = "";
  };
  for (let index = 0; index < template.length; index += 1) {
    const char = template[index];
    if (char !== "%" || index === template.length - 1) {
      textBuffer += char;
      continue;
    }
    const token = template[index + 1];
    index += 1;
    if (token === "%") {
      textBuffer += "%";
      continue;
    }
    if (token === "c") {
      flushText();
      currentStyle = parseConsoleCss(consolePlainString(args[argIndex]));
      argIndex += 1;
      continue;
    }
    if (token === "s" || token === "d" || token === "i" || token === "f") {
      const value = args[argIndex];
      argIndex += 1;
      textBuffer += value ? consoleSubstitutionText(value, token) : `%${token}`;
      continue;
    }
    if (token === "o" || token === "O") {
      flushText();
      const value = args[argIndex];
      argIndex += 1;
      if (value) parts.push({ kind: "value", value });
      else textBuffer += `%${token}`;
      continue;
    }
    textBuffer += `%${token}`;
  }
  flushText();
  for (; argIndex < args.length; argIndex += 1) {
    parts.push({ kind: "value", value: args[argIndex] });
  }
  return parts.length ? parts : null;
}

function renderFormattedConsolePart(part: ConsoleFormattedPart, index: number, searchQuery: string): ReactNode {
  if (part.kind === "value") {
    return (
      <ConsoleValueView
        key={`value-${index}-${part.value.type}-${part.value.preview}`}
        value={part.value}
        searchQuery={searchQuery}
        depth={0}
        topLevel
      />
    );
  }
  return (
    <span key={`text-${index}`} data-testid="devtools-console-formatted-text" style={{ ...formattedTextStyle, ...part.style }}>
      <HighlightedText text={part.text} query={searchQuery} />
    </span>
  );
}

function consolePlainString(value: DebugConsoleValue | undefined): string {
  if (!value) return "";
  if (typeof value.value === "string") return value.value;
  if (value.raw) return value.raw;
  if (value.preview) return unquotePreviewString(value.preview);
  return "";
}

function consoleSubstitutionText(value: DebugConsoleValue, token: string): string {
  const plain = consolePlainString(value);
  if (token === "s") return plain || valuePreview(value, true);
  const numeric = Number(plain || value.value);
  if (Number.isFinite(numeric)) return token === "f" ? String(numeric) : String(Math.trunc(numeric));
  return valuePreview(value, true);
}

function parseConsoleCss(cssText: string): CSSProperties {
  const style: CSSProperties = {};
  for (const declaration of cssText.split(";")) {
    const [rawName, ...rawValueParts] = declaration.split(":");
    const name = rawName?.trim().toLowerCase();
    const value = rawValueParts.join(":").trim();
    if (!name || !value) continue;
    if (name === "color") style.color = value;
    if (name === "background" || name === "background-color") style.backgroundColor = value;
    if (name === "font-weight") style.fontWeight = value;
    if (name === "font-style") style.fontStyle = value;
    if (name === "text-decoration") style.textDecoration = value;
    if (name === "font-size") style.fontSize = value;
  }
  return style;
}

function ConsoleValueView({
  value,
  searchQuery,
  depth,
  label,
  topLevel,
  level,
}: {
  value: DebugConsoleValue;
  searchQuery: string;
  depth: number;
  label?: string;
  topLevel?: boolean;
  level?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const children = value.properties ?? [];
  const expandable = children.length > 0;
  const type = value.subtype || value.type || "unknown";
  const primitive = !expandable && (type === "string" || type === "number" || type === "boolean" || type === "null" || type === "undefined" || type === "bigint");
  const preview = valuePreview(value, Boolean(topLevel));
  return (
    <span style={valueShellStyle(depth)}>
      <span style={valueLineStyle}>
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
        <span
          data-testid={`devtools-console-value-${type}`}
          style={primitive ? primitiveValueStyle(type, Boolean(topLevel), level) : objectPreviewStyle(type)}
        >
          <HighlightedText text={preview} query={searchQuery} />
        </span>
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
              level={level}
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
    kind === "input" ? "var(--devtools-surface-alt)"
      : normalized === "error" ? "var(--devtools-console-error-bg)"
        : normalized === "warn" || normalized === "warning" ? "var(--devtools-console-warning-bg)"
          : normalized === "debug" ? "var(--devtools-console-debug-bg)"
            : normalized === "result" ? "var(--devtools-console-result-bg)"
              : "var(--devtools-surface)";
  const inputColor = kind === "input" ? "var(--devtools-surface-alt)" : color;
  return {
    ...consoleRowStyle,
    background: inputColor,
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



function valuePreview(value: DebugConsoleValue, topLevel = false): string {
  if (topLevel && value.type === "string") {
    if (typeof value.value === "string") return value.value;
    if (value.raw) return value.raw;
    if (value.preview) return unquotePreviewString(value.preview);
  }
  if (value.preview) return value.preview;
  if (typeof value.value === "string") return JSON.stringify(value.value);
  if (value.value !== undefined) return String(value.value);
  return value.type || "unknown";
}

function unquotePreviewString(value: string): string {
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    try {
      return JSON.parse(value) as string;
    } catch {
      return value.slice(1, -1);
    }
  }
  return value;
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
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  overflow: "hidden",
  background: "var(--devtools-surface)",
  color: "var(--devtools-text)",
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
  borderBottom: "1px solid var(--devtools-border)",
  minWidth: 520,
};

const consoleInputBarStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) auto",
  alignItems: "end",
  gap: 8,
  padding: 8,
  borderTop: "1px solid var(--devtools-border)",
  background: "var(--devtools-surface-alt)",
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
  borderTop: "1px solid var(--devtools-border)",
  borderBottom: "1px solid var(--devtools-border)",
};

const consolePromptStyle: CSSProperties = {
  color: "var(--devtools-text-secondary)",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
};

const consoleLevelTagStyle: CSSProperties = {
  marginInlineEnd: 0,
  textTransform: "lowercase",
  textAlign: "center",
};

const consoleTimeStyle: CSSProperties = {
  color: "var(--devtools-text-tertiary)",
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

function consoleMessageStyleForLevel(level: string): CSSProperties {
  return {
    display: "flex",
    flexWrap: "wrap",
    alignItems: "flex-start",
    gap: "2px 10px",
    minWidth: 0,
    color: consoleTextColor(level),
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  };
}

function consoleTextColor(level: string | undefined): string {
  const normalized = (level || "log").toLowerCase();
  if (normalized === "error") return "var(--devtools-code-string)";
  if (normalized === "warn" || normalized === "warning") return "var(--devtools-code-attr)";
  if (normalized === "debug") return "var(--devtools-code-boolean)";
  if (normalized === "result") return "var(--devtools-code-tag)";
  return "var(--devtools-text)";
}

const consoleCopyButtonStyle: CSSProperties = {
  opacity: 0.68,
};

const formattedTextStyle: CSSProperties = {
  whiteSpace: "pre-wrap",
  overflowWrap: "anywhere",
};

function valueShellStyle(depth: number): CSSProperties {
  return {
    display: depth === 0 ? "inline-block" : "block",
    flexDirection: "column",
    alignItems: "flex-start",
    minWidth: 0,
    marginLeft: depth === 0 ? 0 : 16,
    lineHeight: "22px",
    verticalAlign: "top",
  };
}

const valueLineStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "baseline",
  minWidth: 0,
  maxWidth: "100%",
};

const valueToggleStyle: CSSProperties = {
  border: 0,
  background: "transparent",
  color: "var(--devtools-text-secondary)",
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
  minWidth: 0,
  marginTop: 1,
  paddingLeft: 14,
};

const propertyNameStyle: CSSProperties = {
  color: "var(--devtools-code-boolean)",
};

const overflowStyle: CSSProperties = {
  color: "var(--devtools-text-tertiary)",
  marginLeft: 28,
};

function objectPreviewStyle(type: string): CSSProperties {
  return {
    color: type === "array" ? "var(--devtools-code-number)" : "var(--devtools-text)",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  };
}

function primitiveValueStyle(type: string, topLevel = false, level?: string): CSSProperties {
  const color =
    topLevel && type === "string" ? consoleTextColor(level)
      : type === "string" ? "var(--devtools-code-string)"
      : type === "number" || type === "bigint" ? "var(--devtools-code-number)"
        : type === "boolean" ? "var(--devtools-code-boolean)"
          : "var(--devtools-text-secondary)";
  return {
    color,
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  };
}
