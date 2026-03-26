import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Button,
  Space,
  Typography,
  Spin,
  message,
  Modal,
  Input,
  Tag,
  Empty,
  Dropdown,
  theme,
  Tooltip,
  Switch,
  Form,
} from "antd";
import type { MenuProps } from "antd";
import {
  FileOutlined,
  FolderOutlined,
  FolderOpenOutlined,
  PlusOutlined,
  SaveOutlined,
  DeleteOutlined,
  PlayCircleOutlined,
  CheckCircleOutlined,
  CloseCircleOutlined,
  SearchOutlined,
  UpOutlined,
  DownOutlined,
  ExportOutlined,
  SettingOutlined,
  EditOutlined,
  MoreOutlined,
} from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import { useScriptsStore } from "../../stores/useScriptsStore";
import type {
  ScriptType,
  ScriptLogEntry,
  ScriptInfo,
  ScriptExecutionResult,
} from "../../api/scripts";
import { useThemeStore } from "../../stores/useThemeStore";
import SplitPane from "../../components/SplitPane";
import VerticalSplitPane from "../../components/VerticalSplitPane";
import { ImportBifrostButton } from "../../components/ImportBifrostButton";
import { useExportBifrost } from "../../hooks/useExportBifrost";
import { getSandboxConfig, updateSandboxConfig } from "../../api/config";
import pushService from "../../services/pushService";
import styles from "./index.module.css";

const { Text } = Typography;

const BIFROST_TYPES_DECODE = `
/**
 * Bifrost Decode Script Types
 *
 * Decode scripts are executed BEFORE body is stored and pushed.
 * - ctx.phase === "request"  : request body decode (response is null)
 * - ctx.phase === "response" : response body decode (response.request carries request snapshot)
 */

interface BifrostDecodeRequest {
  readonly url: string;
  readonly host: string;
  readonly path: string;
  readonly protocol: string;
  readonly clientIp: string;
  readonly clientApp: string | null;
  readonly method: string;
  readonly headers: Record<string, string>;

  /** UTF-8 preview (may be truncated) */
  readonly body: string;
  /** Hex preview (may be truncated) */
  readonly bodyHex: string;
  /** Original byte length */
  readonly bodySize: number;
  readonly bodyHexTruncated: boolean;
  readonly bodyTextTruncated: boolean;
}

interface BifrostDecodeResponse {
  readonly status: number;
  readonly statusText: string;
  readonly headers: Record<string, string>;
  readonly body: string;
  readonly bodyHex: string;
  readonly bodySize: number;
  readonly bodyHexTruncated: boolean;
  readonly bodyTextTruncated: boolean;
  readonly request: {
    url: string;
    method: string;
    host: string;
    path: string;
    protocol: string;
    clientIp: string;
    clientApp: string | null;
    headers: Record<string, string>;
  };
}

interface BifrostDecodeOutput {
  data: string;
  code: string;
  msg: string;
}

interface BifrostContext {
  readonly requestId: string;
  readonly scriptName: string;
  readonly scriptType: "request" | "response" | "decode";
  readonly phase?: "request" | "response";
  output?: BifrostDecodeOutput;
  readonly values: Record<string, string>;
  readonly matchedRules: Array<{ pattern: string; protocol: string; value: string }>;
}

interface BifrostLog {
  log(...args: any[]): void;
  debug(...args: any[]): void;
  info(...args: any[]): void;
  warn(...args: any[]): void;
  error(...args: any[]): void;
}

interface BifrostFile {
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

interface BifrostNet {
  readonly enabled: boolean;
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

declare const request: BifrostDecodeRequest;
declare const response: BifrostDecodeResponse | null;
declare const ctx: BifrostContext;
declare let output: BifrostDecodeOutput | undefined;
declare const log: BifrostLog;
declare const console: BifrostLog;
declare const file: BifrostFile;
declare const net: BifrostNet;
`;

const BIFROST_TYPES_REQUEST = `
/**
 * Bifrost Request Script Types
 * 
 * Request scripts are executed BEFORE the request is sent to the upstream server.
 * You can modify: method, headers, body
 * Read-only properties: url, host, path, protocol, clientIp, clientApp
 */

/** HTTP Request object - available in request scripts */
interface BifrostRequest {
  /** Full request URL (read-only) */
  readonly url: string;
  /** Host name from the request (read-only) */
  readonly host: string;
  /** Request path (read-only) */
  readonly path: string;
  /** Protocol: "http" or "https" (read-only) */
  readonly protocol: string;
  /** Client IP address (read-only) */
  readonly clientIp: string;
  /** Client application identifier, if available (read-only) */
  readonly clientApp: string | null;
  /** HTTP method (GET, POST, PUT, DELETE, etc.) - modifiable */
  method: string;
  /** Request headers as key-value pairs - modifiable */
  headers: Record<string, string>;
  /** Request body content - modifiable */
  body: string | null;
}

/** Script execution context - provides metadata and configuration */
interface BifrostContext {
  /** Unique identifier for this request */
  readonly requestId: string;
  /** Name of the current script */
  readonly scriptName: string;
  /** Type of script: "request" | "response" | "decode" */
  readonly scriptType: "request" | "response" | "decode";
  /** Current phase for decode: "request" | "response" */
  readonly phase?: "request" | "response";
  /** Custom key-value configuration from Bifrost settings */
  readonly values: Record<string, string>;
  /** List of rules that matched this request */
  readonly matchedRules: Array<{
    /** Rule pattern (e.g., "*.example.com") */
    pattern: string;
    /** Protocol (http/https) */
    protocol: string;
    /** Rule value/target */
    value: string;
  }>;
}

/** Logging interface - logs are captured and displayed in test results */
interface BifrostLog {
  /** Log a message (alias for info) */
  log(...args: any[]): void;
  /** Log debug level message */
  debug(...args: any[]): void;
  /** Log info level message */
  info(...args: any[]): void;
  /** Log warning level message */
  warn(...args: any[]): void;
  /** Log error level message */
  error(...args: any[]): void;
}

/** Sandbox file API (path is relative to scripts/_sandbox) */
interface BifrostFile {
  /** Whether file APIs are enabled */
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

/** Network request API (returns JSON string, use JSON.parse) */
interface BifrostNet {
  /** Whether net APIs are enabled */
  readonly enabled: boolean;
  /**
   * net.fetch(url, optionsJson?) -> JSON string
   * optionsJson example: {"method":"POST","headers":{"Content-Type":"application/json"},"body":"...","timeoutMs":3000}
   */
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

/** The request object to inspect and modify */
declare const request: BifrostRequest;
/** Script execution context with metadata */
declare const ctx: BifrostContext;
/** Logging interface */
declare const log: BifrostLog;
/** Console logging (alias for log) */
declare const console: BifrostLog;
/** File API */
declare const file: BifrostFile;
/** Network API */
declare const net: BifrostNet;
`;

