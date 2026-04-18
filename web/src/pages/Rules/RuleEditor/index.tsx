import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { editor as MonacoEditor, KeyCode, KeyMod } from "monaco-editor";
import { Empty, Spin, message, Button, Space, Modal } from "antd";
import { SaveOutlined, CopyOutlined, DeleteOutlined } from "@ant-design/icons";
import dayjs from "dayjs";
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
  type DebouncedValidator,
  type ReferenceLocation,
} from "../../../components/BifrostEditor";
import { useRulesStore } from "../../../stores/useRulesStore";
import { useThemeStore } from "../../../stores/useThemeStore";
import { useValuesStore } from "../../../stores/useValuesStore";
import type { RuleSyncInfo } from "../../../types";
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

export default function RuleEditor() {
  const navigate = useNavigate();
  const {
    currentRule,
    selectedRuleName,
    editingContent,
    loading,
    saving,
    isGroupMode,
    groupWritable,
    setEditingContent,
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
  const currentRuleRef = useRef<{
    currentRule: typeof currentRule;
    selectedRuleName: typeof selectedRuleName;
    editingContent: typeof editingContent;
  } | null>(null);
  const [containerElement, setContainerElement] =
    useState<HTMLDivElement | null>(null);

  const handleValidationComplete = useCallback(
    (result: { defined_variables?: { name: string; defined_at?: number | null }[] }) => {
      const localVars = (result.defined_variables || [])
        .filter((v) => v.defined_at != null)
        .map((v) => ({ name: v.name, line: v.defined_at! }));
      setLocalVariables(localVars);
      localVariablesRef.current = localVars;
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
    currentRuleRef.current = { currentRule, selectedRuleName, editingContent };
  }, [currentRule, selectedRuleName, editingContent]);

  const handleChange = useCallback(() => {
    if (isSettingValueRef.current) return;
    if (!modelRef.current || modelRef.current.isDisposed()) return;
    const selectedName = currentRuleRef.current?.selectedRuleName;
    if (!selectedName) return;

    const content = modelRef.current.getValue();
    setEditingContent(selectedName, content);

    if (validatorRef.current && modelRef.current) {
      validatorRef.current.validate(modelRef.current, handleValidationComplete);
    }
  }, [setEditingContent, handleValidationComplete]);

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

    const getGlobalValues = () => {
      const result: Record<string, string> = {};
      for (const v of valuesRef.current) {
        result[v.name] = v.value;
      }
      return result;
    };

    const validator = createDebouncedValidator(500, getGlobalValues);
    validatorRef.current = validator;

    if (initialContent) {
      validator.validate(model, handleValidationComplete);
    }

    editorRef.current = ed;
    modelRef.current = model;

    return () => {
      validator.cancel();
      validatorRef.current = null;
      clearValidationMarkers(model);
      setLocalVariables([]);
      changeDisposable.dispose();
      model.dispose();
      ed.dispose();
      editorRef.current = null;
      modelRef.current = null;
    };
  }, [containerElement, handleChange, handleSave, resolvedTheme, handleValidationComplete, canEdit]);

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
      isSettingValueRef.current = true;
      modelRef.current.setValue("");
      isSettingValueRef.current = false;
      return;
    }

    const edited = editingContent[currentRule.name];
    const content = edited !== undefined ? edited : currentRule.content || "";
    const currentContent = modelRef.current.getValue();

    if (currentContent !== content) {
      isSettingValueRef.current = true;
      modelRef.current.setValue(content);
      isSettingValueRef.current = false;
      editorRef.current.setScrollTop(0);
      editorRef.current.setScrollLeft(0);

      if (validatorRef.current && modelRef.current) {
        validatorRef.current.validate(modelRef.current, handleValidationComplete);
      }
    }
  }, [currentRule, editingContent, containerElement, handleValidationComplete]);

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
    selectedRuleName && editingContent[selectedRuleName] !== undefined;
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
