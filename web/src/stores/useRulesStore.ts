import { create } from 'zustand';
import type { RuleFile, RuleFileDetail } from '../types';
import * as api from '../api';
import { isConnectionIssueError, isNotFoundError, normalizeApiErrorMessage } from '../api/client';
import { message } from 'antd';
import { clearDesktopDocumentEdited } from '../desktop/tauri';
import {
  fetchGroupRules,
  getGroupRule,
  createGroupRule,
  updateGroupRule,
  deleteGroupRule,
  enableGroupRule,
  disableGroupRule,
  type GroupRuleInfo,
} from '../api/group';

function isGlobalDefaultRule(
  rule: Pick<RuleFile, 'name' | 'is_global_default'>,
  rootRulesMode = true
): boolean {
  return rule.is_global_default === true || (rootRulesMode && rule.name === 'Default');
}

function sortRulesByManualOrder(rules: RuleFile[]): RuleFile[] {
  return [...rules].sort((left, right) => {
    const leftDefault = isGlobalDefaultRule(left);
    const rightDefault = isGlobalDefaultRule(right);
    if (leftDefault !== rightDefault) return leftDefault ? -1 : 1;
    return left.sort_order - right.sort_order || left.name.localeCompare(right.name);
  });
}

function groupRuleToRuleFile(info: GroupRuleInfo): RuleFile {
  return {
    name: info.name,
    enabled: info.enabled,
    sort_order: info.sort_order,
    rule_count: info.rule_count,
    created_at: info.created_at,
    updated_at: info.updated_at,
    is_system: false,
    is_global_default: false,
    can_delete: true,
    can_disable: true,
    can_rename: true,
    can_reorder: true,
    can_edit_content: true,
  };
}

function formatRuleMutationError(error: unknown, fallback: string): string {
  const base = normalizeApiErrorMessage(error, fallback);
  const data = (error as { response?: { data?: unknown } })?.response?.data;
  if (!data || typeof data !== 'object' || !('syntax' in data)) {
    return base;
  }
  const syntax = (data as {
    syntax?: {
      errors?: Array<{
        line?: number;
        start_column?: number;
        code?: string;
        message?: string;
        suggestion?: string;
      }>;
      guidance?: { summary?: string };
    };
  }).syntax;
  const first = syntax?.errors?.[0];
  if (!first) {
    return syntax?.guidance?.summary ? `${base}: ${syntax.guidance.summary}` : base;
  }
  const code = first.code ? `${first.code} ` : '';
  const line = first.line ? `line ${first.line}${first.start_column ? `:${first.start_column}` : ''}` : 'rule';
  const suggestion = first.suggestion ? ` Suggestion: ${first.suggestion}` : '';
  return `${base}: ${code}${line}: ${first.message || 'syntax validation failed'}.${suggestion}`;
}

interface RulesState {
  rules: RuleFile[];
  currentRule: RuleFileDetail | null;
  selectedRuleName: string | null;
  editingContent: Record<string, string>;
  savedContent: Record<string, string>;
  searchKeyword: string;
  loading: boolean;
  saving: boolean;
  error: string | null;
  activeGroupId: string | null;
  activeGroupName: string | null;
  isGroupMode: boolean;
  groupWritable: boolean;
  setActiveGroupId: (groupId: string | null) => void;
  fetchRules: () => Promise<void>;
  fetchRule: (name: string) => Promise<void>;
  selectRule: (name: string | null) => Promise<void>;
  createRule: (name: string, content: string) => Promise<boolean>;
  updateRule: (name: string, content?: string, enabled?: boolean) => Promise<boolean>;
  saveCurrentRule: () => Promise<boolean>;
  deleteRule: (name: string) => Promise<boolean>;
  toggleRule: (name: string, enabled: boolean) => Promise<boolean>;
  renameRule: (oldName: string, newName: string) => Promise<boolean>;
  reorderRules: (order: string[]) => Promise<boolean>;
  setEditingContent: (name: string, content: string) => void;
  clearEditingContent: (name: string) => void;
  setSearchKeyword: (keyword: string) => void;
  hasUnsavedChanges: (name: string) => boolean;
  clearError: () => void;
}

