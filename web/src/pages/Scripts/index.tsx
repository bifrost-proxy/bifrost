// Legacy page: runtime language declarations are extracted. Split the remaining
// editor panes and script-management flows incrementally without changing behavior.
// Legacy page: runtime declarations now live in scriptEditorLanguage.ts; split
// the remaining list, editor, and result panels in a dedicated refactor.
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
  ImportOutlined,
  SettingOutlined,
  EditOutlined,
  MoreOutlined,
  CodeOutlined,
} from "@ant-design/icons";
import Editor from "@monaco-editor/react";
import { useScriptsStore } from "../../stores/useScriptsStore";
import { isDesktopShell } from "../../runtime";
import { registerDesktopMonacoCommands } from "../../components/MonacoDesktopCommands";
import { clearDesktopDocumentEdited } from "../../desktop/tauri";
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
import { configureScriptEditor } from "./scriptEditorLanguage";
import { useI18n } from "../../i18n";

const { Text } = Typography;

type TFunc = ReturnType<typeof useI18n>["t"];

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

function getScriptTypeLabel(type: ScriptType, t: TFunc) {
  switch (type) {
    case "request":
      return t("scripts.type.request");
    case "response":
      return t("scripts.type.response");
    case "decode":
      return t("scripts.type.decode");
    case "parser":
      return t("scripts.type.parser");
  }
}

function getScriptTypeShortLabel(type: ScriptType, t: TFunc) {
  switch (type) {
    case "request":
      return t("scripts.typeShort.request");
    case "response":
      return t("scripts.typeShort.response");
    case "decode":
      return t("scripts.typeShort.decode");
    case "parser":
      return t("scripts.typeShort.parser");
  }
}

