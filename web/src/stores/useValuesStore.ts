import { create } from 'zustand';
import type { ValueItem } from '../api/values';
import * as api from '../api';
import { isConnectionIssueError } from '../api/client';
import { clearDesktopDocumentEdited } from '../desktop/tauri';

const VALUES_SYNC_CHANNEL = 'bifrost-values-sync';
const valuesSyncChannel =
  typeof BroadcastChannel !== 'undefined'
    ? new BroadcastChannel(VALUES_SYNC_CHANNEL)
    : null;

function sortValuesByCreatedAt(values: ValueItem[]): ValueItem[] {
  return [...values].sort((left, right) => {
    const leftTime = Date.parse(left.created_at);
    const rightTime = Date.parse(right.created_at);
    return rightTime - leftTime || left.name.localeCompare(right.name);
  });
}

function broadcastValuesSnapshot(values: ValueItem[]): void {
  valuesSyncChannel?.postMessage({ type: 'snapshot', values });
}

interface ValuesState {
  values: ValueItem[];
  currentValue: ValueItem | null;
  selectedValueName: string | null;
  editingContent: Record<string, string>;
  searchKeyword: string;
  loading: boolean;
  saving: boolean;
  error: string | null;
  applyValuesSnapshot: (values: ValueItem[]) => void;
  fetchValues: () => Promise<void>;
  selectValue: (name: string | null) => void;
  createValue: (name: string, value: string) => Promise<boolean>;
  updateValue: (name: string, value: string) => Promise<boolean>;
  saveCurrentValue: () => Promise<boolean>;
  deleteValue: (name: string) => Promise<boolean>;
  renameValue: (oldName: string, newName: string) => Promise<boolean>;
  setEditingContent: (name: string, content: string) => void;
  setSearchKeyword: (keyword: string) => void;
  hasUnsavedChanges: (name: string) => boolean;
  clearError: () => void;
}

