import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  getUiConfig,
  updateUiConfig,
  type FilterType,
  type PinnedFilter,
  type CollapsedSections,
  type FilterPanelConfig,
} from "../api/ui";
import { isConnectionIssueError } from "../api/client";

export type { FilterType, PinnedFilter };

interface FilterPanelState {
  pinnedFilters: PinnedFilter[];
  selectedClientIps: string[];
  selectedProxyPorts: string[];
  selectedClientApps: string[];
  selectedDomains: string[];
  panelCollapsed: boolean;
  panelWidth: number;
  collapsedSections: CollapsedSections;
  detailPanelCollapsed: boolean;
  loading: boolean;
  initialized: boolean;
  searchKeyword: string;

  addPinnedFilter: (filter: Omit<PinnedFilter, "id">) => void;
  removePinnedFilter: (id: string) => void;
  togglePinnedFilter: (id: string) => void;
  setSelectedClientIps: (ips: string[]) => void;
  setSelectedProxyPorts: (ports: string[]) => void;
  setSelectedClientApps: (apps: string[]) => void;
  setSelectedDomains: (domains: string[]) => void;
  toggleClientIp: (ip: string) => void;
  toggleProxyPort: (port: string) => void;
  toggleClientApp: (app: string) => void;
  toggleDomain: (domain: string) => void;
  clearAllSelections: () => void;
  setPanelCollapsed: (collapsed: boolean) => void;
  setPanelWidth: (width: number) => void;
  setCollapsedSection: (
    section: keyof CollapsedSections,
    collapsed: boolean
  ) => void;
  setDetailPanelCollapsed: (collapsed: boolean) => void;
  setSearchKeyword: (keyword: string) => void;
  loadFromServer: () => Promise<void>;
  saveToServer: () => Promise<void>;
}

