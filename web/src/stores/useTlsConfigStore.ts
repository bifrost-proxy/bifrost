import { create } from "zustand";
import {
  getTlsConfig,
  updateTlsConfig,
  disconnectByApp,
  disconnectByDomain,
  type TlsConfig,
} from "../api/config";

interface TlsConfigState {
  config: TlsConfig | null;
  loading: boolean;
  error: string | null;
  applyConfigSnapshot: (config: TlsConfig) => void;
  fetchConfig: () => Promise<void>;
  addAppToIntercept: (app: string) => Promise<boolean>;
  removeAppFromIntercept: (app: string) => Promise<boolean>;
  addDomainToIntercept: (domain: string) => Promise<boolean>;
  removeDomainFromIntercept: (domain: string) => Promise<boolean>;
  addAppToPassthrough: (app: string) => Promise<boolean>;
  removeAppFromPassthrough: (app: string) => Promise<boolean>;
  addDomainToPassthrough: (domain: string) => Promise<boolean>;
  removeDomainFromPassthrough: (domain: string) => Promise<boolean>;
  isAppInIntercept: (app: string) => boolean;
  isAppInPassthrough: (app: string) => boolean;
  isDomainInIntercept: (domain: string) => boolean;
  isDomainInPassthrough: (domain: string) => boolean;
  addIpToIntercept: (ip: string) => Promise<boolean>;
  removeIpFromIntercept: (ip: string) => Promise<boolean>;
  addIpToPassthrough: (ip: string) => Promise<boolean>;
  removeIpFromPassthrough: (ip: string) => Promise<boolean>;
  isIpInIntercept: (ip: string) => boolean;
  isIpInPassthrough: (ip: string) => boolean;
}