const BIFROST_TYPES_RESPONSE = `
/**
 * Bifrost Response Script Types
 * 
 * Response scripts are executed AFTER receiving the response from upstream.
 * You can modify: status, statusText, headers, body
 * Read-only: request (original request information)
 */

/** HTTP Response object - available in response scripts */
interface BifrostResponse {
  /** HTTP status code (e.g., 200, 404, 500) - modifiable */
  status: number;
  /** HTTP status text (e.g., "OK", "Not Found") - modifiable */
  statusText: string;
  /** Response headers as key-value pairs - modifiable */
  headers: Record<string, string>;
  /** Response body content - modifiable */
  body: string | null;
  /** Original request information (read-only) */
  readonly request: {
    /** Full request URL */
    url: string;
    /** HTTP method used */
    method: string;
    /** Host name */
    host: string;
    /** Request path */
    path: string;
    /** Request headers */
    headers: Record<string, string>;
  };
}

/** Script execution context - provides metadata and configuration */
interface BifrostContext {
  /** Unique identifier for this request */
  readonly requestId: string;
  /** Name of the current script */
  readonly scriptName: string;
  /** Type of script: "request" | "response" | "decode" */
  readonly scriptType: "request" | "response" | "decode";
  /** Current phase for decode: "request" | "response" */
  readonly phase?: "request" | "response";
  /** Custom key-value configuration from Bifrost settings */
  readonly values: Record<string, string>;
  /** List of rules that matched this request */
  readonly matchedRules: Array<{
    /** Rule pattern (e.g., "*.example.com") */
    pattern: string;
    /** Protocol (http/https) */
    protocol: string;
    /** Rule value/target */
    value: string;
  }>;
}

/** Logging interface - logs are captured and displayed in test results */
interface BifrostLog {
  /** Log a message (alias for info) */
  log(...args: any[]): void;
  /** Log debug level message */
  debug(...args: any[]): void;
  /** Log info level message */
  info(...args: any[]): void;
  /** Log warning level message */
  warn(...args: any[]): void;
  /** Log error level message */
  error(...args: any[]): void;
}

/** Sandbox file API (path is relative to scripts/_sandbox) */
interface BifrostFile {
  readonly enabled: boolean;
  readText(path: string): string;
  writeText(path: string, content: string): boolean;
  appendText(path: string, content: string): boolean;
  exists(path: string): boolean;
  remove(path: string): boolean;
  listDir(path?: string): string[];
}

/** Network request API (returns JSON string, use JSON.parse) */
interface BifrostNet {
  readonly enabled: boolean;
  fetch(url: string, optionsJson?: string): string;
  request(url: string, optionsJson?: string): string;
}

/** The response object to inspect and modify */
declare const response: BifrostResponse;
/** Script execution context with metadata */
declare const ctx: BifrostContext;
/** Logging interface */
declare const log: BifrostLog;
/** Console logging (alias for log) */
declare const console: BifrostLog;
/** File API */
declare const file: BifrostFile;
/** Network API */
declare const net: BifrostNet;
`;

function LogLevel({ level }: { level: ScriptLogEntry["level"] }) {
  const colors: Record<string, string> = {
    debug: "default",
    info: "blue",
    warn: "orange",
    error: "red",
  };
  return <Tag color={colors[level]}>{level.toUpperCase()}</Tag>;
}

function HighlightText({
  text,
  highlight,
  highlightStyle,
}: {
  text: string;
  highlight: string;
  highlightStyle?: React.CSSProperties;
}) {
  if (!highlight.trim()) {
    return <span>{text}</span>;
  }

  const regex = new RegExp(
    `(${highlight.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`,
    "gi",
  );
  const parts = text.split(regex);

  return (
    <span>
      {parts.map((part, index) =>
        regex.test(part) ? (
          <span
            key={index}
            style={{
              backgroundColor: "#faad14",
              color: "#000",
              padding: "0 2px",
              borderRadius: 2,
              ...highlightStyle,
            }}
          >
            {part}
          </span>
        ) : (
          <span key={index}>{part}</span>
        ),
      )}
    </span>
  );
}

interface FlatNode {
  key: string;
  type: "folder" | "script";
  depth: number;
  label: string;
  scriptType?: ScriptType;
  scriptName?: string;
  folderPath?: string;
  scriptCount?: number;
}

function getScriptItemId(key: string) {
  return `script-item-${encodeURIComponent(key)}`;
}