const generateId = () =>
  `filter_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;

const defaultCollapsedSections: CollapsedSections = {
  pinned: false,
  clientIp: false,
  proxyPort: false,
  clientApp: false,
  domain: false,
};

export const useFilterPanelStore = create<FilterPanelState>()(
  persist(
    (set, get) => ({
      pinnedFilters: [],
      selectedClientIps: [],
      selectedProxyPorts: [],
      selectedClientApps: [],
      selectedDomains: [],
      panelCollapsed: false,
      panelWidth: 220,
      collapsedSections: defaultCollapsedSections,
      detailPanelCollapsed: false,
      loading: false,
      initialized: false,
      searchKeyword: "",

      addPinnedFilter: (filter) => {
        const state = get();
        const exists = state.pinnedFilters.some(
          (f) => f.type === filter.type && f.value === filter.value,
        );
        if (exists) return;

        const newFilter: PinnedFilter = {
          ...filter,
          id: generateId(),
        };
        const newFilters = [...state.pinnedFilters, newFilter];
        set({ pinnedFilters: newFilters });

        updateUiConfig({ pinnedFilters: newFilters }).catch((err) => {
          if (!isConnectionIssueError(err)) {
            console.error("Failed to persist pinned filter:", err);
          }
        });
      },

      removePinnedFilter: (id) => {
        const state = get();
        const newFilters = state.pinnedFilters.filter((f) => f.id !== id);
        set({ pinnedFilters: newFilters });

        updateUiConfig({ pinnedFilters: newFilters }).catch((err) => {
          if (!isConnectionIssueError(err)) {
            console.error("Failed to persist pinned filter removal:", err);
          }
        });
      },

      togglePinnedFilter: (id) => {
        const state = get();
        const filter = state.pinnedFilters.find((f) => f.id === id);
        if (!filter) return;

        switch (filter.type) {
          case "client_ip":
            get().toggleClientIp(filter.value);
            break;
          case "proxy_port":
            get().toggleProxyPort(filter.value);
            break;
          case "client_app":
            get().toggleClientApp(filter.value);
            break;
          case "domain":
            get().toggleDomain(filter.value);
            break;
        }
      },

      setSelectedClientIps: (ips) => set({ selectedClientIps: ips }),
      setSelectedProxyPorts: (ports) => set({ selectedProxyPorts: ports }),
      setSelectedClientApps: (apps) => set({ selectedClientApps: apps }),
      setSelectedDomains: (domains) => set({ selectedDomains: domains }),

      toggleClientIp: (ip) => {
        const state = get();
        const ips = state.selectedClientIps.includes(ip)
          ? state.selectedClientIps.filter((i) => i !== ip)
          : [...state.selectedClientIps, ip];
        set({ selectedClientIps: ips });
      },

      toggleProxyPort: (port) => {
        const state = get();
        const ports = state.selectedProxyPorts.includes(port)
          ? state.selectedProxyPorts.filter((item) => item !== port)
          : [...state.selectedProxyPorts, port];
        set({ selectedProxyPorts: ports });
      },

      toggleClientApp: (app) => {
        const state = get();
        const apps = state.selectedClientApps.includes(app)
          ? state.selectedClientApps.filter((a) => a !== app)
          : [...state.selectedClientApps, app];
        set({ selectedClientApps: apps });
      },

      toggleDomain: (domain) => {
        const state = get();
        const domains = state.selectedDomains.includes(domain)
          ? state.selectedDomains.filter((d) => d !== domain)
          : [...state.selectedDomains, domain];
        set({ selectedDomains: domains });
      },

      clearAllSelections: () => {
        set({
          selectedClientIps: [],
          selectedProxyPorts: [],
          selectedClientApps: [],
          selectedDomains: [],
        });
      },

      setPanelCollapsed: (collapsed) => {
        set({ panelCollapsed: collapsed });
        get().saveToServer();
      },

      setPanelWidth: (width) => {
        set({ panelWidth: width });
        get().saveToServer();
      },

      setCollapsedSection: (section, collapsed) => {
        const state = get();
        set({
          collapsedSections: {
            ...state.collapsedSections,
            [section]: collapsed,
          },
        });
        get().saveToServer();
      },

      setDetailPanelCollapsed: (collapsed) => {
        set({ detailPanelCollapsed: collapsed });
        updateUiConfig({ detailPanelCollapsed: collapsed }).catch((err) => {
          if (!isConnectionIssueError(err)) {
            console.error("Failed to save detail panel collapsed state:", err);
          }
        });
      },

      setSearchKeyword: (keyword) => set({ searchKeyword: keyword }),

      loadFromServer: async () => {
        const state = get();
        if (state.loading || state.initialized) return;

        set({ loading: true });
        try {
          const config = await getUiConfig();
          set({
            pinnedFilters: config.pinnedFilters ?? [],
            panelCollapsed: config.filterPanel?.collapsed ?? false,
            panelWidth: config.filterPanel?.width ?? 220,
            collapsedSections:
              config.filterPanel?.collapsedSections ?? defaultCollapsedSections,
            detailPanelCollapsed: config.detailPanelCollapsed ?? false,
            initialized: true,
          });
        } catch (err) {
          if (!isConnectionIssueError(err)) {
            console.error("Failed to load UI config:", err);
          }
          set({ initialized: true });
        } finally {
          set({ loading: false });
        }
      },

      saveToServer: async () => {
        const state = get();
        const filterPanel: FilterPanelConfig = {
          collapsed: state.panelCollapsed,
          width: state.panelWidth,
          collapsedSections: state.collapsedSections,
        };

        try {
          await updateUiConfig({ filterPanel });
        } catch (err) {
          if (!isConnectionIssueError(err)) {
            console.error("Failed to save UI config:", err);
          }
        }
      },
    }),
    {
      name: "bifrost-filter-panel-ui",
      partialize: (state) => ({
        selectedClientIps: state.selectedClientIps,
        selectedProxyPorts: state.selectedProxyPorts,
        selectedClientApps: state.selectedClientApps,
        selectedDomains: state.selectedDomains,
        searchKeyword: state.searchKeyword,
      }),
      version: 1,
    },
  ),
);

export const isFilterSelected = (
  state: FilterPanelState,
  type: FilterType,
  value: string
): boolean => {
  switch (type) {
    case "client_ip":
      return state.selectedClientIps.includes(value);
    case "proxy_port":
      return state.selectedProxyPorts.includes(value);
    case "client_app":
      return state.selectedClientApps.includes(value);
    case "domain":
      return state.selectedDomains.includes(value);
    default:
      return false;
  }
};

export const isPinned = (
  state: FilterPanelState,
  type: FilterType,
  value: string
): boolean => {
  return state.pinnedFilters.some(
    (f) => f.type === type && f.value === value
  );
};