export const useRulesStore = create<RulesState>((set, get) => ({
  rules: [],
  currentRule: null,
  selectedRuleName: null,
  editingContent: {},
  savedContent: {},
  searchKeyword: '',
  loading: false,
  saving: false,
  error: null,
  activeGroupId: null,
  activeGroupName: null,
  isGroupMode: false,
  groupWritable: false,

  setActiveGroupId: (groupId: string | null) => {
    set({
      activeGroupId: groupId,
      activeGroupName: null,
      isGroupMode: groupId !== null,
      groupWritable: false,
      rules: [],
      selectedRuleName: null,
      currentRule: null,
      editingContent: {},
      savedContent: {},
    });
  },

  fetchRules: async () => {
    const { activeGroupId } = get();
    set({ loading: true, error: null });

    if (!activeGroupId) {
      try {
        const rules = await api.getRules();
        set({ rules: sortRulesByManualOrder(rules), loading: false, isGroupMode: false, activeGroupName: null, groupWritable: false });
      } catch (e) {
        set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      }
      return;
    }

    try {
      const resp = await fetchGroupRules(activeGroupId);
      const ruleFiles = resp.rules.map(groupRuleToRuleFile);
      set({
        rules: ruleFiles,
        loading: false,
        isGroupMode: true,
        activeGroupName: resp.group_name,
        groupWritable: resp.writable,
      });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
    }
  },

  fetchRule: async (name: string) => {
    const { isGroupMode, activeGroupId } = get();
    set({ loading: true, error: null });

    if (isGroupMode && activeGroupId) {
      try {
        const detail = await getGroupRule(activeGroupId, name);
        set((state) => ({
          currentRule: {
            name: detail.name,
            content: detail.content,
            enabled: detail.enabled,
            sort_order: detail.sort_order,
            created_at: detail.created_at,
            updated_at: detail.updated_at,
            sync: detail.sync,
          },
          savedContent: { ...state.savedContent, [name]: detail.content },
          loading: false,
        }));
      } catch (e) {
        if (isNotFoundError(e)) {
          message.warning(`Rule "${name}" no longer exists, refreshing list`);
          await get().fetchRules();
          const rules = get().rules;
          const nextRule = rules.length > 0 ? rules[0].name : null;
          set({ selectedRuleName: nextRule, currentRule: null, loading: false });
          if (nextRule) await get().selectRule(nextRule);
          return;
        }
        set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      }
      return;
    }

    try {
      const rule = await api.getRule(name);
      set((state) => ({
        currentRule: rule,
        savedContent: { ...state.savedContent, [name]: rule.content },
        loading: false,
      }));
    } catch (e) {
      if (isNotFoundError(e)) {
        message.warning(`Rule "${name}" no longer exists, refreshing list`);
        await get().fetchRules();
        const rules = get().rules;
        const nextRule = rules.length > 0 ? rules[0].name : null;
        set({ selectedRuleName: nextRule, currentRule: null, loading: false });
        if (nextRule) await get().selectRule(nextRule);
        return;
      }
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
    }
  },

  selectRule: async (name: string | null) => {
    if (!name) {
      set({ selectedRuleName: null, currentRule: null });
      return;
    }
    const { isGroupMode, activeGroupId } = get();
    set({ selectedRuleName: name, loading: true, error: null });

    if (isGroupMode && activeGroupId) {
      try {
        const detail = await getGroupRule(activeGroupId, name);
        set((state) => ({
          currentRule: {
            name: detail.name,
            content: detail.content,
            enabled: detail.enabled,
            sort_order: detail.sort_order,
            created_at: detail.created_at,
            updated_at: detail.updated_at,
            sync: detail.sync,
          },
          savedContent: { ...state.savedContent, [name]: detail.content },
          loading: false,
        }));
      } catch (e) {
        if (isNotFoundError(e)) {
          message.warning(`Rule "${name}" no longer exists, refreshing list`);
          await get().fetchRules();
          const rules = get().rules;
          const nextRule = rules.length > 0 ? rules[0].name : null;
          set({ selectedRuleName: nextRule, currentRule: null, loading: false });
          if (nextRule) await get().selectRule(nextRule);
          return;
        }
        set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      }
      return;
    }

    try {
      const rule = await api.getRule(name);
      set((state) => ({
        currentRule: rule,
        savedContent: { ...state.savedContent, [name]: rule.content },
        loading: false,
      }));
    } catch (e) {
      if (isNotFoundError(e)) {
        message.warning(`Rule "${name}" no longer exists, refreshing list`);
        await get().fetchRules();
        const rules = get().rules;
        const nextRule = rules.length > 0 ? rules[0].name : null;
        set({ selectedRuleName: nextRule, currentRule: null, loading: false });
        if (nextRule) await get().selectRule(nextRule);
        return;
      }
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
    }
  },

  createRule: async (name: string, content: string) => {
    const { isGroupMode, groupWritable, activeGroupId } = get();

    if (isGroupMode) {
      if (!groupWritable || !activeGroupId) return false;
      set({ loading: true, error: null });
      try {
        const detail = await createGroupRule(activeGroupId, name, content);
        await get().fetchRules();
        set({ selectedRuleName: detail.name });
        await get().selectRule(detail.name);
        return true;
      } catch (e) {
        set({
          error: isConnectionIssueError(e) ? null : formatRuleMutationError(e, 'Failed to create rule'),
          loading: false,
        });
        return false;
      }
    }

    if (isGlobalDefaultRule({ name })) {
      message.warning('Rule name "Default" is reserved for the global default rule');
      return false;
    }

    set({ loading: true, error: null });
    try {
      await api.createRule(name, content);
      await get().fetchRules();
      set({ selectedRuleName: name });
      await get().selectRule(name);
      return true;
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : formatRuleMutationError(e, 'Failed to create rule'),
        loading: false,
      });
      return false;
    }
  },

  updateRule: async (name: string, content?: string, enabled?: boolean) => {
    set({ loading: true, error: null });
    try {
      await api.updateRule(name, content, enabled);
      await get().fetchRules();
      if (get().currentRule?.name === name) {
        await get().fetchRule(name);
      }
      return true;
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : formatRuleMutationError(e, 'Failed to update rule'),
        loading: false,
      });
      return false;
    }
  },

  saveCurrentRule: async () => {
    const { selectedRuleName, editingContent, currentRule, isGroupMode, groupWritable, activeGroupId } = get();
    if (!selectedRuleName) return false;

    const content = editingContent[selectedRuleName];
    if (content === undefined || content === currentRule?.content) {
      if (content !== undefined) {
        set((state) => {
          const ec = { ...state.editingContent };
          delete ec[selectedRuleName];
          return { editingContent: ec };
        });
        await clearDesktopDocumentEdited().catch(() => undefined);
      }
      return true;
    }

    set({ saving: true, error: null });

    if (isGroupMode) {
      if (!groupWritable || !activeGroupId) {
        set({ saving: false });
        return false;
      }
      try {
        const detail = await updateGroupRule(activeGroupId, selectedRuleName, content);
        const newEditingContent = { ...get().editingContent };
        delete newEditingContent[selectedRuleName];
        set((state) => ({
          currentRule: {
            name: detail.name,
            content: detail.content,
            enabled: detail.enabled,
            sort_order: detail.sort_order,
            created_at: detail.created_at,
            updated_at: detail.updated_at,
            sync: detail.sync,
          },
          saving: false,
          editingContent: newEditingContent,
          savedContent: { ...state.savedContent, [selectedRuleName]: detail.content },
        }));
        await clearDesktopDocumentEdited().catch(() => undefined);
        await get().fetchRules();
        return true;
      } catch (e) {
        set({
          error: isConnectionIssueError(e) ? null : formatRuleMutationError(e, 'Failed to save rule'),
          saving: false,
        });
        return false;
      }
    }

    try {
      await api.updateRule(selectedRuleName, content);

      // Optimistic update: clear dirty state immediately after save succeeds.
      // This prevents the yellow dot from lingering due to async timing issues
      // in WKWebView where onDidChangeContent may fire after isSettingValueRef is cleared.
      const ec = { ...get().editingContent };
      delete ec[selectedRuleName];
      const cr = get().currentRule;
      set((state) => ({
        editingContent: ec,
        currentRule: cr ? { ...cr, content } : null,
        savedContent: { ...state.savedContent, [selectedRuleName]: content },
      }));
      await clearDesktopDocumentEdited().catch(() => undefined);

      // Refresh canonical data from server (best-effort after successful save)
      try {
        await get().fetchRules();
        const rule = await api.getRule(selectedRuleName);
        set((state) => ({
          editingContent: Object.fromEntries(
            Object.entries(state.editingContent).filter(([name]) => name !== selectedRuleName),
          ),
          currentRule: rule,
          saving: false,
          savedContent: { ...state.savedContent, [selectedRuleName]: rule.content },
        }));
      } catch {
        set({ saving: false });
      }
      return true;
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : formatRuleMutationError(e, 'Failed to save rule'),
        saving: false,
      });
      return false;
    }
  },

  deleteRule: async (name: string) => {
    const { isGroupMode, groupWritable, activeGroupId } = get();
    const rule = get().rules.find((item) => item.name === name);
    if (!isGroupMode && (isGlobalDefaultRule(rule ?? { name }) || rule?.can_delete === false)) {
      message.warning('Default rule cannot be deleted');
      return false;
    }

    if (isGroupMode) {
      if (!groupWritable || !activeGroupId) return false;
      set({ loading: true, error: null });
      try {
        await deleteGroupRule(activeGroupId, name);
        await get().fetchRules();
        const { selectedRuleName } = get();
        if (selectedRuleName === name) {
          const rules = get().rules;
          const nextRule = rules.length > 0 ? rules[0].name : null;
          set({ selectedRuleName: nextRule, currentRule: null });
          if (nextRule) await get().selectRule(nextRule);
        }
        return true;
      } catch (e) {
        const msg = normalizeApiErrorMessage(e, 'Failed to delete rule');
        if (!isConnectionIssueError(e)) {
          message.error(msg);
        }
        set({ error: isConnectionIssueError(e) ? null : msg, loading: false });
        return false;
      }
    }

    set({ loading: true, error: null });
    try {
      await api.deleteRule(name);
      const { selectedRuleName, editingContent, savedContent } = get();
      const newEditingContent = { ...editingContent };
      delete newEditingContent[name];
      const newSavedContent = { ...savedContent };
      delete newSavedContent[name];

      if (selectedRuleName === name) {
        await get().fetchRules();
        const rules = get().rules;
        const nextRule = rules.length > 0 ? rules[0].name : null;
        set({
          selectedRuleName: nextRule,
          currentRule: null,
          editingContent: newEditingContent,
          savedContent: newSavedContent,
        });
        if (nextRule) {
          await get().selectRule(nextRule);
        }
      } else {
        await get().fetchRules();
        set({ editingContent: newEditingContent, savedContent: newSavedContent });
      }
      return true;
    } catch (e) {
      const msg = normalizeApiErrorMessage(e, 'Failed to delete rule');
      if (!isConnectionIssueError(e)) {
        message.error(msg);
      }
      set({ error: isConnectionIssueError(e) ? null : msg, loading: false });
      return false;
    }
  },

  toggleRule: async (name: string, enabled: boolean) => {
    const { isGroupMode, activeGroupId } = get();
    const currentRule = get().rules.find((item) => item.name === name);
    if (
      !isGroupMode &&
      !enabled &&
      (isGlobalDefaultRule(currentRule ?? { name }) || currentRule?.can_disable === false)
    ) {
      message.warning('Default rule is always enabled');
      return false;
    }
    const previousRules = get().rules;
    set({
      rules: previousRules.map((r) =>
        r.name === name ? { ...r, enabled } : r
      ),
    });
    try {
      if (isGroupMode && activeGroupId) {
        if (enabled) {
          await enableGroupRule(activeGroupId, name);
        } else {
          await disableGroupRule(activeGroupId, name);
        }
        const resp = await fetchGroupRules(activeGroupId);
        set({ rules: resp.rules.map(groupRuleToRuleFile), activeGroupName: resp.group_name });
        if (get().currentRule?.name === name) {
          const detail = await getGroupRule(activeGroupId, name);
          set({
            currentRule: {
              name: detail.name,
              content: detail.content,
              enabled: detail.enabled,
              sort_order: detail.sort_order,
              created_at: detail.created_at,
              updated_at: detail.updated_at,
              sync: detail.sync,
            },
          });
        }
      } else {
        if (enabled) {
          await api.enableRule(name);
        } else {
          await api.disableRule(name);
        }
        const rules = await api.getRules();
        set({ rules: sortRulesByManualOrder(rules) });
        if (get().currentRule?.name === name) {
          const rule = await api.getRule(name);
          set({ currentRule: rule });
        }
      }
      return true;
    } catch (e) {
      if (isNotFoundError(e)) {
        message.warning(`Rule "${name}" no longer exists, refreshing list`);
        await get().fetchRules();
        const rules = get().rules;
        const nextRule = rules.length > 0 ? rules[0].name : null;
        set({ selectedRuleName: nextRule, currentRule: null });
        if (nextRule) await get().selectRule(nextRule);
        return false;
      }
      set({ rules: previousRules, error: isConnectionIssueError(e) ? null : (e as Error).message });
      return false;
    }
  },

  renameRule: async (oldName: string, newName: string) => {
    const { isGroupMode } = get();
    const rule = get().rules.find((item) => item.name === oldName);
    if (!isGroupMode && (isGlobalDefaultRule(rule ?? { name: oldName }) || rule?.can_rename === false)) {
      message.warning('Default rule cannot be renamed');
      return false;
    }
    if (!isGroupMode && isGlobalDefaultRule({ name: newName })) {
      message.warning('Rule name "Default" is reserved for the global default rule');
      return false;
    }
    set({ loading: true, error: null });
    try {
      await api.renameRule(oldName, newName);
      const { selectedRuleName, editingContent } = get();
      const newEditingContent = { ...editingContent };
      if (newEditingContent[oldName] !== undefined) {
        newEditingContent[newName] = newEditingContent[oldName];
        delete newEditingContent[oldName];
      }
      const newSavedContent = { ...get().savedContent };
      if (newSavedContent[oldName] !== undefined) {
        newSavedContent[newName] = newSavedContent[oldName];
        delete newSavedContent[oldName];
      }
      await get().fetchRules();
      if (selectedRuleName === oldName) {
        set({
          selectedRuleName: newName,
          editingContent: newEditingContent,
          savedContent: newSavedContent,
        });
        await get().selectRule(newName);
      } else {
        set({ editingContent: newEditingContent, savedContent: newSavedContent });
      }
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  reorderRules: async (order: string[]) => {
    set({ loading: true, error: null });
    try {
      await api.reorderRules(order);
      await get().fetchRules();
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  setEditingContent: (name: string, content: string) => {
    set((state) => ({
      editingContent: {
        ...state.editingContent,
        [name]: content,
      },
    }));
  },

  clearEditingContent: (name: string) => {
    set((state) => {
      if (state.editingContent[name] === undefined) {
        return state;
      }
      const editingContent = { ...state.editingContent };
      delete editingContent[name];
      return { editingContent };
    });
  },

  setSearchKeyword: (keyword: string) => {
    set({ searchKeyword: keyword });
  },

  hasUnsavedChanges: (name: string) => {
    const { editingContent, savedContent } = get();
    const edited = editingContent[name];
    if (edited === undefined) return false;
    const saved = savedContent[name];
    if (saved === undefined) return false;
    return edited !== saved;
  },

  clearError: () => set({ error: null }),
}));