function ScriptListPanel({
  allScripts,
  selectedScript,
  selectedType,
  loading,
  searchValue,
  onSearchChange,
  onNewScript,
  onSelectScript,
  onDeleteScript,
  onRenameScript,
  onDeleteFolder,
  onExport,
  onExportAll,
  onImportSuccess,
  onOpenSandboxSettings,
  expandedKeys,
  onToggleFolder,
}: {
  allScripts: ScriptInfo[];
  selectedScript: ScriptInfo | null;
  selectedType: ScriptType;
  loading: boolean;
  searchValue: string;
  onSearchChange: (value: string) => void;
  onNewScript: (type: ScriptType) => void;
  onSelectScript: (type: ScriptType, name: string) => void;
  onDeleteScript: (type: ScriptType, name: string) => void;
  onRenameScript: (type: ScriptType, oldName: string, newName: string) => Promise<boolean>;
  onDeleteFolder: (folderPath: string) => void;
  onExport: (scriptNames: string[]) => void;
  onExportAll: () => void;
  onImportSuccess: () => void;
  onOpenSandboxSettings: () => void;
  expandedKeys: React.Key[];
  onToggleFolder: (folderKey: string) => void;
}) {
  const [selectedScripts, setSelectedScripts] = useState<string[]>([]);
  const lastClickedIndexRef = useRef<number | null>(null);
  const [renameTarget, setRenameTarget] = useState<{ type: ScriptType; name: string } | null>(null);
  const [newName, setNewName] = useState("");
  const [renameModalVisible, setRenameModalVisible] = useState(false);
  const listContainerRef = useRef<HTMLDivElement | null>(null);

  const flatNodes = useMemo<FlatNode[]>(() => {
    const filteredScripts = searchValue
      ? allScripts.filter((s) =>
          s.name.toLowerCase().includes(searchValue.toLowerCase()),
        )
      : allScripts;

    const sorted = [...filteredScripts].sort((a, b) =>
      a.name.localeCompare(b.name),
    );

    interface TreeNode {
      folderName: string;
      folderPath: string;
      children: TreeNode[];
      scripts: ScriptInfo[];
    }

    const root: TreeNode = {
      folderName: "",
      folderPath: "",
      children: [],
      scripts: [],
    };

    for (const script of sorted) {
      const parts = script.name.split("/");
      const fileName = parts.pop()!;
      let current = root;

      for (const part of parts) {
        const childPath = current.folderPath
          ? `${current.folderPath}/${part}`
          : part;
        let child = current.children.find((c) => c.folderName === part);
        if (!child) {
          child = {
            folderName: part,
            folderPath: childPath,
            children: [],
            scripts: [],
          };
          current.children.push(child);
        }
        current = child;
      }
      current.scripts.push({ ...script, name: script.name } as ScriptInfo & {
        _fileName: string;
      });
      void fileName;
    }

    const countScripts = (node: TreeNode): number => {
      let count = node.scripts.length;
      for (const child of node.children) {
        count += countScripts(child);
      }
      return count;
    };

    const result: FlatNode[] = [];

    const flatten = (node: TreeNode, depth: number) => {
      const sortedChildren = [...node.children].sort((a, b) =>
        a.folderName.localeCompare(b.folderName),
      );
      const sortedScripts = [...node.scripts].sort((a, b) =>
        a.name.localeCompare(b.name),
      );

      for (const child of sortedChildren) {
        const folderKey = `folder:${child.folderPath}`;
        result.push({
          key: folderKey,
          type: "folder",
          depth,
          label: child.folderName,
          folderPath: child.folderPath,
          scriptCount: countScripts(child),
        });
        if (expandedKeys.includes(folderKey)) {
          flatten(child, depth + 1);
        }
      }

      for (const script of sortedScripts) {
        const fileName = script.name.split("/").pop()!;
        result.push({
          key: `${script.script_type}/${script.name}`,
          type: "script",
          depth,
          label: fileName,
          scriptType: script.script_type,
          scriptName: script.name,
        });
      }
    };

    flatten(root, 0);
    return result;
  }, [allScripts, searchValue, expandedKeys]);

  const scriptNodes = useMemo(
    () => flatNodes.filter((n) => n.type === "script"),
    [flatNodes],
  );

  const selectedKey = selectedScript
    ? `${selectedType}/${selectedScript.name}`
    : null;

  const handleClick = useCallback(
    (node: FlatNode, e: React.MouseEvent) => {
      listContainerRef.current?.focus();
      if (node.type === "folder") {
        onToggleFolder(node.key);
        return;
      }

      const isCtrl = e.ctrlKey || e.metaKey;
      const isShift = e.shiftKey;
      const currentIndex = scriptNodes.findIndex((n) => n.key === node.key);

      if (isShift && lastClickedIndexRef.current !== null) {
        const start = Math.min(lastClickedIndexRef.current, currentIndex);
        const end = Math.max(lastClickedIndexRef.current, currentIndex);
        const rangeKeys = scriptNodes.slice(start, end + 1).map((n) => n.key);
        setSelectedScripts((prev) => {
          const combined = new Set([...prev, ...rangeKeys]);
          return Array.from(combined);
        });
      } else if (isCtrl) {
        setSelectedScripts((prev) =>
          prev.includes(node.key)
            ? prev.filter((k) => k !== node.key)
            : [...prev, node.key],
        );
        lastClickedIndexRef.current = currentIndex;
      } else {
        setSelectedScripts([]);
        lastClickedIndexRef.current = currentIndex;
        if (node.scriptType && node.scriptName !== undefined) {
          onSelectScript(node.scriptType, node.scriptName);
        }
      }
    },
    [scriptNodes, onSelectScript, onToggleFolder],
  );

  const handleListKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) {
        return;
      }

      if (scriptNodes.length === 0) {
        return;
      }

      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
        return;
      }

      event.preventDefault();

      const currentIndex = selectedKey
        ? scriptNodes.findIndex((node) => node.key === selectedKey)
        : -1;

      const fallbackIndex = event.key === "ArrowDown" ? 0 : scriptNodes.length - 1;
      const nextIndex =
        currentIndex === -1
          ? fallbackIndex
          : Math.min(
              scriptNodes.length - 1,
              Math.max(0, currentIndex + (event.key === "ArrowDown" ? 1 : -1)),
            );

      const nextNode = scriptNodes[nextIndex];
      if (
        !nextNode ||
        !nextNode.scriptType ||
        nextNode.scriptName === undefined ||
        nextNode.key === selectedKey
      ) {
        return;
      }

      setSelectedScripts([]);
      lastClickedIndexRef.current = nextIndex;
      void onSelectScript(nextNode.scriptType, nextNode.scriptName);
    },
    [onSelectScript, scriptNodes, selectedKey],
  );

  useEffect(() => {
    if (!selectedKey) {
      return;
    }

    const selectedItem = listContainerRef.current?.querySelector<HTMLElement>(
      `[data-script-key="${CSS.escape(selectedKey)}"]`,
    );
    selectedItem?.scrollIntoView({ block: "nearest" });
  }, [flatNodes, selectedKey]);

  const getScriptsInFolder = useCallback(
    (folderPath: string): { type: ScriptType; name: string }[] => {
      const result: { type: ScriptType; name: string }[] = [];
      for (const script of allScripts) {
        if (
          script.name.startsWith(folderPath + "/") ||
          script.name === folderPath
        ) {
          result.push({ type: script.script_type, name: script.name });
        }
      }
      return result;
    },
    [allScripts],
  );

  const getContextMenuItems = useCallback(
    (node: FlatNode): MenuProps["items"] => {
      if (node.type === "folder") {
        const folderPath = node.folderPath!;
        const scriptsInFolder = getScriptsInFolder(folderPath);
        const scriptNames = scriptsInFolder.map((s) => `${s.type}/${s.name}`);
        return [
          {
            key: "new-request",
            label: "New Request Script",
            onClick: () => onNewScript("request"),
          },
          {
            key: "new-response",
            label: "New Response Script",
            onClick: () => onNewScript("response"),
          },
          {
            key: "new-decode",
            label: "New Decode Script",
            onClick: () => onNewScript("decode"),
          },
          { type: "divider" },
          {
            key: "export-folder",
            icon: <ExportOutlined />,
            label: `Export Folder (${scriptsInFolder.length})`,
            onClick: () => onExport(scriptNames),
            disabled: scriptsInFolder.length === 0,
          },
          { type: "divider" },
          {
            key: "delete-folder",
            label: `Delete Folder (${scriptsInFolder.length})`,
            danger: true,
            onClick: () => onDeleteFolder(folderPath),
          },
        ];
      }

      const isInSelection = selectedScripts.includes(node.key);
      const bulkKeys =
        isInSelection && selectedScripts.length > 1
          ? selectedScripts
          : [node.key];

      return [
        {
          key: "rename",
          icon: <EditOutlined />,
          label: "Rename",
          onClick: () => {
            const [type, ...nameParts] = node.key.split("/");
            setRenameTarget({ type: type as ScriptType, name: nameParts.join("/") });
            setNewName(nameParts.join("/"));
            setRenameModalVisible(true);
          },
        },
        { type: "divider" },
        {
          key: "export",
          icon: <ExportOutlined />,
          label: `Export${bulkKeys.length > 1 ? ` (${bulkKeys.length})` : ""}`,
          onClick: () => onExport(bulkKeys),
        },
        { type: "divider" },
        {
          key: "delete",
          icon: <DeleteOutlined />,
          label: `Delete${bulkKeys.length > 1 ? ` (${bulkKeys.length})` : ""}`,
          danger: true,
          onClick: () => {
            if (bulkKeys.length > 1) {
              Modal.confirm({
                title: "Delete Scripts",
                content: `Are you sure you want to delete ${bulkKeys.length} scripts?`,
                okText: "Delete All",
                okType: "danger",
                onOk: async () => {
                  for (const key of bulkKeys) {
                    const [type, ...nameParts] = key.split("/");
                    onDeleteScript(type as ScriptType, nameParts.join("/"));
                  }
                  setSelectedScripts([]);
                  message.success(`Deleted ${bulkKeys.length} scripts`);
                },
              });
            } else {
              const [type, ...nameParts] = bulkKeys[0].split("/");
              const scriptName = nameParts.join("/");
              Modal.confirm({
                title: "Delete Script",
                content: `Are you sure you want to delete "${scriptName}"?`,
                okText: "Delete",
                okType: "danger",
                onOk: async () => {
                  onDeleteScript(type as ScriptType, scriptName);
                  message.success("Script deleted");
                },
              });
            }
          },
        },
      ];
    },
    [
      selectedScripts,
      getScriptsInFolder,
      onNewScript,
      onExport,
      onDeleteFolder,
      onDeleteScript,
    ],
  );

  const hasScripts = allScripts.length > 0;

  return (
    <div className={styles.container} data-testid="scripts-list-panel">
      <div className={styles.header}>
        <span className={styles.headerTitle}>Scripts</span>
        <div className={styles.headerActions}>
          <Tooltip title="Sandbox Settings">
            <Button
              type="text"
              size="small"
              icon={<SettingOutlined />}
              onClick={onOpenSandboxSettings}
              data-testid="scripts-sandbox-button"
            />
          </Tooltip>
          <Tooltip title="New Request">
            <Button
              type="text"
              size="small"
              icon={<PlusOutlined />}
              onClick={() => onNewScript("request")}
              data-testid="scripts-new-request-button"
            />
          </Tooltip>
          <Tooltip title="New Response">
            <Button
              type="text"
              size="small"
              icon={<PlusOutlined />}
              onClick={() => onNewScript("response")}
              style={{ color: "#52c41a" }}
              data-testid="scripts-new-response-button"
            />
          </Tooltip>
          <Tooltip title="New Decode">
            <Button
              type="text"
              size="small"
              icon={<PlusOutlined />}
              onClick={() => onNewScript("decode")}
              style={{ color: "#722ed1" }}
              data-testid="scripts-new-decode-button"
            />
          </Tooltip>
          {hasScripts && (
            <Tooltip title="Export All">
              <Button
                type="text"
                size="small"
                icon={<ExportOutlined />}
                onClick={onExportAll}
                data-testid="scripts-export-all-button"
              />
            </Tooltip>
          )}
          <ImportBifrostButton
            expectedType="script"
            onImportSuccess={onImportSuccess}
            buttonText=""
            buttonType="text"
            size="small"
            testId="scripts-import-button"
          />
        </div>
      </div>

      <div className={styles.searchBox}>
        <Input
          placeholder="Search scripts..."
          prefix={<SearchOutlined style={{ color: "#999" }} />}
          value={searchValue}
          onChange={(e) => onSearchChange(e.target.value)}
          allowClear
          size="small"
          data-testid="scripts-search-input"
        />
      </div>

      <div
        ref={listContainerRef}
        className={styles.listContainer}
        tabIndex={0}
        role="listbox"
        aria-label="Scripts list"
        aria-activedescendant={selectedKey ? getScriptItemId(selectedKey) : undefined}
        onKeyDown={handleListKeyDown}
      >
        {loading && allScripts.length === 0 ? (
          <div className={styles.loading}>
            <Spin size="small" />
          </div>
        ) : (
          <div className={styles.list}>
            {flatNodes.map((node) => {
              if (node.type === "folder") {
                const isExpanded = expandedKeys.includes(node.key);
                return (
                  <Dropdown
                    key={node.key}
                    menu={{ items: getContextMenuItems(node) }}
                    trigger={["contextMenu"]}
                  >
                    <div
                      className={styles.item}
                      role="presentation"
                      style={{ paddingLeft: 12 + node.depth * 16 }}
                      onClick={(e) => handleClick(node, e)}
                    >
                      <div className={styles.itemContent}>
                        <span className={styles.folderIcon}>
                          {isExpanded ? (
                            <FolderOpenOutlined />
                          ) : (
                            <FolderOutlined />
                          )}
                        </span>
                        <span className={styles.itemName}>
                          <HighlightText
                            text={node.label}
                            highlight={searchValue}
                          />
                        </span>
                        <div className={styles.itemMeta}>
                          <Tag style={{ fontSize: 10, padding: "0 2px", lineHeight: "16px", margin: 0, color: "#999", borderColor: "#d9d9d9" }}>
                            {node.scriptCount}
                          </Tag>
                        </div>
                      </div>
                    </div>
                  </Dropdown>
                );
              }

              const isSelected = node.key === selectedKey;
              const isMultiSelected = selectedScripts.includes(node.key);

              return (
                <Dropdown
                  key={node.key}
                  menu={{ items: getContextMenuItems(node) }}
                  trigger={["contextMenu"]}
                >
                  <div
                    id={getScriptItemId(node.key)}
                    className={`${styles.item} ${isSelected ? styles.selected : ""} ${isMultiSelected ? styles.multiSelected : ""}`}
                    style={{ paddingLeft: 12 + node.depth * 16 }}
                    role="option"
                    aria-selected={isSelected}
                    onClick={(e) => handleClick(node, e)}
                    data-testid="script-item"
                    data-script-key={node.key}
                    data-script-name={node.scriptName}
                    data-script-type={node.scriptType}
                  >
                    <div className={styles.itemContent}>
                      <span className={styles.folderIcon}>
                        <FileOutlined />
                      </span>
                      <span className={styles.itemName}>
                        <HighlightText
                          text={node.label}
                          highlight={searchValue}
                        />
                      </span>
                      <div className={styles.itemMeta}>
                        <Tag
                          color={
                            node.scriptType === "request"
                              ? "blue"
                              : node.scriptType === "response"
                                ? "green"
                                : "purple"
                          }
                          className={styles.scriptTag}
                        >
                          {node.scriptType === "request"
                            ? "REQ"
                            : node.scriptType === "response"
                              ? "RES"
                              : "DEC"}
                        </Tag>
                      </div>
                    </div>
                    <div className={styles.itemExtra}>
                      <Dropdown
                        menu={{ items: getContextMenuItems(node) }}
                        trigger={["click"]}
                      >
                        <Button
                          type="text"
                          size="small"
                          icon={<MoreOutlined />}
                          onClick={(e) => e.stopPropagation()}
                          className={styles.moreBtn}
                        />
                      </Dropdown>
                    </div>
                  </div>
                </Dropdown>
              );
            })}
            {flatNodes.length === 0 && !loading && (
              <div className={styles.empty}>
                {searchValue ? "No matching scripts" : "No scripts yet"}
              </div>
            )}
          </div>
        )}
      </div>

      <div className={styles.footer}>
        <span className={styles.stats}>{allScripts.length} scripts</span>
      </div>

      <Modal
        title="Rename Script"
        open={renameModalVisible}
        onOk={async () => {
          if (!renameTarget || !newName.trim()) return;
          const success = await onRenameScript(renameTarget.type, renameTarget.name, newName.trim());
          if (success) {
            message.success("Script renamed");
            setRenameModalVisible(false);
            setRenameTarget(null);
            setNewName("");
          } else {
            message.error("Failed to rename script");
          }
        }}
        onCancel={() => {
          setRenameModalVisible(false);
          setRenameTarget(null);
          setNewName("");
        }}
        okText="Rename"
        cancelText="Cancel"
      >
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onPressEnter={async () => {
            if (!renameTarget || !newName.trim()) return;
            const success = await onRenameScript(renameTarget.type, renameTarget.name, newName.trim());
            if (success) {
              message.success("Script renamed");
              setRenameModalVisible(false);
              setRenameTarget(null);
              setNewName("");
            } else {
              message.error("Failed to rename script");
            }
          }}
          placeholder="Enter new name"
          autoFocus
        />
      </Modal>
    </div>
  );
}