export const useValuesStore = create<ValuesState>((set, get) => ({
  values: [],
  currentValue: null,
  selectedValueName: null,
  editingContent: {},
  searchKeyword: '',
  loading: false,
  saving: false,
  error: null,

  applyValuesSnapshot: (values) => {
    const sortedValues = sortValuesByCreatedAt(values);
    const { selectedValueName } = get();
    const currentValue = selectedValueName
      ? sortedValues.find((item) => item.name === selectedValueName) ?? null
      : null;
    set({ values: sortedValues, currentValue, loading: false, error: null });
  },

  fetchValues: async () => {
    set({ loading: true, error: null });
    try {
      const response = await api.getValues();
      set({ values: sortValuesByCreatedAt(response.values), loading: false });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
    }
  },

  selectValue: (name: string | null) => {
    if (!name) {
      set({ selectedValueName: null, currentValue: null });
      return;
    }
    const { values } = get();
    const value = values.find((v) => v.name === name);
    set({ selectedValueName: name, currentValue: value || null });
  },

  createValue: async (name: string, value: string) => {
    set({ loading: true, error: null });
    try {
      await api.createValue(name, value);
      set((state) => {
        const now = new Date().toISOString();
        const values = sortValuesByCreatedAt([
          ...state.values,
          {
            name,
            value,
            created_at: now,
            updated_at: now,
          },
        ]);
        broadcastValuesSnapshot(values);
        return {
          values,
          selectedValueName: name,
          currentValue: values.find((item) => item.name === name) ?? null,
          loading: false,
        };
      });
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  updateValue: async (name: string, value: string) => {
    set({ loading: true, error: null });
    try {
      await api.updateValue(name, value);
      set((state) => {
        const updatedAt = new Date().toISOString();
        const values = state.values.map((item) =>
          item.name === name ? { ...item, value, updated_at: updatedAt } : item,
        );
        const sortedValues = sortValuesByCreatedAt(values);
        broadcastValuesSnapshot(sortedValues);
        return {
          values: sortedValues,
          currentValue:
            state.currentValue?.name === name
              ? sortedValues.find((item) => item.name === name) ?? null
              : state.currentValue,
          loading: false,
        };
      });
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  saveCurrentValue: async () => {
    const { selectedValueName, editingContent, currentValue } = get();
    if (!selectedValueName) return false;

    const content = editingContent[selectedValueName];
    if (content === undefined || content === currentValue?.value) {
      if (content !== undefined) {
        const newEditingContent = { ...get().editingContent };
        delete newEditingContent[selectedValueName];
        set({ editingContent: newEditingContent });
        await clearDesktopDocumentEdited().catch(() => undefined);
      }
      return true;
    }

    set({ saving: true, error: null });
    try {
      await api.updateValue(selectedValueName, content);
      const newEditingContent = { ...get().editingContent };
      delete newEditingContent[selectedValueName];
      set((state) => {
        const updatedAt = new Date().toISOString();
        const values = state.values.map((item) =>
          item.name === selectedValueName
            ? { ...item, value: content, updated_at: updatedAt }
            : item,
        );
        const sortedValues = sortValuesByCreatedAt(values);
        broadcastValuesSnapshot(sortedValues);
        return {
          values: sortedValues,
          currentValue: sortedValues.find((item) => item.name === selectedValueName) ?? null,
          editingContent: newEditingContent,
          saving: false,
        };
      });
      await clearDesktopDocumentEdited().catch(() => undefined);
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, saving: false });
      return false;
    }
  },

  deleteValue: async (name: string) => {
    set({ loading: true, error: null });
    try {
      await api.deleteValue(name);
      const { selectedValueName, editingContent } = get();
      const newEditingContent = { ...editingContent };
      delete newEditingContent[name];
      const values = get().values.filter((item) => item.name !== name);
      broadcastValuesSnapshot(values);

      if (selectedValueName === name) {
        const nextValue = values.length > 0 ? values[0].name : null;
        set({
          values,
          selectedValueName: nextValue,
          currentValue: nextValue
            ? values.find((item) => item.name === nextValue) ?? null
            : null,
          editingContent: newEditingContent,
          loading: false,
        });
      } else {
        set({
          values,
          editingContent: newEditingContent,
          loading: false,
        });
      }
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  renameValue: async (oldName: string, newName: string) => {
    set({ loading: true, error: null });
    try {
      const { values, editingContent } = get();
      const oldValue = values.find((v) => v.name === oldName);
      if (!oldValue) {
        set({ error: 'Value not found', loading: false });
        return false;
      }

      const content = editingContent[oldName] ?? oldValue.value;
      await api.deleteValue(oldName);
      await api.createValue(newName, content);

      const { selectedValueName } = get();
      const newEditingContent = { ...editingContent };
      if (newEditingContent[oldName] !== undefined) {
        newEditingContent[newName] = newEditingContent[oldName];
        delete newEditingContent[oldName];
      }

      const nextValues = values
        .filter((item) => item.name !== oldName)
        .concat({
          name: newName,
          value: content,
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        });
      const sortedValues = sortValuesByCreatedAt(nextValues);
      broadcastValuesSnapshot(sortedValues);
      if (selectedValueName === oldName) {
        set({
          values: sortedValues,
          selectedValueName: newName,
          currentValue: sortedValues.find((item) => item.name === newName) ?? null,
          editingContent: newEditingContent,
          loading: false,
        });
      } else {
        set({
          values: sortedValues,
          editingContent: newEditingContent,
          loading: false,
        });
      }
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

  setSearchKeyword: (keyword: string) => {
    set({ searchKeyword: keyword });
  },

  hasUnsavedChanges: (name: string) => {
    const { editingContent, values } = get();
    const edited = editingContent[name];
    if (edited === undefined) return false;
    const value = values.find((v) => v.name === name);
    if (!value) return false;
    return edited !== value.value;
  },

  clearError: () => set({ error: null }),
}));

valuesSyncChannel?.addEventListener('message', (event: MessageEvent) => {
  const data = event.data as { type?: string; values?: ValueItem[] };
  if (data?.type !== 'snapshot' || !Array.isArray(data.values)) {
    return;
  }

  useValuesStore.getState().applyValuesSnapshot(data.values);
});
