import { create } from "zustand";
import { persist } from "zustand/middleware";

interface RuleTreeState {
  collapsedFoldersByGroup: Record<string, string[]>;
  toggleFolder: (groupId: string | null, path: string) => void;
}

export const useRuleTreeStore = create<RuleTreeState>()(
  persist(
    (set) => ({
      collapsedFoldersByGroup: {},
      toggleFolder: (groupId, path) =>
        set((state) => {
          // Encode null separately from every group ID
          const scope = JSON.stringify(groupId);
          const collapsed = state.collapsedFoldersByGroup[scope] ?? [];
          return {
            collapsedFoldersByGroup: {
              ...state.collapsedFoldersByGroup,
              [scope]: collapsed.includes(path)
                ? collapsed.filter((folder) => folder !== path)
                : [...collapsed, path],
            },
          };
        }),
    }),
    {
      name: "bifrost-rule-tree",
      partialize: (state) => ({
        collapsedFoldersByGroup: state.collapsedFoldersByGroup,
      }),
    },
  ),
);