function EditorPanel({
  selectedScript,
  selectedType,
  isNewScript,
  editorContent,
  onEditorChange,
  onSave,
  onDelete,
  onTest,
  saving,
  testing,
}: {
  selectedScript: ScriptInfo | null;
  selectedType: ScriptType;
  isNewScript: boolean;
  editorContent: string;
  onEditorChange: (value: string) => void;
  onSave: () => void;
  onDelete: () => void;
  onTest: () => void;
  saving: boolean;
  testing: boolean;
}) {
  const resolvedTheme = useThemeStore((s) => s.resolvedTheme);
  const saveRef = useRef(onSave);

  useEffect(() => {
    saveRef.current = onSave;
  }, [onSave]);

  const handleEditorMount = useCallback(
    (
      editor: Parameters<
        NonNullable<Parameters<typeof Editor>[0]["onMount"]>
      >[0],
      monaco: Parameters<
        NonNullable<Parameters<typeof Editor>[0]["onMount"]>
      >[1],
    ) => {
      editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
        saveRef.current();
      });
    },
    [],
  );

  if (!selectedScript) {
    return (
      <div
        style={{
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <div style={{ textAlign: "center", maxWidth: 560 }}>
          <Empty description="Select a script or create a new one" />
          <div
            style={{
              marginTop: 24,
              padding: "16px 24px",
              borderRadius: 8,
              textAlign: "left",
              background: resolvedTheme === "dark" ? "#1f1f1f" : "#fafafa",
            }}
          >
            <Text strong style={{ display: "block", marginBottom: 12 }}>
              How to use Scripts
            </Text>
            <ul
              style={{
                margin: 0,
                paddingLeft: 20,
                color: resolvedTheme === "dark" ? "#a6a6a6" : "#666",
              }}
            >
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  <b>Request scripts</b> run before forwarding to upstream -
                  modify method, headers, or body
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  <b>Response scripts</b> run after receiving response - modify
                  status, headers, or body
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  Use <code>log.info()</code>, <code>log.warn()</code> to debug
                  your scripts
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  Access <code>ctx.values</code> for custom configuration values
                </Text>
              </li>
            </ul>
            <Text
              strong
              style={{ display: "block", marginTop: 16, marginBottom: 8 }}
            >
              Bind scripts to rules
            </Text>
            <Text
              type="secondary"
              style={{ display: "block", marginBottom: 8 }}
            >
              In the <b>Rules</b> page, use <code>reqScript://</code> or{" "}
              <code>resScript://</code> protocol:
            </Text>
            <pre
              style={{
                margin: 0,
                padding: "8px 12px",
                borderRadius: 4,
                fontSize: 12,
                background: resolvedTheme === "dark" ? "#141414" : "#f0f0f0",
                color: resolvedTheme === "dark" ? "#d9d9d9" : "#434343",
                overflow: "auto",
              }}
            >{`# Add auth header to API requests
api.example.com reqScript://add-auth-header

# Modify response for testing
*.example.com resScript://mock-response`}</pre>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div style={{ height: "100%", display: "flex", flexDirection: "column" }} data-testid="scripts-editor-panel">
      <div
        style={{
          padding: "8px 12px",
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          flexShrink: 0,
          borderBottom: `1px solid ${resolvedTheme === "dark" ? "#303030" : "#e8e8e8"}`,
          backgroundColor: resolvedTheme === "dark" ? "#1f1f1f" : "#fafafa",
        }}
      >
        <Space size={8}>
          <Text
            style={{
              fontSize: 13,
              fontWeight: 500,
              fontFamily: "monospace",
            }}
          >
            {isNewScript ? "New Script" : selectedScript.name}
          </Text>
          <Tag
            color={
              selectedType === "request"
                ? "blue"
                : selectedType === "response"
                  ? "green"
                  : "purple"
            }
            style={{ margin: 0 }}
          >
            {selectedType}
          </Tag>
        </Space>
        <Space size={8}>
          <Button
            size="small"
            icon={<PlayCircleOutlined />}
            onClick={onTest}
            loading={testing}
            data-testid="scripts-test-button"
          >
            Run
          </Button>
          <Button
            type="primary"
            size="small"
            icon={<SaveOutlined />}
            onClick={onSave}
            loading={saving}
            data-testid="scripts-save-button"
          >
            Save
          </Button>
          {!isNewScript && (
            <Button
              size="small"
              danger
              icon={<DeleteOutlined />}
              onClick={onDelete}
              data-testid="scripts-delete-button"
            >
              Delete
            </Button>
          )}
        </Space>
      </div>

      <div style={{ flex: 1, minHeight: 0 }}>
        <Editor
          height="100%"
          language="typescript"
          theme={resolvedTheme === "dark" ? "vs-dark" : "light"}
          value={editorContent}
          onChange={(value) => onEditorChange(value || "")}
          onMount={handleEditorMount}
          beforeMount={(monaco) => {
            monaco.languages.typescript.typescriptDefaults.setCompilerOptions({
              target: monaco.languages.typescript.ScriptTarget.ES2020,
              allowNonTsExtensions: true,
              moduleResolution:
                monaco.languages.typescript.ModuleResolutionKind.NodeJs,
              module: monaco.languages.typescript.ModuleKind.CommonJS,
              noEmit: true,
              strict: false,
              allowJs: true,
              checkJs: true,
              noImplicitAny: false,
              noUnusedLocals: false,
              noUnusedParameters: false,
            });
            monaco.languages.typescript.typescriptDefaults.setDiagnosticsOptions(
              {
                noSemanticValidation: false,
                noSyntaxValidation: false,
              },
            );
            monaco.languages.typescript.typescriptDefaults.setExtraLibs([]);
            const typeDefinition =
              selectedType === "request"
                ? BIFROST_TYPES_REQUEST
                : selectedType === "response"
                  ? BIFROST_TYPES_RESPONSE
                  : BIFROST_TYPES_DECODE;
            monaco.languages.typescript.typescriptDefaults.addExtraLib(
              typeDefinition,
              "bifrost.d.ts",
            );
          }}
          key={selectedType}
          wrapperProps={{ "data-testid": "scripts-editor" }}
          options={{
            minimap: { enabled: false },
            fontSize: 13,
            lineNumbers: "on",
            scrollBeyondLastLine: false,
            automaticLayout: true,
            tabSize: 2,
            quickSuggestions: true,
            suggestOnTriggerCharacters: true,
            parameterHints: { enabled: true },
            wordBasedSuggestions: "currentDocument",
            folding: true,
            foldingHighlight: true,
            showFoldingControls: "mouseover",
            bracketPairColorization: { enabled: true },
          }}
        />
      </div>
    </div>
  );
}

function TestResultPanel({
  testResult,
  isExpanded,
  onToggle,
}: {
  testResult: ScriptExecutionResult | null;
  isExpanded: boolean;
  onToggle: () => void;
}) {
  const resolvedTheme = useThemeStore((s) => s.resolvedTheme);
  const { token } = theme.useToken();

  if (!isExpanded) {
    return (
      <div
        style={{
          position: "absolute",
          bottom: 12,
          right: 12,
          zIndex: 10,
        }}
      >
        <Tooltip title="Show Test Results">
          <Button
            type="default"
            size="small"
            icon={<UpOutlined />}
            onClick={onToggle}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              backgroundColor: testResult
                ? testResult.success
                  ? token.colorSuccessBg
                  : token.colorErrorBg
                : undefined,
              borderColor: testResult
                ? testResult.success
                  ? token.colorSuccess
                  : token.colorError
                : undefined,
            }}
          >
            {testResult ? (
              <>
                {testResult.success ? (
                  <CheckCircleOutlined style={{ color: token.colorSuccess }} />
                ) : (
                  <CloseCircleOutlined style={{ color: token.colorError }} />
                )}
                <span>Test Result</span>
              </>
            ) : (
              <span>Test Results</span>
            )}
          </Button>
        </Tooltip>
      </div>
    );
  }

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
      data-testid="scripts-test-result-panel"
    >
      <div
        style={{
          padding: "8px 12px",
          flexShrink: 0,
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {testResult ? (
            <>
              {testResult.success ? (
                <CheckCircleOutlined style={{ color: "#52c41a" }} />
              ) : (
                <CloseCircleOutlined style={{ color: "#ff4d4f" }} />
              )}
              <Text strong>Test Result ({testResult.duration_ms}ms)</Text>
            </>
          ) : (
            <Text strong>Test Results</Text>
          )}
        </div>
        <Tooltip title="Hide Test Results">
          <Button
            type="text"
            size="small"
            icon={<DownOutlined />}
            onClick={onToggle}
          />
        </Tooltip>
      </div>

      <div style={{ flex: 1, overflow: "auto", padding: 12 }}>
        {!testResult ? (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              height: "100%",
              color: token.colorTextSecondary,
            }}
          >
            <Text type="secondary">Run a test to see results here</Text>
          </div>
        ) : (
          <>
            {testResult.error && (
              <div style={{ marginBottom: 12 }}>
                <pre
                  style={{
                    background:
                      resolvedTheme === "dark" ? "#1f1f1f" : "#f5f5f5",
                    color: token.colorError,
                    padding: 8,
                    borderRadius: 4,
                    fontSize: 12,
                    overflow: "auto",
                    maxHeight: 200,
                    margin: 0,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  {testResult.error}
                </pre>
              </div>
            )}

            {testResult.decode_output && (
              <div style={{ marginBottom: 12 }}>
                <Text strong>Decode Output:</Text>
                <pre
                  style={{
                    background:
                      resolvedTheme === "dark" ? "#1f1f1f" : "#f5f5f5",
                    color: resolvedTheme === "dark" ? "#e6e6e6" : "#1f1f1f",
                    padding: 8,
                    borderRadius: 4,
                    fontSize: 12,
                    overflow: "auto",
                    maxHeight: 200,
                    margin: "8px 0 0",
                  }}
                >
                  {JSON.stringify(testResult.decode_output, null, 2)}
                </pre>
              </div>
            )}

            {testResult.request_modifications && (
              <div style={{ marginBottom: 12 }}>
                <Text strong>Request Modifications:</Text>
                <pre
                  style={{
                    background:
                      resolvedTheme === "dark" ? "#1f1f1f" : "#f5f5f5",
                    color: resolvedTheme === "dark" ? "#e6e6e6" : "#1f1f1f",
                    padding: 8,
                    borderRadius: 4,
                    fontSize: 12,
                    overflow: "auto",
                    maxHeight: 150,
                    margin: "8px 0 0",
                  }}
                >
                  {JSON.stringify(testResult.request_modifications, null, 2)}
                </pre>
              </div>
            )}

            {testResult.response_modifications && (
              <div style={{ marginBottom: 12 }}>
                <Text strong>Response Modifications:</Text>
                <pre
                  style={{
                    background:
                      resolvedTheme === "dark" ? "#1f1f1f" : "#f5f5f5",
                    color: resolvedTheme === "dark" ? "#e6e6e6" : "#1f1f1f",
                    padding: 8,
                    borderRadius: 4,
                    fontSize: 12,
                    overflow: "auto",
                    maxHeight: 150,
                    margin: "8px 0 0",
                  }}
                >
                  {JSON.stringify(testResult.response_modifications, null, 2)}
                </pre>
              </div>
            )}

            <div>
              <Text strong>Logs:</Text>
              {testResult.logs.length > 0 ? (
                <div
                  style={{
                    fontFamily: "monospace",
                    fontSize: 12,
                    marginTop: 8,
                  }}
                >
                  {testResult.logs.map(
                    (logEntry: ScriptLogEntry, i: number) => (
                      <div key={i} style={{ marginBottom: 4 }}>
                        <LogLevel level={logEntry.level} />
                        <Text code style={{ marginLeft: 8 }}>
                          {new Date(logEntry.timestamp).toLocaleTimeString()}
                        </Text>
                        <Text style={{ marginLeft: 8 }}>
                          {logEntry.message}
                        </Text>
                      </div>
                    ),
                  )}
                </div>
              ) : (
                <Text type="secondary" style={{ marginLeft: 8 }}>
                  No logs
                </Text>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export default function ScriptsPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const {
    requestScripts,
    responseScripts,
    decodeScripts,
    selectedScript,
    selectedType,
    loading,
    saving,
    testing,
    testResult,
    applyScriptsSnapshot,
    fetchScripts,
    selectScript,
    saveScript,
    deleteScript,
    renameScript,
    testScript,
    createNewScript,
  } = useScriptsStore();
  const { exportFile } = useExportBifrost();

  const [editorContent, setEditorContent] = useState("");
  const [newScriptName, setNewScriptName] = useState("");
  const [isNewScript, setIsNewScript] = useState(false);
  const [showNameModal, setShowNameModal] = useState(false);
  const [searchValue, setSearchValue] = useState("");
  const [expandedKeys, setExpandedKeys] = useState<React.Key[]>([]);
  const [lastSelectedScriptId, setLastSelectedScriptId] = useState<
    string | null
  >(null);
  const [lastScriptsHash, setLastScriptsHash] = useState<string>("");
  const [testResultExpanded, setTestResultExpanded] = useState(false);
  const urlParamRef = useRef(false);

  const [sandboxModalOpen, setSandboxModalOpen] = useState(false);
  const [sandboxLoading, setSandboxLoading] = useState(false);
  const [sandboxSaving, setSandboxSaving] = useState(false);
  const [sandboxEnabled, setSandboxEnabled] = useState(true);
  const [sandboxDirsText, setSandboxDirsText] = useState("");
  const [sandboxDirName, setSandboxDirName] = useState("_sandbox");
  const [sandboxFileMaxBytes, setSandboxFileMaxBytes] = useState(1024 * 1024);
  const [sandboxNetMaxReqBytes, setSandboxNetMaxReqBytes] = useState(256 * 1024);
  const [sandboxNetMaxRespBytes, setSandboxNetMaxRespBytes] = useState(1024 * 1024);
  const [sandboxNetTimeoutMs, setSandboxNetTimeoutMs] = useState(5000);
  const [sandboxTimeoutMs, setSandboxTimeoutMs] = useState(10000);
  const [sandboxMaxMemoryBytes, setSandboxMaxMemoryBytes] = useState(32 * 1024 * 1024);
  const [sandboxMaxDecodeInputBytes, setSandboxMaxDecodeInputBytes] = useState(2 * 1024 * 1024);
  const [sandboxMaxDecompressOutputBytes, setSandboxMaxDecompressOutputBytes] = useState(
    10 * 1024 * 1024,
  );

  useEffect(() => {
    pushService.connect({ need_scripts: true });
    const unsubscribe = pushService.onScriptsUpdate((data) => {
      applyScriptsSnapshot(data);
    });

    return () => {
      unsubscribe();
      pushService.updateSubscription({ need_scripts: false });
      pushService.disconnectIfIdle();
    };
  }, [applyScriptsSnapshot]);

  useEffect(() => {
    if (testResult && !testResultExpanded) {
      setTestResultExpanded(true);
    }
  }, [testResult, testResultExpanded]);

  useEffect(() => {
    const typeParam = searchParams.get("type") as ScriptType | null;
    const nameParam = searchParams.get("name");
    const scripts = [...requestScripts, ...responseScripts, ...decodeScripts];

    if (typeParam && nameParam && scripts.length > 0 && !urlParamRef.current) {
      const exists = scripts.some(
        (s) => s.script_type === typeParam && s.name === nameParam,
      );
      if (exists) {
        urlParamRef.current = true;
        selectScript(typeParam, nameParam);
        setSearchParams({}, { replace: true });
      }
    }
  }, [
    searchParams,
    requestScripts,
    responseScripts,
    decodeScripts,
    selectScript,
    setSearchParams,
  ]);

  const allScripts = useMemo(
    () => [...requestScripts, ...responseScripts, ...decodeScripts],
    [requestScripts, responseScripts, decodeScripts],
  );

  const getAllFolderKeys = useCallback((scripts: ScriptInfo[]): string[] => {
    const folderSet = new Set<string>();
    for (const script of scripts) {
      const parts = script.name.split("/");
      parts.pop();
      let currentPath = "";
      for (const part of parts) {
        currentPath = currentPath ? `${currentPath}/${part}` : part;
        folderSet.add(`folder:${currentPath}`);
      }
    }
    return Array.from(folderSet);
  }, []);

  const currentScriptId = selectedScript
    ? `${selectedType}/${selectedScript.name}`
    : null;
  if (currentScriptId !== lastSelectedScriptId) {
    setLastSelectedScriptId(currentScriptId);
    if (selectedScript) {
      setEditorContent(selectedScript.content);
      setIsNewScript(!selectedScript.name);
    }
  }

  const scriptsHash = allScripts.map((s) => s.name).join(",");
  if (scriptsHash !== lastScriptsHash) {
    setLastScriptsHash(scriptsHash);
    const allFolderKeys = getAllFolderKeys(allScripts);
    setExpandedKeys(allFolderKeys);
  }

  const handleDeleteFolder = useCallback(
    async (folderPath: string) => {
      const scriptsToDelete: { type: ScriptType; name: string }[] = [];
      for (const script of allScripts) {
        if (
          script.name.startsWith(folderPath + "/") ||
          script.name === folderPath
        ) {
          scriptsToDelete.push({ type: script.script_type, name: script.name });
        }
      }
      if (scriptsToDelete.length === 0) {
        message.info("No scripts in this folder");
        return;
      }

      Modal.confirm({
        title: "Delete Folder",
        content: `Are you sure you want to delete folder "${folderPath}" and all ${scriptsToDelete.length} script(s) inside?`,
        okText: "Delete All",
        okType: "danger",
        onOk: async () => {
          for (const script of scriptsToDelete) {
            await deleteScript(script.type, script.name);
          }
          message.success(`Deleted ${scriptsToDelete.length} script(s)`);
        },
      });
    },
    [allScripts, deleteScript],
  );

  const handleSave = useCallback(async () => {
    if (isNewScript) {
      setShowNameModal(true);
      return;
    }
    if (selectedScript?.name) {
      await saveScript(selectedType, selectedScript.name, editorContent);
      message.success("Script saved");
    }
  }, [isNewScript, selectedScript, selectedType, editorContent, saveScript]);

  const handleSaveNewScript = useCallback(async () => {
    if (!newScriptName.trim()) {
      message.error("Please enter a script name");
      return;
    }
    const validName = /^[a-zA-Z0-9_\-/]+$/.test(newScriptName);
    if (!validName) {
      message.error(
        "Script name can only contain letters, numbers, hyphens, underscores and slashes",
      );
      return;
    }
    await saveScript(selectedType, newScriptName, editorContent);
    setShowNameModal(false);
    setNewScriptName("");
    setIsNewScript(false);
    message.success("Script created");
  }, [newScriptName, selectedType, editorContent, saveScript]);

  const handleDelete = useCallback(async () => {
    if (!selectedScript?.name) return;
    Modal.confirm({
      title: "Delete Script",
      content: `Are you sure you want to delete "${selectedScript.name}"?`,
      okText: "Delete",
      okType: "danger",
      onOk: async () => {
        await deleteScript(selectedType, selectedScript.name);
        message.success("Script deleted");
      },
    });
  }, [selectedScript, selectedType, deleteScript]);

  const handleDeleteScript = useCallback(
    async (type: ScriptType, name: string) => {
      await deleteScript(type, name);
    },
    [deleteScript],
  );

  const handleRenameScript = useCallback(
    async (type: ScriptType, oldName: string, newName: string) => {
      return await renameScript(type, oldName, newName);
    },
    [renameScript],
  );

  const handleTest = useCallback(async () => {
    await testScript(selectedType, editorContent);
  }, [selectedType, editorContent, testScript]);

  const handleNewScript = useCallback(
    (type: ScriptType) => {
      createNewScript(type);
      setIsNewScript(true);
    },
    [createNewScript],
  );

  const handleExport = useCallback(
    async (scriptNames: string[]) => {
      if (scriptNames.length === 0) return;
      await exportFile("script", { script_names: scriptNames });
    },
    [exportFile],
  );

  const handleExportAll = useCallback(async () => {
    const names = allScripts.map((s) => `${s.script_type}/${s.name}`);
    if (names.length === 0) return;
    await exportFile("script", { script_names: names });
  }, [allScripts, exportFile]);

  const handleImportSuccess = useCallback(() => {
    fetchScripts();
  }, [fetchScripts]);

  const openSandboxSettings = useCallback(async () => {
    setSandboxModalOpen(true);
    setSandboxLoading(true);
    try {
      const cfg = await getSandboxConfig();
      setSandboxEnabled(cfg.net.enabled);
      setSandboxDirName(cfg.file.sandbox_dir || "_sandbox");
      setSandboxDirsText((cfg.file.allowed_dirs || []).join("\n"));
      setSandboxFileMaxBytes(cfg.file.max_bytes);
      setSandboxNetMaxReqBytes(cfg.net.max_request_bytes);
      setSandboxNetMaxRespBytes(cfg.net.max_response_bytes);
      setSandboxNetTimeoutMs(cfg.net.timeout_ms);
      setSandboxTimeoutMs(cfg.limits.timeout_ms);
      setSandboxMaxMemoryBytes(cfg.limits.max_memory_bytes);
      setSandboxMaxDecodeInputBytes(cfg.limits.max_decode_input_bytes);
      setSandboxMaxDecompressOutputBytes(cfg.limits.max_decompress_output_bytes);
    } catch {
      message.error("加载 Sandbox 配置失败");
    } finally {
      setSandboxLoading(false);
    }
  }, []);

  const saveSandboxSettings = useCallback(async () => {
    const dirName = sandboxDirName.trim();
    if (!dirName) {
      message.error("Sandbox 目录不能为空");
      return;
    }

    const asPositiveInt = (v: number) => {
      if (!Number.isFinite(v)) return null;
      const n = Math.floor(v);
      return n > 0 ? n : null;
    };
    const fileMax = asPositiveInt(sandboxFileMaxBytes);
    const netReqMax = asPositiveInt(sandboxNetMaxReqBytes);
    const netRespMax = asPositiveInt(sandboxNetMaxRespBytes);
    const netTimeout = asPositiveInt(sandboxNetTimeoutMs);
    const timeoutMs = asPositiveInt(sandboxTimeoutMs);
    const memBytes = asPositiveInt(sandboxMaxMemoryBytes);
    const maxDecodeBytes = asPositiveInt(sandboxMaxDecodeInputBytes);
    const maxDecompressBytes = asPositiveInt(sandboxMaxDecompressOutputBytes);
    if (
      !fileMax ||
      !netReqMax ||
      !netRespMax ||
      !netTimeout ||
      !timeoutMs ||
      !memBytes ||
      !maxDecodeBytes ||
      !maxDecompressBytes
    ) {
      message.error("数值配置必须为正整数");
      return;
    }

    setSandboxSaving(true);
    try {
      const dirs = sandboxDirsText
        .split(/\r?\n/)
        .map((s) => s.trim())
        .filter(Boolean);

      const invalid = dirs.find((d) => !d.startsWith("/"));
      if (invalid) {
        message.error(`allowed_dirs 必须是绝对路径：${invalid}`);
        return;
      }

      await updateSandboxConfig({
        file: {
          sandbox_dir: dirName,
          allowed_dirs: dirs,
          max_bytes: fileMax,
        },
        net: {
          enabled: sandboxEnabled,
          timeout_ms: netTimeout,
          max_request_bytes: netReqMax,
          max_response_bytes: netRespMax,
        },
        limits: {
          timeout_ms: timeoutMs,
          max_memory_bytes: memBytes,
          max_decode_input_bytes: maxDecodeBytes,
          max_decompress_output_bytes: maxDecompressBytes,
        },
      });
      message.success("Sandbox 配置已保存");
      setSandboxModalOpen(false);
    } catch {
      message.error("保存 Sandbox 配置失败（请检查目录是否为绝对路径）");
    } finally {
      setSandboxSaving(false);
    }
  }, [
    sandboxDirsText,
    sandboxDirName,
    sandboxFileMaxBytes,
    sandboxEnabled,
    sandboxNetTimeoutMs,
    sandboxNetMaxReqBytes,
    sandboxNetMaxRespBytes,
    sandboxTimeoutMs,
    sandboxMaxMemoryBytes,
    sandboxMaxDecodeInputBytes,
    sandboxMaxDecompressOutputBytes,
  ]);

  const handleSearch = useCallback(
    (value: string) => {
      setSearchValue(value);
      if (value) {
        const matchedFolders = new Set<string>();
        for (const script of allScripts) {
          if (script.name.toLowerCase().includes(value.toLowerCase())) {
            const parts = script.name.split("/");
            parts.pop();
            let currentPath = "";
            for (const part of parts) {
              currentPath = currentPath ? `${currentPath}/${part}` : part;
              matchedFolders.add(`folder:${currentPath}`);
            }
          }
        }
        setExpandedKeys(Array.from(matchedFolders));
      } else {
        const allFolderKeys = getAllFolderKeys(allScripts);
        setExpandedKeys(allFolderKeys);
      }
    },
    [allScripts, getAllFolderKeys],
  );

  const handleSelectScript = useCallback(
    (type: ScriptType, name: string) => {
      selectScript(type, name);
      setIsNewScript(false);
    },
    [selectScript],
  );

  const handleToggleFolder = useCallback(
    (folderKey: string) => {
      setExpandedKeys((prev) =>
        prev.includes(folderKey)
          ? prev.filter((k) => k !== folderKey)
          : [...prev, folderKey],
      );
    },
    [],
  );

  const leftPanel = (
    <ScriptListPanel
      allScripts={allScripts}
      selectedScript={selectedScript}
      selectedType={selectedType}
      loading={loading}
      searchValue={searchValue}
      onSearchChange={handleSearch}
      onNewScript={handleNewScript}
      onSelectScript={handleSelectScript}
      onDeleteScript={handleDeleteScript}
      onRenameScript={handleRenameScript}
      onDeleteFolder={handleDeleteFolder}
      onExport={handleExport}
      onExportAll={handleExportAll}
      onImportSuccess={handleImportSuccess}
      onOpenSandboxSettings={openSandboxSettings}
      expandedKeys={expandedKeys}
      onToggleFolder={handleToggleFolder}
    />
  );

  const editorPanel = (
    <EditorPanel
      selectedScript={selectedScript}
      selectedType={selectedType}
      isNewScript={isNewScript}
      editorContent={editorContent}
      onEditorChange={setEditorContent}
      onSave={handleSave}
      onDelete={handleDelete}
      onTest={handleTest}
      saving={saving}
      testing={testing}
    />
  );

  const handleToggleTestResult = useCallback(() => {
    setTestResultExpanded((prev) => !prev);
  }, []);

  const testResultPanel = (
    <TestResultPanel
      testResult={testResult}
      isExpanded={testResultExpanded}
      onToggle={handleToggleTestResult}
    />
  );

  const rightPanel = testResultExpanded ? (
    <VerticalSplitPane
      top={editorPanel}
      bottom={testResultPanel}
      defaultTopHeight="60%"
      minTopHeight={200}
      minBottomHeight={120}
    />
  ) : (
    <div style={{ height: "100%", position: "relative" }}>
      {editorPanel}
      {testResultPanel}
    </div>
  );

  return (
    <>
      <div style={{ height: "100%", overflow: "hidden" }}>
        <SplitPane
          left={leftPanel}
          right={rightPanel}
          defaultLeftWidth="280px"
          minLeftWidth={200}
          minRightWidth={400}
        />
      </div>

      <Modal
        title="Save New Script"
        open={showNameModal}
        onOk={handleSaveNewScript}
        onCancel={() => setShowNameModal(false)}
        okText="Save"
      >
        <Input
          placeholder="Enter script name (e.g., api/add-auth-header)"
          value={newScriptName}
          onChange={(e) => setNewScriptName(e.target.value)}
          onPressEnter={handleSaveNewScript}
        />
        <Text type="secondary" style={{ display: "block", marginTop: 8 }}>
          Use "/" to create folders. Only letters, numbers, hyphens, underscores
          and slashes are allowed.
        </Text>
      </Modal>

      <Modal
        title="Sandbox 设置"
        open={sandboxModalOpen}
        onOk={saveSandboxSettings}
        onCancel={() => setSandboxModalOpen(false)}
        okText="保存"
        confirmLoading={sandboxSaving}
      >
        <Spin spinning={sandboxLoading}>
          <Form layout="vertical">
            <Form.Item label="网络请求 (net.fetch) 开关">
              <Switch checked={sandboxEnabled} onChange={setSandboxEnabled} />
            </Form.Item>
            <Form.Item label="Sandbox 目录 (scripts 下的相对目录名，或绝对路径)">
              <Input value={sandboxDirName} onChange={(e) => setSandboxDirName(e.target.value)} />
            </Form.Item>
            <Form.Item label="允许访问的系统目录 (allowed_dirs，每行一个绝对路径)">
              <Input.TextArea
                value={sandboxDirsText}
                onChange={(e) => setSandboxDirsText(e.target.value)}
                autoSize={{ minRows: 4, maxRows: 10 }}
                placeholder="/Users/xxx/data\n/var/log"
              />
            </Form.Item>
            <Form.Item label="文件最大字节数 (file.max_bytes)">
              <Input
                type="number"
                value={sandboxFileMaxBytes}
                onChange={(e) => setSandboxFileMaxBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="网络请求体最大字节数 (net.max_request_bytes)">
              <Input
                type="number"
                value={sandboxNetMaxReqBytes}
                onChange={(e) => setSandboxNetMaxReqBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="网络响应体最大字节数 (net.max_response_bytes)">
              <Input
                type="number"
                value={sandboxNetMaxRespBytes}
                onChange={(e) => setSandboxNetMaxRespBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="网络超时 (net.timeout_ms)">
              <Input
                type="number"
                value={sandboxNetTimeoutMs}
                onChange={(e) => setSandboxNetTimeoutMs(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="脚本超时 (limits.timeout_ms)">
              <Input
                type="number"
                value={sandboxTimeoutMs}
                onChange={(e) => setSandboxTimeoutMs(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="脚本最大内存 (limits.max_memory_bytes)">
              <Input
                type="number"
                value={sandboxMaxMemoryBytes}
                onChange={(e) => setSandboxMaxMemoryBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label="decode 输入最大字节数 (limits.max_decode_input_bytes)">
              <Input
                type="number"
                value={sandboxMaxDecodeInputBytes}
                onChange={(e) =>
                  setSandboxMaxDecodeInputBytes(Number(e.target.value))
                }
              />
            </Form.Item>
            <Form.Item label="HTTP 解压输出最大字节数 (limits.max_decompress_output_bytes)">
              <Input
                type="number"
                value={sandboxMaxDecompressOutputBytes}
                onChange={(e) =>
                  setSandboxMaxDecompressOutputBytes(Number(e.target.value))
                }
              />
            </Form.Item>
          </Form>
        </Spin>
      </Modal>
    </>
  );
}