function getScriptTypeTagColor(type: ScriptType) {
  switch (type) {
    case "request":
      return "blue";
    case "response":
      return "green";
    case "decode":
      return "purple";
    case "parser":
      return "geekblue";
  }
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
  const { t } = useI18n();
  const [selectedScripts, setSelectedScripts] = useState<string[]>([]);
  const lastClickedIndexRef = useRef<number | null>(null);
  const [renameTarget, setRenameTarget] = useState<{ type: ScriptType; name: string } | null>(null);
  const [newName, setNewName] = useState("");
  const [renameModalVisible, setRenameModalVisible] = useState(false);
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const hiddenImportRef = useRef<HTMLDivElement | null>(null);

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
            label: t("scripts.menu.newRequestScript"),
            onClick: () => onNewScript("request"),
          },
          {
            key: "new-response",
            label: t("scripts.menu.newResponseScript"),
            onClick: () => onNewScript("response"),
          },
          {
            key: "new-decode",
            label: t("scripts.menu.newDecodeScript"),
            onClick: () => onNewScript("decode"),
          },
          { type: "divider" },
          {
            key: "export-folder",
            icon: <ExportOutlined />,
            label: t("scripts.menu.exportFolder", { count: scriptsInFolder.length }),
            onClick: () => onExport(scriptNames),
            disabled: scriptsInFolder.length === 0,
          },
          { type: "divider" },
          {
            key: "delete-folder",
            label: t("scripts.menu.deleteFolder", { count: scriptsInFolder.length }),
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
          label: t("common.rename"),
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
          label:
            bulkKeys.length > 1
              ? t("scripts.menu.exportCount", { count: bulkKeys.length })
              : t("common.export"),
          onClick: () => onExport(bulkKeys),
        },
        { type: "divider" },
        {
          key: "delete",
          icon: <DeleteOutlined />,
          label:
            bulkKeys.length > 1
              ? t("scripts.menu.deleteCount", { count: bulkKeys.length })
              : t("common.delete"),
          danger: true,
          onClick: () => {
            if (bulkKeys.length > 1) {
              Modal.confirm({
                title: t("scripts.delete.scriptsTitle"),
                content: t("scripts.delete.scriptsContent", { count: bulkKeys.length }),
                okText: t("scripts.delete.deleteAll"),
                okType: "danger",
                onOk: async () => {
                  for (const key of bulkKeys) {
                    const [type, ...nameParts] = key.split("/");
                    onDeleteScript(type as ScriptType, nameParts.join("/"));
                  }
                  setSelectedScripts([]);
                  message.success(t("scripts.delete.scriptsDone", { count: bulkKeys.length }));
                },
              });
            } else {
              const [type, ...nameParts] = bulkKeys[0].split("/");
              const scriptName = nameParts.join("/");
              Modal.confirm({
                title: t("scripts.delete.scriptTitle"),
                content: t("scripts.delete.scriptContent", { name: scriptName }),
                okText: t("common.delete"),
                okType: "danger",
                onOk: async () => {
                  onDeleteScript(type as ScriptType, scriptName);
                  message.success(t("scripts.delete.scriptDone"));
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
      t,
    ],
  );

  const hasScripts = allScripts.length > 0;
  const createMenuItems = useMemo<MenuProps["items"]>(
    () => [
      {
        key: "request",
        icon: <FileOutlined />,
        label: <span data-testid="scripts-create-request-item">{t("scripts.list.createRequest")}</span>,
        onClick: () => onNewScript("request"),
      },
      {
        key: "response",
        icon: <FileOutlined />,
        label: <span data-testid="scripts-create-response-item">{t("scripts.list.createResponse")}</span>,
        onClick: () => onNewScript("response"),
      },
      {
        key: "decode",
        icon: <CodeOutlined />,
        label: <span data-testid="scripts-create-decode-item">{t("scripts.list.createDecode")}</span>,
        onClick: () => onNewScript("decode"),
      },
      {
        key: "parser",
        icon: <CodeOutlined />,
        label: <span data-testid="scripts-create-parser-item">{t("scripts.list.createParser")}</span>,
        onClick: () => onNewScript("parser"),
      },
    ],
    [onNewScript, t],
  );

  const moreMenuItems = useMemo<MenuProps["items"]>(
    () => [
      {
        key: "sandbox",
        icon: <SettingOutlined />,
        label: <span data-testid="scripts-more-sandbox-item">{t("scripts.list.sandboxSettings")}</span>,
        onClick: onOpenSandboxSettings,
      },
      {
        key: "export-all",
        icon: <ExportOutlined />,
        label: <span data-testid="scripts-more-export-all-item">{t("scripts.list.exportAll")}</span>,
        disabled: !hasScripts,
        onClick: onExportAll,
      },
      {
        key: "import",
        icon: <ImportOutlined />,
        label: <span data-testid="scripts-more-import-item">{t("common.import")}</span>,
        onClick: () => {
          hiddenImportRef.current?.querySelector("button")?.click();
        },
      },
    ],
    [hasScripts, onExportAll, onOpenSandboxSettings, t],
  );

  return (
    <div className={styles.container} data-testid="scripts-list-panel">
      <div className={styles.header}>
        <span className={styles.headerTitle}>{t("scripts.list.title")}</span>
        <div className={styles.headerActions}>
          <Dropdown
            menu={{ items: createMenuItems }}
            trigger={["click"]}
            placement="bottomRight"
          >
            <Tooltip title={t("scripts.list.newScript")}>
              <Button
                type="text"
                size="small"
                icon={<PlusOutlined />}
                data-testid="scripts-create-menu-button"
              />
            </Tooltip>
          </Dropdown>
          <Dropdown
            menu={{ items: moreMenuItems }}
            trigger={["click"]}
            placement="bottomRight"
          >
            <Tooltip title={t("scripts.list.moreActions")}>
              <Button
                type="text"
                size="small"
                icon={<MoreOutlined />}
                data-testid="scripts-more-menu-button"
              />
            </Tooltip>
          </Dropdown>
          <div ref={hiddenImportRef} className={styles.hiddenImport}>
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
      </div>

      <div className={styles.searchBox}>
        <Input
          placeholder={t("scripts.list.searchPlaceholder")}
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
        aria-label={t("scripts.list.ariaLabel")}
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
              const scriptType = node.scriptType;
              if (!scriptType) {
                return null;
              }

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
                    data-script-type={scriptType}
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
                          color={getScriptTypeTagColor(scriptType)}
                          className={styles.scriptTag}
                        >
                          {getScriptTypeShortLabel(scriptType, t)}
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
                {searchValue ? t("scripts.list.noMatching") : t("scripts.list.empty")}
              </div>
            )}
          </div>
        )}
      </div>

      <div className={styles.footer}>
        <span className={styles.stats}>{t("scripts.list.count", { count: allScripts.length })}</span>
      </div>

      <Modal
        title={t("scripts.rename.title")}
        open={renameModalVisible}
        onOk={async () => {
          if (!renameTarget || !newName.trim()) return;
          const success = await onRenameScript(renameTarget.type, renameTarget.name, newName.trim());
          if (success) {
            message.success(t("scripts.rename.success"));
            setRenameModalVisible(false);
            setRenameTarget(null);
            setNewName("");
          } else {
            message.error(t("scripts.rename.failed"));
          }
        }}
        onCancel={() => {
          setRenameModalVisible(false);
          setRenameTarget(null);
          setNewName("");
        }}
        okText={t("common.rename")}
        cancelText={t("common.cancel")}
      >
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onPressEnter={async () => {
            if (!renameTarget || !newName.trim()) return;
            const success = await onRenameScript(renameTarget.type, renameTarget.name, newName.trim());
            if (success) {
              message.success(t("scripts.rename.success"));
              setRenameModalVisible(false);
              setRenameTarget(null);
              setNewName("");
            } else {
              message.error(t("scripts.rename.failed"));
            }
          }}
          placeholder={t("scripts.rename.placeholder")}
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
  const { t } = useI18n();
  const resolvedTheme = useThemeStore((s) => s.resolvedTheme);
  const isStreamScript =
    selectedType === "response" && /\bstream\s*\./.test(editorContent);
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
      registerDesktopMonacoCommands(editor, isDesktopShell());

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
          <Empty description={t("scripts.editor.emptyDescription")} />
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
              {t("scripts.editor.howToUse")}
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
                  <b>{t("scripts.editor.help.requestLabel")}</b>{" "}
                  {t("scripts.editor.help.requestText")}
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  <b>{t("scripts.editor.help.responseLabel")}</b>{" "}
                  {t("scripts.editor.help.responseText")}
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  {t("scripts.editor.help.streamBefore")}{" "}
                  <code>resStreamScript://</code>{" "}
                  {t("scripts.editor.help.streamAfter")}
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  {t("scripts.editor.help.logBefore")}{" "}
                  <code>log.info()</code>, <code>log.warn()</code>{" "}
                  {t("scripts.editor.help.logAfter")}
                </Text>
              </li>
              <li style={{ marginBottom: 8 }}>
                <Text type="secondary">
                  {t("scripts.editor.help.valuesBefore")}{" "}
                  <code>ctx.values</code>{" "}
                  {t("scripts.editor.help.valuesAfter")}
                </Text>
              </li>
            </ul>
            <Text
              strong
              style={{ display: "block", marginTop: 16, marginBottom: 8 }}
            >
              {t("scripts.editor.bindTitle")}
            </Text>
            <Text
              type="secondary"
              style={{ display: "block", marginBottom: 8 }}
            >
              {t("scripts.editor.bindDescription")}
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
            >{`api.example.com reqScript://{add_auth} resStreamScript://{map_sse}

\`\`\`add_auth
request.headers["Authorization"] = "Bearer " + ctx.values.token;
\`\`\`

\`\`\`map_sse
stream.mode = "transform";
stream.onEvent = (event) => ({ event: event.event, data: event.data });
\`\`\``}</pre>
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
            {isNewScript ? t("scripts.list.newScript") : selectedScript.name}
          </Text>
          <Tag
            color={getScriptTypeTagColor(selectedType)}
            style={{ margin: 0 }}
          >
            {getScriptTypeLabel(selectedType, t)}
          </Tag>
        </Space>
        <Space size={8}>
          <Button
            size="small"
            icon={<PlayCircleOutlined />}
            onClick={onTest}
            loading={testing}
            data-testid="scripts-test-button"
            disabled={isStreamScript}
            title={
              isStreamScript
                ? t("scripts.editor.streamRunDisabled")
                : undefined
            }
          >
            {t("scripts.editor.run")}
          </Button>
          <Button
            type="primary"
            size="small"
            icon={<SaveOutlined />}
            onClick={onSave}
            loading={saving}
            data-testid="scripts-save-button"
          >
            {t("common.save")}
          </Button>
          {!isNewScript && (
            <Button
              size="small"
              danger
              icon={<DeleteOutlined />}
              onClick={onDelete}
              data-testid="scripts-delete-button"
            >
              {t("common.delete")}
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
          beforeMount={(monaco) => configureScriptEditor(monaco, selectedType)}
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
  const { t } = useI18n();
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
        <Tooltip title={t("scripts.test.show")}>
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
                <span>{t("scripts.test.result")}</span>
              </>
            ) : (
              <span>{t("scripts.test.results")}</span>
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
              <Text strong>
                {t("scripts.test.resultWithDuration", { duration: testResult.duration_ms })}
              </Text>
            </>
          ) : (
            <Text strong>{t("scripts.test.results")}</Text>
          )}
        </div>
        <Tooltip title={t("scripts.test.hide")}>
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
            <Text type="secondary">{t("scripts.test.emptyHint")}</Text>
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
                <Text strong>{t("scripts.test.decodeOutput")}</Text>
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
                <Text strong>{t("scripts.test.requestModifications")}</Text>
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
                <Text strong>{t("scripts.test.responseModifications")}</Text>
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
              <Text strong>{t("scripts.test.logs")}</Text>
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
                  {t("scripts.test.noLogs")}
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
  const { t } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const {
    requestScripts,
    responseScripts,
    decodeScripts,
    parserScripts,
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
  const [sandboxAllowPrivateNetwork, setSandboxAllowPrivateNetwork] = useState(false);
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
    const scripts = [...requestScripts, ...responseScripts, ...decodeScripts, ...parserScripts];

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
    parserScripts,
    selectScript,
    setSearchParams,
  ]);

  const allScripts = useMemo(
    () => [...requestScripts, ...responseScripts, ...decodeScripts, ...parserScripts],
    [requestScripts, responseScripts, decodeScripts, parserScripts],
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
        message.info(t("scripts.delete.noScriptsInFolder"));
        return;
      }

      Modal.confirm({
        title: t("scripts.delete.folderTitle"),
        content: t("scripts.delete.folderContent", {
          folder: folderPath,
          count: scriptsToDelete.length,
        }),
        okText: t("scripts.delete.deleteAll"),
        okType: "danger",
        onOk: async () => {
          for (const script of scriptsToDelete) {
            await deleteScript(script.type, script.name);
          }
          message.success(t("scripts.delete.folderDone", { count: scriptsToDelete.length }));
        },
      });
    },
    [allScripts, deleteScript, t],
  );

  const handleSave = useCallback(async () => {
    if (isNewScript) {
      setShowNameModal(true);
      return;
    }
    if (selectedScript?.name) {
      const success = await saveScript(selectedType, selectedScript.name, editorContent);
      if (success) {
        await clearDesktopDocumentEdited().catch(() => undefined);
        message.success(t("scripts.save.saved"));
      }
    }
  }, [isNewScript, selectedScript, selectedType, editorContent, saveScript, t]);

  const handleSaveNewScript = useCallback(async () => {
    if (!newScriptName.trim()) {
      message.error(t("scripts.save.nameRequired"));
      return;
    }
    const validName = /^[a-zA-Z0-9_\-/]+$/.test(newScriptName);
    if (!validName) {
      message.error(t("scripts.save.nameInvalid"));
      return;
    }
    const success = await saveScript(selectedType, newScriptName, editorContent);
    if (!success) return;

    await clearDesktopDocumentEdited().catch(() => undefined);
    setShowNameModal(false);
    setNewScriptName("");
    setIsNewScript(false);
    message.success(t("scripts.save.created"));
  }, [newScriptName, selectedType, editorContent, saveScript, t]);

  const handleDelete = useCallback(async () => {
    if (!selectedScript?.name) return;
    Modal.confirm({
      title: t("scripts.delete.scriptTitle"),
      content: t("scripts.delete.scriptContent", { name: selectedScript.name }),
      okText: t("common.delete"),
      okType: "danger",
      onOk: async () => {
        await deleteScript(selectedType, selectedScript.name);
        message.success(t("scripts.delete.scriptDone"));
      },
    });
  }, [selectedScript, selectedType, deleteScript, t]);

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
      setSandboxAllowPrivateNetwork(cfg.net.allow_private_network);
      setSandboxTimeoutMs(cfg.limits.timeout_ms);
      setSandboxMaxMemoryBytes(cfg.limits.max_memory_bytes);
      setSandboxMaxDecodeInputBytes(cfg.limits.max_decode_input_bytes);
      setSandboxMaxDecompressOutputBytes(cfg.limits.max_decompress_output_bytes);
    } catch {
      message.error(t("scripts.sandbox.loadFailed"));
    } finally {
      setSandboxLoading(false);
    }
  }, [t]);

  const saveSandboxSettings = useCallback(async () => {
    const dirName = sandboxDirName.trim();
    if (!dirName) {
      message.error(t("scripts.sandbox.dirEmpty"));
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
      message.error(t("scripts.sandbox.positiveInt"));
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
        message.error(t("scripts.sandbox.allowedDirsAbsolute", { dir: invalid }));
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
          allow_private_network: sandboxAllowPrivateNetwork,
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
      message.success(t("scripts.sandbox.saved"));
      setSandboxModalOpen(false);
    } catch {
      message.error(t("scripts.sandbox.saveFailed"));
    } finally {
      setSandboxSaving(false);
    }
  }, [
    sandboxDirsText,
    sandboxDirName,
    sandboxFileMaxBytes,
    sandboxEnabled,
    sandboxAllowPrivateNetwork,
    sandboxNetTimeoutMs,
    sandboxNetMaxReqBytes,
    sandboxNetMaxRespBytes,
    sandboxTimeoutMs,
    sandboxMaxMemoryBytes,
    sandboxMaxDecodeInputBytes,
    sandboxMaxDecompressOutputBytes,
    t,
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
        title={t("scripts.save.newTitle")}
        open={showNameModal}
        onOk={handleSaveNewScript}
        onCancel={() => setShowNameModal(false)}
        okText={t("common.save")}
      >
        <Input
          placeholder={t("scripts.save.namePlaceholder")}
          value={newScriptName}
          onChange={(e) => setNewScriptName(e.target.value)}
          onPressEnter={handleSaveNewScript}
        />
        <Text type="secondary" style={{ display: "block", marginTop: 8 }}>
          {t("scripts.save.nameHint")}
        </Text>
      </Modal>

      <Modal
        title={t("scripts.sandbox.title")}
        open={sandboxModalOpen}
        onOk={saveSandboxSettings}
        onCancel={() => setSandboxModalOpen(false)}
        okText={t("common.save")}
        confirmLoading={sandboxSaving}
      >
        <Spin spinning={sandboxLoading}>
          <Form layout="vertical">
            <Form.Item label={t("scripts.sandbox.netToggle")}>
              <Switch checked={sandboxEnabled} onChange={setSandboxEnabled} />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.dir")}>
              <Input value={sandboxDirName} onChange={(e) => setSandboxDirName(e.target.value)} />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.allowedDirs")}>
              <Input.TextArea
                value={sandboxDirsText}
                onChange={(e) => setSandboxDirsText(e.target.value)}
                autoSize={{ minRows: 4, maxRows: 10 }}
                placeholder="/Users/xxx/data\n/var/log"
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.fileMaxBytes")}>
              <Input
                type="number"
                value={sandboxFileMaxBytes}
                onChange={(e) => setSandboxFileMaxBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.netMaxRequestBytes")}>
              <Input
                type="number"
                value={sandboxNetMaxReqBytes}
                onChange={(e) => setSandboxNetMaxReqBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.netMaxResponseBytes")}>
              <Input
                type="number"
                value={sandboxNetMaxRespBytes}
                onChange={(e) => setSandboxNetMaxRespBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.netTimeout")}>
              <Input
                type="number"
                value={sandboxNetTimeoutMs}
                onChange={(e) => setSandboxNetTimeoutMs(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item
              label={t("scripts.sandbox.allowPrivateNetwork")}
              extra={t("scripts.sandbox.allowPrivateNetworkExtra")}
            >
              <Switch
                checked={sandboxAllowPrivateNetwork}
                onChange={setSandboxAllowPrivateNetwork}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.timeout")}>
              <Input
                type="number"
                value={sandboxTimeoutMs}
                onChange={(e) => setSandboxTimeoutMs(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.maxMemory")}>
              <Input
                type="number"
                value={sandboxMaxMemoryBytes}
                onChange={(e) => setSandboxMaxMemoryBytes(Number(e.target.value))}
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.maxDecodeInput")}>
              <Input
                type="number"
                value={sandboxMaxDecodeInputBytes}
                onChange={(e) =>
                  setSandboxMaxDecodeInputBytes(Number(e.target.value))
                }
              />
            </Form.Item>
            <Form.Item label={t("scripts.sandbox.maxDecompressOutput")}>
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