export const useTlsConfigStore = create<TlsConfigState>((set, get) => ({
  config: null,
  loading: false,
  error: null,

  applyConfigSnapshot: (config: TlsConfig) => {
    set({ config, loading: false, error: null });
  },

  fetchConfig: async () => {
    set({ loading: true, error: null });
    try {
      const config = await getTlsConfig();
      set({ config, loading: false });
    } catch (e) {
      set({
        error: e instanceof Error ? e.message : "Failed to fetch TLS config",
        loading: false,
      });
    }
  },

  addAppToIntercept: async (app: string) => {
    const { config } = get();
    if (!config) return false;

    if (config.app_intercept_include.includes(app)) return true;

    const newList = [...config.app_intercept_include, app];
    const fromPassthrough = config.app_intercept_exclude.includes(app);
    const excludeList = fromPassthrough
      ? config.app_intercept_exclude.filter((p) => p !== app)
      : config.app_intercept_exclude;

    try {
      const updatedConfig = await updateTlsConfig({
        app_intercept_include: newList,
        app_intercept_exclude: excludeList,
      });
      set({ config: updatedConfig });

      await disconnectByApp(app);
      return true;
    } catch {
      return false;
    }
  },

  removeAppFromIntercept: async (app: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = config.app_intercept_include.filter((p) => p !== app);

    try {
      const updatedConfig = await updateTlsConfig({
        app_intercept_include: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  addDomainToIntercept: async (domain: string) => {
    const { config } = get();
    if (!config) return false;

    if (config.intercept_include.includes(domain)) return true;

    const newList = [...config.intercept_include, domain];
    const fromPassthrough = config.intercept_exclude.includes(domain);
    const excludeList = fromPassthrough
      ? config.intercept_exclude.filter((p) => p !== domain)
      : config.intercept_exclude;

    try {
      const updatedConfig = await updateTlsConfig({
        intercept_include: newList,
        intercept_exclude: excludeList,
      });
      set({ config: updatedConfig });

      await disconnectByDomain(domain);
      return true;
    } catch {
      return false;
    }
  },

  removeDomainFromIntercept: async (domain: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = config.intercept_include.filter((p) => p !== domain);

    try {
      const updatedConfig = await updateTlsConfig({
        intercept_include: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  addAppToPassthrough: async (app: string) => {
    const { config } = get();
    if (!config) return false;

    if (config.app_intercept_exclude.includes(app)) return true;

    const newList = [...config.app_intercept_exclude, app];
    const fromIntercept = config.app_intercept_include.includes(app);
    const includeList = fromIntercept
      ? config.app_intercept_include.filter((p) => p !== app)
      : config.app_intercept_include;

    try {
      const updatedConfig = await updateTlsConfig({
        app_intercept_exclude: newList,
        app_intercept_include: includeList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  removeAppFromPassthrough: async (app: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = config.app_intercept_exclude.filter((p) => p !== app);

    try {
      const updatedConfig = await updateTlsConfig({
        app_intercept_exclude: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  addDomainToPassthrough: async (domain: string) => {
    const { config } = get();
    if (!config) return false;

    if (config.intercept_exclude.includes(domain)) return true;

    const newList = [...config.intercept_exclude, domain];
    const fromIntercept = config.intercept_include.includes(domain);
    const includeList = fromIntercept
      ? config.intercept_include.filter((p) => p !== domain)
      : config.intercept_include;

    try {
      const updatedConfig = await updateTlsConfig({
        intercept_exclude: newList,
        intercept_include: includeList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  removeDomainFromPassthrough: async (domain: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = config.intercept_exclude.filter((p) => p !== domain);

    try {
      const updatedConfig = await updateTlsConfig({
        intercept_exclude: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  isAppInIntercept: (app: string) => {
    const { config } = get();
    return config?.app_intercept_include.includes(app) ?? false;
  },

  isAppInPassthrough: (app: string) => {
    const { config } = get();
    return config?.app_intercept_exclude.includes(app) ?? false;
  },

  isDomainInIntercept: (domain: string) => {
    const { config } = get();
    return config?.intercept_include.includes(domain) ?? false;
  },

  isDomainInPassthrough: (domain: string) => {
    const { config } = get();
    return config?.intercept_exclude.includes(domain) ?? false;
  },

  addIpToIntercept: async (ip: string) => {
    const { config } = get();
    if (!config) return false;

    if ((config.ip_intercept_include || []).includes(ip)) return true;

    const newList = [...(config.ip_intercept_include || []), ip];
    const fromPassthrough = (config.ip_intercept_exclude || []).includes(ip);
    const excludeList = fromPassthrough
      ? (config.ip_intercept_exclude || []).filter((p) => p !== ip)
      : (config.ip_intercept_exclude || []);

    try {
      const updatedConfig = await updateTlsConfig({
        ip_intercept_include: newList,
        ip_intercept_exclude: excludeList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  removeIpFromIntercept: async (ip: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = (config.ip_intercept_include || []).filter((p) => p !== ip);

    try {
      const updatedConfig = await updateTlsConfig({
        ip_intercept_include: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  addIpToPassthrough: async (ip: string) => {
    const { config } = get();
    if (!config) return false;

    if ((config.ip_intercept_exclude || []).includes(ip)) return true;

    const newList = [...(config.ip_intercept_exclude || []), ip];
    const fromIntercept = (config.ip_intercept_include || []).includes(ip);
    const includeList = fromIntercept
      ? (config.ip_intercept_include || []).filter((p) => p !== ip)
      : (config.ip_intercept_include || []);

    try {
      const updatedConfig = await updateTlsConfig({
        ip_intercept_exclude: newList,
        ip_intercept_include: includeList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  removeIpFromPassthrough: async (ip: string) => {
    const { config } = get();
    if (!config) return false;

    const newList = (config.ip_intercept_exclude || []).filter((p) => p !== ip);

    try {
      const updatedConfig = await updateTlsConfig({
        ip_intercept_exclude: newList,
      });
      set({ config: updatedConfig });
      return true;
    } catch {
      return false;
    }
  },

  isIpInIntercept: (ip: string) => {
    const { config } = get();
    return (config?.ip_intercept_include || []).includes(ip) ?? false;
  },

  isIpInPassthrough: (ip: string) => {
    const { config } = get();
    return (config?.ip_intercept_exclude || []).includes(ip) ?? false;
  },
}));
