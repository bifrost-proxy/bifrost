import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { editor as MonacoEditor, KeyCode, KeyMod, Position } from "monaco-editor";
import type { IRange } from "monaco-editor";
import { Empty, Spin, message, Button, Space, Modal } from "antd";
import { SaveOutlined, CopyOutlined, DeleteOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
import { getRule, getRuleReferenceCandidates, type RuleReferenceCandidate } from "../../../api";
import { getGroupRule } from "../../../api/group";
import { copyToClipboard } from "../../../utils/clipboard";
import { isDesktopShell } from "../../../runtime";
import { registerDesktopMonacoCommands } from "../../../components/MonacoDesktopCommands";
import BifrostEditor, {
  THEME_DARK,
  THEME_LIGHT,
  createDebouncedValidator,
  clearValidationMarkers,
  setNavigateCallback,
  setLocalVariables,
  setLocalVariablesGetter,
  setRuleReferenceErrors,
  updateDynamicData,
  type DebouncedValidator,
  type ReferenceLocation,
  type ValidationResult,
} from "../../../components/BifrostEditor";
import { useRulesStore } from "../../../stores/useRulesStore";
import { useThemeStore } from "../../../stores/useThemeStore";
import { useValuesStore } from "../../../stores/useValuesStore";
import type { RuleSyncInfo } from "../../../types";
import {
  findRuleReferenceAtColumn,
  parseRuleReferenceLine,
} from "../../../components/BifrostEditor/snippet/ruleReference";
import styles from "./index.module.css";

function formatRuleTime(value?: string | null): string {
  if (!value) return "--";
  const date = dayjs(value);
  return date.isValid() ? date.format("YYYY-MM-DD HH:mm:ss") : value;
}

function formatSyncStatus(sync?: RuleSyncInfo | null): string {
  switch (sync?.status) {
    case "synced":
      return "Synced";
    case "modified":
      return "Modified";
    default:
      return "Local only";
  }
}

function normalizeRuleEditorContent(content: string | undefined | null): string {
  return (content ?? "").replace(/\r\n/g, "\n").replace(/\n+$/g, "");
}

interface RuleReferenceMatch {
  name: string;
  range: IRange;
}

function findRuleReferenceAtPosition(
  model: MonacoEditor.ITextModel,
  position: Position,
): RuleReferenceMatch | null {
  const lineContent = model.getLineContent(position.lineNumber);
  const reference = findRuleReferenceAtColumn(lineContent, position.column);
  if (!reference) return null;
  return {
    name: reference.name,
    range: {
      startLineNumber: position.lineNumber,
      endLineNumber: position.lineNumber,
      startColumn: reference.startColumn,
      endColumn: reference.endColumn,
    },
  };
}

const RULE_REFERENCE_ZONE_LINE_HEIGHT = 18;
const RULE_REFERENCE_ZONE_CHROME = 52;
const RULE_REFERENCE_ZONE_MIN_HEIGHT = 96;
const RULE_REFERENCE_ZONE_MAX_HEIGHT = 320;

function ruleReferenceZoneHeight(content: string): number {
  const lineCount = content.length === 0 ? 1 : content.split("\n").length;
  const estimated =
    RULE_REFERENCE_ZONE_CHROME + lineCount * RULE_REFERENCE_ZONE_LINE_HEIGHT;
  return Math.min(
    RULE_REFERENCE_ZONE_MAX_HEIGHT,
    Math.max(RULE_REFERENCE_ZONE_MIN_HEIGHT, estimated),
  );
}

function findRuleReferences(model: MonacoEditor.ITextModel): RuleReferenceMatch[] {
  const matches: RuleReferenceMatch[] = [];
  for (let lineNumber = 1; lineNumber <= model.getLineCount(); lineNumber += 1) {
    const lineContent = model.getLineContent(lineNumber);
    const reference = parseRuleReferenceLine(lineContent);
    if (!reference) continue;
    matches.push({
      name: reference.name,
      range: {
        startLineNumber: lineNumber,
        endLineNumber: lineNumber,
        startColumn: reference.startColumn,
        endColumn: reference.endColumn,
      },
    });
  }
  return matches;
}

export default function RuleEditor() {
  const navigate = useNavigate();
  const {
    currentRule,
    selectedRuleName,
    editingContent,
    savedContent,
    loading,
    saving,
    isGroupMode,
    activeGroupId,
    activeGroupName,
    groupWritable,
    rules,
    setEditingContent,
    clearEditingContent,
    saveCurrentRule,
    deleteRule,
  } = useRulesStore();
  const { resolvedTheme } = useThemeStore();
  const { values } = useValuesStore();
  const canEdit = !isGroupMode || groupWritable;

  const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null);
  const modelRef = useRef<MonacoEditor.ITextModel | null>(null);
  const saveRef = useRef<typeof saveCurrentRule | null>(null);
  const validatorRef = useRef<DebouncedValidator | null>(null);
  const isSettingValueRef = useRef(false);
  const valuesRef = useRef(values);
  const localVariablesRef = useRef<Array<{ name: string; line: number }>>([]);
  const ruleReferenceDecorationIdsRef = useRef<string[]>([]);
  const ruleReferenceValidationErrorsRef = useRef<Map<number, string>>(new Map());
  const refreshRuleReferenceDecorationsRef = useRef<() => void>(() => {});
  const ruleReferenceCandidatesRef = useRef<Map<string, RuleReferenceCandidate>>(new Map());
  const expandedRuleReferenceRef = useRef<{
    key: string;
    zoneId: string;
    domNode: HTMLDivElement;
  } | null>(null);
  const ruleDetailCacheRef = useRef<Map<string, string>>(new Map());
  const currentRuleRef = useRef<{
    currentRule: typeof currentRule;
    selectedRuleName: typeof selectedRuleName;
    editingContent: typeof editingContent;
    savedContent: typeof savedContent;
    activeGroupId: typeof activeGroupId;
    activeGroupName: typeof activeGroupName;
    isGroupMode: typeof isGroupMode;
  } | null>(null);
  const [containerElement, setContainerElement] =
    useState<HTMLDivElement | null>(null);

  const handleValidationComplete = useCallback(
    (result: ValidationResult) => {
      const localVars = (result.defined_variables || [])
        .filter((v) => v.defined_at != null)
        .map((v) => ({ name: v.name, line: v.defined_at! }));
      setLocalVariables(localVars);
      localVariablesRef.current = localVars;

      const referenceErrors = new Map<number, string>();
      for (const error of result.errors || []) {
        if (error.code === "E020") {
          referenceErrors.set(error.line, error.message);
        }
      }
      ruleReferenceValidationErrorsRef.current = referenceErrors;
      setRuleReferenceErrors(
        Array.from(referenceErrors.entries()).map(([line, message]) => ({ line, message })),
      );
      refreshRuleReferenceDecorationsRef.current();
    },
    []
  );

  useEffect(() => {
    setLocalVariablesGetter(() => localVariablesRef.current);
    return () => {
      setLocalVariablesGetter(() => []);
    };
  }, []);

  useEffect(() => {
    saveRef.current = saveCurrentRule;
  }, [saveCurrentRule]);

  useEffect(() => {
    valuesRef.current = values;
  }, [values]);

  useEffect(() => {
    currentRuleRef.current = {
      currentRule,
      selectedRuleName,
      editingContent,
      savedContent,
      activeGroupId,
      activeGroupName,
      isGroupMode,
    };
  }, [
    currentRule,
    selectedRuleName,
    editingContent,
    savedContent,
    activeGroupId,
    activeGroupName,
    isGroupMode,
  ]);

  useEffect(() => {
    ruleDetailCacheRef.current.clear();
    const localCandidates = rules.map((rule) => ({
      name: activeGroupName ? `${activeGroupName}/${rule.name}` : rule.name,
      rule_name: rule.name,
      group_name: activeGroupName,
      group_id: activeGroupId,
    }));
    ruleReferenceCandidatesRef.current = new Map(
      localCandidates.map((candidate) => [candidate.name, candidate]),
    );
    updateDynamicData({ rules: localCandidates.map((candidate) => candidate.name) });
    refreshRuleReferenceDecorationsRef.current();

    let cancelled = false;
    getRuleReferenceCandidates()
      .then((candidates) => {
        if (cancelled) return;
        const merged = new Map<string, RuleReferenceCandidate>();
        for (const candidate of candidates) {
          merged.set(candidate.name, candidate);
        }
        for (const candidate of localCandidates) {
          merged.set(candidate.name, candidate);
        }
        ruleReferenceCandidatesRef.current = merged;
        updateDynamicData({ rules: Array.from(merged.keys()) });
        refreshRuleReferenceDecorationsRef.current();
      })
      .catch(() => {
        if (!cancelled) {
          updateDynamicData({ rules: localCandidates.map((candidate) => candidate.name) });
          refreshRuleReferenceDecorationsRef.current();
        }
      });

    return () => {
      cancelled = true;
    };
  }, [rules, activeGroupId, activeGroupName]);

  const collapseRuleReference = useCallback(() => {
    const expanded = expandedRuleReferenceRef.current;
    const editor = editorRef.current;
    if (!expanded || !editor) {
      expandedRuleReferenceRef.current = null;
      return;
    }
    editor.changeViewZones((accessor) => {
      accessor.removeZone(expanded.zoneId);
    });
    expandedRuleReferenceRef.current = null;
  }, []);

  const renderRuleReferenceZone = useCallback(
    (
      node: HTMLDivElement,
      name: string,
      state: "loading" | "loaded" | "error",
      content?: string,
    ) => {
      node.replaceChildren();
      node.className = styles.ruleReferenceZone;
      node.setAttribute("data-testid", "rule-reference-zone");
      node.setAttribute("data-rule-reference-name", name);

      const header = document.createElement("div");
      header.className = styles.ruleReferenceZoneHeader;

      const title = document.createElement("span");
      title.className = styles.ruleReferenceZoneTitle;
      title.textContent = `@${name}`;
      header.appendChild(title);

      const close = document.createElement("button");
      close.type = "button";
      close.className = styles.ruleReferenceZoneClose;
      close.title = "Close";
      close.setAttribute("aria-label", `Close @${name} details`);
      close.textContent = "x";
      close.addEventListener("click", collapseRuleReference);
      header.appendChild(close);

      const body = document.createElement("pre");
      body.className = styles.ruleReferenceZoneBody;
      body.textContent =
        state === "loading"
          ? "Loading..."
          : state === "error"
            ? content || "Rule not found"
            : content || "";

      node.appendChild(header);
      node.appendChild(body);
    },
    [collapseRuleReference],
  );

  const refreshRuleReferenceDecorations = useCallback(() => {
    const editor = editorRef.current;
    const model = modelRef.current;
    if (!editor || !model || model.isDisposed()) return;

    const decorations = findRuleReferences(model).map((reference) => {
      const errorMessage = ruleReferenceValidationErrorsRef.current.get(
        reference.range.startLineNumber,
      ) ?? (
        ruleReferenceCandidatesRef.current.has(reference.name)
          ? null
          : `Rule reference '@${reference.name}' was not found.`
      );

      return {
        range: reference.range,
        options: {
          inlineClassName: errorMessage
            ? `${styles.ruleReferenceDecoration} ${styles.ruleReferenceErrorDecoration}`
            : styles.ruleReferenceDecoration,
          hoverMessage: {
            value: errorMessage
              ? `Rule reference error: \`@${reference.name}\`\n\n${errorMessage}`
              : `Rule reference \`@${reference.name}\`. Click to expand inline.`,
          },
        },
      };
    });

    ruleReferenceDecorationIdsRef.current = editor.deltaDecorations(
      ruleReferenceDecorationIdsRef.current,
      decorations,
    );
  }, []);

  useEffect(() => {
    refreshRuleReferenceDecorationsRef.current = refreshRuleReferenceDecorations;
  }, [refreshRuleReferenceDecorations]);

  const loadRuleReferenceContent = useCallback(async (name: string): Promise<string> => {
    const cached = ruleDetailCacheRef.current.get(name);
    if (cached !== undefined) return cached;

    const snapshot = currentRuleRef.current;
    const currentQualifiedName =
      snapshot?.isGroupMode && snapshot.activeGroupName && snapshot.currentRule?.name
        ? `${snapshot.activeGroupName}/${snapshot.currentRule.name}`
        : snapshot?.currentRule?.name;
    if (snapshot?.currentRule && currentQualifiedName === name) {
      const content =
        snapshot.editingContent[snapshot.currentRule.name] !== undefined
          ? snapshot.editingContent[snapshot.currentRule.name]
          : snapshot.currentRule.content || "";
      ruleDetailCacheRef.current.set(name, content);
      return content;
    }

    const candidate = ruleReferenceCandidatesRef.current.get(name);
    const fallbackGroupSeparator = name.indexOf("/");
    const fallbackGroupName =
      !candidate && fallbackGroupSeparator > 0 ? name.slice(0, fallbackGroupSeparator) : null;
    const fallbackRuleName =
      !candidate && fallbackGroupSeparator > 0 ? name.slice(fallbackGroupSeparator + 1) : null;
    const groupRef = candidate?.group_id || candidate?.group_name || fallbackGroupName;
    const ruleName = candidate?.rule_name || fallbackRuleName || name;

    const detail = groupRef ? await getGroupRule(groupRef, ruleName) : await getRule(ruleName);
    ruleDetailCacheRef.current.set(name, detail.content);
    return detail.content;
  }, []);

  const toggleRuleReferenceExpansion = useCallback(
    async (reference: RuleReferenceMatch) => {
      const editor = editorRef.current;
      if (!editor) return;

      const key = `${reference.range.startLineNumber}:${reference.name}`;
      if (expandedRuleReferenceRef.current?.key === key) {
        collapseRuleReference();
        return;
      }

      collapseRuleReference();

      const domNode = document.createElement("div");
      renderRuleReferenceZone(domNode, reference.name, "loading");
      const zone: MonacoEditor.IViewZone = {
        afterLineNumber: reference.range.endLineNumber,
        heightInPx: ruleReferenceZoneHeight(""),
        domNode,
      };
      let zoneId = "";
      editor.changeViewZones((accessor) => {
        zoneId = accessor.addZone(zone);
      });
      expandedRuleReferenceRef.current = { key, zoneId, domNode };

      const relayoutZone = (content: string) => {
        const currentEditor = editorRef.current;
        if (!currentEditor || expandedRuleReferenceRef.current?.key !== key) return;
        zone.heightInPx = ruleReferenceZoneHeight(content);
        currentEditor.changeViewZones((accessor) => {
          accessor.layoutZone(zoneId);
        });
      };

      try {
        const content = await loadRuleReferenceContent(reference.name);
        if (expandedRuleReferenceRef.current?.key !== key) return;
        renderRuleReferenceZone(domNode, reference.name, "loaded", content);
        relayoutZone(content);
      } catch (error) {
        if (expandedRuleReferenceRef.current?.key !== key) return;
        renderRuleReferenceZone(
          domNode,
          reference.name,
          "error",
          error instanceof Error ? error.message : "Rule not found",
        );
      }
    },
    [collapseRuleReference, loadRuleReferenceContent, renderRuleReferenceZone],
  );

  const handleChange = useCallback(() => {
    if (isSettingValueRef.current) return;
    if (!modelRef.current || modelRef.current.isDisposed()) return;
    const selectedName = currentRuleRef.current?.selectedRuleName;
    if (!selectedName) return;

    collapseRuleReference();
    const content = modelRef.current.getValue();
    ruleDetailCacheRef.current.set(selectedName, content);
    const savedContent =
      currentRuleRef.current?.currentRule?.name === selectedName
        ? currentRuleRef.current.savedContent[selectedName] ??
          currentRuleRef.current.currentRule.content
        : undefined;
    if (
      savedContent !== undefined &&
      normalizeRuleEditorContent(content) === normalizeRuleEditorContent(savedContent)
    ) {
      clearEditingContent(selectedName);
    } else {
      setEditingContent(selectedName, content);
    }
    refreshRuleReferenceDecorations();

    if (validatorRef.current && modelRef.current) {
      validatorRef.current.validate(modelRef.current, handleValidationComplete);
    }
  }, [
    collapseRuleReference,
    clearEditingContent,
    refreshRuleReferenceDecorations,
    setEditingContent,
    handleValidationComplete,
  ]);

  const handleSave = useCallback(async () => {
    if (!saveRef.current) return;
    const success = await saveRef.current();
    if (success) {
      message.success("Saved");
    }
  }, []);

  const handleCopy = useCallback(async () => {
    if (!modelRef.current || modelRef.current.isDisposed()) return;
    const content = modelRef.current.getValue();
    try {
      await copyToClipboard(content);
      message.success("Copied");
    } catch {
      message.error("Failed to copy");
    }
  }, []);

  const handleDelete = useCallback(() => {
    if (!currentRule?.name) return;
    Modal.confirm({
      title: "Delete Rule",
      content: `Are you sure to delete "${currentRule.name}"?`,
      okText: "Delete",
      okType: "danger",
      cancelText: "Cancel",
      onOk: async () => {
        const success = await deleteRule(currentRule.name);
        if (success) {
          message.success("Rule deleted");
        }
      },
    });
  }, [currentRule, deleteRule]);

  const handleNavigate = useCallback(
    (location: ReferenceLocation) => {
      if (location.navigationType === "page" && location.uri) {
        navigate(location.uri);
      }
    },
    [navigate]
  );

  useEffect(() => {
    setNavigateCallback(handleNavigate);
    return () => {
      setNavigateCallback(null);
    };
  }, [handleNavigate]);

  useEffect(() => {
    if (!containerElement) return;
    if (editorRef.current) return;

    const editorTheme = resolvedTheme === "dark" ? THEME_DARK : THEME_LIGHT;
    const rule = currentRuleRef.current?.currentRule;
    const edited = rule
      ? currentRuleRef.current?.editingContent[rule.name]
      : undefined;
    const initialContent = rule
      ? edited !== undefined
        ? edited
        : rule.content || ""
      : "";
    const model = BifrostEditor.createModel(initialContent);
    const ed = BifrostEditor.create(containerElement, {
      theme: editorTheme,
      readOnly: !canEdit,
    });

    registerDesktopMonacoCommands(ed, isDesktopShell());

    ed.setModel(model);
    ed.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => {
      handleSave();
    });

    const changeDisposable = model.onDidChangeContent(() => {
      handleChange();
    });
    const mouseDownDisposable = ed.onMouseDown((event) => {
      if (!event.target.position || event.event.metaKey || event.event.ctrlKey) {
        return;
      }
      const reference = findRuleReferenceAtPosition(model, event.target.position);
      if (!reference) return;
      event.event.preventDefault();
      toggleRuleReferenceExpansion(reference);
    });

    const getGlobalValues = () => {
      const result: Record<string, string> = {};
      for (const v of valuesRef.current) {
        result[v.name] = v.value;
      }
      return result;
    };

    const validator = createDebouncedValidator(
      500,
      getGlobalValues,
      () => currentRuleRef.current?.selectedRuleName,
      () =>
        currentRuleRef.current?.isGroupMode
          ? currentRuleRef.current?.activeGroupName
          : null,
    );
    validatorRef.current = validator;

    if (initialContent) {
      validator.validate(model, handleValidationComplete);
    }

    editorRef.current = ed;
    modelRef.current = model;
    refreshRuleReferenceDecorations();

    return () => {
      collapseRuleReference();
      validator.cancel();
      validatorRef.current = null;
      clearValidationMarkers(model);
      setLocalVariables([]);
      setRuleReferenceErrors([]);
      ruleReferenceValidationErrorsRef.current.clear();
      ed.deltaDecorations(ruleReferenceDecorationIdsRef.current, []);
      ruleReferenceDecorationIdsRef.current = [];
      mouseDownDisposable.dispose();
      changeDisposable.dispose();
      model.dispose();
      ed.dispose();
      editorRef.current = null;
      modelRef.current = null;
    };
  }, [
    canEdit,
    collapseRuleReference,
    containerElement,
    handleChange,
    handleSave,
    handleValidationComplete,
    refreshRuleReferenceDecorations,
    resolvedTheme,
    toggleRuleReferenceExpansion,
  ]);

  useEffect(() => {
    if (!editorRef.current) return;
    const editorTheme = resolvedTheme === "dark" ? THEME_DARK : THEME_LIGHT;
    editorRef.current.updateOptions({ theme: editorTheme });
  }, [resolvedTheme]);

  useEffect(() => {
    if (!editorRef.current) return;
    editorRef.current.updateOptions({ readOnly: !canEdit });
  }, [canEdit]);

  useEffect(() => {
    if (
      !modelRef.current ||
      !editorRef.current ||
      modelRef.current.isDisposed()
    ) {
      return;
    }

    if (!currentRule) {
      collapseRuleReference();
      isSettingValueRef.current = true;
      modelRef.current.setValue("");
      isSettingValueRef.current = false;
      setRuleReferenceErrors([]);
      ruleReferenceValidationErrorsRef.current.clear();
      refreshRuleReferenceDecorations();
      return;
    }

    const edited = editingContent[currentRule.name];
    const content = edited !== undefined ? edited : currentRule.content || "";
    const currentContent = modelRef.current.getValue();
    ruleDetailCacheRef.current.set(currentRule.name, content);

    if (currentContent !== content) {
      collapseRuleReference();
      isSettingValueRef.current = true;
      modelRef.current.setValue(content);
      isSettingValueRef.current = false;
      setRuleReferenceErrors([]);
      ruleReferenceValidationErrorsRef.current.clear();
      editorRef.current.setScrollTop(0);
      editorRef.current.setScrollLeft(0);
      refreshRuleReferenceDecorations();

      if (validatorRef.current && modelRef.current) {
        validatorRef.current.validate(modelRef.current, handleValidationComplete);
      }
    }
  }, [
    collapseRuleReference,
    currentRule,
    editingContent,
    containerElement,
    handleValidationComplete,
    refreshRuleReferenceDecorations,
  ]);

  if (!selectedRuleName) {
    return (
      <div className={styles.empty}>
        <Empty description="Select a rule to edit" />
      </div>
    );
  }

  if (loading && !currentRule) {
    return (
      <div className={styles.loading}>
        <Spin size="large" />
      </div>
    );
  }

  const hasChanges =
    selectedRuleName &&
    editingContent[selectedRuleName] !== undefined &&
    normalizeRuleEditorContent(editingContent[selectedRuleName]) !==
      normalizeRuleEditorContent(savedContent[selectedRuleName] ?? currentRule?.content);
  const metaItems = currentRule
    ? isGroupMode
      ? [
          `Created ${formatRuleTime(currentRule.created_at)}`,
          `Updated ${formatRuleTime(currentRule.updated_at)}`,
        ]
      : [
          `Sync ${formatSyncStatus(currentRule.sync)}`,
          `Created ${formatRuleTime(currentRule.created_at)}`,
          `Updated ${formatRuleTime(currentRule.updated_at)}`,
          `Last sync ${formatRuleTime(currentRule.sync?.last_synced_at)}`,
        ]
    : [];

  return (
    <div className={styles.container} data-testid="rule-editor">
      <div className={styles.header}>
        <div className={styles.titleSection}>
          <div className={styles.titleBlock}>
            <span className={styles.title} data-testid="rule-editor-title">{currentRule?.name}</span>
            {currentRule && (
              <div className={styles.metaRow} data-testid="rule-editor-meta">
                {metaItems.map((item) => (
                  <span key={item} className={styles.metaItem}>
                    {item}
                  </span>
                ))}
              </div>
            )}
          </div>
          {saving && <Spin size="small" style={{ marginLeft: 8 }} />}
        </div>
        <Space size={8}>
          <Button
            size="small"
            icon={<CopyOutlined />}
            onClick={handleCopy}
            data-testid="rule-copy-button"
          >
            Copy
          </Button>
          {canEdit && (
            <Button
              type="primary"
              size="small"
              icon={<SaveOutlined />}
              onClick={handleSave}
              disabled={!hasChanges}
              loading={saving}
              data-testid="rule-save-button"
            >
              Save
            </Button>
          )}
          {canEdit && (
            <Button
              size="small"
              danger
              icon={<DeleteOutlined />}
              onClick={handleDelete}
              data-testid="rule-delete-button"
            >
              Delete
            </Button>
          )}
        </Space>
      </div>
      <div className={styles.editorContainer} ref={setContainerElement} data-testid="rule-editor-container" />
    </div>
  );
}
